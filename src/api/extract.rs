use axum::{
    extract::{rejection::JsonRejection, FromRequest, FromRequestParts, Json, Path, Query, Request},
    http::{request::Parts, StatusCode},
};
use serde::de::DeserializeOwned;

use crate::domain::FieldError;
use crate::error::AppError;

/// `Json<T>` whose rejections become problem+json: `415` for a wrong or missing
/// `Content-Type`, `413` for an over-limit body, and `422` (field `body`) for an
/// unparseable or schema-invalid body.
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
            Err(rej) => Err(json_error(rej)),
        }
    }
}

fn json_error(rej: JsonRejection) -> AppError {
    match &rej {
        JsonRejection::MissingJsonContentType(_) => AppError::UnsupportedMediaType,
        JsonRejection::BytesRejection(_) if rej.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            AppError::PayloadTooLarge
        }
        // Syntax and schema errors, and a non-limit body-read failure, stay a 422
        // field error; `body_text()` carries the extractor's message.
        _ => AppError::Validation(vec![FieldError::new("body", rej.body_text())]),
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
