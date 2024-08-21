use crate::api::transactions_api::{SignatureInfo, TransactionApi};
use crate::models::transactions_model::TransactionModel;
use crate::repositories::transactions_repo::TransactionRepo;
use crate::services::transactions_amm_service::{constants, AMMService};
use crate::utils::transaction_utils::retry_with_backoff;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{from_str, Value};
use std::env;

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
        cursor: Option<Cursor>,
    ) -> Result<Value> {
        // Main logic separation for the JSON URL thing. Make hte optimized fn in the other Orca implementation and the check url health which is a constant at the top.
        // In the other else just use the standard fetch signatures and then batch fetch tx data. then convert the batch with the arrays.

        // Remember! The conversion is different for the optimized txs since their shape is different. so the "set" optimization bool that you will use must be used there too when
        // you are converting the raw data to the transactions model.

        // Dont forget to adjust the liquidity data to support the lower and upper tick since u missed that!
        // Also dont forget to add notes for raydium tx conversions. Its fine to have some duplicate code for the fetching sigs and tx.
        // But the conversion will be completely different!

        todo!("Implement fetch_transactions for OrcaAMM")
    }

    fn convert_data_to_transactions_model(
        &self,
        pool_address: &str,
        tx_data: Value,
    ) -> Vec<TransactionModel> {
        vec![]
    }

    async fn fetch_and_insert_transactions(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        latest_db_transaction: Option<TransactionModel>,
    ) -> Result<()> {
        // REMEMBER THIS MUST BE LOOPED! IT WILL RUN UP UNTIL THE START_TIME IS REACHED. CHECK OG CODE.
        // THIS MUST BE ON EVERY AMM IMPLEMENTATION SINCE THEIR CURSOR UPDATES WORKED DIFFERENTLY ETC.

        // fetch transactions must give back a cursor to pass on next!!!!

        let data = self
            .fetch_transactions(pool_address, start_time, None)
            .await?;

        let transactions = self.convert_data_to_transactions_model(pool_address, data);

        self.insert_transactions(transactions).await
    }
}
