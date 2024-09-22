use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::fmt;
use std::str::FromStr;
use tokio::time::Duration;
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use crate::services::{orca_amm_standard::CommonTransactionData, transactions_sync_amm_service::AMMPlatforms};

pub async fn retry_with_backoff<F, Fut, T, E>(
    f: F,
    max_retries: u32,
    base_delay: u64,
    max_delay: u64,
) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    let retry_strategy = ExponentialBackoff::from_millis(base_delay)
        .max_delay(Duration::from_millis(max_delay))
        .map(jitter)
        .take(max_retries as usize);

    Retry::spawn(retry_strategy, f)
        .await
        .map_err(|e| anyhow::anyhow!("Operation failed after retries: {:?}", e))
}

impl FromStr for AMMPlatforms {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "ORCA" => Ok(AMMPlatforms::Orca),
            "RAYDIUM" => Ok(AMMPlatforms::Raydium),
            // Add other platforms as needed
            _ => Err(anyhow!("Unknown platform: {}", s)),
        }
    }
}

impl fmt::Display for AMMPlatforms {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            AMMPlatforms::Orca => write!(f, "ORCA"),
            AMMPlatforms::Raydium => write!(f, "RAYDIUM"),
        }
    }
}

// extract common data from solana
pub fn extract_common_data(tx_data: &Value) -> Result<CommonTransactionData> {
    let signature = tx_data["transaction"]["signatures"][0]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing logMessages"))?
        .to_string();

    let block_time = tx_data["blockTime"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("Missing logMessages"))?;

    let block_time_utc = DateTime::<Utc>::from_timestamp(block_time, 0)
        .ok_or_else(|| anyhow::anyhow!("Missing logMessages"))?;

    let account_keys: Vec<String> = tx_data["transaction"]["message"]["accountKeys"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing accountKeys"))?
        .iter()
        .map(|value: &serde_json::Value| value.to_string())
        .collect::<Vec<String>>();

    Ok(CommonTransactionData {
        signature,
        block_time,
        block_time_utc,
        account_keys,
    })
}
