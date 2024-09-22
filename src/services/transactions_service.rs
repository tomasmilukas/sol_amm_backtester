use std::collections::HashMap;

use crate::models::positions_model::LivePositionModel;
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
        // any version works, so we pick the first one, since we just need the tick data.
        let position_data = self
            .positions_repo
            .get_live_positions_by_pool_address_and_version(pool_address, 1)
            .await
            .context("Failed to get positions by pool address")?;

        let mut last_tx_id = 0;
        let batch_size = 5000;

        let position_map: HashMap<String, LivePositionModel> = position_data
            .iter()
            .map(|p| (p.address.clone(), p.clone()))
            .collect();

        loop {
            let transactions = self
                .tx_repo
                .fetch_liquidity_txs_to_update(last_tx_id, batch_size)
                .await
                .map_err(|e| {
                    eprintln!("{}", e);
                    anyhow::anyhow!("Failed to fetch transactions to update: {}", e)
                })?;

            if transactions.is_empty() {
                break; // No more transactions to process
            }

            let updated_transactions: Vec<TransactionModelFromDB> = transactions
                .into_iter()
                .map(|mut tx| {
                    let liquidity_data = match tx.data.to_liquidity_data() {
                        Ok(data) => data,
                        Err(e) => {
                            eprintln!("Error processing transaction: {}", e);
                            return tx; // Skip this transaction since it's not a liquidity transaction
                        }
                    };
                    println!("FULL TX INFO: {:?}", tx);

                    tx.data = self.update_transaction_data(
                        liquidity_data,
                        &position_map,
                        &tx.transaction_type,
                    );

                    tx.ready_for_backtesting = false;
                    tx
                })
                .collect();

            let upserted_count = self
                .tx_repo
                .upsert_liquidity_transactions(&updated_transactions)
                .await
                .context("Failed to upsert updated transactions")?;

            println!("Updated {} transactions", upserted_count);

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
        position_map: &HashMap<String, LivePositionModel>,
        transaction_type: &String,
    ) -> TransactionData {
        let mut updated_data = data.clone();

        // Check if any of the possible_positions exist in the HashMap
        for position_address in &updated_data.possible_positions {
            if let Some(position) = position_map.get(position_address) {
                // Update tick_lower and tick_upper if the position is found
                updated_data.tick_lower = Some(position.tick_lower);
                updated_data.tick_upper = Some(position.tick_upper);
                break; // Assuming we only need to update based on the first matching position
            }
        }

        // Convert the updated data into TransactionData
        updated_data.into_transaction_data(transaction_type)
    }
}
