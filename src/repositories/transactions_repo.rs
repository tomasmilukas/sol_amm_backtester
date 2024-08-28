use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::postgres::{PgPool, PgRow};
use sqlx::Row;

use crate::models::transactions_model::{TransactionData, TransactionModel};

pub struct TransactionRepo {
    pool: PgPool,
}

impl TransactionRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, transactions: &[TransactionModel]) -> Result<usize> {
        let mut tx = self.pool.begin().await?;

        let mut inserted_count = 0;

        for transaction in transactions {
            let result = sqlx::query(
            r#"
            INSERT INTO transactions (signature, pool_address, block_time, block_time_utc, transaction_type, ready_for_backtesting, data)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (signature, transaction_type) 
            DO UPDATE SET
                pool_address = EXCLUDED.pool_address,
                block_time = EXCLUDED.block_time,
                block_time_utc = EXCLUDED.block_time_utc,
                data = EXCLUDED.data,
                ready_for_backtesting = EXCLUDED.ready_for_backtesting
            "#
            )
            .bind(&transaction.signature)
            .bind(&transaction.pool_address)
            .bind(transaction.block_time)
            .bind(transaction.block_time_utc)
            .bind(&transaction.transaction_type)
            .bind(transaction.ready_for_backtesting)
            .bind(&serde_json::to_value(&transaction.data)?)
            .execute(&mut tx)
            .await?;

            inserted_count += result.rows_affected() as usize;
        }

        tx.commit().await?;

        Ok(inserted_count)
    }

    pub async fn fetch_lowest_block_time_transaction(
        &self,
        pool_address: &str,
    ) -> Result<Option<TransactionModel>> {
        let result = sqlx::query(
            r#"
            SELECT * FROM transactions 
            WHERE pool_address = $1 
            ORDER BY block_time ASC 
            LIMIT 1
            "#,
        )
        .bind(pool_address)
        .fetch_optional(&self.pool)
        .await?;

        result
            .map(|row| self.row_to_transaction_model(&row))
            .transpose()
    }

    pub async fn fetch_highest_block_time_transaction(
        &self,
        pool_address: &str,
    ) -> Result<Option<TransactionModel>> {
        let result = sqlx::query(
            r#"
            SELECT * FROM transactions 
            WHERE pool_address = $1 
            ORDER BY block_time DESC 
            LIMIT 1
            "#,
        )
        .bind(pool_address)
        .fetch_optional(&self.pool)
        .await?;

        result
            .map(|row| self.row_to_transaction_model(&row))
            .transpose()
    }

    pub async fn fetch_transactions_by_time_range(
        &self,
        pool_address: &str,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Result<Vec<TransactionModel>> {
        let rows = sqlx::query(
            r#"
            SELECT signature, pool_address, block_time, slot, transaction_type, data
            FROM transactions
            WHERE pool_address = $1 AND block_time BETWEEN $2 AND $3
            ORDER BY block_time ASC
            "#,
        )
        .bind(pool_address)
        .bind(start_time)
        .bind(end_time)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row: PgRow| {
                let signature: String = row.get("signature");
                let pool_address: String = row.get("pool_address");
                let block_time: i64 = row.get("block_time");
                let block_time_utc: DateTime<Utc> = row.get("block_time_utc");
                let ready_for_backtesting = row.get("ready_for_backtesting");
                let transaction_type = row.get("transaction_type");
                let data_json: Value = row.get("data");

                let transaction_data: TransactionData = serde_json::from_value(data_json)
                    .with_context(|| {
                        format!(
                            "Failed to deserialize transaction data for signature: {}",
                            signature
                        )
                    })?;

                let transaction = TransactionModel::new(
                    signature,
                    pool_address,
                    block_time,
                    block_time_utc,
                    transaction_type,
                    ready_for_backtesting,
                    transaction_data,
                );

                Ok(transaction)
            })
            .collect()
    }

    fn row_to_transaction_model(&self, row: &sqlx::postgres::PgRow) -> Result<TransactionModel> {
        Ok(TransactionModel {
            signature: row.get("signature"),
            pool_address: row.get("pool_address"),
            block_time: row.get("block_time"),
            block_time_utc: row.get("block_time_utc"),
            transaction_type: row.get("transaction_type"),
            ready_for_backtesting: row.get("ready_for_backtesting"),
            data: serde_json::from_value(row.get("transaction_data"))
                .context("Failed to deserialize transaction_data")?,
        })
    }
}
