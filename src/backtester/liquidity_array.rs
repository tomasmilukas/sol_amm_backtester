use std::collections::HashMap;

use crate::utils::{
    core_math::{calculate_amounts, calculate_new_sqrt_price, tick_to_sqrt_price_u256, Q128, U256},
    error::LiquidityArrayError,
};

#[derive(Debug, Clone, Copy)]
pub struct TickData {
    pub tick: i32,
    // FEES SCALED BY Q128.
    pub fee_growth_outside_a: U256,
    pub fee_growth_outside_b: U256,
    // can be negative, so we dont use U256.
    pub net_liquidity: i128,
    // created this so we can keep a static array to make an easier architecture.
    pub is_initialized: bool,
}

#[derive(Debug, Clone)]
pub struct LiquidityArray {
    pub data: Vec<TickData>,
    pub positions: HashMap<String, OwnersPosition>,
    // LIQUIDITY NOT SCALED. sum of liquidity from active positions in current_tick
    pub active_liquidity: U256,
    // FEES SCALED BY Q128.
    pub fee_growth_global_a: U256,
    pub fee_growth_global_b: U256,
    pub min_tick: i32,
    pub fee_rate: i16,
    pub tick_spacing: i32,
    pub current_tick: i32,
    // SQRT PRICE SCALED BY Q64. B/A. So in SOL/USDC pool it would be 150/1 = 150.
    pub current_sqrt_price: U256,
    pub cached_upper_initialized_tick: Option<TickData>,
    pub cached_lower_initialized_tick: Option<TickData>,
}

#[derive(Debug, Clone)]
pub struct OwnersPosition {
    pub owner: String,
    pub lower_tick: i32,
    pub upper_tick: i32,
    pub liquidity: i128,
    pub fee_growth_inside_a_last: U256,
    pub fee_growth_inside_b_last: U256,
}

// The liquidity array is a static array where we create 1M  indices (except for testing), to represent form -500k to 500k tick range.
// Each index contains the TickData where u have tick, fee growth, net liquidity.
impl LiquidityArray {
    pub fn new(min_tick: i32, max_tick: i32, tick_spacing: i32, fee_rate: i16) -> Self {
        let size = ((max_tick - min_tick) as usize) + 1; // +1 due to arr nature
        let mut data = Vec::with_capacity(size);
        let mut current_tick = min_tick;

        for _ in 0..size {
            data.push(TickData {
                net_liquidity: 0,
                tick: current_tick,
                is_initialized: false,
                fee_growth_outside_a: U256::zero(),
                fee_growth_outside_b: U256::zero(),
            });

            current_tick += 1;
        }

        LiquidityArray {
            data,
            positions: HashMap::new(),
            min_tick,
            fee_rate,
            tick_spacing,
            current_tick: 0,
            active_liquidity: U256::zero(),
            fee_growth_global_a: U256::zero(),
            fee_growth_global_b: U256::zero(),
            current_sqrt_price: U256::zero(),
            cached_lower_initialized_tick: None,
            cached_upper_initialized_tick: None,
        }
    }

    pub fn get_index(&self, tick: i32) -> usize {
        // Offset the tick by min_tick to get a positive index
        ((tick - self.min_tick) as usize).clamp(0, self.data.len() - 1)
    }

    pub fn get_next_initialized_tick(
        &self,
        tick: i32,
        direction_up: bool,
    ) -> Result<TickData, LiquidityArrayError> {
        // the current_tick is used up so we need to +1/-1
        let start_index = if direction_up {
            self.get_index(tick) + 1
        } else {
            self.get_index(tick) - 1
        };

        let end_index = if direction_up { self.data.len() } else { 0 };

        if direction_up {
            // range here goes from mid to upper end.
            for i in start_index..end_index {
                if self.data[i].is_initialized {
                    return Ok(self.data[i]);
                }
            }
        } else {
            // range here goes from mid to lower end. reverse loop sinc we start from start index and go down.
            for i in (0..start_index).rev() {
                if self.data[i].is_initialized {
                    return Ok(self.data[i]);
                }
            }
        }

        Err(LiquidityArrayError::InitializedTickNotFound)
    }

