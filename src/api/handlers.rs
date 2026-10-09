use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

use crate::api::{
    dto::{ListQuery, PageResponse, RecordRequest, RecordResponse},
    entity::Entity,
    extract::{AppJson, AppPath, AppQuery, IfMatch},
    AppState,
};
use crate::auth::WriteAuth;
use crate::domain::ListFilter;
use crate::error::AppError;

pub async fn create<E: Entity>(
    State(state): State<AppState>,
    WriteAuth(ctx): WriteAuth,
    AppJson(body): AppJson<RecordRequest>,
) -> Result<Response, AppError> {
    let rec = state.service.create(E::TYPE, body.into(), &ctx.subject).await?;
    let location = format!("/api/v1/{}/{}", E::PATH, rec.id);
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(RecordResponse::from(rec)),
    )
        .into_response())
}

pub async fn get_one<E: Entity>(
    State(state): State<AppState>,
    AppPath(id): AppPath<i64>,
) -> Result<Json<RecordResponse>, AppError> {
    tracing::Span::current().record("id", id);
    Ok(Json(state.service.get(E::TYPE, id).await?.into()))
}

pub async fn list<E: Entity>(
    State(state): State<AppState>,
    AppQuery(q): AppQuery<ListQuery>,
) -> Result<Json<PageResponse>, AppError> {
    let filter = ListFilter::new(q.code, q.name, q.limit, q.offset).map_err(AppError::Validation)?;
    Ok(Json(state.service.list(E::TYPE, &filter).await?.into()))
}

pub async fn update<E: Entity>(
    State(state): State<AppState>,
    WriteAuth(ctx): WriteAuth,
    AppPath(id): AppPath<i64>,
    IfMatch(version): IfMatch,
    AppJson(body): AppJson<RecordRequest>,
) -> Result<Json<RecordResponse>, AppError> {
    tracing::Span::current().record("id", id);
    let rec = state
        .service
        .update(E::TYPE, id, version, body.into(), &ctx.subject)
        .await?;
    Ok(Json(rec.into()))
}

pub async fn delete<E: Entity>(
    State(state): State<AppState>,
    WriteAuth(ctx): WriteAuth,
    AppPath(id): AppPath<i64>,
) -> Result<StatusCode, AppError> {
    tracing::Span::current().record("id", id);
    state.service.delete(E::TYPE, id, &ctx.subject).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Catches unknown paths so they answer problem+json instead of axum's empty 404.
pub async fn fallback_404() -> AppError {
    AppError::NotFound
}

/// Catches wrong-method requests so they answer problem+json instead of axum's
/// empty 405.
pub async fn fallback_405() -> AppError {
    AppError::MethodNotAllowed
}

pub async fn health() -> StatusCode {
    StatusCode::OK
}

pub async fn ready(State(state): State<AppState>) -> Result<StatusCode, AppError> {
    state.service.ready().await.map_err(|e| {
        tracing::warn!(error = %e, "readiness check failed");
        AppError::ServiceUnavailable
    })?;
    Ok(StatusCode::OK)
}
