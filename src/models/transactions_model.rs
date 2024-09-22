use anyhow::Result;
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

// Transaction model from DB.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TransactionModelFromDB {
    pub tx_id: i64,
    pub signature: String,
    pub pool_address: String,
    pub block_time: i64,
    pub block_time_utc: DateTime<Utc>,
    pub transaction_type: String,
    pub ready_for_backtesting: bool,
    #[serde(flatten)]
    pub data: TransactionData,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "transaction_type", content = "data")]
pub enum TransactionData {
    Swap(SwapData),
    IncreaseLiquidity(LiquidityData),
    DecreaseLiquidity(LiquidityData),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SwapData {
    pub token_in: String,
    pub token_out: String,
    // DB only supports up to 2^64
    pub amount_in: u64,
    pub amount_out: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LiquidityData {
    pub token_a: String,
    pub token_b: String,
    // DB only supports up to 2^64
    pub amount_a: u64,
    pub amount_b: u64,
    pub liquidity_amount: String,
    pub tick_lower: Option<i32>,
    pub tick_upper: Option<i32>,
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

impl TransactionModelFromDB {
    pub fn transform_to_tx_model(&self) -> TransactionModel {
        TransactionModel {
            signature: self.signature.clone(),
            pool_address: self.pool_address.clone(),
            block_time: self.block_time,
            block_time_utc: self.block_time_utc,
            transaction_type: self.transaction_type.clone(),
            ready_for_backtesting: self.ready_for_backtesting,
            data: self.data.clone(),
        }
    }
}

impl TransactionData {
    pub fn to_liquidity_data(&self) -> Result<&LiquidityData> {
        match self {
            TransactionData::IncreaseLiquidity(data) | TransactionData::DecreaseLiquidity(data) => {
                Ok(data)
            }
            _ => Err(anyhow::anyhow!(
                "Transaction is not a liquidity transaction"
            )),
        }
    }

    pub fn from_liquidity_data(data: LiquidityData, is_increase: bool) -> Self {
        if is_increase {
            TransactionData::IncreaseLiquidity(data)
        } else {
            TransactionData::DecreaseLiquidity(data)
        }
    }

    pub fn to_swap_data(&self) -> Result<&SwapData> {
        match self {
            TransactionData::Swap(data) => Ok(data),
            _ => Err(anyhow::anyhow!("Transaction is not a swap transaction")),
        }
    }
}

impl LiquidityData {
    pub fn into_transaction_data(self, transaction_type: &str) -> TransactionData {
        match transaction_type {
            "IncreaseLiquidity" => TransactionData::from_liquidity_data(self, true),
            "DecreaseLiquidity" => TransactionData::from_liquidity_data(self, false),
            _ => todo!(),
        }
    }
}
