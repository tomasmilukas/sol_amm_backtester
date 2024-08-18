use crate::api::transactions_api::{ApiError, SignatureInfo, TransactionApi};
use crate::config::SyncMode;
use crate::models::transactions_model::TransactionModel;
use crate::repositories::transactions_repo::TransactionRepo;
use anyhow::{anyhow, Result};
use chrono::{DateTime, TimeZone, Utc};
use futures::future::join_all;
use tokio::time::Duration;
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

const MAX_RETRIES: u32 = 5;
const BASE_DELAY: u64 = 5000; // 5 second
const MAX_DELAY: u64 = 300_000; // 5 minutes
const SIGNATURE_BATCH_SIZE: u32 = 1000; // Maximum signatures to fetch in one batch
const TX_BATCH_SIZE: usize = 10; // Maximum transactions to process in one batch

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
            SyncMode::FullRange => {
                self.fetch_and_insert_transactions(pool_address, start_time, None)
                    .await
            }
        }
    }

    async fn update_sync(&self, pool_address: &str) -> Result<()> {
        let highest_block_tx = self
            .transaction_repo
            .fetch_highest_block_time_transaction(pool_address)
            .await?;

        match highest_block_tx {
            Some(tx) => {
                self.fetch_and_insert_transactions(pool_address, tx.block_time_utc, None)
                    .await
            }
            None => Err(anyhow!("No existing transactions found for update sync")),
        }
    }

    async fn historical_sync(&self, pool_address: &str, start_time: DateTime<Utc>) -> Result<()> {
        let lowest_block_tx = self
            .transaction_repo
            .fetch_lowest_block_time_transaction(pool_address)
            .await?;

        match lowest_block_tx {
            Some(tx) => {
                self.fetch_and_insert_transactions(pool_address, start_time, Some(tx.signature))
                    .await
            }
            None => Err(anyhow!(
                "No existing transactions found for historical sync"
            )),
        }
    }

    async fn fetch_and_insert_transactions(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        mut signature: Option<String>,
    ) -> Result<()> {
        loop {
            let signatures = self
                .fetch_signatures_with_retry(
                    pool_address,
                    SIGNATURE_BATCH_SIZE,
                    signature.as_deref(),
                )
                .await?;

            let filtered_signatures: Vec<SignatureInfo> = signatures
                .into_iter()
                .filter(|sig| sig.err.is_none())
                .collect();

            println!("Processing {} signatures", filtered_signatures.len());

            signature = filtered_signatures.last().map(|sig| sig.signature.clone());

            // Process transactions in smaller batches
            let mut reached_start_time = false;
            for chunk in filtered_signatures.chunks(TX_BATCH_SIZE) {
                let result = self
                    .process_transaction_batch(pool_address, chunk, start_time)
                    .await;

                match result {
                    Ok(_) => {}
                    Err(e) if e.to_string().contains("Reached start_time limit") => {
                        println!("Reached start_time limit. Exiting.");
                        reached_start_time = true;
                        break;
                    }
                    Err(e) => {
                        println!("Error processing batch: {:?}", e);
                    }
                }
            }

            if reached_start_time {
                return Ok(());
            }
        }
    }

    async fn process_transaction_batch(
        &self,
        pool_address: &str,
        signatures: &[SignatureInfo],
        start_time: DateTime<Utc>,
    ) -> Result<()> {
        let signature_strings: Vec<String> =
            signatures.iter().map(|sig| sig.signature.clone()).collect();
        let tx_data_batch = self
            .fetch_transaction_data_batch_with_retry(&signature_strings)
            .await?;

        let futures =
            tx_data_batch
                .into_iter()
                .zip(signatures.iter())
                .map(|(tx_data, sig_info)| {
                    self.process_single_transaction(pool_address, tx_data, sig_info, start_time)
                });

        let results = join_all(futures).await;

        for result in results {
            if let Err(e) = result {
                if e.to_string().contains("Reached start_time limit") {
                    return Err(e);
                }
                println!("Error processing transaction: {:?}", e);
            }
        }

        Ok(())
    }

    async fn process_single_transaction(
        &self,
        pool_address: &str,
        tx_data: serde_json::Value,
        sig_info: &SignatureInfo,
        start_time: DateTime<Utc>,
    ) -> Result<()> {
        let block_tx_time = Utc
            .timestamp_opt(sig_info.block_time, 0)
            .single()
            .ok_or_else(|| anyhow!("Invalid timestamp"))?;

        if block_tx_time <= start_time {
            return Err(anyhow!("Reached start_time limit"));
        }

        let model = TransactionModel::convert_from_json(
            pool_address,
            &tx_data,
            &self.token_a_address,
            &self.token_b_address,
        )
        .map_err(|e| anyhow!("Failed to convert transaction: {:?}", e))?;

        if matches!(
            model.transaction_type.as_str(),
            "Swap" | "IncreaseLiquidity" | "DecreaseLiquidity"
        ) {
            self.transaction_repo
                .insert(&model)
                .await
                .map_err(|e| anyhow!("Failed to insert transaction: {:?}", e))?;

            println!(
                "Successfully processed tx. Current tx: {:?}",
                sig_info.signature
            );
        }

        Ok(())
    }

    async fn fetch_transaction_data_batch_with_retry(
        &self,
        signatures: &[String],
    ) -> Result<Vec<serde_json::Value>> {
        let retry_strategy = ExponentialBackoff::from_millis(BASE_DELAY)
            .max_delay(Duration::from_millis(MAX_DELAY))
            .map(jitter)
            .take(MAX_RETRIES as usize);

        Retry::spawn(retry_strategy, || async {
            match self
                .transaction_api
                .fetch_transaction_data(signatures)
                .await
            {
                Ok(tx_data) => Ok(tx_data),
                Err(ApiError::RateLimit) => {
                    println!("Rate limit hit for transaction data. Retrying...");
                    Err(anyhow!("Rate limit hit"))
                }
                Err(ApiError::Other(e)) => Err(e),
            }
        })
        .await
    }

    async fn fetch_signatures_with_retry(
        &self,
        pool_address: &str,
        batch_size: u32,
        before: Option<&str>,
    ) -> Result<Vec<SignatureInfo>> {
        let retry_strategy = ExponentialBackoff::from_millis(BASE_DELAY)
            .max_delay(Duration::from_millis(MAX_DELAY))
            .map(jitter)
            .take(MAX_RETRIES as usize);

        Retry::spawn(retry_strategy, || async {
            match self
                .transaction_api
                .fetch_transaction_signatures(pool_address, batch_size, before)
                .await
            {
                Ok(signatures) => Ok(signatures),
                Err(ApiError::RateLimit) => {
                    println!("Rate limit hit for signatures. Retrying...");
                    Err(anyhow!("Rate limit hit"))
                }
                Err(ApiError::Other(e)) => Err(e),
            }
        })
        .await
    }
}
