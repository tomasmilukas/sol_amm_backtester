use crate::{models::transactions_model::TransactionModelFromDB, utils::price_calcs::U256};

use super::{
    backtester::{Action, Strategy},
    liquidity_array::LiquidityArray,
};

pub struct NoRebalanceStrategy {
    lower_tick: i32,
    upper_tick: i32,
}

impl NoRebalanceStrategy {
    pub fn new(lower_tick: i32, upper_tick: i32) -> Self {
        Self {
            lower_tick,
            upper_tick,
        }
    }
}

impl Strategy for NoRebalanceStrategy {
    fn initialize_strategy(&self, amount_a: U256, amount_b: U256) -> Vec<Action> {
        vec![Action::CreatePosition {
            position_id: String::from("no_rebalance"),
            lower_tick: self.lower_tick,
            upper_tick: self.upper_tick,
            amount_a,
            amount_b,
        }]
    }

    fn update(
        &mut self,
        liquidity_array: &LiquidityArray,
        transaction: TransactionModelFromDB,
    ) -> Vec<Action> {
        vec![]
    }

    fn finalize_strategy(&self, starting_sqrt_price: U256) -> Vec<Action> {
        vec![Action::FinalizeStrategy {
            position_id: String::from("no_rebalance"),
            starting_sqrt_price,
        }]
    }
}
