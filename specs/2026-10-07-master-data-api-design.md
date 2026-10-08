# Master Data REST API — Design

**Date:** 2026-10-07
**Status:** Approved for planning
**Repo:** `axum-master-data-api`

## 1. Purpose and intent

A Rust REST API providing CRUD over ten master-data entities (reference lists
such as countries or currencies). The project has two goals:

1. A working, production-shaped service: typed config, structured tracing,
   migrations, auth, tests.
2. A learning vehicle for comparing storage designs. Three were evaluated
   (Postgres table-per-entity, Postgres generic table, DynamoDB single-table);
   one is built. The repository trait keeps the other two as a bounded,
   local change rather than a rewrite.

**Consumers:** internal services. Reads are open; writes need a bearer JWT.

**Selection criterion:** performance for the actual workload, which is small
tables, read-heavy, lookups by `id` or `code`, occasional filtered list.

### Storage decision

| Option | Hot-path latency | Verdict |
|---|---|---|
| Postgres, table per entity | sub-ms index hit | Fast; 10 near-identical schemas |
| Postgres, one generic table | sub-ms index hit (`(entity_type, id)`) | **Chosen** — same speed, one query set |
| DynamoDB single-table | ~2–5 ms (network hop to service) | Scales horizontally, which master data never needs; no compile-time query checks |

Performance is a tie between the two Postgres layouts, so the generic table
wins on simpler code. An in-process read cache is the next performance lever
if ever needed; it is not built.

## 2. Requirements

Every entity has: `id` (integer, unique **per entity type**, dense), `code`,
`name`, `short_name` (strings), and `attributes`: a free-form map of at most
two string key/value pairs.

In scope:

- Create / get / list / update / delete, one route set per entity
- List pagination and filtering (`code` exact, `name` substring)
- Audit: `created_at`, `updated_at`, `updated_by`, `version`
- Optimistic locking on update via `If-Match`
- Soft delete
- Bearer JWT auth on mutating routes
- OpenAPI document and Swagger UI
- Health and readiness endpoints

Out of scope: `PATCH`, per-entity columns, metrics/OpenTelemetry export,
cursor pagination, hard delete, read cache, load testing, other storage
backends.

## 3. Stack

| Concern | Choice | Notes |
|---|---|---|
| Runtime / HTTP | tokio, axum 0.8, tower-http | `TraceLayer`, request-id, timeout, CORS layers |
| Database | sqlx 0.8 (postgres, macros, json, chrono) | compile-time `query!`; `.sqlx/` committed, `SQLX_OFFLINE=true` in CI |
| Migrations | `sqlx migrate` | run at startup |
| Config | figment | TOML files + `APP_` env overrides |
| Logging/tracing | tracing, tracing-subscriber (env-filter, fmt, json) | no `log` macros |
| Errors | thiserror (domain), anyhow (internal chain) | RFC 9457 problem+json responses |
| Auth | jsonwebtoken, HS256 | shared secret from env |
| OpenAPI | utoipa + utoipa-swagger-ui | |
| Serialisation | serde, serde_json | camelCase on the wire |
| Tests | tokio::test, tower `oneshot`, testcontainers (postgres), reqwest | |

Rejected: sea-orm (runtime-checked queries, extra abstraction for one table
shape); actix-web (marginal benchmark gain, departs from Tower ecosystem).

## 4. Data model

One table, discriminated by a Postgres enum.

```sql
CREATE TYPE entity_type AS ENUM (
  'country','currency','language','unit_of_measure','region',
  'industry','payment_term','incoterm','tax_code','cost_center'
);
-- Placeholder names. Replace with the real ten before the first migration;
-- each value maps to one Rust marker type and one route prefix.

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
-- Seeded with one row per enum value in the same migration.
```

Rules:

- **IDs are per type and dense.** Insert runs in a transaction:
  `UPDATE id_counters SET next_id = next_id + 1 WHERE entity_type = $1
  RETURNING next_id - 1`, then `INSERT`. The row lock serialises inserts per
  type, which is acceptable at master-data write rates.
- **`entity_type` is an enum**, so an unknown type fails at the database.
  Adding an entity = `ALTER TYPE … ADD VALUE` + a counter row + a Rust marker
  + one line in the route macro.
- **`attributes`** limit (≤2 keys, non-empty string keys, string values) is
  enforced in the Rust `Attributes` newtype at deserialisation and backstopped
  by the CHECK.
- **Soft delete**: `DELETE` sets `deleted_at` and `updated_by`. All reads
  filter `deleted_at IS NULL`. The partial unique index lets a deleted `code`
  be reused.
- **Optimistic locking**: update runs
  `UPDATE … SET …, version = version + 1, updated_at = now(), updated_by = $n
   WHERE entity_type = $1 AND id = $2 AND version = $3 AND deleted_at IS NULL
   RETURNING *`. Zero rows → distinguish not-found from version-conflict with a
  follow-up `SELECT version`.
