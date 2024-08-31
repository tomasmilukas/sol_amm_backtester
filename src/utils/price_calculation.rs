pub fn tick_to_price(tick: i32) -> f64 {
    let raw_price = 1.0001f64.powi(tick);
    raw_price * 1000.0
}

pub fn price_to_tick(price: f64) -> i32 {
    ((price / 1000.0).ln() / 1.0001f64.ln()).round() as i32
}
