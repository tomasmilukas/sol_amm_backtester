use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine as _};
use chrono::Utc;
use serde_json::Value;

use crate::{models::positions_model::PositionModel, utils::decode::decode_position};

const ORCA_RPC_URL: &str = "https://rpc-proxy-account-microscope-240617.yugure.dev/";

pub struct PositionsApi {
    client: reqwest::Client,
}

impl PositionsApi {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::new(),
        })
    }

    pub async fn get_positions(&self, pool_address: &str) -> Result<Vec<PositionModel>> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "getProgramAccounts",
            "params": [
                "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
                {
                    "encoding": "base64",
                    "filters": [
                        {"dataSize": 216},
                        {"memcmp": {"offset": 8, "bytes": pool_address}}
                    ]
                }
            ]
        });

        let response = self
            .client
            .post(ORCA_RPC_URL)
            .json(&payload)
            .send()
            .await?
            .json::<Value>()
            .await?;

        self.parse_positions(response)
    }

    fn parse_positions(&self, response: Value) -> Result<Vec<PositionModel>> {
        let accounts = response["result"]
            .as_array()
            .ok_or_else(|| anyhow!("Invalid JSON structure"))?;

        let mut positions = Vec::new();

        for account in accounts.iter() {
            let data = account["account"]["data"][0]
                .as_str()
                .ok_or_else(|| anyhow!("Invalid data structure"))?;
            let decoded = general_purpose::STANDARD.decode(data)?;

            match decode_position(&decoded) {
                Ok(position) => {
                    let address = String::from(
                        account["pubkey"]
                            .as_str()
                            .ok_or_else(|| anyhow!("Invalid pubkey"))?,
                    );

                    let position_model = PositionModel::new(
                        address,
                        position.liquidity,
                        position.tick_lower_index,
                        position.tick_upper_index,
                    );

                    positions.push(position_model);
                }
                Err(e) => println!("Error decoding position: {:?}", e),
            }
        }
        Ok(positions)
    }
}
