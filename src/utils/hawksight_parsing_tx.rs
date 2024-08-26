use crate::{models::transactions_model::{
    LiquidityData, SwapData, TransactionData, TransactionModel,
}, services::orca_amm_standard::CommonTransactionData};
use anyhow::{anyhow, Result};
use serde_json::Value;

pub struct HawksightParser;

pub struct PoolInfo {
    pub address: String,
    pub token_a: String,
    pub token_b: String,
    pub decimals_a: i16,
    pub decimals_b: i16,
}

impl HawksightParser {
    pub fn is_hawksight_transaction(transaction: &Value) -> bool {
        transaction["transaction"]["message"]["accountKeys"]
            .as_array()
            .map_or(false, |keys| {
                keys.iter()
                    .any(|key| key.as_str() == Some("HAWK3BVnwptKRFYfVoVGhBc2TYxpyG9jmAbkHeW9tyKE"))
            })
    }

    pub fn parse_hawksight_program(
        transaction: &Value,
        pool_info: &PoolInfo,
        common_data: &CommonTransactionData,
    ) -> Result<Vec<TransactionModel>> {
        let mut transactions = Vec::new();

        let log_messages = transaction["meta"]["logMessages"]
            .as_array()
            .ok_or_else(|| anyhow!("Missing logMessages"))?;

        let mut swap_data = None;
        let mut liquidity_data = None;

        for message in log_messages {
            let message = message.as_str().unwrap_or("");

            if message.contains("Instruction: Swap") {
                swap_data = Some(Self::extract_swap_from_logs(log_messages, pool_info)?);
            } else if message.contains("Instruction: IncreaseLiquidity") {
                liquidity_data = Some(Self::extract_liquidity_from_logs(log_messages, pool_info)?);
            }
        }

        if let Some(swap) = swap_data {
            transactions.push(TransactionModel {
                signature: common_data.signature.clone(),
                pool_address: pool_info.address.clone(),
                block_time: common_data.block_time,
                block_time_utc: common_data.block_time_utc,
                transaction_type: "Swap".to_string(),
                ready_for_backtesting: true,
                data: TransactionData::Swap(swap),
            });
        }

        if let Some(liquidity) = liquidity_data {
            transactions.push(TransactionModel {
                signature: common_data.signature.clone(),
                pool_address: pool_info.address.clone(),
                block_time: common_data.block_time,
                block_time_utc: common_data.block_time_utc,
                transaction_type: "IncreaseLiquidity".to_string(),
                ready_for_backtesting: false,
                data: TransactionData::IncreaseLiquidity(liquidity),
            });
        }

        Ok(transactions)
    }

    fn extract_swap_from_logs(log_messages: &[Value], pool_info: &PoolInfo) -> Result<SwapData> {
        let mut amount_to_swap = 0.0;
        let mut price_numerator = 0.0;

        for message in log_messages {
            let message = message.as_str().unwrap_or("");
            if message.starts_with("Program log: amount_to_swap ") {
                amount_to_swap = message
                    .trim_start_matches("Program log: amount_to_swap ")
                    .parse()
                    .unwrap_or(0.0);
            } else if message.starts_with("Program log: price_numerator ") {
                price_numerator = message
                    .trim_start_matches("Program log: price_numerator ")
                    .parse()
                    .unwrap_or(0.0);
            }
        }

        if amount_to_swap == 0.0 || price_numerator == 0.0 {
            return Err(anyhow!("Failed to extract swap data from logs"));
        }

        let amount_in = amount_to_swap / 10f64.powi(pool_info.decimals_a as i32);
        let amount_out =
            amount_to_swap * price_numerator / 10f64.powi((9 + pool_info.decimals_b) as i32);

        Ok(SwapData {
            token_in: pool_info.token_a.clone(),
            token_out: pool_info.token_b.clone(),
            amount_in,
            amount_out,
        })
    }

    fn extract_liquidity_from_logs(
        log_messages: &[Value],
        pool_info: &PoolInfo,
    ) -> Result<LiquidityData> {
        let mut amount_a = 0.0;
        let mut amount_b = 0.0;

        for message in log_messages {
            let message = message.as_str().unwrap_or("");
            if message.starts_with("Program log: Will deposit: ") {
                let parts: Vec<&str> = message.split_whitespace().collect();
                if parts.len() >= 6 {
                    let amount: f64 = parts[4].parse().unwrap_or(0.0);
                    if amount_a == 0.0 {
                        amount_a = amount / 10f64.powi(pool_info.decimals_a as i32);
                    } else {
                        amount_b = amount / 10f64.powi(pool_info.decimals_b as i32);
                    }
                }
            }
        }

        if amount_a == 0.0 || amount_b == 0.0 {
            return Err(anyhow!("Failed to extract liquidity data from logs"));
        }

        Ok(LiquidityData {
            token_a: pool_info.token_a.clone(),
            token_b: pool_info.token_b.clone(),
            amount_a,
            amount_b,
            tick_lower: None,
            tick_upper: None,
        })
    }
}
