use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TokenMetadata {
    pub symbol: String,
    pub decimals: i64,
}
