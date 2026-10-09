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
        errors.push(FieldError::new(
            field,
            format!("must be at most {max} characters"),
        ));
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
    Ok(RecordFields {
        code,
        name,
        short_name,
        attributes,
    })
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
    pub async fn create(
        &self,
        t: EntityType,
        input: RecordInput,
        by: &str,
    ) -> Result<MasterRecord, AppError> {
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
            UpdateOutcome::VersionConflict { current } => Err(AppError::VersionConflict {
                expected: expected_version,
                current,
            }),
        }
    }

    #[instrument(skip(self), fields(entity_type = t.as_str()))]
    pub async fn delete(&self, t: EntityType, id: i64, by: &str) -> Result<(), AppError> {
        self.repo.soft_delete(t, id, by).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::memory::InMemoryRepository;

    fn svc() -> MasterDataService {
        MasterDataService::new(Arc::new(InMemoryRepository::new()))
    }

    fn input(code: &str, name: &str, short: &str, attrs: &[(&str, &str)]) -> RecordInput {
        RecordInput {
            code: code.into(),
            name: name.into(),
            short_name: short.into(),
            attributes: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[tokio::test]
    async fn create_normalises_code_and_trims_fields() {
        let s = svc();
        let rec = s
            .create(
                EntityType::Country,
                input("  us ", " United States ", " USA ", &[]),
                "me",
            )
            .await
            .unwrap();
        assert_eq!(rec.code, "US");
        assert_eq!(rec.name, "United States");
        assert_eq!(rec.short_name, "USA");
        assert_eq!(rec.updated_by, "me");
        assert_eq!(rec.id, 1);
    }

    #[tokio::test]
    async fn validation_names_every_bad_field() {
        let errs = validate(input(
            "   ",
            "",
            &"x".repeat(65),
            &[("a", "1"), ("b", "2"), ("c", "3")],
        ))
        .unwrap_err();
        let fields: Vec<&str> = errs.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, vec!["code", "name", "shortName", "attributes"]);
    }

    #[tokio::test]
    async fn validation_enforces_lengths_and_blank_attribute_key() {
        assert!(validate(input(&"c".repeat(64), "n", "s", &[])).is_ok());
        assert_eq!(
            validate(input(&"c".repeat(65), "n", "s", &[])).unwrap_err()[0].field,
            "code"
        );
        assert_eq!(
            validate(input("c", &"n".repeat(257), "s", &[])).unwrap_err()[0].field,
            "name"
        );
        assert_eq!(
            validate(input("c", "n", "s", &[(" ", "v")])).unwrap_err()[0].field,
            "attributes"
        );
    }

    #[tokio::test]
    async fn get_missing_is_not_found_and_duplicate_is_conflict() {
        let s = svc();
        assert!(matches!(
            s.get(EntityType::Age, 1).await.unwrap_err(),
            AppError::NotFound
        ));
        s.create(EntityType::Age, input("A", "Adult", "A", &[]), "m")
            .await
            .unwrap();
        let err = s
            .create(EntityType::Age, input("a", "Adult2", "A", &[]), "m")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::DuplicateCode(ref c) if c == "A"));
    }

    #[tokio::test]
    async fn update_maps_outcomes() {
        let s = svc();
        let rec = s
            .create(EntityType::Geography, input("EU", "Europe", "EU", &[]), "m")
            .await
            .unwrap();
        let upd = s
            .update(
                EntityType::Geography,
                rec.id,
                1,
                input("EU", "Europe!", "EU", &[]),
                "m2",
            )
            .await
            .unwrap();
        assert_eq!(
            (upd.version, upd.name.as_str(), upd.updated_by.as_str()),
            (2, "Europe!", "m2")
        );
        let err = s
            .update(EntityType::Geography, rec.id, 1, input("EU", "x", "EU", &[]), "m")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::VersionConflict {
                expected: 1,
                current: 2
            }
        ));
        let err = s
            .update(EntityType::Geography, 99, 1, input("EU", "x", "EU", &[]), "m")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::NotFound));
    }

    #[tokio::test]
    async fn delete_is_idempotent_and_hides_record() {
        let s = svc();
        let rec = s
            .create(EntityType::ProductTier, input("T1", "Tier 1", "T1", &[]), "m")
            .await
            .unwrap();
        s.delete(EntityType::ProductTier, rec.id, "m").await.unwrap();
        s.delete(EntityType::ProductTier, rec.id, "m").await.unwrap();
        assert!(matches!(
            s.get(EntityType::ProductTier, rec.id).await.unwrap_err(),
            AppError::NotFound
        ));
        let page = s
            .list(
                EntityType::ProductTier,
                &ListFilter::new(None, None, None, None).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(page.total, 0);
    }
}
