use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgPool;
use sqlx::Row;

use crate::models::transactions_model::{TransactionModel, TransactionModelFromDB};

#[derive(Clone)]
pub struct TransactionRepo {
    pool: PgPool,
}
pub enum OrderDirection {
    Ascending,
    Descending,
}

#[async_trait]
pub trait TransactionRepoTrait {
    async fn fetch_transactions(
        &self,
        pool_address: &str,
        cursor: Option<i64>,
        limit: i64,
        order: OrderDirection,
    ) -> Result<Vec<TransactionModelFromDB>>;
}

#[async_trait]
impl TransactionRepoTrait for TransactionRepo {
    // async fn fetch_highest_tx_swap(
    //     &self,
    //     pool_address: &str,
    // ) -> Result<Option<TransactionModelFromDB>> {
    //     // The most recent transaction is one that starts with tx_id 1 but is a swap
    //     let result = sqlx::query(
    //         r#"
    //         SELECT * FROM transactions
    //         WHERE pool_address = $1 AND transaction_type = 'Swap'
    //         ORDER BY block_time DESC, tx_id DESC
    //         LIMIT 1
    //         "#,
    //     )
    //     .bind(pool_address)
    //     .fetch_optional(&self.pool)
    //     .await?;

    //     result
    //         .map(|row| self.row_to_transaction_model(&row))
    //         .transpose()
    // }

    async fn fetch_transactions(
        &self,
        pool_address: &str,
        cursor: Option<i64>,
        limit: i64,
        order: OrderDirection,
    ) -> Result<Vec<TransactionModelFromDB>> {
        let order_by = match order {
            OrderDirection::Descending => "DESC",
            OrderDirection::Ascending => "ASC",
        };

        let cursor_condition = match (order, cursor) {
            (OrderDirection::Descending, Some(_c)) => "tx_id < $3",
            (OrderDirection::Ascending, Some(_c)) => "tx_id > $3",
            (_, None) => "1=1", // No cursor, fetch from the beginning/end
        };

        let query = format!(
            r#"
            SELECT
                tx_id, signature, pool_address, block_time, block_time_utc,
                transaction_type, ready_for_backtesting, data
            FROM transactions
            WHERE 
                pool_address = $1
                AND ({})
            ORDER BY tx_id {}
            LIMIT $2
            "#,
            cursor_condition, order_by
        );

        let mut q = sqlx::query(&query).bind(pool_address).bind(limit);

        if let Some(c) = cursor {
            q = q.bind(c);
        }

        let rows = q.fetch_all(&self.pool).await?;

        rows.into_iter()
            .map(|row| self.row_to_transaction_model(&row))
            .collect()
    }
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
            ON CONFLICT (signature, pool_address, transaction_type) 
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

