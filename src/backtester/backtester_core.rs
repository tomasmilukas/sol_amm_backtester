use anyhow::Result;
use serde_json::json;

use crate::{
    backtester::backtest_utils::{
        calculate_amount_a_needed_for_liquidity, calculate_amount_b_needed_for_liquidity,
        calculate_rebalance_ratio,
    },
    models::transactions_model::{SwapData, TransactionModelFromDB},
    repositories::transactions_repo::{OrderDirection, TransactionRepoTrait},
    utils::{
        core_math::{calculate_amounts, calculate_liquidity, tick_to_sqrt_price_u256, Q64, U256},
        data_logger::DataLogger,
        error::{BacktestError, SyncError},
    },
};

use super::liquidity_array::{LiquidityArray, OwnersPosition};

pub struct StartInfo {
    pub token_a_amount: U256,
    pub token_b_amount: U256,
}

pub struct SwappingData {
    pub current_swap_nmr: u128,
    pub current_token_a_volume: u128,
    pub current_token_b_volume: u128,
    pub swap_nmr_in_position: u128,
    pub token_a_volume_in_position: u128,
    pub token_b_volume_in_position: u128,
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
    pub data_logger: DataLogger,
    pub data: SwappingData,
}

pub trait Strategy {
    fn initialize_strategy(&self) -> Vec<Action>;

    fn update(
        &mut self,
        liquidity_array: &LiquidityArray,
        transaction: TransactionModelFromDB,
    ) -> Vec<Action>;

    fn finalize_strategy(&self) -> Vec<Action>;

    fn get_ticks(&self) -> (i32, i32);
}

