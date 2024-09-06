use crate::models::transactions_model::TransactionModelFromDB;

use super::liquidity_array::{LiquidityArray, TickData};

pub struct Wallet {
    token_a_addr: String,
    token_b_addr: String,
    amount_token_a: f64,
    amount_token_b: f64,
    impermanent_loss: f64,
    liquidity_fees_collected: f64,
    total_profit: f64,
}

pub enum Action {
    ProvideLiquidity {
        lower_tick: i32,
        upper_tick: i32,
        amount: u128,
    },
    RemoveLiquidity {
        lower_tick: i32,
        upper_tick: i32,
        amount: u128,
    },
    Swap {
        amount_in: i32,
        token_in: String,
    },
}

pub struct Backtest {
    wallet: Wallet,
    liquidity_arr: LiquidityArray,
    strategy: Box<dyn Strategy>,
}

pub trait Strategy {
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

    fn execute_action(&mut self, action: Action) {
        match action {
            Action::ProvideLiquidity {
                lower_tick,
                upper_tick,
                amount,
            } => {
                todo!();
            }
            Action::RemoveLiquidity {
                lower_tick,
                upper_tick,
                amount,
            } => {
                todo!();
            }
            Action::Swap {
                amount_in,
                token_in,
            } => {
                println!("Swapping");
            }
        }
    }
}
