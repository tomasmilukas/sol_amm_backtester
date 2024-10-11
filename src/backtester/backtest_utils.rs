use anyhow::Result;

use crate::{
    models::{
        pool_model::PoolModel, positions_model::LivePositionModel,
        transactions_model::TransactionModelFromDB,
    },
    repositories::transactions_repo::{OrderDirection, TransactionRepoTrait},
    utils::{
        core_math::{
            calculate_liquidity_a, calculate_liquidity_b, calculate_token_a_from_liquidity,
            calculate_token_b_from_liquidity, price_to_tick, tick_to_sqrt_price_u256, U256,
        },
        error::SyncError,
    },
};

use super::liquidity_array::LiquidityArray;

pub fn create_full_liquidity_range(
    tick_spacing: i16,
    positions: Vec<LivePositionModel>,
    pool_model: PoolModel,
    latest_transaction: TransactionModelFromDB,
    fee_rate: i16,
) -> Result<LiquidityArray> {
    let min_tick = -500_000;
    let max_tick = 500_000;

    let mut liquidity_array =
        LiquidityArray::new(min_tick, max_tick, tick_spacing as i32, fee_rate);

    // set price to correctly calculate active liquidity inside update_liquidity
    let swap_data = latest_transaction
        .data
        .to_swap_data()
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?
        .clone();

    let is_sell = swap_data.token_in == pool_model.token_a_address;

    // Set essential info before simulation.
    if is_sell {
        let tick = price_to_tick(swap_data.amount_out as f64 / swap_data.amount_in as f64);

        liquidity_array.current_tick = tick;
        liquidity_array.current_sqrt_price = tick_to_sqrt_price_u256(tick);
    } else {
        let tick = price_to_tick(swap_data.amount_in as f64 / swap_data.amount_out as f64);

        liquidity_array.current_tick = tick;
        liquidity_array.current_sqrt_price = tick_to_sqrt_price_u256(tick);
    };

    for position in positions {
        // default true since we are adding all positions.
        liquidity_array.update_liquidity(
            position.tick_lower,
            position.tick_upper,
            position.liquidity as i128,
            true,
        );
    }

    // AFTER the whole liquidity distribution range is set up, we can set the essential caches.
    let (upper_tick_data, lower_tick_data) =
        liquidity_array.get_upper_and_lower_ticks(liquidity_array.current_tick, is_sell)?;

    liquidity_array.cached_lower_initialized_tick = Some(lower_tick_data.tick);
    liquidity_array.cached_upper_initialized_tick = Some(upper_tick_data.tick);

    Ok(liquidity_array)
}

pub async fn sync_backwards<T: TransactionRepoTrait>(
    transaction_repo: &T,
    mut liquidity_array: LiquidityArray,
    pool_model: PoolModel,
    latest_transaction: TransactionModelFromDB,
    batch_size: i64,
) -> Result<(LiquidityArray, TransactionModelFromDB), SyncError> {
    // Initialize the cursor with the latest tx_id
    let mut cursor = Some(latest_transaction.tx_id);

    // Initialize highest_tx_id with the latest transaction ID. The latest txs are the first ones being inserted, so its a low nmr. Then we ascend to the past.
    let mut highest_tx = latest_transaction;

    loop {
        let transactions = transaction_repo
            .fetch_transactions(
                &pool_model.address,
                cursor,
                batch_size,
                OrderDirection::Ascending,
            )
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        if transactions.is_empty() {
            break;
        }

        // Process transactions in order (oldest to newest)
        for transaction in transactions.iter() {
            match transaction.transaction_type.as_str() {
                "IncreaseLiquidity" | "DecreaseLiquidity" => {
                    let liquidity_data = transaction
                        .data
                        .to_liquidity_data()
                        .map_err(|e| SyncError::ParseError(e.to_string()))?;

                    // Reverse the operation for backwards sync
                    let is_increase = transaction.transaction_type.as_str() != "IncreaseLiquidity";

                    let (tick_lower, tick_upper, liquidity_amount) = match (
                        liquidity_data.tick_lower,
                        liquidity_data.tick_upper,
                        liquidity_data.liquidity_amount.parse::<i128>(),
                    ) {
                        (Some(lower), Some(upper), Ok(amount)) => (lower, upper, amount),
                        _ => {
                            // eprintln!(
                            //     "Liquidity transaction missing tick data, skipping: {}",
                            //     transaction.signature
                            // );
                            continue;
                        }
                    };

                    liquidity_array.update_liquidity(
                        tick_lower,
                        tick_upper,
                        liquidity_amount,
                        is_increase,
                    );
                }
                "Swap" => {
                    let swap_data = transaction
                        .data
                        .to_swap_data()
                        .map_err(|e| SyncError::ParseError(e.to_string()))?;

                    let is_sell = swap_data.token_in == pool_model.token_a_address;

                    // Flip the is_sell for backwards sync and always pass in amount_out since we reversing each tx.
                    // For instance we have SOL -> POPCAT (aka sell) with amount_in being SOL. So now we are pasing POPCAT -> SOL and flip sell to buy. Both need reversion!
                    liquidity_array.simulate_swap(U256::from(swap_data.amount_out), !is_sell)?;
                }
                _ => {}
            }

            // Update highest_tx_id
            if highest_tx.tx_id < transaction.tx_id {
                highest_tx = transaction.clone();
            }
        }

        // Update cursor for the next iteration
        cursor = transactions.last().map(|t| t.tx_id + 1);

        if transactions.len() < batch_size as usize {
            break;
        }
    }

    Ok((liquidity_array, highest_tx))
}

