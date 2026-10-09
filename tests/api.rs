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
    let got: Value = client
        .get(format!("{base}/{id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got, created);

    // list with filter
    let page: Value = client
        .get(format!("{base}?name=alpha"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
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
    let resp = client
        .delete(format!("{base}/{id}"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
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
    let resp = client
        .post(&base)
        .header("authorization", ro)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = client.get(&base).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "reads are open");
}

#[tokio::test]
async fn readiness_and_openapi_over_the_wire() {
    let app = spawn_app().await;
    let client = Client::new();
    assert_eq!(
        client
            .get(format!("{}/ready", app.base_url))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let doc: Value = client
        .get(format!("{}/openapi.json", app.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(doc["info"]["title"], "Master Data API");
}
