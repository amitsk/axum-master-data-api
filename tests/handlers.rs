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
    format!(
        "Bearer {}",
        keys.issue("tester", &["masterdata:write"], Duration::from_secs(60))
    )
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value, axum::http::HeaderMap) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body, headers)
}

fn json_req(method: &str, uri: &str, auth: Option<&str>, body: Value) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
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
    let body =
        json!({"code": "us", "name": "United States", "shortName": "USA", "attributes": {"iso3": "USA"}});
    let (status, created, headers) = send(
        &app,
        json_req("POST", "/api/v1/countries", Some(&token(&keys)), body),
    )
    .await;
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
        "countries",
        "currencies",
        "languages",
        "categories",
        "genders",
        "ages",
        "geographies",
        "product-lines",
        "business-units",
        "product-tiers",
    ];
    for p in paths {
        let body = json!({"code": "X", "name": "X", "shortName": "X"});
        let (status, created, _) = send(
            &app,
            json_req("POST", &format!("/api/v1/{p}"), Some(&token(&keys)), body),
        )
        .await;
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
    let ro = format!(
        "Bearer {}",
        keys.issue("ro", &["masterdata:read"], Duration::from_secs(60))
    );
    let (status, _, _) = send(&app, json_req("POST", "/api/v1/ages", Some(&ro), body)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn validation_errors_are_problem_json_with_fields() {
    let (app, keys) = app();
    let body =
        json!({"code": " ", "name": "N", "shortName": "S", "attributes": {"a": "1", "b": "2", "c": "3"}});
    let (status, p, _) = send(
        &app,
        json_req("POST", "/api/v1/categories", Some(&token(&keys)), body),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let fields: Vec<&str> = p["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, vec!["code", "attributes"]);
}

#[tokio::test]
async fn malformed_bodies_are_422_problem_json() {
    let (app, keys) = app();
    let t = token(&keys);
    // unknown field
    let (status, p, _) = send(
        &app,
        json_req(
            "POST",
            "/api/v1/genders",
            Some(&t),
            json!({"code": "X", "name": "X", "shortName": "X", "extra": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(p["status"], 422);
    // non-string attribute value
    let (status, _, _) = send(
        &app,
        json_req(
            "POST",
            "/api/v1/genders",
            Some(&t),
            json!({"code": "X", "name": "X", "shortName": "X", "attributes": {"a": 1}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // server-owned field supplied
    let (status, _, _) = send(
        &app,
        json_req(
            "POST",
            "/api/v1/genders",
            Some(&t),
            json!({"code": "X", "name": "X", "shortName": "X", "updatedBy": "me"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // not JSON at all
    let req = Request::post("/api/v1/genders")
        .header("authorization", &t)
        .header("content-type", "application/json")
        .body(Body::from("{nope"))
        .unwrap();
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
    for (c, n) in [
        ("US", "United States"),
        ("GB", "United Kingdom"),
        ("FR", "France"),
    ] {
        send(
            &app,
            json_req(
                "POST",
                "/api/v1/countries",
                Some(&t),
                json!({"code": c, "name": n, "shortName": c}),
            ),
        )
        .await;
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
    send(
        &app,
        json_req(
            "POST",
            "/api/v1/currencies",
            Some(&t),
            json!({"code": "USD", "name": "Dollar", "shortName": "USD"}),
        ),
    )
    .await;
    let body = json!({"code": "USD", "name": "US Dollar", "shortName": "USD", "attributes": {"symbol": "$"}});

    let (status, p, _) = send(
        &app,
        json_req("PUT", "/api/v1/currencies/1", Some(&t), body.clone()),
    )
    .await;
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
    send(
        &app,
        json_req(
            "POST",
            "/api/v1/languages",
            Some(&t),
            json!({"code": "EN", "name": "English", "shortName": "EN"}),
        ),
    )
    .await;
    let del = || {
        Request::delete("/api/v1/languages/1")
            .header("authorization", &t)
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(send(&app, del()).await.0, StatusCode::NO_CONTENT);
    assert_eq!(send(&app, del()).await.0, StatusCode::NO_CONTENT);
    assert_eq!(
        send(&app, get_req("/api/v1/languages/1")).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, rec, _) = send(
        &app,
        json_req(
            "POST",
            "/api/v1/languages",
            Some(&t),
            json!({"code": "EN", "name": "English", "shortName": "EN"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(rec["id"], 2);
}

#[tokio::test]
async fn duplicate_code_is_409() {
    let (app, keys) = app();
    let t = token(&keys);
    let body = json!({"code": "X", "name": "X", "shortName": "X"});
    send(
        &app,
        json_req("POST", "/api/v1/business-units", Some(&t), body.clone()),
    )
    .await;
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

    let req = Request::get("/health")
        .header("x-request-id", "abc-123")
        .body(Body::empty())
        .unwrap();
    let (_, _, headers) = send(&app, req).await;
    assert_eq!(headers["x-request-id"], "abc-123");
    let (_, _, headers) = send(&app, get_req("/health")).await;
    assert!(
        headers.get("x-request-id").is_some(),
        "generated id when none supplied"
    );
}

#[tokio::test]
async fn openapi_lists_every_entity_and_docs_served() {
    let (app, _) = app();
    let (status, doc, _) = send(&app, get_req("/openapi.json")).await;
    assert_eq!(status, StatusCode::OK);
    let paths = doc["paths"].as_object().unwrap();
    for p in [
        "countries",
        "currencies",
        "languages",
        "categories",
        "genders",
        "ages",
        "geographies",
        "product-lines",
        "business-units",
        "product-tiers",
    ] {
        assert!(paths.contains_key(&format!("/api/v1/{p}")), "{p} collection");
        assert!(paths.contains_key(&format!("/api/v1/{p}/{{id}}")), "{p} item");
    }
    let put = &doc["paths"]["/api/v1/countries/{id}"]["put"];
    assert!(put["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "If-Match"));
    assert!(put["security"].as_array().is_some());
    assert!(doc["components"]["schemas"].get("RecordResponse").is_some());
    assert!(doc["components"]["schemas"].get("ProblemDetails").is_some());

    let resp = app.clone().oneshot(get_req("/docs/")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn unknown_routes_are_problem_json_not_empty() {
    let (app, _) = app();
    let (status, p, headers) = send(&app, get_req("/api/v1/nope")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(headers["content-type"], "application/problem+json");
    let p: ProblemDetails = serde_json::from_value(p).unwrap();
    assert_eq!(p.detail, "not found");
    assert_eq!(p.instance.as_deref(), Some("/api/v1/nope"));
}

#[tokio::test]
async fn wrong_method_is_405_problem_json() {
    let (app, _) = app();
    let (status, _, _) = send(&app, get_req("/api/v1/countries")).await;
    assert_eq!(status, StatusCode::OK, "sanity: GET collection exists");
    let req = Request::patch("/api/v1/countries/1").body(Body::empty()).unwrap();
    let (status, p, headers) = send(&app, req).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(headers["content-type"], "application/problem+json");
    let p: ProblemDetails = serde_json::from_value(p).unwrap();
    assert_eq!(p.detail, "method not allowed");
    assert_eq!(p.instance.as_deref(), Some("/api/v1/countries/1"));
}

#[tokio::test]
async fn trailing_slash_is_404_problem_json() {
    // Collection routes are strict: `/api/v1/countries/` (trailing slash) does
    // not match. This locks in the behaviour so a future nesting change cannot
    // silently start serving — or shadowing — the slash variant.
    let (app, _) = app();
    let (status, p, _) = send(&app, get_req("/api/v1/countries/")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let p: ProblemDetails = serde_json::from_value(p).unwrap();
    assert_eq!(p.instance.as_deref(), Some("/api/v1/countries/"));
}

#[tokio::test]
async fn ready_is_503_when_database_is_down() {
    use async_trait::async_trait;
    use axum_master_data_api::domain::{EntityType, ListFilter};
    use axum_master_data_api::repository::{MasterDataRepository, RepoError, UpdateOutcome};

    struct AlwaysDown;
    #[async_trait]
    impl MasterDataRepository for AlwaysDown {
        async fn ping(&self) -> Result<(), RepoError> {
            Err(RepoError::Database(sqlx::Error::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "db gone",
            ))))
        }
        async fn create(
            &self,
            _t: EntityType,
            _new: axum_master_data_api::domain::NewRecord,
            _by: &str,
        ) -> Result<axum_master_data_api::domain::MasterRecord, RepoError> {
            unimplemented!()
        }
        async fn get(
            &self,
            _t: EntityType,
            _id: i64,
        ) -> Result<Option<axum_master_data_api::domain::MasterRecord>, RepoError> {
            unimplemented!()
        }
        async fn list(
            &self,
            _t: EntityType,
            _f: &ListFilter,
        ) -> Result<axum_master_data_api::domain::Page<axum_master_data_api::domain::MasterRecord>, RepoError>
        {
            unimplemented!()
        }
        async fn update(
            &self,
            _t: EntityType,
            _id: i64,
            _v: i32,
            _u: axum_master_data_api::domain::UpdateRecord,
            _by: &str,
        ) -> Result<UpdateOutcome, RepoError> {
            unimplemented!()
        }
        async fn soft_delete(&self, _t: EntityType, _id: i64, _by: &str) -> Result<bool, RepoError> {
            unimplemented!()
        }
    }

    let state = AppState {
        service: Arc::new(MasterDataService::new(Arc::new(AlwaysDown))),
        jwt: JwtKeys::new("test-secret", "masterdata:write"),
    };
    let app = build_app(state, Duration::from_secs(5));
    let (status, p, headers) = send(&app, get_req("/ready")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(headers["content-type"], "application/problem+json");
    let p: ProblemDetails = serde_json::from_value(p).unwrap();
    assert_eq!(p.detail, "service unavailable");
    assert_eq!(p.instance.as_deref(), Some("/ready"));
    // The database cause must not leak to the client.
    assert!(!p.detail.contains("db gone"));
}

#[tokio::test]
async fn cors_allows_cross_origin_reads_but_not_writes() {
    let (app, _) = app();
    // Simple cross-origin GET: answered with `*` so public reads stay embeddable.
    let req = Request::get("/api/v1/countries")
        .header("origin", "https://example.com")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["access-control-allow-origin"], "*");

    // Preflight for a credentialed write: the approved method/header lists must not
    // cover POST or `authorization`, which is what makes the browser block the
    // real request. (The layer still answers the OPTIONS itself; denial is
    // expressed through the allow-lists, not the status.)
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/api/v1/countries")
        .header("origin", "https://evil.example")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "authorization,content-type")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let methods = resp.headers()["access-control-allow-methods"]
        .to_str()
        .unwrap()
        .to_string();
    let req_headers = resp.headers()["access-control-allow-headers"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        !methods.contains("POST") && !methods.contains("PUT") && !methods.contains("DELETE"),
        "write methods must not be preflight-approved, got {methods:?}"
    );
    assert!(
        !req_headers.contains("authorization"),
        "authorization header must not be preflight-approved, got {req_headers:?}"
    );
}
