use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Duration as stdDuration;

use crate::api::transactions_api::{SignatureInfo, TransactionApi};
use crate::models::transactions_model::TransactionModel;
use crate::repositories::transactions_repo::TransactionRepo;
use crate::services::transactions_amm_service::{constants, AMMService};
use crate::utils::transaction_utils::retry_with_backoff;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, Utc};
use futures::StreamExt;
use reqwest::Client;
use serde_json::{from_str, Value};
use std::env;

use super::transactions_amm_service::constants::ORCA_OPTIMIZED_PATH_BASE_URL;
use super::transactions_amm_service::Cursor;

pub struct OrcaOptimizedAMM {
    transaction_repo: TransactionRepo,
    transaction_api: TransactionApi,
    token_a_address: String,
    token_b_address: String,
    http_client: Client,
}

impl OrcaOptimizedAMM {
    pub async fn new(
        transaction_repo: TransactionRepo,
        transaction_api: TransactionApi,
        token_a_address: String,
        token_b_address: String,
    ) -> Self {
        let feature_flag = env::var("FEATURE_FLAG_OPTIMIZATION")
            .unwrap_or_else(|_| "FALSE".to_string())
            .to_uppercase();

        let use_optimized_path = if feature_flag == "TRUE" {
            Self::check_url_health(ORCA_OPTIMIZED_PATH_BASE_URL).await
        } else {
            false
        };

        let http_client = Client::builder()
            .timeout(stdDuration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            transaction_repo,
            transaction_api,
            token_a_address,
            token_b_address,
            http_client,
        }
    }

    async fn check_url_health(url: &str) -> bool {
        let client = Client::new();

        match client
            .head(url)
            .timeout(stdDuration::from_secs(10))
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    async fn fetch_txs_optimized_path(
        &self,
        pool_address: &str,
        date: DateTime<Utc>,
    ) -> Result<Vec<Value>> {
        let url = self.construct_url(&date)?;

        // Download file
        let response = self.http_client.get(&url).send().await?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to download file: HTTP {}",
                response.status()
            ));
        }

        let content = response.bytes().await?;
        let text = String::from_utf8_lossy(&content);

        let mut relevant_transactions = Vec::new();

        for line in text.lines() {
            if let Ok(json) = from_str::<Value>(line) {
                if let Some(transactions) = json["transactions"].as_array() {
                    for tx in transactions {
                        if self.is_relevant_transaction_optimized_path(tx, pool_address) {
                            relevant_transactions.push(tx.clone());
                        }
                    }
                }
            }
        }

        Ok(relevant_transactions)
    }

    fn is_relevant_transaction_optimized_path(&self, tx: &Value, pool_address: &str) -> bool {
        if let Some(instructions) = tx["instructions"].as_array() {
            for instruction in instructions {
                if let Some(name) = instruction["name"].as_str() {
                    match name {
                        "swap" | "decreaseLiquidity" | "increaseLiquidity" => {
                            if let Some(payload) = instruction["payload"].as_object() {
                                if payload.get("keyWhirlpool")
                                    == Some(&Value::String(pool_address.to_string()))
                                {
                                    return true;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        false
    }

    fn construct_url(&self, date: &DateTime<Utc>) -> Result<String> {
        Ok(format!(
            "{}/{}/{:02}{:02}/whirlpool-transaction-{}{:02}{:02}.jsonl.gz",
            ORCA_OPTIMIZED_PATH_BASE_URL,
            date.year(),
            date.month(),
            date.day(),
            date.year(),
            date.month(),
            date.day()
        ))
    }
}

#[async_trait]
impl AMMService for OrcaOptimizedAMM {
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
        if self.use_optimized_path {
            // flatten curent_time and make sure the logic is updated well.
            todo!("Implement NEW ROUTE FOR OrcaAMM")
        } else {
            todo!("Implement OLD ROUTE FOR OrcaAMM")
        }
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
        // The Value shape is different depending on the conversion path used.
        if self.use_optimized_path {
            todo!("Implement NEW CONVERSION ROUTE FOR OrcaAMM")
        } else {
            todo!("Implement OLD CONVERSION ROUTE FOR OrcaAMM")
        }
    }

    async fn fetch_and_insert_transactions(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        latest_db_transaction: Option<TransactionModel>,
    ) -> Result<()> {
        // REMEMBER THIS MUST BE LOOPED! IT WILL RUN UP UNTIL THE START_TIME IS REACHED. CHECK OG CODE.
        // THIS MUST BE ON EVERY AMM IMPLEMENTATION SINCE THEIR CURSOR UPDATES WORKED DIFFERENTLY ETC.

        let mut cursor: Option<Cursor> = None;

        // Logic to provide initial cursor.
        if Some(&latest_db_transaction).is_some() {
            if self.use_optimized_path {
                cursor = Some(Cursor::DateTime(
                    latest_db_transaction.unwrap().block_time_utc,
                ));
            } else {
                cursor = Some(Cursor::String(latest_db_transaction.unwrap().signature));
            }
        }

        // fetch transactions must give back a cursor to pass on next!!!!

        let data = self
            .fetch_transactions(pool_address, start_time, cursor)
            .await?;

        let transactions = self.convert_data_to_transactions_model(pool_address, data);

        self.insert_transactions(transactions).await
    }
}
