use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::utils::decode::Pubkey;

#[derive(Debug, FromRow, Serialize, Deserialize)]
pub struct PositionModel {
    pub address: String,
    pub liquidity: u128,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub created_at: DateTime<Utc>,
    pub last_updated_at: DateTime<Utc>,
}

// Below is for decoding.
#[derive(Debug)]
pub struct Position {
    pub whirlpool: Pubkey,
    pub position_mint: Pubkey,
    pub liquidity: u128,
    pub tick_lower_index: i32,
    pub tick_upper_index: i32,
    pub fee_growth_checkpoint_a: u128,
    pub fee_owed_a: u64,
    pub fee_growth_checkpoint_b: u128,
    pub fee_owed_b: u64,
    pub reward_infos: [PositionRewardInfo; NUM_REWARDS],
}

#[derive(Debug, Copy, Clone)]
pub struct PositionRewardInfo {
    pub growth_inside_checkpoint: u128,
    pub amount_owed: u64,
}

const NUM_REWARDS: usize = 3;

impl PositionModel {
    pub fn new(
        address: String,
        liquidity: u128,
        tick_lower: i32,
        tick_upper: i32,
    ) -> Self {
        Self {
            address,
            liquidity,
            tick_lower,
            tick_upper,
            created_at: chrono::Utc::now(),
            last_updated_at: chrono::Utc::now(),
        }
    }
}
