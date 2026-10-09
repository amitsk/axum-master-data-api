# axum-master-data-api

A CRUD REST API over ten master-data entities — countries, currencies, languages,
categories, genders, ages, geographies, product lines, business units and product
tiers — all stored in **one generic Postgres table** (`master_data`) keyed by an
`entity_type` enum and a per-entity `id` sequence.

There is exactly one handler set, one DTO set and one route table for all ten
entities; the per-entity difference is a single `const PATH`. Adding an eleventh
entity means an enum variant, a migration, and one marker struct — not a new set of
handlers or DTOs.

The design spec — entity model, API contract, error model, observability — lives at
[`specs/2026-10-07-master-data-api-design.md`](specs/2026-10-07-master-data-api-design.md).
The task-by-task implementation plan is at
[`specs/2026-10-07-master-data-api-plan.md`](specs/2026-10-07-master-data-api-plan.md).

Built with axum 0.8, sqlx 0.8, figment, utoipa and `tracing`.

## Data model

```sql
CREATE TABLE master_data (
  entity_type  entity_type NOT NULL,   -- enum of the ten kinds
  id           BIGINT      NOT NULL,   -- dense per entity_type, from id_counters
  code         TEXT        NOT NULL,   -- unique among live rows
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
```

- `attributes` is a flat string→string map with **at most two keys**, enforced both in
  Rust (`Attributes::MAX_KEYS`) and by a database `CHECK` constraint.
- `code` is normalised to upper case and must be unique among rows that are not
  soft-deleted (`master_data_code_live`, a partial unique index). A deleted code can
  be reused.
- `DELETE` is a soft delete: it stamps `deleted_at`, `updated_at` and `updated_by`,
  and is idempotent (`204` even if the row was already deleted). Reads of a
  soft-deleted row return `404`.

## Endpoints

Base path `/api/v1`. The same five operations exist for every entity; only the path
segment changes.

| Method   | Path                              | Auth      | Success                     | Failures |
| -------- | --------------------------------- | --------- | --------------------------- | -------- |
| `POST`   | `/{entity}`                       | write     | `201`, record body, `Location` | `409` duplicate code, `422` validation |
| `GET`    | `/{entity}/{id}`                  | none      | `200`, record               | `404` missing or soft-deleted |
| `GET`    | `/{entity}?code=&name=&limit=&offset=` | none | `200`, page                 | `422` bad parameters |
| `PUT`    | `/{entity}/{id}`                  | write     | `200`, updated record       | `428` missing `If-Match`, `409` version mismatch or duplicate code, `404`, `422` |
| `DELETE` | `/{entity}/{id}`                  | write     | `204`, idempotent           | — |

The ten `{entity}` path segments:

| Entity        | Path segment      | `entity_type` value |
| ------------- | ----------------- | ------------------- |
| Country       | `countries`       | `country` |
| Currency      | `currencies`      | `currency` |
| Language      | `languages`       | `language` |
| Category      | `categories`      | `category` |
| Gender        | `genders`         | `gender` |
| Age           | `ages`            | `age` |
| Geography     | `geographies`     | `geography` |
| Product line  | `product-lines`   | `product_line` |
| Business unit | `business-units`  | `business_unit` |
| Product tier  | `product-tiers`   | `product_tier` |

So the full country routes are `/api/v1/countries`, `/api/v1/countries/{id}`, and so
on; a product tier lives under `/api/v1/product-tiers`.

List parameters:

- `code` — exact match after normalisation (trim + upper case).
- `name` — case-insensitive substring (`ILIKE '%…%'`); `%` and `_` in the input are
  escaped, so they match literally.
- `limit` — default `50`, must be `1..=500`.
- `offset` — default `0`, must be `>= 0`.
- Results are ordered by `id` ascending; `total` is a `COUNT(*)` over the same filter.

```json
{ "items": [], "limit": 50, "offset": 0, "total": 0 }
```

Non-entity endpoints:

| Path            | Description |
| --------------- | ----------- |
| `/health`       | `200` if the process is up. Touches no dependency. |
| `/ready`        | `200` after `SELECT 1` succeeds, `503` otherwise. |
| `/openapi.json` | The OpenAPI 3 document. |
| `/docs`         | Swagger UI, served by `utoipa-swagger-ui`. |

## Run locally

Rust **1.89.0** is pinned in `rust-toolchain.toml`; `rustup` installs it automatically.

```bash
# 1. Database (postgres:18, database `master_data`, password `postgres`, port 5432)
docker compose up -d

# 2. sqlx-cli — pinned to 0.8.6 because newer releases need a newer rustc than the
#    1.89.0 this project pins. --no-default-features drops sqlite/mysql/native-tls.
cargo install sqlx-cli --version 0.8.6 --no-default-features --features postgres,rustls

# 3. Schema. DATABASE_URL comes from the committed .env file.
sqlx migrate run

# 4. Run. Both secrets must come from the environment; they are not in any TOML.
APP_AUTH__JWT_SECRET=dev-secret \
APP_DATABASE__URL=postgres://postgres:postgres@localhost:5432/master_data \
cargo run
```

