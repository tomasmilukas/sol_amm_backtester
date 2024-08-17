use anyhow::Result;
use bs58;
use byteorder::{LittleEndian, ReadBytesExt};
use std::fmt;
use std::io::{Cursor, Read};

use crate::models::pool_model::Whirlpool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pubkey([u8; 32]);

impl Pubkey {
    pub fn new(bytes: [u8; 32]) -> Self {
        Pubkey(bytes)
    }

    pub fn to_base58(&self) -> String {
        bs58::encode(&self.0).into_string()
    }
}

impl fmt::Display for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_base58())
    }
}

impl From<[u8; 32]> for Pubkey {
    fn from(bytes: [u8; 32]) -> Self {
        Pubkey::new(bytes)
    }
}

pub fn read_pubkey(rdr: &mut Cursor<&[u8]>) -> Result<Pubkey> {
    let mut buf = [0u8; 32];
    rdr.read_exact(&mut buf)?;
    Ok(Pubkey::from(buf))
}

pub fn decode_whirlpool(data: &[u8]) -> Result<Whirlpool> {
    let mut rdr = Cursor::new(data);

    // Skip the first 8 bytes (discriminator)
    rdr.set_position(8);

    Ok(Whirlpool {
        whirlpools_config: read_pubkey(&mut rdr)?,
        whirlpool_bump: [rdr.read_u8()?],
        tick_spacing: rdr.read_u16::<LittleEndian>()?,
        tick_spacing_seed: [rdr.read_u8()?, rdr.read_u8()?],
        fee_rate: rdr.read_u16::<LittleEndian>()?,
        protocol_fee_rate: rdr.read_u16::<LittleEndian>()?,
        liquidity: rdr.read_u128::<LittleEndian>()?,
        sqrt_price: rdr.read_u128::<LittleEndian>()?,
        tick_current_index: rdr.read_i32::<LittleEndian>()?,
        protocol_fee_owed_a: rdr.read_u64::<LittleEndian>()?,
        protocol_fee_owed_b: rdr.read_u64::<LittleEndian>()?,
        token_mint_a: read_pubkey(&mut rdr)?,
        token_vault_a: read_pubkey(&mut rdr)?,
        fee_growth_global_a: rdr.read_u128::<LittleEndian>()?,
        token_mint_b: read_pubkey(&mut rdr)?,
        token_vault_b: read_pubkey(&mut rdr)?,
        fee_growth_global_b: rdr.read_u128::<LittleEndian>()?,
    })
}
