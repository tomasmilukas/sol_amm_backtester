use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionModel {
    pub signature: String,
    pub pool_address: String,
    pub block_time: i64,
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
    pub token_a: String,
    pub token_b: String,
    pub amount_a: f64,
    pub amount_b: f64,
    pub tick_lower: Option<u64>,
    pub tick_upper: Option<u64>,
    pub possible_positions: Vec<String>,
}

impl TransactionModel {
    pub fn new(
        signature: String,
        pool_address: String,
        block_time: i64,
        block_time_utc: DateTime<Utc>,
        transaction_type: String,
        ready_for_backtesting: bool,
        data: TransactionData,
    ) -> Self {
        Self {
            signature,
            pool_address,
            block_time,
            block_time_utc,
            transaction_type,
            ready_for_backtesting,
            data,
        }
    }
}
