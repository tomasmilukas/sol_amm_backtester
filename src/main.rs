mod api;
mod backtester;
mod db;
mod models;
mod repositories;
mod services;
mod utils;

use crate::{
    api::{pool_api::PoolApi, transaction_api::TransactionApi},
    db::initialize_amm_backtester_database,
    repositories::{pool_repo::PoolRepo, transactions_repo::TransactionRepo},
    services::{pool_service::PoolService, transaction_service::TransactionService},
};

use chrono::{Duration, Utc};
use dotenv::dotenv;
use sqlx::postgres::PgPoolOptions;
use std::{env, process};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <pool_address> <sync_days>", args[0]);
        process::exit(1);
    }

    let pool_address = &args[1];
    let sync_days: i64 = args[2].parse().expect("sync_days must be a number");

    if sync_days > 2 {
        eprintln!("sync_days must be under 10")
    }

    dotenv().ok();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    initialize_amm_backtester_database(&pool).await?;

    let pool_repo = PoolRepo::new(pool.clone());
    let pool_api = PoolApi::new()?;
    let pool_service = PoolService::new(pool_repo, pool_api);

    // let pool_address = "FpCMFDFGYotvufJ7HrFHsWEiiQCGbkLCtwHiDnh7o28Q";
    match pool_service.fetch_and_store_pool_data(&pool_address).await {
        Ok(()) => println!("Pool data fetched and stored successfully"),
        Err(e) => eprintln!("Error: {}", e),
    }

    let pool_data = pool_service.get_pool_data(&pool_address).await?;

    let tx_repo = TransactionRepo::new(pool);
    let tx_api = TransactionApi::new()?;
    let tx_service = TransactionService::new(
        tx_repo,
        tx_api,
        pool_data.token_a_address,
        pool_data.token_b_address,
        pool_data.token_a_decimals,
        pool_data.token_b_decimals,
    );

    // Sync transactions
    let end_time = Utc::now();
    let start_time = end_time - Duration::days(sync_days);
    match tx_service
        .sync_transactions(pool_address, start_time, end_time)
        .await
    {
        Ok(count) => println!("Synced {} transactions successfully", count),
        Err(e) => eprintln!("Error syncing transactions: {}", e),
    }

    Ok(())
}
