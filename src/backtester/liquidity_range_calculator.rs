use anyhow::Result;

use crate::{
    models::{pool_model::PoolModel, positions_model::PositionModel},
    repositories::transactions_repo::{OrderDirection, TransactionRepo},
    try_calc,
    utils::{
        error::{PriceCalcError, SyncError},
        price_calcs::{sqrt_price_to_fixed, tick_to_sqrt_price, Q32},
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
    current_sqrt_price: u128,
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
            current_sqrt_price: 0,
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
        amount_in: u128,
        is_sell: bool,
        fee_rate: i16,
    ) -> Result<(i32, i32), PriceCalcError> {
        let mut current_tick = self.current_tick;
        let mut mut_current_sqrt_price = self.current_sqrt_price;

        let mut remaining_amount = amount_in * (1 - fee_rate / 10000) as u128;

        let starting_tick = current_tick;
        let mut ending_tick = starting_tick;

        // High level explanation: sqrt prices are differently calculate if we are within a tick range or at the corners of ticks.
        // Since Liquidity is constant at a tick range, we need a variable that can efficiently track the ratio of token_a and token_b in the range.
        // Thats the sqrt price and it can be very granular. We cant use ticks since they offer 0 granurality. If tick_spacing is 2, the pool would only have 3 ratios (100/0, 50/50, 0/100).
        while remaining_amount > 0 {
            let index = self.get_index(current_tick, is_sell);
            let liquidity = self.data[index].liquidity;
            let (lower_tick, upper_tick) =
                (self.data[index].lower_tick, self.data[index].upper_tick);

            let lower_sqrt_price_fixed = sqrt_price_to_fixed(tick_to_sqrt_price(lower_tick));
            let upper_sqrt_price_fixed = sqrt_price_to_fixed(tick_to_sqrt_price(upper_tick));

            // we get amounts in the tick_range by using the sqrt_price
            let (amount_a_in_tick_range, amount_b_in_tick_range) = self.calculate_amounts(
                liquidity,
                mut_current_sqrt_price,
                lower_sqrt_price_fixed,
                upper_sqrt_price_fixed,
            );

            // After getting the amounts, we have some branching logic.
            // If we remain in range, we simply update sqrtPrice but not tick. This will be used to get the new ratio in the next loop.
            // If we leave range, we must update tick and sqrtPrice and keep the loop going.
            if is_sell {
                if amount_a_in_tick_range > remaining_amount {
                    // Update sqrt price
                    mut_current_sqrt_price = self.calculate_new_sqrt_price(
                        mut_current_sqrt_price,
                        liquidity,
                        remaining_amount,
                        is_sell,
                    )?;

                    remaining_amount = 0;
                } else {
                    // Cross to the next tick range
                    remaining_amount -= amount_a_in_tick_range;
                    current_tick -= self.tick_spacing;
                    mut_current_sqrt_price = sqrt_price_to_fixed(tick_to_sqrt_price(current_tick));
                }
            } else {
                #[warn(clippy::collapsible_else_if)]
                if amount_b_in_tick_range > remaining_amount {
                    // Update sqrt price
                    mut_current_sqrt_price = self.calculate_new_sqrt_price(
                        mut_current_sqrt_price,
                        liquidity,
                        remaining_amount,
                        is_sell,
                    )?;

                    remaining_amount = 0;
                } else {
                    // Cross to the next tick range
                    remaining_amount -= amount_b_in_tick_range;
                    current_tick += self.tick_spacing;
                    mut_current_sqrt_price = sqrt_price_to_fixed(tick_to_sqrt_price(current_tick));
                }
            }

            ending_tick = current_tick;
        }

        self.current_tick = ending_tick;
        self.current_sqrt_price = mut_current_sqrt_price;

        Ok((starting_tick, ending_tick))
    }

    #[allow(clippy::too_many_arguments)]
    fn calculate_amounts(
        &self,
        liquidity: u128,
        current_sqrt_price_fixed: u128,
        lower_sqrt_price_fixed: u128,
        upper_sqrt_price_fixed: u128,
    ) -> (u128, u128) {
        // We calculate amounts based on the position of current_sqrt_price relative to the range

        if current_sqrt_price_fixed <= lower_sqrt_price_fixed {
            // Price is at or below the lower bound
            // All liquidity is in token B
            let amount_b = (liquidity * (upper_sqrt_price_fixed - lower_sqrt_price_fixed)) / Q32;

            (0, amount_b)
        } else if current_sqrt_price_fixed >= upper_sqrt_price_fixed {
            // Price is at or above the upper bound
            // All liquidity is in token A
            let amount_a = (liquidity * Q32 / lower_sqrt_price_fixed)
                .checked_sub(liquidity * Q32 / upper_sqrt_price_fixed)
                .unwrap();

            (amount_a, 0)
        } else {
            // Price is within the range
            // Liquidity is split between token A and B
            // FYI formulas re-arranged from official docs. I think they got it wrong. Used p_u-p_c for a amount which reflects b amount. idk... my math looks solid when testing it.

            let amount_a = (liquidity * Q32 / lower_sqrt_price_fixed)
                .checked_sub(liquidity * Q32 / current_sqrt_price_fixed)
                .unwrap();

            // Amount of token B: L * (sqrt(P_u) - sqrt(P_c))
            let amount_b = (liquidity * (upper_sqrt_price_fixed - current_sqrt_price_fixed)) / Q32;

            (amount_a, amount_b)
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
            let denominator = try_calc!(liquidity.checked_add(product / Q32))?;
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
            let product = try_calc!(amount_in.checked_mul(Q32))?;
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
        .fetch_highest_tx_swap(&pool_model.address)
        .await
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?;

    // Initialize the cursor with the latest tx_id
    let mut cursor = latest_transaction.clone().map(|tx| tx.tx_id);

    let swap_data = latest_transaction
        .unwrap()
        .data
        .to_swap_data()
        .map_err(|e| SyncError::DatabaseError(e.to_string()))?
        .clone();

    let is_sell = swap_data.token_in == pool_model.token_a_address;

    // DOUBLE CHECK IF RIGHT.
    // PLS
    liquidity_array.current_sqrt_price = if is_sell {
        sqrt_price_to_fixed((swap_data.amount_out / swap_data.amount_in).sqrt())
    } else {
        sqrt_price_to_fixed((swap_data.amount_in / swap_data.amount_out).sqrt())
    };

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

                    let is_increase = transaction.transaction_type.as_str() == "IncreaseLiquidity";

                    liquidity_array.update_liquidity_from_tx(tick_data, is_increase);
                }
                "Swap" => {
                    let swap_data = transaction
                        .data
                        .to_swap_data()
                        .map_err(|e| SyncError::ParseError(e.to_string()))?;

                    let is_sell = swap_data.token_in == pool_model.token_a_address;

                    // fee rates are in bps
                    let fee_rate_pct = pool_model.fee_rate / 10000;

                    let scaling_factor = if is_sell {
                        10f64.powi(pool_model.token_a_decimals as i32)
                    } else {
                        10f64.powi(pool_model.token_b_decimals as i32)
                    };

                    liquidity_array.simulate_swap(
                        (swap_data.amount_in * scaling_factor) as u128,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_liquidity_array() -> LiquidityArray {
        let mut array = LiquidityArray::new(-20, 20, 2);
        array.current_tick = 4;
        array.current_sqrt_price = sqrt_price_to_fixed(tick_to_sqrt_price(array.current_tick));
        array.update_liquidity(TickData {
            lower_tick: -10,
            upper_tick: 10,
            liquidity: 1_000_000_000_000,
        });
        array
    }

    #[test]
    fn test_simulate_swap_sell_direction() {
        let mut array = setup_liquidity_array();
        let starting_price = array.current_sqrt_price;

        let result = array
            .simulate_swap(6 * 10_i32.pow(6) as u128, true, 30)
            .unwrap();

        assert!(
            result.0 == 4 && result.1 == 4,
            "Swap should not have moved range at all. Starting tick and ending tick here."
        );
        assert!(
            array.current_sqrt_price < starting_price,
            "Price should decrease for a sell"
        );

        let result = array
            .simulate_swap(6 * 10_i32.pow(6) as u128, true, 30)
            .unwrap();

        assert!(
            result.0 == 4 && result.1 == 2,
            "Swap should have moved range to 0-2. By 1 tick spacing."
        );
        assert!(
            array.current_sqrt_price < sqrt_price_to_fixed(tick_to_sqrt_price(array.current_tick)),
            "Price should decrease slightly below tick 2."
        );
    }

    #[test]
    fn test_simulate_swap_buy_direction() {
        let mut array = setup_liquidity_array();
        let starting_price = array.current_sqrt_price;

        let result = array
            .simulate_swap(6 * 10_i32.pow(6) as u128, false, 30)
            .unwrap();

        assert!(
            result.0 == 4 && result.1 == 4,
            "Swap should not have moved range at all. Starting tick and ending tick here."
        );
        assert!(
            array.current_sqrt_price > starting_price,
            "Price should increase for a sell"
        );

        // SWAP AGAIN. TICKS AND STUFF NEEDS TO MOVE.
        let result = array
            .simulate_swap(6 * 10_i32.pow(6) as u128, false, 30)
            .unwrap();

        assert!(
            result.0 == 4 && result.1 == 6,
            "Swap should have moved range to 4-6. By 1 tick spacing."
        );
        assert!(
            array.current_sqrt_price > sqrt_price_to_fixed(tick_to_sqrt_price(array.current_tick)),
            "Price should decrease slightly above tick 4."
        );
    }

    #[test]
    fn test_calculate_new_sqrt_price() {
        let array = setup_liquidity_array();
        let liquidity = 1_000_000_000_000;
        let current_sqrt_price = sqrt_price_to_fixed(1.0);
        let amount_in = sqrt_price_to_fixed(10.0);

        // Test sell
        let new_sqrt_price_sell = array
            .calculate_new_sqrt_price(current_sqrt_price, liquidity, amount_in, true)
            .unwrap();

        assert!(
            new_sqrt_price_sell < current_sqrt_price,
            "Sell should decrease price"
        );

        // Test buy
        let new_sqrt_price_buy = array
            .calculate_new_sqrt_price(current_sqrt_price, liquidity, amount_in, false)
            .unwrap();
        assert!(
            new_sqrt_price_buy > current_sqrt_price,
            "Buy should increase price"
        );
    }

    #[test]
    fn test_calculate_amounts() {
        let array = setup_liquidity_array();
        let liquidity = 1_000_000_000_000;
        let current_sqrt_price = sqrt_price_to_fixed(1.0);
        let lower_sqrt_price = sqrt_price_to_fixed(0.99);
        let upper_sqrt_price = sqrt_price_to_fixed(1.01);

        // Equal 50/50 case.
        let (amount_a, amount_b) = array.calculate_amounts(
            liquidity,
            current_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        assert!(
            amount_a > 99 / 10 * 10_i32.pow(9) as u128
                && amount_a <= 110 / 10 * 10_i32.pow(9) as u128
                && amount_b > 99 / 10 * 10_i32.pow(9) as u128
                && amount_b <= 100 * 10_i32.pow(9) as u128,
            "Both amounts close to 50/50"
        );

        // 100/0 case.
        let (amount_a, amount_b) = array.calculate_amounts(
            liquidity,
            lower_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        assert!(
            amount_b >= 19 * 10_i32.pow(9) as u128 && amount_a == 0,
            "amountb is full and a is 0"
        );

        // 0/100 case.
        let (amount_a, amount_b) = array.calculate_amounts(
            liquidity,
            upper_sqrt_price,
            lower_sqrt_price,
            upper_sqrt_price,
        );

        assert!(
            amount_a >= 19 * 10_i32.pow(9) as u128 && amount_b == 0,
            "amount a is full and b is 0"
        );
    }
}
