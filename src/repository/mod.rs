pub mod memory;
pub mod postgres;

use async_trait::async_trait;

use crate::domain::{EntityType, ListFilter, MasterRecord, NewRecord, Page, UpdateRecord};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("code '{0}' already exists")]
    DuplicateCode(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug)]
pub enum UpdateOutcome {
    Updated(MasterRecord),
    NotFound,
    VersionConflict { current: i32 },
}

#[async_trait]
pub trait MasterDataRepository: Send + Sync {
    async fn ping(&self) -> Result<(), RepoError>;
    async fn create(&self, t: EntityType, new: NewRecord, by: &str) -> Result<MasterRecord, RepoError>;
    async fn get(&self, t: EntityType, id: i64) -> Result<Option<MasterRecord>, RepoError>;
    async fn list(&self, t: EntityType, f: &ListFilter) -> Result<Page<MasterRecord>, RepoError>;
    async fn update(
        &self,
        t: EntityType,
        id: i64,
        expected_version: i32,
        upd: UpdateRecord,
        by: &str,
    ) -> Result<UpdateOutcome, RepoError>;
    async fn soft_delete(&self, t: EntityType, id: i64, by: &str) -> Result<bool, RepoError>;
}
