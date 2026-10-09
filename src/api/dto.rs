use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::domain::{MasterRecord, Page};
use crate::service::RecordInput;

/// Write payload. camelCase on the wire; unknown fields (including server-owned
/// ones such as `updatedBy`) are rejected so a client cannot forge them.
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
        RecordInput {
            code: r.code,
            name: r.name,
            short_name: r.short_name,
            attributes: r.attributes,
        }
    }
}

/// Read payload. Serialised camelCase; deserialisable so tests can round-trip it.
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

/// A page of records ordered by `id`.
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

/// List filters. Bounds are validated by `ListFilter`, which rejects rather than clamps.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    pub code: Option<String>,
    pub name: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
