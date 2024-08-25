use crate::api::pool_api::PoolApi;
use crate::models::pool_model::{PoolModel, Whirlpool};
use crate::repositories::pool_repo::PoolRepo;
use crate::utils::decode::decode_whirlpool;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};

use super::transactions_amm_service::AMMPlatforms;

pub struct PoolService {
    repo: PoolRepo,
    api: PoolApi,
}

impl PoolService {
    pub fn new(repo: PoolRepo, api: PoolApi) -> Self {
        Self { repo, api }
    }

    pub async fn fetch_and_store_pool_data(
        &self,
        pool_address: &str,
        pool_platform: AMMPlatforms,
    ) -> Result<()> {
        let whirlpool = self.fetch_and_decode_pool_data(pool_address).await?;

        let pool = self
            .convert_whirlpool_to_pool(pool_address.to_string(), whirlpool)
            .await?;

        self.repo.upsert(&pool, pool_platform).await?;

        Ok(())
    }

    pub async fn fetch_and_decode_pool_data(&self, pool_address: &str) -> Result<Whirlpool> {
        let result = self.api.fetch_pool_data(pool_address).await?;

        let whirlpool = result
            .get("value")
            .and_then(|value| value.get("data"))
            .and_then(|account_info| account_info[0].as_str())
            .map(|base64_data| general_purpose::STANDARD.decode(base64_data))
            .transpose()
            .context("Failed to decode base64 data")?
            .map(|decoded| decode_whirlpool(&decoded))
            .transpose()
            .context("Failed to decode whirlpool data")?
            .ok_or_else(|| anyhow::anyhow!("No valid pool data found"))?;

        Ok(whirlpool)
    }

    async fn convert_whirlpool_to_pool(
        &self,
        pool_address: String,
        whirlpool: Whirlpool,
    ) -> Result<PoolModel> {
        let token_a_address = whirlpool.token_mint_a.to_string();
        let token_b_address = whirlpool.token_mint_b.to_string();

        let token_a_metadata = self.api.fetch_token_metadata(&token_a_address).await?;
        let token_b_metadata = self.api.fetch_token_metadata(&token_b_address).await?;

        let pool = PoolModel::from_whirlpool(
            pool_address.to_string(),
            whirlpool,
            token_a_metadata,
            token_b_metadata,
        )?;

        Ok(pool)
    }

    pub async fn get_pool_data(&self, pool_address: &str) -> Result<PoolModel> {
        self.repo
            .get_pool_by_address(pool_address)
            .await?
            .ok_or_else(|| anyhow!("Pool not found for address: {}", pool_address))
    }
}
