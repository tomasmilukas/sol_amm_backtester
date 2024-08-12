use crate::models::Pool;
use crate::repositories::PoolRepository;

pub struct PoolService {
    repo: PoolRepository,
}

impl PoolService {
    pub fn new(repo: PoolRepository) -> Self {
        Self { repo }
    }

    pub async fn fetch_and_store_pool_data(&self) -> Result<(), Error> {
        // Fetch data from API
        let raw_data = self.fetch_pool_data().await?;
        
        // Parse the data
        let pool = self.parse_pool_data(raw_data)?;
        
        // Store in repository
        self.repo.insert(&pool).await?;
        
        Ok(())
    }

    pub async fn fetch_and_decode_pool_data(&self, pool_address: &str) -> Result<WhirlpoolData, Box<dyn std::error::Error>> {
        let account_info = pool_api::fetch_pool_data(pool_address).await?;
        
        // Extract the base64 data from the account_info
        let base64_data = account_info["value"]["data"][0].as_str()
            .ok_or("Failed to extract base64 data")?;

        // Decode the data
        self.decode_whirlpool_data(base64_data)
    }

    fn decode_whirlpool_data(&self, base64_data: &str) -> Result<WhirlpoolData, Box<dyn std::error::Error>> {
        let decoded = general_purpose::STANDARD.decode(base64_data)?;
        let mut rdr = Cursor::new(decoded);

        // Skip the anchor discriminator (8 bytes)
        rdr.set_position(8);

        let mut token_mint_a = [0u8; 32];
        rdr.read_exact(&mut token_mint_a)?;

        let mut token_mint_b = [0u8; 32];
        rdr.read_exact(&mut token_mint_b)?;

        let tick_spacing = rdr.read_u16::<LittleEndian>()?;
        
        let mut tick_spacing_seed = [0u8; 2];
        rdr.read_exact(&mut tick_spacing_seed)?;

        let fee_rate = rdr.read_u16::<LittleEndian>()?;
        let protocol_fee_rate = rdr.read_u16::<LittleEndian>()?;
        let liquidity = rdr.read_u128::<LittleEndian>()?;
        let sqrt_price = rdr.read_u128::<LittleEndian>()?;
        let tick_current_index = rdr.read_i32::<LittleEndian>()?;
        let protocol_fee_owed_a = rdr.read_u64::<LittleEndian>()?;
        let protocol_fee_owed_b = rdr.read_u64::<LittleEndian>()?;

        Ok(WhirlpoolData {
            token_mint_a,
            token_mint_b,
            tick_spacing,
            tick_spacing_seed,
            fee_rate,
            protocol_fee_rate,
            liquidity,
            sqrt_price,
            tick_current_index,
            protocol_fee_owed_a,
            protocol_fee_owed_b,
        })
    }
}
