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
        transactions_sync_amm_service::{AMMPlatforms, AMMService},
    },
};

use anyhow::Context;
use api::{positions_api::PositionsApi, transactions_api::TransactionApi};
use chrono::{Duration, Utc};
use config::AppConfig;
use dotenv::dotenv;
use repositories::{positions_repo::PositionsRepo, transactions_repo::TransactionRepo};
use services::{
    positions_service::PositionsService, transactions_service::TransactionsService,
    transactions_sync_amm_service::create_amm_service,
};
use sqlx::postgres::PgPoolOptions;
use std::{env, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let config = AppConfig::from_env()?;

    let platform = env::var("POOL_PLATFORM")
        .context("POOL_PLATFORM environment variable not set")?
        .parse::<AMMPlatforms>()?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await?;

    initialize_sol_amm_backtester_database(&pool)
        .await
        .context("Failed to initialize database")?;

    let pool_repo = PoolRepo::new(pool.clone());
    let pool_api = PoolApi::new()?;
    let pool_service = PoolService::new(pool_repo.clone(), pool_api);

    match pool_service
        .fetch_and_store_pool_data(&config.pool_address, platform.clone())
        .await
    {
        Ok(()) => println!("Pool data fetched and stored successfully"),
        Err(e) => eprintln!("Pool fetching related error: {}", e),
    }

    let pool_data = pool_service.get_pool_data(&config.pool_address).await?;

    let positions_repo = PositionsRepo::new(pool.clone());
    let positions_api = PositionsApi::new()?;
    let positions_service = PositionsService::new(positions_repo.clone(), pool_repo, positions_api);

    match positions_service
        .fetch_and_store_positions_data(&config.pool_address)
        .await
    {
        Ok(()) => println!("Positions data fetched and stored successfully"),
        Err(e) => eprintln!("Positions fetching related error: {}", e),
    }

    let tx_repo = TransactionRepo::new(pool);
    let tx_api = TransactionApi::new()?;

    let amm_service: Arc<dyn AMMService> = match create_amm_service(
        platform,
        tx_repo.clone(),
        tx_api,
        &pool_data.token_a_address,
        &pool_data.token_b_address,
        &pool_data.token_a_vault,
        &pool_data.token_b_vault,
        pool_data.token_a_decimals,
        pool_data.token_b_decimals,
    )
    .await
    {
        Ok(service) => service,
        Err(e) => {
            panic!("Critical error: Failed to create AMM service: {}", e);
        }
    };

    println!("Transaction sync kick off!");

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

    println!("Transactions synced, time to fill in missing data!");

    // Update transactions since not all data can be retrieved during sync. Updates will happen using position_data, to fill in liquidity info.
    let transactions_service = TransactionsService::new(tx_repo, positions_repo);

    match transactions_service
        .update_and_fill_transactions(&config.pool_address)
        .await
    {
        Ok(_f) => println!("Updated txs successfully"),
        Err(e) => eprintln!("Error updating txs: {}", e),
    }

    Ok(())
}
