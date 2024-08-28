use crate::repositories::{positions_repo::PositionsRepo, transactions_repo::TransactionRepo};
use crate::utils::decode::decode_whirlpool;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};

pub struct TransactionsService {
    tx_repo: TransactionRepo,
    positions_repo: PositionsRepo,
}

impl TransactionsService {
    pub fn new(tx_repo: TransactionRepo, positions_repo: PositionsRepo) -> Self {
        Self {
            tx_repo,
            positions_repo,
        }
    }

    pub async fn update_and_fill_transactions(&self, pool_address: &str) -> Result<()> {
        let position_data = self
            .positions_repo
            .get_positions_by_pool_address(pool_address)
            .await?;

        Ok(())
    }
}
