use sqlx::postgres::PgPool;
use anyhow::Result;

pub async fn initialize_sol_amm_backtester_database(pool: &PgPool) -> Result<()> {
    let statements = [
        // Create pools table (unchanged)
        r#"
        CREATE TABLE IF NOT EXISTS pools (
            address TEXT PRIMARY KEY,
            platform TEXT NOT NULL,
            name TEXT NOT NULL,
            token_a_name TEXT NOT NULL,
            token_b_name TEXT NOT NULL,
            token_a_address TEXT NOT NULL,
            token_b_address TEXT NOT NULL,
            token_a_decimals SMALLINT NOT NULL,
            token_b_decimals SMALLINT NOT NULL,
            tick_spacing SMALLINT NOT NULL,
            fee_rate SMALLINT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            last_updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
        // Create transactions table (corrected)
        r#"
        CREATE TABLE IF NOT EXISTS transactions (
            signature TEXT PRIMARY KEY,
            pool_address TEXT NOT NULL REFERENCES pools(address),
            block_time BIGINT NOT NULL,
            block_time_utc TIMESTAMPTZ NOT NULL,
            slot BIGINT NOT NULL,
            transaction_type TEXT NOT NULL,
            data JSONB NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
        // Create indexes (unchanged)
        "CREATE INDEX IF NOT EXISTS idx_transactions_pool_address ON transactions(pool_address)",
        "CREATE INDEX IF NOT EXISTS idx_transactions_block_time ON transactions(block_time)",
        "CREATE INDEX IF NOT EXISTS idx_transactions_block_time_utc ON transactions(block_time_utc)",
    ];

    for statement in statements.iter() {
        sqlx::query(statement).execute(pool).await?;
    }

    Ok(())
}
