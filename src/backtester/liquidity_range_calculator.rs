use anyhow::Result;

use crate::models::positions_model::PositionModel;

#[derive(Debug, Clone, Copy)]
pub struct TickData {
    lower_tick: i32,
    upper_tick: i32,
    liquidity: u128,
}

#[derive(Debug, Clone, Copy)]
struct TickSpaceData {
    upper_tick: i32,
    lower_tick: i32,
    liquidity: u128,
}

#[derive(Debug)]
pub struct LiquidityArray {
    data: Vec<TickData>,
    min_tick: i32,
    tick_spacing: i32,
}

// The liquidity array is a static array that usually has 500k indices.
// Each index contains the TickSpaceData where u have the spacing of the tick and the amount of liquidity in it.
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

    fn update_liquidity(&mut self, tick_space_data: TickSpaceData) {
        let lower_tick_index = self.get_index(tick_space_data.lower_tick, false);
        let upper_tick_index = self.get_index(tick_space_data.upper_tick, true);

        let tick_count = upper_tick_index - lower_tick_index;

        // In case a position is providing liquidity in a single tick space, the tick_count needs to be set to zero.
        let final_tick_count = if tick_count == 0 { 1 } else { tick_count };
        let liquidity_per_tick_spacing = tick_space_data.liquidity / (final_tick_count as u128);

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

        let tick_space_data = TickSpaceData {
            lower_tick,
            upper_tick,
            liquidity,
        };

        liquidity_array.update_liquidity(tick_space_data);
    }

    Ok(liquidity_array)
}
