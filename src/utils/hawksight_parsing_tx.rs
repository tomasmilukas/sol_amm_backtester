use crate::{
    models::transactions_model::{LiquidityData, SwapData, TransactionData, TransactionModel},
    services::orca_amm_standard::CommonTransactionData,
};
use anyhow::{anyhow, Result};
use serde_json::Value;

use super::decode::{
    decode_hawksight_swap_data, find_encoded_inner_instruction, HAWKSIGHT_SWAP_DISCRIMINANT,
};

pub struct HawksightParser;

pub struct PoolInfo {
    pub address: String,
    pub token_a: String,
    pub token_b: String,
    pub decimals_a: i16,
    pub decimals_b: i16,
}

// This hawksight parser ONLY parses auto compound which happens very often. There are other transactions it does but we only parse this one so far.
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
                swap_data = Some(Self::extract_swap_from_logs(
                    transaction,
                    log_messages,
                    pool_info,
                )?);
            } else if message.contains("Instruction: IncreaseLiquidity") {
                liquidity_data = Some(Self::extract_liquidity_from_logs(
                    log_messages,
                    pool_info,
                    common_data.account_keys.clone(),
                )?);
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
                ready_for_backtesting: true,
                data: TransactionData::IncreaseLiquidity(liquidity),
            });
        }

        Ok(transactions)
    }

    fn extract_swap_from_logs(
        transaction: &Value,
        log_messages: &Vec<Value>,
        pool_info: &PoolInfo,
    ) -> Result<SwapData> {
        let encoded_data =
            find_encoded_inner_instruction(transaction, HAWKSIGHT_SWAP_DISCRIMINANT)?;
        let swap_data = decode_hawksight_swap_data(&encoded_data)?;

        // Extract price_numerator from logs
        let price_numerator = log_messages
            .iter()
            .find_map(|msg| {
                let msg_str = msg.as_str()?;
                if msg_str.starts_with("Program log: price_numerator ") {
                    msg_str
                        .strip_prefix("Program log: price_numerator ")?
                        .parse::<u64>()
                        .ok()
                } else {
                    None
                }
            })
            .ok_or_else(|| anyhow!("Price numerator not found in logs"))?;

        // price numerator always in terms of token a.
        let price = (price_numerator as f64) / 10f64.powi(pool_info.decimals_a as i32);

        // we check the path of a to b and calculate the correct amountin/amountout
        let (amount_in, amount_out) = if swap_data.a_to_b {
            let amount_a = swap_data.amount as f64 / 10f64.powi(pool_info.decimals_a as i32);
            let amount_b = amount_a * price;
            (amount_a, amount_b)
        } else {
            let amount_b = swap_data.amount as f64 / 10f64.powi(pool_info.decimals_b as i32);
            let amount_a = amount_b / price;
            (amount_b, amount_a)
        };

        let (token_in, token_out) = if swap_data.a_to_b {
            (pool_info.token_a.clone(), pool_info.token_b.clone())
        } else {
            (pool_info.token_b.clone(), pool_info.token_a.clone())
        };

        Ok(SwapData {
            token_in,
            token_out,
            amount_in,
            amount_out,
        })
    }

    fn extract_liquidity_from_logs(
        log_messages: &[Value],
        pool_info: &PoolInfo,
        account_keys: Vec<String>,
    ) -> Result<LiquidityData> {
        let mut amount_a = 0.0;
        let mut amount_b = 0.0;

        let mut tick_lower: Option<u64> = Some(0);
        let mut tick_upper: Option<u64> = Some(0);
        let mut liquidity_amount: u128 = 0;

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
            } else if message.starts_with("Program log: Tick lower index: ") {
                let tick_lower_result = message
                    .trim_start_matches("Program log: Tick lower index: ")
                    .parse()
                    .unwrap_or(0);

                tick_lower = Some(tick_lower_result)
            } else if message.starts_with("Program log: Tick upper index: ") {
                let tick_upper_result = message
                    .trim_start_matches("Program log: Tick upper index: ")
                    .parse()
                    .unwrap_or(0);

                tick_upper = Some(tick_upper_result)
            } else if message.starts_with("Program log: liquidity_amount: ") {
                let liquidity_amount_parsed = message
                    .trim_start_matches("Program log: liquidity_amount: ")
                    .parse()
                    .unwrap_or(0);

                liquidity_amount = liquidity_amount_parsed
            }
        }

        if amount_a == 0.0 || amount_b == 0.0 {
            return Err(anyhow!("Failed to extract liquidity data from logs"));
        }

        // The amount_a and token_a and so on match since the pools keep it matched. If pool is SOL/USDC token_a = SOL, token_b = USDC and the same is in the logs.
        Ok(LiquidityData {
            token_a: pool_info.token_a.clone(),
            token_b: pool_info.token_b.clone(),
            amount_a,
            amount_b,
            liquidity_amount,
            tick_lower,
            tick_upper,
            possible_positions: account_keys,
        })
    }
}
