use crate::api::transactions_api::{SignatureInfo, TransactionApi};
use crate::models::transactions_model::{
    LiquidityData, SwapData, TransactionData, TransactionModel,
};
use crate::repositories::transactions_repo::TransactionRepo;
use crate::services::transactions_sync_amm_service::{constants, AMMService};
use crate::utils::hawksight_parsing_tx::{HawksightParser, PoolInfo};
use crate::utils::transaction_utils::retry_with_backoff;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use futures::future::join_all;
use futures::stream::{self, StreamExt};
use serde_json::Value;

use super::transactions_sync_amm_service::constants::{SIGNATURE_BATCH_SIZE, TX_BATCH_SIZE};
use super::transactions_sync_amm_service::Cursor;

#[derive(Clone)]
pub struct OrcaStandardAMM {
    transaction_repo: TransactionRepo,
    transaction_api: TransactionApi,
    token_a_address: String,
    token_b_address: String,
    token_a_decimals: i16,
    token_b_decimals: i16,
}

pub struct CommonTransactionData {
    pub signature: String,
    pub block_time: i64,
    pub block_time_utc: DateTime<Utc>,
    pub account_keys: Vec<String>,
}

impl OrcaStandardAMM {
    pub async fn new(
        transaction_repo: TransactionRepo,
        transaction_api: TransactionApi,
        token_a_address: String,
        token_b_address: String,
        token_a_decimals: i16,
        token_b_decimals: i16,
    ) -> Self {
        Self {
            transaction_repo,
            transaction_api,
            token_a_address,
            token_b_address,
            token_a_decimals,
            token_b_decimals,
        }
    }

    async fn fetch_signatures(
        &self,
        pool_address: &str,
        batch_size: u32,
        before: Option<&str>,
    ) -> Result<Vec<SignatureInfo>> {
        retry_with_backoff(
            || {
                self.transaction_api
                    .fetch_transaction_signatures(pool_address, batch_size, before)
            },
            constants::MAX_RETRIES,
            constants::BASE_DELAY,
            constants::MAX_DELAY,
        )
        .await
        .map_err(|e| anyhow!("Failed to fetch signatures: {:?}", e))
    }

    async fn fetch_transactions_from_signatures(
        &self,
        signatures: &Vec<String>,
    ) -> Result<Vec<serde_json::Value>> {
        retry_with_backoff(
            || self.transaction_api.fetch_transaction_data(signatures),
            constants::MAX_RETRIES,
            constants::BASE_DELAY,
            constants::MAX_DELAY,
        )
        .await
        .map_err(|e| anyhow!("Failed to fetch signatures: {:?}", e))
    }

