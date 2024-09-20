use anyhow::Result;

use crate::{
    models::transactions_model::TransactionModelFromDB,
    repositories::transactions_repo::{OrderDirection, TransactionRepoTrait},
    utils::{
        error::{BacktestError, SyncError},
        price_calcs::{
            calculate_amounts, calculate_liquidity, calculate_rebalance_amount,
            tick_to_sqrt_price_u256, Q64, U256,
        },
    },
};

use super::liquidity_array::{LiquidityArray, OwnersPosition};

pub struct StartInfo {
    pub token_a_amount: U256,
    pub token_b_amount: U256,
}

#[derive(Debug, Clone)]
pub struct Wallet {
    pub token_a_addr: String,
    pub token_b_addr: String,
    // ALREADY MULTIPLIED BY Q64.
    pub amount_token_a: U256,
    pub amount_token_b: U256,
    pub token_a_decimals: i16,
    pub token_b_decimals: i16,
    // FEES CALCULATED SEPARATELY BUT ALSO ADDED DURING REBALANCING
    pub amount_a_fees_collected: U256,
    pub amount_b_fees_collected: U256,
    pub total_profit: f64,
    pub total_profit_pct: f64,
}

pub enum Action {
    ProvideLiquidity {
        position_id: i32,
        liquidity_to_add: U256,
    },
    RemoveLiquidity {
        position_id: i32,
        liquidity_to_remove: U256,
    },
    Swap {
        amount_in: U256,
        is_sell: bool,
    },
    // Rebalance will take the OwnersPosition, remove the liquidity, swap to get 50/50 in token_a/token_b and then provide liquidity.
    Rebalance {
        position_id: String,
        rebalance_ratio: f64, // the rebalance ratio that we both want to sell at but also provide liquidity at. so if 0.6 the range will mean 60% in token_b and 40% token_a with a projection of token a increasing in price.
        new_upper_tick: i32,
        new_lower_tick: i32,
    },
    CreatePosition {
        position_id: String,
        lower_tick: i32,
        upper_tick: i32,
        amount_a: U256,
        amount_b: U256,
    },
    FinalizeStrategy {
        position_id: String,
        starting_sqrt_price: U256,
    },
}

pub struct Backtest {
    pub wallet: Wallet,
    pub liquidity_arr: LiquidityArray,
    pub strategy: Box<dyn Strategy>,
    pub start_info: StartInfo,
}

pub trait Strategy {
    fn initialize_strategy(&self, amount_a: U256, amount_b: U256) -> Vec<Action>;

    fn update(
        &mut self,
        liquidity_array: &LiquidityArray,
        transaction: TransactionModelFromDB,
    ) -> Vec<Action>;

    fn finalize_strategy(&self, starting_sqrt_price: U256) -> Vec<Action>;
}

impl Backtest {
    pub fn new(
        amount_a_start: U256,
        amount_b_start: U256,
        liquidity_arr: LiquidityArray,
        wallet_state: Wallet,
        strategy: Box<dyn Strategy>,
    ) -> Self {
        Self {
            start_info: StartInfo {
                token_a_amount: amount_a_start,
                token_b_amount: amount_b_start,
            },
            liquidity_arr,
            wallet: wallet_state,
            strategy,
        }
    }

