use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::utils::decode::Pubkey;

use super::token_metadata::TokenMetadata;

// Copied from Orcas source program
#[derive(Debug)]
pub struct Whirlpool {
    pub whirlpools_config: Pubkey,
    pub whirlpool_bump: [u8; 1],
    pub tick_spacing: u16,
    pub tick_spacing_seed: [u8; 2],
    pub fee_rate: u16,
    pub protocol_fee_rate: u16,
    pub liquidity: u128,
    pub sqrt_price: u128,
    pub tick_current_index: i32,
    pub protocol_fee_owed_a: u64,
    pub protocol_fee_owed_b: u64,
    pub token_mint_a: Pubkey,
    pub token_vault_a: Pubkey,
    pub fee_growth_global_a: u128,
    pub token_mint_b: Pubkey,
    pub token_vault_b: Pubkey,
    pub fee_growth_global_b: u128,
}

#[derive(Debug, FromRow, Serialize, Deserialize)]
pub struct PoolModel {
    pub address: String,
    pub name: String,
    pub token_a_name: String,
    pub token_b_name: String,
    pub token_a_address: String,
    pub token_b_address: String,
    pub token_a_vault: String,
    pub token_b_vault: String,
    pub token_a_decimals: i16,
    pub token_b_decimals: i16,
    pub tick_spacing: i16,
    pub total_liquidity: Option<i64>,
    pub fee_rate: i16,
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
        token_a_vault: String,
        token_b_vault: String,
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
            token_a_vault,
            token_b_vault,
            tick_spacing,
            total_liquidity: None,
            fee_rate,
            last_updated_at: chrono::Utc::now(),
        }
    }

    pub fn from_whirlpool(
        pool_address: String,
        whirlpool: Whirlpool,
        token_a_metadata: TokenMetadata,
        token_b_metadata: TokenMetadata,
    ) -> Result<Self> {
        Ok(Self::new(
            pool_address,
            token_a_metadata.symbol,
            token_b_metadata.symbol,
            whirlpool.token_mint_a.to_string(),
            whirlpool.token_mint_b.to_string(),
            token_a_metadata.decimals as i16,
            token_b_metadata.decimals as i16,
            whirlpool.token_vault_a.to_string(),
            whirlpool.token_vault_b.to_string(),
            whirlpool.tick_spacing as i16,
            whirlpool.fee_rate as i16,
        ))
    }
}