    // TAKE NOTE, FIRST TICK DATA IS UPPER.
    pub fn get_upper_and_lower_ticks(
        &self,
        current_tick: i32,
        direction_up: bool,
    ) -> Result<(TickData, TickData), LiquidityArrayError> {
        let current_init_tick = self.data[self.get_index(current_tick)];

        // curr_tick initialized
        if current_init_tick.is_initialized {
            // The logic here is that if current_tick is initialized, its either at the upper or lower bounds.
            let upper_tick = if direction_up {
                self.get_next_initialized_tick(current_tick, direction_up)?
            } else {
                current_init_tick
            };

            let lower_tick = if direction_up {
                current_init_tick
            } else {
                self.get_next_initialized_tick(current_tick, direction_up)?
            };

            Ok((upper_tick, lower_tick))
        } else {
            Ok((
                self.get_next_initialized_tick(current_tick, true)?,
                self.get_next_initialized_tick(current_tick, false)?,
            ))
        }
    }

    // ALSO initializes/uninitializes ticks.
    // ONLY USED FOR LIQ TRANSACTIONS AND LIVE POSITIONS SET UP.
    pub fn update_liquidity(
        &mut self,
        lower_tick: i32,
        upper_tick: i32,
        liquidity_delta: i128,
        is_increase: bool,
    ) {
        let lower_tick_index = self.get_index(lower_tick);
        let upper_tick_index = self.get_index(upper_tick);

        // Convert liquidity_delta to a signed value
        let signed_liquidity_delta = if is_increase {
            liquidity_delta
        } else {
            -liquidity_delta
        };

        // Update lower tick
        self.data[lower_tick_index].net_liquidity += signed_liquidity_delta;
        self.data[lower_tick_index].is_initialized = self.data[lower_tick_index].net_liquidity != 0;

        // Update upper tick
        self.data[upper_tick_index].net_liquidity -= signed_liquidity_delta;
        self.data[upper_tick_index].is_initialized = self.data[upper_tick_index].net_liquidity != 0;

        // Update active liquidity if the current price is within the range
        let in_range = self.current_tick >= lower_tick && self.current_tick < upper_tick;
        if in_range {
            if is_increase {
                self.active_liquidity = self
                    .active_liquidity
                    .checked_add(U256::from(liquidity_delta as u128))
                    .expect("Liquidity overflow");
            } else {
                self.active_liquidity = self
                    .active_liquidity
                    .checked_sub(U256::from(liquidity_delta as u128))
                    .expect("Liquidity underflow");
            }
        }
    }

    pub fn add_owners_position(&mut self, position: OwnersPosition, position_id: String) {
        self.positions.insert(position_id.clone(), position.clone());
        self.update_liquidity(
            position.lower_tick,
            position.upper_tick,
            position.liquidity,
            true,
        );
    }

    pub fn remove_owners_position(
        &mut self,
        position_id: &str,
    ) -> Result<OwnersPosition, LiquidityArrayError> {
        if let Some(position) = self.positions.remove(position_id) {
            self.update_liquidity(
                position.lower_tick,
                position.upper_tick,
                position.liquidity,
                false,
            );

            Ok(position)
        } else {
            Err(LiquidityArrayError::PositionNotFound(
                position_id.to_string(),
            ))
        }
    }

    pub fn collect_fees(&mut self, position_id: &str) -> Result<(U256, U256), LiquidityArrayError> {
        let position = self
            .positions
            .get(position_id)
            .ok_or_else(|| LiquidityArrayError::PositionNotFound(position_id.to_string()))?;

        let (fees_a, fees_b, new_fee_growth_inside_a, new_fee_growth_inside_b) =
            self.calculate_fees_for_position(position)?;

        // update fee growth inside the position.
        if let Some(position) = self.positions.get_mut(position_id) {
            position.fee_growth_inside_a_last = new_fee_growth_inside_a;
            position.fee_growth_inside_b_last = new_fee_growth_inside_b;
        }

        Ok((fees_a, fees_b))
    }

