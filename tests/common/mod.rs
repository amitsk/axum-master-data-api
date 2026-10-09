#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum_master_data_api::api::{build_app, AppState};
use axum_master_data_api::auth::JwtKeys;
use axum_master_data_api::domain::{Attributes, RecordFields};
use axum_master_data_api::repository::postgres::{connect, PgMasterDataRepository, MIGRATOR};
use axum_master_data_api::service::MasterDataService;
use sqlx::PgPool;
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt},
};

/// Starts a throwaway Postgres, runs migrations, returns the container (keep it alive) and a pool.
///
/// The exact image tag the tests run against. Keep this byte-identical to the
/// `image:` in `docker-compose.yml` and to the `postgres` service in
/// `.github/workflows/ci.yml`.
///
/// Two things make this worth pinning rather than using `Postgres::default()`:
/// the major version, and the `-alpine` variant. `default()` resolves to
/// `11-alpine`, so migrations would only ever be executed against a major version
/// older than the one we deploy on. And alpine is musl-based with different locale
/// and collation defaults than the Debian-based `postgres:18`, which is exactly what
/// decides sort order for the `lower(name)` index and for `ILIKE` filtering — a test
/// pass on alpine would not prove the same ordering in production.
const PG_IMAGE_TAG: &str = "18";

/// Starts a throwaway Postgres, runs migrations, returns the container (keep it alive) and a pool.
pub async fn test_pool() -> (ContainerAsync<Postgres>, PgPool) {
    let container = Postgres::default()
        .with_tag(PG_IMAGE_TAG)
        .start()
        .await
        .expect("start postgres");
    let port = container.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = connect(&url, 5, Duration::from_secs(10)).await.expect("connect");
    MIGRATOR.run(&pool).await.expect("migrate");
    (container, pool)
}

pub fn fields(code: &str, name: &str, attrs: &[(&str, &str)]) -> RecordFields {
    let map: BTreeMap<String, String> = attrs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    RecordFields {
        code: code.to_string(),
        name: name.to_string(),
        short_name: code.to_string(),
        attributes: Attributes::new(map).unwrap(),
    }
}

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
        service: Arc::new(MasterDataService::new(Arc::new(PgMasterDataRepository::new(
            pool,
        )))),
        jwt: jwt.clone(),
    };
    let app = build_app(state, Duration::from_secs(10));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    TestApp {
        base_url: format!("http://{addr}"),
        jwt,
        _container: container,
    }
}
