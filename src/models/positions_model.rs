use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::utils::decode::Pubkey;

#[derive(Debug, FromRow, Serialize, Deserialize)]
pub struct PositionModel {
    pub address: String,
    pub liquidity: i64,
    pub tick_lower: i16,
    pub tick_upper: i16,
    pub token_a_amount: i16,
    pub token_b_amount: i16,
    pub time_scraped_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub last_updated_at: DateTime<Utc>,
}

impl PositionsModel {
    pub fn new(
        address: String,
        liquidity: i16,
        tick_lower: i16,
        tick_upper: i16,
        token_a_amount: i16,
        token_b_amount: i16,
        time_scraped_at: DateTime<Utc>,
    ) -> Self {
        Self {
            address,
            liquidity,
            tick_lower,
            tick_upper,
            token_a_amount,
            token_b_amount,
            time_scraped_at,
            created_at: chrono::Utc::now(),
            last_updated_at: chrono::Utc::now(),
        }
    }
}
