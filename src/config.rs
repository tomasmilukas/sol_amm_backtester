use anyhow::{anyhow, Context, Result};
use std::env;

pub enum SyncMode {
    Update,
    Historical,
    FullRange,
}

pub struct AppConfig {
    pub database_url: String,
    pub pool_address: String,
    pub sync_days: i64,
    pub sync_mode: SyncMode,
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

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: env::var("DATABASE_URL").context("DATABASE_URL must be set")?,
            pool_address: env::var("POOL_ADDRESS").context("POOL_ADDRESS must be set")?,
            sync_days: env::var("SYNC_DAYS")
                .unwrap_or_else(|_| "30".to_string())
                .parse()
                .context("Failed to parse SYNC_DAYS")?,
            sync_mode: SyncMode::from_str(&env::var("SYNC_MODE")
                .unwrap_or_else(|_| "update".to_string()))?
        })
    }
}
