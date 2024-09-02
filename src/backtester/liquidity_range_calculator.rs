use anyhow::Result;

use crate::{
    models::{pool_model::PoolModel, positions_model::PositionModel},
    repositories::transactions_repo::{OrderDirection, TransactionRepo},
    utils::price_calcs::{get_current_tick, tick_to_sqrt_price},
};

#[derive(Debug, Clone, Copy)]
pub struct TickData {
    lower_tick: i32,
    upper_tick: i32,
    liquidity: u128,
}

#[derive(Debug)]
pub struct LiquidityArray {
    data: Vec<TickData>,
    min_tick: i32,
    tick_spacing: i32,
}

// The liquidity array is a static array that usually has 500k indices.
// Each index contains the TickData where u have the spacing of the tick and the amount of liquidity in it.
impl LiquidityArray {
    fn new(min_tick: i32, max_tick: i32, tick_spacing: i32) -> Self {
        let size = ((max_tick - min_tick) / tick_spacing) as usize;
        let mut data = Vec::with_capacity(size);
        let mut current_tick = min_tick;

        for _ in 0..size {
            data.push(TickData {
                upper_tick: current_tick + tick_spacing,
                lower_tick: current_tick,
                liquidity: 0,
            });
            current_tick += tick_spacing;
        }

        LiquidityArray {
            data,
            min_tick,
            tick_spacing,
        }
    }

    fn get_index(&self, tick: i32, upper_tick: bool) -> usize {
        if upper_tick {
            ((tick - self.min_tick) / self.tick_spacing) as usize - 1
        } else {
            ((tick - self.min_tick) / self.tick_spacing) as usize
        }
    }

    fn update_liquidity(&mut self, tick_data: TickData) {
        let lower_tick_index = self.get_index(tick_data.lower_tick, false);
        let upper_tick_index = self.get_index(tick_data.upper_tick, true);

        let tick_count = upper_tick_index - lower_tick_index;

        // In case a position is providing liquidity in a single tick space, the tick_count needs to be set to zero.
        let final_tick_count = if tick_count == 0 { 1 } else { tick_count };
        let liquidity_per_tick_spacing = tick_data.liquidity / (final_tick_count as u128);

        // Distribute the liquidity evenly amongst the indices.
        for i in lower_tick_index..=upper_tick_index {
            self.data[i].liquidity += liquidity_per_tick_spacing;
        }
    }

    fn get_liquidity_in_range(&self, start_tick: i32, end_tick: i32) -> i128 {
        let start_index = self.get_index(start_tick, false);
        let end_index = self.get_index(end_tick, true);

        let range = &self.data[start_index..=end_index.min(self.data.len() - 1)];
        range
            .iter()
            .map(|tick_data| tick_data.liquidity as i128)
            .sum()
    }

    // is_sell represents the directional movement of token_a. In SOL/USDC case is_sell represents selling SOL for USDC.
    pub fn simulate_swap(
        &mut self,
        amount_in: f64,
        amount_out: f64,
        is_sell: bool,
        fee_rate: i16,
    ) -> Result<(i32, i32, f64)> {
        let starting_price = if is_sell {
            amount_in / amount_out
        } else {
            amount_out / amount_in
        };

        let amount_in_after_fee = amount_in * (1 - fee_rate as i64) as f64;

        let mut current_tick = get_current_tick(starting_price.sqrt(), self.tick_spacing);
        let mut remaining_amount = amount_in_after_fee;

        let starting_tick = current_tick;
        let mut ending_tick = starting_tick;

        while remaining_amount > 0.0 {
            let index = self.get_index(current_tick, is_sell);
            let liquidity = self.data[index].liquidity as f64;
            let (lower_tick, upper_tick) =
                (self.data[index].lower_tick, self.data[index].upper_tick);
            let (lower_sqrt_price, upper_sqrt_price) = (
                tick_to_sqrt_price(lower_tick),
                tick_to_sqrt_price(upper_tick),
            );
            let current_sqrt_price = tick_to_sqrt_price(current_tick);

            let (amount_a, amount_b) = self.calculate_amounts(
                current_tick,
                lower_tick,
                upper_tick,
                liquidity,
                current_sqrt_price,
                lower_sqrt_price,
                upper_sqrt_price,
            );

            let (new_amount, new_tick) = if is_sell {
                self.process_sell(amount_a, amount_in, lower_tick)
            } else {
                self.process_buy(amount_b, amount_in, upper_tick)
            };

            remaining_amount -= amount_in - new_amount;
            current_tick = new_tick;
            ending_tick = new_tick;

            let new_liquidity = self.calculate_liquidity(
                new_amount,
                if is_sell { amount_b } else { amount_a },
                current_sqrt_price,
                lower_sqrt_price,
                upper_sqrt_price,
            );
            self.data[index].liquidity = new_liquidity as u128;
        }

        Ok((starting_tick, ending_tick, fee_rate as f64 * amount_in))
    }

