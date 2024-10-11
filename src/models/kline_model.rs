#[allow(dead_code)]
pub struct KlineModel {
    pub open_time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub close_time: i64,
    pub quote_asset_volume: f64,
    pub number_of_trades: u64,
    pub taker_buy_base_asset_volume: f64,
    pub taker_buy_quote_asset_volume: f64,
}

impl KlineModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        open_time: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: f64,
        close_time: i64,
        quote_asset_volume: f64,
        number_of_trades: u64,
        taker_buy_base_asset_volume: f64,
        taker_buy_quote_asset_volume: f64,
    ) -> Self {
        Self {
            open_time,
            open,
            high,
            low,
            close,
            volume,
            close_time,
            quote_asset_volume,
            number_of_trades,
            taker_buy_base_asset_volume,
            taker_buy_quote_asset_volume,
        }
    }
}
