use anyhow::Result;
use bs58;
use byteorder::{LittleEndian, ReadBytesExt};
use std::fmt;
use std::io::{Cursor, Read};
use sha2::{Digest, Sha256};

use crate::models::pool_model::Whirlpool;
use crate::models::positions_model::{Position, PositionRewardInfo};

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

pub fn decode_position(data: &[u8]) -> Result<Position> {
    let mut rdr = Cursor::new(data);

    let expected_discriminator = compute_discriminator("Position");

    let mut actual_discriminator = [0u8; 8];
    rdr.read_exact(&mut actual_discriminator)?;

    println!("Expected discriminator: {:?}", expected_discriminator);
    println!("Actual discriminator: {:?}", actual_discriminator);

    if actual_discriminator != expected_discriminator {
        return Err(anyhow::anyhow!("Invalid account discriminator"));
    }

    Ok(Position {
        whirlpool: read_pubkey(&mut rdr)?,
        position_mint: read_pubkey(&mut rdr)?,
        liquidity: rdr.read_u128::<LittleEndian>()?,
        tick_lower_index: rdr.read_i32::<LittleEndian>()?,
        tick_upper_index: rdr.read_i32::<LittleEndian>()?,
        fee_growth_checkpoint_a: rdr.read_u128::<LittleEndian>()?,
        fee_owed_a: rdr.read_u64::<LittleEndian>()?,
        fee_growth_checkpoint_b: rdr.read_u128::<LittleEndian>()?,
        fee_owed_b: rdr.read_u64::<LittleEndian>()?,
        reward_infos: [
            read_position_reward_info(&mut rdr)?,
            read_position_reward_info(&mut rdr)?,
            read_position_reward_info(&mut rdr)?,
        ],
    })
}

fn compute_discriminator(name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update("account:".as_bytes());
    hasher.update(name.as_bytes());
    let result = hasher.finalize();
    let mut discriminator = [0u8; 8];
    discriminator.copy_from_slice(&result[..8]);
    discriminator
}

fn read_position_reward_info(rdr: &mut Cursor<&[u8]>) -> Result<PositionRewardInfo> {
    Ok(PositionRewardInfo {
        growth_inside_checkpoint: rdr.read_u128::<LittleEndian>()?,
        amount_owed: rdr.read_u64::<LittleEndian>()?,
    })
}
