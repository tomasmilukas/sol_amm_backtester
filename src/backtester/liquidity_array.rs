use std::collections::HashMap;

use anyhow::{anyhow, Result};

use crate::{
    try_calc,
    utils::{
        error::PriceCalcError,
        price_calcs::{
            calculate_amounts, calculate_new_sqrt_price, sqrt_price_to_fixed, tick_to_sqrt_price,
            Q32, Q64,
        },
    },
};

#[derive(Debug, Clone, Copy)]
pub struct TickData {
    pub lower_tick: i32,
    pub upper_tick: i32,
    pub liquidity: u128,
}

#[derive(Debug, Clone)]
pub struct LiquidityArray {
    pub data: Vec<TickData>,
    pub positions: HashMap<String, OwnersPosition>,
    pub total_liquidity_provided: u128,
    pub min_tick: i32,
    pub fee_rate: i16,
    pub tick_spacing: i32,
    pub current_tick: i32,
    pub current_sqrt_price: u128,
}

#[derive(Debug, Clone)]
pub struct OwnersPosition {
    pub owner: String,
    pub lower_tick: i32,
    pub upper_tick: i32,
    pub liquidity: u128,
    pub fees_owed_a: u128,
    pub fees_owed_b: u128,
}

// The liquidity array is a static array that usually has 500k indices.
// Each index contains the TickData where u have the spacing of the tick and the amount of liquidity in it.
impl LiquidityArray {
    pub fn new(min_tick: i32, max_tick: i32, tick_spacing: i32, fee_rate: i16) -> Self {
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
            positions: HashMap::new(),
            min_tick,
            fee_rate,
            tick_spacing,
            total_liquidity_provided: 0,
            current_tick: 0,
            current_sqrt_price: 0,
        }
    }

    pub fn get_index(&self, tick: i32, is_upper_tick: bool) -> usize {
        let index = ((tick - self.min_tick) / self.tick_spacing) as usize;
        if is_upper_tick {
            index.saturating_sub(1).min(self.data.len() - 1)
        } else {
            index.min(self.data.len() - 1)
        }
    }

    pub fn update_liquidity(&mut self, tick_data: TickData) {
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

    pub fn add_owners_position(&mut self, position: OwnersPosition, position_id: String) {
        self.positions.insert(position_id.clone(), position.clone());
        self.update_liquidity_from_tx(
            TickData {
                lower_tick: position.lower_tick,
                upper_tick: position.upper_tick,
                liquidity: position.liquidity,
            },
            true,
        );
    }

    pub fn remove_owners_position(&mut self, position_id: &str) -> Result<OwnersPosition> {
        if let Some(position) = self.positions.remove(position_id) {
            self.update_liquidity_from_tx(
                TickData {
                    lower_tick: position.lower_tick,
                    upper_tick: position.upper_tick,
                    liquidity: position.liquidity,
                },
                false,
            );
            Ok(position)
        } else {
            return Err(anyhow::anyhow!("Position not found"));
        }
    }

    fn get_liquidity_in_range(&self, start_tick: i32, end_tick: i32) -> i128 {
        let adj_start_tick = if start_tick == end_tick {
            end_tick - self.tick_spacing
        } else {
            start_tick
        };

        let start_index = self.get_index(adj_start_tick, false);
        let end_index = self.get_index(end_tick, true);

        let range = &self.data[start_index..=end_index.min(self.data.len() - 1)];
        range
            .iter()
            .map(|tick_data| tick_data.liquidity as i128)
            .sum()
    }

    pub fn simulate_swap_with_fees(
        &mut self,
        amount_in: u128,
        is_sell: bool,
    ) -> Result<(i32, i32, u128), PriceCalcError> {
        let fees = amount_in * self.fee_rate as u128 / 10000;
        let amount_after_fees = amount_in - fees;

        let (start_tick, end_tick) = self.simulate_swap(amount_after_fees, is_sell)?;

        // Simplistic distribution since technically ticks dont evenly distribute fees but will suffice for now.
        self.distribute_fees(fees, start_tick, end_tick, is_sell);

        Ok((start_tick, end_tick, fees))
    }

    fn distribute_fees(&mut self, total_fees: u128, start_tick: i32, end_tick: i32, is_sell: bool) {
        let total_liquidity = self.get_liquidity_in_range(start_tick, end_tick) as u128;

        for position in self.positions.values_mut() {
            if (position.lower_tick <= end_tick && position.upper_tick >= start_tick)
                || (position.lower_tick >= end_tick && position.upper_tick <= start_tick)
            {
                let position_liquidity = position.liquidity;
                let fee_share = (total_fees * Q64 / total_liquidity) * position_liquidity;

                if is_sell {
                    position.fees_owed_a += fee_share;
                } else {
                    position.fees_owed_b += fee_share;
                }
            }
        }
    }

    pub fn collect_fees(&mut self, position_id: &str) -> Result<(u128, u128)> {
        if let Some(position) = self.positions.get_mut(position_id) {
            let fees_a = position.fees_owed_a / Q64;
            let fees_b = position.fees_owed_b / Q64;
            position.fees_owed_a = 0;
            position.fees_owed_b = 0;
            Ok((fees_a, fees_b))
        } else {
            return Err(anyhow::anyhow!("Position not found"));
        }
    }

    // is_sell represents the directional movement of token_a. In SOL/USDC case is_sell represents selling SOL for USDC.
    pub fn simulate_swap(
        &mut self,
        amount_in: u128,
        is_sell: bool,
    ) -> Result<(i32, i32), PriceCalcError> {
        let mut current_tick = self.current_tick;
        let mut mut_current_sqrt_price = self.current_sqrt_price;

        let mut remaining_amount = amount_in;

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
            let (amount_a_in_tick_range, amount_b_in_tick_range) = calculate_amounts(
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
                    mut_current_sqrt_price = calculate_new_sqrt_price(
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
                    mut_current_sqrt_price = calculate_new_sqrt_price(
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

    pub fn update_liquidity_from_tx(&mut self, tick_data: TickData, is_increase: bool) {
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

        if is_increase {
            self.total_liquidity_provided += tick_data.liquidity
        } else {
            self.total_liquidity_provided -= tick_data.liquidity
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_liquidity_array() -> LiquidityArray {
        let mut array = LiquidityArray::new(-20, 20, 2, 300);
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
            .simulate_swap(6 * 10_i32.pow(6) as u128, true)
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
            .simulate_swap(6 * 10_i32.pow(6) as u128, true)
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
            .simulate_swap(6 * 10_i32.pow(6) as u128, false)
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
            .simulate_swap(6 * 10_i32.pow(6) as u128, false)
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
    fn test_simulate_swap_with_fees_sell() {
        let mut array = setup_liquidity_array();
        let amount_in = 1_000_u128;

        // Add a test position
        array.add_owners_position(
            OwnersPosition {
                owner: "Alice".to_string(),
                lower_tick: -10,
                upper_tick: 10,
                liquidity: 1_000_000,
                fees_owed_a: 0,
                fees_owed_b: 0,
            },
            String::from("Alice_-10_10_1000000"),
        );

        let (start_tick, end_tick, fees) = array.simulate_swap_with_fees(amount_in, true).unwrap();

        assert_eq!(fees, 30, "3% of 100should be 30");

        // Check if fees were accrued correctly
        let position = array.positions.get("Alice_-10_10_1000000").unwrap();
        assert_eq!(
            position.fees_owed_a, 4980615919000000,
            "All fees should be accrued to token A for a sell"
        );
        assert_eq!(
            position.fees_owed_b, 0,
            "No fees should be accrued to token B for a sell"
        );
    }

    #[test]
    fn test_simulate_swap_with_fees_buy() {
        let mut array = setup_liquidity_array();
        let amount_in = 200_u128;

        // Add a test position
        array.add_owners_position(
            OwnersPosition {
                owner: "Bob".to_string(),
                lower_tick: -10,
                upper_tick: 10,
                liquidity: 1_000_000,
                fees_owed_a: 0,
                fees_owed_b: 0,
            },
            String::from("Bob_-10_10_1000000"),
        );

        let (start_tick, end_tick, fees) = array.simulate_swap_with_fees(amount_in, false).unwrap();

        assert_eq!(fees, 6, "3% of 200 should be 6");

        let position = array.positions.get("Bob_-10_10_1000000").unwrap();

        // TAKE INTO ACCOUNT THESE FEES ARE MULTIPLIED BY Q64 for precision. U can see in collect_fees it is divided to remove it.
        assert_eq!(
            position.fees_owed_b, 996123183000000,
            "All fees should be accrued to token A for a buy"
        );
        assert_eq!(
            position.fees_owed_a, 0,
            "No fees should be accrued to token B for a buy"
        );
    }

    #[test]
    fn test_collect_fees() {
        let mut array = setup_liquidity_array();

        // Add a test position with pre-existing fees
        array.add_owners_position(
            OwnersPosition {
                owner: "Charlie".to_string(),
                lower_tick: -10,
                upper_tick: 10,
                liquidity: 1_000_000_000_000,
                fees_owed_a: 50_000 * Q64,
                fees_owed_b: 75_000 * Q64,
            },
            String::from("Charlie_-10_10_1000000000000"),
        );

        let position_id = "Charlie_-10_10_1000000000000";
        let (collected_fees_a, collected_fees_b) = array.collect_fees(position_id).unwrap();

        assert_eq!(
            collected_fees_a, 50_000,
            "Should collect all of token A fees"
        );
        assert_eq!(
            collected_fees_b, 75_000,
            "Should collect all of token B fees"
        );

        // Check if fees were reset after collection
        let position = array.positions.get(position_id).unwrap();
        assert_eq!(
            position.fees_owed_a, 0,
            "Token A fees should be reset to 0 after collection"
        );
        assert_eq!(
            position.fees_owed_b, 0,
            "Token B fees should be reset to 0 after collection"
        );
    }
}
