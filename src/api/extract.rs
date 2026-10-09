use axum::{
    extract::{rejection::JsonRejection, FromRequest, FromRequestParts, Json, Path, Query, Request},
    http::request::Parts,
};
use serde::de::DeserializeOwned;

use crate::domain::FieldError;
use crate::error::AppError;

/// `Json<T>` whose rejection is a 422 problem+json with field `body`.
pub struct AppJson<T>(pub T);

impl<S, T> FromRequest<S> for AppJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(AppJson(v)),
            Err(rej) => Err(AppError::Validation(vec![FieldError::new(
                "body",
                body_text(&rej),
            )])),
        }
    }
}

fn body_text(rej: &JsonRejection) -> String {
    match rej {
        JsonRejection::JsonDataError(e) => e.body_text(),
        JsonRejection::JsonSyntaxError(e) => e.body_text(),
        JsonRejection::MissingJsonContentType(_) => "expected Content-Type: application/json".to_string(),
        // `JsonRejection` is `#[non_exhaustive]`, so the remaining variants
        // (currently only `BytesRejection`) are covered here.
        other => other.body_text(),
    }
}

/// `Query<T>` whose rejection is a 422 problem+json with field `query`.
pub struct AppQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for AppQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(v)) => Ok(AppQuery(v)),
            Err(rej) => Err(AppError::Validation(vec![FieldError::new(
                "query",
                rej.body_text(),
            )])),
        }
    }
}

/// `Path<T>` whose rejection is a 422 problem+json with field `id`.
pub struct AppPath<T>(pub T);

impl<S, T> FromRequestParts<S> for AppPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(v)) => Ok(AppPath(v)),
            Err(rej) => Err(AppError::Validation(vec![FieldError::new("id", rej.body_text())])),
        }
    }
}

/// `If-Match: "<version>"` header, required and strictly a quoted integer.
pub struct IfMatch(pub i32);

impl<S> FromRequestParts<S> for IfMatch
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let raw = parts
            .headers
            .get(axum::http::header::IF_MATCH)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::PreconditionRequired("If-Match header is required".into()))?;
        let trimmed = raw.trim();
        let inner = trimmed
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .and_then(|s| s.parse::<i32>().ok())
            .ok_or_else(|| {
                AppError::PreconditionRequired(format!(
                    "If-Match must be a quoted integer version, e.g. \"3\"; got {trimmed}"
                ))
            })?;
        Ok(IfMatch(inner))
    }
}
