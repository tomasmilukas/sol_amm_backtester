use anyhow::Result;

use crate::{
    models::{pool_model::PoolModel, positions_model::PositionModel},
    repositories::transactions_repo::{OrderDirection, TransactionRepo, TransactionRepoTrait},
    utils::{
        error::SyncError,
        price_calcs::{sqrt_price_to_fixed, sqrt_price_to_tick},
    },
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

pub async fn sync_backwards<T: TransactionRepoTrait>(
    transaction_repo: &T,
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

    if is_sell {
        let sqrt_price = (swap_data.amount_out / swap_data.amount_in).sqrt();
        let tick = sqrt_price_to_tick(sqrt_price);

        liquidity_array.current_sqrt_price = sqrt_price_to_fixed(sqrt_price);
        liquidity_array.current_tick = tick;
    } else {
        let sqrt_price = (swap_data.amount_in / swap_data.amount_out).sqrt();
        let tick = sqrt_price_to_tick(sqrt_price);

        liquidity_array.current_sqrt_price = sqrt_price_to_fixed(sqrt_price);
        liquidity_array.current_tick = tick;
    };

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

        // As we are syncing backwards, everything needs to be the opposite. Increase liquidity = remove and so on. Sell swap is a buy swap with reverse amount_in and amount_out.
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

                    // reverse
                    let is_increase = transaction.transaction_type.as_str() != "IncreaseLiquidity";

                    liquidity_array.update_liquidity_from_tx(tick_data, is_increase);
                }
                "Swap" => {
                    let swap_data = transaction
                        .data
                        .to_swap_data()
                        .map_err(|e| SyncError::ParseError(e.to_string()))?;

                    // reverse
                    let is_sell = swap_data.token_in != pool_model.token_a_address;

                    // reverse amount_in and amount_out.
                    let reverse_amount_in = swap_data.amount_out;

                    liquidity_array
                        .simulate_swap_with_fees((reverse_amount_in) as u128, is_sell)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::transactions_model::{
         SwapData, TransactionData, TransactionModelFromDB,
    };
    use anyhow::Result;
    use async_trait::async_trait;
    use chrono::Utc;

    struct MockTransactionRepo {
        transactions: Vec<TransactionModelFromDB>,
    }

    #[async_trait]
    impl TransactionRepoTrait for MockTransactionRepo {
        async fn fetch_highest_tx_swap(
            &self,
            _pool_address: &str,
        ) -> Result<Option<TransactionModelFromDB>> {
            Ok(self.transactions.last().cloned())
        }

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
                    amount_in: 50.0,
                    amount_out: 100.0,
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
            token_a_decimals: 6,
            token_b_decimals: 6,
            tick_spacing: 1,
            total_liquidity: Some("100000000000".to_string()), // 100B
            fee_rate: 300,                                     // 0.03%
            last_updated_at: Utc::now(),
        };

        let mut initial_liquidity_array = LiquidityArray::new(-10000, 10000, 2, 300);
        initial_liquidity_array.update_liquidity(TickData {
            lower_tick: 5500,
            upper_tick: 8000,
            liquidity: 10_000_000_000,
        });

        let starting_tick = 6931;
        let starting_sqrt_price_u128 = 6074000999_u128;

        let result_1 = sync_backwards(
            &mock_repo_1,
            initial_liquidity_array,
            pool_model.clone(),
            10,
        )
        .await;

        assert!(result_1.is_ok(), "sync_backwards should succeed");

        let final_liquidity_array = result_1.unwrap();

        assert!(
            final_liquidity_array.current_tick >= starting_tick,
            "The SELL reversed transaction should have increased the tick."
        );
        assert!(
            final_liquidity_array.current_sqrt_price > starting_sqrt_price_u128,
            "The SELL reversed transaction should have increased the sqrtPrice."
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
                    amount_in: 100.0,
                    amount_out: 50.0,
                }),
            }],
        };

        let result_2 = sync_backwards(&mock_repo_2, final_liquidity_array, pool_model, 10).await;
        let final_liquidity_array_2 = result_2.unwrap();

        assert!(
            final_liquidity_array_2.current_tick <= starting_tick,
            "The BUY reversed transaction should have decreased the tick."
        );
        assert!(
            final_liquidity_array_2.current_sqrt_price < starting_sqrt_price_u128,
            "The BUY reversed transaction should have decreased the sqrtPrice."
        );

        // SHOULD END UP QUITE CLOSE TO EACH OTHER. OBV BECAUSE OF PRICE DIFF IT DOESNT.
        let sqrt_price_diff = (final_liquidity_array_2.current_sqrt_price as i128 - starting_sqrt_price_u128 as i128).abs();
        let percentage_diff = (sqrt_price_diff as f64 / starting_sqrt_price_u128 as f64) * 100.0;

        assert!(
            percentage_diff < 0.001, 
            "Final sqrt_price should be within 0.001% of starting sqrt_price. Actual difference: {}%", 
            percentage_diff
        );
    }
}