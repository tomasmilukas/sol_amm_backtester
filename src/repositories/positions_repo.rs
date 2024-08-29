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
    address: String,
    pool_address: String,
    liquidity: String,
    tick_lower: i32,
    tick_upper: i32,
    created_at: DateTime<Utc>,
    last_updated_at: DateTime<Utc>,
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
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO positions (
                address, pool_address, liquidity, tick_lower, tick_upper, created_at, last_updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (address) DO UPDATE SET
                pool_address = EXCLUDED.pool_address,
                liquidity = EXCLUDED.liquidity,
                tick_lower = EXCLUDED.tick_lower,
                tick_upper = EXCLUDED.tick_upper,
                last_updated_at = NOW()
            "#,
        )
        .bind(&position.address)
        .bind(pool_address)
        .bind(position.liquidity.to_string())
        .bind(position.tick_lower)
        .bind(position.tick_upper)
        .bind(position.created_at)
        .bind(position.last_updated_at)
        .execute(transaction)
        .await?;

        Ok(())
    }

    pub async fn get_position_by_address(
        &self,
        address: &str,
    ) -> Result<Option<PositionModel>, sqlx::Error> {
        let row: Option<PositionRow> = query_as("SELECT * FROM positions WHERE address = $1")
            .bind(address)
            .fetch_optional(&self.db)
            .await?;

        row.map(|r| self.row_to_model(r)).transpose()
    }

    pub async fn get_positions_by_pool_address(
        &self,
        pool_address: &str,
    ) -> Result<Vec<PositionModel>, sqlx::Error> {
        let rows: Vec<PositionRow> = query_as("SELECT * FROM positions WHERE pool_address = $1")
            .bind(pool_address)
            .fetch_all(&self.db)
            .await?;

        rows.into_iter().map(|r| self.row_to_model(r)).collect()
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
            last_updated_at: row.last_updated_at,
        })
    }
}
