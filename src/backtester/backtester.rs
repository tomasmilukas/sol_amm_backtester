use crate::models::transactions_model::TransactionModelFromDB;

use super::liquidity_array::{LiquidityArray, OwnersPosition, TickData};

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
        position_id: i32,
        liquidity_to_add: u128,
    },
    RemoveLiquidity {
        position_id: i32,
        liquidity_to_remove: u128,
    },
    Swap {
        amount_in: i32,
        token_in: String,
    },
    // Rebalance will take the OwnersPosition, remove the liquidity, swap to get 50/50 in token_a/token_b and then provide liquidity.
    Rebalance {
        position_id: String,
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

    fn execute_action(&mut self, action: Action) {
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
            Action::Swap {
                amount_in,
                token_in,
            } => {
                println!("Swapping");
            }
            Action::Rebalance { position_id } => {
                println!("Swapping");
            }
            Action::CreatePosition {
                position_id,
                lower_tick,
                upper_tick,
                amount_a,
                amount_b,
            } => {
                let liquidity = 0;

                self.liquidity_arr.add_owners_position(
                    OwnersPosition {
                        owner: String::from(""),
                        lower_tick,
                        upper_tick,
                        liquidity,
                        fees_owed_a: 0,
                        fees_owed_b: 0,
                    },
                    position_id,
                )
            }
        }
    }
}
