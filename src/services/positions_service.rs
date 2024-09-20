use crate::models::positions_model::PositionModel;
use crate::models::transactions_model::TransactionModelFromDB;
use crate::repositories::pool_repo::PoolRepo;
use crate::repositories::positions_repo::PositionsRepo;
use crate::{api::positions_api::PositionsApi, repositories::transactions_repo::TransactionRepo};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};

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

        // Get the latest version for the pool and increment it
        let latest_version = self
            .positions_repo
            .get_latest_version_for_pool(pool_address)
            .await
            .context("Failed to get latest version for pool")?;
        let new_version = latest_version + 1;

        // Upsert positions within the transaction
        for position in positions {
            self.positions_repo
                .upsert_in_transaction(
                    &mut transaction,
                    pool_address,
                    &position,
                    new_version,
                )
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

    pub async fn get_position_data_for_transaction(
        &self,
        tx_repo: TransactionRepo,
        pool_address: &str,
        latest_tx_timestamp: DateTime<Utc>,
    ) -> Result<(Vec<PositionModel>, TransactionModelFromDB)> {
        let mut current_version = self
            .positions_repo
            .get_latest_version_for_pool(pool_address)
            .await
            .context("Failed to get latest version")?;

        while current_version >= 0 {
            let positions = self
                .positions_repo
                .get_positions_by_pool_address_and_version(pool_address, current_version)
                .await
                .context("Failed to get positions for version")?;

            let position_timestamp = positions
                .iter()
                .map(|p| p.created_at)
                .max()
                .ok_or_else(|| anyhow!("No positions found for version {}", current_version))?;

            if latest_tx_timestamp >= position_timestamp {
                // This is the case we want
                let transaction = tx_repo
                    .get_transaction_at_or_after_timestamp(pool_address, position_timestamp)
                    .await
                    .context("Failed to get transaction at or after position timestamp")?;

                return Ok((positions, transaction));
            }

            // If we didn't find a match, decrement the version and try again
            current_version -= 1;
        }

        // If we've gone through all versions and still haven't found a match
        Err(anyhow!("No suitable positions found. The discrepancy between the latest transaction and the earliest positions is too large."))
    }
}
