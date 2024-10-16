use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::utils::decode::Pubkey;

#[derive(Debug, FromRow, Serialize, Deserialize, Clone)]
pub struct LivePositionModel {
    pub address: String,
    pub liquidity: u128,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow, Serialize, Deserialize, Clone)]
pub struct ClosedPositionModel {
    pub address: String,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub position_created_at: DateTime<Utc>,
}

#[allow(dead_code)]
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

#[allow(dead_code)]
#[derive(Debug, Copy, Clone)]
pub struct PositionRewardInfo {
    pub growth_inside_checkpoint: u128,
    pub amount_owed: u64,
}

const NUM_REWARDS: usize = 3;

impl LivePositionModel {
    pub fn new(address: String, liquidity: u128, tick_lower: i32, tick_upper: i32) -> Self {
        Self {
            address,
            liquidity,
            tick_lower,
            tick_upper,
            created_at: chrono::Utc::now(),
        }
    }
}