    fn calculate_fees_for_position(
        &self,
        position: &OwnersPosition,
    ) -> Result<(U256, U256, U256, U256), LiquidityArrayError> {
        let lower_tick_index = self.get_index(position.lower_tick);
        let upper_tick_index = self.get_index(position.upper_tick);

        let fee_growth_inside_a =
            self.calculate_fee_growth_inside(lower_tick_index, upper_tick_index, true);
        let fee_growth_inside_b =
            self.calculate_fee_growth_inside(lower_tick_index, upper_tick_index, false);

        let fee_growth_delta_a =
            fee_growth_inside_a.saturating_sub(position.fee_growth_inside_a_last);
        let fee_growth_delta_b =
            fee_growth_inside_b.saturating_sub(position.fee_growth_inside_b_last);

        let fees_a = (U256::from(position.liquidity) * fee_growth_delta_a) / Q128;
        let fees_b = (U256::from(position.liquidity) * fee_growth_delta_b) / Q128;

        Ok((fees_a, fees_b, fee_growth_inside_a, fee_growth_inside_b))
    }

    // The way the fee calculations works is u have global fee growth - lower - upper fee growths (if in range) and only one of those if outside.
    // The logic is if u know all the fees calculated outside of ur range, u know how much is in ur range.
    fn calculate_fee_growth_inside(
        &self,
        lower_tick_index: usize,
        upper_tick_index: usize,
        is_token_a: bool,
    ) -> U256 {
        let global_fee_growth = if is_token_a {
            self.fee_growth_global_a
        } else {
            self.fee_growth_global_b
        };

        let lower_tick = self.data[lower_tick_index];
        let upper_tick = self.data[upper_tick_index];

        let lower_fee_growth_outside = if is_token_a {
            lower_tick.fee_growth_outside_a
        } else {
            lower_tick.fee_growth_outside_b
        };

        let upper_fee_growth_outside = if is_token_a {
            upper_tick.fee_growth_outside_a
        } else {
            upper_tick.fee_growth_outside_b
        };

        // The fees are dynamically updated every time we cross a tick to reflect the fee_growth_outside, so they can be used to get the full picture of fees from the position.
        if self.current_tick >= upper_tick.tick {
            // Position is entirely below the current tick
            upper_fee_growth_outside.saturating_sub(lower_fee_growth_outside)
        } else if self.current_tick < lower_tick.tick {
            // Position is entirely above the current tick
            lower_fee_growth_outside.saturating_sub(upper_fee_growth_outside)
        } else {
            global_fee_growth
                .saturating_sub(lower_fee_growth_outside)
                .saturating_sub(upper_fee_growth_outside)
        }
    }

