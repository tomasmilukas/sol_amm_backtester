use crate::models::transactions_model::{LiquidityData, SwapData, TransactionModel};
use crate::repositories::transactions_repo::TransactionRepo;
use crate::utils::transaction_utils;
use crate::{api::transactions_api::TransactionApi, models::transactions_model::TransactionData};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

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

            println!("{} {}", desired_end_time, desired_start_time);
            println!("{:?} {:?}", highest_block_time, lowest_block_time);

            // Sync forward from highest_block_time to desired_end_time
            if let Some(highest_time) = highest_block_time {
                println!("1");

                if highest_time < desired_end_time {
                    total_synced += self
                        .sync_forward(pool_address, highest_time, desired_end_time)
                        .await?;
                }
            } else {
                println!("2");

                // If no highest_block_time, sync everything forward
                total_synced += self
                    .sync_forward(pool_address, desired_start_time, desired_end_time)
                    .await?;
            }

            // Sync backward from lowest_block_time to desired_start_time
            if let Some(lowest_time) = lowest_block_time {
                println!("3");

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
        let mut last_signature = None;
        let batch_size = 100;

        while current_time < end_time {
            println!(
                "Fetching signatures. Current time: {}, End time: {}",
                current_time, end_time
            );
            let signatures = self
                .transaction_api
                .fetch_transaction_signatures(pool_address, batch_size, last_signature.as_deref())
                .await?;

            println!("New batch of signatures: {}", signatures.len());

            if signatures.is_empty() {
                println!("No more signatures to process");
                break;
            }

            let mut batch_earliest_time = current_time;

            for sig_info in signatures.iter() {
                if let Some(block_time) = sig_info.blockTime {
                    let tx_time = Utc
                        .timestamp_opt(block_time, 0)
                        .single()
                        .context("Invalid timestamp")?;

                    println!(
                        "Evaluating signature: {}. Time: {}",
                        sig_info.signature, tx_time
                    );

                    if tx_time < batch_earliest_time {
                        batch_earliest_time = tx_time;
                    }

                    if tx_time >= current_time && tx_time <= end_time {
                        println!("Processing transaction: {}", sig_info.signature);
                        match self
                            .process_transaction(pool_address, &sig_info.signature)
                            .await
                        {
                            Ok(_) => {
                                synced_count += 1;
                                if tx_time > current_time {
                                    current_time = tx_time;
                                }
                                println!("Successfully processed. Current time: {}", current_time);
                            }
                            Err(e) => println!(
                                "Failed to process transaction: {}. Error: {:?}",
                                sig_info.signature, e
                            ),
                        }
                    } else if tx_time > end_time {
                        println!("Transaction time is beyond end time. Stopping sync.");
                        return Ok(synced_count);
                    } else {
                        println!(
                            "Skipping transaction: {}. Time out of range.",
                            sig_info.signature
                        );
                    }
                } else {
                    println!(
                        "Skipping transaction: {}. No block time.",
                        sig_info.signature
                    );
                }
            }

            // Update current_time to the earliest time in the batch if no transactions were processed
            if synced_count == 0 {
                current_time = batch_earliest_time;
            }

            last_signature = signatures.last().map(|sig| sig.signature.clone());
            println!(
                "Batch processed. Synced count: {}. Last signature: {:?}",
                synced_count, last_signature
            );
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        println!("Sync forward completed. Total synced: {}", synced_count);
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
        let batch_size = 100;

        loop {
            let signatures = self
                .transaction_api
                .fetch_transaction_signatures(pool_address, batch_size, before_signature.as_deref())
                .await?;

            println!("New signatures batch 1!");

            if signatures.is_empty() {
                println!("Empty :(");
                break;
            }

            for sig_info in signatures.iter().rev() {
                if let Some(block_time) = sig_info.blockTime {
                    let tx_time = Utc
                        .timestamp_opt(block_time, 0)
                        .single()
                        .context("Invalid timestamp")?;
                    if tx_time >= start_time && tx_time <= end_time {
                        self.process_transaction(pool_address, &sig_info.signature)
                            .await;
                        synced_count += 1;
                    } else if tx_time < start_time {
                        return Ok(synced_count);
                    }
                }
            }

            before_signature = signatures.last().map(|sig| sig.signature.clone());
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        Ok(synced_count)
    }

    async fn process_transaction(&self, pool_address: &str, signature: &str) -> Result<()> {
        let tx_data = self
            .transaction_api
            .fetch_transaction_data(signature)
            .await?;
        let model = self.convert_to_transaction_model(pool_address, &tx_data)?;

        if matches!(
            model.transaction_type.as_str(),
            "Swap" | "AddLiquidity" | "DecreaseLiquidity"
        ) {
            self.transaction_repo.insert(&model).await?;
            println!(
                "Transaction inserted: {} {}",
                model.transaction_type, model.signature
            );
            Ok(())
        } else {
            Err(anyhow!(
                "Unsupported transaction type: {}",
                model.transaction_type
            ))
        }
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

        if amount_a == 0.0 || amount_b == 0.0 {
            println!("Skip transfer");
        }

        let transaction_data = match transaction_type.as_str() {
            "Swap" => TransactionData::Swap(SwapData {
                token_in: if amount_a < 0.0 {
                    token_b.clone()
                } else {
                    token_a.clone()
                },
                token_out: if amount_a < 0.0 { token_a } else { token_b },
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
