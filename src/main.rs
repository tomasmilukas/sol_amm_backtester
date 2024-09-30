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
use dotenv::dotenv;
use repositories::{positions_repo::PositionsRepo, transactions_repo::TransactionRepo};
use services::{
    positions_service::PositionsService, transactions_service::TransactionsService,
    transactions_sync_amm_service::create_amm_service,
};
use sqlx::postgres::PgPoolOptions;
use std::{env, sync::Arc};
use utils::price_calcs::U256;

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
    // match amm_service
    //     .sync_transactions(&config.pool_address, start_time, config.sync_mode.clone())
    //     .await
    // {
    //     Ok(_f) => println!("Synced transactions successfully"),
    //     Err(e) => eprintln!("Error syncing transactions: {}", e),
    // }

    // Update transactions since not all data can be retrieved during sync. Updates will happen using position_data, to fill in liquidity info.
    let transactions_service = TransactionsService::new(tx_repo, tx_api, positions_repo);

    // match transactions_service
    //     .create_closed_positions_from_txs(&config.pool_address)
    //     .await
    // {
    //     Ok(_) => println!("Created all the closed positions for liquidity info"),
    //     Err(e) => eprintln!("Error updating txs: {}", e),
    // }

    // match transactions_service
    //     .update_and_fill_liquidity_transactions(&config.pool_address)
    //     .await
    // {
    //     Ok(_) => println!("Updated liquidity transactions successfully"),
    //     Err(e) => eprintln!("Error updating txs: {}", e),
    // }

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

    let starting_sqrt_price_pre_sync_forward = original_starting_liquidity_arr.current_sqrt_price;

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
            highest_tx.tx_id, // the higher, the more in the past it is.
            tx_to_sync_from.tx_id,
            &config.pool_address,
            10_000,
        )
        .await
        .unwrap();

    // Price calculations from start to show growth in strategy in USD.
    // We are using Binance public market data for pricing so niche tokens will not be supported.

    let token_metadata_api = TokenMetadataApi::new()?;
    let price_api = PriceApi::new()?;

    let token_a_addr = backtest.wallet.token_a_addr;
    let token_b_addr = backtest.wallet.token_b_addr;

    let token_addr_arr = [token_a_addr.clone(), token_b_addr.clone()];

    let symbols = token_metadata_api
        .get_token_symbols_for_addresses(&token_addr_arr)
        .await?;

    // Symbols for binance
    let token_a_symbol = symbols[0].clone() + "USDT";
    let token_b_symbol = symbols[1].clone() + "USDT";

    let token_a_starting_price_usd = price_api
        .get_historical_price(&token_a_symbol, highest_tx.block_time_utc)
        .await?;
    let token_a_ending_price_usd = price_api
        .get_historical_price(&token_a_symbol, tx_to_sync_from.block_time_utc)
        .await?;

    let token_b_starting_price_usd = price_api
        .get_historical_price(&token_b_symbol, highest_tx.block_time_utc)
        .await?;
    let token_b_ending_price_usd = price_api
        .get_historical_price(&token_b_symbol, tx_to_sync_from.block_time_utc)
        .await?;

    let starting_amount_token_a = (backtest.start_info.token_a_amount.as_u128() as f64)
        / 10.0f64.powi(backtest.wallet.token_a_decimals as i32);

    let starting_amount_token_b = (backtest.start_info.token_b_amount.as_u128() as f64)
        / 10.0f64.powi(backtest.wallet.token_b_decimals as i32);

    let starting_total_value_in_usd = starting_amount_token_a * token_a_starting_price_usd
        + starting_amount_token_b * token_b_starting_price_usd;

    // To calculate PnL if we only held.
    let start_amount_end_value_in_usd = starting_amount_token_a * token_a_ending_price_usd
        + starting_amount_token_b * token_b_ending_price_usd;

    let ending_total_value_in_usd = (backtest.wallet.amount_token_a.as_u128() as f64
        / 10.0f64.powi(backtest.wallet.token_a_decimals as i32))
        * token_a_ending_price_usd
        + (backtest.wallet.amount_token_b.as_u128() as f64
            / 10.0f64.powi(backtest.wallet.token_b_decimals as i32))
            * token_b_ending_price_usd;

    let pnl_no_lping = start_amount_end_value_in_usd - starting_total_value_in_usd;
    let pnl_no_lping_pct = ((start_amount_end_value_in_usd - starting_total_value_in_usd)
        / starting_total_value_in_usd)
        * 100.0;

    let final_value_total = ending_total_value_in_usd - starting_total_value_in_usd;

    // println!(
    //     "Token A price change in USD pct: {:.3}%",
    //     ((token_a_ending_price_usd - token_a_starting_price_usd) / token_a_starting_price_usd)
    //         * 100.0
    // );
    // println!(
    //     "Token B price change in USD pct: {:.3}%",
    //     ((token_b_ending_price_usd - token_b_starting_price_usd) / token_b_starting_price_usd)
    //         * 100.0
    // );

    // println!(
    //     "PnL in USD if held (no LPing): {:.3}",
    //     start_amount_end_value_in_usd - starting_total_value_in_usd
    // );
    // println!(
    //     "PnL in USD if held in pct (no LPing): {:.3}%",
    //     ((start_amount_end_value_in_usd - starting_total_value_in_usd)
    //         / starting_total_value_in_usd)
    //         * 100.0
    // );

    // println!(
    //     "Total value starting in USD: {:.3}",
    //     starting_total_value_in_usd
    // );
    // println!(
    //     "Total value ending in USD: {:.3}",
    //     ending_total_value_in_usd
    // );
    // println!(
    //     "Total value changed in USD pct: {:.3}%",
    //     ((ending_total_value_in_usd - starting_total_value_in_usd) / starting_total_value_in_usd)
    //         * 100.0
    // );

    println!("\n{}", "Strategy Results".bold().underline());
    println!("{}", "=================".bold());

    println!("\n{}", "Timespan of strategy".underline());
    println!("  From:        {}", highest_tx.block_time_utc);
    println!("  To:          {}", tx_to_sync_from.block_time_utc);

    println!("\n{}", "Price Changes".underline());
    // println!(
    //     "  Price change over period: {}%",
    //     format!("{:.2}", price_pct_change).red()
    // );
    println!(
        "  Token A price change (vs USD):     {}%",
        format!(
            "{:.3}",
            ((token_a_ending_price_usd - token_a_starting_price_usd) / token_a_starting_price_usd)
                * 100.0
        )
        .yellow()
    );
    println!(
        "  Token B price change (vs USD):     {}%",
        format!(
            "{:.3}",
            ((token_b_ending_price_usd - token_b_starting_price_usd) / token_b_starting_price_usd)
                * 100.0
        )
        .yellow()
    );

    println!("\n{}", "Holding Analysis".underline());
    println!(
        "  PnL if held (no LPing):           ${}",
        format!("{:.3}", pnl_no_lping).blue()
    );
    println!(
        "  PnL if held pct (no LPing):        {}%",
        format!("{:.3}", pnl_no_lping_pct).blue()
    );

    println!("\n{}", "Total Value Analysis".underline());
    println!(
        "  Starting value in USD:            ${:.3}",
        starting_total_value_in_usd
    );
    println!(
        "  Ending value in USD:              ${:.3}",
        ending_total_value_in_usd
    );
    println!(
        "  Total PnL in USD:                 ${}",
        format!("{:.3}", final_value_total).green()
    );
    println!(
        "  Total PnL in pct:                  {}%",
        format!(
            "{:.3}",
            (final_value_total / starting_total_value_in_usd) * 100.0
        )
        .green()
    );

    println!("\n{}", "LPing analysis".underline());
    println!(
        "  Profits LPing in USD:           ${:.3}",
        final_value_total - pnl_no_lping
    );
    println!(
        "  Profits LPing in pct:            {}%",
        format!(
            "{:.3}",
            ((final_value_total - pnl_no_lping) / starting_total_value_in_usd) * 100.0
        )
        .red()
    );

    // if token_a_addr == SOL_ADDR || token_b_addr == SOL_ADDR {
    //     let sol_is_token_a = token_a_addr == SOL_ADDR;

    //     let starting_price =
    //         (starting_sqrt_price_pre_sync_forward * starting_sqrt_price_pre_sync_forward) / Q128;

    //     let current_price = (backtest.liquidity_arr.current_sqrt_price
    //         * backtest.liquidity_arr.current_sqrt_price)
    //         / Q128;

    //     println!(
    //         "SOL price change in USD pct: {:.3}%",
    //         ((sol_ending_price_usd - sol_starting_price_usd) / sol_starting_price_usd) * 100.0
    //     );

    //     // convert to final value in token a
    //     if sol_is_token_a {
    //         let starting_total_value = (backtest.start_info.token_a_amount
    //             + (backtest.start_info.token_b_amount / starting_price))
    //             .as_u128() as f64
    //             / 10.0f64.powi(backtest.wallet.token_a_decimals as i32);

    //         let starting_usd_total_value = starting_total_value * sol_starting_price_usd;

    //         let ending_total_value = (backtest.wallet.amount_token_a
    //             + (backtest.wallet.amount_token_b / current_price))
    //             .as_u128() as f64
    //             / 10.0f64.powi(backtest.wallet.token_a_decimals as i32);

    //         let ending_usd_total_value = ending_total_value * sol_ending_price_usd;

    //         println!(
    //             "Total profit in USD: {:.3}",
    //             ending_usd_total_value - starting_usd_total_value
    //         );
    //         println!(
    //             "Total profit pct in USD: {:.3}%",
    //             ((ending_usd_total_value - starting_usd_total_value) / starting_usd_total_value)
    //                 * 100.0
    //         );
    //     } else {
    //         // convert to final value in token b
    //         let starting_total_value = (backtest.start_info.token_b_amount
    //             + (backtest.start_info.token_a_amount / starting_price))
    //             .as_u128() as f64
    //             / 10.0f64.powi(backtest.wallet.token_a_decimals as i32);

    //         let starting_usd_total_value = starting_total_value * sol_starting_price_usd;

    //         let ending_total_value = (backtest.wallet.amount_token_b
    //             + (backtest.wallet.amount_token_a / current_price))
    //             .as_u128() as f64
    //             / 10.0f64.powi(backtest.wallet.token_a_decimals as i32);

    //         let ending_usd_total_value = ending_total_value * sol_ending_price_usd;

    //         println!(
    //             "Total profit in USD: {:.3}",
    //             ending_usd_total_value - starting_usd_total_value
    //         );
    //         println!(
    //             "Total profit pct in USD: {:.3}%",
    //             ((ending_usd_total_value - starting_usd_total_value) / starting_usd_total_value)
    //                 * 100.0
    //         );
    //     }
    // } else {
    //     println!("No mapping for SOL addresses => symbols, must calculate manually. Only SOL -> USDC conversion supported for now.")
    // };

    Ok(())
}
