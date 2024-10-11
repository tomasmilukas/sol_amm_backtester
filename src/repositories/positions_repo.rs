use crate::models::positions_model::{ClosedPositionModel, LivePositionModel};
use chrono::{DateTime, Utc};
use sqlx::{prelude::FromRow, Pool, Postgres, Transaction};
use std::str::FromStr;

#[derive(Clone)]
pub struct PositionsRepo {
    db: Pool<Postgres>,
}

#[allow(dead_code)]
#[derive(FromRow)]
struct LivePositionRow {
    id: i32,
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

    pub async fn upsert_live_positions_in_transaction<'a>(
        &self,
        transaction: &mut Transaction<'a, Postgres>,
        pool_address: &str,
        position: &LivePositionModel,
        version: i32,
    ) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            r#"
                INSERT INTO live_positions (
                    address, pool_address, liquidity, tick_lower, tick_upper, version
                )
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (address, pool_address, version) DO UPDATE SET
                    liquidity = EXCLUDED.liquidity,
                    tick_lower = EXCLUDED.tick_lower,
                    tick_upper = EXCLUDED.tick_upper
            "#,
        )
        .bind(&position.address)
        .bind(pool_address)
        .bind(position.liquidity.to_string())
        .bind(position.tick_lower)
        .bind(position.tick_upper)
        .bind(version)
        .execute(transaction)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                eprintln!("SQL Error: {:?}", e);
                eprintln!("Error details: {}", e);
                eprintln!("Position: {:?}", position);
                eprintln!("Pool address: {}", pool_address);
                eprintln!("Version: {}", version);
                Err(e)
            }
        }
    }

    pub async fn upsert_closed_positions_in_transaction<'a>(
        &self,
        transaction: &mut Transaction<'a, Postgres>,
        pool_address: &str,
        position: &ClosedPositionModel,
    ) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            r#"
                INSERT INTO closed_positions (
                    address, pool_address, tick_lower, tick_upper, position_created_at
                )
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (address, pool_address) DO UPDATE SET
                    tick_lower = EXCLUDED.tick_lower,
                    tick_upper = EXCLUDED.tick_upper,
                    position_created_at = EXCLUDED.position_created_at
            "#,
        )
        .bind(&position.address)
        .bind(pool_address)
        .bind(position.tick_lower)
        .bind(position.tick_upper)
        .bind(position.position_created_at)
        .execute(transaction)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                eprintln!("SQL Error: {:?}", e);
                eprintln!("Error details: {}", e);
                eprintln!("Position: {:?}", position);
                eprintln!("Pool address: {}", pool_address);
                Err(e)
            }
        }
    }

    pub async fn get_live_positions_by_pool_address_and_version(
        &self,
        pool_address: &str,
        version: i32,
    ) -> Result<Vec<LivePositionModel>, sqlx::Error> {
        let rows: Vec<LivePositionRow> =
            sqlx::query_as("SELECT * FROM live_positions WHERE pool_address = $1 AND version = $2 AND liquidity != '0'")
                .bind(pool_address)
                .bind(version)
                .fetch_all(&self.db)
                .await?;

        rows.into_iter()
            .map(|r| self.live_position_row_to_model(r))
            .collect()
    }

    pub async fn get_closed_positions_by_pool_address(
        &self,
        pool_address: &str,
    ) -> Result<Vec<ClosedPositionModel>, sqlx::Error> {
        let rows: Vec<ClosedPositionModel> =
            sqlx::query_as("SELECT * FROM closed_positions WHERE pool_address = $1")
                .bind(pool_address)
                .fetch_all(&self.db)
                .await?;

        rows.into_iter()
            .map(|r| self.closed_position_row_to_model(r))
            .collect()
    }

    pub async fn get_latest_version_for_live_pool(
        &self,
        pool_address: &str,
    ) -> Result<i32, sqlx::Error> {
        let result: Option<i32> = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version), 0) FROM live_positions WHERE pool_address = $1",
        )
        .bind(pool_address)
        .fetch_one(&self.db)
        .await
        .unwrap_or(Some(0));

        result.ok_or(sqlx::Error::RowNotFound)
    }

    fn live_position_row_to_model(
        &self,
        row: LivePositionRow,
    ) -> Result<LivePositionModel, sqlx::Error> {
        let liquidity =
            u128::from_str(&row.liquidity).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;

        Ok(LivePositionModel {
            address: row.address,
            liquidity,
            tick_lower: row.tick_lower,
            tick_upper: row.tick_upper,
            created_at: row.created_at,
        })
    }

    fn closed_position_row_to_model(
        &self,
        row: ClosedPositionModel,
    ) -> Result<ClosedPositionModel, sqlx::Error> {
        Ok(ClosedPositionModel {
            address: row.address,
            tick_lower: row.tick_lower,
            tick_upper: row.tick_upper,
            position_created_at: row.position_created_at,
        })
    }
}
