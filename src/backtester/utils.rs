use anyhow::Result;

use crate::{
    models::{pool_model::PoolModel, positions_model::PositionModel},
    repositories::transactions_repo::{OrderDirection, TransactionRepo},
    utils::{error::SyncError, price_calcs::sqrt_price_to_fixed},
};

use super::liquidity_array::{LiquidityArray, TickData};

pub fn create_full_liquidity_range(
    tick_spacing: i16,
    positions: Vec<PositionModel>,
    fee_rate: i16,
) -> Result<LiquidityArray> {
    let min_tick = -500_000;
    let max_tick = 500_000;

    let mut liquidity_array =
        LiquidityArray::new(min_tick, max_tick, tick_spacing as i32, fee_rate);

    for position in positions {
        let lower_tick: i32 = position.tick_lower;
        let upper_tick: i32 = position.tick_upper;
        let liquidity: u128 = position.liquidity;

        let tick_data = TickData {
            lower_tick,
            upper_tick,
            liquidity,
        };

        liquidity_array.update_liquidity(tick_data);
    }

    Ok(liquidity_array)
}

pub async fn sync_backwards(
    transaction_repo: &TransactionRepo,
    mut liquidity_array: LiquidityArray,
    pool_model: PoolModel,
    batch_size: i64,
) -> Result<LiquidityArray, SyncError> {
    let latest_transaction = transaction_repo
        .fetch_highest_tx_swap(&pool_model.address)
        .await
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

    // Initialize the cursor with the latest tx_id
    let mut cursor = latest_transaction.clone().map(|tx| tx.tx_id);

    let swap_data = latest_transaction
        .unwrap()
        .data
        .to_swap_data()
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?
        .clone();

    let is_sell = swap_data.token_in == pool_model.token_a_address;

    // DOUBLE CHECK IF RIGHT.
    // PLS
    liquidity_array.current_sqrt_price = if is_sell {
        sqrt_price_to_fixed((swap_data.amount_out / swap_data.amount_in).sqrt())
    } else {
        sqrt_price_to_fixed((swap_data.amount_in / swap_data.amount_out).sqrt())
    };

    // Implement logic to fetch the most accurate price at that timestamp.
    // Then caculate the equiv current tick and sqrtPrice and add it to self.

    loop {
        let transactions = transaction_repo
            .fetch_transactions(
                &pool_model.address,
                cursor,
                batch_size,
                OrderDirection::Descending,
            )
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        if transactions.is_empty() {
            break;
        }

        for transaction in transactions.iter().rev() {
            match transaction.transaction_type.as_str() {
                "IncreaseLiquidity" | "DecreaseLiquidity" => {
                    let liquidity_data = transaction
                        .data
                        .to_liquidity_data()
                        .map_err(|e| SyncError::ParseError(e.to_string()))?;

                    let tick_data = TickData {
                        lower_tick: liquidity_data.tick_lower.unwrap(),
                        upper_tick: liquidity_data.tick_upper.unwrap(),
                        liquidity: liquidity_data.liquidity_amount.parse::<u128>().unwrap(),
                    };

                    let is_increase = transaction.transaction_type.as_str() == "IncreaseLiquidity";

                    liquidity_array.update_liquidity_from_tx(tick_data, is_increase);
                }
                "Swap" => {
                    let swap_data = transaction
                        .data
                        .to_swap_data()
                        .map_err(|e| SyncError::ParseError(e.to_string()))?;

                    let is_sell = swap_data.token_in == pool_model.token_a_address;

                    // fee rates are in bps
                    let fee_rate_pct = pool_model.fee_rate / 10000;

                    let scaling_factor = if is_sell {
                        10f64.powi(pool_model.token_a_decimals as i32)
                    } else {
                        10f64.powi(pool_model.token_b_decimals as i32)
                    };

                    liquidity_array
                        .simulate_swap((swap_data.amount_in * scaling_factor) as u128, is_sell)?;
                }
                _ => {}
            }
        }

        cursor = transactions.last().map(|t| t.tx_id);

        if transactions.len() < batch_size as usize {
            break;
        }
    }

    Ok(liquidity_array)
}