// Since the tick might be anywhere in between lower and upper provided ticks from env, we need to rebalance.
// The ratio nmr represents how much % of assets should be in token_a. If ratio is 0.3, then 30% should be in token a. Since token a is on upper side of liquidity.
pub fn calculate_rebalance_ratio(
    curr_sqrt_price: U256,
    upper_sqrt_price: U256,
    lower_sqrt_price: U256,
) -> f64 {
    if curr_sqrt_price >= upper_sqrt_price {
        // All liquidity is in token B.
        0.0
    } else if curr_sqrt_price <= lower_sqrt_price {
        // All liquidity is in token A.
        1.0
    } else {
        ((upper_sqrt_price - curr_sqrt_price).as_u128() as f64)
            / ((upper_sqrt_price - lower_sqrt_price).as_u128() as f64)
    }
}

pub fn calculate_amount_a_needed_for_liquidity(
    rebalance_ratio: f64,
    total_amount_a: f64,
    current_price: f64,
    lower_sqrt_price: U256,
    curr_sqrt_price: U256,
    upper_sqrt_price: U256,
) -> U256 {
    let hypothetical_amount_b = ((1.0 - rebalance_ratio) * total_amount_a) * current_price;

    if rebalance_ratio == 0.0 {
        // Manual amount_a set to avoid overflow errors
        // aka sell all amount a
        U256::zero()
    } else {
        let liquidity_b = calculate_liquidity_b(
            U256::from(hypothetical_amount_b as u128),
            lower_sqrt_price,
            curr_sqrt_price,
        );

        calculate_token_a_from_liquidity(liquidity_b, curr_sqrt_price, upper_sqrt_price)
    }
}

