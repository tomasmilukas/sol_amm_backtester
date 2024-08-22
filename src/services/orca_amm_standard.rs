use crate::api::transactions_api::{SignatureInfo, TransactionApi};
use crate::models::transactions_model::TransactionModel;
use crate::repositories::transactions_repo::TransactionRepo;
use crate::services::transactions_amm_service::{constants, AMMService};
use crate::utils::transaction_utils::retry_with_backoff;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::transactions_amm_service::Cursor;

pub struct OrcaStandardAMM {
    transaction_repo: TransactionRepo,
    transaction_api: TransactionApi,
    token_a_address: String,
    token_b_address: String,
}

impl OrcaStandardAMM {
    pub async fn new(
        transaction_repo: TransactionRepo,
        transaction_api: TransactionApi,
        token_a_address: String,
        token_b_address: String,
    ) -> Self {
        Self {
            transaction_repo,
            transaction_api,
            token_a_address,
            token_b_address,
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

    async fn fetch_transactions_with_retry(
        &self,
        pool_address: &str,
        signatures: &[String],
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
            .context("Missing logMessages")?;

        for message in log_messages {
            let message = message.as_str().unwrap_or("");
            if message.contains("Instruction: Swap") {
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

    fn extract_common_data(
        &self,
        tx_data: &Value,
        pool_address: &str,
    ) -> Result<CommonTransactionData> {
        let signature = tx_data["transaction"]["signatures"][0]
            .as_str()
            .context("Missing signature")?
            .to_string();
        let block_time = tx_data["blockTime"].as_i64().context("Missing blockTime")?;
        let block_time_utc =
            DateTime::<Utc>::from_timestamp(block_time, 0).context("Invalid blockTime")?;

        let pre_balances = self.get_token_balances(tx_data, "preTokenBalances", pool_address)?;
        let post_balances = self.get_token_balances(tx_data, "postTokenBalances", pool_address)?;

        let amount_a = post_balances
            .get(&self.token_a_address)
            .copied()
            .unwrap_or(0.0)
            - pre_balances
                .get(&self.token_a_address)
                .copied()
                .unwrap_or(0.0);
        let amount_b = post_balances
            .get(&self.token_b_address)
            .copied()
            .unwrap_or(0.0)
            - pre_balances
                .get(&self.token_b_address)
                .copied()
                .unwrap_or(0.0);

        Ok(CommonTransactionData {
            signature,
            block_time,
            block_time_utc,
            amount_a,
            amount_b,
        })
    }

    fn get_token_balances(
        &self,
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

    fn extract_liquidity_amounts(&self, tx_data: &Value, pool_address: &str) -> Result<(f64, f64)> {
        let pre_balances = self.get_token_balances(tx_data, "preTokenBalances", pool_address)?;
        let post_balances = self.get_token_balances(tx_data, "postTokenBalances", pool_address)?;

        let amount_a = post_balances
            .get(&self.token_a_address)
            .copied()
            .unwrap_or(0.0)
            - pre_balances
                .get(&self.token_a_address)
                .copied()
                .unwrap_or(0.0);
        let amount_b = post_balances
            .get(&self.token_b_address)
            .copied()
            .unwrap_or(0.0)
            - pre_balances
                .get(&self.token_b_address)
                .copied()
                .unwrap_or(0.0);

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
            ready_for_backtesting: true,
            data: match transaction_type.as_str() {
                "IncreaseLiquidity" | "IncreaseLiquidityV2" => {
                    TransactionData::IncreaseLiquidity(LiquidityData {
                        key_position: "".to_string(),
                        token_a: self.token_a_address.clone(),
                        token_b: self.token_b_address.clone(),
                        amount_a,
                        amount_b,
                        tick_lower: None,
                        tick_upper: None,
                    })
                }
                "DecreaseLiquidity" | "DecreaseLiquidityV2" => {
                    TransactionData::DecreaseLiquidity(LiquidityData {
                        key_position: "".to_string(),
                        token_a: self.token_a_address.clone(),
                        token_b: self.token_b_address.clone(),
                        amount_a,
                        amount_b,
                        tick_lower: None,
                        tick_upper: None,
                    })
                }
                _ => return Err(anyhow::anyhow!("Unexpected transaction type")),
            },
        })
    }

    fn convert_swap_data(&self, tx_data: &Value, pool_address: &str) -> Result<TransactionModel> {
        let common_data = self.extract_common_data(tx_data)?;
        let transaction_type = Self::determine_transaction_type(tx_data)?;

        let (token_in, token_out, amount_in, amount_out) =
            self.extract_swap_amounts(tx_data, pool_address, &transaction_type)?;

        Ok(TransactionModel {
            signature: common_data.signature,
            pool_address: pool_address.to_string(),
            block_time: common_data.block_time,
            block_time_utc: common_data.block_time_utc,
            transaction_type,
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
        transaction_type: &str,
    ) -> Result<(String, String, f64, f64)> {
        let pre_balances = self.get_token_balances(tx_data, "preTokenBalances", pool_address)?;
        let post_balances = self.get_token_balances(tx_data, "postTokenBalances", pool_address)?;

        let (token_in, token_out) = if transaction_type == "TwoHopSwap" {
            self.identify_two_hop_tokens(&pre_balances, &post_balances)?
        } else {
            (self.token_a_address.clone(), self.token_b_address.clone())
        };

        let amount_in = pre_balances.get(&token_in).copied().unwrap_or(0.0)
            - post_balances.get(&token_in).copied().unwrap_or(0.0);
        let amount_out = post_balances.get(&token_out).copied().unwrap_or(0.0)
            - pre_balances.get(&token_out).copied().unwrap_or(0.0);

        if amount_in > 0.0 && amount_out < 0.0 {
            Ok((token_in, token_out, amount_in.abs(), amount_out.abs()))
        } else if amount_in < 0.0 && amount_out > 0.0 {
            Ok((token_out, token_in, amount_out.abs(), amount_in.abs()))
        } else {
            Err(anyhow::anyhow!("Unable to determine swap direction"))
        }
    }

    fn identify_two_hop_tokens(
        &self,
        pre_balances: &HashMap<String, f64>,
        post_balances: &HashMap<String, f64>,
    ) -> Result<(String, String)> {
        let token_in = pre_balances
            .iter()
            .find(|(_, &amount)| amount > 0.0)
            .map(|(token, _)| token.clone())
            .ok_or_else(|| anyhow::anyhow!("Unable to determine input token"))?;

        let token_out = post_balances
            .iter()
            .find(|(token, &amount)| amount > 0.0 && *token != token_in)
            .map(|(token, _)| token.clone())
            .ok_or_else(|| anyhow::anyhow!("Unable to determine output token"))?;

        Ok((token_in, token_out))
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

    async fn fetch_transactions(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        cursor: Cursor,
    ) -> Result<Vec<Value>> {
        let signatures = self
            .fetch_signatures_with_retry(pool_address, SIGNATURE_BATCH_SIZE, cursor.as_deref())
            .await?;

        let filtered_signatures: Vec<SignatureInfo> = signatures
            .into_iter()
            .filter(|sig| sig.err.is_none())
            .collect();

        let mut all_relevant_transactions = Vec::new();

        // Process transactions in batches of 10
        for chunk in filtered_signatures.chunks(10) {
            let signature_strings: Vec<String> =
                chunk.iter().map(|sig| sig.signature.clone()).collect();

            let tx_data_batch = self
                .fetch_transactions_with_retry(pool_address, &signature_strings)
                .await?;

            let futures = tx_data_batch.into_iter().map(|tx_data| async move {
                if Self::determine_transaction_type(&tx_data).is_ok() {
                    Some(tx_data)
                } else {
                    None
                }
            });

            let batch_results = join_all(futures).await;
            all_relevant_transactions.extend(batch_results.into_iter().filter_map(|x| x));
        }

        Ok(all_relevant_transactions)
    }

    fn convert_data_to_transactions_model(
        &self,
        pool_address: &str,
        tx_data: &Value,
    ) -> Result<Vec<TransactionModel>> {
        let mut transactions = Vec::new();

        match Self::determine_transaction_type(tx_data)?.as_str() {
            "Swap" => {
                if let Ok(swap_data) = self.convert_swap_data(tx_data, pool_address) {
                    transactions.push(swap_data);
                }
            }
            "IncreaseLiquidity"
            | "DecreaseLiquidity"
            | "IncreaseLiquidityV2"
            | "DecreaseLiquidityV2" => {
                if let Ok(liquidity_data) = self.convert_liquidity_data(tx_data, pool_address) {
                    transactions.push(liquidity_data);
                }
            }
            _ => {}
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
                .fetch_transactions(pool_address, start_time, cursor)
                .await?;

            println!("Processing {} transactions", transactions.len());

            let transaction_models =
                self.convert_data_to_transactions_model(pool_address, transactions.clone());

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
