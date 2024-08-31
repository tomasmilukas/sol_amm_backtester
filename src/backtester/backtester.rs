use chrono::{DateTime, Utc};

use crate::{
    models::transactions_model::{TransactionData, TransactionModelFromDB},
    repositories::transactions_repo::TransactionRepo,
};

use super::liquidity_range_calculator::TickData;

struct PoolInfo {
    fee_rate: i32,
    token_a: String,
    token_b: String,
}

struct PoolState {
    current_tick: i32,
    liquidity: f64,
    tick_spacing: i32,
    liquidity_range: Vec<TickData>,
}

struct PositionState {
    liquidity: f64,
    lower_tick: i32,
    upper_tick: i32,
}

struct WalletState {
    token_a: f64,
    token_b: f64,
}

struct BacktestState {
    pool: PoolState,
    position: Option<PositionState>,
    wallet: WalletState,
}

enum EventType {
    Swap,
    AddLiquidity,
    RemoveLiquidity,
}

enum Action {
    Unstake {
        amount: f64,
        fees_earned: f64,
    },
    Stake {
        amount: f64,
        lower_tick: i32,
        upper_tick: i32,
    },
    Swap {
        token_sell: String,
        amount_sell: f64,
        token_buy: String,
    },
    DoNothing,
}

struct Backtester<S: Strategy> {
    state: BacktestState,
    strategy: S,
    transaction_repo: TransactionRepo,
    total_fees_a: f64,
    total_fees_b: f64,
}

pub trait Strategy {
    fn process_event(&mut self, event: &TransactionModelFromDB) -> Vec<Action>;
}

impl<S: Strategy> Backtester<S> {
    fn new(initial_state: BacktestState, strategy: S, transaction_repo: TransactionRepo) -> Self {
        Self {
            state: initial_state,
            strategy,
            transaction_repo,
            total_fees_a: 0.0,
            total_fees_b: 0.0,
        }
    }

    fn process_transaction(&mut self, transaction: &TransactionModelFromDB) {
        match transaction.transaction_type.as_str() {
            "Swap" => {
                if let TransactionData::Swap(swap_data) = &transaction.data {
                    // self.process_swap(swap_data, transaction.block_time_utc);
                } else {
                    println!("Error: Mismatch between transaction_type and data");
                }
            }
            "IncreaseLiquidity" => {
                if let TransactionData::IncreaseLiquidity(liquidity_data) = &transaction.data {
                    // self.process_increase_liquidity(liquidity_data, transaction.block_time_utc);
                } else {
                    println!("Error: Mismatch between transaction_type and data");
                }
            }
            "DecreaseLiquidity" => {
                if let TransactionData::DecreaseLiquidity(liquidity_data) = &transaction.data {
                    // self.process_decrease_liquidity(liquidity_data, transaction.block_time_utc);
                } else {
                    println!("Error: Mismatch between transaction_type and data");
                }
            }
            _ => println!("Unknown transaction type: {}", transaction.transaction_type),
        }

        let actions = self.strategy.process_event(transaction);

        for action in actions {
            self.execute_action(action);
        }

        self.update_fees();
    }

    fn process_swap(&mut self, transaction: &TransactionModelFromDB) {
        // Implement swap logic, including price impact and fee collection
        // let fees_a = transaction.amount_a * self.state.pool.fee_rate;
        // let fees_b = transaction.amount_b * self.state.pool.fee_rate;
        // self.state.pool.token_a += transaction.amount_a - fees_a;
        // self.state.pool.token_b += transaction.amount_b - fees_b;
        // self.state.pool.fee_growth_global_a += fees_a / self.state.pool.liquidity;
        // self.state.pool.fee_growth_global_b += fees_b / self.state.pool.liquidity;
        // Update sqrt_price and current_tick based on the swap
    }

    fn process_add_liquidity(&mut self, transaction: &TransactionModelFromDB) {
        // Implement add liquidity logic
    }

    fn process_remove_liquidity(&mut self, transaction: &TransactionModelFromDB) {
        // Implement remove liquidity logic
    }

    fn update_fees(&mut self) {
        if let Some(position) = &mut self.state.position {}
    }

    fn execute_action(&mut self, action: Action) {
        match action {
            Action::Unstake {
                amount,
                fees_earned,
            } => {
                if let Some(position) = &mut self.state.position {
                    // let (tokens_a, tokens_b) = self.calculate_unstake_amounts(amount);
                    // position.liquidity -= amount;
                    // self.state.wallet.token_a += tokens_a;
                    // self.state.wallet.token_b += tokens_b;
                    // if position.liquidity == 0.0 {
                    //     self.state.position = None;
                    // }
                }
            }
            Action::Stake {
                amount,
                lower_tick,
                upper_tick,
            } => {
                let (required_a, required_b) =
                    self.calculate_stake_amounts(amount, lower_tick, upper_tick);
                if self.state.wallet.token_a >= required_a
                    && self.state.wallet.token_b >= required_b
                {
                    self.state.wallet.token_a -= required_a;
                    self.state.wallet.token_b -= required_b;
                    let new_position = PositionState {
                        liquidity: amount,
                        lower_tick,
                        upper_tick,
                    };
                    self.state.position = Some(new_position);
                }
            }
            Action::Swap {
                token_buy,
                amount_sell,
                token_sell,
            } => {
                // let amount_out = self.simulate_swap(token_in, amount_in, token_out);
                // match token_in {}
            }
            Action::DoNothing => {}
        }
    }

    fn calculate_unstake_amounts(&self, amount: f64) -> (f64, f64) {
        // Implement logic to calculate tokens received when unstaking
        // This is a placeholder implementation
        (amount / 2.0, amount / 2.0)
    }

    fn calculate_stake_amounts(&self, amount: f64, lower_tick: i32, upper_tick: i32) -> (f64, f64) {
        // Implement logic to calculate tokens required for staking
        // This is a placeholder implementation
        (amount / 2.0, amount / 2.0)
    }

    fn simulate_swap(&self, token_in: String, amount_in: f64, token_out: String) -> f64 {
        // Implement swap simulation logic
        // This is a placeholder implementation
        amount_in * 0.98 // Assuming 2% slippage
    }

    fn run(&mut self, start_time: DateTime<Utc>, end_time: DateTime<Utc>) {
        // let mut cursor = None;
        println!("Nothing to see!")
        // loop {
        //     // let (transactions, new_cursor) = self
        //     //     .transaction_repo
        //     //     .get_transactions(start_time, end_time, cursor);

        //     // for transaction in transactions {
        //     //     self.process_transaction(&transaction);
        //     // }
        //     // if new_cursor.is_none() {
        //     //     break;
        //     // }
        //     // cursor = new_cursor;
        // }
    }
}