pub fn calculate_amount_b_needed_for_liquidity(
    rebalance_ratio: f64,
    total_amount_a: f64,
    current_price: f64,
    lower_sqrt_price: U256,
    curr_sqrt_price: U256,
    upper_sqrt_price: U256,
) -> U256 {
    // we have too little amount a and so we need to sell B for A.
    let hypothetical_amount_a = rebalance_ratio * total_amount_a;

    if rebalance_ratio == 1.0 {
        // Manual amount_a set to avoid overflow errors
        // aka sell all amount b
        U256::zero()
    } else {
        let liquidity_a = calculate_liquidity_a(
            U256::from(hypothetical_amount_a as u128),
            curr_sqrt_price,
            upper_sqrt_price,
        );

        calculate_token_b_from_liquidity(liquidity_a, curr_sqrt_price, lower_sqrt_price)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::transactions_model::{SwapData, TransactionData, TransactionModelFromDB},
        utils::core_math::{calculate_liquidity, tick_to_sqrt_price_u256},
    };
    use anyhow::Result;
    use async_trait::async_trait;
    use chrono::Utc;

    struct MockTransactionRepo {
        transactions: Vec<TransactionModelFromDB>,
    }

    #[async_trait]
    impl TransactionRepoTrait for MockTransactionRepo {
        async fn fetch_transactions(
            &self,
            _pool_address: &str,
            _cursor: Option<i64>,
            _batch_size: i64,
            _order: OrderDirection,
        ) -> Result<Vec<TransactionModelFromDB>> {
            Ok(self.transactions.clone())
        }
    }

    #[tokio::test]
    async fn test_sync_backwards() {
        let mock_repo_1 = MockTransactionRepo {
            transactions: vec![TransactionModelFromDB {
                tx_id: 1,
                signature: "sig1".to_string(),
                pool_address: "pool1".to_string(),
                block_time: 1000,
                block_time_utc: Utc::now(),
                transaction_type: "Swap".to_string(),
                ready_for_backtesting: true,
                data: TransactionData::Swap(SwapData {
                    token_in: "TokenAAddress".to_string(),
                    token_out: "TokenBAddress".to_string(),
                    // real prices (stuff below has to INCLUDE decimals):
                    // amount_in: 5.301077056,
                    // amount_out: 718.793826,
                    amount_in: 5301077056,
                    amount_out: 718793826,
                }),
            }],
        };

        let pool_model = PoolModel {
            address: "pool1".to_string(),
            name: "TokenA/TokenB".to_string(),
            token_a_name: "TokenA".to_string(),
            token_b_name: "TokenB".to_string(),
            token_a_address: "TokenAAddress".to_string(),
            token_b_address: "TokenBAddress".to_string(),
            token_a_vault: "TokenAVault".to_string(),
            token_b_vault: "TokenBVault".to_string(),
            token_a_decimals: 9,
            token_b_decimals: 6,
            tick_spacing: 1,
            fee_rate: 300, // 0.03%
            last_updated_at: Utc::now(),
        };

        let starting_tick = -19969;
        let lower_tick = -20000;
        let upper_tick = -17000;

        let starting_sqrt_price_u256 = tick_to_sqrt_price_u256(starting_tick);

        let liquidity_1 = calculate_liquidity(
            U256::from(5 * 10_u128.pow(9)),
            U256::from(678 * 10_u128.pow(6)),
            starting_sqrt_price_u256,
            tick_to_sqrt_price_u256(lower_tick),
            tick_to_sqrt_price_u256(upper_tick),
        );

        let mut initial_liquidity_array = LiquidityArray::new(-30000, 30000, 2, 300);
        // necessary so update liquidity sets liq as active liquidity
        initial_liquidity_array.current_tick = starting_tick;
        initial_liquidity_array.current_sqrt_price = starting_sqrt_price_u256;

        initial_liquidity_array.update_liquidity(
            lower_tick,
            upper_tick,
            liquidity_1.as_u128() as i128,
            true,
        );

        // U need two liquidity positions since we only "cross" initialized ticks, so a wide position is very hard to cross.
        let liquidity_2 = calculate_liquidity(
            U256::from(2 * 10_u128.pow(9)),
            U256::from(2 * 135 * 10_u128.pow(6)),
            starting_sqrt_price_u256,
            tick_to_sqrt_price_u256(starting_tick - 10),
            tick_to_sqrt_price_u256(starting_tick + 10),
        );

        initial_liquidity_array.update_liquidity(
            starting_tick - 10,
            starting_tick + 10,
            liquidity_2.as_u128() as i128,
            true,
        );

        let (upper_tick_data, lower_tick_data) = initial_liquidity_array
            .get_upper_and_lower_ticks(starting_tick, true)
            .unwrap();
        initial_liquidity_array.cached_lower_initialized_tick = Some(lower_tick_data.tick);
        initial_liquidity_array.cached_upper_initialized_tick = Some(upper_tick_data.tick);

        let result_1 = sync_backwards(
            &mock_repo_1,
            initial_liquidity_array,
            pool_model.clone(),
            TransactionModelFromDB {
                tx_id: 1,
                signature: "sig1".to_string(),
                pool_address: "pool1".to_string(),
                block_time: 1000,
                block_time_utc: Utc::now(),
                transaction_type: "Swap".to_string(),
                ready_for_backtesting: true,
                data: TransactionData::Swap(SwapData {
                    token_in: "TokenAAddress".to_string(),
                    token_out: "TokenBAddress".to_string(),
                    // THIS CORRESPONDS TO TICK -19969. DONT TOUCH
                    // real nmrs (below has to be decimal included):
                    // amount_in: 5.301077056,
                    // amount_out: 718.793826,
                    amount_in: 5301077056,
                    amount_out: 718793826,
                }),
            },
            10,
        )
        .await;

        assert!(result_1.is_ok(), "sync_backwards should succeed");

        let final_liquidity_array = result_1.unwrap().0;
        let new_curr_sqrt_price = final_liquidity_array.current_sqrt_price;
        let new_curr_tick = final_liquidity_array.current_tick;

        assert!(
            final_liquidity_array.current_tick >= starting_tick,
            "The SELL reversed transaction (ie buy) should have increased the tick."
        );
        assert!(
            final_liquidity_array.current_sqrt_price > starting_sqrt_price_u256,
            "The SELL reversed transaction (ie buy) should have increased the sqrtPrice."
        );

        let mock_repo_2 = MockTransactionRepo {
            transactions: vec![TransactionModelFromDB {
                tx_id: 3,
                signature: "sig3".to_string(),
                pool_address: "pool1".to_string(),
                block_time: 1200,
                block_time_utc: Utc::now(),
                transaction_type: "Swap".to_string(),
                ready_for_backtesting: true,
                data: TransactionData::Swap(SwapData {
                    token_in: "TokenBAddress".to_string(),
                    token_out: "TokenAAddress".to_string(),
                    // MUST CORRESPOND TO TICK (-19959) since thats where it ended last time. the PRICE is 135.904
                    // real price (below has to be real code):
                    // amount_in: 4.0 * 135.904,
                    // amount_out: 4.0,
                    amount_in: 4 * 135904 * 10_u64.pow(6) / 1000, // the 1000 to normalize the price to 135.904
                    amount_out: 4 * 10_u64.pow(9),
                }),
            }],
        };

        let result_2 = sync_backwards(
            &mock_repo_2,
            final_liquidity_array,
            pool_model,
            TransactionModelFromDB {
                tx_id: 1,
                signature: "sig1".to_string(),
                pool_address: "pool1".to_string(),
                block_time: 1000,
                block_time_utc: Utc::now(),
                transaction_type: "Swap".to_string(),
                ready_for_backtesting: true,
                data: TransactionData::Swap(SwapData {
                    token_in: "TokenBAddress".to_string(),
                    token_out: "TokenAAddress".to_string(),
                    // MUST CORRESPOND TO TICK (-19959) since thats where it ended last time. the PRICE is 135.904
                    // real price (below has to be real code):
                    // amount_in: 1.0 * 135.904,
                    // amount_out: 1.0,
                    amount_in: 135904 * 10_u64.pow(6) / 1000, // the 1000 to normalize the price to 135.904
                    amount_out: 1 * 10_u64.pow(9),
                }),
            },
            10,
        )
        .await;

        let final_liquidity_array_2 = result_2.unwrap().0;

        assert!(
            final_liquidity_array_2.current_tick <= new_curr_tick,
            "The BUY reversed transaction (ie sell) should have decreased the tick."
        );
        assert!(
            final_liquidity_array_2.current_sqrt_price < new_curr_sqrt_price,
            "The BUY reversed transaction (ie sell) should have decreased the sqrtPrice."
        );

        // SHOULD END UP QUITE CLOSE TO EACH OTHER. OBV BECAUSE OF PRICE DIFF IT DOESNT.
        let sqrt_price_diff =
            if final_liquidity_array_2.current_sqrt_price > starting_sqrt_price_u256 {
                final_liquidity_array_2.current_sqrt_price - starting_sqrt_price_u256
            } else {
                starting_sqrt_price_u256 - final_liquidity_array_2.current_sqrt_price
            };

        let percentage_diff =
            (sqrt_price_diff.as_u128() as f64 / starting_sqrt_price_u256.as_u128() as f64) * 100.0;

        assert!(
            percentage_diff < 0.1,
            "Final sqrt_price should be within 0.1% of starting sqrt_price. Actual difference: {}%",
            percentage_diff
        );
    }
}
