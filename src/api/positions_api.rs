use anyhow::{Context, Result};
use chrono::Utc;
use headless_chrome::{Browser, LaunchOptionsBuilder};
use serde::{Deserialize, Serialize};

const POSITIONS_URL: &str =
    "https://everlastingsong.github.io/account-microscope/#/whirlpool/listPositions/";
const METADATA_URL: &str =
    "https://everlastingsong.github.io/account-microscope/#/whirlpool/whirlpool/";

#[derive(Debug, Serialize, Deserialize)]
pub struct Position {
    pub address: String,
    pub status: String,
    pub liquidity: String,
    pub tick_lower_index: i32,
    pub tick_upper_index: i32,
    pub token_a_amount: f64,
    pub token_b_amount: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub timestamp: String,
    pub pool_address: String,
    pub position_count: usize,
    pub tick_spacing: String,
    pub liquidity: String,
    pub sqrt_price: String,
    pub tick_current_index: String,
    pub fee_growth_global_a: String,
    pub fee_growth_global_b: String,
    pub fee_rate: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PoolData {
    pub metadata: Metadata,
    pub positions: Vec<Position>,
}

pub struct PositionsApi {
    browser: Browser,
}

impl PositionsApi {
    pub fn new() -> Result<Self> {
        let options = LaunchOptionsBuilder::default()
            .headless(true)
            .build()
            .expect("Couldn't build launch options");

        let browser = Browser::new(options)?;

        Ok(Self { browser })
    }

    pub async fn scrape_metadata(&self, pool_address: &str) -> Result<Metadata> {
        let url = format!("{}{}", METADATA_URL, pool_address);
        let tab = self.browser.new_tab()?;
        tab.navigate_to(&url)?;
        tab.wait_for_element("dt")?;

        let metadata = tab
            .evaluate(
                r#"
            () => {
                const getValueByLabel = (label) => {
                    const dt = Array.from(document.querySelectorAll("dt")).find(el => 
                        el.textContent.includes(label)
                    );
                    return dt ? dt.nextElementSibling.textContent.trim() : null;
                };

                return {
                    tickSpacing: getValueByLabel("tickSpacing"),
                    liquidity: getValueByLabel("liquidity"),
                    sqrtPrice: getValueByLabel("sqrtPrice"),
                    tickCurrentIndex: getValueByLabel("tickCurrentIndex"),
                    feeGrowthGlobalA: getValueByLabel("feeGrowthGlobalA"),
                    feeGrowthGlobalB: getValueByLabel("feeGrowthGlobalB"),
                    feeRate: getValueByLabel("feeRate"),
                };
            }
        "#,
            )?
            .value
            .into_serde()?;

        Ok(Metadata {
            timestamp: Utc::now().to_rfc3339(),
            pool_address: pool_address.to_string(),
            position_count: 0, // This will be updated later
            ..metadata
        })
    }

    pub async fn scrape_positions(&self, pool_address: &str) -> Result<Vec<Position>> {
        let url = format!("{}{}", POSITIONS_URL, pool_address);
        let tab = self.browser.new_tab()?;
        tab.navigate_to(&url)?;
        tab.wait_for_element("table")?;

        let positions: Vec<Position> = tab
            .evaluate(
                r#"
            () => {
                const rows = document.querySelectorAll("tbody tr");
                return Array.from(rows).map(row => {
                    const cells = row.querySelectorAll("td");
                    return {
                        address: cells[0].textContent.trim(),
                        status: cells[1].textContent.trim(),
                        liquidity: cells[2].textContent.trim(),
                        tickLowerIndex: parseInt(cells[3].textContent.trim()),
                        tickUpperIndex: parseInt(cells[4].textContent.trim()),
                        tokenAAmount: parseFloat(cells[5].textContent.trim()),
                        tokenBAmount: parseFloat(cells[6].textContent.trim()),
                    };
                }).filter(p => p.liquidity !== "0");
            }
        "#,
            )?
            .value
            .into_serde()?;

        // Clean up addresses
        let cleaned_positions: Vec<Position> = positions
            .into_iter()
            .map(|mut p| {
                p.address = clean_address(&p.address);
                p
            })
            .collect();

        Ok(cleaned_positions)
    }

    pub async fn get_pool_data(&self, pool_address: &str) -> Result<PoolData> {
        let mut metadata = self.scrape_metadata(pool_address).await?;
        let positions = self.scrape_positions(pool_address).await?;

        metadata.position_count = positions.len();

        Ok(PoolData {
            metadata,
            positions,
        })
    }
}

