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

        // price numerator always in terms of token a (incl decimals)
        // swap_data.amount is always amount in (either token a or b)

        // we check the path of a to b and calculate the correct amountin/amountout
        let (amount_in, amount_out) = if swap_data.a_to_b {
            let amount_b = (swap_data.amount as u128 * price_numerator as u128)
                / 10_u128.pow(pool_info.decimals_a as u32);

            (swap_data.amount as u128, amount_b)
        } else {
            let amount_a = (swap_data.amount as u128 * 10_u128.pow(pool_info.decimals_a as u32))
                / price_numerator as u128;

            (swap_data.amount as u128, amount_a)
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
        let mut amount_a = 0;
        let mut amount_b = 0;

        let mut tick_lower: Option<i32> = Some(0);
        let mut tick_upper: Option<i32> = Some(0);
        let mut liquidity_amount: u128 = 0;

        for message in log_messages {
            let message = message.as_str().unwrap_or("");
            if message.starts_with("Program log: Will deposit: ") {
                let parts: Vec<&str> = message.split_whitespace().collect();
                if parts.len() >= 6 {
                    let amount: u128 = parts[4].parse().unwrap_or(0);

                    // since amount_a is always parsed first, we update that one.
                    if amount_a == 0 {
                        amount_a = amount;
                    } else {
                        amount_b = amount;
                    }
                }
            } else if message.starts_with("Program log: Tick lower index: ") {
                let tick_lower_result = message
                    .trim_start_matches("Program log: Tick lower index: ")
                    .parse::<i32>()
                    .unwrap_or(0);

                tick_lower = Some(tick_lower_result)
            } else if message.starts_with("Program log: Tick upper index: ") {
                let tick_upper_result = message
                    .trim_start_matches("Program log: Tick upper index: ")
                    .parse::<i32>()
                    .unwrap_or(0);

                tick_upper = Some(tick_upper_result);
            } else if message.starts_with("Program log: liquidity_amount: ") {
                let liquidity_amount_parsed = message
                    .trim_start_matches("Program log: liquidity_amount: ")
                    .parse()
                    .unwrap_or(0);

                liquidity_amount = liquidity_amount_parsed
            }
        }

        if amount_a == 0 || amount_b == 0 {
            return Err(anyhow!("Failed to extract liquidity data from logs"));
        }

        // The amount_a and token_a and so on match since the pools keep it matched. If pool is SOL/USDC token_a = SOL, token_b = USDC and the same is in the logs.
        Ok(LiquidityData {
            token_a: pool_info.token_a.clone(),
            token_b: pool_info.token_b.clone(),
            amount_a,
            amount_b,
            liquidity_amount: liquidity_amount.to_string(),
            tick_lower,
            tick_upper,
            possible_positions: account_keys,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_hawksight_parser_with_real_data() {
        // The full JSON string from the transaction
        let transaction_json = json!({
            "blockTime": 1725258498,
            "meta": {
                "innerInstructions": [
                    {
                        "index": 2,
                        "instructions": [
                            {
                                "accounts": [22, 2, 16, 7, 17, 8, 18, 9, 10, 11, 21],
                                "data": "59p8WydnSZtSgvdX6cMyZ95kjwDLE5vHpsDgpehCuLrZYbeCCYHrdHkhbu",
                                "programIdIndex": 25,
                                "stackHeight": 3
                            },
                            {
                                "accounts": [16, 22, 2, 5, 6, 7, 8, 17, 18, 19, 20],
                                "data": "3KLKPPgnNhbYeMn4Buq7dEb9wgXg5rTDeXSwNoPXxjSP2nDxhFqk81D",
                                "programIdIndex": 25,
                                "stackHeight": 3
                            }
                        ]
                    }
                ],
                "logMessages": [
                    "Program log: Instruction: Swap",
                    "Program log: price_numerator 128695633296",
                    "Program log: amount_to_swap 118566",
                    "Program log: Token balance A: 2536570",
                    "Program log: Token balance B: 89966",
                    "Program log: Tick lower index: -21752",
                    "Program log: Tick upper index: -15560",
                    "Program log: liquidity_amount: 4153033",
                    "Program log: Will deposit: 2536570 amount in A",
                    "Program log: Will deposit: 89966 amount in B",
                    "Program log: Instruction: IncreaseLiquidity"
                ]
            },
            "transaction": {
                "message": {
                    "accountKeys": [
                        "HAWK3BVnwptKRFYfVoVGhBc2TYxpyG9jmAbkHeW9tyKE",
                        "dche7M2764e8AxNihBdn7uffVzZvTBNeL8x4LZg5E2c",
                        "HN5jKXfzyg6KXaq6X8GxYyPH1WQtWHYx4zN2DwFvoPAi",
                        "FpCMFDFGYotvufJ7HrFHsWEiiQCGbkLCtwHiDnh7o28Q"
                    ]
                }
            }
        });

        let pool_info = PoolInfo {
            address: "FpCMFDFGYotvufJ7HrFHsWEiiQCGbkLCtwHiDnh7o28Q".to_string(),
            token_a: "So11111111111111111111111111111111111111112".to_string(),
            token_b: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            decimals_a: 9,
            decimals_b: 6,
        };

        let common_data = CommonTransactionData {
            signature: "test_signature".to_string(),
            block_time: 1725258498,
            block_time_utc: chrono::DateTime::from_timestamp(1725258498, 0).unwrap(),
            account_keys: transaction_json["transaction"]["message"]["accountKeys"]
                .as_array()
                .unwrap()
                .iter()
                .map(|key| key.as_str().unwrap().to_string())
                .collect(),
        };

        let result =
            HawksightParser::parse_hawksight_program(&transaction_json, &pool_info, &common_data);
        assert!(result.is_ok());

        let parsed_transactions = result.unwrap();
        assert_eq!(parsed_transactions.len(), 2); // We expect both a swap and a liquidity transaction

        // Check Swap transaction
        let swap_transaction = parsed_transactions
            .iter()
            .find(|t| t.transaction_type == "Swap")
            .unwrap();

        if let TransactionData::Swap(swap_data) = &swap_transaction.data {
            assert_eq!(
                swap_data.token_out,
                "So11111111111111111111111111111111111111112"
            );
            assert_eq!(
                swap_data.token_in,
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
            );

            assert_eq!(swap_data.amount_in, 118566);
            assert_eq!(swap_data.amount_out, 921);
        } else {
            panic!("Expected Swap data");
        }

        // Check IncreaseLiquidity transaction
        let liquidity_transaction = parsed_transactions
            .iter()
            .find(|t| t.transaction_type == "IncreaseLiquidity")
            .unwrap();

        if let TransactionData::IncreaseLiquidity(liquidity_data) = &liquidity_transaction.data {
            assert_eq!(
                liquidity_data.token_a,
                "So11111111111111111111111111111111111111112"
            );
            assert_eq!(
                liquidity_data.token_b,
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
            );
            assert_eq!(liquidity_data.liquidity_amount, "4153033");
            assert_eq!(liquidity_data.tick_lower, Some(-21752));
            assert_eq!(liquidity_data.tick_upper, Some(-15560));
            assert!(liquidity_data
                .possible_positions
                .contains(&"HAWK3BVnwptKRFYfVoVGhBc2TYxpyG9jmAbkHeW9tyKE".to_string()));
            assert!(liquidity_data
                .possible_positions
                .contains(&"FpCMFDFGYotvufJ7HrFHsWEiiQCGbkLCtwHiDnh7o28Q".to_string()));
        } else {
            panic!("Expected IncreaseLiquidity data");
        }
    }
}