    fn calculate_amounts(
        &self,
        current_tick: i32,
        lower_tick: i32,
        upper_tick: i32,
        liquidity: f64,
        current_sqrt_price: f64,
        lower_sqrt_price: f64,
        upper_sqrt_price: f64,
    ) -> (f64, f64) {
        if current_tick > lower_tick && current_tick < upper_tick {
            (
                liquidity * (1.0 / current_sqrt_price - 1.0 / upper_sqrt_price),
                liquidity * (current_sqrt_price - lower_sqrt_price),
            )
        } else if current_tick == lower_tick {
            (
                liquidity * (1.0 / lower_sqrt_price - 1.0 / upper_sqrt_price),
                0.0,
            )
        } else {
            (0.0, liquidity * (upper_sqrt_price - lower_sqrt_price))
        }
    }

    fn process_sell(&self, amount_a: f64, amount_in: f64, lower_tick: i32) -> (f64, i32) {
        if amount_a > amount_in {
            (amount_a - amount_in, lower_tick)
        } else {
            (0.0, lower_tick)
        }
    }

    fn process_buy(&self, amount_b: f64, amount_in: f64, upper_tick: i32) -> (f64, i32) {
        if amount_b > amount_in {
            (amount_b - amount_in, upper_tick)
        } else {
            (0.0, upper_tick)
        }
    }

    fn calculate_liquidity(
        &self,
        amount_a: f64,
        amount_b: f64,
        sqrt_price_current: f64,
        sqrt_price_lower: f64,
        sqrt_price_upper: f64,
    ) -> f64 {
        let l_a = amount_a * sqrt_price_current / (sqrt_price_upper - sqrt_price_current);
        let l_b = amount_b / (sqrt_price_current - sqrt_price_lower);
        l_a.min(l_b)
    }

    fn update_liquidity_from_tx(&mut self, tick_data: TickData, is_increase: bool) {
        let lower_tick_index = self.get_index(tick_data.lower_tick, false);
        let upper_tick_index = self.get_index(tick_data.upper_tick, true);

        let tick_count = upper_tick_index - lower_tick_index;

        // In case a position is providing liquidity in a single tick space, the tick_count needs to be set to zero.
        let final_tick_count = if tick_count == 0 { 1 } else { tick_count };
        let liquidity_per_tick_spacing = tick_data.liquidity / (final_tick_count as u128);

        // Distribute the liquidity evenly amongst the indices.
        for i in lower_tick_index..=upper_tick_index {
            if is_increase {
                self.data[i].liquidity += liquidity_per_tick_spacing
            } else {
                self.data[i].liquidity -= liquidity_per_tick_spacing
            }
        }
    }
}

pub fn create_full_liquidity_range(
    tick_spacing: i16,
    positions: Vec<PositionModel>,
) -> Result<LiquidityArray> {
    let min_tick = -500_000;
    let max_tick = 500_000;

    let mut liquidity_array = LiquidityArray::new(min_tick, max_tick, tick_spacing as i32);

    for position in positions {
        let lower_tick: i32 = position.tick_lower;
        let upper_tick: i32 = position.tick_upper;
        let liquidity: u128 = position.liquidity;

        let tick_data = TickData {
            lower_tick,
            upper_tick,
            liquidity,
        };

        liquidity_array.update_liquidity(tick_data);
    }

    Ok(liquidity_array)
}

pub async fn sync_backwards(
    transaction_repo: &TransactionRepo,
    mut liquidity_array: LiquidityArray,
    pool_model: PoolModel,
    batch_size: i64,
) -> Result<LiquidityArray> {
    let latest_transaction = transaction_repo
        .fetch_highest_block_time_transaction(&pool_model.address)
        .await?;

    // Initialize the cursor with the latest tx_id
    let mut cursor = latest_transaction.map(|tx| tx.tx_id);

    loop {
        let transactions = transaction_repo
            .fetch_transactions(
                &pool_model.address,
                cursor,
                batch_size,
                OrderDirection::Descending,
            )
            .await?;

        if transactions.is_empty() {
            break;
        }

        for transaction in transactions.iter().rev() {
            // Process in reverse order
            match transaction.transaction_type.as_str() {
                "IncreaseLiquidity" | "DecreaseLiquidity" => {
                    let liquidity_data = transaction.data.to_liquidity_data()?;

                    let tick_data = TickData {
                        lower_tick: liquidity_data.tick_lower.unwrap(),
                        upper_tick: liquidity_data.tick_upper.unwrap(),
                        liquidity: liquidity_data.liquidity_amount.parse::<u128>().unwrap(),
                    };

                    let is_increase =
                        if transaction.transaction_type.as_str() == "IncreaseLiquidity" {
                            true
                        } else {
                            false
                        };

                    liquidity_array.update_liquidity_from_tx(tick_data, is_increase);
                }
                "Swap" => {
                    let swap_data = transaction.data.to_swap_data()?;

                    let is_sell = if swap_data.token_in == pool_model.token_a_address {
                        true
                    } else {
                        false
                    };

                    liquidity_array.simulate_swap(
                        swap_data.amount_in,
                        swap_data.amount_out,
                        is_sell,
                        pool_model.fee_rate,
                    )?;
                }
                _ => {}
            }
        }

        // Update cursor for next batch
        cursor = transactions.last().map(|t| t.tx_id);

        if transactions.len() < batch_size as usize {
            break; // We've reached the end
        }
    }

    Ok(liquidity_array)
}
