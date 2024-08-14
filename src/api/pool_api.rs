use anyhow::{Context, Result};
use dotenv::dotenv;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;

use crate::models::token_metadata::TokenMetadata;

#[derive(Serialize, Deserialize, Debug)]
pub struct ApiResponse {
    pub jsonrpc: String,
    pub result: Value,
    pub id: u64,
}

pub struct PoolApi {
    client: reqwest::Client,
    coingecko_api_key: String,
    coingecko_api_url: String,
    coingecko_header: String,
    alchemy_api_key: String,
    alchemy_api_url: String,
}

impl PoolApi {
    pub fn new() -> Result<Self> {
        dotenv().ok();
        let coingecko_api_key =
            env::var("COINGECKO_API_KEY").context("COINGECKO_API_KEY must be set")?;
        let coingecko_api_url =
            env::var("COINGECKO_API_URL").context("COINGECKO_API_URL must be set")?;
        let coingecko_header =
            env::var("COINGECKO_HEADER").context("COINGECKO_HEADER must be set")?;
        let alchemy_api_key = env::var("ALCHEMY_API_KEY").context("ALCHEMY_API_KEY must be set")?;
        let alchemy_api_url = env::var("ALCHEMY_API_URL").context("ALCHEMY_API_URL must be set")?;

        Ok(Self {
            client: reqwest::Client::new(),
            coingecko_api_key,
            coingecko_api_url,
            coingecko_header,
            alchemy_api_key,
            alchemy_api_url,
        })
    }

    pub async fn fetch_pool_data(&self, pool_address: &str) -> Result<Value> {
        let url = format!("{}/v2/{}", self.alchemy_api_url, self.alchemy_api_key);

        let response = self
            .client
            .post(&url)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "id": 1,
                "jsonrpc": "2.0",
                "method": "getAccountInfo",
                "params": [
                    pool_address,
                    {
                        "encoding": "jsonParsed"
                    }
                ]
            }))
            .send()
            .await?;

        let api_response: ApiResponse = response.json().await?;
        Ok(api_response.result)
    }

    pub async fn fetch_token_metadata(&self, token_address: &str) -> Result<TokenMetadata> {
        let url = format!(
            "{}coins/solana/contract/{}",
            self.coingecko_api_url, token_address
        );

        let response = self
            .client
            .get(&url)
            .header(&self.coingecko_header, &self.coingecko_api_key)
            .send()
            .await
            .context("Failed to send request to CoinGecko")?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "CoinGecko API error: {}",
                response.status()
            ));
        }

        let json_response: Value = response.json().await?;

        let token_info = TokenMetadata {
            symbol: json_response["symbol"].to_string(),
            decimals: json_response["detail_platforms"]["solana"]["decimal_place"]
                .as_i64()
                .expect("Failed to get decimal_place as i64"),
        };

        Ok(token_info)
    }
}
