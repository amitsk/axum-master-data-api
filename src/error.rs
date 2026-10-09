use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::FieldError;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("code '{0}' already exists")]
    DuplicateCode(String),
    #[error("version mismatch: expected {expected}, current {current}")]
    VersionConflict { expected: i32, current: i32 },
    #[error("{0}")]
    PreconditionRequired(String),
    #[error("validation failed")]
    Validation(Vec<FieldError>),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("method not allowed")]
    MethodNotAllowed,
    #[error("request timed out")]
    RequestTimeout,
    #[error("service unavailable")]
    ServiceUnavailable,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

/// RFC 9457 problem details.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub r#type: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<FieldError>>,
}

pub const PROBLEM_JSON: &str = "application/problem+json";

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::DuplicateCode(_) | AppError::VersionConflict { .. } => StatusCode::CONFLICT,
            AppError::PreconditionRequired(_) => StatusCode::PRECONDITION_REQUIRED,
            AppError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            AppError::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            AppError::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn problem(&self) -> ProblemDetails {
        let status = self.status();
        let (detail, errors) = match self {
            AppError::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                ("an internal error occurred".to_string(), None)
            }
            AppError::Validation(errs) => (self.to_string(), Some(errs.clone())),
            other => (other.to_string(), None),
        };
        ProblemDetails {
            r#type: "about:blank".to_string(),
            title: status.canonical_reason().unwrap_or("Error").to_string(),
            status: status.as_u16(),
            detail,
            instance: None,
            errors,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let mut resp = (status, Json(self.problem())).into_response();
        resp.headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
        resp
    }
}

/// Rewrites problem+json bodies to include `instance` = request path.
pub async fn add_problem_instance(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let resp = next.run(req).await;
    let is_problem = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        // Media types are case-insensitive and may carry parameters, e.g. `; charset=utf-8`.
        // Match on the media type alone so a formatted variant still gets `instance`.
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or(v)
                .trim()
                .eq_ignore_ascii_case(PROBLEM_JSON)
        })
        .unwrap_or(false);
    if !is_problem {
        return resp;
    }
    let (parts, body) = resp.into_parts();
    let bytes = match body.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            tracing::warn!(error = ?e, "failed to read problem+json body");
            return Response::from_parts(parts, Body::empty());
        }
    };
    match serde_json::from_slice::<ProblemDetails>(&bytes) {
        Ok(mut p) => {
            p.instance = Some(path);
            let mut parts = parts;
            parts.headers.remove(header::CONTENT_LENGTH);
            Response::from_parts(parts, Body::from(serde_json::to_vec(&p).unwrap_or_default()))
        }
        Err(_) => Response::from_parts(parts, Body::from(bytes)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, middleware, routing::get, Router};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn problem_from(err: AppError) -> (StatusCode, ProblemDetails) {
        let resp = err.into_response();
        let status = resp.status();
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn each_variant_maps_to_status_and_title() {
        let cases = vec![
            (AppError::NotFound, 404, "Not Found"),
            (AppError::DuplicateCode("US".into()), 409, "Conflict"),
            (
                AppError::VersionConflict {
                    expected: 3,
                    current: 4,
                },
                409,
                "Conflict",
            ),
            (
                AppError::PreconditionRequired("If-Match".into()),
                428,
                "Precondition Required",
            ),
            (
                AppError::Validation(vec![FieldError::new("code", "required")]),
                422,
                "Unprocessable Entity",
            ),
            (AppError::Unauthorized("no token".into()), 401, "Unauthorized"),
            (AppError::Forbidden("scope".into()), 403, "Forbidden"),
            (AppError::MethodNotAllowed, 405, "Method Not Allowed"),
            (AppError::RequestTimeout, 408, "Request Timeout"),
            (AppError::ServiceUnavailable, 503, "Service Unavailable"),
            (
                AppError::Internal(anyhow::anyhow!("db down")),
                500,
                "Internal Server Error",
            ),
        ];
        for (err, status, title) in cases {
            let (s, p) = problem_from(err).await;
            assert_eq!(s.as_u16(), status);
            assert_eq!(p.status, status);
            assert_eq!(p.title, title);
        }
    }

    #[tokio::test]
    async fn version_conflict_detail_and_validation_errors() {
        let (_, p) = problem_from(AppError::VersionConflict {
            expected: 3,
            current: 4,
        })
        .await;
        assert_eq!(p.detail, "version mismatch: expected 3, current 4");
        let (_, p) = problem_from(AppError::Validation(vec![FieldError::new(
            "attributes",
            "at most 2 keys",
        )]))
        .await;
        assert_eq!(p.errors.unwrap()[0].field, "attributes");
    }

    #[tokio::test]
    async fn internal_error_hides_cause() {
        let (_, p) = problem_from(AppError::Internal(anyhow::anyhow!("password=hunter2"))).await;
        assert!(!p.detail.contains("hunter2"));
    }

    #[tokio::test]
    async fn middleware_fills_instance_with_request_path() {
        async fn boom() -> Result<(), AppError> {
            Err(AppError::NotFound)
        }
        let app = Router::new()
            .route("/api/v1/countries/{id}", get(boom))
            .layer(middleware::from_fn(add_problem_instance));
        let resp = app
            .oneshot(Request::get("/api/v1/countries/42").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let p: ProblemDetails = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(p.instance.as_deref(), Some("/api/v1/countries/42"));
    }

    #[tokio::test]
    async fn middleware_matches_media_type_case_insensitively_and_ignores_parameters() {
        const BODY: &str = r#"{"type":"about:blank","title":"Not Found","status":404,"detail":"not found"}"#;

        async fn with_content_type(req: axum::extract::Request) -> Response {
            let ct = req
                .headers()
                .get("x-test-content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or(PROBLEM_JSON);
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(header::CONTENT_TYPE, ct)
                .body(Body::from(BODY))
                .unwrap()
        }

        let app = Router::new()
            .route("/api/v1/countries/{id}", get(with_content_type))
            .layer(middleware::from_fn(add_problem_instance));

        for ct in [
            "application/problem+json; charset=utf-8",
            "Application/Problem+JSON;charset=UTF-8",
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::get("/api/v1/countries/42")
                        .header("x-test-content-type", ct)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{ct}");
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let p: ProblemDetails = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(p.instance.as_deref(), Some("/api/v1/countries/42"), "{ct}");
        }
    }
}