    pub async fn sync_forward<T: TransactionRepoTrait>(
        &mut self,
        transaction_repo: &T,
        start_tx_id: i64,
        end_tx_id: i64,
        pool_address: &str,
        batch_size: i64,
    ) -> Result<(), SyncError> {
        let mut cursor = Some(start_tx_id);
        let starting_price = self.liquidity_arr.current_sqrt_price;

        while cursor.is_some() && cursor.unwrap() <= end_tx_id {
            let transactions = transaction_repo
                .fetch_transactions(pool_address, cursor, batch_size, OrderDirection::Ascending)
                .await
                .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

            if transactions.is_empty() {
                break;
            }

            for transaction in transactions.iter() {
                match transaction.transaction_type.as_str() {
                    "IncreaseLiquidity" | "DecreaseLiquidity" => {
                        let liquidity_data = transaction
                            .data
                            .to_liquidity_data()
                            .map_err(|e| SyncError::ParseError(e.to_string()))?;

                        let is_increase =
                            transaction.transaction_type.as_str() == "IncreaseLiquidity";

                        self.liquidity_arr.update_liquidity(
                            liquidity_data.tick_lower.unwrap(),
                            liquidity_data.tick_upper.unwrap(),
                            liquidity_data.liquidity_amount.parse::<u128>().unwrap() as i128,
                            is_increase,
                        );
                    }
                    "Swap" => {
                        let swap_data = transaction
                            .data
                            .to_swap_data()
                            .map_err(|e| SyncError::ParseError(e.to_string()))?;

                        let is_sell = swap_data.token_in == self.wallet.token_a_addr;

                        // Adjust the amount based on the correct token decimals
                        let adjusted_amount = if is_sell {
                            // If selling token A, use token A decimals
                            swap_data.amount_in as u128
                                * 10u128.pow(self.wallet.token_a_decimals as u32)
                        } else {
                            // If selling token B, use token B decimals
                            swap_data.amount_in as u128
                                * 10u128.pow(self.wallet.token_b_decimals as u32)
                        };

                        self.liquidity_arr
                            .simulate_swap(U256::from(adjusted_amount), is_sell)?;
                    }
                    _ => {}
                }

                // Process strategy actions
                let actions = self
                    .strategy
                    .update(&self.liquidity_arr, transaction.clone());

                for action in actions {
                    self.execute_action(action)
                        .map_err(|e| SyncError::Other(e.to_string()))?;
                }
            }

            cursor = transactions.last().map(|t| t.tx_id + 1);

            if transactions.len() < batch_size as usize || cursor.unwrap() > end_tx_id {
                break;
            }
        }

        self.strategy.finalize_strategy(starting_price);

        Ok(())
    }

