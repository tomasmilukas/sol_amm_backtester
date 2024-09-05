use std::fmt;
use std::error::Error;

#[derive(Debug)]
pub struct PriceCalcError(pub String);

impl fmt::Display for PriceCalcError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Price calculation error: {}", self.0)
    }
}

#[derive(Debug)]
pub enum SyncError {
    DatabaseError(String),
    CalculationError(String),
    ParseError(String),
    Other(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SyncError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            SyncError::CalculationError(msg) => write!(f, "Calculation error: {}", msg),
            SyncError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            SyncError::Other(msg) => write!(f, "Other error: {}", msg),
        }
    }
}

impl Error for SyncError {}

// You might want to implement From traits for specific error types
impl From<PriceCalcError> for SyncError {
    fn from(error: PriceCalcError) -> Self {
        SyncError::CalculationError(error.to_string())
    }
}

#[macro_export]
macro_rules! try_calc {
    ($expr:expr) => {
        match $expr {
            Some(val) => Ok(val),
            None => Err(PriceCalcError(stringify!($expr).to_string())),
        }
    };
}
