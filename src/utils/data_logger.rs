use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use std::fs::File;
use std::io::Write;

#[derive(Clone)]
pub enum FieldValue {
    String(String),
    Integer(i64),
    UnsignedInteger(u128),
    Float(f64),
}

pub struct LogEntry {
    fields: Vec<(String, FieldValue)>,
}

impl LogEntry {
    pub fn new() -> Self {
        Self { fields: Vec::new() }
    }

    pub fn add_field<T: Into<FieldValue>>(&mut self, key: &str, value: T) {
        self.fields.push((key.to_string(), value.into()));
    }
}

impl Serialize for LogEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.fields.len()))?;
        for (key, value) in &self.fields {
            match value {
                FieldValue::String(s) => map.serialize_entry(key, s)?,
                FieldValue::Integer(i) => map.serialize_entry(key, i)?,
                FieldValue::UnsignedInteger(u) => map.serialize_entry(key, u)?,
                FieldValue::Float(f) => map.serialize_entry(key, f)?,
            }
        }
        map.end()
    }
}

pub struct DataLogger {
    entries: Vec<LogEntry>,
}

impl DataLogger {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn log(&mut self, entry: LogEntry) {
        self.entries.push(entry);
    }

    pub fn export_to_json(&self, filename: &str) -> std::io::Result<()> {
        let json_string = serde_json::to_string_pretty(&self.entries)?;
        let mut file = File::create(filename)?;
        file.write_all(json_string.as_bytes())?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn log_create_position(
        &mut self,
        position_id: String,
        lower_tick: i32,
        upper_tick: i32,
        current_tick: i32,
        token_a_balance: u128,
        token_b_balance: u128,
        token_a_lped: u128,
        token_b_lped: u128,
        liquidity_provided: u128,
        current_block_time: u128,
        current_swap_nmr: u128,
        current_token_a_volume: u128,
        current_token_b_volume: u128,
        current_active_liquidity: u128,
    ) {
        let mut entry = LogEntry::new();
        entry.add_field("action", "CreatePosition".to_string());
        entry.add_field("position_id", position_id);
        entry.add_field("lower_tick", lower_tick as i64);
        entry.add_field("upper_tick", upper_tick as i64);
        entry.add_field("current_tick", current_tick as i64);
        entry.add_field("token_a_balance", token_a_balance);
        entry.add_field("token_b_balance", token_b_balance);
        entry.add_field("token_a_lped", token_a_lped);
        entry.add_field("token_b_lped", token_b_lped);
        entry.add_field("liquidity_provided", liquidity_provided);
        entry.add_field("current_block_time", current_block_time);
        entry.add_field("current_swap_nmr", current_swap_nmr);
        entry.add_field("current_token_a_volume", current_token_a_volume);
        entry.add_field("current_token_b_volume", current_token_b_volume);
        entry.add_field("current_active_liquidity", current_active_liquidity);
        self.log(entry);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn log_close_position(
        &mut self,
        position_id: String,
        lower_tick: i32,
        upper_tick: i32,
        current_tick: i32,
        token_a_balance: u128,
        token_b_balance: u128,
        token_a_returned: u128,
        token_b_returned: u128,
        fees_a: u128,
        fees_b: u128,
        current_block_time: u128,
        current_swap_nmr: u128,
        current_token_a_volume: u128,
        current_token_b_volume: u128,
        swap_nmr_in_position: u128,
        token_a_volume_in_position: u128,
        token_b_volume_in_position: u128,
    ) {
        let mut entry = LogEntry::new();
        entry.add_field("action", "ClosePosition".to_string());
        entry.add_field("position_id", position_id);
        entry.add_field("lower_tick", lower_tick as i64);
        entry.add_field("upper_tick", upper_tick as i64);
        entry.add_field("current_tick", current_tick as i64);
        entry.add_field("token_a_balance", token_a_balance);
        entry.add_field("token_b_balance", token_b_balance);
        entry.add_field("token_a_returned", token_a_returned);
        entry.add_field("token_b_returned", token_b_returned);
        entry.add_field("fees_a", fees_a);
        entry.add_field("fees_b", fees_b);
        entry.add_field("current_block_time", current_block_time);
        entry.add_field("current_swap_nmr", current_swap_nmr);
        entry.add_field("current_token_a_volume", current_token_a_volume);
        entry.add_field("current_token_b_volume", current_token_b_volume);
        entry.add_field("swap_nmr_in_position", swap_nmr_in_position);
        entry.add_field("token_a_volume_in_position", token_a_volume_in_position);
        entry.add_field("token_b_volume_in_position", token_b_volume_in_position);
        self.log(entry);
    }
}

// Implement From traits for FieldValue
impl From<String> for FieldValue {
    fn from(value: String) -> Self {
        FieldValue::String(value)
    }
}

impl From<i64> for FieldValue {
    fn from(value: i64) -> Self {
        FieldValue::Integer(value)
    }
}

impl From<u128> for FieldValue {
    fn from(value: u128) -> Self {
        FieldValue::UnsignedInteger(value)
    }
}

impl From<f64> for FieldValue {
    fn from(value: f64) -> Self {
        FieldValue::Float(value)
    }
}
