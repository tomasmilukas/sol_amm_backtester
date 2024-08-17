use crate::api::transactions_api::TransactionApi;
use crate::api::transactions_api::{ApiError, SignatureInfo};
use crate::config::SyncMode;
use crate::models::transactions_model::TransactionModel;
use crate::repositories::transactions_repo::TransactionRepo;
use anyhow::{anyhow, Result};
use chrono::{DateTime, TimeZone, Utc};
use futures::future::{join_all, FutureExt};
use rand::Rng;
use tokio::time::{sleep, Duration};

const MAX_RETRIES: u32 = 3;
const BASE_DELAY: u64 = 2000; // 2s
const RATE_LIMIT_DELAY: u64 = 30000; // 30s

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
            let signatures = match self
                .fetch_signatures_with_retry(pool_address, batch_size, signature_to_pass.as_deref())
                .await
            {
                Ok(sigs) => sigs,
                Err(ApiError::RateLimit) => {
                    println!(
                        "Rate limit hit. Waiting for {} seconds before retrying...",
                        RATE_LIMIT_DELAY / 1000
                    );
                    sleep(Duration::from_millis(RATE_LIMIT_DELAY)).await;
                    continue;
                }
                Err(ApiError::Other(e)) => return Err(e),
            };

            // Sleep after fetching signatures
            sleep(Duration::from_secs(6)).await;

            // Filter out failed transactions
            let successful_signatures: Vec<SignatureInfo> = signatures
                .into_iter()
                .filter(|sig| sig.err.is_none())
                .collect();

            println!("New batch of signatures: {}", successful_signatures.len());

            if successful_signatures.is_empty() {
                println!("No more signatures to process");
                return Ok(());
            }

            // Clone the last signature before consuming the vector
            signature_to_pass = successful_signatures
                .last()
                .map(|sig| sig.signature.clone());

            println!("{}", start_time);

            let transaction_futures = successful_signatures.into_iter().map(|sig_info| {
                async move {
                    let block_tx_time = Utc
                        .timestamp_opt(sig_info.block_time, 0)
                        .single()
                        .ok_or_else(|| anyhow!("Invalid timestamp"))?;

                    // Sleep after processing each transaction
                    sleep(Duration::from_millis(400)).await;

                    if block_tx_time > start_time {
                        match self
                            .process_transaction_data_with_retry(pool_address, &sig_info.signature)
                            .await
                        {
                            Ok(_) => {
                                println!(
                                    "Successfully processedt tx. Current tx: {:?}",
                                    sig_info.signature
                                );
                                Ok(())
                            }
                            Err(ApiError::RateLimit) => {
                                println!("Rate limit hit. Skipping this transaction.");
                                Ok(()) // We're treating rate limit as a soft error here
                            }
                            Err(ApiError::Other(e)) => Err(anyhow!(
                                "Failed to process transaction: {}. Error: {}",
                                sig_info.signature,
                                e
                            )),
                        }
                    } else {
                        Err(anyhow!("Reached start_time limit"))
                    }
                }
                .boxed()
            });

            let results = join_all(transaction_futures).await;

            // Add a longer delay after processing all transactions in the batch
            sleep(Duration::from_secs(13)).await;

            for result in results {
                if let Err(e) = result {
                    if e.to_string().contains("Reached start_time limit") {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn fetch_signatures_with_retry(
        &self,
        pool_address: &str,
        batch_size: u32,
        before: Option<&str>,
    ) -> Result<Vec<SignatureInfo>, ApiError> {
        for attempt in 0..MAX_RETRIES {
            match self
                .transaction_api
                .fetch_transaction_signatures(pool_address, batch_size, before)
                .await
            {
                Ok(signatures) => return Ok(signatures),
                Err(ApiError::RateLimit) => {
                    if attempt < MAX_RETRIES - 1 {
                        println!("Rate limit hit. Retrying after delay...");
                        sleep(Duration::from_millis(RATE_LIMIT_DELAY)).await;
                    } else {
                        return Err(ApiError::RateLimit);
                    }
                }
                Err(ApiError::Other(e)) => return Err(ApiError::Other(e)),
            }
        }

        Err(ApiError::Other(anyhow!(
            "Max retries reached while fetching signatures"
        )))
    }

    async fn process_transaction_data_with_retry(
        &self,
        pool_address: &str,
        signature: &str,
    ) -> Result<(), ApiError> {
        for attempt in 0..MAX_RETRIES {
            match self.process_transaction_data(pool_address, signature).await {
                Ok(()) => return Ok(()),
                Err(ApiError::RateLimit) => {
                    if attempt < MAX_RETRIES - 1 {
                        println!("Rate limit hit. Retrying after delay...");
                        sleep(Duration::from_millis(RATE_LIMIT_DELAY)).await;
                    } else {
                        return Err(ApiError::RateLimit);
                    }
                }
                Err(ApiError::Other(e)) => return Err(ApiError::Other(e)),
            }
        }

        Err(ApiError::Other(anyhow!(
            "Max retries reached while processing transaction data"
        )))
    }

    fn calculate_delay(&self, attempt: u32) -> Duration {
        let mut rng = rand::thread_rng();
        let jitter = rng.gen_range(0..1000);
        Duration::from_millis(BASE_DELAY * 2u64.pow(attempt) + jitter)
    }

    async fn process_transaction_data(
        &self,
        pool_address: &str,
        signature: &str,
    ) -> Result<(), ApiError> {
        let tx_data = self
            .transaction_api
            .fetch_transaction_data(signature)
            .await?;

        let model = TransactionModel::convert_from_json(
            pool_address,
            &tx_data,
            &self.token_a_address,
            &self.token_b_address,
        )
        .map_err(|e| ApiError::Other(e.into()))?;

        if matches!(
            model.transaction_type.as_str(),
            "Swap" | "AddLiquidity" | "DecreaseLiquidity"
        ) {
            self.transaction_repo
                .insert(&model)
                .await
                .map_err(|e| ApiError::Other(e.into()))?;

            Ok(())
        } else {
            Err(ApiError::Other(anyhow!("Not a desired transaction type.")))
        }
    }
}
