mod api;
mod backtester;
mod config;
mod db;
mod models;
mod repositories;
mod services;
mod utils;

use crate::{
    api::{pool_api::PoolApi, transactions_api::TransactionApi},
    db::initialize_sol_amm_backtester_database,
    repositories::{pool_repo::PoolRepo, transactions_repo::TransactionRepo},
    services::{pool_service::PoolService, transactions_service::TransactionService},
};

use anyhow::Context;
use chrono::{Duration, Utc};
use config::AppConfig;
use dotenv::dotenv;
use sqlx::postgres::PgPoolOptions;

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
    let tx_service = TransactionService::new(
        tx_repo,
        tx_api,
        pool_data.token_a_address,
        pool_data.token_b_address,
    );

    // Sync transactions
    let end_time = Utc::now();
    let start_time = end_time - Duration::days(config.sync_days);
    match tx_service
        .sync_transactions(&config.pool_address, start_time, config.sync_mode)
        .await
    {
        Ok(f) => println!("Synced transactions successfully"),
        Err(e) => eprintln!("Error syncing transactions: {}", e),
    }

    Ok(())
}
