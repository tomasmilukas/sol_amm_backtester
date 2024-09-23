use std::collections::HashMap;

use crate::api::transactions_api::{SignatureInfo, TransactionApi};
use crate::models::positions_model::{ClosedPositionModel, LivePositionModel};
use crate::models::transactions_model::TransactionModelFromDB;
use crate::repositories::{positions_repo::PositionsRepo, transactions_repo::TransactionRepo};
use crate::utils::decode::{
    decode_hawksight_open_position_data, decode_open_position_data, find_encoded_inner_instruction,
    OPEN_POSITION_HAWKSIGHT_DISCRIMINANT, OPEN_POSITION_ORCA_STANDARD_DISCRIMINANT,
};
use crate::utils::hawksight_parsing_tx::HawksightParser;
use crate::utils::transaction_utils::{extract_common_data, retry_with_backoff};
use anyhow::{anyhow, Context, Result};
use futures::stream::{self, StreamExt};
use serde_json::Value;

use super::transactions_sync_amm_service::constants;

pub struct TransactionsService {
    tx_repo: TransactionRepo,
    tx_api: TransactionApi,
    positions_repo: PositionsRepo,
}

impl TransactionsService {
    pub fn new(
        tx_repo: TransactionRepo,
        tx_api: TransactionApi,
        positions_repo: PositionsRepo,
    ) -> Self {
        Self {
            tx_repo,
            tx_api,
            positions_repo,
        }
    }

    // Since liquidity transactions dont have the ticks that they provide liquidity at, we need to fetch the openPosition transactions.
    // We fetch the openPosition transactions from the synced closedPositions transactions. After fetching those, we save new openPosition transactions which can be used to fill in that info in update_and_fill_liquidity_transactions.
    // The following below is ONLY FOR ORCA. IF you expand this backtester for raydium and others, it needs to be adjusted.
    pub async fn create_closed_positions_from_txs(&self, pool_address: &str) -> Result<()> {
        let mut last_tx_id = 0;

        loop {
            let closed_position_transactions = self
                .tx_repo
                .get_closed_position_transactions_to_update(pool_address, last_tx_id, 50)
                .await
                .map_err(|e| {
                    eprintln!("{}", e);
                    anyhow::anyhow!("Failed to fetch close positions transactions: {}", e)
                })?;

            if closed_position_transactions.is_empty() {
                break; // No more transactions to process
            }

            let mut open_position_signatures: Vec<SignatureInfo> = Vec::new();
            let mut closed_position_ids: Vec<i64> = Vec::new();

            for closed_position_tx in &closed_position_transactions {
                let key_position_address = closed_position_tx
                    .data
                    .to_close_position_data()?
                    .position_address
                    .clone();

                if let Some(first_signature) = self
                    .fetch_first_signature_for_position(&key_position_address)
                    .await?
                {
                    open_position_signatures.push(first_signature);
                    closed_position_ids.push(closed_position_tx.tx_id);
                }
            }

            let signature_chunks: Vec<Vec<String>> = open_position_signatures
                .chunks(constants::TX_BATCH_SIZE)
                .map(|chunk| chunk.iter().map(|sig| sig.signature.clone()).collect())
                .collect();

            let fetch_futures = signature_chunks.into_iter().map(|chunk| {
                let chunk_clone = chunk.clone();
                async move {
                    retry_with_backoff(
                        || self.tx_api.fetch_transaction_data(&chunk_clone),
                        constants::MAX_RETRIES,
                        constants::BASE_DELAY,
                        constants::MAX_DELAY,
                    )
                    .await
                    .map_err(|e| anyhow!("Failed to fetch transaction data: {:?}", e))
                }
            });

            let all_tx_data: Vec<Value> = stream::iter(fetch_futures)
                .buffer_unordered(3)
                .flat_map(|result| stream::iter(result.unwrap_or_default()))
                .collect()
                .await;

            // Decode and insert the data into the db.
            let _ = self
                .decode_and_insert_closed_position_data(pool_address, all_tx_data)
                .await;

            // Update ready_for_backtesting flag
            self.tx_repo
                .update_ready_for_backtesting(&closed_position_ids)
                .await
                .context("Failed to update ready_for_backtesting flag")?;

            // Update last_tx_id for the next iteration
            if let Some(last_tx) = closed_position_transactions.last() {
                last_tx_id = last_tx.tx_id;
            }
        }

        Ok(())
    }

