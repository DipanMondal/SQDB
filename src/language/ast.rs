use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::SqdbError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableType {
    Stack,
    Queue,
}

impl TableType {
    pub fn from_str(value: &str) -> Result<Self, SqdbError> {
        match value.to_lowercase().as_str() {
            "stack" => Ok(TableType::Stack),
            "queue" => Ok(TableType::Queue),
            _ => Err(SqdbError::ParseError(format!(
                "Invalid table type `{}`. Expected `stack` or `queue`.",
                value
            ))),
        }
    }
}

impl fmt::Display for TableType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableType::Stack => write!(f, "stack"),
            TableType::Queue => write!(f, "queue"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    Int,
    Real,
    String,
    Json,
}

impl DataType {
    pub fn from_str(value: &str) -> Result<Self, SqdbError> {
        match value.to_lowercase().as_str() {
            "int" => Ok(DataType::Int),
            "real" => Ok(DataType::Real),
            "string" => Ok(DataType::String),
            "json" => Ok(DataType::Json),
            _ => Err(SqdbError::ParseError(format!(
                "Invalid data type `{}`. Expected `int`, `real`, `string`, or `json`.",
                value
            ))),
        }
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataType::Int => write!(f, "int"),
            DataType::Real => write!(f, "real"),
            DataType::String => write!(f, "string"),
            DataType::Json => write!(f, "json"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    CreateDb {
        name: String,
    },

    DropDb {
        name: String,
    },

    CreateTable {
        name: String,
        table_type: TableType,
        data_type: DataType,
    },

    DropTable {
        name: String,
    },

    ShowTables,

    Type {
        table_name: String,
    },

    DType {
        table_name: String,
    },

    Insert {
        table_name: String,
        raw_value: String,
    },

    Read {
        table_name: String,
    },

    Delete {
        table_name: String,
    },

    Commit,

    Rollback,

    Help,

    Exit,
}

impl Command {
    pub fn is_exit(&self) -> bool {
        matches!(self, Command::Exit)
    }
}