mod api;
mod backtester;
mod config;
mod models;
mod db;
mod repositories;
mod service;
mod utils;

use crate::config::Config;
use api::pool_api::PoolApi;
use dotenv::dotenv;
use repositories::pool_repo::PoolRepo;
use service::pool_service::PoolService;
use sqlx::postgres::PgPoolOptions;
use std::env;
use std::process;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool_address = match env::args().nth(1) {
        Some(address) => address,
        None => {
            eprintln!("Error: Pool address is required.");
            eprintln!("Usage: cargo run -- <pool_address>");
            process::exit(1);
        }
    };

    dotenv().ok();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    let pool_repo = PoolRepo::new(pool);
    let pool_api = PoolApi::new()?;

    let pool_service = PoolService::new(pool_repo, pool_api);

    // let pool_address = "FpCMFDFGYotvufJ7HrFHsWEiiQCGbkLCtwHiDnh7o28Q";
    match pool_service.fetch_and_store_pool_data(&pool_address).await {
        Ok(()) => println!("Pool data fetched and stored successfully"),
        Err(e) => eprintln!("Error: {}", e),
    }

    Ok(())
}
