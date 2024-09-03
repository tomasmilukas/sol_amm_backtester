use anyhow::Result;

use crate::{
    models::{pool_model::PoolModel, positions_model::PositionModel},
    repositories::transactions_repo::{OrderDirection, TransactionRepo},
    try_calc,
    utils::{
        error::{PriceCalcError, SyncError},
        price_calcs::{f64_to_fixed_point, fixed_point_to_f64, tick_to_sqrt_price},
    },
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
    current_tick: i32,
    current_sqrt_price: f64,
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
            current_tick: 0,
            current_sqrt_price: 0.0,
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
    ) -> Result<(i32, i32, f64), PriceCalcError> {
        const Q64: u128 = 1u128 << 64;

        let mut current_tick = self.current_tick;
        let mut current_sqrt_price_fixed = f64_to_fixed_point(self.current_sqrt_price);

        let mut remaining_amount =
            f64_to_fixed_point(amount_in * (1.0 - fee_rate as f64 / 10000.0));
        let original_amount_out = f64_to_fixed_point(amount_out);
        let mut amount_out: u128 = 0;

        let starting_tick = current_tick;
        let mut ending_tick = starting_tick;

        // High level explanation: sqrt prices are differently calculate if we are within a tick range or at the corners of ticks.
        // Since Liquidity is constant at a tick range, we need a variable that can efficiently track the ratio of token_a and token_b in the range.
        // Thats the sqrt price and it can be very granular. We cant use ticks since they offer 0 granurality. If tick_spacing is 2, the pool would only have 3 ratios (100/0, 50/50, 0/100).
        while remaining_amount > 0 || amount_out >= original_amount_out {
            let index = self.get_index(current_tick, is_sell);
            let liquidity = self.data[index].liquidity;
            let (lower_tick, upper_tick) =
                (self.data[index].lower_tick, self.data[index].upper_tick);

            let lower_sqrt_price_fixed = f64_to_fixed_point(tick_to_sqrt_price(lower_tick));
            let upper_sqrt_price_fixed = f64_to_fixed_point(tick_to_sqrt_price(upper_tick));

            // we get amounts in the tick_range by using the sqrt_price
            let (amount_a_in_tick_range, amount_b_in_tick_range) = self.calculate_amounts(
                current_tick,
                lower_tick,
                upper_tick,
                liquidity,
                current_sqrt_price_fixed,
                lower_sqrt_price_fixed,
                upper_sqrt_price_fixed,
            );

            // After getting the amounts, we have some branching logic.
            // If we remain in range, we simply update sqrtPrice but not tick. This will be used to get the new ratio in the next loop.
            // If we leave range, we must update tick and sqrtPrice and keep the loop going.
            if is_sell {
                if amount_a_in_tick_range > remaining_amount {
                    // Stay in the same tick range
                    let amount_a_used = remaining_amount;
                    let amount_b_out =
                        amount_b_in_tick_range * amount_a_used / amount_a_in_tick_range;

                    // Update sqrt price
                    current_sqrt_price_fixed = self.calculate_new_sqrt_price(
                        current_sqrt_price_fixed,
                        liquidity,
                        amount_a_used,
                        is_sell,
                    )?;

                    remaining_amount = 0;
                    amount_out += amount_b_out;
                } else {
                    // Cross to the next tick range
                    remaining_amount -= amount_a_in_tick_range;
                    amount_out += amount_b_in_tick_range;
                    current_tick = lower_tick - self.tick_spacing;
                    current_sqrt_price_fixed = f64_to_fixed_point(tick_to_sqrt_price(current_tick));
                }
            } else {
                #[warn(clippy::collapsible_else_if)]
                if amount_b_in_tick_range > remaining_amount {
                    // Stay in the same tick range
                    let amount_b_used = remaining_amount;
                    let amount_a_out =
                        amount_a_in_tick_range * amount_b_used / amount_b_in_tick_range;

                    // Update sqrt price
                    current_sqrt_price_fixed = self.calculate_new_sqrt_price(
                        current_sqrt_price_fixed,
                        liquidity,
                        amount_b_used,
                        is_sell,
                    )?;

                    remaining_amount = 0;
                    amount_out += amount_a_out;
                } else {
                    // Cross to the next tick range
                    remaining_amount -= amount_b_in_tick_range;
                    amount_out += amount_a_in_tick_range;
                    current_tick = upper_tick + self.tick_spacing;
                    current_sqrt_price_fixed = f64_to_fixed_point(tick_to_sqrt_price(current_tick));
                }
            }

            ending_tick = current_tick;
        }

        self.current_tick = ending_tick;
        self.current_sqrt_price = fixed_point_to_f64(current_sqrt_price_fixed);

        Ok((starting_tick, ending_tick, fixed_point_to_f64(amount_out)))
    }

    #[allow(clippy::too_many_arguments)]
    fn calculate_amounts(
        &self,
        current_tick: i32,
        lower_tick: i32,
        upper_tick: i32,
        liquidity: u128,
        current_sqrt_price_fixed: u128,
        lower_sqrt_price_fixed: u128,
        upper_sqrt_price_fixed: u128,
    ) -> (u128, u128) {
        const Q64: u128 = 1u128 << 64;

        // The check is necessary since diff formulas are used. If we are within range we use current price to calculate amounts. If outside we use full Pb-Pa range.
        if current_tick > lower_tick && current_tick < upper_tick {
            // The standard deltaX = L/Pc - L/Pb
            let amount_a = (liquidity * Q64 / current_sqrt_price_fixed)
                .checked_sub(liquidity * Q64 / upper_sqrt_price_fixed)
                .unwrap();

            // The standard deltaY = Pc - Pa
            let amount_b = (liquidity * (current_sqrt_price_fixed - lower_sqrt_price_fixed)) / Q64;

            (amount_a, amount_b)
        } else if current_tick == lower_tick {
            let amount_a = (liquidity * Q64 / lower_sqrt_price_fixed)
                .checked_sub(liquidity * Q64 / upper_sqrt_price_fixed)
                .unwrap();

            (amount_a, 0)
        } else {
            let amount_b = (liquidity * (upper_sqrt_price_fixed - lower_sqrt_price_fixed)) / Q64;

            (0, amount_b)
        }
    }

    // General formulas:
    // amount_a changing: sqrt_P_new = (sqrt_P * L) / (L + Δx * sqrt_P)
    // amount_b changing: sqrt_P_new = sqrt_P + (Δy / L)
    fn calculate_new_sqrt_price(
        &self,
        current_sqrt_price: u128,
        liquidity: u128,
        amount_in: u128,
        is_sell: bool,
    ) -> Result<u128, PriceCalcError> {
        const Q64: u128 = 1u128 << 64;

        // Formula explanations for later in case need to edit:
        // x = L / sqrt(P) also y = L * sqrt(P)

        if is_sell {
            /*
            for this case:
            (x + Δx) * y = L^2

            (L/sqrt(P) + Δx) * (L*sqrt(P)) = L^2
            L^2 + Δx*L*srt(P)q = L^2
            Δx*L*sqrt(P) = L^2 - L^2 = 0

            After price change we must satisfy: (L/sqrt(P_new)) * (L*sqrt(P_new)) = L^2.
            Hence, L/sqrt(P_new) = L/sqrt(P) + Δx. sqrt(P_new) = L / (L/sqrt(P) + Δx).

            sqrt(P_new) = (L * sqrt(P)) / (L + Δx * sqrt(P))
            */
            let numerator = try_calc!(current_sqrt_price.checked_mul(liquidity))?;
            let product = try_calc!(amount_in.checked_mul(current_sqrt_price))?;
            let denominator = try_calc!(liquidity.checked_add(product / Q64))?;
            try_calc!(numerator.checked_div(denominator))
        } else {
            /*
            for this case:
            x * (y + Δy) = L^2

            (L/sqrt(P)) * (L*sqrt(P) + Δy) = L^2
            L^2 + L*Δy = L^2
            L*Δy = L^2 - L^2 = 0

            After price change we must satisfy: (L/sqrt(P_new)) * (L*sqrt(P_new)) = L^2.
            Hence, L*sqrt(P_new) = L*sqrt(P) + Δy. sqrt(P_new) = sqrt(P) + (Δy / L).

            sqrt(P_new) = sqrt(P) + (Δy / L)
            */
            let product = try_calc!(amount_in.checked_mul(Q64))?;
            let increment = try_calc!(product.checked_div(liquidity))?;
            try_calc!(current_sqrt_price.checked_add(increment))
        }
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
) -> Result<LiquidityArray, SyncError> {
    let latest_transaction = transaction_repo
        .fetch_highest_block_time_transaction(&pool_model.address)
        .await
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

    // Initialize the cursor with the latest tx_id
    let mut cursor = latest_transaction.map(|tx| tx.tx_id);

    // Implement logic to fetch the most accurate price at that timestamp.
    // Then caculate the equiv current tick and sqrtPrice and add it to self.

    loop {
        let transactions = transaction_repo
            .fetch_transactions(
                &pool_model.address,
                cursor,
                batch_size,
                OrderDirection::Descending,
            )
            .await
            .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

        if transactions.is_empty() {
            break;
        }

        for transaction in transactions.iter().rev() {
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
                        if transaction.transaction_type.as_str() == "IncreaseLiquidity" {
                            true
                        } else {
                            false
                        };

                    liquidity_array.update_liquidity_from_tx(tick_data, is_increase);
                }
                "Swap" => {
                    let swap_data = transaction
                        .data
                        .to_swap_data()
                        .map_err(|e| SyncError::ParseError(e.to_string()))?;

                    let is_sell = if swap_data.token_in == pool_model.token_a_address {
                        true
                    } else {
                        false
                    };

                    // fee rates are in bps
                    let fee_rate_pct = pool_model.fee_rate / 10000;

                    liquidity_array.simulate_swap(
                        swap_data.amount_in,
                        swap_data.amount_out,
                        is_sell,
                        fee_rate_pct,
                    )?;
                }
                _ => {}
            }
        }

        cursor = transactions.last().map(|t| t.tx_id);

        if transactions.len() < batch_size as usize {
            break;
        }
    }

    Ok(liquidity_array)
}