    pub fn determine_transaction_type(json: &Value) -> Result<String> {
        let log_messages = json["meta"]["logMessages"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Missing logMessages"))?;

        for message in log_messages {
            let message = message.as_str().unwrap_or("");
            if message.contains("Instruction: Swap") || message.contains("Instruction: SwapV2") {
                return Ok("Swap".to_string());
            } else if message.contains("Instruction: TwoHopSwap") {
                return Ok("TwoHopSwap".to_string());
            } else if message.contains("Instruction: IncreaseLiquidity") {
                return Ok("IncreaseLiquidity".to_string());
            } else if message.contains("Instruction: IncreaseLiquidityV2") {
                return Ok("IncreaseLiquidityV2".to_string());
            } else if message.contains("Instruction: DecreaseLiquidity") {
                return Ok("DecreaseLiquidity".to_string());
            } else if message.contains("Instruction: DecreaseLiquidityV2") {
                return Ok("DecreaseLiquidityV2".to_string());
            }
        }

        Err(anyhow!("Unable to determine transaction type"))
    }

    fn extract_common_data(&self, tx_data: &Value) -> Result<CommonTransactionData> {
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

    fn get_token_balances(
        &self,
        json: &Value,
        balance_type: &str,
        pool_address: &str,
    ) -> Result<(String, f64, String, f64)> {
        let balances = json["meta"][balance_type]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Missing token balances"))?;

        let mut token_a_amount = 0.0;
        let mut token_b_amount = 0.0;
        let mut token_a_mint = String::new();
        let mut token_b_mint = String::new();

        for balance in balances {
            let owner = balance["owner"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("Missing owner in token balance"))?;

            if owner == pool_address {
                let mint = balance["mint"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing mint in token balance"))?
                    .to_string();

                let amount = balance["uiTokenAmount"]["uiAmount"]
                    .as_f64()
                    .or_else(|| {
                        balance["uiTokenAmount"]["uiAmountString"]
                            .as_str()
                            .and_then(|s| s.parse::<f64>().ok())
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!("Missing or invalid uiAmount in token balance")
                    })?;

                if mint == self.token_a_address {
                    token_a_amount = amount;
                    token_a_mint = mint;
                } else if mint == self.token_b_address {
                    token_b_amount = amount;
                    token_b_mint = mint;
                }
            }
        }

        Ok((token_a_mint, token_a_amount, token_b_mint, token_b_amount))
    }

    fn extract_liquidity_amounts(&self, tx_data: &Value, pool_address: &str) -> Result<(f64, f64)> {
        let (_, pre_a_amount, _, pre_b_amount) =
            self.get_token_balances(tx_data, "preTokenBalances", pool_address)?;
        let (_, post_a_amount, _, post_b_amount) =
            self.get_token_balances(tx_data, "postTokenBalances", pool_address)?;

        let amount_a = post_a_amount - pre_a_amount;
        let amount_b = post_b_amount - pre_b_amount;

        Ok((amount_a.abs(), amount_b.abs()))
    }

    fn convert_liquidity_data(
        &self,
        tx_data: &Value,
        pool_address: &str,
    ) -> Result<TransactionModel> {
        let common_data = self.extract_common_data(tx_data)?;
        let transaction_type = Self::determine_transaction_type(tx_data)?;

        let (amount_a, amount_b) = self.extract_liquidity_amounts(tx_data, pool_address)?;

        Ok(TransactionModel {
            signature: common_data.signature,
            pool_address: pool_address.to_string(),
            block_time: common_data.block_time,
            block_time_utc: common_data.block_time_utc,
            transaction_type: transaction_type.clone(),
            ready_for_backtesting: false,
            data: match transaction_type.as_str() {
                "IncreaseLiquidity" | "IncreaseLiquidityV2" => {
                    TransactionData::IncreaseLiquidity(LiquidityData {
                        token_a: self.token_a_address.clone(),
                        token_b: self.token_b_address.clone(),
                        amount_a,
                        amount_b,
                        tick_lower: None,
                        tick_upper: None,
                        possible_positions: common_data.account_keys,
                    })
                }
                "DecreaseLiquidity" | "DecreaseLiquidityV2" => {
                    TransactionData::DecreaseLiquidity(LiquidityData {
                        token_a: self.token_a_address.clone(),
                        token_b: self.token_b_address.clone(),
                        amount_a,
                        amount_b,
                        tick_lower: None,
                        tick_upper: None,
                        possible_positions: common_data.account_keys,
                    })
                }
                _ => return Err(anyhow::anyhow!("Unexpected transaction type")),
            },
        })
    }

    fn convert_swap_data(&self, tx_data: &Value, pool_address: &str) -> Result<TransactionModel> {
        let common_data = self.extract_common_data(tx_data)?;
        let (token_in, token_out, amount_in, amount_out) =
            self.extract_swap_amounts(tx_data, pool_address)?;

        Ok(TransactionModel {
            signature: common_data.signature,
            pool_address: pool_address.to_string(),
            block_time: common_data.block_time,
            block_time_utc: common_data.block_time_utc,
            transaction_type: "Swap".to_string(),
            ready_for_backtesting: true,
            data: TransactionData::Swap(SwapData {
                token_in,
                token_out,
                amount_in,
                amount_out,
            }),
        })
    }

    fn extract_swap_amounts(
        &self,
        tx_data: &Value,
        pool_address: &str,
    ) -> Result<(String, String, f64, f64)> {
        let (token_a, pre_a, token_b, pre_b) =
            self.get_token_balances(tx_data, "preTokenBalances", pool_address)?;
        let (_, post_a, _, post_b) =
            self.get_token_balances(tx_data, "postTokenBalances", pool_address)?;

        let (token_in, token_out, amount_in, amount_out) = if pre_a > post_a {
            (token_a, token_b, pre_a - post_a, post_b - pre_b)
        } else {
            (token_b, token_a, pre_b - post_b, post_a - pre_a)
        };

        if amount_in <= 0.0 || amount_out <= 0.0 {
            return Err(anyhow::anyhow!(
                "Invalid swap amounts: in = {}, out = {}",
                amount_in,
                amount_out
            ));
        }

        Ok((token_in, token_out, amount_in, amount_out))
    }
}

#[async_trait]
impl AMMService for OrcaStandardAMM {
    fn repo(&self) -> &TransactionRepo {
        &self.transaction_repo
    }

    fn api(&self) -> &TransactionApi {
        &self.transaction_api
    }

    async fn fetch_transactions(&self, pool_address: &str, cursor: Cursor) -> Result<Vec<Value>> {
        let optional_signature = match cursor {
            Cursor::OptionalSignature(sig) => sig,
            Cursor::DateTime(_) => None,
        };

        let signatures = self
            .fetch_signatures(
                pool_address,
                SIGNATURE_BATCH_SIZE,
                optional_signature.as_deref(),
            )
            .await?;

        let filtered_signatures: Vec<SignatureInfo> = signatures
            .into_iter()
            .filter(|sig| sig.err.is_none())
            .collect();

        println!(
            "Fetched filtered signatures: {}. Now fetching txs.",
            filtered_signatures.len()
        );

        let signature_chunks: Vec<Vec<String>> = filtered_signatures
            .chunks(TX_BATCH_SIZE)
            .map(|chunk| chunk.iter().map(|sig| sig.signature.clone()).collect())
            .collect();

        let fetch_futures = signature_chunks.into_iter().map(|chunk| {
            let chunk_clone = chunk.clone(); // Clone the chunk
            async move { self.fetch_transactions_from_signatures(&chunk_clone).await }
        });

        let all_tx_data: Vec<Value> = stream::iter(fetch_futures)
            .buffer_unordered(3)
            .flat_map(|result| stream::iter(result.unwrap_or_default()))
            .collect()
            .await;

        let futures = all_tx_data.into_iter().map(|tx_data| async move {
            if Self::determine_transaction_type(&tx_data).is_ok() {
                Some(tx_data)
            } else {
                None
            }
        });

        let all_relevant_transactions: Vec<Value> = join_all(futures)
            .await
            .into_iter()
            .filter_map(|x| x)
            .collect();

        println!(
            "Processed {} relevant transactions.",
            all_relevant_transactions.len()
        );

        Ok(all_relevant_transactions)
    }

    fn convert_data_to_transactions_model(
        &self,
        pool_address: &str,
        tx_data: Vec<Value>,
    ) -> Result<Vec<TransactionModel>> {
        let mut transactions = Vec::new();

        for transaction in tx_data {
            if HawksightParser::is_hawksight_transaction(&transaction) {
                let pool_info = PoolInfo {
                    address: pool_address.to_string(),
                    token_a: self.token_a_address.clone(),
                    token_b: self.token_b_address.clone(),
                    decimals_a: self.token_a_decimals,
                    decimals_b: self.token_b_decimals,
                };
                let common_data = self.extract_common_data(&transaction)?;

                if let Ok(hawksight_transactions) =
                    HawksightParser::parse_hawksight_program(&transaction, &pool_info, &common_data)
                {
                    transactions.extend(hawksight_transactions);
                }
            } else {
                match Self::determine_transaction_type(&transaction)?.as_str() {
                    "Swap" | "TwoHopSwap" => {
                        if let Ok(transaction_model) =
                            self.convert_swap_data(&transaction, pool_address)
                        {
                            if let TransactionData::Swap(swap_data) = &transaction_model.data {
                                transactions.push(transaction_model);
                            } else {
                                // This block is technically unreachable bcos it will always be swap data.
                                unreachable!("Expected Swap data for Swap transaction");
                            }
                        }
                    }
                    "IncreaseLiquidity"
                    | "DecreaseLiquidity"
                    | "IncreaseLiquidityV2"
                    | "DecreaseLiquidityV2" => {
                        if let Ok(liquidity_data) =
                            self.convert_liquidity_data(&transaction, pool_address)
                        {
                            transactions.push(liquidity_data);
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(transactions)
    }

    async fn fetch_and_insert_transactions(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        latest_db_transaction: Option<TransactionModel>,
    ) -> Result<()> {
        let mut cursor = if let Some(latest_tx) = latest_db_transaction {
            Cursor::OptionalSignature(Some(latest_tx.signature))
        } else {
            Cursor::OptionalSignature(None)
        };

        loop {
            let transactions = self
                .fetch_transactions(pool_address, cursor.clone())
                .await?;

            let transaction_models =
                self.convert_data_to_transactions_model(pool_address, transactions.clone())?;

            self.insert_transactions(transaction_models).await?;

            // Update cursor for the next iteration
            if let Some(last_transaction) = transactions.last() {
                if let Some(signature) = last_transaction["transaction"]["signatures"].get(0) {
                    cursor =
                        Cursor::OptionalSignature(Some(signature.as_str().unwrap().to_string()));
                }
            }

            // Check if we've reached or gone past the start_time
            if let Some(first_transaction) = transactions.first() {
                let block_time = first_transaction["blockTime"].as_i64().unwrap_or(0);
                let transaction_time = Utc.timestamp_opt(block_time, 0).unwrap();
                if transaction_time <= start_time {
                    println!("Reached start_time limit. Exiting.");
                    break;
                }
            }
        }

        Ok(())
    }
}
