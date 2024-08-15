use std::env;
use anyhow::{Context, Result};

pub struct AppConfig {
    pub database_url: String,
    pub alchemy_api_key: String,
    pub alchemy_api_url: String,
    pub pool_address: String,
    pub token_a_address: String,
    pub token_b_address: String,
    pub sync_days: i64,
    pub full_sync: bool,
    pub batch_size: u32,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: env::var("DATABASE_URL").context("DATABASE_URL must be set")?,
            alchemy_api_key: env::var("ALCHEMY_API_KEY").context("ALCHEMY_API_KEY must be set")?,
            alchemy_api_url: env::var("ALCHEMY_API_URL").context("ALCHEMY_API_URL must be set")?,
            pool_address: env::var("POOL_ADDRESS").context("POOL_ADDRESS must be set")?,
            token_a_address: env::var("TOKEN_A_ADDRESS").context("TOKEN_A_ADDRESS must be set")?,
            token_b_address: env::var("TOKEN_B_ADDRESS").context("TOKEN_B_ADDRESS must be set")?,
            sync_days: env::var("SYNC_DAYS").unwrap_or_else(|_| "30".to_string()).parse()?,
            full_sync: env::var("FULL_SYNC").unwrap_or_else(|_| "false".to_string()).parse()?,
            batch_size: env::var("BATCH_SIZE").unwrap_or_else(|_| "1000".to_string()).parse()?,
        })
    }
}
