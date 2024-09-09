use crate::models::transactions_model::TransactionModelFromDB;

use super::{
    backtester::{Action, Strategy},
    liquidity_array::LiquidityArray,
};

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
    fn initialize_strategy(&self, amount_a: u128, amount_b: u128) -> Vec<Action> {
        vec![Action::CreatePosition {
            position_id: String::from("simple_rebalance"),
            lower_tick: self.current_lower_tick - self.range / 2,
            upper_tick: self.current_lower_tick + self.range / 2,
            amount_a,
            amount_b,
        }]
    }

    fn update(
        &mut self,
        liquidity_array: &LiquidityArray,
        transaction: TransactionModelFromDB,
    ) -> Vec<Action> {
        match transaction.transaction_type.as_str() {
            "Swap" => {
                let current_tick = liquidity_array.current_tick;

                if current_tick < self.current_lower_tick || current_tick > self.current_upper_tick
                {
                    let actions = vec![Action::Rebalance {
                        position_id: String::from("simple_rebalance"),
                        rebalance_ratio: 0.5,
                        new_lower_tick: current_tick - self.range / 2,
                        new_upper_tick: current_tick + self.range / 2,
                    }];

                    return actions;
                }

                vec![]
            }
            _ => {
                vec![]
            }
        }
    }
}
