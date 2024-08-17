use anyhow::{Context, Result};
use reqwest::StatusCode;
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
#[serde(rename_all = "camelCase")]
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

#[derive(Debug)]
pub enum ApiError {
    RateLimit,
    Other(anyhow::Error),
}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        match error.status() {
            Some(StatusCode::TOO_MANY_REQUESTS) => ApiError::RateLimit,
            _ => ApiError::Other(error.into()),
        }
    }
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
    ) -> Result<Vec<SignatureInfo>, ApiError> {
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

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(ApiError::RateLimit);
        }

        let api_response: SignatureApiResponse = response.json().await?;
        Ok(api_response.result)
    }

    pub async fn fetch_transaction_data(
        &self,
        signatures: &[String],
    ) -> Result<Vec<Value>, ApiError> {
        let url = format!("{}/v2/{}", self.alchemy_api_url, self.alchemy_api_key);

        let batch_requests: Vec<Value> = signatures
            .iter()
            .enumerate()
            .map(|(id, signature)| {
                serde_json::json!({
                    "id": id + 1,
                    "jsonrpc": "2.0",
                    "method": "getTransaction",
                    "params": [
                        signature,
                        {"encoding": "json", "maxSupportedTransactionVersion": 0}
                    ]
                })
            })
            .collect();

        println!("batch requests: {:?}", batch_requests.len());

        let response = self
            .client
            .post(&url)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .json(&batch_requests)
            .send()
            .await?;

        println!("API RESPONSE: {:?}", response);

        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(ApiError::RateLimit);
        }

        let response_text = response.text().await?;
        let api_responses: Vec<TransactionApiResponse> = serde_json::from_str(&response_text)
            .context("Failed to parse API response")
            .map_err(|e| ApiError::Other(e))?;

        Ok(api_responses.into_iter().map(|r| r.result).collect())
    }
}
