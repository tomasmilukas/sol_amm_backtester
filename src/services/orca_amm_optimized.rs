use std::io::{BufRead, Read};
use std::time::Duration as stdDuration;

use crate::api::transactions_api::TransactionApi;
use crate::models::transactions_model::{
    LiquidityData, SwapData, TransactionData, TransactionModel,
};
use crate::repositories::transactions_repo::TransactionRepo;
use crate::services::transactions_sync_amm_service::AMMService;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use flate2::bufread::GzDecoder;
use reqwest::Client;
use serde_json::{json, Value};

use super::transactions_sync_amm_service::constants::ORCA_OPTIMIZED_PATH_BASE_URL;
use super::transactions_sync_amm_service::Cursor;

pub struct OrcaOptimizedAMM {
    transaction_repo: TransactionRepo,
    transaction_api: TransactionApi,
    token_a_address: String,
    token_b_address: String,
    token_a_vault: String,
    token_b_vault: String,
    http_client: Client,
}

impl OrcaOptimizedAMM {
    pub async fn new(
        transaction_repo: TransactionRepo,
        transaction_api: TransactionApi,
        token_a_address: String,
        token_b_address: String,
        token_a_vault: String,
        token_b_vault: String,
    ) -> Self {
        let http_client = Client::builder()
            .timeout(stdDuration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            transaction_repo,
            transaction_api,
            token_a_address,
            token_b_address,
            token_a_vault,
            token_b_vault,
            http_client,
        }
    }

    fn construct_url(&self, date: &DateTime<Utc>) -> Result<String> {
        Ok(format!(
            "{}/{}/{:02}{:02}/whirlpool-transaction-{}{:02}{:02}.jsonl.gz",
            ORCA_OPTIMIZED_PATH_BASE_URL,
            date.year(),
            date.month(),
            date.day(),
            date.year(),
            date.month(),
            date.day()
        ))
    }

