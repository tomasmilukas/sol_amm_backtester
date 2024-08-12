use sqlx::{Pool, Postgres};
use crate::models::Pool;

pub struct PoolRepo {
    db: Pool<Postgres>,
}

impl PoolRepo {
    pub fn new(db: Pool<Postgres>) -> Self {
        Self { db }
    }

    pub async fn insert_pool(&self, pool: &Pool) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "INSERT INTO pools (address, name, token_a_name, token_b_name, token_a_address, token_b_address, token_a_decimals, token_b_decimals, tick_spacing, fee_rate, created_at, last_updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            pool.address, pool.name, pool.token_a_name, pool.token_b_name, pool.token_a_address, pool.token_b_address, pool.token_a_decimals, pool.token_b_decimals,
            pool.tick_spacing, pool.fee_rate, pool.created_at, pool.last_updated_at, pool.is_active
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    pub async fn get_pool_by_address(&self, address: &str) -> Result<Option<Pool>, sqlx::Error> {
        sqlx::query_as!(
            Pool,
            "SELECT * FROM pools WHERE address = $1",
            address
        )
        .fetch_optional(&self.db)
        .await
    }
}
