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
            return Err(AttributesError::TooManyKeys {
                max: Self::MAX_KEYS,
                got: map.len(),
            });
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
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
        assert_eq!(
            Attributes::new(map(&[("  ", "x")])).unwrap_err(),
            AttributesError::EmptyKey
        );
        assert_eq!(
            Attributes::new(map(&[("", "x")])).unwrap_err(),
            AttributesError::EmptyKey
        );
    }

    #[test]
    fn attributes_from_json_rejects_non_object_and_non_string_values() {
        assert_eq!(
            Attributes::from_json(serde_json::json!([1])).unwrap_err(),
            AttributesError::NotAnObject
        );
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
