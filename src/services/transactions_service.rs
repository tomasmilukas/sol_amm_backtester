use crate::config::SyncMode;
use crate::models::transactions_model::{LiquidityData, SwapData, TransactionModel};
use crate::repositories::transactions_repo::TransactionRepo;
use crate::utils::transaction_utils;
use crate::{api::transactions_api::TransactionApi, models::transactions_model::TransactionData};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use futures::future::{join_all, FutureExt};
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
        start_time: DateTime<Utc>,
        sync_mode: SyncMode,
    ) -> Result<()> {
        match sync_mode {
            // Update sync uses the most recent transaction from db and updates from the current time to that transaction.
            SyncMode::Update => self.update_sync(pool_address).await,
            // Historical sync uses the oldest transaction from db and updates from the "start time" passed in until it reaches that transaction.
            SyncMode::Historical => self.historical_sync(pool_address, start_time).await,
            // Ignores all transactions in db and just full syncs from start_time to end_time.
            SyncMode::FullRange => self.full_range_sync(pool_address, start_time).await,
        }
    }

    async fn update_sync(&self, pool_address: &str) -> Result<()> {
        let highest_block_tx = self
            .transaction_repo
            .fetch_highest_block_time_transaction(pool_address)
            .await?;

        if let Some(tx) = highest_block_tx {
            self.fetch_and_insert_transactions(pool_address, tx.block_time_utc, None)
                .await?;
            Ok(())
        } else {
            Err(anyhow!("No existing transactions found for update sync"))
        }
    }

    async fn historical_sync(&self, pool_address: &str, start_time: DateTime<Utc>) -> Result<()> {
        let lowest_block_tx = self
            .transaction_repo
            .fetch_lowest_block_time_transaction(pool_address)
            .await?;

        if let Some(tx) = lowest_block_tx {
            self.fetch_and_insert_transactions(pool_address, start_time, Some(tx.signature))
                .await?;
            Ok(())
        } else {
            Err(anyhow!("No existing transactions found for update sync"))
        }
    }

    async fn full_range_sync(&self, pool_address: &str, start_time: DateTime<Utc>) -> Result<()> {
        self.fetch_and_insert_transactions(pool_address, start_time, None)
            .await?;
        Ok(())
    }

    pub async fn fetch_and_insert_transactions(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        signature: Option<String>,
    ) -> Result<()> {
        let batch_size = 100;
        let mut signature_to_pass = signature;

        loop {
            let signatures = self
                .transaction_api
                .fetch_transaction_signatures(
                    pool_address,
                    batch_size,
                    signature_to_pass.as_deref(),
                )
                .await?;

            println!("New batch of signatures: {}", signatures.len());

            if signatures.is_empty() {
                println!("No more signatures to process");
                return Ok(());
            }

            // Clone the last signature before consuming the vector
            signature_to_pass = signatures.last().map(|sig| sig.signature.clone());

            let transaction_futures = signatures.into_iter().map(|sig_info| {
                async move {
                    let block_tx_time = Utc
                        .timestamp_opt(sig_info.block_time, 0)
                        .single()
                        .ok_or_else(|| anyhow!("Invalid timestamp"))?;

                    if block_tx_time > start_time {
                        println!("Processing transaction: {}", sig_info.signature);
                        match self
                            .process_transaction_data(pool_address, &sig_info.signature)
                            .await
                        {
                            Ok(_) => {
                                println!(
                                    "Successfully processed. Current time: {:?}",
                                    block_tx_time
                                );
                                Ok(())
                            }
                            Err(e) => Err(anyhow!(
                                "Failed to process transaction: {}",
                                sig_info.signature
                            )),
                        }
                    } else {
                        Err(anyhow!("Reached start_time limit"))
                    }
                }
                .boxed()
            });

            let results = join_all(transaction_futures).await;

            for result in results {
                if let Err(e) = result {
                    if e.to_string().contains("Reached start_time limit") {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn process_transaction_data(&self, pool_address: &str, signature: &str) -> Result<()> {
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
            Ok(())
        } else {
            Err(anyhow!("Not a desired transaction type."))
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
        let block_time_utc = Utc
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
            block_time_utc,
            slot,
            transaction_type,
            transaction_data,
        ))
    }
}
