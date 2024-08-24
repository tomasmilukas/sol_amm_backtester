use crate::models::pool_model::PoolModel;
use chrono::Utc;
use sqlx::{query, query_as, Pool, Postgres};

#[derive(Clone)]
pub struct PoolRepo {
    db: Pool<Postgres>,
}

impl PoolRepo {
    pub fn new(db: Pool<Postgres>) -> Self {
        Self { db }
    }

    pub async fn upsert(&self, pool: &PoolModel) -> Result<(), sqlx::Error> {
        query(
            r#"
            INSERT INTO pools (
                address, name, token_a_name, token_b_name, 
                token_a_address, token_b_address, token_a_decimals, token_b_decimals, 
                tick_spacing, fee_rate, last_updated_at
            ) 
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (address) DO UPDATE SET
                name = EXCLUDED.name,
                token_a_name = EXCLUDED.token_a_name,
                token_b_name = EXCLUDED.token_b_name,
                token_a_address = EXCLUDED.token_a_address,
                token_b_address = EXCLUDED.token_b_address,
                token_a_decimals = EXCLUDED.token_a_decimals,
                token_b_decimals = EXCLUDED.token_b_decimals,
                tick_spacing = EXCLUDED.tick_spacing,
                fee_rate = EXCLUDED.fee_rate,
                last_updated_at = $12
            "#,
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
        .bind(pool.last_updated_at)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    pub async fn update_liquidity(
        &self,
        address: &str,
        total_liquidity: i64,
    ) -> Result<(), sqlx::Error> {
        query(
            r#"
            UPDATE pools
            SET total_liquidity = $1, last_updated_at = $2
            WHERE address = $3
            "#,
        )
        .bind(total_liquidity)
        .bind(Utc::now())
        .bind(address)
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
