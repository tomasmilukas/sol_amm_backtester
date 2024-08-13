mod api;
mod backtester;
mod config;
mod repositories;
mod service;
mod utils;

use crate::config::Config;
use dotenv::dotenv;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok(); // Load .env file if present
    let config = Config::new()?;

    Ok(())
}
