use std::time::Duration as stdDuration;

use crate::api::transactions_api::TransactionApi;
use crate::models::transactions_model::TransactionModel;
use crate::repositories::transactions_repo::TransactionRepo;
use crate::services::transactions_amm_service::AMMService;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Utc};
use flate2::bufread::GzDecoder;
use reqwest::Client;
use serde_json::{from_str, Value};
use std::io::{BufRead, BufReader};

use super::transactions_amm_service::constants::ORCA_OPTIMIZED_PATH_BASE_URL;
use super::transactions_amm_service::Cursor;

pub struct OrcaOptimizedAMM {
    transaction_repo: TransactionRepo,
    transaction_api: TransactionApi,
    token_a_address: String,
    token_b_address: String,
    token_a_vault: String,
    token_b_vault: String,
    http_client: Client,
}

impl OrcaOptimizedAMM {
    pub async fn new(
        transaction_repo: TransactionRepo,
        transaction_api: TransactionApi,
        token_a_address: String,
        token_b_address: String,
        token_a_vault: String,
        token_b_vault: String,
    ) -> Self {
        let http_client = Client::builder()
            .timeout(stdDuration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            transaction_repo,
            transaction_api,
            token_a_address,
            token_b_address,
            token_a_vault,
            token_b_vault,
            http_client,
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

        let bytes = response.bytes().await?;
        let gz = GzDecoder::new(&bytes[..]);

        // Create a JSON stream parser
        let mut stream = JsonStream::new(gz);

        let mut relevant_transactions = Vec::new();
        let mut current_transaction = None;

        while let Some(token) = stream.next()? {
            match token {
                Token::ObjectStart => {
                    current_transaction = Some(Value::Object(serde_json::Map::new()));
                }
                Token::ObjectEnd => {
                    if let Some(transaction) = current_transaction.take() {
                        if self.is_relevant_transaction(&transaction, pool_address) {
                            relevant_transactions.push(transaction);
                        }
                    }
                }
                Token::PropertyName(name) => {
                    if let Some(Value::Object(map)) = &mut current_transaction {
                        let value = stream.parse_next_value()?;
                        map.insert(name.to_string(), value);
                    }
                }
                _ => {}
            }
        }

        Ok(relevant_transactions)
    }

    fn is_relevant_transaction(&self, tx: &Value, pool_address: &str) -> bool {
        if let Some(instructions) = tx["instructions"].as_array() {
            for instruction in instructions {
                if let Some(name) = instruction["name"].as_str() {
                    match name {
                        "swap"
                        | "decreaseLiquidity"
                        | "increaseLiquidity"
                        | "twoHopSwap"
                        | "increaseLiquidityV2"
                        | "decreaseLiquidityV2" => {
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

    // Conversion related logic

    fn convert_single_transaction(
        &self,
        pool_address: &str,
        tx: Value,
        estimated_time: DateTime<Utc>,
    ) -> Option<TransactionModel> {
        let signature = tx["signature"].as_str()?.to_string();
        let instructions = tx["instructions"].as_array()?;

        let mut open_position_data = None;
        let mut increase_liquidity_data = None;

        for instruction in instructions {
            if let Some(name) = instruction["name"].as_str() {
                match name {
                    "openPositionWithMetadata" => {
                        open_position_data = Some(instruction["payload"].clone());
                    }
                    "increaseLiquidity" => {
                        increase_liquidity_data = Some(instruction["payload"].clone());
                    }
                    "swap" => {
                        return Some(self.convert_swap(
                            pool_address,
                            &signature,
                            instruction,
                            estimated_time,
                        ));
                    }
                    // Add other instruction types as needed
                    _ => {}
                }
            }
        }

        if let (Some(open_data), Some(increase_data)) =
            (open_position_data, increase_liquidity_data)
        {
            return Some(self.convert_increase_liquidity(
                pool_address,
                &signature,
                &increase_data,
                &open_data,
                estimated_time,
            ));
        }

        None
    }

    fn estimate_block_time(
        &self,
        index: usize,
        total_transactions: usize,
        day_start: DateTime<Utc>,
        total_seconds: i64,
    ) -> DateTime<Utc> {
        let day_fraction = if total_transactions > 1 {
            index as f64 / (total_transactions - 1) as f64
        } else {
            0.0
        };

        let seconds_since_midnight = (day_fraction * total_seconds as f64) as i64;
        day_start + chrono::Duration::seconds(seconds_since_midnight)
    }

    fn convert_swap(
        &self,
        pool_address: &str,
        signature: &str,
        instruction: &Value,
        estimated_time: DateTime<Utc>,
    ) -> TransactionModel {
        let payload = instruction["payload"].as_object().unwrap();
        let data_a_to_b = payload["dataAToB"].as_i64().unwrap_or(0) == 1;

        let (token_in, token_out) = if data_a_to_b {
            (&self.token_a_address, &self.token_b_address)
        } else {
            (&self.token_b_address, &self.token_a_address)
        };

        let amount_in = payload["transferAmount0"]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            .unwrap_or(0.0);
        let amount_out = payload["transferAmount1"]
            .as_str()
            .unwrap_or("0")
            .parse::<f64>()
            .unwrap_or(0.0);

        TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time_utc: Utc::now(), // TODO: Convert block_time to DateTime<Utc>
            transaction_type: "Swap".to_string(),
            ready_for_backtesting: true,
            data: TransactionData::Swap(SwapData {
                token_in: token_in.clone(),
                token_out: token_out.clone(),
                amount_in,
                amount_out,
            }),
        }
    }

    fn convert_increase_liquidity(
        &self,
        pool_address: &str,
        signature: &str,
        increase_payload: &Value,
        open_payload: &Value,
        estimated_time: DateTime<Utc>,
    ) -> TransactionModel {
        let payload = increase_payload["payload"].as_object().unwrap();
        let tick_lower = open_payload["payload"]["dataTickLowerIndex"]
            .as_i64()
            .map(|t| t as u64);
        let tick_upper = open_payload["payload"]["dataTickUpperIndex"]
            .as_i64()
            .map(|t| t as u64);

        let key_position = payload["keyPosition"].as_str().unwrap_or("").to_string();

        let (amount_a, amount_b) = self.match_token_amounts(
            payload["keyTokenVaultA"].as_str().unwrap(),
            payload["keyTokenVaultB"].as_str().unwrap(),
            payload["transferAmount0"].as_str().unwrap_or("0"),
            payload["transferAmount1"].as_str().unwrap_or("0"),
        );

        TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time_utc: estimated_time,
            transaction_type: "IncreaseLiquidity".to_string(),
            ready_for_backtesting: true,
            data: TransactionData::IncreaseLiquidity(LiquidityData {
                token_a: self.token_a_address.clone(),
                token_b: self.token_b_address.clone(),
                amount_a,
                amount_b,
                tick_lower,
                tick_upper,
                key_position: Some(key_position),
            }),
        }
    }

    fn convert_increase_liquidity_v2(
        &self,
        pool_address: &str,
        signature: &str,
        instruction: &Value,
        estimated_time: DateTime<Utc>,
    ) -> TransactionModel {
        let payload = instruction["payload"].as_object().unwrap();

        let (amount_a, amount_b) = self.match_token_amounts(
            payload["keyTokenVaultA"].as_str().unwrap(),
            payload["keyTokenVaultB"].as_str().unwrap(),
            payload["transfer0"]["amount"].as_str().unwrap_or("0"),
            payload["transfer1"]["amount"].as_str().unwrap_or("0"),
        );

        let key_position = payload["keyPosition"].as_str().unwrap_or("").to_string();

        TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time_utc: estimated_time,
            transaction_type: "IncreaseLiquidityV2".to_string(),
            ready_for_backtesting: true,
            data: TransactionData::IncreaseLiquidity(LiquidityData {
                token_a: self.token_a_address.clone(),
                token_b: self.token_b_address.clone(),
                amount_a,
                amount_b,
                tick_lower: None,
                tick_upper: None,
                key_position: Some(key_position),
            }),
        }
    }

    fn convert_decrease_liquidity(
        &self,
        pool_address: &str,
        signature: &str,
        instruction: &Value,
        estimated_time: DateTime<Utc>,
    ) -> TransactionModel {
        let payload = instruction["payload"].as_object().unwrap();
        let key_position = payload["keyPosition"].as_str().unwrap_or("").to_string();

        let (amount_a, amount_b) = self.match_token_amounts(
            payload["keyTokenVaultA"].as_str().unwrap(),
            payload["keyTokenVaultB"].as_str().unwrap(),
            payload["transferAmount0"].as_str().unwrap_or("0"),
            payload["transferAmount1"].as_str().unwrap_or("0"),
        );

        TransactionModel {
            signature: signature.to_string(),
            pool_address: pool_address.to_string(),
            block_time_utc: estimated_time,
            transaction_type: "DecreaseLiquidity".to_string(),
            ready_for_backtesting: true,
            data: TransactionData::DecreaseLiquidity(LiquidityData {
                token_a: self.token_a_address.clone(),
                token_b: self.token_b_address.clone(),
                amount_a,
                amount_b,
                tick_lower: None,
                tick_upper: None,
                key_position: Some(key_position),
            }),
        }
    }

    fn match_token_amounts(
        &self,
        vault_a: &str,
        vault_b: &str,
        amount0: &str,
        amount1: &str,
    ) -> (f64, f64) {
        let amount0 = amount0.parse::<f64>().unwrap_or(0.0);
        let amount1 = amount1.parse::<f64>().unwrap_or(0.0);

        if vault_a == self.token_a_vault {
            (amount0, amount1)
        } else {
            (amount1, amount0)
        }
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
        cursor: Cursor,
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
        // The Value shape is different depending on the conversion path used.

        // let total_transactions = tx_data.len();
        // let day_start = date.date().and_hms(0, 0, 0);
        // let total_seconds = 86400; // Number of seconds in a day

        // tx_data.into_iter().enumerate()
        //     .filter_map(|(index, tx)| {
        //         let estimated_time = self.estimate_block_time(index, total_transactions, day_start, total_seconds);
        //         self.convert_single_transaction(pool_address, tx, estimated_time)
        //     })
        //     .collect()

        todo!("Implement OLD CONVERSION ROUTE FOR OrcaAMM")
    }

    async fn fetch_and_insert_transactions(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        latest_db_transaction: Option<TransactionModel>,
    ) -> Result<()> {
        let mut cursor: Cursor = Cursor::DateTime(Utc::now());

        if Some(&latest_db_transaction).is_some() {
            cursor = Cursor::DateTime(latest_db_transaction.unwrap().block_time_utc)
        }

        // REMEMBER THIS MUST BE LOOPED! IT WILL RUN UP UNTIL THE START_TIME IS REACHED. CHECK OG CODE.
        // THIS MUST BE ON EVERY AMM IMPLEMENTATION SINCE THEIR CURSOR UPDATES WORKED DIFFERENTLY ETC.

        // fetch transactions must give back a cursor to pass on next!!!!

        let data = self
            .fetch_transactions(pool_address, start_time, cursor)
            .await?;

        let transactions = self.convert_data_to_transactions_model(pool_address, data);

        self.insert_transactions(transactions).await
    }
}
