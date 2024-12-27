use chrono::{DateTime, Utc};
use std::error::Error;

use crate::{
    api::{price_api::PriceApi, token_metadata_api::TokenMetadataApi},
    backtester::backtester_core::Backtest,
    models::transactions_model::TransactionModelFromDB,
    utils::core_math::Q64,
};

pub struct PriceCalculationResult {
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub token_a_price_change_pct: f64,
    pub token_b_price_change_pct: f64,
    pub pnl_no_lping: f64,
    pub pnl_no_lping_pct: f64,
    pub starting_total_value_in_usd: f64,
    pub ending_total_value_in_usd: f64,
    pub final_value_total: f64,
    pub total_pnl_pct: f64,
    pub token_a_collected_fees: f64,
    pub token_b_collected_fees: f64,
    pub capital_earned_in_token_a: f64,
    pub capital_earned_in_token_a_in_pct: f64,
    pub total_fees_collected_in_usd: f64,
    pub total_fees_in_pct: f64,
    pub range_efficiency: f64,
    pub fee_apr_percentage: f64,
    pub impermanent_loss: f64,
}

// Price calculations from start to show growth in strategy in USD.
// We are using Binance public market data for pricing so niche tokens will not be supported
pub async fn calculate_prices_and_pnl(
    token_metadata_api: &TokenMetadataApi,
    price_api: &PriceApi,
    backtest: &Backtest,
    highest_tx: &TransactionModelFromDB,
    tx_to_sync_from: &TransactionModelFromDB,
) -> Result<PriceCalculationResult, Box<dyn Error>> {
    let token_a_addr = &backtest.wallet.token_a_addr;
    let token_b_addr = &backtest.wallet.token_b_addr;
    let token_addr_arr = [token_a_addr.clone(), token_b_addr.clone()];

    println!(
        "{} {}",
        backtest.wallet.amount_token_a.as_u128() as f64
            / 10.0f64.powi(backtest.wallet.token_a_decimals as i32),
        backtest.wallet.amount_token_b.as_u128() as f64
            / 10.0f64.powi(backtest.wallet.token_b_decimals as i32)
    );

    let symbols = token_metadata_api
        .get_token_symbols_for_addresses(&token_addr_arr)
        .await?;
    let token_a_symbol = format!("{}USDT", symbols[0]);
    let token_b_symbol = format!("{}USDT", symbols[1]);

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

    let a_b_end_price = (backtest.liquidity_arr.current_sqrt_price.as_u128() as f64
        / Q64.as_u128() as f64)
        .powf(2.0)
        * 10.0_f64.powf(
            backtest.wallet.token_a_decimals as f64 - backtest.wallet.token_b_decimals as f64,
        );
    let a_b_start_price = starting_amount_token_b / starting_amount_token_a;

    // Real starting value in USD
    let starting_total_value_in_usd = starting_amount_token_a * token_a_starting_price_usd
        + starting_amount_token_b * token_b_starting_price_usd;

    // Ending value in USD if position were held (you can see only prices changed)
    let start_amount_end_value_in_usd = starting_amount_token_a * token_a_ending_price_usd
        + starting_amount_token_b * token_b_ending_price_usd;

    // PnL stats with no LP
    let pnl_no_lping = start_amount_end_value_in_usd - starting_total_value_in_usd;
    let pnl_no_lping_pct = (pnl_no_lping / starting_total_value_in_usd) * 100.0;

    // Fees already included when closing position in backtester.
    let token_a_end_amount = backtest.wallet.amount_token_a.as_u128() as f64
        / 10.0f64.powi(backtest.wallet.token_a_decimals as i32);
    let token_b_end_amount = backtest.wallet.amount_token_b.as_u128() as f64
        / 10.0f64.powi(backtest.wallet.token_b_decimals as i32);

    // Final real ending value using wallet stuff
    let ending_total_value_in_usd = token_a_end_amount * token_a_ending_price_usd
        + token_b_end_amount * token_b_ending_price_usd;

    let final_value_total = ending_total_value_in_usd - starting_total_value_in_usd;
    let total_pnl_pct = (final_value_total / starting_total_value_in_usd) * 100.0;

    let token_a_collected_fees = (backtest.wallet.amount_a_fees_collected.as_u128() as f64)
        / 10.0f64.powi(backtest.wallet.token_a_decimals as i32);

    let token_b_collected_fees = (backtest.wallet.amount_b_fees_collected.as_u128() as f64)
        / 10.0f64.powi(backtest.wallet.token_b_decimals as i32);

    let total_fees_collected_in_usd = (token_a_collected_fees * token_a_ending_price_usd)
        + (token_b_collected_fees * token_b_ending_price_usd);

    let total_fees_in_pct = (total_fees_collected_in_usd / starting_total_value_in_usd) * 100.0;

    let capital_earned_in_token_a = token_a_collected_fees + token_b_collected_fees / a_b_end_price;

    let capital_earned_in_token_a_in_pct = (capital_earned_in_token_a
        / (starting_amount_token_a + starting_amount_token_b / a_b_start_price))
        * 100.0;

    let token_a_price_change_pct = ((token_a_ending_price_usd - token_a_starting_price_usd)
        / token_a_starting_price_usd)
        * 100.0;
    let token_b_price_change_pct = ((token_b_ending_price_usd - token_b_starting_price_usd)
        / token_b_starting_price_usd)
        * 100.0;

    // number of swaps within range
    let range_efficiency =
        (backtest.data.swap_nmr_in_position as f64 / backtest.data.current_swap_nmr as f64) * 100.0;

    let days =
        ((tx_to_sync_from.block_time_utc - highest_tx.block_time_utc).num_days() as f64).max(1.0);
    let fee_apr = (total_fees_in_pct / 100.0 + 1.0).powf(365.0 / days) - 1.0;
    let fee_apr_percentage = fee_apr * 100.0;

    // impermanent loss
    let price_ratio = a_b_end_price / a_b_start_price;
    let impermanent_loss = (2.0 * (price_ratio.sqrt() / (1.0 + price_ratio)) - 1.0) * 100.0;

    // Loss Vs Rebalance
    let growth_factor = ((token_a_ending_price_usd / token_a_starting_price_usd)
        * (token_b_ending_price_usd / token_b_starting_price_usd))
        .sqrt();
    let rebalanced_value = starting_total_value_in_usd * growth_factor;

    Ok(PriceCalculationResult {
        start_time: highest_tx.block_time_utc,
        end_time: tx_to_sync_from.block_time_utc,
        token_a_price_change_pct,
        token_b_price_change_pct,
        pnl_no_lping,
        pnl_no_lping_pct,
        starting_total_value_in_usd,
        ending_total_value_in_usd,
        final_value_total,
        total_pnl_pct,
        token_a_collected_fees,
        token_b_collected_fees,
        capital_earned_in_token_a,
        capital_earned_in_token_a_in_pct,
        total_fees_collected_in_usd,
        total_fees_in_pct,
        range_efficiency,
        fee_apr_percentage,
        impermanent_loss,
    })
}
