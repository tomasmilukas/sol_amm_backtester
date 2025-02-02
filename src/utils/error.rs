use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub struct PriceCalcError(pub String);

impl fmt::Display for PriceCalcError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Price calculation error: {}", self.0)
    }
}

impl Error for PriceCalcError {}

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

impl From<LiquidityArrayError> for SyncError {
    fn from(error: LiquidityArrayError) -> Self {
        match error {
            LiquidityArrayError::PriceCalculation(err) => {
                SyncError::CalculationError(err.to_string())
            }
            _ => SyncError::Other(error.to_string()),
        }
    }
}

impl From<PriceCalcError> for SyncError {
    fn from(error: PriceCalcError) -> Self {
        SyncError::CalculationError(error.to_string())
    }
}

#[derive(Debug)]
pub enum BacktestError {
    InitializedTickNotFound,
    PriceCalculationError(String),
    PositionNotFound(String),
    Other(String),
}

impl fmt::Display for BacktestError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BacktestError::PriceCalculationError(msg) => {
                write!(f, "Price calculation error: {}", msg)
            }
            BacktestError::PositionNotFound(id) => write!(f, "Position not found: {}", id),
            BacktestError::InitializedTickNotFound => write!(f, "Initialized tick not found."),
            BacktestError::Other(msg) => write!(f, "Unknown error: {}", msg),
        }
    }
}

impl Error for BacktestError {}

#[derive(Debug)]
pub enum LiquidityArrayError {
    PositionNotFound(String),
    InitializedTickNotFound,
    FeeCalculationError,
    PriceCalculation(PriceCalcError),
}

impl fmt::Display for LiquidityArrayError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LiquidityArrayError::PositionNotFound(id) => write!(f, "Position not found: {}", id),
            LiquidityArrayError::FeeCalculationError => {
                write!(f, "Overflow/underflow fee calculation error")
            }
            LiquidityArrayError::PriceCalculation(err) => write!(f, "{}", err),
            LiquidityArrayError::InitializedTickNotFound => {
                write!(f, "Initialized tick not found")
            }
        }
    }
}

impl Error for LiquidityArrayError {}

impl From<LiquidityArrayError> for BacktestError {
    fn from(error: LiquidityArrayError) -> Self {
        match error {
            LiquidityArrayError::PositionNotFound(id) => BacktestError::PositionNotFound(id),
            LiquidityArrayError::FeeCalculationError => {
                BacktestError::Other("Fee Calculation Error".to_string())
            }
            LiquidityArrayError::PriceCalculation(err) => {
                BacktestError::PriceCalculationError(err.to_string())
            }
            LiquidityArrayError::InitializedTickNotFound => BacktestError::InitializedTickNotFound,
        }
    }
}

impl From<PriceCalcError> for LiquidityArrayError {
    fn from(error: PriceCalcError) -> Self {
        LiquidityArrayError::PriceCalculation(error)
    }
}
