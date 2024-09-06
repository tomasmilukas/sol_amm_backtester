
use crate::models::transactions_model::TransactionModelFromDB;

use super::{backtester::{Action, Strategy}, liquidity_array::LiquidityArray};

pub struct SimpleRebalanceStrategy {
    current_lower_tick: i32,
    current_upper_tick: i32,
    range: i32,
}

impl SimpleRebalanceStrategy {
    pub fn new(initial_tick: i32, range: i32) -> Self {
        Self {
            current_lower_tick: initial_tick - range / 2,
            current_upper_tick: initial_tick + range / 2,
            range,
        }
    }
}

impl Strategy for SimpleRebalanceStrategy {
    fn update(
        &mut self,
        liquidity_array: &LiquidityArray,
        transaction: TransactionModelFromDB,
    ) -> Vec<Action> {
        match transaction.transaction_type.as_str() {
            "IncreaseLiquidity" | "DecreaseLiquidity" => {
                vec![]
            }
            "Swap" => {
                // strategy to adjust for when we cross a condition like the current tick. bad example below:
                //    if pool.current_tick < self.current_lower_tick || pool.current_tick > self.current_upper_tick {
                //     let new_lower_tick = pool.current_tick - self.range / 2;
                //     let new_upper_tick = pool.current_tick + self.range / 2;

                //     let actions = vec![
                //         Action::RemoveLiquidity {
                //             lower_tick: self.current_lower_tick,
                //             upper_tick: self.current_upper_tick,
                //             amount: 0, // Remove all
                //         },
                //         Action::ProvideLiquidity {
                //             lower_tick: new_lower_tick,
                //             upper_tick: new_upper_tick,
                //             amount: std::cmp::min(pool.token_a_reserve, pool.token_b_reserve) / 2,
                //         },
                //     ];

                //     self.current_lower_tick = new_lower_tick;
                //     self.current_upper_tick = new_upper_tick;

                //     actions
                vec![]
            }
            _ => {
                vec![]
            }
        }
    }
}
