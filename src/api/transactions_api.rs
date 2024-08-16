use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;

#[derive(Serialize, Deserialize, Debug)]
pub struct SignatureApiResponse {
    pub jsonrpc: String,
    pub result: Vec<SignatureInfo>,
    pub id: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TransactionApiResponse {
    pub jsonrpc: String,
    pub result: Value,
    pub id: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SignatureInfo {
    pub signature: String,
    pub slot: u64,
    pub block_time: i64,
    pub err: Option<Value>,
}

pub struct TransactionApi {
    client: reqwest::Client,
    alchemy_api_key: String,
    alchemy_api_url: String,
}

impl TransactionApi {
    pub fn new() -> Result<Self> {
        let alchemy_api_key = env::var("ALCHEMY_API_KEY").context("ALCHEMY_API_KEY must be set")?;
        let alchemy_api_url = env::var("ALCHEMY_API_URL").context("ALCHEMY_API_URL must be set")?;

        Ok(Self {
            client: reqwest::Client::new(),
            alchemy_api_key,
            alchemy_api_url,
        })
    }

    pub async fn fetch_transaction_signatures(
        &self,
        pool_address: &str,
        limit: u32,
        before: Option<&str>,
    ) -> Result<Vec<SignatureInfo>> {
        let url = format!("{}/v2/{}", self.alchemy_api_url, self.alchemy_api_key);

        let mut params = serde_json::json!({
            "limit": limit.max(10)
        });

        if let Some(before_sig) = before {
            params["before"] = serde_json::json!(before_sig);
        }

        let response = self
            .client
            .post(&url)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "method": "getSignaturesForAddress",
                "params": [
                    pool_address,
                    params
                ]
            }))
            .send()
            .await?;

        let api_response: SignatureApiResponse = response.json().await?;

        // Filter out failed transactions
        let successful_signatures = api_response
            .result
            .into_iter()
            .filter(|sig| sig.err.is_none())
            .collect();

        Ok(successful_signatures)
    }

    pub async fn fetch_transaction_data(&self, signature: &str) -> Result<Value> {
        let url = format!("{}/v2/{}", self.alchemy_api_url, self.alchemy_api_key);

        println!("New tx being fetched!");

        let response = self
            .client
            .post(&url)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "method": "getTransaction",
                "params": [
                    signature,
                    {"encoding": "jsonParsed", "maxSupportedTransactionVersion": 0}
                ]
            }))
            .send()
            .await?;

        let response_text = response.text().await?;

        let api_response: TransactionApiResponse =
            serde_json::from_str(&response_text).context("Failed to parse API response")?;

        Ok(api_response.result)
    }
}
