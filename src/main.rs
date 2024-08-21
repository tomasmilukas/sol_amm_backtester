mod api;
mod backtester;
mod config;
mod db;
mod models;
mod repositories;
mod services;
mod utils;

use crate::{
    api::pool_api::PoolApi,
    db::initialize_sol_amm_backtester_database,
    repositories::pool_repo::PoolRepo,
    services::{
        pool_service::PoolService,
        transactions_amm_service::{AMMPlatforms, AMMService},
    },
};

use anyhow::Context;
use api::transactions_api::TransactionApi;
use chrono::{Duration, Utc};
use config::AppConfig;
use dotenv::dotenv;
use repositories::transactions_repo::TransactionRepo;
use services::transactions_amm_service::create_amm_service;
use sqlx::postgres::PgPoolOptions;
use std::{env, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let config = AppConfig::from_env()?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await?;

    initialize_sol_amm_backtester_database(&pool)
        .await
        .context("Failed to initialize database")?;

    let pool_repo = PoolRepo::new(pool.clone());
    let pool_api = PoolApi::new()?;
    let pool_service = PoolService::new(pool_repo, pool_api);

    match pool_service
        .fetch_and_store_pool_data(&config.pool_address)
        .await
    {
        Ok(()) => println!("Pool data fetched and stored successfully"),
        Err(e) => eprintln!("Pool fetching related error: {}", e),
    }

    let pool_data = pool_service.get_pool_data(&config.pool_address).await?;

    let tx_repo = TransactionRepo::new(pool);
    let tx_api = TransactionApi::new()?;

    let platform = env::var("POOL_PLATFORM")
        .context("POOL_PLATFORM environment variable not set")?
        .parse::<AMMPlatforms>()?;

    let amm_service: Arc<dyn AMMService> = match create_amm_service(
        platform,
        tx_repo,
        tx_api,
        &pool_data.token_a_address,
        &pool_data.token_b_address,
        &pool_data.token_a_vault,
        &pool_data.token_b_vault,
    )
    .await
    {
        Ok(service) => service,
        Err(e) => {
            panic!("Critical error: Failed to create AMM service: {}", e);
        }
    };

    // Sync transactions
    let end_time = Utc::now();
    let start_time = end_time - Duration::days(config.sync_days);
    match amm_service
        .sync_transactions(&config.pool_address, start_time, config.sync_mode)
        .await
    {
        Ok(_f) => println!("Synced transactions successfully"),
        Err(e) => eprintln!("Error syncing transactions: {}", e),
    }

    Ok(())
}
