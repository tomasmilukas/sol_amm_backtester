#[derive(Debug, Clone, Copy)]
pub struct TickData {
    lower_tick: i32,
    upper_tick: i32,
    liquidity: i128,
}

#[derive(Debug, Clone, Copy)]
struct PositionData {
    upper_tick: i32,
    lower_tick: i32,
    liquidity: i128,
}

#[derive(Debug)]
struct LiquidityArray {
    data: Vec<TickData>,
    min_tick: i32,
    tick_spacing: i32,
    current_tick: i32,
}