    pub async fn fetch_most_recent_swap(
        &self,
        pool_address: &str,
    ) -> Result<Option<TransactionModelFromDB>> {
        // The most recent transaction is one that starts with tx_id 1 but is a swap
        let result = sqlx::query(
            r#"
            SELECT * FROM transactions 
            WHERE pool_address = $1 AND transaction_type = 'Swap'
            ORDER BY block_time DESC, tx_id DESC
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

    pub async fn upsert_liquidity_transactions(
        &self,
        transactions: &Vec<TransactionModelFromDB>,
    ) -> Result<usize> {
        let mut tx = self.pool.begin().await?;

        let mut upserted_count = 0;

        for transaction in transactions {
            let result = sqlx::query(
                r#"
                INSERT INTO transactions (
                    signature, 
                    pool_address, 
                    block_time, 
                    block_time_utc, 
                    transaction_type, 
                    ready_for_backtesting, 
                    data
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (signature, pool_address, transaction_type)
                DO UPDATE SET
                    data = EXCLUDED.data,
                    ready_for_backtesting = EXCLUDED.ready_for_backtesting
                RETURNING tx_id
                "#,
            )
            .bind(&transaction.signature)
            .bind(&transaction.pool_address)
            .bind(transaction.block_time)
            .bind(transaction.block_time_utc)
            .bind(&transaction.transaction_type)
            .bind(transaction.ready_for_backtesting)
            .bind(&serde_json::to_value(&transaction.data)?) // Assuming data is already a Value or can be serialized to JSON
            .execute(&mut tx)
            .await?;

            upserted_count += result.rows_affected() as usize;
        }

        tx.commit().await?;

        Ok(upserted_count)
    }

    pub async fn fetch_liquidity_txs_to_update(
        &self,
        last_tx_id: i64,
        limit: i64,
    ) -> Result<Vec<TransactionModelFromDB>> {
        let rows = sqlx::query(
            r#"
                SELECT
                    tx_id, signature, pool_address, block_time, block_time_utc,
                    transaction_type, ready_for_backtesting, data
                FROM transactions
                WHERE 
                    tx_id > $1 
                    AND ready_for_backtesting = FALSE
                    AND transaction_type IN ('IncreaseLiquidity', 'DecreaseLiquidity')
                ORDER BY tx_id
                LIMIT $2
            "#,
        )
        .bind(last_tx_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            eprintln!("SQL Error: {:?}", e);
            e
        })?;

        rows.into_iter()
            .map(|row| self.row_to_transaction_model(&row))
            .collect()
    }

    pub async fn fetch_lowest_block_time_transaction(
        &self,
        pool_address: &str,
    ) -> Result<Option<TransactionModelFromDB>> {
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
    ) -> Result<Option<TransactionModelFromDB>> {
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

    pub async fn get_transaction_at_or_after_timestamp(
        &self,
        pool_address: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<TransactionModelFromDB> {
        let row = sqlx::query(
            r#"
            SELECT
                tx_id, signature, pool_address, block_time, block_time_utc,
                transaction_type, ready_for_backtesting, data
            FROM transactions
            WHERE 
                pool_address = $1
                AND block_time_utc >= $2
            ORDER BY block_time_utc ASC
            LIMIT 1
            "#,
        )
        .bind(pool_address)
        .bind(timestamp)
        .fetch_one(&self.pool)
        .await?;

        self.row_to_transaction_model(&row)
    }

    pub async fn get_transactions_to_create_closed_positions(
        &self,
        pool_address: &str,
        last_tx_id: i64,
        limit: i64,
    ) -> Result<Vec<TransactionModelFromDB>> {
        let rows = sqlx::query(
            r#"
                SELECT
                    tx_id, signature, pool_address, block_time, block_time_utc,
                    transaction_type, ready_for_backtesting, data
                FROM transactions
                WHERE 
                    tx_id > $1 
                    AND pool_address = $2
                    AND ready_for_backtesting = FALSE
                    AND transaction_type IN ('IncreaseLiquidity', 'DecreaseLiquidity', 'ClosePosition')
                ORDER BY tx_id
                LIMIT $3
            "#,
        )
        .bind(last_tx_id)
        .bind(pool_address)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            eprintln!("SQL Error: {:?}", e);
            e
        })?;

        rows.into_iter()
            .map(|row| self.row_to_transaction_model(&row))
            .collect()
    }

    pub async fn update_ready_for_backtesting(&self, tx_ids: &[i64]) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE transactions
            SET ready_for_backtesting = TRUE
            WHERE tx_id = ANY($1)
            "#,
        )
        .bind(tx_ids)
        .execute(&self.pool)
        .await
        .context("Failed to update ready_for_backtesting flag")?;

        Ok(())
    }

    fn row_to_transaction_model(
        &self,
        row: &sqlx::postgres::PgRow,
    ) -> Result<TransactionModelFromDB> {
        Ok(TransactionModelFromDB {
            tx_id: row.try_get("tx_id").context("Failed to get tx_id")?,
            signature: row.get("signature"),
            pool_address: row.get("pool_address"),
            block_time: row.get("block_time"),
            block_time_utc: row.get("block_time_utc"),
            transaction_type: row.get("transaction_type"),
            ready_for_backtesting: row.get("ready_for_backtesting"),
            data: serde_json::from_value(row.get("data")).context("Failed to deserialize data")?,
        })
    }
}
