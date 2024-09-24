use std::{collections::HashMap, thread, time::Duration};

use crate::utils::{
    error::LiquidityArrayError,
    price_calcs::{
        calculate_amounts, calculate_new_sqrt_price, tick_to_sqrt_price_u256, Q128, U256,
    },
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
    pub fn update_liquidity(
        &mut self,
        lower_tick: i32,
        upper_tick: i32,
        net_liquidity: i128,
        is_increase: bool,
    ) {
        let lower_tick_index = self.get_index(lower_tick);
        let upper_tick_index = self.get_index(upper_tick);

        if is_increase {
            // if price moves down we minus both values (so the upper gets added and lower subtracts)
            // if price moves up we add both values (so the upper gets removed and lower added)
            self.data[lower_tick_index].net_liquidity += net_liquidity;
            self.data[upper_tick_index].net_liquidity -= net_liquidity;

            // always true for increasing liq.
            self.data[lower_tick_index].is_initialized = true;
            self.data[upper_tick_index].is_initialized = true;
        } else {
            // opposite for removing liquidity
            self.data[lower_tick_index].net_liquidity -= net_liquidity;
            self.data[upper_tick_index].net_liquidity += net_liquidity;

            // keep tick initialized if not 0 (aka uninitialized when 0)
            self.data[lower_tick_index].is_initialized =
                self.data[lower_tick_index].net_liquidity != 0;
            self.data[upper_tick_index].is_initialized =
                self.data[upper_tick_index].net_liquidity != 0;
        }

        // this range logic comes from here: https://github.com/Uniswap/v3-core/blob/main/contracts/UniswapV3Pool.sol#L328
        let in_range = self.current_tick >= lower_tick && self.current_tick < upper_tick;

        if in_range && is_increase {
            // If in range, add it to active liquidity
            self.active_liquidity += U256::from(net_liquidity)
        } else if in_range && !is_increase {
            // in range but decrease liquidity (mainl from txs)
            self.active_liquidity -= U256::from(net_liquidity)
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

        let lower_fee_growth_outside = if is_token_a {
            self.data[lower_tick_index].fee_growth_outside_a
        } else {
            self.data[lower_tick_index].fee_growth_outside_b
        };

        let upper_fee_growth_outside = if is_token_a {
            self.data[upper_tick_index].fee_growth_outside_a
        } else {
            self.data[upper_tick_index].fee_growth_outside_b
        };

        // The fees are dynamically updated every time we cross a tick to reflect the fee_growth_otuside, so they can be used to get the full picture of fees from the position.
        if self.current_tick >= upper_tick_index as i32 {
            global_fee_growth.saturating_sub(upper_fee_growth_outside)
        } else if self.current_tick < lower_tick_index as i32 {
            global_fee_growth.saturating_sub(lower_fee_growth_outside)
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

        // Calculate amount after fees at the top
        let fee_amount = amount_in * self.fee_rate / 10000;
        let mut remaining_fee = fee_amount;

        let mut remaining_amount = amount_in - fee_amount;
        let mut amount_out = U256::zero();

        while remaining_amount > U256::zero() {
            // thread::sleep(Duration::from_millis(20));

            let liquidity = self.active_liquidity;

            // is_sell == direction_down not up, thus reverse
            let (upper_initialized_tick_data, lower_initialized_tick_data) =
                self.get_upper_and_lower_ticks(current_tick, !is_sell)?;

            let lower_sqrt_price = tick_to_sqrt_price_u256(lower_initialized_tick_data.tick);
            let upper_sqrt_price = tick_to_sqrt_price_u256(upper_initialized_tick_data.tick);

            let max_in = if is_sell {
                // Token_a are tokens on the upper side of current price. The logic here is that we calculate amount of liquidity by going from lower to current, which is the same as from curr to lower.
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

            // fee_remaining_amount has to be correctly adjusted to distribute at separate ticks.
            let step_amount = std::cmp::min(remaining_amount, max_in);
            let step_fee = (remaining_fee * step_amount) / remaining_amount;
            let fee_growth = (step_fee * Q128) / liquidity;

            // Stay within the initialized range.
            if remaining_amount <= max_in {
                let old_sqrt_price = current_sqrt_price;
                let new_sqrt_price = calculate_new_sqrt_price(
                    current_sqrt_price,
                    liquidity,
                    remaining_amount,
                    is_sell,
                );

                // Use old and new amounts to get amount_out correct.
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
                } else {
                    amount_out += new_amount_a.abs_diff(old_amount_a);
                }

                if is_sell {
                    self.fee_growth_global_a += fee_growth;
                } else {
                    self.fee_growth_global_b += fee_growth;
                }

                current_sqrt_price = new_sqrt_price;
                break;
            } else {
                // Cross the tick range and calculate new active liquidity and new fee_growth_outside for upper or lower tick.
                remaining_amount -= max_in;

                // Also deduct remaining_fee to correctly adjust the fee calculations.
                remaining_fee -= step_fee;

                let mut relevant_tick: TickData;

                if is_sell {
                    current_tick = lower_initialized_tick_data.tick;
                    current_sqrt_price = lower_sqrt_price;
                    relevant_tick = lower_initialized_tick_data;

                    // Update fee growth global
                    self.fee_growth_global_a += fee_growth;

                    // fee growth
                    relevant_tick.fee_growth_outside_a =
                        self.fee_growth_global_a - relevant_tick.fee_growth_outside_a;

                    // amount_out, since we swapping a we get b amount out.
                    let (_, amount_b) = calculate_amounts(
                        liquidity,
                        current_sqrt_price,
                        lower_sqrt_price,
                        current_sqrt_price,
                    );
                    amount_out += amount_b;

                    // manually add/subtract since its unsigned int.
                    if relevant_tick.net_liquidity > 0 {
                        self.active_liquidity -= U256::from(relevant_tick.net_liquidity as u128)
                    } else {
                        self.active_liquidity +=
                            U256::from(relevant_tick.net_liquidity.unsigned_abs())
                    }
                } else {
                    current_tick = upper_initialized_tick_data.tick;
                    current_sqrt_price = upper_sqrt_price;
                    relevant_tick = upper_initialized_tick_data;

                    // Update fee growth global
                    self.fee_growth_global_b += fee_growth;
                    relevant_tick.fee_growth_outside_b =
                        self.fee_growth_global_b - relevant_tick.fee_growth_outside_b;

                    // amount_out, since we swapping b we get a amount out.
                    let (amount_a, _) = calculate_amounts(
                        liquidity,
                        current_sqrt_price,
                        current_sqrt_price,
                        upper_sqrt_price,
                    );

                    amount_out += amount_a;

                    if relevant_tick.net_liquidity > 0 {
                        self.active_liquidity += U256::from(relevant_tick.net_liquidity as u128)
                    } else {
                        self.active_liquidity -=
                            U256::from(relevant_tick.net_liquidity.unsigned_abs())
                    }
                }

                // Update initialized tick
                let index = self.get_index(relevant_tick.tick);
                self.data[index] = relevant_tick;
            }
        }

        self.current_tick = current_tick;
        self.current_sqrt_price = current_sqrt_price;
        Ok(amount_out)
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::price_calcs::{calculate_liquidity, price_to_tick};

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
        let total_fee = (swap_amount_a * U256::from(array.fee_rate)) / U256::from(10000);
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
        let total_fee_b = (swap_amount_b * U256::from(array.fee_rate)) / U256::from(10000);
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
