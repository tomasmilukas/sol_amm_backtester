use std::{collections::HashMap, thread, time::Duration};

use crate::utils::{
    error::LiquidityArrayError,
    price_calcs::{
        calculate_amounts, calculate_new_sqrt_price, tick_to_sqrt_price_u256, Q64, U256,
    },
};

#[derive(Debug, Clone, Copy)]
pub struct TickData {
    pub lower_tick: i32,
    pub upper_tick: i32,
    pub liquidity: U256,
}

#[derive(Debug, Clone)]
pub struct LiquidityArray {
    pub data: Vec<TickData>,
    pub positions: HashMap<String, OwnersPosition>,
    pub total_liquidity_provided: U256,
    pub min_tick: i32,
    pub fee_rate: i16,
    pub tick_spacing: i32,
    pub current_tick: i32,
    // B/A. So in SOL/USDC pool it would be 150/1 = 150.
    pub current_sqrt_price: U256,
}

#[derive(Debug, Clone)]
pub struct OwnersPosition {
    pub owner: String,
    pub lower_tick: i32,
    pub upper_tick: i32,
    // LIQUIDITY ALRDY SCALED IMPLICITLY. DOESNT NEED Q64 SCALING.
    pub liquidity: U256,
    // ALL FEES STORED IN Q64
    pub fees_owed_a: U256,
    pub fees_owed_b: U256,
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
                liquidity: U256::zero(),
            });
            current_tick += tick_spacing;
        }

        LiquidityArray {
            data,
            positions: HashMap::new(),
            min_tick,
            fee_rate,
            tick_spacing,
            total_liquidity_provided: U256::zero(),
            current_tick: 0,
            current_sqrt_price: U256::zero(),
        }
    }

    pub fn get_index(&self, tick: i32, is_upper_tick: bool) -> usize {
        let tick_index = (tick - self.min_tick) as f64 / self.tick_spacing as f64;

        let index = if is_upper_tick {
            tick_index.ceil() as usize - 1
        } else {
            tick_index.floor() as usize
        };

        index.clamp(0, self.data.len() - 1)
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

    pub fn remove_owners_position(
        &mut self,
        position_id: &str,
    ) -> Result<OwnersPosition, LiquidityArrayError> {
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
            Err(LiquidityArrayError::PositionNotFound(
                position_id.to_string(),
            ))
        }
    }

    fn get_liquidity_in_range(&self, start_tick: i32, end_tick: i32) -> U256 {
        let adj_start_tick = if start_tick == end_tick {
            end_tick - self.tick_spacing
        } else {
            start_tick
        };

        let start_index = self.get_index(adj_start_tick, false);
        let end_index = self.get_index(end_tick, true);

        let range = &self.data[start_index..=end_index.min(self.data.len() - 1)];
        let mut sum = U256::zero();

        for range_item in range {
            sum += U256::from(range_item.liquidity)
        }

        sum
    }

    pub fn simulate_swap_with_fees(
        &mut self,
        amount_in: U256,
        is_sell: bool,
    ) -> Result<(i32, i32, U256, U256), LiquidityArrayError> {
        let fees = (amount_in * U256::from(self.fee_rate)) / 10000;
        let amount_after_fees = amount_in - fees;

        let (start_tick, end_tick, amount_out) = self.simulate_swap(amount_after_fees, is_sell)?;

        // Simplistic distribution since technically ticks dont evenly distribute fees but will suffice for now.
        self.distribute_fees(fees, start_tick, end_tick, is_sell);

        Ok((start_tick, end_tick, amount_out, fees))
    }

    fn distribute_fees(&mut self, total_fees: U256, start_tick: i32, end_tick: i32, is_sell: bool) {
        let total_liquidity = self.get_liquidity_in_range(start_tick, end_tick);
        println!("ENTER DISTRIBUTE FEES FN");

        for position in self.positions.values_mut() {
            println!("POSITION INFO: {:?}", position);

            if (position.lower_tick <= end_tick && position.upper_tick >= start_tick)
                || (position.lower_tick >= end_tick && position.upper_tick <= start_tick)
            {
                let position_liquidity = position.liquidity;
                let fee_share = (total_fees * Q64 / total_liquidity) * position_liquidity;
                println!(
                    "FEES INFO: {} {} {}",
                    position_liquidity, fee_share, total_fees
                );

                if is_sell {
                    position.fees_owed_a += fee_share;
                } else {
                    position.fees_owed_b += fee_share;
                }
            }
        }
    }

    pub fn collect_fees(&mut self, position_id: &str) -> Result<(U256, U256), LiquidityArrayError> {
        if let Some(position) = self.positions.get_mut(position_id) {
            let fees_a = position.fees_owed_a / Q64;
            let fees_b = position.fees_owed_b / Q64;
            position.fees_owed_a = U256::zero();
            position.fees_owed_b = U256::zero();

            Ok((fees_a, fees_b))
        } else {
            Err(LiquidityArrayError::PositionNotFound(
                position_id.to_string(),
            ))
        }
    }

    // is_sell represents the directional movement of token_a. In SOL/USDC case is_sell represents selling SOL for USDC.
    // High level explanation: use sqrt prices to track ratios of tokens within ticks. Liquidity is constant and doesnt need to be touched during swaps.
    pub fn simulate_swap(
        &mut self,
        amount_in: U256,
        is_sell: bool,
    ) -> Result<(i32, i32, U256), LiquidityArrayError> {
        let mut current_tick = self.current_tick;
        let mut current_sqrt_price = self.current_sqrt_price;
        let mut remaining_amount = amount_in;
        let mut amount_out = U256::zero();
        let starting_tick = current_tick;

        fn calculate_output(
            liquidity: U256,
            sqrt_price_start: U256,
            sqrt_price_end: U256,
            is_sell: bool,
        ) -> U256 {
            if is_sell {
                // Δy = L * (√P_start - √P_end)
                liquidity * (sqrt_price_start - sqrt_price_end) / Q64
            } else {
                // Δx = L * (√P_end - √P_start) / (√P_start * √P_end)
                liquidity * (sqrt_price_end - sqrt_price_start) / sqrt_price_start
            }
        }

        while remaining_amount > U256::zero() {
            thread::sleep(Duration::from_secs(1));

            let index = self.get_index(current_tick, is_sell);
            let liquidity = self.data[index].liquidity;
            let (lower_tick, upper_tick) =
                (self.data[index].lower_tick, self.data[index].upper_tick);
            let lower_sqrt_price = tick_to_sqrt_price_u256(lower_tick);
            let upper_sqrt_price = tick_to_sqrt_price_u256(upper_tick);

            println!(
                "PRICES: {} {} {} {} {} {}",
                current_sqrt_price,
                upper_sqrt_price,
                upper_sqrt_price - current_sqrt_price,
                remaining_amount,
                is_sell,
                liquidity
            );

            let (max_in, sqrt_price_target) = if is_sell {
                let max_in =
                    liquidity * (current_sqrt_price - lower_sqrt_price) / current_sqrt_price;
                (max_in, lower_sqrt_price)
            } else {
                let max_in = liquidity * (upper_sqrt_price - current_sqrt_price) / upper_sqrt_price;
                (max_in, upper_sqrt_price)
            };

            println!("STUFF: {} {}", max_in, sqrt_price_target);

            if remaining_amount <= max_in {
                let new_sqrt_price = calculate_new_sqrt_price(
                    current_sqrt_price,
                    liquidity,
                    remaining_amount,
                    is_sell,
                );
                let actual_output =
                    calculate_output(liquidity, current_sqrt_price, new_sqrt_price, is_sell);
                amount_out = amount_out + actual_output;
                current_sqrt_price = new_sqrt_price;
                break;
            } else {
                let actual_output =
                    calculate_output(liquidity, current_sqrt_price, sqrt_price_target, is_sell);
                amount_out = amount_out + actual_output;
                remaining_amount = remaining_amount - max_in;
                if is_sell {
                    current_tick -= self.tick_spacing;
                } else {
                    current_tick += self.tick_spacing;
                }
                current_sqrt_price = sqrt_price_target;
            }
        }

        self.current_tick = current_tick;
        self.current_sqrt_price = current_sqrt_price;
        Ok((starting_tick, current_tick, amount_out))
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
        array.current_sqrt_price = tick_to_sqrt_price_u256(array.current_tick);
        array.update_liquidity(TickData {
            lower_tick: -10,
            upper_tick: 10,
            liquidity: U256::from(1_000_000_000_000_u128),
        });
        array
    }

    #[test]
    fn test_simulate_swap_sell_direction() {
        let mut array = setup_liquidity_array();
        let starting_price = array.current_sqrt_price;

        let result = array
            .simulate_swap(U256::from(6 * 10_i32.pow(6) as u128), true)
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
            .simulate_swap(U256::from(6 * 10_i32.pow(6) as u128), true)
            .unwrap();

        assert!(
            result.0 == 4 && result.1 == 2,
            "Swap should have moved range to 0-2. By 1 tick spacing."
        );
        assert!(
            array.current_sqrt_price < tick_to_sqrt_price_u256(array.current_tick),
            "Price should decrease slightly below tick 2."
        );
    }

    #[test]
    fn test_simulate_swap_buy_direction() {
        let mut array = setup_liquidity_array();
        let starting_price = array.current_sqrt_price;

        let result = array
            .simulate_swap(U256::from(6 * 10_i32.pow(6) as u128), false)
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
            .simulate_swap(U256::from(6 * 10_i32.pow(6) as u128), false)
            .unwrap();

        assert!(
            result.0 == 4 && result.1 == 6,
            "Swap should have moved range to 4-6. By 1 tick spacing."
        );
        assert!(
            array.current_sqrt_price > tick_to_sqrt_price_u256(array.current_tick),
            "Price should decrease slightly above tick 4."
        );
    }

    #[test]
    fn test_simulate_swap_with_fees_sell() {
        let mut array = setup_liquidity_array();
        let amount_in = U256::from(1_000_u128);

        // Add a test position
        array.add_owners_position(
            OwnersPosition {
                owner: "Alice".to_string(),
                lower_tick: -10,
                upper_tick: 10,
                liquidity: U256::from(1_000_000),
                fees_owed_a: U256::zero(),
                fees_owed_b: U256::zero(),
            },
            String::from("Alice_-10_10_1000000"),
        );

        let (start_tick, end_tick, _amount_out, fees) =
            array.simulate_swap_with_fees(amount_in, true).unwrap();

        assert_eq!(fees, U256::from(30), "3% of 100 should be 30");

        // Check if fees were accrued correctly
        let position = array.positions.get("Alice_-10_10_1000000").unwrap();
        assert_eq!(
            position.fees_owed_a,
            U256::from(4980615919000000_u128),
            "All fees should be accrued to token A for a sell"
        );
        assert_eq!(
            position.fees_owed_b,
            U256::zero(),
            "No fees should be accrued to token B for a sell"
        );
    }

    #[test]
    fn test_simulate_swap_with_fees_buy() {
        let mut array = setup_liquidity_array();
        let amount_in = U256::from(200_u128);

        // Add a test position
        array.add_owners_position(
            OwnersPosition {
                owner: "Bob".to_string(),
                lower_tick: -10,
                upper_tick: 10,
                liquidity: U256::from(1_000_000_u128),
                fees_owed_a: U256::zero(),
                fees_owed_b: U256::zero(),
            },
            String::from("Bob_-10_10_1000000"),
        );

        let (start_tick, end_tick, _amount_out, fees) =
            array.simulate_swap_with_fees(amount_in, false).unwrap();

        assert_eq!(fees, U256::from(6), "3% of 200 should be 6");

        let position = array.positions.get("Bob_-10_10_1000000").unwrap();

        // TAKE INTO ACCOUNT THESE FEES ARE MULTIPLIED BY Q32 for precision. U can see in collect_fees it is divided to remove it.
        assert_eq!(
            position.fees_owed_b,
            U256::from(996123183000000_u128),
            "All fees should be accrued to token B for a buy"
        );
        assert_eq!(
            position.fees_owed_a,
            U256::from(0),
            "No fees should be accrued to token A for a buy"
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
                liquidity: U256::from(1_000_000_000_000_u128),
                fees_owed_a: U256::from(50_000) * Q64,
                fees_owed_b: U256::from(75_000) * Q64,
            },
            String::from("Charlie_-10_10_1000000000000"),
        );

        let position_id = "Charlie_-10_10_1000000000000";
        let (collected_fees_a, collected_fees_b) = array.collect_fees(position_id).unwrap();

        assert_eq!(
            collected_fees_a,
            U256::from(50_000),
            "Should collect all of token A fees"
        );
        assert_eq!(
            collected_fees_b,
            U256::from(75_000),
            "Should collect all of token B fees"
        );

        // Check if fees were reset after collection
        let position = array.positions.get(position_id).unwrap();
        assert_eq!(
            position.fees_owed_a,
            U256::zero(),
            "Token A fees should be reset to 0 after collection"
        );
        assert_eq!(
            position.fees_owed_b,
            U256::zero(),
            "Token B fees should be reset to 0 after collection"
        );
    }
}
