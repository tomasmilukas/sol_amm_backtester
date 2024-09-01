pub fn get_current_tick(sqrt_price: f64, tick_spacing: i32) -> i32 {
    ((sqrt_price.powi(2).log(1.0001)) as i32 / tick_spacing) * tick_spacing
}

pub fn tick_to_sqrt_price(tick: i32) -> f64 {
    1.0001f64.powi(tick / 2)
}
