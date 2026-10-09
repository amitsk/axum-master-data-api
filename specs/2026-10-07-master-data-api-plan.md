# Master Data REST API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust/axum REST API giving CRUD over ten master-data entities stored in one generic Postgres table, with JWT-guarded writes, optimistic locking, soft delete, tracing, typed config, OpenAPI, and tests at unit, repository, and end-to-end level.

**Architecture:** Layers point downward: `api` (axum handlers generic over an `Entity` marker, one route set per entity) → `service` (normalisation, validation, error mapping) → `repository` (trait; Postgres impl via sqlx, in-memory fake for tests) → `domain` (types). One `AppError` converts to RFC 9457 problem+json at the boundary. Config via figment, observability via `tracing` + tower-http layers.

**Tech Stack:** Rust 1.89, tokio, axum 0.8, tower-http 0.6, sqlx 0.8 (postgres, compile-time macros, `.sqlx` offline cache), figment 0.10, tracing/tracing-subscriber, thiserror/anyhow, jsonwebtoken 9 (HS256), utoipa 5 + utoipa-swagger-ui 9, serde, testcontainers-modules 0.11, reqwest 0.12.

**Spec:** `specs/2026-10-07-master-data-api-design.md`

## Global Constraints

- Toolchain pinned to `1.89.0` via `rust-toolchain.toml`; edition 2021.
- Crate versions as in Task 1 `Cargo.toml`; do not upgrade majors mid-plan.
- Wire JSON is camelCase; unknown request fields are rejected (`deny_unknown_fields`).
- Error responses are `application/problem+json` (RFC 9457) with `instance` = request path.
- `code` is trimmed and upper-cased before every read and write; max 64 chars. `name` max 256, `short_name` max 64, all non-empty after trim.
- `attributes`: object, ≤ 2 keys, keys non-empty after trim, string values. Enforced in Rust and by DB CHECK.
- IDs are per `entity_type`, dense, from `id_counters`.
- All reads exclude rows with `deleted_at IS NOT NULL`.
- Mutating routes need `Authorization: Bearer <HS256 JWT>` with scope `masterdata:write` (configurable); `sub` becomes `updated_by`.
- `PUT` requires `If-Match: "<version>"`; absent → 428, mismatch → 409.
- List: `limit` default 50, max 500, min 1; `offset` ≥ 0; ordered by `id`.
- Only `tracing` macros; no `log`.
- Compile-time sqlx macros need either `DATABASE_URL` (dev DB from `docker-compose.yml`) or the committed `.sqlx/` directory with `SQLX_OFFLINE=true`. After any change to a `query!`/`query_as!` string, run `cargo sqlx prepare` and commit `.sqlx/`.
- Every commit message ends with the trailer line `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` (shown in each commit step as a second `-m`).

## Review Focus

1. `If-Match` present but malformed (`If-Match: abc`, `If-Match: 3` unquoted) — expected: `428` with detail saying the header must be a quoted integer version, not a 500 or silent acceptance. Test in Task 7.
2. `limit=0`, `limit=-1`, `offset=-1`, `limit=10000` — expected: `limit` 0/negative → 422, `offset` negative → 422, `limit` over 500 → 422 (not silently clamped, so clients learn the cap). Test in Task 3.
3. `code`/`name`/`shortName` that are whitespace-only or over length — expected: 422 with the field named. Test in Task 5.
4. `attributes` with a whitespace-only key, or three keys, or a non-string value — expected: 422 with field `attributes`. Tests in Tasks 3, 5 and 7.
5. Non-numeric path id (`GET /api/v1/countries/abc`) — expected: 422 problem+json naming field `id`, not axum's plain-text 400. Test in Task 7.

---

### Task 1: Project scaffold, config, and dev database

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, `.env`, `docker-compose.yml`, `config/default.toml`, `src/lib.rs`, `src/main.rs`, `src/config.rs`

**Interfaces:**
- Produces: `config::AppConfig::load() -> Result<AppConfig, figment::Error>`; `AppConfig { server: ServerConfig { host: String, port: u16, request_timeout_secs: u64 }, database: DatabaseConfig { url: SecretString, max_connections: u32, acquire_timeout_secs: u64 }, auth: AuthConfig { jwt_secret: SecretString, write_scope: String }, log: LogConfig { format: LogFormat } }`, `enum LogFormat { Pretty, Json }`.

- [ ] **Step 1: Create the crate and support files**

