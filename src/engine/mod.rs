use std::collections::{HashMap, VecDeque};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::SqdbError;
use crate::language::ast::{Command, DataType, TableType};
use crate::storage;

#[derive(Debug, Clone)]
pub struct Engine {
    working_db: Option<Database>,
    committed_db: Option<Database>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Database {
    pub name: String,
    pub tables: HashMap<String, Table>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    pub table_type: TableType,
    pub data_type: DataType,
    pub data: VecDeque<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value {
    Int(i64),
    Real(f64),
    String(String),
    Json(String),
}

impl Engine {
    pub fn new() -> Self {
        Self {
            working_db: None,
            committed_db: None,
        }
    }

    pub fn execute(&mut self, command: Command) -> Result<String, SqdbError> {
        match command {
            Command::CreateDb { name } => self.create_db(name),

            Command::DropDb { name } => self.drop_db(name),

            Command::CreateTable {
                name,
                table_type,
                data_type,
            } => self.create_table(name, table_type, data_type),

            Command::DropTable { name } => self.drop_table(name),

            Command::ShowTables => self.show_tables(),

            Command::Type { table_name } => self.get_table_type(table_name),

            Command::DType { table_name } => self.get_table_dtype(table_name),

            Command::Insert {
                table_name,
                raw_value,
            } => self.insert_value(table_name, raw_value),

            Command::Read { table_name } => self.read_value(table_name),

            Command::Delete { table_name } => self.delete_value(table_name),

            Command::Commit => self.commit(),

            Command::Rollback => self.rollback(),

            Command::Help => Ok(help_text()),

            Command::Exit => Ok("Goodbye.".to_string()),
        }
    }

    fn create_db(&mut self, name: String) -> Result<String, SqdbError> {
        if self.working_db.is_some() {
            return Err(SqdbError::RuntimeError(
                "A database is already open. Drop it first or restart SQDB.".to_string(),
            ));
        }
		
		storage::recover_if_needed(&name)?;

        if storage::database_exists(&name) {
            let database = storage::load_database(&name)?;

            self.working_db = Some(database.clone());
            self.committed_db = Some(database);

            return Ok(format!("Database `{}` loaded from file.", name));
        }

        let database = Database {
            name: name.clone(),
            tables: HashMap::new(),
        };

        storage::save_database_atomic(&database)?;

        self.working_db = Some(database.clone());
        self.committed_db = Some(database);

        Ok(format!("Database `{}` created.", name))
    }

    fn drop_db(&mut self, name: String) -> Result<String, SqdbError> {
        let db = self.get_db()?;

        if db.name != name {
            return Err(SqdbError::RuntimeError(format!(
                "Cannot drop database `{}` because current database is `{}`.",
                name, db.name
            )));
        }

        storage::delete_database_file_with_journal(&name)?;

        self.working_db = None;
        self.committed_db = None;

        Ok(format!("Database `{}` dropped.", name))
    }

    fn create_table(
        &mut self,
        name: String,
        table_type: TableType,
        data_type: DataType,
    ) -> Result<String, SqdbError> {
        let db = self.get_db_mut()?;

        if db.tables.contains_key(&name) {
            return Err(SqdbError::RuntimeError(format!(
                "Table `{}` already exists.",
                name
            )));
        }

        let table = Table {
            name: name.clone(),
            table_type,
            data_type,
            data: VecDeque::new(),
        };

        db.tables.insert(name.clone(), table);

        Ok(format!("Table `{}` created.", name))
    }

    fn drop_table(&mut self, name: String) -> Result<String, SqdbError> {
        let db = self.get_db_mut()?;

        match db.tables.remove(&name) {
            Some(_) => Ok(format!("Table `{}` dropped.", name)),
            None => Err(SqdbError::RuntimeError(format!(
                "Table `{}` does not exist.",
                name
            ))),
        }
    }

    fn show_tables(&self) -> Result<String, SqdbError> {
        let db = self.get_db()?;

        if db.tables.is_empty() {
            return Ok("No tables found.".to_string());
        }

        let mut output = String::new();

        output.push_str("Tables:\n");
        output.push_str("---------------------------------------------\n");
        output.push_str("Name                 Type      DType     Items\n");
        output.push_str("---------------------------------------------\n");

        let mut tables: Vec<&Table> = db.tables.values().collect();

        tables.sort_by(|a, b| a.name.cmp(&b.name));

        for table in tables {
            output.push_str(&format!(
                "{:<20} {:<9} {:<9} {}\n",
                table.name,
                table.table_type,
                table.data_type,
                table.data.len()
            ));
        }

        Ok(output)
    }

    fn get_table_type(&self, table_name: String) -> Result<String, SqdbError> {
        let table = self.get_table(&table_name)?;

        Ok(format!("{}", table.table_type))
    }

    fn get_table_dtype(&self, table_name: String) -> Result<String, SqdbError> {
        let table = self.get_table(&table_name)?;

        Ok(format!("{}", table.data_type))
    }

    fn insert_value(&mut self, table_name: String, raw_value: String) -> Result<String, SqdbError> {
        let table = self.get_table_mut(&table_name)?;

        let value = parse_value(&table.data_type, &raw_value)?;

        match table.table_type {
            TableType::Stack => {
                table.data.push_back(value);
                Ok(format!("Value inserted into stack `{}`.", table_name))
            }

            TableType::Queue => {
                table.data.push_back(value);
                Ok(format!("Value inserted into queue `{}`.", table_name))
            }
        }
    }

    fn read_value(&self, table_name: String) -> Result<String, SqdbError> {
        let db_name = self.get_db()?.name.clone();
        let table = self.get_table(&table_name)?;

        let value = match table.table_type {
            TableType::Stack => table.data.back(),
            TableType::Queue => table.data.front(),
        };

        match value {
            Some(value) => Ok(value.to_string()),
            None => Ok(format!("{}.None", db_name)),
        }
    }

    fn delete_value(&mut self, table_name: String) -> Result<String, SqdbError> {
        let db_name = self.get_db()?.name.clone();
        let table = self.get_table_mut(&table_name)?;

        let value = match table.table_type {
            TableType::Stack => table.data.pop_back(),
            TableType::Queue => table.data.pop_front(),
        };

        match value {
            Some(value) => Ok(value.to_string()),
            None => Ok(format!("{}.None", db_name)),
        }
    }

    fn commit(&mut self) -> Result<String, SqdbError> {
        let database = self.get_db()?.clone();

        storage::save_database_atomic(&database)?;

        self.committed_db = Some(database);

        Ok("Transaction committed and saved to disk.".to_string())
    }

    fn rollback(&mut self) -> Result<String, SqdbError> {
        if self.committed_db.is_none() {
            return Err(SqdbError::RuntimeError(
                "No committed database state found.".to_string(),
            ));
        }

        self.working_db = self.committed_db.clone();

        Ok("Transaction rolled back.".to_string())
    }

    fn get_db(&self) -> Result<&Database, SqdbError> {
        self.working_db.as_ref().ok_or_else(|| {
            SqdbError::RuntimeError(
                "No database is currently open. Use `create db <database_name>;` first."
                    .to_string(),
            )
        })
    }

    fn get_db_mut(&mut self) -> Result<&mut Database, SqdbError> {
        self.working_db.as_mut().ok_or_else(|| {
            SqdbError::RuntimeError(
                "No database is currently open. Use `create db <database_name>;` first."
                    .to_string(),
            )
        })
    }

    fn get_table(&self, table_name: &str) -> Result<&Table, SqdbError> {
        let db = self.get_db()?;

        db.tables.get(table_name).ok_or_else(|| {
            SqdbError::RuntimeError(format!("Table `{}` does not exist.", table_name))
        })
    }

    fn get_table_mut(&mut self, table_name: &str) -> Result<&mut Table, SqdbError> {
        let db = self.get_db_mut()?;

        db.tables.get_mut(table_name).ok_or_else(|| {
            SqdbError::RuntimeError(format!("Table `{}` does not exist.", table_name))
        })
    }
}

fn parse_value(data_type: &DataType, raw_value: &str) -> Result<Value, SqdbError> {
    match data_type {
        DataType::Int => {
            let value = raw_value.parse::<i64>().map_err(|_| {
                SqdbError::RuntimeError(format!("Expected int value, found `{}`.", raw_value))
            })?;

            Ok(Value::Int(value))
        }

        DataType::Real => {
            let value = raw_value.parse::<f64>().map_err(|_| {
                SqdbError::RuntimeError(format!("Expected real value, found `{}`.", raw_value))
            })?;

            Ok(Value::Real(value))
        }

        DataType::String => {
            let value = strip_optional_quotes(raw_value);

            Ok(Value::String(value))
        }

        DataType::Json => {
            let value = raw_value.trim();

            if is_simple_json(value) {
                Ok(Value::Json(value.to_string()))
            } else {
                Err(SqdbError::RuntimeError(format!(
                    "Expected json value, found `{}`.",
                    raw_value
                )))
            }
        }
    }
}

fn strip_optional_quotes(value: &str) -> String {
    let value = value.trim();

    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn is_simple_json(value: &str) -> bool {
    let value = value.trim();

    let is_object = value.starts_with('{') && value.ends_with('}');
    let is_array = value.starts_with('[') && value.ends_with(']');

    is_object || is_array
}

impl fmt::Display for TableType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableType::Stack => write!(f, "stack"),
            TableType::Queue => write!(f, "queue"),
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

fn help_text() -> String {
    let text = r#"
SQDB Commands:

Database:
  create db <database_name>;
  drop db <database_name>;

Table:
  create table <table_name> <stack|queue> <int|real|string|json>;
  drop <table_name>;
  drop table <table_name>;
  show tables;

Info:
  type <table_name>;
  dtype <table_name>;

Data:
  insert <table_name> <value>;
  read <table_name>;
  delete <table_name>;

Transaction:
  commit;
  rollback;

Other:
  help;
  exit;
"#;

    text.to_string()
}