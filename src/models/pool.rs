use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, FromRow, Serialize, Deserialize)]
pub struct PoolModel {
    pub address: String,
    pub name: String,
    pub token_a_name: String,
    pub token_b_name: String,
    pub token_a_address: String,
    pub token_b_address: String,
    pub token_a_decimals: i16,
    pub token_b_decimals: i16,
    pub tick_spacing: i16,
    pub fee_rate: i16,
    pub created_at: DateTime<Utc>,
    pub last_updated_at: DateTime<Utc>,
}

impl PoolModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        address: String,
        token_a_name: String,
        token_b_name: String,
        token_a_address: String,
        token_b_address: String,
        token_a_decimals: i16,
        token_b_decimals: i16,
        tick_spacing: i16,
        fee_rate: i16,
    ) -> Self {
        Self {
            address,
            name: token_a_name.to_owned() + "/" + &token_b_name.to_owned(),
            token_a_name,
            token_b_name,
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
