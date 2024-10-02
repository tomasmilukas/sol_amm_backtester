use crate::models::transactions_model::TransactionModelFromDB;

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
    fn initialize_strategy(&self) -> Vec<Action> {
        vec![Action::CreatePosition {
            position_id: String::from("no_rebalance"),
            lower_tick: self.lower_tick,
            upper_tick: self.upper_tick,
        }]
    }

    fn update(
        &mut self,
        liquidity_array: &LiquidityArray,
        transaction: TransactionModelFromDB,
    ) -> Vec<Action> {
        vec![]
    }

    fn finalize_strategy(&self) -> Vec<Action> {
        vec![Action::ClosePosition {
            position_id: String::from("no_rebalance"),
        }]
    }
}
