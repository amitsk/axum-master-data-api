use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{postgres::PgPoolOptions, PgPool, Postgres, QueryBuilder};

use crate::domain::{Attributes, EntityType, ListFilter, MasterRecord, NewRecord, Page, UpdateRecord};
use crate::repository::{MasterDataRepository, RepoError, UpdateOutcome};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn connect(
    url: &str,
    max_connections: u32,
    acquire_timeout: Duration,
) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(acquire_timeout)
        .connect(url)
        .await
}

#[derive(Clone)]
pub struct PgMasterDataRepository {
    pool: PgPool,
}

impl PgMasterDataRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    entity_type: EntityType,
    id: i64,
    code: String,
    name: String,
    short_name: String,
    attributes: serde_json::Value,
    version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    updated_by: String,
}

impl TryFrom<Row> for MasterRecord {
    type Error = RepoError;
    fn try_from(r: Row) -> Result<Self, RepoError> {
        let attributes = Attributes::from_json(r.attributes)
            .map_err(|e| RepoError::Database(sqlx::Error::Decode(Box::new(e))))?;
        Ok(MasterRecord {
            entity_type: r.entity_type,
            id: r.id,
            code: r.code,
            name: r.name,
            short_name: r.short_name,
            attributes,
            version: r.version,
            created_at: r.created_at,
            updated_at: r.updated_at,
            updated_by: r.updated_by,
        })
    }
}

const UNIQUE_CODE_INDEX: &str = "master_data_code_live";

fn map_unique(err: sqlx::Error, code: &str) -> RepoError {
    match &err {
        sqlx::Error::Database(db)
            if db.is_unique_violation() && db.constraint() == Some(UNIQUE_CODE_INDEX) =>
        {
            RepoError::DuplicateCode(code.to_string())
        }
        _ => RepoError::Database(err),
    }
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

const COLS: &str =
    "entity_type, id, code, name, short_name, attributes, version, created_at, updated_at, updated_by";

fn push_filters<'a>(qb: &mut QueryBuilder<'a, Postgres>, t: EntityType, f: &'a ListFilter) {
    qb.push(" WHERE deleted_at IS NULL AND entity_type = ")
        .push_bind(t);
    if let Some(code) = &f.code {
        qb.push(" AND code = ").push_bind(code);
    }
    if let Some(name) = &f.name {
        qb.push(" AND name ILIKE ")
            .push_bind(format!("%{}%", escape_like(name)));
    }
}

#[async_trait]
impl MasterDataRepository for PgMasterDataRepository {
    async fn ping(&self) -> Result<(), RepoError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    async fn create(&self, t: EntityType, new: NewRecord, by: &str) -> Result<MasterRecord, RepoError> {
        let mut tx = self.pool.begin().await?;
        let id = sqlx::query_scalar!(
            r#"UPDATE id_counters SET next_id = next_id + 1
               WHERE entity_type = $1 RETURNING next_id - 1 AS "id!""#,
            t as EntityType
        )
        .fetch_one(&mut *tx)
        .await?;
        let code_for_error = new.code.clone();
        let row = sqlx::query_as!(
            Row,
            r#"INSERT INTO master_data (entity_type, id, code, name, short_name, attributes, updated_by)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               RETURNING entity_type AS "entity_type: EntityType", id, code, name, short_name,
                         attributes, version, created_at, updated_at, updated_by"#,
            t as EntityType,
            id,
            new.code,
            new.name,
            new.short_name,
            new.attributes.to_json(),
            by
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| map_unique(e, &code_for_error))?;
        tx.commit().await?;
        row.try_into()
    }

    async fn get(&self, t: EntityType, id: i64) -> Result<Option<MasterRecord>, RepoError> {
        let row = sqlx::query_as!(
            Row,
            r#"SELECT entity_type AS "entity_type: EntityType", id, code, name, short_name,
                      attributes, version, created_at, updated_at, updated_by
               FROM master_data
               WHERE entity_type = $1 AND id = $2 AND deleted_at IS NULL"#,
            t as EntityType,
            id
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    async fn list(&self, t: EntityType, f: &ListFilter) -> Result<Page<MasterRecord>, RepoError> {
        let mut count_qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT count(*) FROM master_data");
        push_filters(&mut count_qb, t, f);
        let total: i64 = count_qb.build_query_scalar().fetch_one(&self.pool).await?;

        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(format!("SELECT {COLS} FROM master_data"));
        push_filters(&mut qb, t, f);
        qb.push(" ORDER BY id LIMIT ")
            .push_bind(f.limit)
            .push(" OFFSET ")
            .push_bind(f.offset);
        let rows: Vec<Row> = qb.build_query_as().fetch_all(&self.pool).await?;
        let items = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
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
        let code_for_error = upd.code.clone();
        let row = sqlx::query_as!(
            Row,
            r#"UPDATE master_data
               SET code = $4, name = $5, short_name = $6, attributes = $7,
                   version = version + 1, updated_at = now(), updated_by = $8
               WHERE entity_type = $1 AND id = $2 AND version = $3 AND deleted_at IS NULL
               RETURNING entity_type AS "entity_type: EntityType", id, code, name, short_name,
                         attributes, version, created_at, updated_at, updated_by"#,
            t as EntityType,
            id,
            expected_version,
            upd.code,
            upd.name,
            upd.short_name,
            upd.attributes.to_json(),
            by
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| map_unique(e, &code_for_error))?;
        if let Some(row) = row {
            return Ok(UpdateOutcome::Updated(row.try_into()?));
        }
        let current = sqlx::query_scalar!(
            "SELECT version FROM master_data WHERE entity_type = $1 AND id = $2 AND deleted_at IS NULL",
            t as EntityType,
            id
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(match current {
            Some(current) => UpdateOutcome::VersionConflict { current },
            None => UpdateOutcome::NotFound,
        })
    }

    async fn soft_delete(&self, t: EntityType, id: i64, by: &str) -> Result<bool, RepoError> {
        let result = sqlx::query!(
            "UPDATE master_data SET deleted_at = now(), updated_at = now(), updated_by = $3
             WHERE entity_type = $1 AND id = $2 AND deleted_at IS NULL",
            t as EntityType,
            id,
            by
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
}
