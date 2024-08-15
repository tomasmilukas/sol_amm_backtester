use crate::models::transactions_model::{LiquidityData, SwapData, TransactionModel};
use crate::repositories::transactions_repo::TransactionRepo;
use crate::utils::transaction_utils;
use crate::{api::transaction_api::TransactionApi, models::transactions_model::TransactionData};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use std::time::Duration;

pub struct TransactionService {
    transaction_repo: TransactionRepo,
    transaction_api: TransactionApi,
    token_a_address: String,
    token_b_address: String,
}

impl TransactionService {
    pub fn new(
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

    pub async fn sync_transactions(
        &self,
        pool_address: &str,
        desired_start_time: DateTime<Utc>,
        desired_end_time: DateTime<Utc>,
        full_sync: bool,
    ) -> Result<u64> {
        let mut total_synced = 0;

        if full_sync {
            // Full sync: from end time to start time
            total_synced += self
                .sync_backward(pool_address, desired_start_time, desired_end_time)
                .await?;
        } else {
            // Regular sync: forward and backward as needed
            let highest_block_time = self
                .transaction_repo
                .fetch_highest_block_time(pool_address)
                .await?;
            let lowest_block_time = self
                .transaction_repo
                .fetch_lowest_block_time(pool_address)
                .await?;

            // Sync forward from highest_block_time to desired_end_time
            if let Some(highest_time) = highest_block_time {
                if highest_time < desired_end_time {
                    total_synced += self
                        .sync_forward(pool_address, highest_time, desired_end_time)
                        .await?;
                }
            } else {
                // If no highest_block_time, sync everything forward
                total_synced += self
                    .sync_forward(pool_address, desired_start_time, desired_end_time)
                    .await?;
            }

            // Sync backward from lowest_block_time to desired_start_time
            if let Some(lowest_time) = lowest_block_time {
                if lowest_time > desired_start_time {
                    total_synced += self
                        .sync_backward(pool_address, desired_start_time, lowest_time)
                        .await?;
                }
            }
        }

        Ok(total_synced)
    }

    async fn sync_forward(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Result<u64> {
        let mut synced_count = 0;
        let mut current_time = start_time;
        let batch_size = 10;

        while current_time < end_time {
            let signatures = self
                .transaction_api
                .fetch_transaction_signatures(pool_address, batch_size, None)
                .await?;

            if signatures.is_empty() {
                break;
            }

            for sig_info in signatures.iter() {
                if let Some(block_time) = sig_info.blockTime {
                    let tx_time = Utc
                        .timestamp_opt(block_time, 0)
                        .single()
                        .context("Invalid timestamp")?;

                    if tx_time > current_time && tx_time <= end_time {
                        let tx_data = self
                            .transaction_api
                            .fetch_transaction_data(&sig_info.signature)
                            .await?;

                        let transaction_model =
                            self.convert_to_transaction_model(pool_address, &tx_data)?;

                        self.transaction_repo.insert(&transaction_model).await?;

                        synced_count += 1;
                        current_time = tx_time;
                    } else if tx_time > end_time {
                        return Ok(synced_count);
                    }
                }
            }

            // Add a small delay between batches to avoid overwhelming the API
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Ok(synced_count)
    }

    async fn sync_backward(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Result<u64> {
        let mut synced_count = 0;
        let mut before_signature = None;
        let batch_size = 1000;

        loop {
            let signatures = self
                .transaction_api
                .fetch_transaction_signatures(pool_address, batch_size, before_signature.as_deref())
                .await?;

            if signatures.is_empty() {
                break;
            }

            for sig_info in signatures.iter().rev() {
                if let Some(block_time) = sig_info.blockTime {
                    let tx_time = Utc
                        .timestamp_opt(block_time, 0)
                        .single()
                        .context("Invalid timestamp")?;
                    if tx_time >= start_time && tx_time < end_time {
                        let tx_data = self
                            .transaction_api
                            .fetch_transaction_data(&sig_info.signature)
                            .await?;

                        let transaction_model =
                            self.convert_to_transaction_model(pool_address, &tx_data)?;

                        self.transaction_repo.insert(&transaction_model).await?;

                        synced_count += 1;
                    } else if tx_time < start_time {
                        return Ok(synced_count);
                    }
                }
            }

            before_signature = signatures.last().map(|sig| sig.signature.clone());

            // Add a small delay between batches to avoid overwhelming the API
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Ok(synced_count)
    }

    pub fn convert_to_transaction_model(
        &self,
        pool_address: &str,
        json: &Value,
    ) -> Result<TransactionModel> {
        // Check if this transaction involves our pool
        let post_token_balances = json["meta"]["postTokenBalances"]
            .as_array()
            .context("Missing postTokenBalances")?;

        let pool_involved = post_token_balances
            .iter()
            .any(|balance| balance["owner"].as_str() == Some(pool_address));

        if !pool_involved {
            return Err(anyhow!("Transaction does not involve the specified pool"));
        }

        let signature = json["transaction"]["signatures"][0]
            .as_str()
            .context("Missing signature")?
            .to_string();

        let block_time = json["blockTime"].as_i64().context("Missing blockTime")?;
        let block_time = Utc
            .timestamp_opt(block_time, 0)
            .single()
            .context("Invalid blockTime")?;

        let slot = json["slot"].as_i64().context("Missing slot")?;

        // Use the utility function to determine the transaction type
        let transaction_type = transaction_utils::determine_transaction_type(json)?;

        // Use the utility function to find pool balance changes
        let (token_a, token_b, amount_a, amount_b) = transaction_utils::find_pool_balance_changes(
            json,
            pool_address,
            &self.token_a_address,
            &self.token_b_address,
        )?;

        let transaction_data = match transaction_type.as_str() {
            "Swap" => TransactionData::Swap(SwapData {
                token_in: if amount_a < 0.0 {
                    token_a.clone()
                } else {
                    token_b.clone()
                },
                token_out: if amount_a < 0.0 { token_b } else { token_a },
                amount_in: amount_a.abs().max(amount_b.abs()),
                amount_out: amount_a.abs().min(amount_b.abs()),
            }),
            "AddLiquidity" => TransactionData::AddLiquidity(LiquidityData {
                token_a,
                token_b,
                amount_a: amount_a.abs(),
                amount_b: amount_b.abs(),
            }),
            "DecreaseLiquidity" => TransactionData::DecreaseLiquidity(LiquidityData {
                token_a,
                token_b,
                amount_a: amount_a.abs(),
                amount_b: amount_b.abs(),
            }),
            _ => {
                return Err(anyhow!(
                    "Unsupported transaction type: {}",
                    transaction_type
                ))
            }
        };

        Ok(TransactionModel::new(
            signature,
            pool_address.to_string(),
            block_time,
            slot,
            transaction_type,
            transaction_data,
        ))
    }
}
