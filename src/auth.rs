use std::time::Duration;

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts},
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: i64,
    iat: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}

#[derive(Clone)]
pub struct JwtKeys {
    encoding: EncodingKey,
    decoding: DecodingKey,
    write_scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    pub subject: String,
    pub scopes: Vec<String>,
}

impl JwtKeys {
    pub fn new(secret: &str, write_scope: &str) -> Self {
        Self {
            encoding: EncodingKey::from_secret(secret.as_bytes()),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
            write_scope: write_scope.to_string(),
        }
    }

    pub fn write_scope(&self) -> &str {
        &self.write_scope
    }

    pub fn issue(&self, sub: &str, scopes: &[&str], ttl: Duration) -> String {
        self.issue_at(sub, scopes, Utc::now(), ttl)
    }

    pub fn issue_at(&self, sub: &str, scopes: &[&str], issued: DateTime<Utc>, ttl: Duration) -> String {
        let claims = Claims {
            sub: sub.to_string(),
            iat: issued.timestamp(),
            exp: issued.timestamp() + ttl.as_secs() as i64,
            scope: if scopes.is_empty() {
                None
            } else {
                Some(scopes.join(" "))
            },
        };
        encode(&Header::new(Algorithm::HS256), &claims, &self.encoding).expect("HS256 encode")
    }

    pub fn verify(&self, token: &str) -> Result<AuthContext, AppError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_required_spec_claims(&["exp", "sub"]);
        let data = decode::<Claims>(token, &self.decoding, &validation).map_err(|e| {
            // Keep the reason server-side. The response detail stays generic so a
            // caller cannot distinguish an expired token from a forged one.
            tracing::debug!(error = ?e, "rejected bearer token");
            AppError::Unauthorized("invalid token".to_string())
        })?;
        let scopes = data
            .claims
            .scope
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        Ok(AuthContext {
            subject: data.claims.sub,
            scopes,
        })
    }
}

/// Extracts the token from `Authorization: <scheme> <token>`, matching the scheme
/// case-insensitively as RFC 7235 requires (so `bearer`, `Bearer`, `BEARER` all work).
fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, token) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    Some(token.trim())
}

/// Extractor: valid bearer JWT holding the write scope.
pub struct WriteAuth(pub AuthContext);

impl<S> FromRequestParts<S> for WriteAuth
where
    S: Send + Sync,
    JwtKeys: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let keys = JwtKeys::from_ref(state);
        let header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::Unauthorized("missing Authorization header".into()))?;
        let token =
            bearer_token(header).ok_or_else(|| AppError::Unauthorized("expected Bearer token".into()))?;
        let ctx = keys.verify(token)?;
        if !ctx.scopes.iter().any(|s| s == keys.write_scope()) {
            return Err(AppError::Forbidden(format!(
                "scope '{}' required",
                keys.write_scope()
            )));
        }
        Ok(WriteAuth(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, extract::FromRef, http::Request, routing::post, Router};
    use tower::ServiceExt;

    #[derive(Clone)]
    struct S {
        keys: JwtKeys,
    }
    impl FromRef<S> for JwtKeys {
        fn from_ref(s: &S) -> JwtKeys {
            s.keys.clone()
        }
    }

    fn app() -> (Router, JwtKeys) {
        let keys = JwtKeys::new("test-secret", "masterdata:write");
        async fn h(WriteAuth(ctx): WriteAuth) -> String {
            ctx.subject
        }
        let r = Router::new()
            .route("/w", post(h))
            .with_state(S { keys: keys.clone() });
        (r, keys)
    }

    async fn call(r: Router, auth: Option<&str>) -> (u16, String) {
        let mut req = Request::post("/w");
        if let Some(a) = auth {
            req = req.header("authorization", a);
        }
        let resp = r.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status().as_u16();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        (status, String::from_utf8_lossy(&body).to_string())
    }

    #[tokio::test]
    async fn missing_header_is_401() {
        let (r, _) = app();
        assert_eq!(call(r, None).await.0, 401);
    }

    #[tokio::test]
    async fn garbage_token_is_401() {
        let (r, _) = app();
        assert_eq!(call(r, Some("Bearer not.a.jwt")).await.0, 401);
        let (r, _) = app();
        assert_eq!(call(r, Some("Basic abc")).await.0, 401);
    }

    #[tokio::test]
    async fn wrong_secret_is_401() {
        let (r, _) = app();
        let other = JwtKeys::new("other", "masterdata:write");
        let tok = other.issue("svc", &["masterdata:write"], Duration::from_secs(60));
        assert_eq!(call(r, Some(&format!("Bearer {tok}"))).await.0, 401);
    }

    #[tokio::test]
    async fn expired_token_is_401() {
        let (r, keys) = app();
        let tok = keys.issue_at(
            "svc",
            &["masterdata:write"],
            Utc::now() - chrono::Duration::hours(2),
            Duration::from_secs(60),
        );
        assert_eq!(call(r, Some(&format!("Bearer {tok}"))).await.0, 401);
    }

    #[tokio::test]
    async fn missing_scope_is_403() {
        let (r, keys) = app();
        let tok = keys.issue("svc", &["masterdata:read"], Duration::from_secs(60));
        assert_eq!(call(r, Some(&format!("Bearer {tok}"))).await.0, 403);
    }

    #[tokio::test]
    async fn valid_token_exposes_subject() {
        let (r, keys) = app();
        let tok = keys.issue(
            "svc-catalog",
            &["other", "masterdata:write"],
            Duration::from_secs(60),
        );
        let (status, body) = call(r, Some(&format!("Bearer {tok}"))).await;
        assert_eq!(status, 200);
        assert_eq!(body, "svc-catalog");
    }

    #[tokio::test]
    async fn bearer_scheme_is_case_insensitive() {
        let (r, keys) = app();
        let tok = keys.issue("svc", &["masterdata:write"], Duration::from_secs(60));
        for scheme in ["Bearer", "bearer", "BEARER", "bEaReR"] {
            let (status, body) = call(r.clone(), Some(&format!("{scheme} {tok}"))).await;
            assert_eq!(status, 200, "{scheme}");
            assert_eq!(body, "svc", "{scheme}");
        }
    }

    #[tokio::test]
    async fn invalid_token_detail_does_not_leak_decode_reason() {
        let (r, _) = app();
        let (status, body) = call(r, Some("Bearer not.a.jwt")).await;
        assert_eq!(status, 401);
        // The detail is a fixed string: no `jsonwebtoken` error text is echoed back.
        assert!(body.contains(r#""detail":"invalid token""#), "{body}");
    }
}
