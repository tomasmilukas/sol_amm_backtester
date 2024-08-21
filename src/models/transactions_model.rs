use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::utils::transaction_utils;

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionModel {
    pub signature: String,
    pub pool_address: String,
    // Block time utc for ORCA OPTIMIZED will have incorrect times, but correct dates.
    pub block_time_utc: DateTime<Utc>,
    pub transaction_type: String,
    pub ready_for_backtesting: bool,
    #[serde(flatten)]
    pub data: TransactionData,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "transaction_type", content = "data")]
pub enum TransactionData {
    Swap(SwapData),
    IncreaseLiquidity(LiquidityData),
    DecreaseLiquidity(LiquidityData),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SwapData {
    pub token_in: String,
    pub token_out: String,
    pub amount_in: f64,
    pub amount_out: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LiquidityData {
    pub key_position: String,
    pub token_a: String,
    pub token_b: String,
    pub amount_a: f64,
    pub amount_b: f64,
    pub tick_lower: Option<u64>,
    pub tick_upper: Option<u64>,
}

impl TransactionModel {
    pub fn new(
        signature: String,
        pool_address: String,
        block_time_utc: DateTime<Utc>,
        transaction_type: String,
        data: TransactionData,
    ) -> Self {
        Self {
            signature,
            pool_address,
            block_time_utc,
            transaction_type,
            ready_for_backtesting: false,
            data,
        }
    }

    pub fn convert_from_json(
        pool_address: &str,
        json: &Value,
        token_a_address: &str,
        token_b_address: &str,
    ) -> Result<Self> {
        // Check if this transaction involves our pool
        // let post_token_balances = json["meta"]["postTokenBalances"]
        //     .as_array()
        //     .context("Missing postTokenBalances")?;

        // let pool_involved = post_token_balances
        //     .iter()
        //     .any(|balance| balance["owner"].as_str() == Some(pool_address));

        // if !pool_involved {
        //     return Err(anyhow!("Transaction does not involve the specified pool"));
        // }

        // let signature = json["transaction"]["signatures"][0]
        //     .as_str()
        //     .context("Missing signature")?
        //     .to_string();

        // let block_time = json["blockTime"].as_i64().context("Missing blockTime")?;
        // let block_time_utc = Utc
        //     .timestamp_opt(block_time, 0)
        //     .single()
        //     .context("Invalid blockTime")?;

        // let slot = json["slot"].as_i64().context("Missing slot")?;

        // // Use the utility function to determine the transaction type
        // let transaction_type = transaction_utils::determine_transaction_type(json)?;

        // // Use the utility function to find pool balance changes
        // let (token_a, token_b, amount_a, amount_b) = transaction_utils::find_pool_balance_changes(
        //     json,
        //     pool_address,
        //     token_a_address,
        //     token_b_address,
        // )?;

        // if amount_a == 0.0 || amount_b == 0.0 {
        //     println!("Skip transfer");
        // }

        // let transaction_data = match transaction_type.as_str() {
        //     "Swap" => TransactionData::Swap(SwapData {
        //         token_in: if amount_a < 0.0 {
        //             token_b.clone()
        //         } else {
        //             token_a.clone()
        //         },
        //         token_out: if amount_a < 0.0 { token_a } else { token_b },
        //         amount_in: amount_a.abs().max(amount_b.abs()),
        //         amount_out: amount_a.abs().min(amount_b.abs()),
        //     }),
        //     "IncreaseLiquidity" => TransactionData::IncreaseLiquidity(LiquidityData {
        //         token_a,
        //         token_b,
        //         amount_a: amount_a.abs(),
        //         amount_b: amount_b.abs(),
        //         tick_lower: None,
        //         tick_upper: None,
        //     }),
        //     "DecreaseLiquidity" => TransactionData::DecreaseLiquidity(LiquidityData {
        //         token_a,
        //         token_b,
        //         amount_a: amount_a.abs(),
        //         amount_b: amount_b.abs(),
        //         tick_lower: None,
        //         tick_upper: None,
        //     }),
        //     _ => {
        //         return Err(anyhow!(
        //             "Unsupported transaction type: {}",
        //             transaction_type
        //         ))
        //     }
        // };

        // Ok(Self::new(
        //     signature,
        //     pool_address.to_string(),
        //     block_time,
        //     block_time_utc,
        //     slot,
        //     transaction_type,
        //     transaction_data,
        // ))
        todo!("CHECK LATER!")
    }
}
