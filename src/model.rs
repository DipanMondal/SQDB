use std::collections::{HashMap, VecDeque};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::language::ast::{DataType, TableType};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Database {
    pub name: String,
    pub tables: HashMap<String, Table>,
}

impl Database {
    pub fn new(name: String) -> Self {
        Self {
            name,
            tables: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    pub table_type: TableType,
    pub data_type: DataType,
    pub data: VecDeque<Value>,
}

impl Table {
    pub fn new(name: String, table_type: TableType, data_type: DataType) -> Self {
        Self {
            name,
            table_type,
            data_type,
            data: VecDeque::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value {
    Int(i64),
    Real(f64),
    String(String),
    Json(JsonValue),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(value) => write!(f, "{}", value),
            Value::Real(value) => write!(f, "{}", value),
            Value::String(value) => write!(f, "{}", value),
            Value::Json(value) => write!(f, "{}", value),
        }
    }
}