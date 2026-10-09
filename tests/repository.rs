mod common;

use axum_master_data_api::domain::{EntityType, ListFilter};
use axum_master_data_api::repository::{
    postgres::PgMasterDataRepository, MasterDataRepository, RepoError, UpdateOutcome,
};
use common::{fields, test_pool};

#[tokio::test]
async fn ids_are_dense_per_entity_type() {
    let (_c, pool) = test_pool().await;
    let repo = PgMasterDataRepository::new(pool);
    let a = repo
        .create(EntityType::Country, fields("US", "United States", &[]), "t")
        .await
        .unwrap();
    let b = repo
        .create(EntityType::Country, fields("GB", "United Kingdom", &[]), "t")
        .await
        .unwrap();
    let c = repo
        .create(EntityType::Currency, fields("USD", "US Dollar", &[]), "t")
        .await
        .unwrap();
    assert_eq!((a.id, b.id, c.id), (1, 2, 1));
    assert_eq!(a.version, 1);
    assert_eq!(a.updated_by, "t");
}

#[tokio::test]
async fn duplicate_live_code_is_rejected_but_reusable_after_delete() {
    let (_c, pool) = test_pool().await;
    let repo = PgMasterDataRepository::new(pool);
    let a = repo
        .create(EntityType::Country, fields("US", "United States", &[]), "t")
        .await
        .unwrap();
    let err = repo
        .create(EntityType::Country, fields("US", "Dup", &[]), "t")
        .await
        .unwrap_err();
    assert!(
        matches!(err, RepoError::DuplicateCode(ref c) if c == "US"),
        "{err:?}"
    );
    assert!(repo.soft_delete(EntityType::Country, a.id, "d").await.unwrap());
    assert!(repo.get(EntityType::Country, a.id).await.unwrap().is_none());
    let again = repo
        .create(EntityType::Country, fields("US", "United States", &[]), "t")
        .await
        .unwrap();
    assert_eq!(again.id, 2);
}

#[tokio::test]
async fn soft_delete_is_idempotent_and_records_actor() {
    let (_c, pool) = test_pool().await;
    let repo = PgMasterDataRepository::new(pool.clone());
    let a = repo
        .create(EntityType::Gender, fields("F", "Female", &[]), "t")
        .await
        .unwrap();
    assert!(repo
        .soft_delete(EntityType::Gender, a.id, "deleter")
        .await
        .unwrap());
    assert!(!repo
        .soft_delete(EntityType::Gender, a.id, "deleter")
        .await
        .unwrap());
    let by: String =
        sqlx::query_scalar("SELECT updated_by FROM master_data WHERE entity_type = 'gender' AND id = $1")
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
    let a = repo
        .create(
            EntityType::Currency,
            fields("USD", "US Dollar", &[("symbol", "$")]),
            "t",
        )
        .await
        .unwrap();
    let out = repo
        .update(
            EntityType::Currency,
            a.id,
            1,
            fields("USD", "Dollar", &[("symbol", "$"), ("minor", "2")]),
            "u",
        )
        .await
        .unwrap();
    let rec = match out {
        UpdateOutcome::Updated(r) => r,
        o => panic!("{o:?}"),
    };
    assert_eq!(rec.version, 2);
    assert_eq!(rec.name, "Dollar");
    assert_eq!(rec.updated_by, "u");
    assert_eq!(rec.attributes.as_map()["minor"], "2");
    assert!(rec.updated_at > a.updated_at);

    let out = repo
        .update(EntityType::Currency, a.id, 1, fields("USD", "Stale", &[]), "u")
        .await
        .unwrap();
    assert!(
        matches!(out, UpdateOutcome::VersionConflict { current: 2 }),
        "{out:?}"
    );

    let out = repo
        .update(EntityType::Currency, 999, 1, fields("X", "Missing", &[]), "u")
        .await
        .unwrap();
    assert!(matches!(out, UpdateOutcome::NotFound));
}

#[tokio::test]
async fn update_to_existing_code_is_duplicate() {
    let (_c, pool) = test_pool().await;
    let repo = PgMasterDataRepository::new(pool);
    repo.create(EntityType::Language, fields("EN", "English", &[]), "t")
        .await
        .unwrap();
    let fr = repo
        .create(EntityType::Language, fields("FR", "French", &[]), "t")
        .await
        .unwrap();
    let err = repo
        .update(EntityType::Language, fr.id, 1, fields("EN", "French", &[]), "t")
        .await
        .unwrap_err();
    assert!(matches!(err, RepoError::DuplicateCode(_)));
}

#[tokio::test]
async fn list_filters_pages_and_counts() {
    let (_c, pool) = test_pool().await;
    let repo = PgMasterDataRepository::new(pool);
    for (code, name) in [
        ("US", "United States"),
        ("GB", "United Kingdom"),
        ("FR", "France"),
        ("DE", "Germany"),
    ] {
        repo.create(EntityType::Country, fields(code, name, &[]), "t")
            .await
            .unwrap();
    }
    repo.create(
        EntityType::Currency,
        fields("USD", "United States Dollar", &[]),
        "t",
    )
    .await
    .unwrap();

    let all = repo
        .list(
            EntityType::Country,
            &ListFilter::new(None, None, None, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(all.total, 4);
    assert_eq!(
        all.items.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );

    let united = repo
        .list(
            EntityType::Country,
            &ListFilter::new(None, Some("united".into()), None, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(united.total, 2);

    let page = repo
        .list(
            EntityType::Country,
            &ListFilter::new(None, Some("UNITED".into()), Some(1), Some(1)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].code, "GB");

    let by_code = repo
        .list(
            EntityType::Country,
            &ListFilter::new(Some("fr".into()), None, None, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(by_code.items[0].name, "France");

    let pct = repo
        .list(
            EntityType::Country,
            &ListFilter::new(None, Some("%".into()), None, None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pct.total, 0, "LIKE wildcards in input must be escaped");

    // Each assertion below fails if its corresponding `replace` is removed from
    // `escape_like`: `_` would match every row as a single-char wildcard, and a
    // lone `\` would escape the closing `%` and match everything.
    for probe in ["_", "\\"] {
        let page = repo
            .list(
                EntityType::Country,
                &ListFilter::new(None, Some(probe.into()), None, None).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(page.total, 0, "LIKE metachar {probe:?} in input must be escaped");
    }
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