- **`updated_by`** is the JWT `sub` claim, set on create, update, and delete.
  Never accepted from the client.
- **`code` normalisation**: trimmed and upper-cased before any read or write.

## 5. Code structure

Single crate, binary + library so tests boot the app in-process. Dependencies
point downward only.

```
src/
├── main.rs            config → tracing → pool → migrate → serve (graceful shutdown)
├── lib.rs             pub fn build_app(AppState) -> Router
├── config.rs          AppConfig via figment
├── telemetry.rs       subscriber setup
├── error.rs           AppError + IntoResponse (problem+json)
├── auth.rs            JWT extractor → AuthContext { subject, scopes }
├── domain/
│   ├── entity.rs      EntityType, MasterRecord, Attributes, NewRecord, UpdateRecord
│   └── filter.rs      ListFilter { code, name, limit, offset }, Page<T>
├── repository/
│   ├── mod.rs         trait MasterDataRepository, RepoError, UpdateOutcome
│   └── postgres.rs    PgMasterDataRepository
├── service.rs         MasterDataService: normalisation, validation, error mapping
└── api/
    ├── mod.rs         router assembly, AppState, OpenAPI doc
    ├── handlers.rs    generic handlers: create<E>, get<E>, list<E>, update<E>, delete<E>
    ├── dto.rs         wire structs (serde camelCase, utoipa ToSchema)
    └── routes.rs      macro: one route set per Entity marker
```

### Repository trait

```rust
#[async_trait]
pub trait MasterDataRepository: Send + Sync {
    async fn create(&self, t: EntityType, new: NewRecord, by: &str)
        -> Result<MasterRecord, RepoError>;
    async fn get(&self, t: EntityType, id: i64)
        -> Result<Option<MasterRecord>, RepoError>;
    async fn list(&self, t: EntityType, f: &ListFilter)
        -> Result<Page<MasterRecord>, RepoError>;
    async fn update(&self, t: EntityType, id: i64, expected_version: i32,
                    upd: UpdateRecord, by: &str)
        -> Result<UpdateOutcome, RepoError>;   // Updated(rec) | NotFound | VersionConflict { current }
    async fn soft_delete(&self, t: EntityType, id: i64, by: &str)
        -> Result<bool, RepoError>;            // false if already absent
}
```

`RepoError` wraps `sqlx::Error` and has a distinct variant for unique-violation
on `master_data_code_live`, which the service maps to `DuplicateCode`.

### Per-entity routes from one handler set

```rust
pub trait Entity: Send + Sync + 'static {
    const TYPE: EntityType;
    const PATH: &'static str;   // "countries"
    const TAG:  &'static str;   // OpenAPI tag
}
pub struct Country; impl Entity for Country { … }
```

Handlers are generic over `E: Entity`. `routes.rs` holds a macro invoked once
with the ten markers; it nests `/api/v1/{PATH}` routers and registers OpenAPI
paths per tag. There is exactly one body per operation.

### State

```rust
#[derive(Clone)]
pub struct AppState {
    pub service: Arc<MasterDataService>,     // wraps Arc<dyn MasterDataRepository>
    pub config:  Arc<AppConfig>,
    pub jwt:     Arc<JwtKeys>,
}
```

`dyn` rather than a generic parameter so `build_app` is monomorphic and tests
inject an in-memory fake repository.

### Service

Thin. Owns: `code` normalisation, `Attributes` and field validation, mapping
`UpdateOutcome`/`RepoError` to `AppError`, passing `AuthContext.subject` as
`by`. No per-entity branching; if that appears, split that entity out of the
generic table.

## 6. API contract

Base path `/api/v1`. Identical shape for every entity; `countries` shown.

| Method | Path | Auth | Success | Failure |
|---|---|---|---|---|
| `POST` | `/countries` | write scope | `201`, body, `Location` | `409` duplicate code, `422` validation |
| `GET` | `/countries/{id}` | none | `200` | `404` missing or soft-deleted |
| `GET` | `/countries?code=&name=&limit=&offset=` | none | `200` page | `422` bad params |
| `PUT` | `/countries/{id}` | write scope | `200`, body | `428` no `If-Match`, `409` version mismatch or duplicate code, `404`, `422` |
| `DELETE` | `/countries/{id}` | write scope | `204` (idempotent) | — |

### Record (response)

```json
{
  "id": 42,
  "code": "US",
  "name": "United States",
  "shortName": "USA",
  "attributes": { "iso3": "USA", "region": "NA" },
  "version": 3,
  "createdAt": "2026-10-07T09:12:00Z",
  "updatedAt": "2026-10-07T10:40:12Z",
  "updatedBy": "svc-catalog"
}
```

