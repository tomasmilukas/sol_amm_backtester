const Q64: u128 = 1u128 << 64;

pub fn tick_to_sqrt_price(tick: i32) -> f64 {
    1.0001f64.powi(tick / 2)
}

pub fn f64_to_fixed_point(f64_nmr: f64) -> u128 {
    (f64_nmr * Q64 as f64).round() as u128
}

pub fn fixed_point_to_f64(fixed_point: u128) -> f64 {
    (fixed_point as f64) / (Q64 as f64)
}
