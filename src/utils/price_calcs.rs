pub const Q32: u128 = 1u128 << 32;

pub fn tick_to_sqrt_price(tick: i32) -> f64 {
    1.0001f64.powi(tick / 2)
}

pub fn sqrt_price_to_fixed(sqrt_price: f64) -> u128 {
    (sqrt_price * Q32 as f64) as u128
}