`POST` and `PUT` bodies contain `code`, `name`, `shortName`, `attributes`
(optional, default `{}`). `PUT` is full replacement. Unknown fields → `422`
(`deny_unknown_fields`). `If-Match` carries the integer version, quoted
(`If-Match: "3"`); the response does not set `ETag` beyond the body's
`version`.

### List

- `code`: exact match after normalisation.
- `name`: case-insensitive substring (`ILIKE '%…%'`).
- `limit`: default 50, max 500. `offset`: default 0, ≥ 0.
- Ordered by `id` ascending.

```json
{ "items": [ … ], "limit": 50, "offset": 0, "total": 213 }
```

`total` is a `COUNT(*)` with the same filter.

### Errors — RFC 9457

`Content-Type: application/problem+json`.

```json
{ "type": "about:blank", "title": "Conflict", "status": 409,
  "detail": "version mismatch: expected 3, current 4",
  "instance": "/api/v1/countries/42" }
```

`422` adds `"errors": [{ "field": "attributes", "message": "at most 2 keys" }]`.
`500` returns a generic detail; the cause is logged.

### Auth

`Authorization: Bearer <JWT>`, HS256, secret from config. Claims: `sub`
(required, becomes `updated_by`), `exp` (required), `scope` (space-separated).
Mutating routes require the configured write scope (default
`masterdata:write`). Missing/invalid token → `401`; valid without scope →
`403`. Reads are unauthenticated.

### Other endpoints

- `GET /health` — `200`, no dependencies.
- `GET /ready` — `200` after `SELECT 1` succeeds, else `503`.
- `GET /openapi.json`, Swagger UI at `/docs`.

## 7. Errors, observability, config

### Error model

```rust
pub enum AppError {
    NotFound,                                   // 404
    DuplicateCode(String),                      // 409
    VersionConflict { expected: i32, current: i32 }, // 409
    PreconditionRequired,                       // 428
    Validation(Vec<FieldError>),                // 422
    Unauthorized,                               // 401
    Forbidden,                                  // 403
    Internal(anyhow::Error),                    // 500
}
```

`IntoResponse` builds the problem+json body. `Internal` logs the full chain at
`error` level inside the request span.

### Observability

- `tracing` only. Subscriber: `EnvFilter` from `RUST_LOG` (default
  `info,sqlx=warn`); `fmt` pretty in dev, JSON when `log.format = "json"`.
- Layers (outer → inner): `SetRequestIdLayer` (honours inbound
  `x-request-id`, else UUID v4) → `PropagateRequestIdLayer` → `TraceLayer`
  with the request id recorded on the span → `TimeoutLayer` → `CorsLayer`.
- Handlers record `entity_type` and `id` on the span; service methods are
  `#[instrument(skip(self))]`.
- Spans are OpenTelemetry-compatible; exporting is a later additive step.

### Config

```rust
pub struct AppConfig {
    pub server:   ServerConfig   { host, port, request_timeout_secs },
    pub database: DatabaseConfig { url: SecretString, max_connections, acquire_timeout_secs },
    pub auth:     AuthConfig     { jwt_secret: SecretString, write_scope },
    pub log:      LogConfig      { format: Pretty | Json },
}
```

Precedence low → high: `config/default.toml` → `config/{APP_ENV}.toml`
(optional) → env vars `APP_*` with `__` nesting (`APP_DATABASE__URL`).
Secrets are not present in committed TOML; a missing `jwt_secret` or
`database.url` fails startup with a named error.

Startup: load config → init tracing → build `PgPool` → `sqlx::migrate!` →
bind → serve with graceful shutdown on SIGTERM/SIGINT.

## 8. Testing

**Unit** (no I/O): `Attributes` validation; `code` normalisation;
`ListFilter` bounds; `AppError → Response` per variant; JWT extractor cases
(expired, bad signature, missing scope, ok); handlers via `build_app` +
`tower::ServiceExt::oneshot` with an in-memory fake repository.

**Repository integration** (`tests/repository.rs`): Postgres via
testcontainers, migrations applied once per binary, table truncated between
tests. Covers per-type ID counters, code reuse after soft delete,
version-conflict update, attributes CHECK, `ILIKE` filter and `total`.

**API end-to-end** (`tests/api.rs`): full app on a random port against the
container, driven by `reqwest`. One generic scenario executed for at least two
entity types: create → get → list-with-filter → update (ok) → update (stale
`If-Match` → 409) → delete → get (404) → recreate same code (201). Auth:
no token → 401, token without scope → 403.

**Build**: `.sqlx/` committed; CI builds with `SQLX_OFFLINE=true` and a
separate job runs `cargo sqlx prepare --check` against a container.

## 9. Open items to settle before implementation

- Replace the ten placeholder `entity_type` values with the real entity names
  and their route paths.
