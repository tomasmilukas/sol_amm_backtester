use std::collections::HashMap;

use crate::models::positions_model::PositionModel;
use crate::models::transactions_model::{LiquidityData, TransactionData, TransactionModelFromDB};
use crate::repositories::{positions_repo::PositionsRepo, transactions_repo::TransactionRepo};
use anyhow::{Context, Result};

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
            .await
            .context("Failed to get positions by pool address")?;

        let mut last_tx_id = 0;
        let batch_size = 5000;

        let position_map: HashMap<String, PositionModel> = position_data
            .iter()
            .map(|p| (p.address.clone(), p.clone()))
            .collect();

        loop {
            let transactions = self
                .tx_repo
                .fetch_liquidity_txs_to_update(last_tx_id, batch_size)
                .await
                .context("Failed to fetch transactions to update")?;

            if transactions.is_empty() {
                break; // No more transactions to process
            }

            let updated_transactions: Vec<TransactionModelFromDB> = transactions
                .into_iter()
                .map(|mut tx| {
                    let liquidity_data = match tx.data.to_liquidity_data() {
                        Ok(data) => data,
                        Err(e) => {
                            // Handle the error case. You might want to log it, skip this transaction,
                            // or handle it in some other way depending on your requirements.
                            eprintln!("Error processing transaction: {}", e);
                            return tx; // Skip this transaction if it's not a liquidity transaction
                        }
                    };

                    tx.data = self.update_transaction_data(
                        liquidity_data,
                        &position_map,
                        &tx.transaction_type,
                    );
                    tx.ready_for_backtesting = true;
                    tx
                })
                .collect();

            let upserted_count = self
                .tx_repo
                .upsert_liquidity_transactions(&updated_transactions)
                .await
                .context("Failed to upsert updated transactions")?;

            println!("Upserted {} transactions", upserted_count);

            // Update last_tx_id for the next iteration
            if let Some(last_tx) = updated_transactions.last() {
                last_tx_id = last_tx.tx_id;
            }
        }

        Ok(())
    }

    pub fn update_transaction_data(
        &self,
        data: &LiquidityData,
        position_map: &HashMap<String, PositionModel>,
        transaction_type: &String,
    ) -> TransactionData {
        let mut updated_data = data.clone();

        // Check if any of the possible_positions exist in the HashMap
        for position_address in &updated_data.possible_positions {
            if let Some(position) = position_map.get(position_address) {
                // Update tick_lower and tick_upper if the position is found
                updated_data.tick_lower = Some(position.tick_lower as i32);
                updated_data.tick_upper = Some(position.tick_upper as i32);
                break; // Assuming we only need to update based on the first matching position
            }
        }

        // Convert the updated data into TransactionData
        updated_data.into_transaction_data(transaction_type)
    }
}
