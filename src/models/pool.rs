use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Serialize, Deserialize, FromRow)]
pub struct Pool {
    pub address: String,
    pub name: String,
    pub token_a_name: String,
    pub token_b_name: String,
    pub token_a_address: String,
    pub token_b_address: String,
    pub token_a_decimals: i32,
    pub token_b_decimals: i32,
    pub tick_spacing: i32,
    pub fee_rate: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_updated_at: chrono::DateTime<chrono::Utc>,
}

impl Pool {
    pub fn new(
        address: String,
        name: String,
        token_a_address: String,
        token_b_address: String,
        token_a_decimals: i32,
        token_b_decimals: i32,
        tick_spacing: i32,
        fee_rate: i32,
    ) -> Self {
        let pool_name_split: Vec<&str> = pair.split('/').collect();

        Self {
            address,
            name,
            token_a_name: pool_name_split[0],
            token_b_name: pool_name_split[1],
            token_a_address,
            token_b_address,
            token_a_decimals,
            token_b_decimals,
            tick_spacing,
            fee_rate,
            created_at: chrono::Utc::now(),
            last_updated_at: chrono::Utc::now(),
        }
    }
}