The server listens on `0.0.0.0:8080` (`config/default.toml`). Then:

- Swagger UI: <http://localhost:8080/docs>
- OpenAPI document: <http://localhost:8080/openapi.json>
- Liveness/readiness: <http://localhost:8080/health>, <http://localhost:8080/ready>

### Using an existing Postgres instead of Docker

If you already run Postgres somewhere — a shared instance on your network, a managed
database, or a local install — skip `docker compose` entirely and point the service at
it. Two things must line up:

1. **Two different env vars read the connection string.** The service reads
   `APP_DATABASE__URL` (figment, `APP_`-prefixed). The `sqlx` tooling — `sqlx migrate
   run`, and the compile-time query macros — reads plain `DATABASE_URL`. Set **both**,
   or migrations will land in one database while the service talks to another.
2. **The database must be reachable and empty enough.** The service runs the
   migrations itself at startup (`MIGRATOR.run(&pool)` in `src/main.rs`), so step 3
   above is optional when you only want to run the server.

```bash
export DATABASE_URL='postgres://USER:PASSWORD@db-host.example.com:5432/master_data'   # sqlx CLI
export APP_DATABASE__URL="$DATABASE_URL"                                            # the service
export APP_AUTH__JWT_SECRET=dev-secret

# Optional, only if you want migrations applied outside the server process:
sqlx migrate run

cargo run
```

The URL is the standard `postgres://user:password@host:port/database` form. A few
practical notes:

- **Create the database first.** The migration creates tables inside an existing
  database; it will not create the database itself.
  `createdb -h db-host.example.com -U USER master_data`
- **`sslmode` is supported** by the normal libpq query string, e.g.
  `postgres://USER:PASSWORD@db-host:5432/master_data?sslmode=require`.
  `sqlx` is built with `tls-rustls`, so TLS needs no system OpenSSL.
- **`.env` is only a sqlx convenience.** The committed `.env` holds a
  `DATABASE_URL` pointing at the local compose container, which `sqlx` CLI reads
  automatically. The service never reads `.env`, so it does not affect `cargo run`.
  Export the vars in your shell (as above), or put them in
  `config/development.toml` — but secrets belong in the environment, which is why
  `database.url` and `auth.jwt_secret` are deliberately absent from every TOML file.