    fn execute_action(&mut self, action: Action) -> Result<(), BacktestError> {
        match action {
            Action::ProvideLiquidity {
                position_id,
                liquidity_to_add,
            } => {
                todo!();
            }
            Action::RemoveLiquidity {
                position_id,
                liquidity_to_remove,
            } => {
                todo!();
            }
            Action::Swap { amount_in, is_sell } => {
                self.liquidity_arr.simulate_swap(amount_in, is_sell)?;

                Ok(())
            }
            Action::Rebalance {
                position_id,
                rebalance_ratio,
                new_lower_tick,
                new_upper_tick,
            } => {
                let (fees_a, fees_b) = self.liquidity_arr.collect_fees(&position_id)?;

                self.wallet.amount_a_fees_collected = fees_a;
                self.wallet.amount_b_fees_collected = fees_b;

                let position = self.liquidity_arr.remove_owners_position(&position_id)?;

                let (amount_a, amount_b) = calculate_amounts(
                    U256::from(position.liquidity),
                    self.liquidity_arr.current_sqrt_price,
                    tick_to_sqrt_price_u256(position.lower_tick),
                    tick_to_sqrt_price_u256(position.upper_tick),
                );
                let mut amount_a_with_fees = amount_a + fees_a;
                let mut amount_b_with_fees = amount_b + fees_b;

                let (amount_to_sell, is_sell) = calculate_rebalance_amount(
                    amount_a_with_fees,
                    amount_b_with_fees,
                    self.liquidity_arr.current_sqrt_price,
                    U256::from((rebalance_ratio * Q64.as_u128() as f64) as u128),
                );

                // TODO: Optimize for small imbalances to avoid unnecessary swaps. Also add slippage later.
                // For now, we'll proceed with the swap even for small amounts
                let amount_out = self.liquidity_arr.simulate_swap(amount_to_sell, is_sell)?;

                if is_sell {
                    amount_a_with_fees -= amount_to_sell;
                    amount_b_with_fees += amount_out;
                    self.wallet.amount_a_fees_collected += fees_a;
                } else {
                    amount_a_with_fees -= amount_to_sell;
                    amount_b_with_fees += amount_out;
                    self.wallet.amount_b_fees_collected += fees_b;
                }

                let new_liquidity = calculate_liquidity(
                    amount_a_with_fees,
                    amount_b_with_fees,
                    self.liquidity_arr.current_sqrt_price,
                    tick_to_sqrt_price_u256(new_lower_tick),
                    tick_to_sqrt_price_u256(new_upper_tick),
                );

                // Re-provide liquidity with the rebalanced amounts
                self.liquidity_arr.add_owners_position(
                    OwnersPosition {
                        owner: position.owner,
                        lower_tick: new_lower_tick,
                        upper_tick: new_upper_tick,
                        liquidity: new_liquidity.as_u128() as i128,
                        fee_growth_inside_a_last: U256::zero(),
                        fee_growth_inside_b_last: U256::zero(),
                    },
                    position_id.clone(),
                );

                // Log the rebalancing action
                println!(
                    "Rebalanced position {}: New liquidity: {}, Amount A: {}, Amount B: {}",
                    position_id, new_liquidity, amount_a_with_fees, amount_b_with_fees
                );

                Ok(())
            }
            Action::CreatePosition {
                position_id,
                lower_tick,
                upper_tick,
                amount_a,
                amount_b,
            } => {
                if amount_a > self.wallet.amount_token_a {
                    return Err(BacktestError::InsufficientBalance {
                        requested: amount_a,
                        available: self.wallet.amount_token_a,
                        token: self.wallet.token_a_addr.clone(),
                    });
                }

                if amount_b > self.wallet.amount_token_b {
                    return Err(BacktestError::InsufficientBalance {
                        requested: amount_b,
                        available: self.wallet.amount_token_b,
                        token: self.wallet.token_b_addr.clone(),
                    });
                }

                let liquidity = calculate_liquidity(
                    amount_a,
                    amount_b,
                    self.liquidity_arr.current_sqrt_price,
                    tick_to_sqrt_price_u256(lower_tick),
                    tick_to_sqrt_price_u256(upper_tick),
                );

                self.liquidity_arr.add_owners_position(
                    OwnersPosition {
                        owner: String::from(""),
                        lower_tick,
                        upper_tick,
                        liquidity: liquidity.as_u128() as i128,
                        fee_growth_inside_a_last: U256::zero(),
                        fee_growth_inside_b_last: U256::zero(),
                    },
                    position_id,
                );

                self.wallet.amount_token_a -= amount_a;
                self.wallet.amount_token_b -= amount_b;

                Ok(())
            }
            Action::FinalizeStrategy {
                position_id,
                starting_sqrt_price,
            } => {
                let (fees_a, fees_b) = self.liquidity_arr.collect_fees(&position_id)?;
                let position = self.liquidity_arr.remove_owners_position(&position_id)?;

                self.wallet.amount_a_fees_collected += fees_a;
                self.wallet.amount_b_fees_collected += fees_b;

                let (amount_a, amount_b) = calculate_amounts(
                    U256::from(position.liquidity),
                    self.liquidity_arr.current_sqrt_price,
                    tick_to_sqrt_price_u256(position.lower_tick),
                    tick_to_sqrt_price_u256(position.upper_tick),
                );

                self.wallet.amount_token_a += amount_a;
                self.wallet.amount_token_b += amount_b;

                // Calculate initial value in terms of token A
                let initial_price = (starting_sqrt_price * starting_sqrt_price) / Q64;
                let initial_value_a = self.start_info.token_a_amount
                    + (self.start_info.token_b_amount * Q64) / initial_price;

                // Calculate the final value in terms of token A
                let current_price = (self.liquidity_arr.current_sqrt_price
                    * self.liquidity_arr.current_sqrt_price)
                    / Q64;
                let final_value_a = (self.wallet.amount_token_a
                    + self.wallet.amount_a_fees_collected)
                    + ((self.wallet.amount_token_b + self.wallet.amount_b_fees_collected) * Q64)
                        / current_price;

                // Calculate profit
                let profit = final_value_a.saturating_sub(initial_value_a);
                let loss = initial_value_a.saturating_sub(final_value_a);

                // Determine if it's a profit or loss
                let is_profit = final_value_a > initial_value_a;

                // Calculate profit or loss as a float
                let profit_or_loss = if is_profit {
                    profit.as_u128() as f64 / 10f64.powi(self.wallet.token_a_decimals as i32)
                } else {
                    -(loss.as_u128() as f64 / 10f64.powi(self.wallet.token_a_decimals as i32))
                };

                // Calculate percentage
                let profit_percentage = (profit_or_loss
                    / (initial_value_a.as_u128() as f64
                        / 10f64.powi(self.wallet.token_a_decimals as i32)))
                    * 100.0;

                // Store results
                self.wallet.total_profit = profit_or_loss;
                self.wallet.total_profit_pct = profit_percentage;

                println!("Strategy finalized:");
                println!("Initial value (in token A): {}", initial_value_a);
                println!("Final value (in token A): {}", final_value_a);
                println!("Total profit: {} token A", self.wallet.total_profit);
                println!("Total profit percentage: {}%", self.wallet.total_profit_pct);

                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct MockStrategy;

    impl Strategy for MockStrategy {
        fn initialize_strategy(&self, _amount_a: U256, _amount_b: U256) -> Vec<Action> {
            vec![]
        }

        fn update(
            &mut self,
            _liquidity_array: &LiquidityArray,
            _transaction: TransactionModelFromDB,
        ) -> Vec<Action> {
            vec![]
        }

        fn finalize_strategy(&self, _starting_sqrt_price: U256) -> Vec<Action> {
            vec![]
        }
    }

    struct MockTransactionRepo {
        transactions: Arc<Mutex<Vec<TransactionModelFromDB>>>,
    }

    #[async_trait::async_trait]
    impl TransactionRepoTrait for MockTransactionRepo {
        async fn fetch_transactions(
            &self,
            _pool_address: &str,
            _cursor: Option<i64>,
            _batch_size: i64,
            _order: OrderDirection,
        ) -> Result<Vec<TransactionModelFromDB>, anyhow::Error> {
            Ok(self.transactions.lock().await.clone())
        }

        async fn fetch_highest_tx_swap(
            &self,
            pool_address: &str,
        ) -> Result<Option<TransactionModelFromDB>> {
            Ok(None)
        }
    }

    fn create_test_liquidity_array() -> LiquidityArray {
        LiquidityArray::new(-500_000, 500_000, 10, 500)
    }

    #[tokio::test]
    async fn test_backtest_initialization() {
        let liquidity_arr = create_test_liquidity_array();

        let starting_token_a_amount = U256::from(1000_u128 * 10_u128.pow(6));
        let starting_token_b_amount = U256::from(1000_u128 * 10_u128.pow(6));

        let wallet = Wallet {
            token_a_addr: "TokenA".to_string(),
            token_b_addr: "TokenB".to_string(),
            amount_token_a: starting_token_a_amount,
            amount_token_b: starting_token_b_amount,
            token_a_decimals: 6,
            token_b_decimals: 6,
            amount_a_fees_collected: U256::zero(),
            amount_b_fees_collected: U256::zero(),
            total_profit: 0.0,
            total_profit_pct: 0.0,
        };
        let strategy = Box::new(MockStrategy);

        let backtest = Backtest::new(
            starting_token_a_amount,
            starting_token_b_amount,
            liquidity_arr,
            wallet,
            strategy,
        );

        assert_eq!(
            backtest.start_info.token_a_amount,
            U256::from(1_000_000_000)
        );
        assert_eq!(
            backtest.start_info.token_b_amount,
            U256::from(1_000_000_000)
        );
    }

    #[tokio::test]
    async fn test_create_position() {
        let liquidity_arr = create_test_liquidity_array();
        let wallet = Wallet {
            token_a_addr: "TokenA".to_string(),
            token_b_addr: "TokenB".to_string(),
            amount_token_a: U256::from(1_000_000_000),
            amount_token_b: U256::from(1_000_000_000),
            token_a_decimals: 6,
            token_b_decimals: 6,
            amount_a_fees_collected: U256::zero(),
            amount_b_fees_collected: U256::zero(),
            total_profit: 0.0,
            total_profit_pct: 0.0,
        };
        let strategy = Box::new(MockStrategy);

        let mut backtest = Backtest::new(
            U256::from(1000),
            U256::from(1000),
            liquidity_arr,
            wallet,
            strategy,
        );

        let action = Action::CreatePosition {
            position_id: "test_position".to_string(),
            lower_tick: 900,
            upper_tick: 1100,
            amount_a: U256::from(500_000_000_u128),
            amount_b: U256::from(500_000_000),
        };

        backtest.execute_action(action).unwrap();

        assert_eq!(backtest.wallet.amount_token_a, U256::from(500_000_000));
        assert_eq!(backtest.wallet.amount_token_b, U256::from(500_000_000));

        assert!(backtest
            .liquidity_arr
            .positions
            .contains_key("test_position"));
    }

    #[tokio::test]
    async fn test_rebalance_action() {
        let starting_amount_a = U256::from(100 * 10_i32.pow(6));
        let starting_amount_b = U256::from(100 * 10_i32.pow(6));

        let current_tick = 11;
        let lower_tick = current_tick - 100;
        let upper_tick = current_tick + 100;

        let wallet = Wallet {
            token_a_addr: "TokenA".to_string(),
            token_b_addr: "TokenB".to_string(),
            amount_token_a: starting_amount_a,
            amount_token_b: starting_amount_b,
            token_a_decimals: 6,
            token_b_decimals: 6,
            amount_a_fees_collected: U256::zero(),
            amount_b_fees_collected: U256::zero(),
            total_profit: 0.0,
            total_profit_pct: 0.0,
        };
        let strategy = Box::new(MockStrategy);

        let mut backtest = Backtest::new(
            starting_amount_a,
            starting_amount_b,
            create_test_liquidity_array(),
            wallet,
            strategy,
        );

        backtest.liquidity_arr.current_tick = current_tick;
        backtest.liquidity_arr.current_sqrt_price = tick_to_sqrt_price_u256(current_tick);

        // have to create multiple positions since if we rebalance the only position that exists, errors happen.
        backtest.liquidity_arr.update_liquidity(
            lower_tick - 100,
            upper_tick + 100,
            calculate_liquidity(
                starting_amount_a * 5,
                starting_amount_b * 5,
                tick_to_sqrt_price_u256(current_tick),
                tick_to_sqrt_price_u256(lower_tick - 100),
                tick_to_sqrt_price_u256(upper_tick + 100),
            )
            .as_u128() as i128,
            true,
        );

        // Actions sequence
        let all_actions = vec![
            Action::CreatePosition {
                position_id: "test_position".to_string(),
                lower_tick,
                upper_tick,
                amount_a: starting_amount_a,
                amount_b: starting_amount_b,
            },
            Action::Swap {
                amount_in: U256::from(7_000_000),
                is_sell: true,
            },
            Action::Swap {
                amount_in: U256::from(10_000_000),
                is_sell: false,
            },
            Action::Swap {
                amount_in: U256::from(3_000_000),
                is_sell: true,
            },
            Action::Rebalance {
                position_id: "test_position".to_string(),
                rebalance_ratio: 0.5,            // 50/50 ratio
                new_lower_tick: lower_tick - 50, // Slightly wider range
                new_upper_tick: upper_tick + 50,
            },
        ];

        // Execute all actions
        for action in all_actions {
            backtest.execute_action(action).unwrap();
        }

        // Get the rebalanced position
        let rebalanced_position = backtest
            .liquidity_arr
            .positions
            .get("test_position")
            .unwrap();

        // Assertions
        assert!(
            rebalanced_position.lower_tick == lower_tick - 50,
            "Lower tick should be updated"
        );
        assert!(
            rebalanced_position.upper_tick == upper_tick + 50,
            "Upper tick should be updated"
        );

        assert!(
            backtest.wallet.amount_a_fees_collected > U256::zero(),
            "Should have collected some fees for token A"
        );
        assert!(
            backtest.wallet.amount_b_fees_collected > U256::zero(),
            "Should have collected some fees for token B"
        );

        assert!(
            backtest.wallet.amount_token_a == U256::zero(),
            "Nothing left in wallet"
        );
        assert!(
            backtest.wallet.amount_token_b == U256::zero(),
            "Nothing left in wallet"
        );
    }

    #[tokio::test]
    async fn test_finalize_strategy_in_range() {
        let starting_amount_a = U256::from(100 * 10_i32.pow(6));
        let starting_amount_b = U256::from(100 * 10_i32.pow(6));

        let current_tick = 11;
        let lower_tick = current_tick - 100;
        let upper_tick = current_tick + 100;

        let wallet = Wallet {
            token_a_addr: "TokenA".to_string(),
            token_b_addr: "TokenB".to_string(),
            amount_token_a: starting_amount_a,
            amount_token_b: starting_amount_b,
            token_a_decimals: 6,
            token_b_decimals: 6,
            amount_a_fees_collected: U256::zero(),
            amount_b_fees_collected: U256::zero(),
            total_profit: 0.0,
            total_profit_pct: 0.0,
        };
        let strategy = Box::new(MockStrategy);

        let mut backtest = Backtest::new(
            starting_amount_a,
            starting_amount_b,
            create_test_liquidity_array(),
            wallet,
            strategy,
        );

        backtest.liquidity_arr.current_tick = current_tick;
        backtest.liquidity_arr.current_sqrt_price = tick_to_sqrt_price_u256(current_tick);

        // Simulate some swaps to accumulate fees
        let all_actions = vec![
            Action::CreatePosition {
                position_id: "test_position".to_string(),
                lower_tick,
                upper_tick,
                amount_a: starting_amount_a,
                amount_b: starting_amount_b,
            },
            Action::Swap {
                amount_in: U256::from(7_000_000),
                is_sell: true,
            },
            Action::Swap {
                amount_in: U256::from(10_000_000_i128),
                is_sell: false,
            },
            Action::Swap {
                amount_in: U256::from(3_000_000),
                is_sell: true,
            },
            Action::FinalizeStrategy {
                position_id: "test_position".to_string(),
                starting_sqrt_price: tick_to_sqrt_price_u256(current_tick), // pass the starting tick
            },
        ];

        for action in all_actions {
            backtest.execute_action(action).unwrap();
        }

        // Assertions
        assert!(
            backtest.wallet.amount_a_fees_collected > U256::zero(),
            "Should have collected some fees for token A"
        );
        assert!(
            backtest.wallet.amount_b_fees_collected > U256::zero(),
            "Should have collected some fees for token B"
        );
        assert!(
            backtest.wallet.total_profit > 0.0,
            "Total profit should be positive"
        );
        assert!(
            backtest.wallet.total_profit_pct > 0.0,
            "Profit percentage should  be positive"
        );
    }

    #[tokio::test]
    async fn test_finalize_strategy_oustide_range_in_token_b() {
        let starting_amount_a = U256::from(100 * 10_i32.pow(6));
        let starting_amount_b = U256::from(100 * 10_i32.pow(6));

        let current_tick = 11;
        let lower_tick = current_tick - 100;
        let upper_tick = current_tick + 100;

        let wallet = Wallet {
            token_a_addr: "TokenA".to_string(),
            token_b_addr: "TokenB".to_string(),
            amount_token_a: starting_amount_a,
            amount_token_b: starting_amount_b,
            token_a_decimals: 6,
            token_b_decimals: 6,
            amount_a_fees_collected: U256::zero(),
            amount_b_fees_collected: U256::zero(),
            total_profit: 0.0,
            total_profit_pct: 0.0,
        };
        let strategy = Box::new(MockStrategy);

        let mut backtest = Backtest::new(
            starting_amount_a,
            starting_amount_b,
            create_test_liquidity_array(),
            wallet,
            strategy,
        );

        backtest.liquidity_arr.current_tick = current_tick;
        backtest.liquidity_arr.current_sqrt_price = tick_to_sqrt_price_u256(current_tick);

        // have to create multiple positions for one to be out of range
        // cant use add owners position since that has limited balance
        backtest.liquidity_arr.update_liquidity(
            lower_tick - 100,
            upper_tick + 100,
            calculate_liquidity(
                starting_amount_a * 5,
                starting_amount_b * 5,
                tick_to_sqrt_price_u256(current_tick),
                tick_to_sqrt_price_u256(lower_tick - 100),
                tick_to_sqrt_price_u256(upper_tick + 100),
            )
            .as_u128() as i128,
            true,
        );

        // Simulate some swaps to accumulate fees
        let all_actions = vec![
            Action::CreatePosition {
                position_id: "test_position".to_string(),
                lower_tick,
                upper_tick,
                amount_a: starting_amount_a,
                amount_b: starting_amount_b,
            },
            Action::Swap {
                amount_in: U256::from(800_000_000_i128),
                is_sell: false,
            },
            Action::FinalizeStrategy {
                position_id: "test_position".to_string(),
                starting_sqrt_price: tick_to_sqrt_price_u256(current_tick), // pass the starting tick
            },
        ];

        for action in all_actions {
            backtest.execute_action(action).unwrap();
        }

        // Assertions
        assert!(
            backtest.wallet.amount_b_fees_collected > U256::zero(),
            "Should have collected some fees for token B"
        );
        assert!(
            backtest.wallet.amount_a_fees_collected == U256::zero(),
            "Should have not collected fees for token A"
        );

        assert!(
            backtest.wallet.amount_token_a == U256::zero(),
            "Wiped out token a liquidity"
        );
        assert!(
            backtest.wallet.amount_token_b > starting_amount_b,
            "All liquidity in token B, above starting"
        );

        assert!(
            backtest.wallet.total_profit > 0.0,
            "Total profit should be positive"
        );
        assert!(
            backtest.wallet.total_profit_pct > 0.0,
            "Profit percentage should  be positive"
        );
    }

    #[tokio::test]
    async fn test_finalize_strategy_oustide_range_in_token_a() {
        let starting_amount_a = U256::from(100 * 10_i32.pow(6));
        let starting_amount_b = U256::from(100 * 10_i32.pow(6));

        let current_tick = 11;
        let lower_tick = current_tick - 100;
        let upper_tick = current_tick + 100;

        let wallet = Wallet {
            token_a_addr: "TokenA".to_string(),
            token_b_addr: "TokenB".to_string(),
            amount_token_a: starting_amount_a,
            amount_token_b: starting_amount_b,
            token_a_decimals: 6,
            token_b_decimals: 6,
            amount_a_fees_collected: U256::zero(),
            amount_b_fees_collected: U256::zero(),
            total_profit: 0.0,
            total_profit_pct: 0.0,
        };
        let strategy = Box::new(MockStrategy);

        let mut backtest = Backtest::new(
            starting_amount_a,
            starting_amount_b,
            create_test_liquidity_array(),
            wallet,
            strategy,
        );

        backtest.liquidity_arr.current_tick = current_tick;
        backtest.liquidity_arr.current_sqrt_price = tick_to_sqrt_price_u256(current_tick);

        // have to create multiple positions for one to be out of range
        // cant use add owners position since that has limited balance
        backtest.liquidity_arr.update_liquidity(
            lower_tick - 100,
            upper_tick + 100,
            calculate_liquidity(
                starting_amount_a * 5,
                starting_amount_b * 5,
                tick_to_sqrt_price_u256(current_tick),
                tick_to_sqrt_price_u256(lower_tick - 100),
                tick_to_sqrt_price_u256(upper_tick + 100),
            )
            .as_u128() as i128,
            true,
        );

        // Simulate some swaps to accumulate fees
        let all_actions = vec![
            Action::CreatePosition {
                position_id: "test_position".to_string(),
                lower_tick,
                upper_tick,
                amount_a: starting_amount_a,
                amount_b: starting_amount_b,
            },
            Action::Swap {
                amount_in: U256::from(600_000_000_i128),
                is_sell: true, // SELLING TOKEN A
            },
            Action::FinalizeStrategy {
                position_id: "test_position".to_string(),
                starting_sqrt_price: tick_to_sqrt_price_u256(current_tick), // pass the starting tick
            },
        ];

        for action in all_actions {
            backtest.execute_action(action).unwrap();
        }

        // Assertions
        assert!(
            backtest.wallet.amount_a_fees_collected > U256::zero(),
            "Should have collected some fees for token A"
        );
        assert!(
            backtest.wallet.amount_b_fees_collected == U256::zero(),
            "Should have not collected fees for token B"
        );

        assert!(
            backtest.wallet.amount_token_b == U256::zero(),
            "Wiped out token B liquidity"
        );
        assert!(
            backtest.wallet.amount_token_a > starting_amount_a,
            "All liquidity in token A, above starting"
        );

        assert!(
            backtest.wallet.total_profit > 0.0,
            "Total profit should be positive"
        );
        assert!(
            backtest.wallet.total_profit_pct > 0.0,
            "Profit percentage should  be positive"
        );
    }
}
