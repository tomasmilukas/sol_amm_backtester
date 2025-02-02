use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use constants::ORCA_OPTIMIZED_PATH_BASE_URL;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    api::transactions_api::TransactionApi, config::SyncMode,
    models::transactions_model::TransactionModel, repositories::transactions_repo::TransactionRepo,
};

use super::{
    orca_amm_optimized::OrcaOptimizedAMM, orca_amm_standard::OrcaStandardAMM,
    raydium_amm::RaydiumAMM,
};

pub mod constants {
    pub const MAX_RETRIES: u32 = 5;
    pub const BASE_DELAY: u64 = 5000; // 5 seconds
    pub const MAX_DELAY: u64 = 60_000; // 1 minute
    pub const SIGNATURE_BATCH_SIZE: u32 = 1000;
    pub const TX_BATCH_SIZE: usize = 25;
    pub const ORCA_OPTIMIZED_PATH_BASE_URL: &str = "https://whirlpool-replay.pleiades.dev/alpha";
}

// Platforms supported.
#[derive(Debug, Clone, Copy)]
pub enum AMMPlatforms {
    Orca,
    Raydium,
}

#[derive(Debug, Clone)]
pub enum Cursor {
    DateTime(DateTime<Utc>),
    OptionalSignature(Option<String>),
}

#[async_trait]
pub trait AMMService: Send + Sync {
    fn repo(&self) -> &TransactionRepo;
    fn api(&self) -> &TransactionApi;

    async fn fetch_transactions(&self, pool_address: &str, cursor: Cursor) -> Result<Vec<Value>>;

    fn convert_data_to_transactions_model(
        &self,
        pool_address: &str,
        tx_data: Vec<Value>,
    ) -> Result<Vec<TransactionModel>>;

    async fn fetch_and_insert_transactions(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        latest_db_transaction: Option<TransactionModel>,
    ) -> Result<()>;

    async fn insert_transactions(&self, transactions: Vec<TransactionModel>) -> Result<()> {
        match self.repo().insert(&transactions).await {
            Ok(count) => {
                println!("Successfully inserted {} transactions", count);
                Ok(())
            }
            Err(e) => {
                println!("Failed to insert transactions: {:?}", e);
                Err(anyhow!("Failed to insert transactions: {:?}", e))
            }
        }
    }

    async fn sync_transactions(
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
            .repo()
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
            .repo()
            .fetch_lowest_block_time_transaction(pool_address)
            .await?;

        let time_range = Utc::now() - start_time;

        match lowest_block_tx {
            Some(tx) => {
                self.fetch_and_insert_transactions(
                    pool_address,
                    // sync from oldest_tx up until we reach end of time range
                    tx.transform_to_tx_model().block_time_utc - time_range,
                    Some(tx.transform_to_tx_model()),
                )
                .await
            }
            None => Err(anyhow!(
                "No existing transactions found for historical sync"
            )),
        }
    }

    async fn full_range_sync(&self, pool_address: &str, start_time: DateTime<Utc>) -> Result<()> {
        self.fetch_and_insert_transactions(pool_address, start_time, None)
            .await
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create_amm_service(
    platform: AMMPlatforms,
    transaction_repo: TransactionRepo,
    transaction_api: TransactionApi,
    token_a_address: &str,
    token_b_address: &str,
    token_a_vault: &str,
    token_b_vault: &str,
    token_a_decimals: i16,
    token_b_decimals: i16,
) -> Result<Arc<dyn AMMService>> {
    match platform {
        AMMPlatforms::Orca => {
            let feature_flag = std::env::var("FEATURE_FLAG_OPTIMIZATION")
                .unwrap_or_else(|_| "FALSE".to_string())
                .to_uppercase();

            if feature_flag != "TRUE" {
                return Ok(Arc::new(
                    OrcaStandardAMM::new(
                        transaction_repo,
                        transaction_api,
                        String::from(token_a_address),
                        String::from(token_b_address),
                        token_a_decimals,
                        token_b_decimals,
                    )
                    .await,
                ));
            }

            // test a random url to see if we can use the optimized path.
            let client = reqwest::Client::new();
            let url = format!(
                "{}/2024/0821/whirlpool-transaction-20240821.jsonl.gz",
                ORCA_OPTIMIZED_PATH_BASE_URL.trim_end_matches('/')
            );

            let response = client
                .head(&url)
                .timeout(Duration::from_secs(10))
                .send()
                .await;

            if let Ok(resp) = response {
                if resp.status().is_success() {
                    return Ok(Arc::new(
                        OrcaOptimizedAMM::new(
                            transaction_repo,
                            transaction_api,
                            String::from(token_a_address),
                            String::from(token_b_address),
                            String::from(token_a_vault),
                            String::from(token_b_vault),
                        )
                        .await,
                    ));
                }
            }

            // Fallback to standard Orca AMM if URL check fails
            Ok(Arc::new(
                OrcaStandardAMM::new(
                    transaction_repo,
                    transaction_api,
                    String::from(token_a_address),
                    String::from(token_b_address),
                    token_a_decimals,
                    token_b_decimals,
                )
                .await,
            ))
        }
        AMMPlatforms::Raydium => Ok(Arc::new(RaydiumAMM::new(
            transaction_repo,
            transaction_api,
            String::from(token_a_address),
            String::from(token_b_address),
        ))),
    }
}
