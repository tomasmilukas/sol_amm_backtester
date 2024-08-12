use dotenv::dotenv;
use crate::config::Config;
use crate::api::pool_api::AlchemyApi;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok(); // Load .env file if present
    let config = Config::new()?;
    let alchemy_api = AlchemyApi::new(config);
    
    // Use alchemy_api in your services...

    Ok(())
}
