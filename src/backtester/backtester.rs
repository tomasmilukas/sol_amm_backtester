use anyhow::Result;

use crate::{
    models::transactions_model::TransactionModelFromDB,
    repositories::transactions_repo::{OrderDirection, TransactionRepoTrait},
    utils::{
        core_math::{
            calculate_amounts, calculate_liquidity, calculate_liquidity_a, calculate_liquidity_b,
            calculate_token_a_from_liquidity, calculate_token_b_from_liquidity,
            tick_to_sqrt_price_u256, Q128, Q64, U256,
        },
        error::{BacktestError, SyncError},
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
}

// The types of actions you can take as a user. For now, creating/closing uses your full wallet amounts. Later support can be added for custom amounts, also updating liquidity (increase/decrease) and so on.
pub enum Action {
    ClosePosition {
        position_id: String,
    },
    CreatePosition {
        position_id: String,
        lower_tick: i32,
        upper_tick: i32,
    },
}

pub struct Backtest {
    pub wallet: Wallet,
    pub liquidity_arr: LiquidityArray,
    pub strategy: Box<dyn Strategy>,
    pub start_info: StartInfo,
}

pub trait Strategy {
    fn initialize_strategy(&self) -> Vec<Action>;

    fn update(
        &mut self,
        liquidity_array: &LiquidityArray,
        transaction: TransactionModelFromDB,
    ) -> Vec<Action>;

    fn finalize_strategy(&self) -> Vec<Action>;
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
        // Initialize the cursor with the start_tx_id
        let mut cursor = Some(start_tx_id);

        // Init strategy
        let actions = self.strategy.initialize_strategy();

        self.execute_actions(actions)
            .map_err(|e| SyncError::Other(e.to_string()))?;

        while cursor.is_some() && cursor.unwrap() >= end_tx_id {
            let transactions = transaction_repo
                .fetch_transactions(pool_address, cursor, batch_size, OrderDirection::Descending)
                .await
                .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

            if transactions.is_empty() {
                break;
            }

            // Process transactions in reverse order (newest to oldest)
            for transaction in transactions.iter().rev() {
                match transaction.transaction_type.as_str() {
                    "IncreaseLiquidity" | "DecreaseLiquidity" => {
                        let liquidity_data = transaction
                            .data
                            .to_liquidity_data()
                            .map_err(|e| SyncError::ParseError(e.to_string()))?;

                        let is_increase =
                            transaction.transaction_type.as_str() == "IncreaseLiquidity";

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

                        self.liquidity_arr.update_liquidity(
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

                        self.liquidity_arr.simulate_swap(
                            U256::from(swap_data.amount_in),
                            swap_data.token_in == self.wallet.token_a_addr,
                        )?;
                    }
                    _ => {}
                }

                // Process strategy actions
                let actions = self
                    .strategy
                    .update(&self.liquidity_arr, transaction.clone());

                self.execute_actions(actions)
                    .map_err(|e| SyncError::Other(e.to_string()))?;
            }

            // Update cursor for the next iteration
            cursor = transactions.last().and_then(|t| {
                if t.tx_id > end_tx_id {
                    Some(t.tx_id - 1)
                } else {
                    None
                }
            });

            if transactions.len() < batch_size as usize {
                break;
            }
        }

        let actions = self.strategy.finalize_strategy();

        self.execute_actions(actions)
            .map_err(|e| SyncError::Other(e.to_string()))?;

        Ok(())
    }

    fn execute_actions(&mut self, actions: Vec<Action>) -> Result<(), BacktestError> {
        for action in actions {
            match action {
                Action::ClosePosition { position_id } => {
                    println!("Closing position and collecting fees");

                    // collect fees and remove position
                    let (fees_a, fees_b) = self.liquidity_arr.collect_fees(&position_id)?;
                    let position = self.liquidity_arr.remove_owners_position(&position_id)?;

                    println!("Fees in token_a: {}, fees in token_b: {}", fees_a, fees_b);

                    self.wallet.amount_a_fees_collected += fees_a;
                    self.wallet.amount_b_fees_collected += fees_b;

                    let (amount_a, amount_b) = calculate_amounts(
                        U256::from(position.liquidity),
                        self.liquidity_arr.current_sqrt_price,
                        tick_to_sqrt_price_u256(position.lower_tick),
                        tick_to_sqrt_price_u256(position.upper_tick),
                    );

                    self.wallet.amount_token_a += amount_a + fees_a;
                    self.wallet.amount_token_b += amount_b + fees_b;
                }
                Action::CreatePosition {
                    position_id,
                    lower_tick,
                    upper_tick,
                } => {
                    println!("Creating position before forward syncing.");

                    let amount_a = self.wallet.amount_token_a;
                    let amount_b = self.wallet.amount_token_b;

                    let current_tick = self.liquidity_arr.current_tick;
                    let upper_sqrt_price = tick_to_sqrt_price_u256(upper_tick);
                    let lower_sqrt_price = tick_to_sqrt_price_u256(lower_tick);
                    let curr_sqrt_price = self.liquidity_arr.current_sqrt_price;

                    // Since the tick might be anywhere in between lower and upper provided ticks from env, we need to rebalance.
                    // The ratio nmr represents how much % of assets should be in token_a. If ratio is 0.3, then 30% should be in token a. Since token a is on upper side of liquidity.
                    let rebalance_ratio = if current_tick >= upper_tick {
                        // All liquidity is in token B.
                        0.0
                    } else if current_tick <= lower_tick {
                        // All liquidity is in token A.
                        1.0
                    } else {
                        ((upper_sqrt_price - curr_sqrt_price).as_u128() as f64)
                            / ((upper_sqrt_price - lower_sqrt_price).as_u128() as f64)
                    };

                    // No need to use decimals since when using raw token amounts as below it sorts itself out.
                    let current_price =
                        (curr_sqrt_price.as_u128() as f64 / Q64.as_u128() as f64).powf(2.0);

                    let total_amount_a =
                        amount_a.as_u128() as f64 + amount_b.as_u128() as f64 / current_price;
                    let current_ratio = amount_a.as_u128() as f64 / total_amount_a;

                    let mut latest_amount_a_in_wallet = amount_a;
                    let mut latest_amount_b_in_wallet = amount_b;

                    // If price is closer to upper limit, we mainly provide liquidity in B. Therefore we need to sell more token A if its below current ratio.
                    if current_ratio > rebalance_ratio {
                        let hypothetical_amount_b =
                            ((1.0 - rebalance_ratio) * total_amount_a) * current_price;

                        let amount_a_needed_for_liquidity = if rebalance_ratio == 0.0 {
                            // Manual amount_a set to avoid overflow errors
                            // aka sell all amount a
                            U256::zero()
                        } else {
                            let liquidity_b = calculate_liquidity_b(
                                U256::from(hypothetical_amount_b as u128),
                                lower_sqrt_price,
                                curr_sqrt_price,
                            );

                            calculate_token_a_from_liquidity(
                                liquidity_b,
                                curr_sqrt_price,
                                upper_sqrt_price,
                            )
                        };

                        // sell whats unnecessary for liquidity
                        let amount_a_to_sell = amount_a - amount_a_needed_for_liquidity;

                        let amount_out =
                            self.liquidity_arr.simulate_swap(amount_a_to_sell, true)?;

                        latest_amount_a_in_wallet -= amount_a_to_sell;
                        latest_amount_b_in_wallet += amount_out;
                    } else {
                        // we have too little amount a and so we need to sell B for A.
                        let hypothetical_amount_a = rebalance_ratio * total_amount_a;

                        let amount_b_needed_for_liq = if rebalance_ratio == 1.0 {
                            // Manual amount_a set to avoid overflow errors
                            // aka sell all amount b
                            U256::zero()
                        } else {
                            let liquidity_a = calculate_liquidity_a(
                                U256::from(hypothetical_amount_a as u128),
                                curr_sqrt_price,
                                upper_sqrt_price,
                            );

                            calculate_token_b_from_liquidity(
                                liquidity_a,
                                curr_sqrt_price,
                                lower_sqrt_price,
                            )
                        };

                        let amount_b_to_sell = amount_b - amount_b_needed_for_liq;

                        let amount_out =
                            self.liquidity_arr.simulate_swap(amount_b_to_sell, false)?;

                        latest_amount_a_in_wallet += amount_out;
                        latest_amount_b_in_wallet -= amount_b_to_sell;
                    }

                    let newest_liquidity = calculate_liquidity(
                        latest_amount_a_in_wallet,
                        latest_amount_b_in_wallet,
                        curr_sqrt_price,
                        lower_sqrt_price,
                        upper_sqrt_price,
                    );

                    let (amount_a_provided_to_pool, amount_b_provided_to_pool) = calculate_amounts(
                        newest_liquidity,
                        curr_sqrt_price,
                        lower_sqrt_price,
                        upper_sqrt_price,
                    );

                    self.wallet.amount_token_a =
                        latest_amount_a_in_wallet - amount_a_provided_to_pool;
                    self.wallet.amount_token_b =
                        latest_amount_b_in_wallet - amount_b_provided_to_pool;

                    self.liquidity_arr.add_owners_position(
                        OwnersPosition {
                            owner: String::from(""),
                            lower_tick,
                            upper_tick,
                            liquidity: newest_liquidity.as_u128() as i128,
                            fee_growth_inside_a_last: U256::zero(),
                            fee_growth_inside_b_last: U256::zero(),
                        },
                        position_id,
                    );

                    println!(
                        "Created position with liquidity {}, amount_a LPed: {}, amount_b LPed: {}",
                        newest_liquidity, amount_a_provided_to_pool, amount_b_provided_to_pool
                    );
                    println!(
                        "Left in wallet - {} token A , {} token B",
                        self.wallet.amount_token_a, self.wallet.amount_token_b
                    );
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct MockStrategy;

    impl Strategy for MockStrategy {
        fn initialize_strategy(&self) -> Vec<Action> {
            vec![]
        }

        fn update(
            &mut self,
            _liquidity_array: &LiquidityArray,
            _transaction: TransactionModelFromDB,
        ) -> Vec<Action> {
            vec![]
        }

        fn finalize_strategy(&self) -> Vec<Action> {
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
            // amount_a: U256::from(500_000_000_u128),
            // amount_b: U256::from(500_000_000),
        };

        backtest.execute_actions(vec![action]).unwrap();

        assert_eq!(backtest.wallet.amount_token_a, U256::from(500_000_000));
        assert_eq!(backtest.wallet.amount_token_b, U256::from(500_000_000));

        assert!(backtest
            .liquidity_arr
            .positions
            .contains_key("test_position"));
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
                // amount_a: starting_amount_a,
                // amount_b: starting_amount_b,
            },
            // Action::Swap {
            //     amount_in: U256::from(7_000_000),
            //     is_sell: true,
            // },
            // Action::Swap {
            //     amount_in: U256::from(10_000_000_i128),
            //     is_sell: false,
            // },
            // Action::Swap {
            //     amount_in: U256::from(3_000_000),
            //     is_sell: true,
            // },
            Action::ClosePosition {
                position_id: "test_position".to_string(),
            },
        ];

        backtest.execute_actions(all_actions).unwrap();

        // Assertions
        assert!(
            backtest.wallet.amount_a_fees_collected > U256::zero(),
            "Should have collected some fees for token A"
        );
        assert!(
            backtest.wallet.amount_b_fees_collected > U256::zero(),
            "Should have collected some fees for token B"
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
                // amount_a: starting_amount_a,
                // amount_b: starting_amount_b,
            },
            // Action::Swap {
            //     amount_in: U256::from(800_000_000_i128),
            //     is_sell: false,
            // },
            Action::ClosePosition {
                position_id: "test_position".to_string(),
            },
        ];

        backtest.execute_actions(all_actions).unwrap();

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
                // amount_a: starting_amount_a,
                // amount_b: starting_amount_b,
            },
            // Action::Swap {
            //     amount_in: U256::from(600_000_000_i128),
            //     is_sell: true, // SELLING TOKEN A
            // },
            Action::ClosePosition {
                position_id: "test_position".to_string(),
            },
        ];

        backtest.execute_actions(all_actions).unwrap();

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
    }
}
