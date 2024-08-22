use crate::api::transactions_api::{SignatureInfo, TransactionApi};
use crate::models::transactions_model::TransactionModel;
use crate::repositories::transactions_repo::TransactionRepo;
use crate::services::transactions_amm_service::{constants, AMMService};
use crate::utils::transaction_utils::retry_with_backoff;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use futures::future::join_all;
use serde_json::Value;

pub struct RaydiumAMM {
    transaction_repo: TransactionRepo,
    transaction_api: TransactionApi,
    token_a_address: String,
    token_b_address: String,
}

impl RaydiumAMM {
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
}

#[async_trait]
impl AMMService for RaydiumAMM {
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
        end_cursor: Option<String>,
    ) -> Result<Value> {
        // Also dont forget to add notes for raydium tx conversions. Its fine to have some duplicate code for the fetching sigs and tx.
        // But the conversion will be completely different!

        todo!("Implement fetch_transactions for OrcaAMM")
    }

    fn convert_data_to_transactions_model(
        &self,
        pool_address: &str,
        tx_data: Value,
    ) -> Vec<TransactionModel> {
        // Implement the conversion logic here
        // This is a placeholder and should be replaced with actual conversion logic
        todo!("Implement fetch_transactions for OrcaAMM")
    }
}
