use anyhow::Result;

use crate::{
    models::transactions_model::TransactionModelFromDB,
    utils::price_calcs::{
        calculate_amounts, calculate_liquidity_for_amount_a, calculate_liquidity_for_amount_b,
        calculate_rebalance_amount, sqrt_price_to_fixed, tick_to_sqrt_price, Q32,
    },
};

use super::liquidity_array::{LiquidityArray, OwnersPosition, TickData};

pub struct Wallet {
    token_a_addr: String,
    token_b_addr: String,
    amount_token_a: u128,
    amount_token_b: u128,
    token_a_decimals: i16,
    token_b_decimals: i16,
    impermanent_loss: f64,
    amount_a_fees_collected: u128,
    amount_b_fees_collected: u128,
    total_profit: f64,
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
    },
    CreatePosition {
        position_id: String,
        lower_tick: i32,
        upper_tick: i32,
        amount_a: u128,
        amount_b: u128,
    },
}

pub struct Backtest {
    wallet: Wallet,
    liquidity_arr: LiquidityArray,
    strategy: Box<dyn Strategy>,
}

pub trait Strategy {
    fn initialize_strategy(&self, amount_a: u128, amount_b: u128) -> Vec<Action>;

    fn update(
        &mut self,
        liquidity_array: &LiquidityArray,
        transaction: TransactionModelFromDB,
    ) -> Vec<Action>;
}

impl Backtest {
    pub fn new(
        liquidity_arr: LiquidityArray,
        wallet_state: Wallet,
        strategy: Box<dyn Strategy>,
    ) -> Self {
        Self {
            liquidity_arr,
            wallet: wallet_state,
            strategy,
        }
    }

    pub fn process_transaction(&mut self, transaction: TransactionModelFromDB) {
        match transaction.transaction_type.as_str() {
            "IncreaseLiquidity" | "DecreaseLiquidity" => {
                let liquidity_data = transaction.data.to_liquidity_data().unwrap();

                let tick_data = TickData {
                    lower_tick: liquidity_data.tick_lower.unwrap(),
                    upper_tick: liquidity_data.tick_upper.unwrap(),
                    liquidity: liquidity_data.liquidity_amount.parse::<u128>().unwrap(),
                };

                let is_increase = transaction.transaction_type.as_str() == "IncreaseLiquidity";

                self.liquidity_arr
                    .update_liquidity_from_tx(tick_data, is_increase);
            }
            "Swap" => {
                let swap_data = transaction.data.to_swap_data().unwrap();

                let is_sell = swap_data.token_in == self.wallet.token_a_addr;

                self.liquidity_arr
                    .simulate_swap_with_fees((swap_data.amount_in) as u128, is_sell);
            }
            _ => {}
        }

        let actions = self
            .strategy
            .update(&self.liquidity_arr, transaction.clone());
        for action in actions {
            self.execute_action(action);
        }
    }

    fn execute_action(&mut self, action: Action) -> Result<()> {
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

                // rebalance to 50/50
                let (amount_to_sell, is_sell) = calculate_rebalance_amount(
                    amount_a,
                    amount_b,
                    self.liquidity_arr.current_sqrt_price,
                    (rebalance_ratio * Q32 as f64) as u128,
                );

                let (_start_tick, _end_tick, amount_out, _fees) = self
                    .liquidity_arr
                    .simulate_swap_with_fees(amount_to_sell, is_sell)
                    .unwrap();

                Ok(())
            }
            Action::CreatePosition {
                position_id,
                lower_tick,
                upper_tick,
                amount_a,
                amount_b,
            } => {
                let liquidity_a = calculate_liquidity_for_amount_a(
                    amount_a,
                    self.liquidity_arr.current_sqrt_price,
                    sqrt_price_to_fixed(tick_to_sqrt_price(lower_tick)),
                );
                let liquidity_b = calculate_liquidity_for_amount_b(
                    amount_b,
                    self.liquidity_arr.current_sqrt_price,
                    sqrt_price_to_fixed(tick_to_sqrt_price(upper_tick)),
                );

                let _ = self.liquidity_arr.add_owners_position(
                    OwnersPosition {
                        owner: String::from(""),
                        lower_tick,
                        upper_tick,
                        liquidity: liquidity_a.min(liquidity_b),
                        fees_owed_a: 0,
                        fees_owed_b: 0,
                    },
                    position_id,
                );

                Ok(())
            }
        }
    }
}