    // is_sell represents the directional movement of token_a. In SOL/USDC case is_sell represents selling SOL for USDC.
    // High level explanation: we use active liquidity as our main liquidity nmr. we use initialized ticks as our ranges for how much can be swapped. after crossing we update liq/feegrowth etc.
    pub fn simulate_swap(
        &mut self,
        amount_in: U256,
        is_sell: bool,
    ) -> Result<U256, LiquidityArrayError> {
        let mut current_tick = self.current_tick;
        let mut current_sqrt_price = self.current_sqrt_price;

        let mut remaining_amount = amount_in;
        let mut amount_out = U256::zero();

        while remaining_amount > U256::zero() {
            let liquidity = self.active_liquidity;

            let upper_initialized_tick_data = self.cached_upper_initialized_tick.unwrap();
            let lower_initialized_tick_data = self.cached_lower_initialized_tick.unwrap();

            let lower_sqrt_price = tick_to_sqrt_price_u256(lower_initialized_tick_data.tick);
            let upper_sqrt_price = tick_to_sqrt_price_u256(upper_initialized_tick_data.tick);

            let max_in = if is_sell {
                let (amount_a_in_range, _) = calculate_amounts(
                    liquidity,
                    lower_sqrt_price,
                    lower_sqrt_price,
                    current_sqrt_price,
                );
                amount_a_in_range
            } else {
                let (_, amount_b_in_range) = calculate_amounts(
                    liquidity,
                    upper_sqrt_price,
                    current_sqrt_price,
                    upper_sqrt_price,
                );
                amount_b_in_range
            };

            let crossing_tick = remaining_amount > max_in;

            // Apply fee logic before the main swap calculation
            let step_amount = if crossing_tick {
                max_in
            } else {
                remaining_amount
            };
            let step_fee = (step_amount * self.fee_rate) / 1_000_000;
            let step_amount_net = step_amount - step_fee;

            if !crossing_tick {
                let old_sqrt_price = current_sqrt_price;
                let new_sqrt_price = calculate_new_sqrt_price(
                    current_sqrt_price,
                    liquidity,
                    step_amount_net,
                    is_sell,
                );

                let (old_amount_a, old_amount_b) = calculate_amounts(
                    liquidity,
                    old_sqrt_price,
                    lower_sqrt_price,
                    upper_sqrt_price,
                );
                let (new_amount_a, new_amount_b) = calculate_amounts(
                    liquidity,
                    new_sqrt_price,
                    lower_sqrt_price,
                    upper_sqrt_price,
                );

                if is_sell {
                    amount_out += new_amount_b.abs_diff(old_amount_b);
                    self.fee_growth_global_a += (step_fee * Q128) / liquidity;
                } else {
                    amount_out += new_amount_a.abs_diff(old_amount_a);
                    self.fee_growth_global_b += (step_fee * Q128) / liquidity;
                }

                current_sqrt_price = new_sqrt_price;
                remaining_amount = U256::zero();
            } else {
                // Swap will cross into the next tick
                let mut relevant_tick: TickData;
                let fee_growth = (step_fee * Q128) / liquidity;

                if is_sell {
                    current_tick = lower_initialized_tick_data.tick;
                    current_sqrt_price = lower_sqrt_price;
                    relevant_tick = lower_initialized_tick_data;

                    self.fee_growth_global_a += fee_growth;

                    // Update fee growth outside for the crossed tick
                    relevant_tick.fee_growth_outside_a =
                        self.fee_growth_global_a - relevant_tick.fee_growth_outside_a;
                    relevant_tick.fee_growth_outside_b =
                        self.fee_growth_global_b - relevant_tick.fee_growth_outside_b;

                    let (_, amount_b) = calculate_amounts(
                        liquidity,
                        current_sqrt_price,
                        lower_sqrt_price,
                        current_sqrt_price,
                    );
                    amount_out += amount_b;

                    if relevant_tick.net_liquidity > 0 {
                        self.active_liquidity -= U256::from(relevant_tick.net_liquidity as u128);
                    } else {
                        self.active_liquidity +=
                            U256::from(relevant_tick.net_liquidity.unsigned_abs());
                    }

                    self.cached_upper_initialized_tick = Some(lower_initialized_tick_data);
                    self.cached_lower_initialized_tick =
                        Some(self.get_next_initialized_tick(current_tick, false)?);
                } else {
                    current_tick = upper_initialized_tick_data.tick;
                    current_sqrt_price = upper_sqrt_price;
                    relevant_tick = upper_initialized_tick_data;

                    self.fee_growth_global_b += fee_growth;

                    // Update fee growth outside for the crossed tick
                    relevant_tick.fee_growth_outside_a =
                        self.fee_growth_global_a - relevant_tick.fee_growth_outside_a;
                    relevant_tick.fee_growth_outside_b =
                        self.fee_growth_global_b - relevant_tick.fee_growth_outside_b;

                    let (amount_a, _) = calculate_amounts(
                        liquidity,
                        current_sqrt_price,
                        current_sqrt_price,
                        upper_sqrt_price,
                    );
                    amount_out += amount_a;

                    if relevant_tick.net_liquidity > 0 {
                        self.active_liquidity += U256::from(relevant_tick.net_liquidity as u128);
                    } else {
                        self.active_liquidity -=
                            U256::from(relevant_tick.net_liquidity.unsigned_abs());
                    }

                    self.cached_upper_initialized_tick =
                        Some(self.get_next_initialized_tick(current_tick, true)?);
                    self.cached_lower_initialized_tick = Some(upper_initialized_tick_data);
                }

                let index = self.get_index(relevant_tick.tick);
                self.data[index] = relevant_tick;

                remaining_amount -= step_amount;
            }
        }

        self.current_tick = current_tick;
        self.current_sqrt_price = current_sqrt_price;
        Ok(amount_out)
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::core_math::{calculate_liquidity, price_to_tick};

    use super::*;

    fn setup_liquidity_array(
        price: u128,
        decimal_diff: i16,
        amount_a: u128,
        amount_b: u128,
    ) -> LiquidityArray {
        let mut array = LiquidityArray::new(-30000, 30000, 2, 300);
        let current_tick = price_to_tick(price as f64 / 10f64.powi(decimal_diff as i32));

        array.current_tick = current_tick;
        array.current_sqrt_price = tick_to_sqrt_price_u256(array.current_tick);

        let lower_tick = -21204 - 3000;
        let upper_tick = -21204 + 3000;

        let liquidity_1 = calculate_liquidity(
            U256::from(amount_a * 10_u128.pow(9)),
            U256::from(amount_b * 10_u128.pow(6)),
            array.current_sqrt_price,
            tick_to_sqrt_price_u256(lower_tick),
            tick_to_sqrt_price_u256(upper_tick),
        );

        array.update_liquidity(lower_tick, upper_tick, liquidity_1.as_u128() as i128, true);

        // for test_get_upper_and_lower_tick
        array.update_liquidity(current_tick - 5, current_tick + 5, 20 as i128, true);

        let (upper_tick_data, lower_tick_data) =
            array.get_upper_and_lower_ticks(current_tick, true).unwrap();
        array.cached_lower_initialized_tick = Some(lower_tick_data);
        array.cached_upper_initialized_tick = Some(upper_tick_data);

        array
    }

    #[test]
    fn test_get_upper_and_lower_ticks() {
        let price = 120;
        let dec_diff = 3;
        let current_tick = price_to_tick(price as f64 / 10f64.powi(dec_diff as i32));

        let mut array = setup_liquidity_array(price, dec_diff, 5, 5 * 120);

        // u can see liquidity setup fn.
        let current_tick = current_tick + 5;
        array.current_tick = current_tick;

        // since current tick == upper tick the upper should be current_tick and lower should be -10 from curr_tick. direction down.
        let (upper_tick, lower_tick) = array
            .get_upper_and_lower_ticks(current_tick, false)
            .unwrap();

        assert!(
            upper_tick.tick == current_tick,
            "Upper tick should equal to current tick"
        );
        assert!(
            lower_tick.tick == current_tick - 10,
            "Lower tick should equal to next lower init tick"
        );

        // u can see liquidity setup fn. we are trying to go direction up. so lower equals curr_tick and higher is +10. direction up.
        let current_tick = current_tick - 10;
        array.current_tick = current_tick;

        let (upper_tick, lower_tick) = array.get_upper_and_lower_ticks(current_tick, true).unwrap();

        assert!(
            upper_tick.tick == current_tick + 10,
            "Upper tick should equal to next upper tick"
        );
        assert!(
            lower_tick.tick == current_tick,
            "Lower tick should equal to current tick"
        );

        // between both so should get 2 new ticks.
        let current_tick = current_tick + 5;
        let (upper_tick, lower_tick) = array.get_upper_and_lower_ticks(current_tick, true).unwrap();

        assert!(
            upper_tick.tick == current_tick + 5,
            "Upper tick should be new"
        );
        assert!(
            lower_tick.tick == current_tick - 5,
            "Lower tick should be new"
        );
    }

    #[test]
    fn test_simulate_swap() {
        // BOTH BUY AND SELL DIRECTIONS.

        let price = 120;
        let dec_diff = 3;

        // in liq array setup we are providing liq with amount a being 9 dec and b being 6 dec.
        let mut array = setup_liquidity_array(price, dec_diff, 5, 5 * 120);
        let starting_price = array.current_sqrt_price;

        array
            .simulate_swap(U256::from(2 * 10_i32.pow(9) as u128), true)
            .unwrap();

        assert!(
            array.current_tick != price_to_tick(price as f64 / 10f64.powi(dec_diff as i32)),
            "Swap should have moved current tick."
        );
        assert!(
            array.current_sqrt_price < starting_price,
            "Price should decrease for a sell"
        );

        let latest_curr_price = array.current_sqrt_price;
        let latest_curr_tick = array.current_tick;

        array
            .simulate_swap(U256::from(2 * 10_i32.pow(6) as u128), false)
            .unwrap();

        assert!(
            array.current_tick == latest_curr_tick,
            "Swap should not have moved current tick."
        );
        assert!(
            array.current_sqrt_price > latest_curr_price,
            "Price should increase for a sell"
        );
    }

    #[test]
    fn test_collect_fees() {
        let price = 120;
        let dec_diff = 3;
        let mut array = setup_liquidity_array(price, dec_diff, 5, 5 * 120);

        // Add Alice's position
        let alice_liquidity = 4_000_000_000_u128;
        array.add_owners_position(
            OwnersPosition {
                owner: "Alice".to_string(),
                lower_tick: array.current_tick - 3000,
                upper_tick: array.current_tick + 3000,
                liquidity: alice_liquidity as i128,
                fee_growth_inside_a_last: U256::zero(),
                fee_growth_inside_b_last: U256::zero(),
            },
            "Alice_position".to_string(),
        );

        // Calculate Alice's liquidity share
        let alice_liquidity_share: f64 =
            alice_liquidity as f64 / array.active_liquidity.as_u128() as f64;

        // Perform a swap (sell direction)
        let swap_amount_a = U256::from(2 * 10_i32.pow(7) as u128);
        array.simulate_swap(swap_amount_a, true).unwrap();

        // Calculate expected fee
        let total_fee = (swap_amount_a * U256::from(array.fee_rate)) / U256::from(1_000_000);
        let expected_fee_a = total_fee.as_u128() as f64 * alice_liquidity_share;

        // Collect fees after first swap
        let (fees_a, fees_b) = array.collect_fees("Alice_position").unwrap();

        // Check token A fees
        let tolerance = U256::from(1000);
        assert!(
            fees_a.abs_diff(U256::from(expected_fee_a as u128)) <= tolerance,
            "Token A fees are not within expected range. Expected: {}, Actual: {}",
            expected_fee_a,
            fees_a
        );
        assert_eq!(
            fees_b,
            U256::zero(),
            "Token B fees should be zero after selling token A. Actual: {}",
            fees_b
        );

        // Perform a second swap (buy direction)
        let swap_amount_b = U256::from(1 * 10_i32.pow(6) as u128);
        array.simulate_swap(swap_amount_b, false).unwrap();

        // Calculate expected fee for second swap
        let total_fee_b = (swap_amount_b * U256::from(array.fee_rate)) / U256::from(1_000_000);
        let expected_fee_b = total_fee_b.as_u128() as f64 * alice_liquidity_share;

        // Collect fees after second swap
        let (fees_a_2, fees_b_2) = array.collect_fees("Alice_position").unwrap();

        // Check token B fees
        assert!(
            fees_b_2.abs_diff(U256::from(expected_fee_b as u128)) <= tolerance,
            "Token B fees are not within expected range. Expected: {}, Actual: {}",
            expected_fee_b,
            fees_b_2
        );
        assert_eq!(
            fees_a_2,
            U256::zero(),
            "Token A fees should be zero after second collection. Actual: {}",
            fees_a_2
        );

        // Verify that fees can't be collected again
        let (fees_a_3, fees_b_3) = array.collect_fees("Alice_position").unwrap();
        assert_eq!(
            fees_a_3,
            U256::zero(),
            "Should not be able to collect token A fees again"
        );
        assert_eq!(
            fees_b_3,
            U256::zero(),
            "Should not be able to collect token B fees again"
        );
    }
}
