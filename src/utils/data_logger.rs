use serde_json::json;
use std::fs::File;
use std::io::Write;

pub struct DataLogger {
    entries: Vec<serde_json::Value>,
}

impl DataLogger {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn log(&mut self, entry: serde_json::Value) {
        self.entries.push(entry);
    }

    pub fn export_to_json(&self, filename: &str) -> std::io::Result<()> {
        let json_string = serde_json::to_string_pretty(&self.entries)?;
        let mut file = File::create(filename)?;
        file.write_all(json_string.as_bytes())?;
        Ok(())
    }
}
