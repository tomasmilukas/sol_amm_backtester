use crate::models::positions_model::PositionModel;
use chrono::{DateTime, Utc};
use sqlx::{prelude::FromRow, query_as, Pool, Postgres, Transaction};
use std::str::FromStr;

#[derive(Clone)]
pub struct PositionsRepo {
    db: Pool<Postgres>,
}

#[derive(FromRow)]
struct PositionRow {
    id: i64,
    address: String,
    pool_address: String,
    liquidity: String,
    tick_lower: i32,
    tick_upper: i32,
    version: i32,
    created_at: DateTime<Utc>,
}

impl PositionsRepo {
    pub fn new(db: Pool<Postgres>) -> Self {
        Self { db }
    }

    pub async fn begin_transaction(&self) -> sqlx::Result<Transaction<'static, Postgres>> {
        self.db.begin().await
    }

    pub async fn upsert_in_transaction<'a>(
        &self,
        transaction: &mut Transaction<'a, Postgres>,
        pool_address: &str,
        position: &PositionModel,
        version: i32,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO positions (
                address, pool_address, liquidity, tick_lower, tick_upper, version, created_at,
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (address) DO UPDATE SET
                pool_address = EXCLUDED.pool_address,
                liquidity = EXCLUDED.liquidity,
                tick_lower = EXCLUDED.tick_lower,
                tick_upper = EXCLUDED.tick_upper,
                version = EXCLUDED.version,
            "#,
        )
        .bind(&position.address)
        .bind(pool_address)
        .bind(position.liquidity.to_string())
        .bind(position.tick_lower)
        .bind(position.tick_upper)
        .bind(version)
        .bind(position.created_at)
        .execute(transaction)
        .await?;

        Ok(())
    }

    pub async fn get_positions_by_pool_address_and_version(
        &self,
        pool_address: &str,
        version: i32,
    ) -> Result<Vec<PositionModel>, sqlx::Error> {
        let rows: Vec<PositionRow> =
            sqlx::query_as("SELECT * FROM positions WHERE pool_address = $1 AND version = $2")
                .bind(pool_address)
                .bind(version)
                .fetch_all(&self.db)
                .await?;

        rows.into_iter().map(|r| self.row_to_model(r)).collect()
    }

    pub async fn get_latest_version_for_pool(
        &self,
        pool_address: &str,
    ) -> Result<i32, sqlx::Error> {
        let result: Option<i32> = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version), 0) FROM positions WHERE pool_address = $1",
        )
        .bind(pool_address)
        .fetch_one(&self.db)
        .await?;

        result.ok_or(sqlx::Error::RowNotFound)
    }

    fn row_to_model(&self, row: PositionRow) -> Result<PositionModel, sqlx::Error> {
        let liquidity =
            u128::from_str(&row.liquidity).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;

        Ok(PositionModel {
            address: row.address,
            liquidity,
            tick_lower: row.tick_lower,
            tick_upper: row.tick_upper,
            created_at: row.created_at,
        })
    }
}