    pub async fn decode_and_insert_closed_position_data(
        &self,
        pool_address: &str,
        json_arr: Vec<Value>,
    ) -> Result<()> {
        let mut closed_position_to_insert: Vec<ClosedPositionModel> = Vec::new();

        for tx_data in json_arr {
            let common_data = extract_common_data(&tx_data)?;
            let is_hawksight_tx = HawksightParser::is_hawksight_transaction(&tx_data);

            let encoded_data = if is_hawksight_tx {
                find_encoded_inner_instruction(&tx_data, OPEN_POSITION_HAWKSIGHT_DISCRIMINANT)
            } else {
                find_encoded_inner_instruction(&tx_data, OPEN_POSITION_ORCA_STANDARD_DISCRIMINANT)
            };

            match encoded_data {
                Ok(data) => {
                    let (tick_lower, tick_upper) = if is_hawksight_tx {
                        decode_hawksight_open_position_data(&data)?
                    } else {
                        decode_open_position_data(&data)?
                    };

                    closed_position_to_insert.push(ClosedPositionModel {
                        tick_lower,
                        tick_upper,
                        position_created_at: common_data.block_time_utc,
                        address: if is_hawksight_tx {
                            // in open position transactions in HAWKSIGHT ORCA, the address is always in 6th position
                            common_data.account_keys[5].trim_matches('"').to_string()
                        } else {
                            // in open position transactions in ORCA, the address is always in 4th position
                            common_data.account_keys[3].trim_matches('"').to_string()
                        },
                    });
                }
                Err(_) => {
                    println!(
                        "NO ENCODING LOGIC FOR TRANSACTION: {}",
                        common_data.signature
                    );
                }
            }
        }

        // Start a transaction
        let mut transaction = self
            .positions_repo
            .begin_transaction()
            .await
            .context("Failed to start transaction")?;

        // Upsert positions within the transaction
        for position in closed_position_to_insert {
            self.positions_repo
                .upsert_closed_positions_in_transaction(&mut transaction, pool_address, &position)
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

    async fn fetch_first_signature_for_position(
        &self,
        key_position_address: &str,
    ) -> Result<Option<SignatureInfo>> {
        let mut before: Option<String> = None;
        let limit = 1000;

        loop {
            let signatures = retry_with_backoff(
                || {
                    self.tx_api.fetch_transaction_signatures(
                        key_position_address,
                        limit,
                        before.as_deref(),
                    )
                },
                constants::MAX_RETRIES,
                constants::BASE_DELAY,
                constants::MAX_DELAY,
            )
            .await
            .context("Failed to fetch signatures")?;

            if signatures.is_empty() {
                return Ok(None); // No more signatures found
            }

            // Check if we've reached the oldest signature
            if signatures.len() < limit as usize {
                return Ok(signatures.last().cloned());
            }

            before = signatures.last().map(|sig| sig.signature.clone());
        }
    }

    pub async fn update_and_fill_liquidity_transactions(&self, pool_address: &str) -> Result<()> {
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

                    let mut updated_data = liquidity_data.clone();

                    if let Some(position) = position_map.get(&liquidity_data.position_address) {
                        // Update tick_lower and tick_upper if the position is found
                        updated_data.tick_lower = Some(position.tick_lower);
                        updated_data.tick_upper = Some(position.tick_upper);
                    }

                    // Convert the updated data into TransactionData
                    tx.data = updated_data.into_transaction_data(&tx.transaction_type);
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
}
