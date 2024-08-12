use serde_json::Value;
use serde::{Deserialize, Serialize};
use dotenv::dotenv;
use std::env;
use base64::{Engine as _, engine::general_purpose};
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::Cursor;

#[derive(Serialize, Deserialize, Debug)]
struct AccountInfo {
    executable: bool,
    lamports: u64,
    owner: String,
    rentEpoch: u64,
    space: u64,
    data: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct ApiResponse {
    jsonrpc: String,
    result: Value,
    id: u64,
}

async fn fetch_pool_data(pool_address: &str) -> Result<Value, Box<dyn std::error::Error>> {
    dotenv().ok(); // Load .env file
    let api_key = env::var("ALCHEMY_API_KEY").expect("ALCHEMY_API_KEY must be set");
    let api_url = env::var("ALCHEMY_API_URL").expect("ALCHEMY_API_URL must be set");

    let url = format!("{}/v2/{}", api_url, api_key);
    
    let client = reqwest::Client::new();
    
    let response = client.post(&url)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "id": 1,
            "jsonrpc": "2.0",
            "method": "getAccountInfo",
            "params": [
                pool_address,
                {
                    "encoding": "jsonParsed"
                }
            ]
        }))
        .send()
        .await?;

    let api_response: ApiResponse = response.json().await?;
    Ok(api_response.result)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool_address = "FpCMFDFGYotvufJ7HrFHsWEiiQCGbkLCtwHiDnh7o28Q";
    match fetch_pool_data(pool_address).await {
        Ok(result) => {
            println!("API Response:");
            println!("{}", serde_json::to_string_pretty(&result)?);
            
            if let Some(value) = result.get("value") {
                if let Some(account_info) = value.get("data") {
                    println!("\nAccount Info Data:");
                    println!("{}", serde_json::to_string_pretty(account_info)?);

                    if let Some(base64_data) = account_info[0].as_str() {
                        let decoded = general_purpose::STANDARD.decode(base64_data)?;
                        let mut rdr = Cursor::new(decoded);
                        rdr.set_position(8);

                        println!("Decoded data: {:?}", rdr);
                    }
                }
            }
        },
        Err(e) => eprintln!("Error: {}", e),
    }
    Ok(())
}
