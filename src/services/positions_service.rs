use crate::models::positions_model::LivePositionModel;
use crate::models::transactions_model::TransactionModelFromDB;
use crate::repositories::positions_repo::PositionsRepo;
use crate::{api::positions_api::PositionsApi, repositories::transactions_repo::TransactionRepo};
use anyhow::{anyhow, Context, Result};

pub struct PositionsService {
    positions_repo: PositionsRepo,
    api: PositionsApi,
}

impl PositionsService {
    pub fn new(positions_repo: PositionsRepo, api: PositionsApi) -> Self {
        Self {
            positions_repo,
            api,
        }
    }

    pub async fn fetch_and_store_positions_data(&self, pool_address: &str) -> Result<()> {
        let positions = self
            .api
            .get_positions(pool_address)
            .await
            .context("Failed to get positions data")?;

        // Get the latest version for the pool and increment it
        let latest_version = self
            .positions_repo
            .get_latest_version_for_live_pool(pool_address)
            .await
            .context("Failed to get latest version for pool")?;
        let new_version = latest_version + 1;

        // Start a transaction
        let mut transaction = self
            .positions_repo
            .begin_transaction()
            .await
            .context("Failed to start transaction")?;

        // Upsert positions within the transaction
        for position in positions {
            self.positions_repo
                .upsert_in_transaction(&mut transaction, pool_address, &position, new_version)
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

    pub async fn get_live_position_data_for_transaction(
        &self,
        tx_repo: TransactionRepo,
        pool_address: &str,
        latest_tx: TransactionModelFromDB,
    ) -> Result<(Vec<LivePositionModel>, TransactionModelFromDB)> {
        let mut current_version = self
            .positions_repo
            .get_latest_version_for_live_pool(pool_address)
            .await
            .context("Failed to get latest version")?;

        let mut latest_positions = None;

        while current_version >= 1 {
            let positions = self
                .positions_repo
                .get_live_positions_by_pool_address_and_version(pool_address, current_version)
                .await
                .context("Failed to get positions for version")?;

            if positions.is_empty() {
                current_version -= 1;
                continue;
            }

            latest_positions = Some(positions.clone());

            let position_timestamp = positions.iter().map(|p| p.created_at).max().unwrap();

            if latest_tx.block_time_utc >= position_timestamp {
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
        if let Some(latest_positions) = latest_positions {
            let latest_position_timestamp =
                latest_positions.iter().map(|p| p.created_at).max().unwrap();

            println!("WARNING: Data gap detected. Latest transaction timestamp: {}, Earliest position timestamp: {}. Will proceed anyways.",
                     latest_tx.block_time_utc, latest_position_timestamp);

            Ok((latest_positions, latest_tx))
        } else {
            Err(anyhow!("No positions found for the given pool address"))
        }
    }
}
