use anyhow::{anyhow, Result};
use bs58;
use byteorder::{LittleEndian, ReadBytesExt};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{Cursor, Read};

use crate::models::pool_model::Whirlpool;
use crate::models::positions_model::{Position, PositionRewardInfo};

// These are the first 8 bytes of each instruction's data, encoded in Base58
pub const INCREASE_LIQUIDITY_DISCRIMINANT: &str = "3KLKPPgnNhb";
pub const DECREASE_LIQUIDITY_DISCRIMINANT: &str = "8xY8jsAzTgX";
pub const HAWKSIGHT_SWAP_DISCRIMINANT: &str = "59p8WydnSZt";
pub const OPEN_POSITION_WITH_METADATA_ORCA_STANDARD_DISCRIMINANT: &str = "B3T3AnPs3Bbw";
pub const OPEN_POSITION_ORCA_STANDARD_DISCRIMINANT: &str = "2GrSomweg35m";
pub const OPEN_POSITION_HAWKSIGHT_DISCRIMINANT: &str = "2GrSomweg35m";

#[derive(Debug, PartialEq)]
pub struct IncreaseLiquidityData {
    pub liquidity_amount: u128,
    pub token_max_a: u64,
    pub token_max_b: u64,
}

#[derive(Debug, PartialEq)]
pub struct DecreaseLiquidityData {
    pub liquidity_amount: u128,
    pub token_min_a: u64,
    pub token_min_b: u64,
}

#[derive(Debug, PartialEq)]
pub struct HawksightSwapData {
    pub amount: u64,
    pub other_amount_threshold: u64,
    pub sqrt_price_limit: u128,
    pub amount_specified_is_input: bool,
    pub a_to_b: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pubkey([u8; 32]);

impl Pubkey {
    pub fn new(bytes: [u8; 32]) -> Self {
        Pubkey(bytes)
    }

