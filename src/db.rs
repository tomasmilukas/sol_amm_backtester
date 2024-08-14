use sqlx::postgres::PgPool;

pub async fn initialize_amm_backtester_database(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS pools (
            address TEXT PRIMARY KEY,
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
        )",
    )
    .execute(pool)
    .await?;

    Ok(())
}
