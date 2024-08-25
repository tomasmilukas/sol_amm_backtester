use crate::api::positions_api::PositionsApi;
use crate::models::positions_model::PositionModel;
use crate::repositories::pool_repo::PoolRepo;
use crate::repositories::positions_repo::PositionsRepo;
use anyhow::{Context, Result};

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
        let positions = self
            .api
            .get_positions(pool_address)
            .await
            .context("Failed to get positions data")?;

        // Start a transaction
        let mut transaction = self
            .positions_repo
            .begin_transaction()
            .await
            .context("Failed to start transaction")?;

        // Calculate total liquidity
        let total_liquidity: u128 = positions.iter().map(|p| p.liquidity).sum();

        // Update pool liquidity
        self.pool_repo
            .update_liquidity(pool_address, total_liquidity)
            .await
            .context("Failed to update pool liquidity")?;

        // Upsert positions within the transaction
        for position in positions {
            self.positions_repo
                .upsert_in_transaction(&mut transaction, pool_address, &position)
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