    fn parse_blocks<R: BufRead>(&self, reader: R, pool_address: &str) -> Result<Vec<Value>> {
        let gz = GzDecoder::new(reader);
        let buf_reader = std::io::BufReader::new(gz);
        let mut relevant_blocks = Vec::new();
        let mut buffer = Vec::new();
        let mut depth = 0;

        for byte in buf_reader.bytes() {
            let byte = byte?;
            buffer.push(byte);
            match byte as char {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        // We've reached the end of a complete JSON object
                        let block: Value = serde_json::from_slice(&buffer)?;
                        if let Some(transactions) = block["transactions"].as_array() {
                            let relevant_txs: Vec<Value> = transactions
                                .iter()
                                .filter(|tx| self.is_relevant_transaction(tx, pool_address))
                                .cloned()
                                .collect();

                            if !relevant_txs.is_empty() {
                                let mut relevant_block = block.clone();
                                relevant_block["transactions"] = json!(relevant_txs);
                                relevant_blocks.push(relevant_block);
                            }
                        }
                        buffer.clear();
                    }
                }
                _ => {}
            }
        }

        Ok(relevant_blocks)
    }

    fn is_relevant_transaction(&self, tx: &Value, pool_address: &str) -> bool {
        tx["instructions"]
            .as_array()
            .iter()
            .flat_map(|instructions| instructions.iter())
            .any(|instruction| {
                let name = instruction["name"].as_str().unwrap_or("");
                let empty_map = serde_json::Map::new();
                let payload = instruction["payload"].as_object().unwrap_or(&empty_map);

                matches!(
                    name,
                    "swap"
                        | "swapV2"
                        | "decreaseLiquidity"
                        | "increaseLiquidity"
                        | "twoHopSwap"
                        | "increaseLiquidityV2"
                        | "decreaseLiquidityV2"
                ) && (payload.get("keyWhirlpool") == Some(&Value::String(pool_address.to_string()))
                    || payload.get("keyWhirlpoolOne")
                        == Some(&Value::String(pool_address.to_string()))
                    || payload.get("keyWhirlpoolTwo")
                        == Some(&Value::String(pool_address.to_string())))
            })
    }

    // CONVERSION RELATED LOGIC

    fn convert_single_transaction(
        &self,
        tx: &Value,
        block_time: i64,
        pool_address: &str,
    ) -> Option<TransactionModel> {
        let signature = tx["signature"].as_str()?.to_string();
        let instructions = tx["instructions"].as_array()?;

        for instruction in instructions {
            if let Some(name) = instruction["name"].as_str() {
                return match name {
                    "swap" | "swapV2" => {
                        Some(self.convert_swap(pool_address, &signature, instruction, block_time))
                    }
                    "increaseLiquidity" | "increaseLiquidityV2" => Some(self.convert_liquidity(
                        pool_address,
                        &signature,
                        instruction,
                        block_time,
                        true,
                    )),
                    "decreaseLiquidity" | "decreaseLiquidityV2" => Some(self.convert_liquidity(
                        pool_address,
                        &signature,
                        instruction,
                        block_time,
                        false,
                    )),
                    "twoHopSwap" => {
                        self.convert_two_hop_swap(pool_address, &signature, instruction, block_time)
                    }
                    _ => None,
                };
            }
        }

        None
    }

    fn convert_swap(
        &self,
        pool_address: &str,
        signature: &str,
        instruction: &Value,
        block_time: i64,
    ) -> TransactionModel {
        let payload = instruction["payload"].as_object().unwrap();
        let data_a_to_b = payload["dataAToB"].as_i64().unwrap_or(0) == 1;
        let is_v2 = instruction["name"].as_str().unwrap().ends_with("V2");

        let (token_in, token_out) = if data_a_to_b {
            (&self.token_a_address, &self.token_b_address)
        } else {
            (&self.token_b_address, &self.token_a_address)
        };

        let (amount_in, amount_out) = if is_v2 {
            let transfer0 = payload["transfer0"].as_object().unwrap();
            let transfer1 = payload["transfer1"].as_object().unwrap();

            (
                transfer0["amount"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<u64>()
                    .unwrap_or(0),
                transfer1["amount"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<u64>()
                    .unwrap_or(0),
            )
        } else {
            (
                payload["transferAmount0"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<u64>()
                    .unwrap_or(0),
                payload["transferAmount1"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<u64>()
                    .unwrap_or(0),
            )
        };

        TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time,
            block_time_utc: Utc.timestamp_opt(block_time, 0).unwrap(),
            transaction_type: "Swap".to_string(),
            ready_for_backtesting: true,
            data: TransactionData::Swap(SwapData {
                token_in: token_in.clone(),
                token_out: token_out.clone(),
                amount_in,
                amount_out,
            }),
        }
    }

    fn convert_liquidity(
        &self,
        pool_address: &str,
        signature: &str,
        instruction: &Value,
        block_time: i64,
        is_increase: bool,
    ) -> TransactionModel {
        let payload = instruction["payload"].as_object().unwrap();
        let is_v2 = instruction["name"].as_str().unwrap().ends_with("V2");

        let (amount_a, amount_b) = self.match_token_amounts(
            payload["keyTokenVaultA"].as_str().unwrap(),
            payload["keyTokenVaultB"].as_str().unwrap(),
            &self.get_transfer_amount(payload, "0", is_v2),
            &self.get_transfer_amount(payload, "1", is_v2),
        );

        let transaction_type = format!(
            "{}Liquidity",
            if is_increase { "Increase" } else { "Decrease" },
        );

        let position = payload["keyPosition"].to_string();
        let liquidity_amount = payload["dataLiquidityAmount"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap_or(0)
            .to_string();

        TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time,
            block_time_utc: Utc.timestamp_opt(block_time, 0).unwrap(),
            transaction_type,
            ready_for_backtesting: false,
            data: if is_increase {
                TransactionData::IncreaseLiquidity(LiquidityData {
                    token_a: self.token_a_address.clone(),
                    token_b: self.token_b_address.clone(),
                    amount_a,
                    amount_b,
                    liquidity_amount,
                    tick_lower: None,
                    tick_upper: None,
                    possible_positions: vec![position],
                })
            } else {
                TransactionData::DecreaseLiquidity(LiquidityData {
                    token_a: self.token_a_address.clone(),
                    token_b: self.token_b_address.clone(),
                    amount_a,
                    amount_b,
                    liquidity_amount,
                    tick_lower: None,
                    tick_upper: None,
                    possible_positions: vec![position],
                })
            },
        }
    }

    fn get_transfer_amount(
        &self,
        payload: &serde_json::Map<String, Value>,
        index: &str,
        is_v2: bool,
    ) -> String {
        if is_v2 {
            payload
                .get(&format!("transfer{}", index))
                .and_then(|v| v.as_str())
                .unwrap_or("0")
                .to_string()
        } else {
            payload
                .get(&format!("transferAmount{}", index))
                .and_then(|v| v.as_str())
                .unwrap_or("0")
                .to_string()
        }
    }

    fn convert_two_hop_swap(
        &self,
        pool_address: &str,
        signature: &str,
        instruction: &Value,
        block_time: i64,
    ) -> Option<TransactionModel> {
        let payload = instruction["payload"].as_object().unwrap();

        let (whirlpool_key, vault_a_key, _vault_b_key, amount_in_key, amount_out_key) =
            if payload["keyWhirlpoolOne"].as_str() == Some(pool_address) {
                (
                    "keyWhirlpoolOne",
                    "keyVaultOneA",
                    "keyVaultOneB",
                    "transferAmount0",
                    "transferAmount1",
                )
            } else if payload["keyWhirlpoolTwo"].as_str() == Some(pool_address) {
                (
                    "keyWhirlpoolTwo",
                    "keyVaultTwoA",
                    "keyVaultTwoB",
                    "transferAmount2",
                    "transferAmount3",
                )
            } else {
                return None; // Exit if pool_address doesn't match either Whirlpool
            };

        let vault_a = payload[vault_a_key].as_str().unwrap();

        let amount_in = payload[amount_in_key]
            .as_str()
            .unwrap_or("0")
            .parse::<u64>()
            .unwrap_or(0);

        let amount_out = payload[amount_out_key]
            .as_str()
            .unwrap_or("0")
            .parse::<u64>()
            .unwrap_or(0);

        let a_to_b = if whirlpool_key == "keyWhirlpoolOne" {
            payload["dataAToBOne"].as_i64().unwrap_or(0) == 1
        } else {
            payload["dataAToBTwo"].as_i64().unwrap_or(0) == 1
        };

        let is_a_vault = vault_a == self.token_a_vault;
        let swap_tokens = is_a_vault ^ a_to_b;

        let (token_in, token_out) = if swap_tokens {
            (self.token_b_address.clone(), self.token_a_address.clone())
        } else {
            (self.token_a_address.clone(), self.token_b_address.clone())
        };

        Some(TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time,
            block_time_utc: Utc.timestamp_opt(block_time, 0).unwrap(),
            transaction_type: "TwoHopSwap".to_string(),
            ready_for_backtesting: false,
            data: TransactionData::Swap(SwapData {
                amount_in,
                amount_out,
                token_in,
                token_out,
            }),
        })
    }

    fn match_token_amounts(
        &self,
        vault_a: &str,
        vault_b: &str,
        amount0: &str,
        amount1: &str,
    ) -> (u64, u64) {
        let amount0 = amount0.parse::<u64>().unwrap_or(0);
        let amount1 = amount1.parse::<u64>().unwrap_or(0);

        if vault_a == self.token_a_vault {
            (amount0, amount1)
        } else {
            (amount1, amount0)
        }
    }
}

#[async_trait]
impl AMMService for OrcaOptimizedAMM {
    fn repo(&self) -> &TransactionRepo {
        &self.transaction_repo
    }

    fn api(&self) -> &TransactionApi {
        &self.transaction_api
    }

    async fn fetch_transactions(&self, pool_address: &str, cursor: Cursor) -> Result<Vec<Value>> {
        let date: Option<DateTime<Utc>> = match cursor {
            Cursor::DateTime(date) => Some(date),
            Cursor::OptionalSignature(_) => None,
        };

        let url = self.construct_url(&date.ok_or_else(|| anyhow!("Wrong cursor"))?)?;
        let response = self.http_client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to download file: HTTP {}",
                response.status()
            ));
        }

        let bytes = response.bytes().await?;
        self.parse_blocks(&bytes[..], pool_address)
    }

    async fn fetch_and_insert_transactions(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        latest_db_transaction: Option<TransactionModel>,
    ) -> Result<()> {
        let yesterday = (Utc::now() - Duration::days(1)).date_naive();
        let mut current_date = if let Some(latest_tx) = latest_db_transaction {
            latest_tx.block_time_utc.date_naive()
        } else {
            yesterday
        };

        let start_date = start_time.date_naive();

        while current_date >= start_date {
            let current_datetime = DateTime::<Utc>::from_naive_utc_and_offset(
                current_date.and_hms_opt(0, 0, 0).unwrap(),
                Utc,
            );
            let cursor = Cursor::DateTime(current_datetime);

            let transactions = self.fetch_transactions(pool_address, cursor).await?;

            if transactions.is_empty() {
                println!(
                    "No transactions for {}. Moving to previous day.",
                    current_date
                );

                current_date = current_date
                    .pred_opt()
                    .expect("Failed to get previous date"); // Move to previous day

                continue;
            }

            println!("Processing transactions for {}", current_date);

            let transaction_models =
                self.convert_data_to_transactions_model(pool_address, transactions)?;

            self.insert_transactions(transaction_models).await?;

            // Move to the previous day
            current_date = current_date
                .pred_opt()
                .expect("Failed to get previous date");
        }

        println!("Reached or passed start_time {}. Exiting.", start_time);
        Ok(())
    }

    fn convert_data_to_transactions_model(
        &self,
        pool_address: &str,
        blocks: Vec<Value>,
    ) -> Result<Vec<TransactionModel>> {
        let mut all_transactions = Vec::new();

        for block in blocks {
            let block_time = block["blockTime"].as_i64().unwrap_or(0);

            if let Some(transactions) = block["transactions"].as_array() {
                for tx in transactions {
                    if let Some(transaction_model) =
                        self.convert_single_transaction(tx, block_time, pool_address)
                    {
                        all_transactions.push(transaction_model);
                    }
                }
            }
        }

        Ok(all_transactions)
    }
}
