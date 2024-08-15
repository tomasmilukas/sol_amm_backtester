use anyhow::{Context, Result};
use std::env;

pub struct AppConfig {
    pub database_url: String,
    pub pool_address: String,
    pub sync_days: i64,
    pub full_sync: bool,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: env::var("DATABASE_URL").context("DATABASE_URL must be set")?,
            pool_address: env::var("POOL_ADDRESS").context("POOL_ADDRESS must be set")?,
            sync_days: env::var("SYNC_DAYS")
                .unwrap_or_else(|_| "30".to_string())
                .parse()?,
            full_sync: env::var("FULL_SYNC")
                .unwrap_or_else(|_| "false".to_string())
                .parse()?,
        })
    }
}
