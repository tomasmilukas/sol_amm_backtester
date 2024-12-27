use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::HashMap;

const JUPITER_API_URL: &str = "https://token.jup.ag/strict";

pub struct TokenMetadataApi {
    client: reqwest::Client,
}

impl TokenMetadataApi {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::new(),
        })
    }

    pub async fn get_token_symbols(&self) -> Result<HashMap<String, String>> {
        let response = self
            .client
            .get(JUPITER_API_URL)
            .send()
            .await?
            .json::<Vec<Value>>()
            .await?;

        self.parse_token_symbols(response)
    }

    fn parse_token_symbols(&self, response: Vec<Value>) -> Result<HashMap<String, String>> {
        let mut symbol_map = HashMap::new();

        for token in response {
            if let (Some(address), Some(symbol)) =
                (token["address"].as_str(), token["symbol"].as_str())
            {
                symbol_map.insert(address.to_string(), symbol.to_string());
            }
        }

        Ok(symbol_map)
    }

    pub async fn get_token_symbols_for_addresses(
        &self,
        addresses: &[String],
    ) -> Result<Vec<String>> {
        let symbols = self.get_token_symbols().await?;

        addresses
            .iter()
            .map(|address| {
                symbols
                    .get(address)
                    .cloned()
                    .ok_or_else(|| anyhow!("Token symbol not found for address: {}", address))
            })
            .collect()
    }
}
