use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::kline_model::KlineModel;

const BINANCE_API_URL: &str = "https://fapi.binance.com/fapi/v1/klines";

pub struct PriceApi {
    client: reqwest::Client,
}

impl PriceApi {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::new(),
        })
    }

    pub async fn get_kline_data(
        &self,
        symbol: &str,
        interval: &str,
        start_time: Option<DateTime<Utc>>,
        end_time: Option<DateTime<Utc>>,
        limit: Option<u32>,
    ) -> Result<Vec<KlineModel>> {
        let mut params = vec![
            ("symbol", symbol.to_string()),
            ("interval", interval.to_string()),
        ];

        if let Some(start) = start_time {
            params.push(("startTime", start.timestamp_millis().to_string()));
        }

        if let Some(end) = end_time {
            params.push(("endTime", end.timestamp_millis().to_string()));
        }

        if let Some(lim) = limit {
            params.push(("limit", lim.to_string()));
        }

        let response = self
            .client
            .get(BINANCE_API_URL)
            .query(&params)
            .send()
            .await?
            .json::<Value>()
            .await?;

        self.parse_klines(response)
    }

    fn parse_klines(&self, response: Value) -> Result<Vec<KlineModel>> {
        let klines = response
            .as_array()
            .ok_or_else(|| anyhow!("Invalid JSON structure"))?;

        let mut kline_models = Vec::new();

        for kline in klines.iter() {
            let kline_data = kline
                .as_array()
                .ok_or_else(|| anyhow!("Invalid kline data"))?;

            if kline_data.len() < 12 {
                return Err(anyhow!("Insufficient kline data"));
            }

            let kline_model = KlineModel::new(
                kline_data[0]
                    .as_i64()
                    .ok_or_else(|| anyhow!("Invalid open time"))?,
                kline_data[1]
                    .as_str()
                    .ok_or_else(|| anyhow!("Invalid open"))?
                    .parse()?,
                kline_data[2]
                    .as_str()
                    .ok_or_else(|| anyhow!("Invalid high"))?
                    .parse()?,
                kline_data[3]
                    .as_str()
                    .ok_or_else(|| anyhow!("Invalid low"))?
                    .parse()?,
                kline_data[4]
                    .as_str()
                    .ok_or_else(|| anyhow!("Invalid close"))?
                    .parse()?,
                kline_data[5]
                    .as_str()
                    .ok_or_else(|| anyhow!("Invalid volume"))?
                    .parse()?,
                kline_data[6]
                    .as_i64()
                    .ok_or_else(|| anyhow!("Invalid close time"))?,
                kline_data[7]
                    .as_str()
                    .ok_or_else(|| anyhow!("Invalid quote asset volume"))?
                    .parse()?,
                kline_data[8]
                    .as_u64()
                    .ok_or_else(|| anyhow!("Invalid number of trades"))?,
                kline_data[9]
                    .as_str()
                    .ok_or_else(|| anyhow!("Invalid taker buy base asset volume"))?
                    .parse()?,
                kline_data[10]
                    .as_str()
                    .ok_or_else(|| anyhow!("Invalid taker buy quote asset volume"))?
                    .parse()?,
            );

            kline_models.push(kline_model);
        }

        Ok(kline_models)
    }

    pub async fn get_historical_price(
        &self,
        symbol: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<f64> {
        let klines = self
            .get_kline_data(
                symbol,
                "1m",
                Some(timestamp - chrono::Duration::minutes(1)),
                Some(timestamp),
                Some(1),
            )
            .await?;

        if klines.is_empty() {
            return Err(anyhow!("No kline data found for the given timestamp"));
        }

        Ok(klines[0].close)
    }
}
