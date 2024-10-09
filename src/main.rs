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
use api::{
    positions_api::PositionsApi, price_api::PriceApi, token_metadata_api::TokenMetadataApi,
    transactions_api::TransactionApi,
};
use backtester::{
    backtest_utils::{create_full_liquidity_range, sync_backwards},
    backtester::{Backtest, Strategy, Wallet},
    no_rebalance_strategy::NoRebalanceStrategy,
    simple_rebalance_strategy::SimpleRebalanceStrategy,
};

use chrono::{Duration, Utc};
use config::{AppConfig, StrategyType};

use colored::*;
use core::sync;
use dotenv::dotenv;
use repositories::{positions_repo::PositionsRepo, transactions_repo::TransactionRepo};
use services::{
    positions_service::PositionsService, transactions_service::TransactionsService,
    transactions_sync_amm_service::create_amm_service,
};
use sqlx::postgres::PgPoolOptions;
use std::{env, sync::Arc};
use utils::{core_math::U256, profit_calcs::calculate_prices_and_pnl};

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
    let positions_service = PositionsService::new(positions_repo.clone(), positions_api);

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
        tx_api.clone(),
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

    // Update transactions since not all data can be retrieved during sync. Updates will happen using position_data, to fill in liquidity info.
    let transactions_service = TransactionsService::new(tx_repo, tx_api, positions_repo);

    match transactions_service
        .create_closed_positions_from_txs(&config.pool_address)
        .await
    {
        Ok(_) => println!("Created all the closed positions for liquidity info"),
        Err(e) => eprintln!("Error updating txs: {}", e),
    }

    match transactions_service
        .update_and_fill_liquidity_transactions(&config.pool_address)
        .await
    {
        Ok(_) => println!("Updated liquidity transactions successfully"),
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
    let positions_service = PositionsService::new(positions_repo, positions_api);

    let tx_repo = TransactionRepo::new(pool);

    let (positions_data, tx_to_sync_from) = positions_service
        .get_live_position_data_for_transaction(tx_repo.clone(), &config.pool_address)
        .await?;

    // Create the liquidity range "at present" from db.
    let liquidity_range_arr = create_full_liquidity_range(
        pool_data.tick_spacing,
        positions_data,
        pool_data.clone(),
        tx_to_sync_from.clone(),
        pool_data.fee_rate,
    )?;

    println!("Current liquidity range recreated! Time to sync it backwards for the backtester.");

    // Sync it backwards using all transactions to get the original liquidity range that we start our backtest from.
    let (original_starting_liquidity_arr, highest_tx) = sync_backwards(
        &tx_repo,
        liquidity_range_arr,
        pool_data.clone(),
        tx_to_sync_from.clone(),
        10_000,
    )
    .await?;

    println!("Sync backwards complete! Time to add position, sync forwards and calculate results!");

    let mut sync_forward_liq_arr = original_starting_liquidity_arr.clone();

    // since backward sync accrued fees, we need to reset all fee data
    sync_forward_liq_arr.fee_growth_global_a = U256::zero();
    sync_forward_liq_arr.fee_growth_global_b = U256::zero();

    // Reset fee growth outside for all ticks
    for tick_data in sync_forward_liq_arr.data.iter_mut() {
        tick_data.fee_growth_outside_a = U256::zero();
        tick_data.fee_growth_outside_b = U256::zero();
    }

    sync_forward_liq_arr.current_block_time = highest_tx.block_time;

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

    let mut backtest = Backtest::new(
        amount_token_a,
        amount_token_b,
        sync_forward_liq_arr,
        wallet,
        strategy,
    );

    backtest
        .sync_forward(
            &tx_repo,
            highest_tx.tx_id, // the higher, the more in the past it is.
            tx_to_sync_from.tx_id,
            &config.pool_address,
            10_000,
        )
        .await
        .unwrap();

    let token_metadata_api = TokenMetadataApi::new()?;
    let price_api = PriceApi::new()?;

    let result = calculate_prices_and_pnl(
        &token_metadata_api,
        &price_api,
        &backtest,
        &highest_tx,
        &tx_to_sync_from,
    )
    .await
    .unwrap();

    println!("\n{}", "Strategy Results".bold().underline());
    println!("{}", "=================".bold());

    println!("\n{}", "Timespan of strategy".underline());
    println!("  From:        {}", result.start_time);
    println!("  To:          {}", result.end_time);

    println!("\n{}", "Price Changes".underline());
    println!(
        "  Token A price change (vs USD):     {}%",
        format!("{:.3}", result.token_a_price_change_pct).yellow()
    );
    println!(
        "  Token B price change (vs USD):     {}%",
        format!("{:.3}", result.token_b_price_change_pct).yellow()
    );

    println!("\n{}", "Holding Analysis".underline());
    println!(
        "  PnL if held (no LPing):           ${}",
        format!("{:.3}", result.pnl_no_lping).blue()
    );
    println!(
        "  PnL if held pct (no LPing):        {}%",
        format!("{:.3}", result.pnl_no_lping_pct).blue()
    );

    println!("\n{}", "Total Value Analysis".underline());
    println!(
        "  Starting value in USD:            ${:.3}",
        result.starting_total_value_in_usd
    );
    println!(
        "  Ending value in USD:              ${:.3}",
        result.ending_total_value_in_usd
    );
    println!(
        "  Total PnL in USD:                 ${}",
        format!("{:.3}", result.final_value_total).green()
    );
    println!(
        "  Total PnL in pct:                  {}%",
        format!("{:.3}", result.total_pnl_pct).green()
    );

    println!("\n{}", "LPing analysis".underline());
    println!(
        "  Tokens A earned:                   {:.6}",
        result.token_a_collected_fees
    );
    println!(
        "  Tokens B earned:                   {:.6}",
        result.token_b_collected_fees
    );
    println!(
        "  Capital earned (in token A):       {:.6}",
        result.capital_earned_in_token_a
    );
    println!(
        "  Capital earned in pct:             {}%",
        format!("{:.3}", result.capital_earned_in_token_a_in_pct).red()
    );
    println!(
        "  Profits LPing in USD:             ${}",
        format!("{:.3}", result.total_fees_collected_in_usd).red()
    );
    println!(
        "  Profits LPing in pct:              {}%",
        format!("{:.3}", result.lping_profits_pct).red()
    );

    let _ = backtest
        .data_logger
        .export_to_json("simulation_results.json");
    println!("\n Simulation actions and detailed results exported to simulation_results.json");

    Ok(())
}
