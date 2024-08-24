use crate::models::positions_model::PositionModel;
use sqlx::{query_as, Pool, Postgres, Transaction};

pub struct PositionsRepo {
    db: Pool<Postgres>,
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
                address, pool_address, liquidity, tick_lower, tick_upper,
                token_a_amount, token_b_amount, time_scraped_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (address) DO UPDATE SET
                pool_address = EXCLUDED.pool_address,
                liquidity = EXCLUDED.liquidity,
                tick_lower = EXCLUDED.tick_lower,
                tick_upper = EXCLUDED.tick_upper,
                token_a_amount = EXCLUDED.token_a_amount,
                token_b_amount = EXCLUDED.token_b_amount,
                time_scraped_at = EXCLUDED.time_scraped_at,
                last_updated_at = NOW()
            "#,
        )
        .bind(&position.address)
        .bind(&pool_address)
        .bind(position.liquidity)
        .bind(position.tick_lower)
        .bind(position.tick_upper)
        .bind(position.token_a_amount)
        .bind(position.token_b_amount)
        .bind(position.time_scraped_at)
        .execute(transaction)
        .await?;

        Ok(())
    }

    pub async fn get_position_by_address(
        &self,
        address: &str,
    ) -> Result<Option<PositionModel>, sqlx::Error> {
        query_as::<_, PositionModel>("SELECT * FROM positions WHERE address = $1")
            .bind(address)
            .fetch_optional(&self.db)
            .await
    }

    pub async fn get_positions_by_pool_address(
        &self,
        pool_address: &str,
    ) -> Result<Vec<PositionModel>, sqlx::Error> {
        query_as::<_, PositionModel>("SELECT * FROM positions WHERE pool_address = $1")
            .bind(pool_address)
            .fetch_all(&self.db)
            .await
    }
}
