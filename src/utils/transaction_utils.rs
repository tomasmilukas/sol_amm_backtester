use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::{collections::HashMap, str::FromStr};
use tokio::time::Duration;
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use crate::services::transactions_amm_service::AMMPlatforms;

pub fn determine_transaction_type(json: &Value) -> Result<String> {
    let log_messages = json["meta"]["logMessages"]
        .as_array()
        .context("Missing logMessages")?;

    for message in log_messages {
        let message = message.as_str().unwrap_or("");
        if message.contains("Instruction: Swap") {
            return Ok("Swap".to_string());
        } else if message.contains("Instruction: IncreaseLiquidity") {
            return Ok("IncreaseLiquidity".to_string());
        } else if message.contains("Instruction: DecreaseLiquidity") {
            return Ok("DecreaseLiquidity".to_string());
        }
    }

    Err(anyhow!("Unable to determine transaction type"))
}

pub fn find_pool_balance_changes(
    json: &Value,
    pool_address: &str,
    token_a: &str,
    token_b: &str,
) -> Result<(String, String, f64, f64)> {
    let pre_balances = get_token_balances(json, "preTokenBalances", pool_address)?;
    let post_balances = get_token_balances(json, "postTokenBalances", pool_address)?;

    let amount_a = calculate_amount_change(&pre_balances, &post_balances, token_a)?;
    let amount_b = calculate_amount_change(&pre_balances, &post_balances, token_b)?;

    Ok((token_a.to_string(), token_b.to_string(), amount_a, amount_b))
}

fn get_token_balances(
    json: &Value,
    balance_type: &str,
    pool_address: &str,
) -> Result<HashMap<String, f64>> {
    let balances = json["meta"][balance_type]
        .as_array()
        .context(format!("Missing {}", balance_type))?;

    let mut result = HashMap::new();
    for balance in balances {
        if balance["owner"].as_str() == Some(pool_address) {
            let mint = balance["mint"]
                .as_str()
                .context("Missing mint")?
                .to_string();

            let amount = balance["uiTokenAmount"]["uiAmount"]
                .as_f64()
                .context("Missing amount")?;

            result.insert(mint, amount);
        }
    }
    Ok(result)
}

fn calculate_amount_change(
    pre_balances: &HashMap<String, f64>,
    post_balances: &HashMap<String, f64>,
    token: &str,
) -> Result<f64> {
    let pre_amount = pre_balances.get(token).copied().unwrap_or(0.0);
    let post_amount = post_balances.get(token).copied().unwrap_or(0.0);

    Ok(post_amount - pre_amount)
}

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
