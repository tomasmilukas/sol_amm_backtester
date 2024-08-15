use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::collections::HashMap;

pub fn determine_transaction_type(json: &Value) -> Result<String> {
    let log_messages = json["meta"]["logMessages"]
        .as_array()
        .context("Missing logMessages")?;

    for message in log_messages {
        let message = message.as_str().unwrap_or("");
        if message.contains("Instruction: Swap") {
            return Ok("Swap".to_string());
        } else if message.contains("Instruction: AddLiquidity") {
            return Ok("AddLiquidity".to_string());
        } else if message.contains("Instruction: RemoveLiquidity") {
            return Ok("RemoveLiquidity".to_string());
        }
    }

    Err(anyhow!("Unable to determine transaction type"))
}

pub fn find_pool_balance_changes(
    json: &Value,
    pool_address: &str,
    token_a: &str,
    token_b: &str,
    decimals_a: i16,
    decimals_b: i16,
) -> Result<(String, String, f64, f64)> {
    let pre_balances = get_token_balances(json, "preTokenBalances", pool_address)?;
    let post_balances = get_token_balances(json, "postTokenBalances", pool_address)?;

    let amount_a = calculate_amount_change(&pre_balances, &post_balances, token_a, decimals_a)?;
    let amount_b = calculate_amount_change(&pre_balances, &post_balances, token_b, decimals_b)?;

    if amount_a > 0.0 && amount_b < 0.0 {
        Ok((
            token_b.to_string(),
            token_a.to_string(),
            -amount_b,
            amount_a,
        ))
    } else if amount_a < 0.0 && amount_b > 0.0 {
        Ok((
            token_a.to_string(),
            token_b.to_string(),
            -amount_a,
            amount_b,
        ))
    } else {
        Err(anyhow!(
            "Unexpected balance changes: {} and {}",
            amount_a,
            amount_b
        ))
    }
}

fn get_token_balances(
    json: &Value,
    balance_type: &str,
    pool_address: &str,
) -> Result<HashMap<String, u64>> {
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
            let amount = balance["uiTokenAmount"]["amount"]
                .as_str()
                .context("Missing amount")?
                .parse::<u64>()?;
            result.insert(mint, amount);
        }
    }
    Ok(result)
}

fn calculate_amount_change(
    pre_balances: &HashMap<String, u64>,
    post_balances: &HashMap<String, u64>,
    token: &str,
    decimals: i16,
) -> Result<f64> {
    let pre_amount = pre_balances.get(token).copied().unwrap_or(0);
    let post_amount = post_balances.get(token).copied().unwrap_or(0);
    let change = (post_amount as i128) - (pre_amount as i128);
    Ok((change as f64) / 10f64.powi(decimals as i32))
}
