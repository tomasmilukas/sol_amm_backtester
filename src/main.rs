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

use anyhow::{Context, Result};
use api::{positions_api::PositionsApi, transactions_api::TransactionApi};
use backtester::{
    backtest_utils::{create_full_liquidity_range, sync_backwards},
    backtester::{Backtest, Strategy, Wallet},
    no_rebalance_strategy::NoRebalanceStrategy,
    simple_rebalance_strategy::SimpleRebalanceStrategy,
};

use chrono::{Duration, Utc};
use config::{AppConfig, StrategyType};
use dotenv::dotenv;
use repositories::{
    positions_repo::PositionsRepo,
    transactions_repo::{TransactionRepo, TransactionRepoTrait},
};
use services::{
    positions_service::PositionsService, transactions_service::TransactionsService,
    transactions_sync_amm_service::create_amm_service,
};
use sqlx::postgres::PgPoolOptions;
use std::{env, sync::Arc};
use utils::{error::SyncError, price_calcs::U256};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let config = AppConfig::from_env()?;
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        println!("Usage: cargo run [sync|backtest]");
        return Ok(());
    }

    match args[1].as_str() {
        "sync" => {
            sync_data(&config, config.sync_days).await?;
        }
        "backtest" => {
            run_backtest(&config).await?;
        }
        _ => {
            println!("Unknown command. Use 'sync' or 'backtest'.");
        }
    }

    Ok(())
}

async fn sync_data(config: &AppConfig, days: i64) -> Result<()> {
    println!("Syncing data for the last {} days", days);

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
        .sync_transactions(&config.pool_address, start_time, config.sync_mode.clone())
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

async fn run_backtest(config: &AppConfig) -> Result<()> {
    println!("Running backtest with strategy: {:?}", &config.strategy);

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await?;

    let pool_repo = PoolRepo::new(pool.clone());
    let pool_api = PoolApi::new()?;
    let pool_service = PoolService::new(pool_repo.clone(), pool_api);

    let pool_data = pool_service
        .get_pool_data(&config.pool_address_to_backtest)
        .await?;

    let positions_repo = PositionsRepo::new(pool.clone());
    let positions_api = PositionsApi::new()?;
    let positions_service = PositionsService::new(positions_repo, pool_repo, positions_api);

    let tx_repo = TransactionRepo::new(pool);

    let latest_transaction = tx_repo
        .fetch_highest_tx_swap(&pool_data.address)
        .await
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

    let (positions_data, tx_to_sync_from) = positions_service
        .get_position_data_for_transaction(
            tx_repo.clone(),
            &config.pool_address,
            latest_transaction.clone().unwrap(),
        )
        .await?;

    // Create the liquidity range "at present" from db.
    let liquidity_range_arr =
        create_full_liquidity_range(pool_data.tick_spacing, positions_data, pool_data.fee_rate)?;

    println!("Current liquidity range recreated! Time to sync it backwards for the backtester.");

    // Sync it backwards using all transactions to get the original liquidity range that we start our backtest from.
    let (original_starting_liquidity_arr, lowest_tx_id) = sync_backwards(
        &tx_repo,
        liquidity_range_arr,
        pool_data.clone(),
        tx_to_sync_from,
        10_000,
    )
    .await?;

    println!("Sync backwards complete! Time to add position, sync forwards and calculate results!");

    let token_a_amount: u128 = config.get_strategy_detail("token_a_amount")?;
    let token_b_amount: u128 = config.get_strategy_detail("token_b_amount")?;

    let amount_token_a =
        U256::from(token_a_amount * 10_u128.pow(pool_data.token_a_decimals as u32));
    let amount_token_b =
        U256::from(token_b_amount * 10_u128.pow(pool_data.token_b_decimals as u32));

    let wallet = Wallet {
        token_a_addr: pool_data.token_a_address,
        token_b_addr: pool_data.token_b_address,
        amount_token_a,
        amount_token_b,
        token_a_decimals: pool_data.token_a_decimals,
        token_b_decimals: pool_data.token_b_decimals,
        amount_a_fees_collected: U256::zero(),
        amount_b_fees_collected: U256::zero(),
        total_profit: 0.0,
        total_profit_pct: 0.0,
    };

    let strategy: Box<dyn Strategy> = match config.strategy {
        StrategyType::NoRebalance => {
            let lower_tick: i32 = config.get_strategy_detail("lower_tick")?;
            let upper_tick: i32 = config.get_strategy_detail("upper_tick")?;
            Box::new(NoRebalanceStrategy::new(lower_tick, upper_tick))
        }
        StrategyType::SimpleRebalance => {
            let range: i32 = config.get_strategy_detail("range")?;
            Box::new(SimpleRebalanceStrategy::new(
                original_starting_liquidity_arr.current_tick,
                range,
            ))
        }
    };

    strategy.initialize_strategy(amount_token_a, amount_token_b);

    let mut backtest = Backtest::new(
        amount_token_a,
        amount_token_b,
        original_starting_liquidity_arr,
        wallet,
        strategy,
    );

    backtest
        .sync_forward(
            &tx_repo,
            lowest_tx_id,
            latest_transaction.unwrap().tx_id,
            &config.pool_address,
            10_000,
        )
        .await
        .unwrap();

    println!(
        "BACTESTING DONE! THIS IS YOUR TOTAL PROFIT {} AND PROFIT PCT {}",
        backtest.wallet.total_profit, backtest.wallet.total_profit
    );

    Ok(())
}
