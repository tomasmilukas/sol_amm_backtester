use anyhow::Result;
use sqlx::postgres::PgPool;

pub async fn initialize_sol_amm_backtester_database(pool: &PgPool) -> Result<()> {
    let statements = [
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
            token_a_vault TEXT NOT NULL,
            token_b_vault TEXT NOT NULL,
            tick_spacing SMALLINT NOT NULL,
            fee_rate SMALLINT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            last_updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,


        r#"
        CREATE TABLE IF NOT EXISTS transactions (
            tx_id BIGSERIAL PRIMARY KEY,
            signature TEXT NOT NULL,
            transaction_type TEXT NOT NULL,
            pool_address TEXT NOT NULL REFERENCES pools(address),
            block_time BIGINT NOT NULL,
            block_time_utc TIMESTAMPTZ NOT NULL,
            ready_for_backtesting BOOL NOT NULL,
            data JSONB NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE (signature, pool_address, transaction_type)
        )
        "#,
        "CREATE INDEX IF NOT EXISTS idx_transactions_pool_address ON transactions(pool_address)",
        "CREATE INDEX IF NOT EXISTS idx_transactions_block_time ON transactions(block_time)",
        "CREATE INDEX IF NOT EXISTS idx_transactions_block_time_utc ON transactions(block_time_utc)",


        r#"
        CREATE TABLE IF NOT EXISTS positions (
            id SERIAL PRIMARY KEY,
            address TEXT NOT NULL,
            pool_address TEXT NOT NULL REFERENCES pools(address),
            liquidity TEXT NOT NULL,
            tick_lower INTEGER NOT NULL,
            tick_upper INTEGER NOT NULL,
            version INTEGER NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        )
        "#,
    ];

    for statement in statements.iter() {
        sqlx::query(statement).execute(pool).await?;
    }

    Ok(())
}
