pub const Q32: u128 = 1u128 << 32;
pub const Q64: u128 = 1u128 << 64;

pub fn tick_to_sqrt_price(tick: i32) -> f64 {
    1.0001f64.powf(tick as f64 / 2.0)
}

pub fn sqrt_price_to_tick(sqrt_price: f64) -> i32 {
    ((sqrt_price.ln() / 1.0001f64.ln()) * 2.0).floor() as i32
}

pub fn sqrt_price_to_fixed(sqrt_price: f64) -> u128 {
    (sqrt_price * Q32 as f64) as u128
}