// both divided by 10^6
const SLIPPAGE_FOR_SWAP: i32 = 10000;

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
            data_logger: DataLogger::new(),
            data: SwappingData {
                current_swap_nmr: 0,
                current_token_a_volume: 0,
                current_token_b_volume: 0,
                swap_nmr_in_position: 0,
                token_a_volume_in_position: 0,
                token_b_volume_in_position: 0,
            },
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

                        let is_sell = swap_data.token_in == self.wallet.token_a_addr;

                        self.save_data(transaction, swap_data, is_sell);

                        self.liquidity_arr
                            .simulate_swap(U256::from(swap_data.amount_in), is_sell)?;
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

    // saving data for later analysis
    fn save_data(
        &mut self,
        transaction: &TransactionModelFromDB,
        swap_data: &SwapData,
        is_sell: bool,
    ) {
        self.liquidity_arr.current_block_time = transaction.block_time;
        self.data.current_swap_nmr += 1;

        let token_a_volume = if is_sell {
            swap_data.amount_in / 10_i32.pow(self.wallet.token_a_decimals as u32) as u64
        } else {
            0
        };

        let token_b_volume = if is_sell {
            0
        } else {
            swap_data.amount_in / 10_i32.pow(self.wallet.token_b_decimals as u32) as u64
        };

        self.data.current_token_a_volume += token_a_volume as u128;
        self.data.current_token_b_volume += token_b_volume as u128;

        let (lower_tick, upper_tick) = self.strategy.get_ticks();

        let within_position_range = self.liquidity_arr.current_tick >= lower_tick
            && self.liquidity_arr.current_tick <= upper_tick;

        if within_position_range {
            self.data.swap_nmr_in_position += 1;
            self.data.token_a_volume_in_position += token_a_volume as u128;
            self.data.token_b_volume_in_position += token_b_volume as u128;
        }
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

                    self.data_logger.log_close_position(
                        position_id,
                        position.lower_tick,
                        position.upper_tick,
                        self.liquidity_arr.current_tick,
                        self.wallet.amount_token_a.as_u128(),
                        self.wallet.amount_token_b.as_u128(),
                        amount_a.as_u128(),
                        amount_b.as_u128(),
                        fees_a.as_u128(),
                        fees_b.as_u128(),
                        self.liquidity_arr.current_block_time as u128,
                        self.data.current_swap_nmr,
                        self.data.current_token_a_volume,
                        self.data.current_token_b_volume,
                        self.data.swap_nmr_in_position,
                        self.data.token_a_volume_in_position,
                        self.data.token_b_volume_in_position,
                    );
                }
                Action::CreatePosition {
                    position_id,
                    lower_tick,
                    upper_tick,
                } => {
                    let amount_a = self.wallet.amount_token_a;
                    let amount_b = self.wallet.amount_token_b;

                    let upper_sqrt_price = tick_to_sqrt_price_u256(upper_tick);
                    let lower_sqrt_price = tick_to_sqrt_price_u256(lower_tick);
                    let curr_sqrt_price = self.liquidity_arr.current_sqrt_price;

                    let rebalance_ratio = calculate_rebalance_ratio(
                        curr_sqrt_price,
                        upper_sqrt_price,
                        lower_sqrt_price,
                    );

                    // No need to use decimals since when using raw token amounts as below it sorts itself out.
                    let current_price =
                        (curr_sqrt_price.as_u128() as f64 / Q64.as_u128() as f64).powf(2.0);

                    let total_amount_a =
                        amount_a.as_u128() as f64 + amount_b.as_u128() as f64 / current_price;
                    let current_ratio = amount_a.as_u128() as f64 / total_amount_a;

                    let mut latest_amount_a_in_wallet = amount_a;
                    let mut latest_amount_b_in_wallet = amount_b;

                    // In case the amounts are very close, dont swap.
                    let no_swap_tolerance = (current_ratio - rebalance_ratio).abs() < 0.05;

                    // If price is closer to upper limit, we mainly provide liquidity in B. Therefore we need to sell more token A if its below current ratio.
                    if current_ratio > rebalance_ratio && !no_swap_tolerance {
                        let amount_a_needed_for_liquidity = calculate_amount_a_needed_for_liquidity(
                            rebalance_ratio,
                            total_amount_a,
                            current_price,
                            lower_sqrt_price,
                            curr_sqrt_price,
                            upper_sqrt_price,
                        );

                        // sell whats unnecessary for liquidity
                        let amount_a_to_sell = if amount_a_needed_for_liquidity >= amount_a {
                            amount_a
                        } else {
                            amount_a - amount_a_needed_for_liquidity
                        };

                        let amount_out =
                            self.liquidity_arr.simulate_swap(amount_a_to_sell, true)?;

                        let amount_out_after_slippage = (amount_out
                            * U256::from(1_000_000 - SLIPPAGE_FOR_SWAP))
                            / U256::from(1_000_000);

                        latest_amount_a_in_wallet -= amount_a_to_sell;
                        latest_amount_b_in_wallet += amount_out_after_slippage;
                    } else if !no_swap_tolerance {
                        let amount_b_needed_for_liq = calculate_amount_b_needed_for_liquidity(
                            rebalance_ratio,
                            total_amount_a,
                            current_price,
                            lower_sqrt_price,
                            curr_sqrt_price,
                            upper_sqrt_price,
                        );

                        let amount_b_to_sell = if amount_b_needed_for_liq >= amount_b {
                            amount_b
                        } else {
                            amount_b - amount_b_needed_for_liq
                        };

                        let amount_out =
                            self.liquidity_arr.simulate_swap(amount_b_to_sell, false)?;

                        let amount_out_after_slippage = (amount_out
                            * U256::from(1_000_000 - SLIPPAGE_FOR_SWAP))
                            / U256::from(1_000_000);

                        latest_amount_a_in_wallet += amount_out_after_slippage;
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
                        position_id.clone(),
                    );

                    println!(
                        "Created position with liquidity {}, amount_a LPed: {}, amount_b LPed: {}, lower tick: {}, upper tick: {}",
                        newest_liquidity, amount_a_provided_to_pool, amount_b_provided_to_pool,lower_tick,upper_tick
                    );
                    println!(
                        "Left in wallet - {} token A , {} token B",
                        self.wallet.amount_token_a, self.wallet.amount_token_b
                    );

                    self.data_logger.log_create_position(
                        position_id,
                        lower_tick,
                        upper_tick,
                        self.liquidity_arr.current_tick,
                        self.wallet.amount_token_a.as_u128(),
                        self.wallet.amount_token_b.as_u128(),
                        amount_a_provided_to_pool.as_u128(),
                        amount_b_provided_to_pool.as_u128(),
                        newest_liquidity.as_u128(),
                        self.liquidity_arr.current_block_time as u128,
                        self.data.current_swap_nmr,
                        self.data.current_token_a_volume,
                        self.data.current_token_b_volume,
                        self.liquidity_arr.active_liquidity.as_u128(),
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

        fn get_ticks(&self) -> (i32, i32) {
            (0_i32, 0_i32)
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

    fn create_test_liquidity_array(current_tick: i32) -> LiquidityArray {
        let mut liquidity_arr = LiquidityArray::new(-500_000, 500_000, 10, 500);

        liquidity_arr.current_tick = current_tick;
        liquidity_arr.current_sqrt_price = tick_to_sqrt_price_u256(current_tick);

        liquidity_arr.update_liquidity(-100_000, 100_000, 1_000_000_000, true);
        liquidity_arr.update_liquidity(-200_000, 200_000, 1_000_000_000, true);

        liquidity_arr
    }

    #[tokio::test]
    async fn test_backtest_initialization() {
        let liquidity_arr = create_test_liquidity_array(0);

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
            create_test_liquidity_array(current_tick),
            wallet,
            strategy,
        );

        backtest
            .execute_actions(vec![Action::CreatePosition {
                position_id: "test_position".to_string(),
                lower_tick,
                upper_tick,
            }])
            .unwrap();

        // manually set cached ticks
        let (upper_tick_data, lower_tick_data) = backtest
            .liquidity_arr
            .get_upper_and_lower_ticks(current_tick, true)
            .unwrap();

        backtest.liquidity_arr.cached_lower_initialized_tick = Some(lower_tick_data.tick);
        backtest.liquidity_arr.cached_upper_initialized_tick = Some(upper_tick_data.tick);

        let _ = backtest
            .liquidity_arr
            .simulate_swap(U256::from(7_000_000), true);

        let _ = backtest
            .liquidity_arr
            .simulate_swap(U256::from(10_000_000_i128), false);

        let _ = backtest
            .liquidity_arr
            .simulate_swap(U256::from(3_000_000), true);

        backtest
            .execute_actions(vec![Action::ClosePosition {
                position_id: "test_position".to_string(),
            }])
            .unwrap();

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
            create_test_liquidity_array(current_tick),
            wallet,
            strategy,
        );

        backtest
            .execute_actions(vec![Action::CreatePosition {
                position_id: "test_position".to_string(),
                lower_tick,
                upper_tick,
            }])
            .unwrap();

        // manually set cached ticks
        let (upper_tick_data, lower_tick_data) = backtest
            .liquidity_arr
            .get_upper_and_lower_ticks(current_tick, true)
            .unwrap();

        backtest.liquidity_arr.cached_lower_initialized_tick = Some(lower_tick_data.tick);
        backtest.liquidity_arr.cached_upper_initialized_tick = Some(upper_tick_data.tick);

        let _ = backtest
            .liquidity_arr
            .simulate_swap(U256::from(8_000_000_000_000_i128), false);

        backtest
            .execute_actions(vec![Action::ClosePosition {
                position_id: "test_position".to_string(),
            }])
            .unwrap();

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
            backtest.wallet.amount_token_a == U256::from(109935),
            "Token A wiped out, this was remainder for when providing liq, since not all tokens get used up"
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
            create_test_liquidity_array(current_tick),
            wallet,
            strategy,
        );

        backtest
            .execute_actions(vec![Action::CreatePosition {
                position_id: "test_position".to_string(),
                lower_tick,
                upper_tick,
            }])
            .unwrap();

        // manually set cached ticks
        let (upper_tick_data, lower_tick_data) = backtest
            .liquidity_arr
            .get_upper_and_lower_ticks(current_tick, true)
            .unwrap();

        backtest.liquidity_arr.cached_lower_initialized_tick = Some(lower_tick_data.tick);
        backtest.liquidity_arr.cached_upper_initialized_tick = Some(upper_tick_data.tick);

        let _ = backtest
            .liquidity_arr
            .simulate_swap(U256::from(8_000_000_000_000_i128), true);

        backtest
            .execute_actions(vec![Action::ClosePosition {
                position_id: "test_position".to_string(),
            }])
            .unwrap();

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
            backtest.wallet.amount_token_b == U256::from(1),
            "Wiped out token B liquidity, remainder from when providing liq"
        );
        assert!(
            backtest.wallet.amount_token_a > starting_amount_a,
            "All liquidity in token A, above starting"
        );
    }
}
