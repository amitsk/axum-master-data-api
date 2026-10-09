pub mod dto;
pub mod entity;
pub mod extract;
pub mod handlers;
pub mod openapi;
pub mod routes;

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{FromRef, Request, State},
    http::{
        header::{HeaderName, CONTENT_TYPE},
        HeaderValue, Method,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use tower::ServiceBuilder;
use tower_http::{
    cors::{Any, CorsLayer},
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use utoipa_swagger_ui::SwaggerUi;

use crate::auth::JwtKeys;
use crate::error::{add_problem_instance, AppError};
use crate::service::MasterDataService;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<MasterDataService>,
    pub jwt: JwtKeys,
}

impl FromRef<AppState> for JwtKeys {
    fn from_ref(s: &AppState) -> JwtKeys {
        s.jwt.clone()
    }
}

const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

fn make_span(req: &Request) -> tracing::Span {
    let request_id = req
        .headers()
        .get(&REQUEST_ID)
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .unwrap_or("-");
    // `id` is declared `Empty` so the per-entity handlers can record the record id into it.
    tracing::info_span!(
        "http.request",
        method = %req.method(),
        path = %req.uri().path(),
        request_id = %request_id,
        id = tracing::field::Empty,
    )
}

pub fn build_app(state: AppState, request_timeout: Duration) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/ready", get(handlers::ready))
        .merge(routes::entity_routes())
        .merge(SwaggerUi::new("/docs").url("/openapi.json", openapi::document()))
        // Unknown paths and wrong methods are part of the API contract too: without
        // these, axum answers with an empty 404/405 body, breaking the
        // "every error is problem+json" rule.
        .fallback(handlers::fallback_404)
        .method_not_allowed_fallback(handlers::fallback_405)
        // Innermost layer: the timeout fires before `add_problem_instance` sees the
        // response, so the 408 still gets `instance` filled in like any other error.
        // (Layers added earlier are inner; `add_problem_instance` below wraps this.)
        .layer(middleware::from_fn_with_state(
            request_timeout,
            request_timeout_mw,
        ))
        .layer(middleware::from_fn(add_problem_instance))
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::new(REQUEST_ID.clone(), MakeRequestUuid))
                .layer(TraceLayer::new_for_http().make_span_with(make_span))
                .layer(PropagateRequestIdLayer::new(REQUEST_ID.clone()))
                // Reads are public, so cross-origin GET/HEAD stays open. Writes need
                // a bearer token and are therefore same-origin (or non-browser)
                // only: a foreign page must not be able to drive credentialed
                // POST/PUT/DELETE through a visitor's browser. `permissive()` would
                // allow exactly that, so the allow-list is explicit.
                .layer(
                    CorsLayer::new()
                        .allow_origin(Any)
                        .allow_methods([Method::GET, Method::HEAD])
                        .allow_headers([CONTENT_TYPE])
                        .expose_headers(Any),
                ),
        )
        .with_state(state)
}

/// Drops handler execution after the configured timeout and answers 408 as
/// problem+json (via `add_problem_instance`, which wraps this layer).
async fn request_timeout_mw(State(timeout): State<Duration>, req: Request, next: Next) -> Response {
    match tokio::time::timeout(timeout, next.run(req)).await {
        Ok(resp) => resp,
        Err(_) => AppError::RequestTimeout.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn timeout_answers_408_problem_json_with_instance() {
        async fn slow() -> StatusCode {
            tokio::time::sleep(Duration::from_secs(30)).await;
            StatusCode::OK
        }
        let app = Router::new()
            .route("/slow", get(slow))
            .layer(middleware::from_fn_with_state(
                Duration::from_millis(50),
                request_timeout_mw,
            ))
            .layer(middleware::from_fn(add_problem_instance));
        let resp = app
            .oneshot(Request::get("/slow").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::REQUEST_TIMEOUT);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/problem+json"
        );
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let p: crate::error::ProblemDetails = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(p.detail, "request timed out");
        assert_eq!(p.instance.as_deref(), Some("/slow"));
    }
}
