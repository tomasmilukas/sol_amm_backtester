use std::time::Duration as stdDuration;

use crate::api::transactions_api::TransactionApi;
use crate::models::transactions_model::TransactionModel;
use crate::repositories::transactions_repo::TransactionRepo;
use crate::services::transactions_amm_service::AMMService;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Utc};
use flate2::bufread::GzDecoder;
use reqwest::Client;
use serde_json::{from_str, Value};
use std::io::{BufRead, BufReader};

use super::transactions_amm_service::constants::ORCA_OPTIMIZED_PATH_BASE_URL;
use super::transactions_amm_service::Cursor;

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

    fn parse_block(
        &self,
        stream: &mut JsonStream<GzDecoder<&[u8]>>,
        pool_address: &str,
    ) -> Result<Option<Value>> {
        let mut block = json!({
            "slot": null,
            "blockHeight": null,
            "blockTime": null,
            "transactions": []
        });

        while let Some(token) = stream.next()? {
            match token {
                Token::ObjectEnd => break,
                Token::PropertyName(name) => {
                    let value = stream.parse_next_value()?;
                    if name == "transactions" {
                        if let Value::Array(transactions) = value {
                            let relevant_txs = transactions
                                .into_iter()
                                .filter(|tx| self.is_relevant_transaction(tx, pool_address))
                                .collect();

                            if !relevant_txs.is_empty() {
                                block["transactions"] = json!(relevant_txs);
                            } else {
                                return Ok(None); // No relevant transactions in this block
                            }
                        }
                    } else {
                        block[name] = value;
                    }
                }
                _ => {}
            }
        }

        if block["transactions"].as_array().unwrap().is_empty() {
            Ok(None)
        } else {
            Ok(Some(block))
        }
    }

    fn is_relevant_transaction(&self, tx: &Value, pool_address: &str) -> bool {
        tx["instructions"]
            .as_array()
            .iter()
            .flat_map(|instructions| instructions.iter())
            .any(|instruction| {
                let name = instruction["name"].as_str().unwrap_or("");
                let payload = instruction["payload"]
                    .as_object()
                    .unwrap_or(&serde_json::Map::new());

                matches!(
                    name,
                    "swap"
                        | "decreaseLiquidity"
                        | "increaseLiquidity"
                        | "twoHopSwap"
                        | "increaseLiquidityV2"
                        | "decreaseLiquidityV2"
                        | "openPositionWithMetadata"
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

        let mut open_position_data = None;
        let mut increase_liquidity_data = None;

        for instruction in instructions {
            if let Some(name) = instruction["name"].as_str() {
                match name {
                    "openPositionWithMetadata" => {
                        open_position_data = Some(instruction["payload"].clone());
                    }
                    "increaseLiquidity" => {
                        increase_liquidity_data = Some(instruction["payload"].clone());
                    }
                    "swap" => {
                        return Some(self.convert_swap(
                            pool_address,
                            &signature,
                            instruction,
                            block_time,
                        ));
                    }
                    "increaseLiquidityV2" => {
                        return Some(self.convert_increase_liquidity_v2(
                            pool_address,
                            &signature,
                            instruction,
                            block_time,
                        ));
                    }
                    "decreaseLiquidity" => {
                        return Some(self.convert_decrease_liquidity(
                            pool_address,
                            &signature,
                            instruction,
                            block_time,
                        ));
                    }
                    "decreaseLiquidityV2" => {
                        return Some(self.convert_decrease_liquidity_v2(
                            pool_address,
                            &signature,
                            instruction,
                            block_time,
                        ));
                    }
                    "twoHopSwap" => {
                        return Some(self.convert_two_hop_swap(
                            pool_address,
                            &signature,
                            instruction,
                            block_time,
                        ));
                    }
                    _ => {}
                }
            }
        }

        if let (Some(open_data), Some(increase_data)) =
            (open_position_data, increase_liquidity_data)
        {
            return Some(self.convert_increase_liquidity(
                pool_address,
                &signature,
                &increase_data,
                &open_data,
                block_time_utc,
            ));
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

        let (token_in, token_out) = if data_a_to_b {
            (&self.token_a_address, &self.token_b_address)
        } else {
            (&self.token_b_address, &self.token_a_address)
        };

        let amount_in = payload["transferAmount0"]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            .unwrap_or(0.0);
        let amount_out = payload["transferAmount1"]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            .unwrap_or(0.0);

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

    fn convert_increase_liquidity(
        &self,
        pool_address: &str,
        signature: &str,
        increase_payload: &Value,
        open_payload: &Value,
        block_time: i64,
    ) -> TransactionModel {
        let payload = increase_payload["payload"].as_object().unwrap();
        let tick_lower = open_payload["payload"]["dataTickLowerIndex"]
            .as_i64()
            .map(|t| t as u64);
        let tick_upper = open_payload["payload"]["dataTickUpperIndex"]
            .as_i64()
            .map(|t| t as u64);

        let key_position = payload["keyPosition"].as_str().unwrap_or("").to_string();

        let (amount_a, amount_b) = self.match_token_amounts(
            payload["keyTokenVaultA"].as_str().unwrap(),
            payload["keyTokenVaultB"].as_str().unwrap(),
            payload["transferAmount0"].as_str().unwrap_or("0"),
            payload["transferAmount1"].as_str().unwrap_or("0"),
        );

        TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time,
            block_time_utc: Utc.timestamp_opt(block_time, 0).unwrap(),
            transaction_type: "IncreaseLiquidity".to_string(),
            ready_for_backtesting: true,
            data: TransactionData::IncreaseLiquidity(LiquidityData {
                token_a: self.token_a_address.clone(),
                token_b: self.token_b_address.clone(),
                amount_a,
                amount_b,
                tick_lower,
                tick_upper,
                key_position: Some(key_position),
            }),
        }
    }

    fn convert_increase_liquidity_v2(
        &self,
        pool_address: &str,
        signature: &str,
        instruction: &Value,
        block_time: i64,
    ) -> TransactionModel {
        let payload = instruction["payload"].as_object().unwrap();

        let (amount_a, amount_b) = self.match_token_amounts(
            payload["keyTokenVaultA"].as_str().unwrap(),
            payload["keyTokenVaultB"].as_str().unwrap(),
            payload["transfer0"]["amount"].as_str().unwrap_or("0"),
            payload["transfer1"]["amount"].as_str().unwrap_or("0"),
        );

        let key_position = payload["keyPosition"].as_str().unwrap_or("").to_string();

        TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time,
            block_time_utc: Utc.timestamp_opt(block_time, 0).unwrap(),
            transaction_type: "IncreaseLiquidityV2".to_string(),
            ready_for_backtesting: false,
            data: TransactionData::IncreaseLiquidity(LiquidityData {
                token_a: self.token_a_address.clone(),
                token_b: self.token_b_address.clone(),
                amount_a,
                amount_b,
                tick_lower: None,
                tick_upper: None,
                key_position: Some(key_position),
            }),
        }
    }

    fn convert_decrease_liquidity(
        &self,
        pool_address: &str,
        signature: &str,
        instruction: &Value,
        block_time: i64,
    ) -> TransactionModel {
        let payload = instruction["payload"].as_object().unwrap();
        let key_position = payload["keyPosition"].as_str().unwrap_or("").to_string();

        let (amount_a, amount_b) = self.match_token_amounts(
            payload["keyTokenVaultA"].as_str().unwrap(),
            payload["keyTokenVaultB"].as_str().unwrap(),
            payload["transferAmount0"].as_str().unwrap_or("0"),
            payload["transferAmount1"].as_str().unwrap_or("0"),
        );

        TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time,
            block_time_utc: Utc.timestamp_opt(block_time, 0).unwrap(),
            transaction_type: "DecreaseLiquidity".to_string(),
            ready_for_backtesting: false,
            data: TransactionData::DecreaseLiquidity(LiquidityData {
                token_a: self.token_a_address.clone(),
                token_b: self.token_b_address.clone(),
                amount_a,
                amount_b,
                tick_lower: None,
                tick_upper: None,
                key_position: Some(key_position),
            }),
        }
    }

    fn convert_decrease_liquidity_v2(
        &self,
        pool_address: &str,
        signature: &str,
        instruction: &Value,
        block_time: i64,
    ) -> TransactionModel {
        let payload = instruction["payload"].as_object().unwrap();

        let (amount_a, amount_b) = self.match_token_amounts(
            payload["keyTokenVaultA"].as_str().unwrap(),
            payload["keyTokenVaultB"].as_str().unwrap(),
            payload["transfer0"]["amount"].as_str().unwrap_or("0"),
            payload["transfer1"]["amount"].as_str().unwrap_or("0"),
        );

        let key_position = payload["keyPosition"].as_str().unwrap_or("").to_string();

        TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time,
            block_time_utc: Utc.timestamp_opt(block_time, 0).unwrap(),
            transaction_type: "DecreaseLiquidityV2".to_string(),
            ready_for_backtesting: false,
            data: TransactionData::DecreaseLiquidity(LiquidityData {
                token_a: self.token_a_address.clone(),
                token_b: self.token_b_address.clone(),
                amount_a,
                amount_b,
                tick_lower: None,
                tick_upper: None,
                key_position: Some(key_position),
            }),
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

        let (whirlpool_key, vault_a_key, vault_b_key, amount_in_key, amount_out_key) =
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
        let vault_b = payload[vault_b_key].as_str().unwrap();

        let amount_in = payload[amount_in_key]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            .unwrap_or(0.0);
        let amount_out = payload[amount_out_key]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            .unwrap_or(0.0);

        let a_to_b = if whirlpool_key == "keyWhirlpoolOne" {
            payload["dataAToBOne"].as_i64().unwrap_or(0) == 1
        } else {
            payload["dataAToBTwo"].as_i64().unwrap_or(0) == 1
        };

        let (token_in, token_out) = if vault_a == self.token_a_vault {
            if a_to_b {
                (self.token_a_address.clone(), self.token_b_address.clone())
            } else {
                (self.token_b_address.clone(), self.token_a_address.clone())
            }
        } else {
            if a_to_b {
                (self.token_b_address.clone(), self.token_a_address.clone())
            } else {
                (self.token_a_address.clone(), self.token_b_address.clone())
            }
        };

        Some(TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time,
            block_time_utc: Utc.timestamp_opt(block_time, 0).unwrap(),
            transaction_type: "TwoHopSwap".to_string(),
            ready_for_backtesting: false,
            data: TransactionData::TwoHopSwap(TwoHopSwapData {
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
    ) -> (f64, f64) {
        let amount0 = amount0.parse::<f64>().unwrap_or(0.0);
        let amount1 = amount1.parse::<f64>().unwrap_or(0.0);

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

    async fn fetch_transactions(
        &self,
        pool_address: &str,
        date: DateTime<Utc>,
        cursor: Cursor,
    ) -> Result<Value> {
        let url = self.construct_url(&date.and_hms(0, 0, 0))?;
        let response = self.http_client.get(&url).send().await?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to download file: HTTP {}",
                response.status()
            ));
        }

        let bytes = response.bytes().await?;
        let gz = GzDecoder::new(&bytes[..]);
        let mut stream = JsonStream::new(gz);

        let mut relevant_blocks = Vec::new();

        while let Some(token) = stream.next()? {
            if let Token::ObjectStart = token {
                if let Some(block) = self.parse_block(&mut stream, pool_address)? {
                    relevant_blocks.push(block);
                }
            }
        }

        Ok(relevant_blocks)
    }

    async fn fetch_and_insert_transactions(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        latest_db_transaction: Option<TransactionModel>,
    ) -> Result<()> {
        let mut current_date = Utc::now().date().pred(); // Start with yesterday
        let start_date = start_time.date();

        while current_date >= start_date {
            // Cursor not used, passed as place holder due to trait format.
            let data = self
                .fetch_transactions(pool_address, current_date, Cursor::OptionalSignature(None))
                .await?;

            let transactions = self.convert_data_to_transactions_model(pool_address, data);

            self.insert_transactions(transactions).await?;

            // Move to the previous day
            current_date = current_date.pred();
        }

        Ok(())
    }

    fn convert_data_to_transactions_model(
        &self,
        pool_address: &str,
        tx_data: Value,
    ) -> Vec<TransactionModel> {
        let block_time = block["blockTime"].as_i64().unwrap_or(0);
        let block_time_utc = Utc.timestamp_opt(block_time, 0).unwrap();

        block["transactions"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|tx| self.convert_single_transaction(tx, block_time_utc, pool_address))
            .collect()
    }
}