```bash
cd /home/amit/projects/rust/axum-master-data-api
cargo init --name axum-master-data-api
```

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "1.89.0"
components = ["rustfmt", "clippy"]
```

`Cargo.toml`:
```toml
[package]
name = "axum-master-data-api"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1"
async-trait = "0.1"
axum = { version = "0.8", features = ["macros"] }
chrono = { version = "0.4", features = ["serde"] }
figment = { version = "0.10", features = ["toml", "env"] }
http-body-util = "0.1"
jsonwebtoken = "9"
secrecy = { version = "0.10", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.8", features = ["runtime-tokio", "tls-rustls", "postgres", "macros", "migrate", "chrono", "json"] }
thiserror = "2"
tokio = { version = "1", features = ["full"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["trace", "request-id", "timeout", "cors", "util"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt", "json"] }
utoipa = { version = "5", features = ["axum_extras", "chrono"] }
utoipa-swagger-ui = { version = "9", features = ["axum"] }
uuid = { version = "1", features = ["v4"] }

[dev-dependencies]
figment = { version = "0.10", features = ["test"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
testcontainers-modules = { version = "0.11", features = ["postgres"] }
tower = { version = "0.5", features = ["util"] }

[lints.clippy]
all = "warn"
```

`.gitignore`:
```
/target
.superpowers/
```

`.env` (committed; dev-only credentials):
```
DATABASE_URL=postgres://postgres:postgres@localhost:5432/master_data
```

`docker-compose.yml`:
```yaml
services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_PASSWORD: postgres
      POSTGRES_DB: master_data
    ports:
      - "5432:5432"
```

`config/default.toml`:
```toml
[server]
host = "0.0.0.0"
port = 8080
request_timeout_secs = 30

[database]
max_connections = 10
acquire_timeout_secs = 5

[auth]
write_scope = "masterdata:write"

[log]
format = "pretty"
```

`src/lib.rs`:
```rust
pub mod config;
```

`src/main.rs`:
```rust
fn main() {
    let cfg = axum_master_data_api::config::AppConfig::load().expect("config");
    println!("{cfg:?}");
}
```

- [ ] **Step 2: Write the failing config tests**

`src/config.rs`:
```rust
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use secrecy::SecretString;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub log: LogConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub request_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    pub url: SecretString,
    pub max_connections: u32,
    pub acquire_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct AuthConfig {
    pub jwt_secret: SecretString,
    pub write_scope: String,
}

#[derive(Debug, Deserialize)]
pub struct LogConfig {
    pub format: LogFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Json,
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::Jail;
    use secrecy::ExposeSecret;

    #[test]
    fn env_overrides_toml_and_secrets_come_from_env() {
        Jail::expect_with(|jail| {
            jail.create_dir("config")?;
            jail.create_file(
                "config/default.toml",
                r#"
                [server]
                host = "0.0.0.0"
                port = 8080
                request_timeout_secs = 30
                [database]
                max_connections = 10
                acquire_timeout_secs = 5
                [auth]
                write_scope = "masterdata:write"
                [log]
                format = "pretty"
                "#,
            )?;
            jail.set_env("APP_SERVER__PORT", "9090");
            jail.set_env("APP_DATABASE__URL", "postgres://u:p@h/db");
            jail.set_env("APP_AUTH__JWT_SECRET", "s3cr3t");
            jail.set_env("APP_LOG__FORMAT", "json");
            let cfg = AppConfig::load().expect("load");
            assert_eq!(cfg.server.port, 9090);
            assert_eq!(cfg.database.url.expose_secret(), "postgres://u:p@h/db");
            assert_eq!(cfg.auth.jwt_secret.expose_secret(), "s3cr3t");
            assert_eq!(cfg.log.format, LogFormat::Json);
            Ok(())
        });
    }

    #[test]
    fn missing_secret_is_an_error_naming_the_field() {
        Jail::expect_with(|jail| {
            jail.create_dir("config")?;
            jail.create_file(
                "config/default.toml",
                r#"
                [server]
                host = "0.0.0.0"
                port = 8080
                request_timeout_secs = 30
                [database]
                max_connections = 10
                acquire_timeout_secs = 5
                [auth]
                write_scope = "masterdata:write"
                [log]
                format = "pretty"
                "#,
            )?;
            jail.set_env("APP_DATABASE__URL", "postgres://u:p@h/db");
            let err = AppConfig::load().expect_err("should fail");
            assert!(err.to_string().contains("jwt_secret"), "{err}");
            Ok(())
        });
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test config`
Expected: compile error, `AppConfig::load` not found.

- [ ] **Step 4: Implement `load`**

Add above the `#[cfg(test)]` block in `src/config.rs`:
```rust
impl AppConfig {
    /// Precedence, low to high: config/default.toml, config/{APP_ENV}.toml, APP_* env vars.
    pub fn load() -> Result<Self, figment::Error> {
        let env_name = std::env::var("APP_ENV").unwrap_or_else(|_| "development".to_string());
        Figment::new()
            .merge(Toml::file("config/default.toml"))
            .merge(Toml::file(format!("config/{env_name}.toml")))
            .merge(Env::prefixed("APP_").split("__"))
            .extract()
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test config`
Expected: 2 passed.

- [ ] **Step 6: Start the dev database and commit**

```bash
docker compose up -d
cargo build
git add -A
git commit -m "feat: scaffold crate with figment config and dev database" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: Error model and problem+json responses

**Files:**
- Create: `src/error.rs`, `src/domain/mod.rs`, `src/domain/validation.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces: `domain::FieldError { field: String, message: String }` with `FieldError::new(field, message)`;
  `error::AppError` enum `{ NotFound, DuplicateCode(String), VersionConflict { expected: i32, current: i32 }, PreconditionRequired(String), Validation(Vec<FieldError>), Unauthorized(String), Forbidden(String), Internal(anyhow::Error) }` implementing `IntoResponse`;
  `error::ProblemDetails { r#type, title, status, detail, instance: Option<String>, errors: Option<Vec<FieldError>> }` (Serialize/Deserialize);
  `error::add_problem_instance` axum middleware fn.

- [ ] **Step 1: Write the failing tests**

`src/domain/mod.rs`:
```rust
pub mod validation;

pub use validation::FieldError;
```

`src/domain/validation.rs`:
```rust
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self { field: field.into(), message: message.into() }
    }
}
```

`src/lib.rs`:
```rust
pub mod config;
pub mod domain;
pub mod error;
```

`src/error.rs` (tests only; implementation in Step 3):
```rust
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
            (AppError::VersionConflict { expected: 3, current: 4 }, 409, "Conflict"),
            (AppError::PreconditionRequired("If-Match".into()), 428, "Precondition Required"),
            (AppError::Validation(vec![FieldError::new("code", "required")]), 422, "Unprocessable Entity"),
            (AppError::Unauthorized("no token".into()), 401, "Unauthorized"),
            (AppError::Forbidden("scope".into()), 403, "Forbidden"),
            (AppError::Internal(anyhow::anyhow!("db down")), 500, "Internal Server Error"),
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
        let (_, p) = problem_from(AppError::VersionConflict { expected: 3, current: 4 }).await;
        assert_eq!(p.detail, "version mismatch: expected 3, current 4");
        let (_, p) = problem_from(AppError::Validation(vec![FieldError::new("attributes", "at most 2 keys")])).await;
        assert_eq!(p.errors.unwrap()[0].field, "attributes");
    }

    #[tokio::test]
    async fn internal_error_hides_cause() {
        let (_, p) = problem_from(AppError::Internal(anyhow::anyhow!("password=hunter2"))).await;
        assert!(!p.detail.contains("hunter2"));
    }

    #[tokio::test]
    async fn middleware_fills_instance_with_request_path() {
        async fn boom() -> Result<(), AppError> { Err(AppError::NotFound) }
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
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test error`
Expected: compile errors for `AppError`, `ProblemDetails`, `add_problem_instance`.

- [ ] **Step 3: Implement**

Put above the tests in `src/error.rs`:
```rust
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
            AppError::Validation(errs) => ("validation failed".to_string(), Some(errs.clone())),
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
        .map(|v| v == PROBLEM_JSON)
        .unwrap_or(false);
    if !is_problem {
        return resp;
    }
    let (parts, body) = resp.into_parts();
    let bytes = match body.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return Response::from_parts(parts, Body::empty()),
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test error`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: AppError with RFC 9457 problem+json responses" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: Domain types — EntityType, Attributes, records, list filter

**Files:**
- Create: `src/domain/entity.rs`, `src/domain/filter.rs`
- Modify: `src/domain/mod.rs`

**Interfaces:**
- Produces:
  `EntityType` enum (10 variants: `Country, Currency, Language, Category, Gender, Age, Geography, ProductLine, BusinessUnit, ProductTier`), `EntityType::ALL: [EntityType; 10]`, `EntityType::as_str(self) -> &'static str` (snake_case, matches the Postgres enum);
  `Attributes` newtype with `Attributes::MAX_KEYS = 2`, `Attributes::new(BTreeMap<String,String>) -> Result<Attributes, AttributesError>`, `Attributes::as_map(&self) -> &BTreeMap<String,String>`, `Attributes::to_json(&self) -> serde_json::Value`, `Attributes::from_json(serde_json::Value) -> Result<Attributes, AttributesError>`, `impl Default`;
  `AttributesError` enum `{ TooManyKeys { max: usize, got: usize }, EmptyKey, NotAnObject, NonStringValue(String) }` with `Display`;
  `normalize_code(&str) -> String` (trim + uppercase);
  `RecordFields { code: String, name: String, short_name: String, attributes: Attributes }`, `type NewRecord = RecordFields`, `type UpdateRecord = RecordFields`;
  `MasterRecord { entity_type: EntityType, id: i64, code: String, name: String, short_name: String, attributes: Attributes, version: i32, created_at: DateTime<Utc>, updated_at: DateTime<Utc>, updated_by: String }`;
  `ListFilter { code: Option<String>, name: Option<String>, limit: i64, offset: i64 }`, `ListFilter::DEFAULT_LIMIT = 50`, `ListFilter::MAX_LIMIT = 500`, `ListFilter::new(code: Option<String>, name: Option<String>, limit: Option<i64>, offset: Option<i64>) -> Result<ListFilter, Vec<FieldError>>`;
  `Page<T> { items: Vec<T>, limit: i64, offset: i64, total: i64 }`.

- [ ] **Step 1: Write the failing tests**

`src/domain/mod.rs`:
```rust
pub mod entity;
pub mod filter;
pub mod validation;

pub use entity::{
    normalize_code, Attributes, AttributesError, EntityType, MasterRecord, NewRecord, RecordFields,
    UpdateRecord,
};
pub use filter::{ListFilter, Page};
pub use validation::FieldError;
```

Tests at the bottom of `src/domain/entity.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn entity_type_round_trips_snake_case() {
        assert_eq!(EntityType::ProductLine.as_str(), "product_line");
        assert_eq!(EntityType::ALL.len(), 10);
        let json = serde_json::to_string(&EntityType::BusinessUnit).unwrap();
        assert_eq!(json, "\"business_unit\"");
    }

    #[test]
    fn attributes_accepts_up_to_two_keys() {
        assert!(Attributes::new(map(&[])).is_ok());
        assert!(Attributes::new(map(&[("a", "1")])).is_ok());
        assert!(Attributes::new(map(&[("a", "1"), ("b", "2")])).is_ok());
    }

    #[test]
    fn attributes_rejects_three_keys() {
        let err = Attributes::new(map(&[("a", "1"), ("b", "2"), ("c", "3")])).unwrap_err();
        assert_eq!(err, AttributesError::TooManyKeys { max: 2, got: 3 });
    }

    #[test]
    fn attributes_rejects_blank_key() {
        assert_eq!(Attributes::new(map(&[("  ", "x")])).unwrap_err(), AttributesError::EmptyKey);
        assert_eq!(Attributes::new(map(&[("", "x")])).unwrap_err(), AttributesError::EmptyKey);
    }

    #[test]
    fn attributes_from_json_rejects_non_object_and_non_string_values() {
        assert_eq!(Attributes::from_json(serde_json::json!([1])).unwrap_err(), AttributesError::NotAnObject);
        assert_eq!(
            Attributes::from_json(serde_json::json!({"a": 1})).unwrap_err(),
            AttributesError::NonStringValue("a".into())
        );
        let a = Attributes::from_json(serde_json::json!({"iso3": "USA"})).unwrap();
        assert_eq!(a.as_map()["iso3"], "USA");
        assert_eq!(a.to_json(), serde_json::json!({"iso3": "USA"}));
    }

    #[test]
    fn normalize_code_trims_and_uppercases() {
        assert_eq!(normalize_code("  us "), "US");
        assert_eq!(normalize_code("gbp"), "GBP");
    }
}
```

Tests at the bottom of `src/domain/filter.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_when_absent() {
        let f = ListFilter::new(None, None, None, None).unwrap();
        assert_eq!(f.limit, 50);
        assert_eq!(f.offset, 0);
        assert_eq!(f.code, None);
    }

    #[test]
    fn code_filter_is_normalized_and_blank_name_dropped() {
        let f = ListFilter::new(Some(" us ".into()), Some("   ".into()), Some(10), Some(5)).unwrap();
        assert_eq!(f.code.as_deref(), Some("US"));
        assert_eq!(f.name, None);
        assert_eq!((f.limit, f.offset), (10, 5));
    }

    #[test]
    fn limit_bounds_are_enforced_not_clamped() {
        for bad in [0, -1, 501, 10_000] {
            let errs = ListFilter::new(None, None, Some(bad), None).unwrap_err();
            assert_eq!(errs[0].field, "limit", "limit={bad}");
        }
        assert!(ListFilter::new(None, None, Some(500), None).is_ok());
    }

    #[test]
    fn negative_offset_rejected() {
        let errs = ListFilter::new(None, None, None, Some(-1)).unwrap_err();
        assert_eq!(errs[0].field, "offset");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test domain`
Expected: compile errors (types missing).

- [ ] **Step 3: Implement `entity.rs`**

```rust
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "entity_type", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Country,
    Currency,
    Language,
    Category,
    Gender,
    Age,
    Geography,
    ProductLine,
    BusinessUnit,
    ProductTier,
}

impl EntityType {
    pub const ALL: [EntityType; 10] = [
        EntityType::Country,
        EntityType::Currency,
        EntityType::Language,
        EntityType::Category,
        EntityType::Gender,
        EntityType::Age,
        EntityType::Geography,
        EntityType::ProductLine,
        EntityType::BusinessUnit,
        EntityType::ProductTier,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EntityType::Country => "country",
            EntityType::Currency => "currency",
            EntityType::Language => "language",
            EntityType::Category => "category",
            EntityType::Gender => "gender",
            EntityType::Age => "age",
            EntityType::Geography => "geography",
            EntityType::ProductLine => "product_line",
            EntityType::BusinessUnit => "business_unit",
            EntityType::ProductTier => "product_tier",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttributesError {
    #[error("at most {max} keys allowed, got {got}")]
    TooManyKeys { max: usize, got: usize },
    #[error("attribute keys must not be empty")]
    EmptyKey,
    #[error("attributes must be a JSON object")]
    NotAnObject,
    #[error("attribute '{0}' must be a string")]
    NonStringValue(String),
}

/// Free-form key/value map limited to [`Attributes::MAX_KEYS`] entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Attributes(BTreeMap<String, String>);

impl Attributes {
    pub const MAX_KEYS: usize = 2;

    pub fn new(map: BTreeMap<String, String>) -> Result<Self, AttributesError> {
        if map.len() > Self::MAX_KEYS {
            return Err(AttributesError::TooManyKeys { max: Self::MAX_KEYS, got: map.len() });
        }
        if map.keys().any(|k| k.trim().is_empty()) {
            return Err(AttributesError::EmptyKey);
        }
        Ok(Self(map))
    }

    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.0).expect("map of strings serialises")
    }

    pub fn from_json(value: serde_json::Value) -> Result<Self, AttributesError> {
        let obj = match value {
            serde_json::Value::Object(o) => o,
            _ => return Err(AttributesError::NotAnObject),
        };
        let mut map = BTreeMap::new();
        for (k, v) in obj {
            match v {
                serde_json::Value::String(s) => {
                    map.insert(k, s);
                }
                _ => return Err(AttributesError::NonStringValue(k)),
            }
        }
        Self::new(map)
    }
}

pub fn normalize_code(raw: &str) -> String {
    raw.trim().to_uppercase()
}

/// Client-supplied fields, already normalised and validated by the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFields {
    pub code: String,
    pub name: String,
    pub short_name: String,
    pub attributes: Attributes,
}

pub type NewRecord = RecordFields;
pub type UpdateRecord = RecordFields;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterRecord {
    pub entity_type: EntityType,
    pub id: i64,
    pub code: String,
    pub name: String,
    pub short_name: String,
    pub attributes: Attributes,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}
```

- [ ] **Step 4: Implement `filter.rs`**

```rust
use crate::domain::{normalize_code, FieldError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListFilter {
    pub code: Option<String>,
    pub name: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl ListFilter {
    pub const DEFAULT_LIMIT: i64 = 50;
    pub const MAX_LIMIT: i64 = 500;

    pub fn new(
        code: Option<String>,
        name: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Self, Vec<FieldError>> {
        let mut errors = Vec::new();
        let limit = limit.unwrap_or(Self::DEFAULT_LIMIT);
        if limit < 1 || limit > Self::MAX_LIMIT {
            errors.push(FieldError::new("limit", format!("must be between 1 and {}", Self::MAX_LIMIT)));
        }
        let offset = offset.unwrap_or(0);
        if offset < 0 {
            errors.push(FieldError::new("offset", "must be >= 0"));
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let code = code.map(|c| normalize_code(&c)).filter(|c| !c.is_empty());
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        Ok(Self { code, name, limit, offset })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test domain`
Expected: 10 passed.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: domain types for entities, attributes, and list filter" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Migration, repository trait, Postgres repository

**Files:**
- Create: `migrations/20261007000001_master_data.sql`, `src/repository/mod.rs`, `src/repository/postgres.rs`, `tests/common/mod.rs`, `tests/repository.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: Task 3 domain types.
- Produces:
  `repository::RepoError` enum `{ DuplicateCode(String), Database(sqlx::Error) }`;
  `repository::UpdateOutcome` enum `{ Updated(MasterRecord), NotFound, VersionConflict { current: i32 } }`;
  `#[async_trait] trait MasterDataRepository: Send + Sync { async fn ping(&self) -> Result<(), RepoError>; async fn create(&self, t: EntityType, new: NewRecord, by: &str) -> Result<MasterRecord, RepoError>; async fn get(&self, t: EntityType, id: i64) -> Result<Option<MasterRecord>, RepoError>; async fn list(&self, t: EntityType, f: &ListFilter) -> Result<Page<MasterRecord>, RepoError>; async fn update(&self, t: EntityType, id: i64, expected_version: i32, upd: UpdateRecord, by: &str) -> Result<UpdateOutcome, RepoError>; async fn soft_delete(&self, t: EntityType, id: i64, by: &str) -> Result<bool, RepoError>; }`;
  `repository::postgres::PgMasterDataRepository::new(PgPool) -> Self`;
  `repository::postgres::connect(url: &str, max_connections: u32, acquire_timeout: Duration) -> Result<PgPool, sqlx::Error>`;
  `repository::postgres::MIGRATOR: sqlx::migrate::Migrator`;
  test helper `tests/common/mod.rs::test_pool() -> (ContainerAsync<Postgres>, PgPool)` (migrated).

- [ ] **Step 1: Write the migration**

`migrations/20261007000001_master_data.sql`:
```sql
CREATE TYPE entity_type AS ENUM (
  'country','currency','language','category','gender',
  'age','geography','product_line','business_unit','product_tier'
);

CREATE FUNCTION jsonb_key_count(j jsonb) RETURNS integer
  LANGUAGE sql IMMUTABLE AS
  $$ SELECT count(*)::integer FROM jsonb_object_keys(j) $$;

CREATE TABLE master_data (
  entity_type  entity_type NOT NULL,
  id           BIGINT      NOT NULL,
  code         TEXT        NOT NULL,
  name         TEXT        NOT NULL,
  short_name   TEXT        NOT NULL,
  attributes   JSONB       NOT NULL DEFAULT '{}'::jsonb,
  version      INTEGER     NOT NULL DEFAULT 1,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_by   TEXT        NOT NULL,
  deleted_at   TIMESTAMPTZ,
  PRIMARY KEY (entity_type, id),
  CONSTRAINT attributes_is_object CHECK (jsonb_typeof(attributes) = 'object'),
  CONSTRAINT attributes_max_two   CHECK (jsonb_key_count(attributes) <= 2)
);

CREATE UNIQUE INDEX master_data_code_live
  ON master_data (entity_type, code) WHERE deleted_at IS NULL;

CREATE INDEX master_data_name_live
  ON master_data (entity_type, lower(name)) WHERE deleted_at IS NULL;

CREATE TABLE id_counters (
  entity_type entity_type PRIMARY KEY,
  next_id     BIGINT NOT NULL DEFAULT 1
);

INSERT INTO id_counters (entity_type)
SELECT unnest(enum_range(NULL::entity_type));
```

Apply it to the dev database so the sqlx macros can compile:
```bash
cargo install sqlx-cli --no-default-features --features postgres,rustls
sqlx migrate run
```

- [ ] **Step 2: Write the trait and error types**

`src/repository/mod.rs`:
```rust
pub mod postgres;

use async_trait::async_trait;

use crate::domain::{EntityType, ListFilter, MasterRecord, NewRecord, Page, UpdateRecord};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("code '{0}' already exists")]
    DuplicateCode(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug)]
pub enum UpdateOutcome {
    Updated(MasterRecord),
    NotFound,
    VersionConflict { current: i32 },
}

#[async_trait]
pub trait MasterDataRepository: Send + Sync {
    async fn ping(&self) -> Result<(), RepoError>;
    async fn create(&self, t: EntityType, new: NewRecord, by: &str) -> Result<MasterRecord, RepoError>;
    async fn get(&self, t: EntityType, id: i64) -> Result<Option<MasterRecord>, RepoError>;
    async fn list(&self, t: EntityType, f: &ListFilter) -> Result<Page<MasterRecord>, RepoError>;
    async fn update(
        &self,
        t: EntityType,
        id: i64,
        expected_version: i32,
        upd: UpdateRecord,
        by: &str,
    ) -> Result<UpdateOutcome, RepoError>;
    async fn soft_delete(&self, t: EntityType, id: i64, by: &str) -> Result<bool, RepoError>;
}
```

`src/lib.rs`:
```rust
pub mod config;
pub mod domain;
pub mod error;
pub mod repository;
```

- [ ] **Step 3: Write the failing integration tests**

`tests/common/mod.rs`:
```rust
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::Duration;

use axum_master_data_api::domain::{Attributes, RecordFields};
use axum_master_data_api::repository::postgres::{connect, MIGRATOR};
use sqlx::PgPool;
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};

/// Starts a throwaway Postgres, runs migrations, returns the container (keep it alive) and a pool.
pub async fn test_pool() -> (ContainerAsync<Postgres>, PgPool) {
    let container = Postgres::default().start().await.expect("start postgres");
    let port = container.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = connect(&url, 5, Duration::from_secs(10)).await.expect("connect");
    MIGRATOR.run(&pool).await.expect("migrate");
    (container, pool)
}

pub fn fields(code: &str, name: &str, attrs: &[(&str, &str)]) -> RecordFields {
    let map: BTreeMap<String, String> =
        attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    RecordFields {
        code: code.to_string(),
        name: name.to_string(),
        short_name: code.to_string(),
        attributes: Attributes::new(map).unwrap(),
    }
}
```

`tests/repository.rs`:
```rust
mod common;

use axum_master_data_api::domain::{EntityType, ListFilter};
use axum_master_data_api::repository::{postgres::PgMasterDataRepository, MasterDataRepository, RepoError, UpdateOutcome};
use common::{fields, test_pool};

#[tokio::test]
async fn ids_are_dense_per_entity_type() {
    let (_c, pool) = test_pool().await;
    let repo = PgMasterDataRepository::new(pool);
    let a = repo.create(EntityType::Country, fields("US", "United States", &[]), "t").await.unwrap();
    let b = repo.create(EntityType::Country, fields("GB", "United Kingdom", &[]), "t").await.unwrap();
    let c = repo.create(EntityType::Currency, fields("USD", "US Dollar", &[]), "t").await.unwrap();
    assert_eq!((a.id, b.id, c.id), (1, 2, 1));
    assert_eq!(a.version, 1);
    assert_eq!(a.updated_by, "t");
}

#[tokio::test]
async fn duplicate_live_code_is_rejected_but_reusable_after_delete() {
    let (_c, pool) = test_pool().await;
    let repo = PgMasterDataRepository::new(pool);
    let a = repo.create(EntityType::Country, fields("US", "United States", &[]), "t").await.unwrap();
    let err = repo.create(EntityType::Country, fields("US", "Dup", &[]), "t").await.unwrap_err();
    assert!(matches!(err, RepoError::DuplicateCode(ref c) if c == "US"), "{err:?}");
    assert!(repo.soft_delete(EntityType::Country, a.id, "d").await.unwrap());
    assert!(repo.get(EntityType::Country, a.id).await.unwrap().is_none());
    let again = repo.create(EntityType::Country, fields("US", "United States", &[]), "t").await.unwrap();
    assert_eq!(again.id, 2);
}

#[tokio::test]
async fn soft_delete_is_idempotent_and_records_actor() {
    let (_c, pool) = test_pool().await;
    let repo = PgMasterDataRepository::new(pool.clone());
    let a = repo.create(EntityType::Gender, fields("F", "Female", &[]), "t").await.unwrap();
    assert!(repo.soft_delete(EntityType::Gender, a.id, "deleter").await.unwrap());
    assert!(!repo.soft_delete(EntityType::Gender, a.id, "deleter").await.unwrap());
    let by: String = sqlx::query_scalar("SELECT updated_by FROM master_data WHERE entity_type = 'gender' AND id = $1")
        .bind(a.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(by, "deleter");
}

#[tokio::test]
async fn update_bumps_version_and_detects_conflict_and_not_found() {
    let (_c, pool) = test_pool().await;
    let repo = PgMasterDataRepository::new(pool);
    let a = repo.create(EntityType::Currency, fields("USD", "US Dollar", &[("symbol", "$")]), "t").await.unwrap();
    let out = repo.update(EntityType::Currency, a.id, 1, fields("USD", "Dollar", &[("symbol", "$"), ("minor", "2")]), "u").await.unwrap();
    let rec = match out { UpdateOutcome::Updated(r) => r, o => panic!("{o:?}") };
    assert_eq!(rec.version, 2);
    assert_eq!(rec.name, "Dollar");
    assert_eq!(rec.updated_by, "u");
    assert_eq!(rec.attributes.as_map()["minor"], "2");
    assert!(rec.updated_at > a.updated_at);

    let out = repo.update(EntityType::Currency, a.id, 1, fields("USD", "Stale", &[]), "u").await.unwrap();
    assert!(matches!(out, UpdateOutcome::VersionConflict { current: 2 }), "{out:?}");

    let out = repo.update(EntityType::Currency, 999, 1, fields("X", "Missing", &[]), "u").await.unwrap();
    assert!(matches!(out, UpdateOutcome::NotFound));
}

#[tokio::test]
async fn update_to_existing_code_is_duplicate() {
    let (_c, pool) = test_pool().await;
    let repo = PgMasterDataRepository::new(pool);
    repo.create(EntityType::Language, fields("EN", "English", &[]), "t").await.unwrap();
    let fr = repo.create(EntityType::Language, fields("FR", "French", &[]), "t").await.unwrap();
    let err = repo.update(EntityType::Language, fr.id, 1, fields("EN", "French", &[]), "t").await.unwrap_err();
    assert!(matches!(err, RepoError::DuplicateCode(_)));
}

#[tokio::test]
async fn list_filters_pages_and_counts() {
    let (_c, pool) = test_pool().await;
    let repo = PgMasterDataRepository::new(pool);
    for (code, name) in [("US", "United States"), ("GB", "United Kingdom"), ("FR", "France"), ("DE", "Germany")] {
        repo.create(EntityType::Country, fields(code, name, &[]), "t").await.unwrap();
    }
    repo.create(EntityType::Currency, fields("USD", "United States Dollar", &[]), "t").await.unwrap();

    let all = repo.list(EntityType::Country, &ListFilter::new(None, None, None, None).unwrap()).await.unwrap();
    assert_eq!(all.total, 4);
    assert_eq!(all.items.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2, 3, 4]);

    let united = repo.list(EntityType::Country, &ListFilter::new(None, Some("united".into()), None, None).unwrap()).await.unwrap();
    assert_eq!(united.total, 2);

    let page = repo.list(EntityType::Country, &ListFilter::new(None, Some("UNITED".into()), Some(1), Some(1)).unwrap()).await.unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].code, "GB");

    let by_code = repo.list(EntityType::Country, &ListFilter::new(Some("fr".into()), None, None, None).unwrap()).await.unwrap();
    assert_eq!(by_code.items[0].name, "France");

    let pct = repo.list(EntityType::Country, &ListFilter::new(None, Some("%".into()), None, None).unwrap()).await.unwrap();
    assert_eq!(pct.total, 0, "LIKE wildcards in input must be escaped");
}

#[tokio::test]
async fn database_check_rejects_three_attribute_keys() {
    let (_c, pool) = test_pool().await;
    let res = sqlx::query(
        "INSERT INTO master_data (entity_type, id, code, name, short_name, attributes, updated_by)
         VALUES ('age', 1, 'A', 'A', 'A', '{\"a\":\"1\",\"b\":\"2\",\"c\":\"3\"}', 't')",
    )
    .execute(&pool)
    .await;
    let err = res.unwrap_err();
    assert!(err.to_string().contains("attributes_max_two"), "{err}");
}

#[tokio::test]
async fn ping_succeeds() {
    let (_c, pool) = test_pool().await;
    PgMasterDataRepository::new(pool).ping().await.unwrap();
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test --test repository`
Expected: compile error, `postgres` module missing.

- [ ] **Step 5: Implement the Postgres repository**

`src/repository/postgres.rs`:
```rust
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{postgres::PgPoolOptions, PgPool, Postgres, QueryBuilder};

use crate::domain::{Attributes, EntityType, ListFilter, MasterRecord, NewRecord, Page, UpdateRecord};
use crate::repository::{MasterDataRepository, RepoError, UpdateOutcome};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn connect(url: &str, max_connections: u32, acquire_timeout: Duration) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(acquire_timeout)
        .connect(url)
        .await
}

#[derive(Clone)]
pub struct PgMasterDataRepository {
    pool: PgPool,
}

impl PgMasterDataRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    entity_type: EntityType,
    id: i64,
    code: String,
    name: String,
    short_name: String,
    attributes: serde_json::Value,
    version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    updated_by: String,
}

impl TryFrom<Row> for MasterRecord {
    type Error = RepoError;
    fn try_from(r: Row) -> Result<Self, RepoError> {
        let attributes = Attributes::from_json(r.attributes)
            .map_err(|e| RepoError::Database(sqlx::Error::Decode(Box::new(e))))?;
        Ok(MasterRecord {
            entity_type: r.entity_type,
            id: r.id,
            code: r.code,
            name: r.name,
            short_name: r.short_name,
            attributes,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
            updated_by: r.updated_by,
        })
    }
}

const UNIQUE_CODE_INDEX: &str = "master_data_code_live";

fn map_unique(err: sqlx::Error, code: &str) -> RepoError {
    match &err {
        sqlx::Error::Database(db)
            if db.is_unique_violation() && db.constraint() == Some(UNIQUE_CODE_INDEX) =>
        {
            RepoError::DuplicateCode(code.to_string())
        }
        _ => RepoError::Database(err),
    }
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

const COLS: &str = "entity_type, id, code, name, short_name, attributes, version, created_at, updated_at, updated_by";

fn push_filters<'a>(qb: &mut QueryBuilder<'a, Postgres>, t: EntityType, f: &'a ListFilter) {
    qb.push(" WHERE deleted_at IS NULL AND entity_type = ").push_bind(t);
    if let Some(code) = &f.code {
        qb.push(" AND code = ").push_bind(code);
    }
    if let Some(name) = &f.name {
        qb.push(" AND name ILIKE ").push_bind(format!("%{}%", escape_like(name)));
    }
}

#[async_trait]
impl MasterDataRepository for PgMasterDataRepository {
    async fn ping(&self) -> Result<(), RepoError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    async fn create(&self, t: EntityType, new: NewRecord, by: &str) -> Result<MasterRecord, RepoError> {
        let mut tx = self.pool.begin().await?;
        let id = sqlx::query_scalar!(
            r#"UPDATE id_counters SET next_id = next_id + 1
               WHERE entity_type = $1 RETURNING next_id - 1 AS "id!""#,
            t as EntityType
        )
        .fetch_one(&mut *tx)
        .await?;
        let code_for_error = new.code.clone();
        let row = sqlx::query_as!(
            Row,
            r#"INSERT INTO master_data (entity_type, id, code, name, short_name, attributes, updated_by)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               RETURNING entity_type AS "entity_type: EntityType", id, code, name, short_name,
                         attributes, version, created_at, updated_at, updated_by"#,
            t as EntityType,
            id,
            new.code,
            new.name,
            new.short_name,
            new.attributes.to_json(),
            by
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| map_unique(e, &code_for_error))?;
        tx.commit().await?;
        row.try_into()
    }

    async fn get(&self, t: EntityType, id: i64) -> Result<Option<MasterRecord>, RepoError> {
        let row = sqlx::query_as!(
            Row,
            r#"SELECT entity_type AS "entity_type: EntityType", id, code, name, short_name,
                      attributes, version, created_at, updated_at, updated_by
               FROM master_data
               WHERE entity_type = $1 AND id = $2 AND deleted_at IS NULL"#,
            t as EntityType,
            id
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    async fn list(&self, t: EntityType, f: &ListFilter) -> Result<Page<MasterRecord>, RepoError> {
        let mut count_qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT count(*) FROM master_data");
        push_filters(&mut count_qb, t, f);
        let total: i64 = count_qb.build_query_scalar().fetch_one(&self.pool).await?;

        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(format!("SELECT {COLS} FROM master_data"));
        push_filters(&mut qb, t, f);
        qb.push(" ORDER BY id LIMIT ").push_bind(f.limit).push(" OFFSET ").push_bind(f.offset);
        let rows: Vec<Row> = qb.build_query_as().fetch_all(&self.pool).await?;
        let items = rows.into_iter().map(TryInto::try_into).collect::<Result<Vec<_>, _>>()?;
        Ok(Page { items, limit: f.limit, offset: f.offset, total })
    }

    async fn update(
        &self,
        t: EntityType,
        id: i64,
        expected_version: i32,
        upd: UpdateRecord,
        by: &str,
    ) -> Result<UpdateOutcome, RepoError> {
        let code_for_error = upd.code.clone();
        let row = sqlx::query_as!(
            Row,
            r#"UPDATE master_data
               SET code = $4, name = $5, short_name = $6, attributes = $7,
                   version = version + 1, updated_at = now(), updated_by = $8
               WHERE entity_type = $1 AND id = $2 AND version = $3 AND deleted_at IS NULL
               RETURNING entity_type AS "entity_type: EntityType", id, code, name, short_name,
                         attributes, version, created_at, updated_at, updated_by"#,
            t as EntityType,
            id,
            expected_version,
            upd.code,
            upd.name,
            upd.short_name,
            upd.attributes.to_json(),
            by
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| map_unique(e, &code_for_error))?;
        if let Some(row) = row {
            return Ok(UpdateOutcome::Updated(row.try_into()?));
        }
        let current = sqlx::query_scalar!(
            "SELECT version FROM master_data WHERE entity_type = $1 AND id = $2 AND deleted_at IS NULL",
            t as EntityType,
            id
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(match current {
            Some(current) => UpdateOutcome::VersionConflict { current },
            None => UpdateOutcome::NotFound,
        })
    }

    async fn soft_delete(&self, t: EntityType, id: i64, by: &str) -> Result<bool, RepoError> {
        let result = sqlx::query!(
            "UPDATE master_data SET deleted_at = now(), updated_at = now(), updated_by = $3
             WHERE entity_type = $1 AND id = $2 AND deleted_at IS NULL",
            t as EntityType,
            id,
            by
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --test repository` (Docker must be running; each test starts its own container, ~3 s each)
Expected: 8 passed.

- [ ] **Step 7: Generate the offline query cache and commit**

```bash
cargo sqlx prepare
SQLX_OFFLINE=true cargo check
git add -A
git commit -m "feat: migration, repository trait, Postgres repository with tests" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: In-memory repository and service layer

**Files:**
- Create: `src/repository/memory.rs`, `src/service.rs`
- Modify: `src/repository/mod.rs`, `src/lib.rs`

**Interfaces:**
- Consumes: Task 4 trait, Task 3 domain, Task 2 `AppError`/`FieldError`.
- Produces:
  `repository::memory::InMemoryRepository::new() -> Self` (implements `MasterDataRepository`; same semantics as Postgres: per-type dense ids, live-code uniqueness, soft delete, version bump);
  `service::RecordInput { code: String, name: String, short_name: String, attributes: BTreeMap<String,String> }`;
  `service::MasterDataService::new(Arc<dyn MasterDataRepository>) -> Self`;
  methods `create(&self, t, RecordInput, by: &str) -> Result<MasterRecord, AppError>`, `get(&self, t, id) -> Result<MasterRecord, AppError>`, `list(&self, t, &ListFilter) -> Result<Page<MasterRecord>, AppError>`, `update(&self, t, id, expected_version: i32, RecordInput, by) -> Result<MasterRecord, AppError>`, `delete(&self, t, id, by) -> Result<(), AppError>`, `ready(&self) -> Result<(), AppError>`;
  `service::validate(RecordInput) -> Result<RecordFields, Vec<FieldError>>`.

- [ ] **Step 1: Write the failing service tests**

`src/service.rs` (tests only for now):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::memory::InMemoryRepository;
    use std::collections::BTreeMap;

    fn svc() -> MasterDataService {
        MasterDataService::new(Arc::new(InMemoryRepository::new()))
    }

    fn input(code: &str, name: &str, short: &str, attrs: &[(&str, &str)]) -> RecordInput {
        RecordInput {
            code: code.into(),
            name: name.into(),
            short_name: short.into(),
            attributes: attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[tokio::test]
    async fn create_normalises_code_and_trims_fields() {
        let s = svc();
        let rec = s.create(EntityType::Country, input("  us ", " United States ", " USA ", &[]), "me").await.unwrap();
        assert_eq!(rec.code, "US");
        assert_eq!(rec.name, "United States");
        assert_eq!(rec.short_name, "USA");
        assert_eq!(rec.updated_by, "me");
        assert_eq!(rec.id, 1);
    }

    #[tokio::test]
    async fn validation_names_every_bad_field() {
        let errs = validate(input("   ", "", &"x".repeat(65), &[("a", "1"), ("b", "2"), ("c", "3")])).unwrap_err();
        let fields: Vec<&str> = errs.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, vec!["code", "name", "shortName", "attributes"]);
    }

    #[tokio::test]
    async fn validation_enforces_lengths_and_blank_attribute_key() {
        assert!(validate(input(&"c".repeat(64), "n", "s", &[])).is_ok());
        assert_eq!(validate(input(&"c".repeat(65), "n", "s", &[])).unwrap_err()[0].field, "code");
        assert_eq!(validate(input("c", &"n".repeat(257), "s", &[])).unwrap_err()[0].field, "name");
        assert_eq!(validate(input("c", "n", "s", &[(" ", "v")])).unwrap_err()[0].field, "attributes");
    }

    #[tokio::test]
    async fn get_missing_is_not_found_and_duplicate_is_conflict() {
        let s = svc();
        assert!(matches!(s.get(EntityType::Age, 1).await.unwrap_err(), AppError::NotFound));
        s.create(EntityType::Age, input("A", "Adult", "A", &[]), "m").await.unwrap();
        let err = s.create(EntityType::Age, input("a", "Adult2", "A", &[]), "m").await.unwrap_err();
        assert!(matches!(err, AppError::DuplicateCode(ref c) if c == "A"));
    }

    #[tokio::test]
    async fn update_maps_outcomes() {
        let s = svc();
        let rec = s.create(EntityType::Geography, input("EU", "Europe", "EU", &[]), "m").await.unwrap();
        let upd = s.update(EntityType::Geography, rec.id, 1, input("EU", "Europe!", "EU", &[]), "m2").await.unwrap();
        assert_eq!((upd.version, upd.name.as_str(), upd.updated_by.as_str()), (2, "Europe!", "m2"));
        let err = s.update(EntityType::Geography, rec.id, 1, input("EU", "x", "EU", &[]), "m").await.unwrap_err();
        assert!(matches!(err, AppError::VersionConflict { expected: 1, current: 2 }));
        let err = s.update(EntityType::Geography, 99, 1, input("EU", "x", "EU", &[]), "m").await.unwrap_err();
        assert!(matches!(err, AppError::NotFound));
    }

    #[tokio::test]
    async fn delete_is_idempotent_and_hides_record() {
        let s = svc();
        let rec = s.create(EntityType::ProductTier, input("T1", "Tier 1", "T1", &[]), "m").await.unwrap();
        s.delete(EntityType::ProductTier, rec.id, "m").await.unwrap();
        s.delete(EntityType::ProductTier, rec.id, "m").await.unwrap();
        assert!(matches!(s.get(EntityType::ProductTier, rec.id).await.unwrap_err(), AppError::NotFound));
        let page = s.list(EntityType::ProductTier, &ListFilter::new(None, None, None, None).unwrap()).await.unwrap();
        assert_eq!(page.total, 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test service`
Expected: compile errors (`MasterDataService`, `InMemoryRepository` missing).

- [ ] **Step 3: Implement the in-memory repository**

`src/repository/memory.rs`:
```rust
use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;

use crate::domain::{EntityType, ListFilter, MasterRecord, NewRecord, Page, UpdateRecord};
use crate::repository::{MasterDataRepository, RepoError, UpdateOutcome};

/// Test double with the same semantics as the Postgres repository.
#[derive(Default)]
pub struct InMemoryRepository {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    rows: HashMap<(EntityType, i64), (MasterRecord, bool /* deleted */)>,
    next_id: HashMap<EntityType, i64>,
}

impl InMemoryRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Inner {
    fn live_code_exists(&self, t: EntityType, code: &str, except_id: Option<i64>) -> bool {
        self.rows.values().any(|(r, deleted)| {
            !deleted && r.entity_type == t && r.code == code && Some(r.id) != except_id
        })
    }
}

#[async_trait]
impl MasterDataRepository for InMemoryRepository {
    async fn ping(&self) -> Result<(), RepoError> {
        Ok(())
    }

    async fn create(&self, t: EntityType, new: NewRecord, by: &str) -> Result<MasterRecord, RepoError> {
        let mut g = self.inner.lock().unwrap();
        if g.live_code_exists(t, &new.code, None) {
            return Err(RepoError::DuplicateCode(new.code));
        }
        let id = {
            let n = g.next_id.entry(t).or_insert(1);
            let id = *n;
            *n += 1;
            id
        };
        let now = Utc::now();
        let rec = MasterRecord {
            entity_type: t,
            id,
            code: new.code,
            name: new.name,
            short_name: new.short_name,
            attributes: new.attributes,
            version: 1,
            created_at: now,
            updated_at: now,
            updated_by: by.to_string(),
        };
        g.rows.insert((t, id), (rec.clone(), false));
        Ok(rec)
    }

    async fn get(&self, t: EntityType, id: i64) -> Result<Option<MasterRecord>, RepoError> {
        let g = self.inner.lock().unwrap();
        Ok(g.rows.get(&(t, id)).filter(|(_, d)| !d).map(|(r, _)| r.clone()))
    }

    async fn list(&self, t: EntityType, f: &ListFilter) -> Result<Page<MasterRecord>, RepoError> {
        let g = self.inner.lock().unwrap();
        let mut all: Vec<MasterRecord> = g
            .rows
            .values()
            .filter(|(r, d)| !d && r.entity_type == t)
            .filter(|(r, _)| f.code.as_ref().is_none_or(|c| &r.code == c))
            .filter(|(r, _)| {
                f.name.as_ref().is_none_or(|n| r.name.to_lowercase().contains(&n.to_lowercase()))
            })
            .map(|(r, _)| r.clone())
            .collect();
        all.sort_by_key(|r| r.id);
        let total = all.len() as i64;
        let items = all.into_iter().skip(f.offset as usize).take(f.limit as usize).collect();
        Ok(Page { items, limit: f.limit, offset: f.offset, total })
    }

    async fn update(
        &self,
        t: EntityType,
        id: i64,
        expected_version: i32,
        upd: UpdateRecord,
        by: &str,
    ) -> Result<UpdateOutcome, RepoError> {
        let mut g = self.inner.lock().unwrap();
        if g.live_code_exists(t, &upd.code, Some(id)) {
            return Err(RepoError::DuplicateCode(upd.code));
        }
        let Some((rec, deleted)) = g.rows.get_mut(&(t, id)) else {
            return Ok(UpdateOutcome::NotFound);
        };
        if *deleted {
            return Ok(UpdateOutcome::NotFound);
        }
        if rec.version != expected_version {
            return Ok(UpdateOutcome::VersionConflict { current: rec.version });
        }
        rec.code = upd.code;
        rec.name = upd.name;
        rec.short_name = upd.short_name;
        rec.attributes = upd.attributes;
        rec.version += 1;
        rec.updated_at = Utc::now();
        rec.updated_by = by.to_string();
        Ok(UpdateOutcome::Updated(rec.clone()))
    }

    async fn soft_delete(&self, t: EntityType, id: i64, by: &str) -> Result<bool, RepoError> {
        let mut g = self.inner.lock().unwrap();
        match g.rows.get_mut(&(t, id)) {
            Some((rec, deleted)) if !*deleted => {
                *deleted = true;
                rec.updated_by = by.to_string();
                rec.updated_at = Utc::now();
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
```

Add `pub mod memory;` at the top of `src/repository/mod.rs`, and `pub mod service;` to `src/lib.rs`.

- [ ] **Step 4: Implement the service**

Put above the tests in `src/service.rs`:
```rust
use std::collections::BTreeMap;
use std::sync::Arc;

use tracing::instrument;

use crate::domain::{
    normalize_code, Attributes, EntityType, FieldError, ListFilter, MasterRecord, Page, RecordFields,
};
use crate::error::AppError;
use crate::repository::{MasterDataRepository, RepoError, UpdateOutcome};

/// Raw client input before normalisation and validation.
#[derive(Debug, Clone)]
pub struct RecordInput {
    pub code: String,
    pub name: String,
    pub short_name: String,
    pub attributes: BTreeMap<String, String>,
}

pub const CODE_MAX: usize = 64;
pub const NAME_MAX: usize = 256;
pub const SHORT_NAME_MAX: usize = 64;

fn check_text(field: &str, value: &str, max: usize, errors: &mut Vec<FieldError>) {
    if value.is_empty() {
        errors.push(FieldError::new(field, "must not be blank"));
    } else if value.chars().count() > max {
        errors.push(FieldError::new(field, format!("must be at most {max} characters")));
    }
}

pub fn validate(input: RecordInput) -> Result<RecordFields, Vec<FieldError>> {
    let mut errors = Vec::new();
    let code = normalize_code(&input.code);
    let name = input.name.trim().to_string();
    let short_name = input.short_name.trim().to_string();
    check_text("code", &code, CODE_MAX, &mut errors);
    check_text("name", &name, NAME_MAX, &mut errors);
    check_text("shortName", &short_name, SHORT_NAME_MAX, &mut errors);
    let attributes = match Attributes::new(input.attributes) {
        Ok(a) => a,
        Err(e) => {
            errors.push(FieldError::new("attributes", e.to_string()));
            Attributes::default()
        }
    };
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(RecordFields { code, name, short_name, attributes })
}

impl From<RepoError> for AppError {
    fn from(e: RepoError) -> Self {
        match e {
            RepoError::DuplicateCode(c) => AppError::DuplicateCode(c),
            RepoError::Database(e) => AppError::Internal(e.into()),
        }
    }
}

pub struct MasterDataService {
    repo: Arc<dyn MasterDataRepository>,
}

impl MasterDataService {
    pub fn new(repo: Arc<dyn MasterDataRepository>) -> Self {
        Self { repo }
    }

    pub async fn ready(&self) -> Result<(), AppError> {
        Ok(self.repo.ping().await?)
    }

    #[instrument(skip(self, input), fields(entity_type = t.as_str()))]
    pub async fn create(&self, t: EntityType, input: RecordInput, by: &str) -> Result<MasterRecord, AppError> {
        let fields = validate(input).map_err(AppError::Validation)?;
        Ok(self.repo.create(t, fields, by).await?)
    }

    #[instrument(skip(self), fields(entity_type = t.as_str()))]
    pub async fn get(&self, t: EntityType, id: i64) -> Result<MasterRecord, AppError> {
        self.repo.get(t, id).await?.ok_or(AppError::NotFound)
    }

    #[instrument(skip(self, filter), fields(entity_type = t.as_str()))]
    pub async fn list(&self, t: EntityType, filter: &ListFilter) -> Result<Page<MasterRecord>, AppError> {
        Ok(self.repo.list(t, filter).await?)
    }

    #[instrument(skip(self, input), fields(entity_type = t.as_str()))]
    pub async fn update(
        &self,
        t: EntityType,
        id: i64,
        expected_version: i32,
        input: RecordInput,
        by: &str,
    ) -> Result<MasterRecord, AppError> {
        let fields = validate(input).map_err(AppError::Validation)?;
        match self.repo.update(t, id, expected_version, fields, by).await? {
            UpdateOutcome::Updated(rec) => Ok(rec),
            UpdateOutcome::NotFound => Err(AppError::NotFound),
            UpdateOutcome::VersionConflict { current } => {
                Err(AppError::VersionConflict { expected: expected_version, current })
            }
        }
    }

    #[instrument(skip(self), fields(entity_type = t.as_str()))]
    pub async fn delete(&self, t: EntityType, id: i64, by: &str) -> Result<(), AppError> {
        self.repo.soft_delete(t, id, by).await?;
        Ok(())
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test service`
Expected: 6 passed.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: service layer with validation and in-memory repository" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: JWT auth extractor

**Files:**
- Create: `src/auth.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `AppError::{Unauthorized, Forbidden}`.
- Produces:
  `auth::JwtKeys::new(secret: &str, write_scope: &str) -> Self` (Clone);
  `JwtKeys::issue(&self, sub: &str, scopes: &[&str], ttl: Duration) -> String` (used by tests and local tooling);
  `JwtKeys::verify(&self, token: &str) -> Result<AuthContext, AppError>`;
  `auth::AuthContext { subject: String, scopes: Vec<String> }`;
  `auth::WriteAuth(pub AuthContext)` implementing `FromRequestParts<S>` where `S: FromRef<JwtKeys>`-style access via `JwtKeys: FromRef<S>`; missing/invalid token → 401, valid without write scope → 403.

- [ ] **Step 1: Write the failing tests**

`src/auth.rs` tests:
```rust
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
        let r = Router::new().route("/w", post(h)).with_state(S { keys: keys.clone() });
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
        let tok = keys.issue_at("svc", &["masterdata:write"], Utc::now() - chrono::Duration::hours(2), Duration::from_secs(60));
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
        let tok = keys.issue("svc-catalog", &["other", "masterdata:write"], Duration::from_secs(60));
        let (status, body) = call(r, Some(&format!("Bearer {tok}"))).await;
        assert_eq!(status, 200);
        assert_eq!(body, "svc-catalog");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test auth`
Expected: compile errors.

- [ ] **Step 3: Implement**

Add `pub mod auth;` to `src/lib.rs`. Above the tests in `src/auth.rs`:
```rust
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
            scope: if scopes.is_empty() { None } else { Some(scopes.join(" ")) },
        };
        encode(&Header::new(Algorithm::HS256), &claims, &self.encoding).expect("HS256 encode")
    }

    pub fn verify(&self, token: &str) -> Result<AuthContext, AppError> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_required_spec_claims(&["exp", "sub"]);
        let data = decode::<Claims>(token, &self.decoding, &validation)
            .map_err(|e| AppError::Unauthorized(format!("invalid token: {e}")))?;
        let scopes = data
            .claims
            .scope
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        Ok(AuthContext { subject: data.claims.sub, scopes })
    }
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
        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| AppError::Unauthorized("expected Bearer token".into()))?;
        let ctx = keys.verify(token.trim())?;
        if !ctx.scopes.iter().any(|s| s == keys.write_scope()) {
            return Err(AppError::Forbidden(format!("scope '{}' required", keys.write_scope())));
        }
        Ok(WriteAuth(ctx))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test auth`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: HS256 JWT write-auth extractor" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: API layer — extractors, DTOs, generic handlers, per-entity routes, app builder

**Files:**
- Create: `src/api/mod.rs`, `src/api/extract.rs`, `src/api/dto.rs`, `src/api/entity.rs`, `src/api/handlers.rs`, `src/api/routes.rs`, `src/telemetry.rs`, `tests/handlers.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `service::{MasterDataService, RecordInput}`, `auth::{JwtKeys, WriteAuth}`, `domain::*`, `error::*`.
- Produces:
  `api::AppState { service: Arc<MasterDataService>, jwt: JwtKeys }` (Clone; `impl FromRef<AppState> for JwtKeys`);
  `api::build_app(state: AppState, request_timeout: Duration) -> Router` (all routes, layers, `/health`, `/ready`);
  `api::entity::Entity` trait `{ const TYPE: EntityType; const PATH: &'static str; const TAG: &'static str; }` and ten marker structs `Country, Currency, Language, Category, Gender, Age, Geography, ProductLine, BusinessUnit, ProductTier`;
  `api::entity::ALL_PATHS: [(EntityType, &str, &str); 10]` (type, path, tag) for OpenAPI;
  `api::dto::{RecordRequest, RecordResponse, PageResponse, ListQuery}`;
  `api::extract::{AppJson<T>, AppQuery<T>, AppPath<T>, IfMatch}`;
  `telemetry::init(format: LogFormat)`.

- [ ] **Step 1: Write the failing handler tests**

`tests/handlers.rs`:
```rust
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use axum_master_data_api::{
    api::{build_app, AppState},
    auth::JwtKeys,
    error::ProblemDetails,
    repository::memory::InMemoryRepository,
    service::MasterDataService,
};
use serde_json::{json, Value};
use tower::ServiceExt;

fn app() -> (Router, JwtKeys) {
    let jwt = JwtKeys::new("test-secret", "masterdata:write");
    let state = AppState {
        service: Arc::new(MasterDataService::new(Arc::new(InMemoryRepository::new()))),
        jwt: jwt.clone(),
    };
    (build_app(state, Duration::from_secs(5)), jwt)
}

fn token(keys: &JwtKeys) -> String {
    format!("Bearer {}", keys.issue("tester", &["masterdata:write"], Duration::from_secs(60)))
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value, axum::http::HeaderMap) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, body, headers)
}

fn json_req(method: &str, uri: &str, auth: Option<&str>, body: Value) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    if let Some(a) = auth {
        b = b.header("authorization", a);
    }
    b.body(Body::from(body.to_string())).unwrap()
}

fn get_req(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn create_then_get_round_trip() {
    let (app, keys) = app();
    let body = json!({"code": "us", "name": "United States", "shortName": "USA", "attributes": {"iso3": "USA"}});
    let (status, created, headers) = send(&app, json_req("POST", "/api/v1/countries", Some(&token(&keys)), body)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(headers["location"], "/api/v1/countries/1");
    assert_eq!(created["code"], "US");
    assert_eq!(created["version"], 1);
    assert_eq!(created["updatedBy"], "tester");
    assert!(created.get("createdAt").is_some());

    let (status, fetched, _) = send(&app, get_req("/api/v1/countries/1")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched, created);
}

#[tokio::test]
async fn entities_are_isolated_and_all_ten_routes_exist() {
    let (app, keys) = app();
    let paths = [
        "countries", "currencies", "languages", "categories", "genders",
        "ages", "geographies", "product-lines", "business-units", "product-tiers",
    ];
    for p in paths {
        let body = json!({"code": "X", "name": "X", "shortName": "X"});
        let (status, created, _) = send(&app, json_req("POST", &format!("/api/v1/{p}"), Some(&token(&keys)), body)).await;
        assert_eq!(status, StatusCode::CREATED, "{p}");
        assert_eq!(created["id"], 1, "{p} ids are per entity");
    }
    let (status, _, _) = send(&app, get_req("/api/v1/nope")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn writes_require_auth_and_scope() {
    let (app, keys) = app();
    let body = json!({"code": "X", "name": "X", "shortName": "X"});
    let (status, p, headers) = send(&app, json_req("POST", "/api/v1/ages", None, body.clone())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers["content-type"], "application/problem+json");
    assert_eq!(p["instance"], "/api/v1/ages");
    let ro = format!("Bearer {}", keys.issue("ro", &["masterdata:read"], Duration::from_secs(60)));
    let (status, _, _) = send(&app, json_req("POST", "/api/v1/ages", Some(&ro), body)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn validation_errors_are_problem_json_with_fields() {
    let (app, keys) = app();
    let body = json!({"code": " ", "name": "N", "shortName": "S", "attributes": {"a": "1", "b": "2", "c": "3"}});
    let (status, p, _) = send(&app, json_req("POST", "/api/v1/categories", Some(&token(&keys)), body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let fields: Vec<&str> = p["errors"].as_array().unwrap().iter().map(|e| e["field"].as_str().unwrap()).collect();
    assert_eq!(fields, vec!["code", "attributes"]);
}

#[tokio::test]
async fn malformed_bodies_are_422_problem_json() {
    let (app, keys) = app();
    let t = token(&keys);
    // unknown field
    let (status, p, _) = send(&app, json_req("POST", "/api/v1/genders", Some(&t), json!({"code": "X", "name": "X", "shortName": "X", "extra": 1}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(p["status"], 422);
    // non-string attribute value
    let (status, _, _) = send(&app, json_req("POST", "/api/v1/genders", Some(&t), json!({"code": "X", "name": "X", "shortName": "X", "attributes": {"a": 1}}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // server-owned field supplied
    let (status, _, _) = send(&app, json_req("POST", "/api/v1/genders", Some(&t), json!({"code": "X", "name": "X", "shortName": "X", "updatedBy": "me"}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // not JSON at all
    let req = Request::post("/api/v1/genders").header("authorization", &t).header("content-type", "application/json").body(Body::from("{nope")).unwrap();
    let (status, p, _) = send(&app, req).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(p["errors"][0]["field"], "body");
}

#[tokio::test]
async fn non_numeric_id_is_422_and_missing_is_404() {
    let (app, _) = app();
    let (status, p, _) = send(&app, get_req("/api/v1/countries/abc")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(p["errors"][0]["field"], "id");
    let (status, p, _) = send(&app, get_req("/api/v1/countries/7")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(p["instance"], "/api/v1/countries/7");
}

#[tokio::test]
async fn list_filters_and_rejects_bad_params() {
    let (app, keys) = app();
    let t = token(&keys);
    for (c, n) in [("US", "United States"), ("GB", "United Kingdom"), ("FR", "France")] {
        send(&app, json_req("POST", "/api/v1/countries", Some(&t), json!({"code": c, "name": n, "shortName": c}))).await;
    }
    let (status, page, _) = send(&app, get_req("/api/v1/countries?name=united&limit=1&offset=1")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 2);
    assert_eq!(page["limit"], 1);
    assert_eq!(page["offset"], 1);
    assert_eq!(page["items"][0]["code"], "GB");

    let (status, page, _) = send(&app, get_req("/api/v1/countries?code=fr")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"][0]["name"], "France");

    for q in ["limit=0", "limit=501", "offset=-1", "limit=abc"] {
        let (status, _, _) = send(&app, get_req(&format!("/api/v1/countries?{q}"))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{q}");
    }
}

#[tokio::test]
async fn update_requires_if_match_and_checks_version() {
    let (app, keys) = app();
    let t = token(&keys);
    send(&app, json_req("POST", "/api/v1/currencies", Some(&t), json!({"code": "USD", "name": "Dollar", "shortName": "USD"}))).await;
    let body = json!({"code": "USD", "name": "US Dollar", "shortName": "USD", "attributes": {"symbol": "$"}});

    let (status, p, _) = send(&app, json_req("PUT", "/api/v1/currencies/1", Some(&t), body.clone())).await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    assert!(p["detail"].as_str().unwrap().contains("If-Match"));

    for bad in ["abc", "3", "\"x\"", "\"\""] {
        let req = Request::put("/api/v1/currencies/1")
            .header("authorization", &t)
            .header("content-type", "application/json")
            .header("if-match", bad)
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, p, _) = send(&app, req).await;
        assert_eq!(status, StatusCode::PRECONDITION_REQUIRED, "If-Match: {bad}");
        assert!(p["detail"].as_str().unwrap().contains("quoted integer"), "{bad}");
    }

    let ok = |v: &str| {
        Request::put("/api/v1/currencies/1")
            .header("authorization", &t)
            .header("content-type", "application/json")
            .header("if-match", format!("\"{v}\""))
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let (status, rec, _) = send(&app, ok("1")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rec["version"], 2);
    assert_eq!(rec["attributes"]["symbol"], "$");

    let (status, p, _) = send(&app, ok("1")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(p["detail"], "version mismatch: expected 1, current 2");
}

#[tokio::test]
async fn delete_is_idempotent_then_code_reusable() {
    let (app, keys) = app();
    let t = token(&keys);
    send(&app, json_req("POST", "/api/v1/languages", Some(&t), json!({"code": "EN", "name": "English", "shortName": "EN"}))).await;
    let del = || Request::delete("/api/v1/languages/1").header("authorization", &t).body(Body::empty()).unwrap();
    assert_eq!(send(&app, del()).await.0, StatusCode::NO_CONTENT);
    assert_eq!(send(&app, del()).await.0, StatusCode::NO_CONTENT);
    assert_eq!(send(&app, get_req("/api/v1/languages/1")).await.0, StatusCode::NOT_FOUND);
    let (status, rec, _) = send(&app, json_req("POST", "/api/v1/languages", Some(&t), json!({"code": "EN", "name": "English", "shortName": "EN"}))).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(rec["id"], 2);
}

#[tokio::test]
async fn duplicate_code_is_409() {
    let (app, keys) = app();
    let t = token(&keys);
    let body = json!({"code": "X", "name": "X", "shortName": "X"});
    send(&app, json_req("POST", "/api/v1/business-units", Some(&t), body.clone())).await;
    let (status, p, _) = send(&app, json_req("POST", "/api/v1/business-units", Some(&t), body)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let p: ProblemDetails = serde_json::from_value(p).unwrap();
    assert_eq!(p.detail, "code 'X' already exists");
}

#[tokio::test]
async fn health_ready_and_request_id() {
    let (app, _) = app();
    let (status, _, _) = send(&app, get_req("/health")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(&app, get_req("/ready")).await;
    assert_eq!(status, StatusCode::OK);

    let req = Request::get("/health").header("x-request-id", "abc-123").body(Body::empty()).unwrap();
    let (_, _, headers) = send(&app, req).await;
    assert_eq!(headers["x-request-id"], "abc-123");
    let (_, _, headers) = send(&app, get_req("/health")).await;
    assert!(headers.get("x-request-id").is_some(), "generated id when none supplied");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test handlers`
Expected: compile error, `api` module missing.

- [ ] **Step 3: Implement `telemetry.rs`**

```rust
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::config::LogFormat;

pub fn init(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn"));
    let registry = tracing_subscriber::registry().with(filter);
    match format {
        LogFormat::Pretty => registry.with(fmt::layer()).init(),
        LogFormat::Json => registry.with(fmt::layer().json().flatten_event(true)).init(),
    }
}
```

- [ ] **Step 4: Implement `api/entity.rs`**

```rust
use crate::domain::EntityType;

/// Compile-time marker binding an entity type to its route path and OpenAPI tag.
pub trait Entity: Send + Sync + 'static {
    const TYPE: EntityType;
    const PATH: &'static str;
    const TAG: &'static str;
}

macro_rules! entities {
    ($( $name:ident => ($ty:expr, $path:literal, $tag:literal) ),* $(,)?) => {
        $(
            pub struct $name;
            impl Entity for $name {
                const TYPE: EntityType = $ty;
                const PATH: &'static str = $path;
                const TAG: &'static str = $tag;
            }
        )*
        pub const ALL_PATHS: [(EntityType, &str, &str); 10] = [ $( ($ty, $path, $tag) ),* ];
    };
}

entities! {
    Country      => (EntityType::Country,      "countries",      "Countries"),
    Currency     => (EntityType::Currency,     "currencies",     "Currencies"),
    Language     => (EntityType::Language,     "languages",      "Languages"),
    Category     => (EntityType::Category,     "categories",     "Categories"),
    Gender       => (EntityType::Gender,       "genders",        "Genders"),
    Age          => (EntityType::Age,          "ages",           "Ages"),
    Geography    => (EntityType::Geography,    "geographies",    "Geographies"),
    ProductLine  => (EntityType::ProductLine,  "product-lines",  "Product Lines"),
    BusinessUnit => (EntityType::BusinessUnit, "business-units", "Business Units"),
    ProductTier  => (EntityType::ProductTier,  "product-tiers",  "Product Tiers"),
}
```

- [ ] **Step 5: Implement `api/dto.rs`**

```rust
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::domain::{MasterRecord, Page};
use crate::service::RecordInput;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordRequest {
    pub code: String,
    pub name: String,
    pub short_name: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

impl From<RecordRequest> for RecordInput {
    fn from(r: RecordRequest) -> Self {
        RecordInput { code: r.code, name: r.name, short_name: r.short_name, attributes: r.attributes }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecordResponse {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub short_name: String,
    pub attributes: BTreeMap<String, String>,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

impl From<MasterRecord> for RecordResponse {
    fn from(r: MasterRecord) -> Self {
        RecordResponse {
            id: r.id,
            code: r.code,
            name: r.name,
            short_name: r.short_name,
            attributes: r.attributes.as_map().clone(),
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
            updated_by: r.updated_by,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageResponse {
    pub items: Vec<RecordResponse>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

impl From<Page<MasterRecord>> for PageResponse {
    fn from(p: Page<MasterRecord>) -> Self {
        PageResponse {
            items: p.items.into_iter().map(Into::into).collect(),
            limit: p.limit,
            offset: p.offset,
            total: p.total,
        }
    }
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    pub code: Option<String>,
    pub name: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
```

- [ ] **Step 6: Implement `api/extract.rs`**

```rust
use axum::{
    extract::{
        rejection::JsonRejection,
        FromRequest, FromRequestParts, Json, Path, Query, Request,
    },
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
            Err(rej) => Err(AppError::Validation(vec![FieldError::new("body", body_text(&rej))])),
        }
    }
}

fn body_text(rej: &JsonRejection) -> String {
    match rej {
        JsonRejection::JsonDataError(e) => e.body_text(),
        JsonRejection::JsonSyntaxError(e) => e.body_text(),
        JsonRejection::MissingJsonContentType(_) => "expected Content-Type: application/json".to_string(),
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
            Err(rej) => Err(AppError::Validation(vec![FieldError::new("query", rej.body_text())])),
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
```

- [ ] **Step 7: Implement `api/handlers.rs`**

```rust
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
    let rec = state.service.update(E::TYPE, id, version, body.into(), &ctx.subject).await?;
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

pub async fn health() -> StatusCode {
    StatusCode::OK
}

pub async fn ready(State(state): State<AppState>) -> Result<StatusCode, AppError> {
    state.service.ready().await.map_err(|e| {
        tracing::warn!(error = %e, "readiness check failed");
        e
    })?;
    Ok(StatusCode::OK)
}
```

Note on extractor order in `update`: `AppJson` consumes the body so it must be last; `WriteAuth` runs before the body is read, so an unauthenticated request never parses JSON.

- [ ] **Step 8: Implement `api/routes.rs`**

```rust
use axum::{
    routing::{get, post},
    Router,
};

use crate::api::{
    entity::{
        Age, BusinessUnit, Category, Country, Currency, Entity, Gender, Geography, Language,
        ProductLine, ProductTier,
    },
    handlers, AppState,
};

fn entity_router<E: Entity>() -> Router<AppState> {
    Router::new()
        // Empty path (not "/") so the collection serves `/api/v1/{PATH}`
        // without a trailing slash when nested. `"/"` would only match
        // `/api/v1/{PATH}/` and break POST/GET collection tests.
        .route("", post(handlers::create::<E>).get(handlers::list::<E>))
        .route(
            "/{id}",
            get(handlers::get_one::<E>)
                .put(handlers::update::<E>)
                .delete(handlers::delete::<E>),
        )
}

fn mount<E: Entity>(router: Router<AppState>) -> Router<AppState> {
    router.nest(&format!("/api/v1/{}", E::PATH), entity_router::<E>())
}

pub fn entity_routes() -> Router<AppState> {
    let r = Router::new();
    let r = mount::<Country>(r);
    let r = mount::<Currency>(r);
    let r = mount::<Language>(r);
    let r = mount::<Category>(r);
    let r = mount::<Gender>(r);
    let r = mount::<Age>(r);
    let r = mount::<Geography>(r);
    let r = mount::<ProductLine>(r);
    let r = mount::<BusinessUnit>(r);
    mount::<ProductTier>(r)
}
```

- [ ] **Step 9: Implement `api/mod.rs` with `build_app` and layers**

```rust
pub mod dto;
pub mod entity;
pub mod extract;
pub mod handlers;
pub mod routes;

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{FromRef, Request},
    http::{header::HeaderName, HeaderValue},
    middleware,
    routing::get,
    Router,
};
use tower::ServiceBuilder;
use tower_http::{
    cors::CorsLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::auth::JwtKeys;
use crate::error::add_problem_instance;
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
        .layer(middleware::from_fn(add_problem_instance))
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::new(REQUEST_ID.clone(), MakeRequestUuid))
                .layer(TraceLayer::new_for_http().make_span_with(make_span))
                .layer(PropagateRequestIdLayer::new(REQUEST_ID.clone()))
                .layer(TimeoutLayer::new(request_timeout))
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}
```

Add `pub mod api;` and `pub mod telemetry;` to `src/lib.rs`.

- [ ] **Step 10: Run tests to verify they pass**

Run: `cargo test --test handlers`
Expected: 11 passed. Also run `cargo clippy --all-targets` and fix warnings.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "feat: axum API with per-entity routes, extractors, and middleware" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: OpenAPI document, Swagger UI, and `main.rs` startup

**Files:**
- Create: `src/api/openapi.rs`
- Modify: `src/api/mod.rs`, `src/main.rs`, `tests/handlers.rs`

**Interfaces:**
- Consumes: `api::entity::ALL_PATHS`, DTO `ToSchema` impls, `ProblemDetails`, `config::AppConfig`, `telemetry::init`, `repository::postgres::{connect, MIGRATOR, PgMasterDataRepository}`.
- Produces: `api::openapi::document() -> utoipa::openapi::OpenApi`; `build_app` additionally serves `GET /openapi.json` and Swagger UI at `/docs`; `main` boots the service.

- [ ] **Step 1: Write the failing test**

Append to `tests/handlers.rs`:
```rust
#[tokio::test]
async fn openapi_lists_every_entity_and_docs_served() {
    let (app, _) = app();
    let (status, doc, _) = send(&app, get_req("/openapi.json")).await;
    assert_eq!(status, StatusCode::OK);
    let paths = doc["paths"].as_object().unwrap();
    for p in ["countries", "currencies", "languages", "categories", "genders", "ages", "geographies", "product-lines", "business-units", "product-tiers"] {
        assert!(paths.contains_key(&format!("/api/v1/{p}")), "{p} collection");
        assert!(paths.contains_key(&format!("/api/v1/{p}/{{id}}")), "{p} item");
    }
    let put = &doc["paths"]["/api/v1/countries/{id}"]["put"];
    assert!(put["parameters"].as_array().unwrap().iter().any(|p| p["name"] == "If-Match"));
    assert!(put["security"].as_array().is_some());
    assert!(doc["components"]["schemas"].get("RecordResponse").is_some());
    assert!(doc["components"]["schemas"].get("ProblemDetails").is_some());

    let resp = app.clone().oneshot(get_req("/docs/")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test handlers openapi`
Expected: FAIL, `/openapi.json` returns 404.

- [ ] **Step 3: Implement `api/openapi.rs`**

The handlers are generic, so `#[utoipa::path]` cannot be used; the document is built with utoipa's builder API in a loop over `ALL_PATHS`. If a builder name does not match the pinned utoipa 5.x, check `cargo doc -p utoipa --open` under `utoipa::openapi`.

```rust
use utoipa::openapi::{
    path::{HttpMethod, OperationBuilder, ParameterBuilder, ParameterIn, PathItemBuilder},
    request_body::RequestBodyBuilder,
    schema::{ObjectBuilder, Type},
    security::{HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme},
    tag::TagBuilder,
    ComponentsBuilder, ContentBuilder, InfoBuilder, OpenApi, OpenApiBuilder, PathsBuilder, Ref, Required,
    ResponseBuilder,
};

use crate::api::dto::{PageResponse, RecordRequest, RecordResponse};
use crate::api::entity::ALL_PATHS;
use crate::domain::FieldError;
use crate::error::ProblemDetails;

const BEARER: &str = "bearerAuth";

fn json_response(desc: &str, schema: &str) -> utoipa::openapi::Response {
    ResponseBuilder::new()
        .description(desc)
        .content("application/json", ContentBuilder::new().schema(Some(Ref::from_schema_name(schema))).build())
        .build()
}

fn problem(desc: &str) -> utoipa::openapi::Response {
    ResponseBuilder::new()
        .description(desc)
        .content(
            "application/problem+json",
            ContentBuilder::new().schema(Some(Ref::from_schema_name("ProblemDetails"))).build(),
        )
        .build()
}

fn id_param() -> utoipa::openapi::path::Parameter {
    ParameterBuilder::new()
        .name("id")
        .parameter_in(ParameterIn::Path)
        .required(Required::True)
        .schema(Some(ObjectBuilder::new().schema_type(Type::Integer).build()))
        .build()
}

fn query_param(name: &str, ty: Type, desc: &str) -> utoipa::openapi::path::Parameter {
    ParameterBuilder::new()
        .name(name)
        .parameter_in(ParameterIn::Query)
        .required(Required::False)
        .description(Some(desc))
        .schema(Some(ObjectBuilder::new().schema_type(ty).build()))
        .build()
}

fn body() -> utoipa::openapi::request_body::RequestBody {
    RequestBodyBuilder::new()
        .required(Some(Required::True))
        .content("application/json", ContentBuilder::new().schema(Some(Ref::from_schema_name("RecordRequest"))).build())
        .build()
}

fn secured(op: OperationBuilder) -> OperationBuilder {
    op.security(SecurityRequirement::new(BEARER, Vec::<String>::new()))
        .response("401", problem("Missing or invalid token"))
        .response("403", problem("Token lacks write scope"))
}

pub fn document() -> OpenApi {
    let mut paths = PathsBuilder::new();
    let mut tags = Vec::new();

    for (_, path, tag) in ALL_PATHS {
        tags.push(TagBuilder::new().name(tag).build());
        let collection = format!("/api/v1/{path}");
        let item = format!("/api/v1/{path}/{{id}}");

        let list = OperationBuilder::new()
            .tag(tag)
            .summary(Some(format!("List {tag}")))
            .parameter(query_param("code", Type::String, "exact code match"))
            .parameter(query_param("name", Type::String, "case-insensitive substring"))
            .parameter(query_param("limit", Type::Integer, "1..=500, default 50"))
            .parameter(query_param("offset", Type::Integer, ">= 0, default 0"))
            .response("200", json_response("Page of records", "PageResponse"))
            .response("422", problem("Invalid query parameters"));

        let create = secured(
            OperationBuilder::new()
                .tag(tag)
                .summary(Some(format!("Create {tag}")))
                .request_body(Some(body()))
                .response("201", json_response("Created", "RecordResponse"))
                .response("409", problem("Duplicate code"))
                .response("422", problem("Validation failed")),
        );

        let get_one = OperationBuilder::new()
            .tag(tag)
            .summary(Some(format!("Get one of {tag}")))
            .parameter(id_param())
            .response("200", json_response("Record", "RecordResponse"))
            .response("404", problem("Not found"));

        let if_match = ParameterBuilder::new()
            .name("If-Match")
            .parameter_in(ParameterIn::Header)
            .required(Required::True)
            .description(Some("Quoted current version, e.g. \"3\""))
            .schema(Some(ObjectBuilder::new().schema_type(Type::String).build()))
            .build();

        let update = secured(
            OperationBuilder::new()
                .tag(tag)
                .summary(Some(format!("Replace one of {tag}")))
                .parameter(id_param())
                .parameter(if_match)
                .request_body(Some(body()))
                .response("200", json_response("Updated", "RecordResponse"))
                .response("404", problem("Not found"))
                .response("409", problem("Version mismatch or duplicate code"))
                .response("422", problem("Validation failed"))
                .response("428", problem("If-Match missing or malformed")),
        );

        let delete = secured(
            OperationBuilder::new()
                .tag(tag)
                .summary(Some(format!("Soft-delete one of {tag}")))
                .parameter(id_param())
                .response("204", ResponseBuilder::new().description("Deleted (idempotent)").build()),
        );

        paths = paths
            .path(
                collection,
                PathItemBuilder::new()
                    .operation(HttpMethod::Get, list.build())
                    .operation(HttpMethod::Post, create.build())
                    .build(),
            )
            .path(
                item,
                PathItemBuilder::new()
                    .operation(HttpMethod::Get, get_one.build())
                    .operation(HttpMethod::Put, update.build())
                    .operation(HttpMethod::Delete, delete.build())
                    .build(),
            );
    }

    let components = ComponentsBuilder::new()
        .schema_from::<RecordRequest>()
        .schema_from::<RecordResponse>()
        .schema_from::<PageResponse>()
        .schema_from::<ProblemDetails>()
        .schema_from::<FieldError>()
        .security_scheme(
            BEARER,
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).bearer_format("JWT").build()),
        )
        .build();

    OpenApiBuilder::new()
        .info(InfoBuilder::new().title("Master Data API").version(env!("CARGO_PKG_VERSION")).build())
        .paths(paths.build())
        .components(Some(components))
        .tags(Some(tags))
        .build()
}
```

- [ ] **Step 4: Serve it from `build_app`**

In `src/api/mod.rs` add `pub mod openapi;` and change the router construction to:
```rust
use utoipa_swagger_ui::SwaggerUi;

pub fn build_app(state: AppState, request_timeout: Duration) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/ready", get(handlers::ready))
        .merge(routes::entity_routes())
        .merge(SwaggerUi::new("/docs").url("/openapi.json", openapi::document()))
        .layer(middleware::from_fn(add_problem_instance))
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::new(REQUEST_ID.clone(), MakeRequestUuid))
                .layer(TraceLayer::new_for_http().make_span_with(make_span))
                .layer(PropagateRequestIdLayer::new(REQUEST_ID.clone()))
                .layer(TimeoutLayer::new(request_timeout))
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test handlers`
Expected: 12 passed.

- [ ] **Step 6: Implement `main.rs`**

```rust
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum_master_data_api::{
    api::{build_app, AppState},
    auth::JwtKeys,
    config::AppConfig,
    repository::postgres::{connect, PgMasterDataRepository, MIGRATOR},
    service::MasterDataService,
    telemetry,
};
use secrecy::ExposeSecret;
use tokio::signal;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = AppConfig::load().context("loading configuration")?;
    telemetry::init(cfg.log.format);

    let pool = connect(
        cfg.database.url.expose_secret(),
        cfg.database.max_connections,
        Duration::from_secs(cfg.database.acquire_timeout_secs),
    )
    .await
    .context("connecting to database")?;
    MIGRATOR.run(&pool).await.context("running migrations")?;

    let state = AppState {
        service: Arc::new(MasterDataService::new(Arc::new(PgMasterDataRepository::new(pool)))),
        jwt: JwtKeys::new(cfg.auth.jwt_secret.expose_secret(), &cfg.auth.write_scope),
    };
    let app = build_app(state, Duration::from_secs(cfg.server.request_timeout_secs));

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;
    tracing::info!("shut down cleanly");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async { signal::ctrl_c().await.expect("ctrl_c handler") };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
```

- [ ] **Step 7: Smoke-run against the dev database**

```bash
APP_DATABASE__URL=postgres://postgres:postgres@localhost:5432/master_data \
APP_AUTH__JWT_SECRET=dev-secret cargo run &
sleep 3
curl -s localhost:8080/health -i | head -1          # HTTP/1.1 200 OK
curl -s localhost:8080/ready  -i | head -1          # HTTP/1.1 200 OK
curl -s localhost:8080/api/v1/countries/1 -i | head -1   # HTTP/1.1 404 Not Found
kill %1
```

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: OpenAPI document, Swagger UI, and server startup" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 9: End-to-end tests against real Postgres

**Files:**
- Create: `tests/api.rs`
- Modify: `tests/common/mod.rs`

**Interfaces:**
- Consumes: `tests/common::test_pool`, `build_app`, `JwtKeys`, `PgMasterDataRepository`.
- Produces: `tests/common::spawn_app() -> TestApp { base_url: String, jwt: JwtKeys, _container: ContainerAsync<Postgres> }`.

- [ ] **Step 1: Add the app spawner to `tests/common/mod.rs`**

```rust
use std::sync::Arc;

use axum_master_data_api::{
    api::{build_app, AppState},
    auth::JwtKeys,
    repository::postgres::PgMasterDataRepository,
    service::MasterDataService,
};

pub struct TestApp {
    pub base_url: String,
    pub jwt: JwtKeys,
    pub _container: ContainerAsync<Postgres>,
}

impl TestApp {
    pub fn bearer(&self, sub: &str, scopes: &[&str]) -> String {
        format!("Bearer {}", self.jwt.issue(sub, scopes, Duration::from_secs(300)))
    }
}

/// Full app on a random port backed by a throwaway Postgres.
pub async fn spawn_app() -> TestApp {
    let (container, pool) = test_pool().await;
    let jwt = JwtKeys::new("e2e-secret", "masterdata:write");
    let state = AppState {
        service: Arc::new(MasterDataService::new(Arc::new(PgMasterDataRepository::new(pool)))),
        jwt: jwt.clone(),
    };
    let app = build_app(state, Duration::from_secs(10));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    TestApp { base_url: format!("http://{addr}"), jwt, _container: container }
}
```

- [ ] **Step 2: Write the end-to-end tests**

`tests/api.rs`:
```rust
mod common;

use common::{spawn_app, TestApp};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

async fn lifecycle(app: &TestApp, client: &Client, path: &str) {
    let base = format!("{}/api/v1/{path}", app.base_url);
    let auth = app.bearer("svc-e2e", &["masterdata:write"]);

    // create
    let resp = client
        .post(&base)
        .header("authorization", &auth)
        .json(&json!({"code": " ab ", "name": "Alpha Beta", "shortName": "AB", "attributes": {"k": "v"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "{path} create");
    let created: Value = resp.json().await.unwrap();
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["code"], "AB");
    assert_eq!(created["updatedBy"], "svc-e2e");

    // get
    let got: Value = client.get(format!("{base}/{id}")).send().await.unwrap().json().await.unwrap();
    assert_eq!(got, created);

    // list with filter
    let page: Value = client.get(format!("{base}?name=alpha")).send().await.unwrap().json().await.unwrap();
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["id"], id);

    // update ok
    let resp = client
        .put(format!("{base}/{id}"))
        .header("authorization", &auth)
        .header("if-match", "\"1\"")
        .json(&json!({"code": "AB", "name": "Alpha Beta 2", "shortName": "AB2", "attributes": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let updated: Value = resp.json().await.unwrap();
    assert_eq!(updated["version"], 2);
    assert_eq!(updated["attributes"], json!({}));

    // update stale
    let resp = client
        .put(format!("{base}/{id}"))
        .header("authorization", &auth)
        .header("if-match", "\"1\"")
        .json(&json!({"code": "AB", "name": "x", "shortName": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let problem: Value = resp.json().await.unwrap();
    assert_eq!(problem["instance"], format!("/api/v1/{path}/{id}"));

    // delete, get 404, recreate same code
    let resp = client.delete(format!("{base}/{id}")).header("authorization", &auth).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = client.get(format!("{base}/{id}")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = client
        .post(&base)
        .header("authorization", &auth)
        .json(&json!({"code": "AB", "name": "Alpha Beta", "shortName": "AB"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let again: Value = resp.json().await.unwrap();
    assert_eq!(again["id"], id + 1);
}

#[tokio::test]
async fn full_lifecycle_across_entities() {
    let app = spawn_app().await;
    let client = Client::new();
    lifecycle(&app, &client, "countries").await;
    lifecycle(&app, &client, "product-lines").await;
    lifecycle(&app, &client, "ages").await;
}

#[tokio::test]
async fn auth_is_enforced_over_the_wire() {
    let app = spawn_app().await;
    let client = Client::new();
    let base = format!("{}/api/v1/currencies", app.base_url);
    let body = json!({"code": "USD", "name": "Dollar", "shortName": "USD"});

    let resp = client.post(&base).json(&body).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");

    let ro = app.bearer("ro", &["masterdata:read"]);
    let resp = client.post(&base).header("authorization", ro).json(&body).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = client.get(&base).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "reads are open");
}

#[tokio::test]
async fn readiness_and_openapi_over_the_wire() {
    let app = spawn_app().await;
    let client = Client::new();
    assert_eq!(client.get(format!("{}/ready", app.base_url)).send().await.unwrap().status(), StatusCode::OK);
    let doc: Value = client.get(format!("{}/openapi.json", app.base_url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(doc["info"]["title"], "Master Data API");
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --test api`
Expected: 3 passed (each spawns its own container).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test: end-to-end API tests against Postgres" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 10: CI workflow and README

**Files:**
- Create: `.github/workflows/ci.yml`, `README.md`, `rustfmt.toml`

**Interfaces:** none; packaging only.

- [ ] **Step 1: Write the CI workflow**

`.github/workflows/ci.yml`:
```yaml
name: ci
on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  SQLX_OFFLINE: "true"

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.89.0
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets -- -D warnings

  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.89.0
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --all-targets

  sqlx-prepare-check:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_PASSWORD: postgres
          POSTGRES_DB: master_data
        ports: ["5432:5432"]
        options: >-
          --health-cmd pg_isready --health-interval 5s --health-timeout 5s --health-retries 10
    env:
      DATABASE_URL: postgres://postgres:postgres@localhost:5432/master_data
      SQLX_OFFLINE: "false"
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.89.0
      - uses: Swatinem/rust-cache@v2
      - run: cargo install sqlx-cli --no-default-features --features postgres,rustls --locked
      - run: sqlx migrate run
      - run: cargo sqlx prepare --check
```

`rustfmt.toml`:
```toml
max_width = 110
```

- [ ] **Step 2: Write the README**

`README.md`:
```markdown
# axum-master-data-api

CRUD REST API over ten master-data entities, one generic Postgres table.
Design: `specs/2026-10-07-master-data-api-design.md`.

## Run locally

    docker compose up -d
    sqlx migrate run            # cargo install sqlx-cli --no-default-features --features postgres,rustls
    APP_AUTH__JWT_SECRET=dev-secret \
    APP_DATABASE__URL=postgres://postgres:postgres@localhost:5432/master_data \
    cargo run

Swagger UI: http://localhost:8080/docs

## Test

    cargo test                  # unit + handler tests (no Docker)
    cargo test --test repository --test api   # needs Docker (testcontainers)

## Config

`config/default.toml` → `config/{APP_ENV}.toml` → env vars `APP_<SECTION>__<KEY>`.
Secrets (`APP_DATABASE__URL`, `APP_AUTH__JWT_SECRET`) come from env only.

## Queries

sqlx checks SQL at compile time. After editing any `query!`, run
`cargo sqlx prepare` and commit `.sqlx/`. CI builds offline.
```

- [ ] **Step 3: Verify everything passes end to end**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
SQLX_OFFLINE=true cargo check
```
Expected: no warnings, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "ci: GitHub Actions workflow and README" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```
