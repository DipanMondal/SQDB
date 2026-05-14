use std::fmt;

#[derive(Debug, Clone)]
pub enum SqdbError {
    ParseError(String),
    RuntimeError(String),
    IoError(String),
}

impl fmt::Display for SqdbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqdbError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            SqdbError::RuntimeError(msg) => write!(f, "Runtime error: {}", msg),
            SqdbError::IoError(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl std::error::Error for SqdbError {}

impl From<std::io::Error> for SqdbError {
    fn from(err: std::io::Error) -> Self {
        SqdbError::IoError(err.to_string())
    }
}