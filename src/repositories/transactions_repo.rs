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

    pub async fn insert(&self, transaction: &TransactionModel) -> Result<()> {
        transaction.validate()?;

        sqlx::query(
            r#"
            INSERT INTO transactions (signature, pool_address, block_time, slot, transaction_type, data)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (signature) DO NOTHING
            "#
        )
        .bind(&transaction.signature)
        .bind(&transaction.pool_address)
        .bind(transaction.block_time)
        .bind(transaction.slot)
        .bind(&transaction.transaction_type)
        .bind(serde_json::to_value(&transaction.data)?)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn fetch_lowest_block_time(
        &self,
        pool_address: &str,
    ) -> Result<Option<DateTime<Utc>>> {
        let result =
            sqlx::query("SELECT MIN(block_time) FROM transactions WHERE pool_address = $1")
                .bind(pool_address)
                .fetch_one(&self.pool)
                .await?;

        Ok(result.get(0))
    }

    pub async fn fetch_highest_block_time(
        &self,
        pool_address: &str,
    ) -> Result<Option<DateTime<Utc>>> {
        let result =
            sqlx::query("SELECT MAX(block_time) FROM transactions WHERE pool_address = $1")
                .bind(pool_address)
                .fetch_one(&self.pool)
                .await?;

        Ok(result.get(0))
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
                let block_time: DateTime<Utc> = row.get("block_time");
                let slot: i64 = row.get("slot");
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
                    slot,
                    transaction_type,
                    transaction_data,
                );

                transaction.validate()?;
                Ok(transaction)
            })
            .collect()
    }

    pub async fn delete_transactions_before(
        &self,
        pool_address: &str,
        before_time: DateTime<Utc>,
    ) -> Result<u64> {
        let result =
            sqlx::query("DELETE FROM transactions WHERE pool_address = $1 AND block_time < $2")
                .bind(pool_address)
                .bind(before_time)
                .execute(&self.pool)
                .await?;

        Ok(result.rows_affected())
    }
}