- **To run the test suite against your instance** you do not need to: the container
  tests are self-contained (see [Tests](#tests)).

### Getting a write token

The service does **not** issue tokens; it only verifies them. Writes (`POST`, `PUT`,
`DELETE`) require

```text
Authorization: Bearer <HS256 JWT>
```

signed with `APP_AUTH__JWT_SECRET`, whose space-separated `scope` claim contains
`masterdata:write` (the value of `auth.write_scope` in `config/default.toml`). Reads
are unauthenticated — no token, no scope. A missing or invalid token is `401`; a
valid token without the write scope is `403`. The `sub` claim becomes `updated_by`
on every write and is required, as is `exp`.

The library exposes the minting side for tests and tooling:

```rust
use std::time::Duration;
use axum_master_data_api::auth::JwtKeys;

let keys = JwtKeys::new("dev-secret", "masterdata:write");
let token = keys.issue("local-dev", &["masterdata:write"], Duration::from_secs(3600));
// -> Header {"alg":"HS256","typ":"JWT"}, claims {"sub","iat","exp","scope"}
```

For a throwaway local token you can also assemble the same HS256 token by hand:

```bash
python3 - <<'PY'
import base64, hashlib, hmac, json, time
b64 = lambda raw: base64.urlsafe_b64encode(raw).rstrip(b"=").decode()
now = int(time.time())
signing_input = ".".join(
    b64(json.dumps(part, separators=(",", ":")).encode())
    for part in (
        {"alg": "HS256", "typ": "JWT"},
        {"sub": "local-dev", "iat": now, "exp": now + 3600, "scope": "masterdata:write"},
    )
)
sig = hmac.new(b"dev-secret", signing_input.encode(), hashlib.sha256).digest()
print(f"{signing_input}.{b64(sig)}")
PY
```

Remember `APP_AUTH__JWT_SECRET=dev-secret` on the server, or the signature will not
verify.

### Try it

Create a country:

```bash
curl -i -X POST http://localhost:8080/api/v1/countries \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"code":"US","name":"United States","shortName":"USA","attributes":{"iso3":"USA"}}'
```

```http
HTTP/1.1 201 Created
Location: /api/v1/countries/1

{
  "id": 1,
  "code": "US",
  "name": "United States",
  "shortName": "USA",
  "attributes": { "iso3": "USA" },
  "version": 1,
  "createdAt": "2026-10-07T09:12:00Z",
  "updatedAt": "2026-10-07T09:12:00Z",
  "updatedBy": "local-dev"
}
```

Update it. `PUT` is a full replacement and requires the version you are editing:

```bash
curl -i -X PUT http://localhost:8080/api/v1/countries/1 \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -H 'If-Match: "1"' \
  -d '{"code":"US","name":"United States of America","shortName":"USA","attributes":{"iso3":"USA","region":"NA"}}'
```

```http
HTTP/1.1 200 OK
```

```json
{
  "id": 1,
  "code": "US",
  "name": "United States of America",
  "shortName": "USA",
  "attributes": { "iso3": "USA", "region": "NA" },
  "version": 2,
  "createdAt": "2026-10-07T09:12:00Z",
  "updatedAt": "2026-10-07T10:40:12Z",
  "updatedBy": "local-dev"
}
```

Replaying that same request now fails, because the stored version is `2`:

```json
{
  "type": "about:blank",
  "title": "Conflict",
  "status": 409,
  "detail": "version mismatch: expected 1, current 2",
  "instance": "/api/v1/countries/1"
}
```

Dropping the `If-Match` header gives `428 Precondition Required`.

## Tests

```bash
# No Docker required: unit tests in src/** plus tests/handlers.rs,
# which drives the router over the in-memory repository.
cargo test --lib
cargo test --test handlers

# Docker required: these start a throwaway Postgres with testcontainers.
cargo test --test repository
cargo test --test api

# Everything the CI runs.
cargo test --all-targets
```

`tests/repository.rs` and `tests/api.rs` both go through the
`tests/common/mod.rs` helper, which starts a throwaway Postgres with testcontainers
and runs the migrations on it. They need a working Docker daemon and never connect
to your `docker compose` instance or to `DATABASE_URL`. Each test starts its own
container on a random host port, so the tests are parallel-safe and leave nothing
behind.

### How the database is faked — or rather, not faked

There is no Postgres mock, and there is no PGlite or embedded-Postgres substitute.
The repository and end-to-end tests run against a **real Postgres server in a Docker
container**. Only two layers are substituted:

- `InMemoryRepository` (`src/repository/memory.rs`) implements the same
  `MasterDataRepository` trait as the Postgres one and backs `tests/handlers.rs` and
  the service unit tests. It gives per-entity dense ids, live-code uniqueness,
  soft delete and version bumps — but no SQL, no transactions, no constraints.
- The HTTP layer is driven by `tower::ServiceExt::oneshot` in `tests/handlers.rs`
  (no socket), and by a real socket on a random port in `tests/api.rs`.

The trade-off this buys and costs:

| | Real container (used here) | Mock / embedded (PGlite, sqlx mock stores) |
|---|---|---|
| SQL fidelity | Real parser, planner, partial unique indexes, `CHECK` constraints, `jsonb` | Often partial; a mock validates the query string, not its behaviour |
| Migration coverage | `MIGRATOR.run()` really executes | Usually skipped or faked |
| Speed | ~2–4 s for the container-based tests | Near-instant |
| Isolation | Perfect — throwaway per test | Perfect |
| Setup cost | Needs a working Docker daemon | None, but a large extra dependency |
| CI cost | Needs a Docker-capable runner | Cheaper, simpler runners |

Two real consequences worth knowing:

- **Version drift is guarded.** All three places that name a Postgres image —
  the testcontainers tag (`PG_IMAGE_TAG` in `tests/common/mod.rs`),
  `docker-compose.yml`, and the `sqlx-prepare-check` CI service — pin the same
  `postgres:18`. Two details matter. The major version, because
  `Postgres::default()` silently resolves to `11-alpine`, so migrations would only
  ever be *executed* against a version older than the one you deploy on. And the
  variant, because alpine is musl-based with different locale and collation
  defaults, which is exactly what decides sort order for the `lower(name)` index
  and for `ILIKE` filtering — a green test run on alpine would not prove the same
  ordering in production. The `Postgres versions agree` CI step parses all three
  and fails the build on mismatch, so this cannot rot silently. If you move to a
  newer Postgres, bump all three together.
- **Docker is a hard test dependency.** `cargo test --all-targets` fails outright
  without a Docker daemon. The no-Docker subset is
  `cargo test --lib && cargo test --test handlers`.

## Configuration

Precedence, lowest to highest:

```text
config/default.toml   →   config/{APP_ENV}.toml   →   APP_<SECTION>__<KEY> env vars
```

`APP_ENV` defaults to `development`, so `config/development.toml` is the second layer
when it exists (the file is optional; `config/default.toml` is the only one
committed). The separator is a double underscore: `APP_SERVER__PORT`,
`APP_DATABASE__MAX_CONNECTIONS`, `APP_LOG__FORMAT`.

Secrets are environment-only and have no TOML default. Both are required at startup:

| Variable                 | Meaning |
| ------------------------ | ------- |
| `APP_DATABASE__URL`      | Postgres connection URL. |
| `APP_AUTH__JWT_SECRET`   | HS256 signing secret. |

Non-secret defaults live in `config/default.toml`:

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
format = "pretty"        # or "json"
```

Logging is `tracing` with an `EnvFilter` from `RUST_LOG` (default `info,sqlx=warn`).

## SQL and the sqlx offline cache

sqlx verifies every `query!` / `query_as!` against a live database **at compile
time**. To let CI and fresh checkouts build without a database, this project commits
the query metadata in `.sqlx/` and builds with `SQLX_OFFLINE=true`.

**After editing any query macro, or changing a query, or changing a migration that a
query depends on:**

```bash
# with the database up and migrated
cargo sqlx prepare
git add .sqlx && git commit -m "chore: refresh sqlx offline query cache"
```

`cargo sqlx prepare` overwrites `.sqlx/` from the live schema. CI verifies the cache
has not drifted with a Postgres service, `sqlx migrate run`, and
`cargo sqlx prepare --check` (exit code 1 means the cache is stale). Locally, when a
query macro fails to compile and you did not intend it to, you probably forgot to
re-prepare.

Note that `cargo sqlx prepare` reads `.env`, so `DATABASE_URL` there points at your
`docker compose` database. Queries are written against `migrations/`, which is the
single source of truth for the schema.

## Concurrency and optimistic locking

Every record carries a monotonically increasing `version`, starting at `1`.

- `PUT /api/v1/{entity}/{id}` **requires** `If-Match: "<version>"`, where the value is
  the integer version in quotes — for example `If-Match: "3"`.
- Missing `If-Match` → `428 Precondition Required`.
- `If-Match` present but not equal to the stored version → `409 Conflict` with
  `detail: "version mismatch: expected 3, current 4"`.
- A successful `PUT` returns the record with `version` incremented. `DELETE` does not
  take part in the version protocol: it soft-deletes, refreshes `updated_at` /
  `updated_by`, and returns `204`.
- The response does not carry an `ETag` header; the body's `version` is the token to
  pass back in `If-Match`.

## Errors

All errors are RFC 9457 problem documents served as
`Content-Type: application/problem+json`:

```json
{
  "type": "about:blank",
  "title": "Conflict",
  "status": 409,
  "detail": "version mismatch: expected 3, current 4",
  "instance": "/api/v1/countries/42"
}
```

`422` responses add an `errors` array naming the offending fields:

```json
{
  "type": "about:blank",
  "title": "Unprocessable Entity",
  "status": 422,
  "detail": "validation failed",
  "errors": [{ "field": "attributes", "message": "at most 2 keys allowed, got 3" }]
}
```

Status codes in use: `200`, `201`, `204`, `401`, `403`, `404`, `405`, `408`,
`409`, `422`, `428`, `500`, `503`. Every one of them — including unknown paths
(`404`), wrong methods (`405`), timeouts (`408`, after `request_timeout_secs`),
and an unhealthy `/ready` (`503`) — answers `application/problem+json` with
`instance` set to the request path. `500` returns a generic detail and logs the
cause; nothing internal is
leaked to the client.

Cross-origin browser access is read-only: `GET`/`HEAD` are served with
`Access-Control-Allow-Origin: *`, while `POST`/`PUT`/`DELETE` preflights are not
approved, so a foreign page cannot drive credentialed writes through a visitor's
browser. Same-origin callers (including the bundled Swagger UI) and non-browser
clients are unaffected.

## CI

`.github/workflows/ci.yml` runs three jobs on pushes to `main` and on every pull
request:

| Job                   | What it does |
| --------------------- | ------------ |
| `check`               | `cargo fmt --all --check`, a Postgres-version consistency check across the test/compose/CI images, and `cargo clippy --all-targets -- -D warnings`. |
| `test`                | `cargo test --all-targets`, including the testcontainers suites (GitHub's `ubuntu-latest` has Docker). |
| `sqlx-prepare-check`  | Boots a `postgres:18` service, applies migrations, and runs `cargo sqlx prepare --check` so a stale `.sqlx/` fails the build. |

Both compilation jobs inherit `SQLX_OFFLINE: "true"` from the workflow, so neither
needs a database to build. `sqlx-cli` is installed with `--version 0.8.6 --locked`;
keep that pin in step with `rust-toolchain.toml`, since a newer `sqlx-cli` will not
compile against Rust 1.89.0.