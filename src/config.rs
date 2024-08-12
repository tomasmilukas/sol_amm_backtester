use std::env;

pub struct Config {
    pub alchemy_api_url: String,
    pub alchemy_api_key: String,
}

impl Config {
    pub fn new() -> Result<Self, env::VarError> {
        Ok(Self {
            alchemy_api_url: env::var("ALCHEMY_API_URL")?,
            alchemy_api_key: env::var("ALCHEMY_API_KEY")?,
        })
    }
}
