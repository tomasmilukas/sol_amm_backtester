use crate::models::pool_model::PoolModel;
use sqlx::{query, query_as, Pool, Postgres};

pub struct PoolRepo {
    db: Pool<Postgres>,
}

impl PoolRepo {
    pub fn new(db: Pool<Postgres>) -> Self {
        Self { db }
    }

    pub async fn insert(&self, pool: &PoolModel) -> Result<(), sqlx::Error> {
        query(
            "INSERT INTO pools (address, name, token_a_name, token_b_name, token_a_address, token_b_address, token_a_decimals, token_b_decimals, tick_spacing, fee_rate, created_at, last_updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"
        )
        .bind(&pool.address)
        .bind(&pool.name)
        .bind(&pool.token_a_name)
        .bind(&pool.token_b_name)
        .bind(&pool.token_a_address)
        .bind(&pool.token_b_address)
        .bind(pool.token_a_decimals)
        .bind(pool.token_b_decimals)
        .bind(pool.tick_spacing)
        .bind(pool.fee_rate)
        .bind(pool.created_at)
        .bind(pool.last_updated_at)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    pub async fn get_pool_by_address(
        &self,
        address: &str,
    ) -> Result<Option<PoolModel>, sqlx::Error> {
        query_as::<_, PoolModel>("SELECT * FROM pools WHERE address = $1")
            .bind(address)
            .fetch_optional(&self.db)
            .await
    }
}
