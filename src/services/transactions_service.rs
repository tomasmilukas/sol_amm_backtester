use crate::api::transactions_api::{ApiError, SignatureInfo, TransactionApi};
use crate::config::SyncMode;
use crate::models::transactions_model::TransactionModel;
use crate::repositories::transactions_repo::TransactionRepo;
use anyhow::{anyhow, Result};
use chrono::{DateTime, TimeZone, Utc};
use futures::future::join_all;
use tokio::time::{sleep, Duration};
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

const MAX_RETRIES: u32 = 5;
const BASE_DELAY: u64 = 1000; // 1 second
const MAX_DELAY: u64 = 1_200_000; // 15 minutes
const SIGNATURE_BATCH_SIZE: u32 = 100;

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

            if signatures.is_empty() {
                println!("No more signatures to process");
                return Ok(());
            }

            signature = signatures.last().map(|sig| sig.signature.clone());

            let transaction_futures = signatures
                .into_iter()
                .filter(|sig| sig.err.is_none())
                .map(|sig_info| self.process_transaction(pool_address, sig_info, start_time));

            let results = join_all(transaction_futures).await;

            if results
                .iter()
                .any(|r| matches!(r, Err(e) if e.to_string().contains("Reached start_time limit")))
            {
                return Ok(());
            }

            sleep(Duration::from_secs(13)).await;
        }
    }

    async fn process_transaction(
        &self,
        pool_address: &str,
        sig_info: SignatureInfo,
        start_time: DateTime<Utc>,
    ) -> Result<()> {
        let block_tx_time = Utc
            .timestamp_opt(sig_info.block_time, 0)
            .single()
            .ok_or_else(|| anyhow!("Invalid timestamp"))?;

        if block_tx_time <= start_time {
            return Err(anyhow!("Reached start_time limit"));
        }

        self.process_transaction_data_with_retry(pool_address, &sig_info.signature)
            .await?;
        println!(
            "Successfully processed tx. Current tx: {:?}",
            sig_info.signature
        );
        Ok(())
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
                    println!("Rate limit hit. Retrying...");
                    Err(anyhow!("Rate limit hit"))
                }
                Err(ApiError::Other(e)) => Err(e),
            }
        })
        .await
    }

    async fn process_transaction_data_with_retry(
        &self,
        pool_address: &str,
        signature: &str,
    ) -> Result<()> {
        let retry_strategy = ExponentialBackoff::from_millis(BASE_DELAY)
            .max_delay(Duration::from_millis(MAX_DELAY))
            .map(jitter)
            .take(MAX_RETRIES as usize);

        Retry::spawn(retry_strategy, || async {
            match self.process_transaction_data(pool_address, signature).await {
                Ok(()) => Ok(()),
                Err(ApiError::RateLimit) => {
                    println!("Rate limit hit. Retrying...");
                    Err(anyhow!("Rate limit hit"))
                }
                Err(ApiError::Other(e)) => Err(e),
            }
        })
        .await
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
        .map_err(ApiError::Other)?;

        if matches!(
            model.transaction_type.as_str(),
            "Swap" | "AddLiquidity" | "DecreaseLiquidity"
        ) {
            self.transaction_repo
                .insert(&model)
                .await
                .map_err(ApiError::Other)?;
            Ok(())
        } else {
            Err(ApiError::Other(anyhow!("Not a desired transaction type.")))
        }
    }
}
