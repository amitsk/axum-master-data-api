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
        self.rows
            .values()
            .any(|(r, deleted)| !deleted && r.entity_type == t && r.code == code && Some(r.id) != except_id)
    }

    /// Mirrors the `WHERE entity_type = $1 AND id = $2 AND version = $3 AND deleted_at IS NULL`
    /// clause of the Postgres update: a missing, soft-deleted, or stale row is reported before
    /// a duplicate code is considered, because the unique index can only fire for a row that
    /// matches the clause. Returns the outcome to report, or `None` when the row may be updated.
    fn blocked_outcome(&self, t: EntityType, id: i64, expected_version: i32) -> Option<UpdateOutcome> {
        match self.rows.get(&(t, id)) {
            None | Some((_, true)) => Some(UpdateOutcome::NotFound),
            Some((rec, false)) if rec.version != expected_version => {
                Some(UpdateOutcome::VersionConflict { current: rec.version })
            }
            Some(_) => None,
        }
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
        Ok(g.rows
            .get(&(t, id))
            .filter(|(_, deleted)| !deleted)
            .map(|(r, _)| r.clone()))
    }

    async fn list(&self, t: EntityType, f: &ListFilter) -> Result<Page<MasterRecord>, RepoError> {
        let g = self.inner.lock().unwrap();
        let mut all: Vec<MasterRecord> = g
            .rows
            .values()
            .filter(|(r, deleted)| !deleted && r.entity_type == t)
            .filter(|(r, _)| f.code.as_ref().is_none_or(|c| &r.code == c))
            .filter(|(r, _)| {
                f.name
                    .as_ref()
                    .is_none_or(|n| r.name.to_lowercase().contains(&n.to_lowercase()))
            })
            .map(|(r, _)| r.clone())
            .collect();
        all.sort_by_key(|r| r.id);
        let total = all.len() as i64;
        let items = all
            .into_iter()
            .skip(f.offset as usize)
            .take(f.limit as usize)
            .collect();
        Ok(Page {
            items,
            limit: f.limit,
            offset: f.offset,
            total,
        })
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
        if let Some(outcome) = g.blocked_outcome(t, id, expected_version) {
            return Ok(outcome);
        }
        if g.live_code_exists(t, &upd.code, Some(id)) {
            return Err(RepoError::DuplicateCode(upd.code));
        }
        // The pre-check above ran under the same lock and found a live row, so this
        // cannot be `None`; it is matched without panicking regardless.
        let Some((rec, _)) = g.rows.get_mut(&(t, id)) else {
            return Ok(UpdateOutcome::NotFound);
        };
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
