use anyhow::Result;

use crate::{
    models::transactions_model::TransactionModelFromDB,
    repositories::transactions_repo::{OrderDirection, TransactionRepoTrait},
    utils::{
        error::{BacktestError, SyncError},
        price_calcs::{
            calculate_amounts, calculate_correct_liquidity, calculate_rebalance_amount,
            sqrt_price_to_fixed, tick_to_sqrt_price, Q32,
        },
    },
};

use super::liquidity_array::{LiquidityArray, OwnersPosition, TickData};

pub struct StartInfo {
    pub token_a_amount: u128,
    pub token_b_amount: u128,
}

pub struct Wallet {
    pub token_a_addr: String,
    pub token_b_addr: String,
    pub amount_token_a: u128,
    pub amount_token_b: u128,
    pub token_a_decimals: i16,
    pub token_b_decimals: i16,
    pub amount_a_fees_collected: u128,
    pub amount_b_fees_collected: u128,
    pub total_profit: f64,
    pub total_profit_pct: f64,
}

pub enum Action {
    ProvideLiquidity {
        position_id: i32,
        liquidity_to_add: u128,
    },
    RemoveLiquidity {
        position_id: i32,
        liquidity_to_remove: u128,
    },
    Swap {
        amount_in: u128,
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
        amount_a: u128,
        amount_b: u128,
    },
    FinalizeStrategy {
        position_id: String,
        starting_sqrt_price: u128,
    },
}

pub struct Backtest {
    pub wallet: Wallet,
    pub liquidity_arr: LiquidityArray,
    pub strategy: Box<dyn Strategy>,
    pub start_info: StartInfo,
}

pub trait Strategy {
    fn initialize_strategy(&self, amount_a: u128, amount_b: u128) -> Vec<Action>;

    fn update(
        &mut self,
        liquidity_array: &LiquidityArray,
        transaction: TransactionModelFromDB,
    ) -> Vec<Action>;

    fn finalize_strategy(&self, starting_sqrt_price: u128) -> Vec<Action>;
}

impl Backtest {
    pub fn new(
        amount_a: u128,
        amount_b: u128,
        liquidity_arr: LiquidityArray,
        wallet_state: Wallet,
        strategy: Box<dyn Strategy>,
    ) -> Self {
        Self {
            start_info: StartInfo {
                token_a_amount: amount_a * 10u128.pow(wallet_state.token_a_decimals as u32),
                token_b_amount: amount_b * 10u128.pow(wallet_state.token_b_decimals as u32),
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

                        let tick_data = TickData {
                            lower_tick: liquidity_data.tick_lower.unwrap(),
                            upper_tick: liquidity_data.tick_upper.unwrap(),
                            liquidity: liquidity_data.liquidity_amount.parse::<u128>().unwrap(),
                        };

                        let is_increase =
                            transaction.transaction_type.as_str() == "IncreaseLiquidity";

                        self.liquidity_arr
                            .update_liquidity_from_tx(tick_data, is_increase);
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
                            .simulate_swap_with_fees(adjusted_amount, is_sell)?;
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
                let _ = self
                    .liquidity_arr
                    .simulate_swap_with_fees(amount_in, is_sell);

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
                    position.liquidity,
                    self.liquidity_arr.current_sqrt_price,
                    sqrt_price_to_fixed(tick_to_sqrt_price(position.lower_tick)),
                    sqrt_price_to_fixed(tick_to_sqrt_price(position.upper_tick)),
                );
                let mut amount_a_with_fees = amount_a + fees_a;
                let mut amount_b_with_fees = amount_b + fees_b;

                let (amount_to_sell, is_sell) = calculate_rebalance_amount(
                    amount_a_with_fees,
                    amount_b_with_fees,
                    self.liquidity_arr.current_sqrt_price,
                    (rebalance_ratio * Q32 as f64) as u128,
                );

                // TODO: Optimize for small imbalances to avoid unnecessary swaps. Also add slippage later.
                // For now, we'll proceed with the swap even for small amounts
                let (_, _, amount_out, fees) = self
                    .liquidity_arr
                    .simulate_swap_with_fees(amount_to_sell, is_sell)?;

                if is_sell {
                    amount_a_with_fees -= amount_to_sell;
                    amount_b_with_fees += amount_out;
                    self.wallet.amount_a_fees_collected += fees;
                } else {
                    amount_a_with_fees -= amount_to_sell;
                    amount_b_with_fees += amount_out;
                    self.wallet.amount_b_fees_collected += fees;
                }

                let new_liquidity = calculate_correct_liquidity(
                    amount_a_with_fees,
                    amount_b_with_fees,
                    self.liquidity_arr.current_sqrt_price,
                    sqrt_price_to_fixed(tick_to_sqrt_price(new_lower_tick)),
                    sqrt_price_to_fixed(tick_to_sqrt_price(new_upper_tick)),
                );

                // Re-provide liquidity with the rebalanced amounts
                self.liquidity_arr.add_owners_position(
                    OwnersPosition {
                        owner: position.owner,
                        lower_tick: new_lower_tick,
                        upper_tick: new_upper_tick,
                        liquidity: new_liquidity,
                        fees_owed_a: 0,
                        fees_owed_b: 0,
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

                self.liquidity_arr.add_owners_position(
                    OwnersPosition {
                        owner: String::from(""),
                        lower_tick,
                        upper_tick,
                        liquidity: calculate_correct_liquidity(
                            amount_a,
                            amount_b,
                            self.liquidity_arr.current_sqrt_price,
                            sqrt_price_to_fixed(tick_to_sqrt_price(lower_tick)),
                            sqrt_price_to_fixed(tick_to_sqrt_price(upper_tick)),
                        ),
                        fees_owed_a: 0,
                        fees_owed_b: 0,
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
                let position = self.liquidity_arr.remove_owners_position(&position_id)?;

                self.wallet.amount_a_fees_collected += position.fees_owed_a;
                self.wallet.amount_b_fees_collected += position.fees_owed_b;

                let (amount_a, amount_b) = calculate_amounts(
                    position.liquidity,
                    self.liquidity_arr.current_sqrt_price,
                    sqrt_price_to_fixed(tick_to_sqrt_price(position.lower_tick)),
                    sqrt_price_to_fixed(tick_to_sqrt_price(position.upper_tick)),
                );

                self.wallet.amount_token_a += amount_a;
                self.wallet.amount_token_b += amount_b;

                let initial_value_a = self.start_info.token_a_amount as f64
                    + (self.start_info.token_b_amount as f64
                        * (starting_sqrt_price as f64 / Q32 as f64).powi(2));

                // Calculate the final value in terms of token A
                let final_value_a = (self.wallet.amount_token_a
                    + self.wallet.amount_a_fees_collected)
                    as f64
                    + ((self.wallet.amount_token_b + self.wallet.amount_b_fees_collected) as f64
                        * (self.liquidity_arr.current_sqrt_price as f64 / Q32 as f64).powi(2));

                self.wallet.total_profit = final_value_a - initial_value_a;
                self.wallet.total_profit_pct = (self.wallet.total_profit / initial_value_a) * 100.0;

                Ok(())
            }
        }
    }
}
