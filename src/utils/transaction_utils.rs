use anyhow::{anyhow, Result};
use std::fmt;
use std::str::FromStr;
use tokio::time::Duration;
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use crate::services::transactions_amm_service::AMMPlatforms;

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
