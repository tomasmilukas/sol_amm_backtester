use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use std::{collections::HashMap, env};

#[derive(Clone)]
pub enum SyncMode {
    Update,
    Historical,
    FullRange,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StrategyType {
    NoRebalance,
    SimpleRebalance,
}

pub struct AppConfig {
    pub database_url: String,
    pub pool_address: String,
    pub strategy: StrategyType,
    pub strategy_details: HashMap<String, serde_json::Value>,
    pub token_a_amount: u128,
    pub token_b_amount: u128,
    pub range: i32,
    pub sync_days: i64,
    pub sync_mode: SyncMode,
    pub pool_address_to_backtest: String,
}

impl SyncMode {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "update" => Ok(SyncMode::Update),
            "historical" => Ok(SyncMode::Historical),
            "full_range" => Ok(SyncMode::FullRange),
            _ => Err(anyhow!("Invalid sync mode: {}", s)),
        }
    }
}

impl StrategyType {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_uppercase().as_str() {
            "NO_REBALANCE" => Ok(StrategyType::NoRebalance),
            "SIMPLE_REBALANCE" => Ok(StrategyType::SimpleRebalance),
            _ => Err(anyhow!("Invalid strategy type: {}", s)),
        }
    }
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        let strategy =
            StrategyType::from_str(&env::var("STRATEGY").context("STRATEGY must be set")?)?;

        let strategy_details_str =
            env::var("STRATEGY_DETAILS").context("STRATEGY_DETAILS must be set")?;

        let strategy_details: HashMap<String, serde_json::Value> =
            serde_json::from_str(&strategy_details_str)
                .context("Failed to parse STRATEGY_DETAILS JSON")?;

        let config = Self {
            database_url: env::var("DATABASE_URL").context("DATABASE_URL must be set")?,
            pool_address: env::var("POOL_ADDRESS").context("POOL_ADDRESS must be set")?,
            strategy,
            token_a_amount: env::var("TOKEN_A_AMOUNT")
                .context("TOKEN_A_AMOUNT must be set")?
                .parse::<u128>()
                .context("Failed to parse TOKEN_A_AMOUNT")?,
            token_b_amount: env::var("TOKEN_B_AMOUNT")
                .context("TOKEN_B_AMOUNT must be set")?
                .parse::<u128>()
                .context("Failed to parse TOKEN_B_AMOUNT")?,
            range: env::var("RANGE")
                .context("RANGE must be set")?
                .parse::<i32>()
                .context("Failed to parse RANGE")?,
            sync_days: env::var("SYNC_DAYS")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .context("Failed to parse SYNC_DAYS")?,
            sync_mode: SyncMode::from_str(
                &env::var("SYNC_MODE").unwrap_or_else(|_| "update".to_string()),
            )?,
            pool_address_to_backtest: env::var("POOL_ADDRESS_TO_BACKTEST")
                .context("POOL_ADDRESS_TO_BACKTEST must be set")?,
            strategy_details,
        };

        config.validate_strategy_details()?;

        Ok(config)
    }

    fn validate_strategy_details(&self) -> Result<()> {
        let required_keys = match self.strategy {
            StrategyType::NoRebalance => vec!["lower_tick", "upper_tick"],
            StrategyType::SimpleRebalance => vec!["range"],
        };

        for key in required_keys {
            if !self.strategy_details.contains_key(key) {
                return Err(anyhow!("Missing required strategy detail: {}", key));
            }
        }

        Ok(())
    }

    pub fn get_strategy_detail<T: DeserializeOwned>(&self, key: &str) -> Result<T> {
        self.strategy_details
            .get(key)
            .ok_or_else(|| anyhow!("Strategy detail '{}' not found", key))
            .and_then(|v| {
                serde_json::from_value(v.clone())
                    .context(format!("Failed to parse '{}' strategy detail", key))
            })
    }
}