    pub fn to_base58(self) -> String {
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

pub fn find_encoded_instruction_data(tx_data: &Value, discriminant: &str) -> Result<String> {
    let instructions = tx_data["transaction"]["message"]["instructions"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Instructions not found in transaction data"))?;

    for instruction in instructions {
        if let Some(data) = instruction["data"].as_str() {
            if data.starts_with(discriminant) {
                return Ok(data.to_string());
            }
        }
    }

    Err(anyhow::anyhow!("Encoded instruction data not found"))
}

pub fn decode_increase_liquidity_data(encoded_data: &str) -> Result<IncreaseLiquidityData> {
    // Decode the Base58 string
    let data = bs58::decode(encoded_data).into_vec()?;
    let mut rdr = Cursor::new(data);

    // Skip the first 8 bytes (instruction discriminator)
    rdr.set_position(8);

    // Read the data
    let liquidity_amount = rdr.read_u128::<LittleEndian>()?;
    let token_max_a = rdr.read_u64::<LittleEndian>()?;
    let token_max_b = rdr.read_u64::<LittleEndian>()?;

    Ok(IncreaseLiquidityData {
        liquidity_amount,
        token_max_a,
        token_max_b,
    })
}

pub fn decode_decrease_liquidity_data(encoded_data: &str) -> Result<DecreaseLiquidityData> {
    let data = bs58::decode(encoded_data).into_vec()?;
    let mut rdr = Cursor::new(data);

    rdr.set_position(8);

    let liquidity_amount = rdr.read_u128::<LittleEndian>()?;
    let token_min_a = rdr.read_u64::<LittleEndian>()?;
    let token_min_b = rdr.read_u64::<LittleEndian>()?;

    Ok(DecreaseLiquidityData {
        liquidity_amount,
        token_min_a,
        token_min_b,
    })
}

pub fn find_encoded_inner_instruction(tx_data: &Value, discriminant: &str) -> Result<String> {
    let inner_instructions = tx_data["meta"]["innerInstructions"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Inner instructions not found in transaction data"))?;

    for inner_instruction_group in inner_instructions {
        if let Some(instructions) = inner_instruction_group["instructions"].as_array() {
            for instruction in instructions {
                if let Some(data) = instruction["data"].as_str() {
                    if data.starts_with(discriminant) {
                        return Ok(data.to_string());
                    }
                }
            }
        }
    }

    Err(anyhow::anyhow!("Encoded inner instruction data not found"))
}

pub fn find_encoded_transaction_instruction(tx_data: &Value, discriminant: &str) -> Result<String> {
    let instructions = tx_data["transaction"]["message"]["instructions"]
        .as_array()
        .ok_or_else(|| anyhow!("Instructions not found in transaction data"))?;

    for instruction in instructions {
        if let Some(data) = instruction["data"].as_str() {
            if data.starts_with(discriminant) {
                return Ok(data.to_string());
            }
        }
    }

    Err(anyhow!(
        "Encoded instruction data not found for the given discriminant"
    ))
}

pub fn decode_hawksight_swap_data(encoded_data: &str) -> Result<HawksightSwapData> {
    let data = bs58::decode(encoded_data).into_vec()?;
    let mut rdr = Cursor::new(data);

    rdr.set_position(8);

    let amount = rdr.read_u64::<LittleEndian>()?;
    let other_amount_threshold = rdr.read_u64::<LittleEndian>()?;
    let sqrt_price_limit = rdr.read_u128::<LittleEndian>()?;
    let amount_specified_is_input = rdr.read_u8()? != 0;
    let a_to_b = rdr.read_u8()? != 0;

    Ok(HawksightSwapData {
        amount,
        other_amount_threshold,
        sqrt_price_limit,
        amount_specified_is_input,
        a_to_b,
    })
}

pub fn decode_open_position_with_metadata_data(encoded_data: &str) -> Result<(i32, i32)> {
    // Decode the Base58 string
    let data = bs58::decode(encoded_data).into_vec()?;
    let mut rdr = Cursor::new(data);

    // Skip the first 8 bytes (instruction discriminator)
    rdr.set_position(8);

    // Skip the bumps (2 bytes)
    rdr.set_position(rdr.position() + 2);

    // Read the tick indices
    let tick_lower_index = rdr.read_i32::<LittleEndian>()?;
    let tick_upper_index = rdr.read_i32::<LittleEndian>()?;

    Ok((tick_lower_index, tick_upper_index))
}

pub fn decode_open_position_data(encoded_data: &str) -> Result<(i32, i32)> {
    // Decode the Base58 string
    let data = bs58::decode(encoded_data).into_vec()?;
    let mut rdr = Cursor::new(data);

    // Skip the first 8 bytes (instruction discriminator)
    rdr.set_position(8);

    // Skip the bumps (2 bytes)
    rdr.set_position(rdr.position() + 2);

    // Read the tick indices
    let tick_lower_index = rdr.read_i32::<LittleEndian>()?;
    let tick_upper_index = rdr.read_i32::<LittleEndian>()?;

    Ok((tick_lower_index, tick_upper_index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_increase_liquidity_data() {
        let encoded_data = "3KLKPPgnNhbLEPrG4SnAHuz32CMyh9PqNtR4MvzWpxA9qgnNNVYKU6K";
        let result = decode_increase_liquidity_data(encoded_data);

        assert!(result.is_ok());

        let decoded = result.unwrap();
        let expected = IncreaseLiquidityData {
            liquidity_amount: 761851408,
            token_max_a: 374597936,
            token_max_b: 1230032,
        };

        assert_eq!(decoded, expected);

        println!("Decoded IncreaseLiquidity Data:");
        println!("Liquidity Amount: {}", decoded.liquidity_amount);
        println!("Token Max A: {}", decoded.token_max_a);
        println!("Token Max B: {}", decoded.token_max_b);
    }

    #[test]
    fn test_decode_decrease_liquidity_data() {
        let encoded_data = "8xY8jsAzTgXmNQfVWfq3imPm4kuC37aXCNVn7WLS8VYmbQy75nFC5ju";
        let result = decode_decrease_liquidity_data(encoded_data);

        assert!(result.is_ok());

        let decoded = result.unwrap();
        let expected = DecreaseLiquidityData {
            liquidity_amount: 9968981910,
            token_min_a: 5375432361,
            token_min_b: 317282184,
        };

        assert_eq!(decoded, expected);

        println!("Decoded DecreaseLiquidity Data:");
        println!("Liquidity Amount: {}", decoded.liquidity_amount);
        println!("Token Min A: {}", decoded.token_min_a);
        println!("Token Min B: {}", decoded.token_min_b);
    }

    #[test]
    fn test_decode_swap_data() {
        let encoded_data = "59p8WydnSZtRrqp7VaC17QabjBLLqjqe8qNioXKzF5gRisxqdGSDja16GQ";
        let result = decode_hawksight_swap_data(encoded_data);

        assert!(result.is_ok());

        let decoded = result.unwrap();
        let expected = HawksightSwapData {
            amount: 140034,
            other_amount_threshold: 17689,
            sqrt_price_limit: 4295048016,
            amount_specified_is_input: true,
            a_to_b: true,
        };

        assert_eq!(decoded, expected);

        println!("Decoded Swap Data:");
        println!("Amount: {}", decoded.amount);
        println!("Other Amount Threshold: {}", decoded.other_amount_threshold);
        println!("Sqrt Price Limit: {}", decoded.sqrt_price_limit);
        println!(
            "Amount Specified Is Input: {}",
            decoded.amount_specified_is_input
        );
        println!("A to B: {}", decoded.a_to_b);
    }

    #[test]
    fn test_decode_open_position_data() {
        let encoded_data = "B3T3AnPs3BbwvCRjFvmUAzk5t";
        let result = decode_open_position_data(encoded_data);

        assert!(result.is_ok());

        let decoded = result.unwrap();
        let expected = (-20328, -19732);

        assert_eq!(decoded, expected);

        println!("Decoded OpenPosition Data:");
        println!("Lower Tick Index: {}", decoded.0);
        println!("Upper Tick Index: {}", decoded.1);
    }
}
