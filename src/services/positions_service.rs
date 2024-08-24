use crate::api::pool_api::PoolApi;
use crate::api::positions_api::PositionsApi;
use crate::models::pool_model::{PoolModel, Whirlpool};
use crate::repositories::pool_repo::PoolRepo;
use crate::repositories::positions_repo::PositionsRepo;
use crate::utils::decode::decode_whirlpool;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};

pub struct PositionsService {
    positions_repo: PositionsRepo,
    pool_repo: PoolRepo,
    api: PositionsApi,
}

impl PositionsService {
    pub fn new(positions_repo: PositionsRepo, pool_repo: PoolRepo, api: PositionsApi) -> Self {
        Self {
            positions_repo,
            pool_repo,
            api,
        }
    }

    pub async fn fetch_and_store_positions_data(&self, pool_address: &str) -> Result<()> {
        // Fetch positions and metadata
        let positions = self
            .api
            .scrape_positions(pool_address)
            .await
            .context("Failed to scrape positions")?;

        let metadata = self
            .api
            .scrape_metadata(pool_address)
            .await
            .context("Failed to scrape metadata")?;

        // Start a transaction
        let mut transaction = self
            .positions_repo
            .begin_transaction()
            .await
            .context("Failed to start transaction")?;

        let liquidity = metadata
            .liquidity
            .parse::<i64>()
            .context("Failed to parse liquidity as i64")?;

        self.pool_repo
            .update_liquidity_in_transaction(&mut transaction, pool_address, liquidity)
            .await
            .context("Failed to update pool liquidity")?;

        // Upsert positions within the transaction
        for position in positions {
            self.positions_repo
                .upsert_in_transaction(&mut transaction, &position)
                .await
                .with_context(|| format!("Failed to upsert position: {}", position.address))?;
        }

        // Commit the transaction
        transaction
            .commit()
            .await
            .context("Failed to commit transaction")?;

        Ok(())
    }

    pub async fn get_position_data(&self, pool_address: &str) -> Result<Vec<PositionModel>> {
        self.positions_repo
            .get_positions_by_pool_address(pool_address)
            .await
            .context("Failed to get positions for pool address")
    }
}
