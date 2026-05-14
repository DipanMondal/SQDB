use serde_json::Value as JsonValue;

use crate::error::SqdbError;
use crate::language::ast::{Command, DataType, TableType};
use crate::model::{Database, Table, Value};
use crate::storage::{JsonFileStorage, StorageManager};

pub struct Engine {
    working_db: Option<Database>,
    committed_db: Option<Database>,
    storage: Box<dyn StorageManager>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            working_db: None,
            committed_db: None,
            storage: Box::new(JsonFileStorage::new()),
        }
    }

    pub fn with_storage(storage: Box<dyn StorageManager>) -> Self {
        Self {
            working_db: None,
            committed_db: None,
            storage,
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

        self.storage.recover_if_needed(&name)?;

        if self.storage.database_exists(&name) {
            let database = self.storage.load_database(&name)?;

            self.working_db = Some(database.clone());
            self.committed_db = Some(database);

            return Ok(format!("Database `{}` loaded from file.", name));
        }

        let database = Database::new(name.clone());

        self.storage.save_database(&database)?;

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

        self.storage.delete_database(&name)?;

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

        let table = Table::new(name.clone(), table_type, data_type);

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
                table.len()
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

        self.storage.save_database(&database)?;

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
            let value = raw_value.trim().parse::<i64>().map_err(|_| {
                SqdbError::RuntimeError(format!("Expected int value, found `{}`.", raw_value))
            })?;

            Ok(Value::Int(value))
        }

        DataType::Real => {
            let value = raw_value.trim().parse::<f64>().map_err(|_| {
                SqdbError::RuntimeError(format!("Expected real value, found `{}`.", raw_value))
            })?;

            Ok(Value::Real(value))
        }

        DataType::String => {
            let value = parse_string_value(raw_value)?;

            Ok(Value::String(value))
        }

        DataType::Json => {
            let value = serde_json::from_str::<JsonValue>(raw_value.trim()).map_err(|err| {
                SqdbError::RuntimeError(format!(
                    "Expected valid json value, found `{}`. JSON error: {}",
                    raw_value, err
                ))
            })?;

            Ok(Value::Json(value))
        }
    }
}

fn parse_string_value(raw_value: &str) -> Result<String, SqdbError> {
    let value = raw_value.trim();

    if value.is_empty() {
        return Ok(String::new());
    }

    let starts_with_quote = value.starts_with('"');
    let ends_with_quote = value.ends_with('"');

    if starts_with_quote || ends_with_quote {
        if !(starts_with_quote && ends_with_quote) {
            return Err(SqdbError::RuntimeError(format!(
                "Invalid string literal `{}`. Quoted strings must start and end with double quotes.",
                raw_value
            )));
        }

        let parsed = serde_json::from_str::<String>(value).map_err(|err| {
            SqdbError::RuntimeError(format!(
                "Invalid string literal `{}`. String error: {}",
                raw_value, err
            ))
        })?;

        Ok(parsed)
    } else {
        Ok(value.to_string())
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::language::parser::parse_command;

    fn run(engine: &mut Engine, input: &str) -> String {
        let command = parse_command(input).expect("Command should parse");
        engine.execute(command).expect("Command should execute")
    }

    fn cleanup_database_files(database_name: &str) {
        let _ = fs::remove_file(format!("{}.sqdb", database_name));
        let _ = fs::remove_file(format!("{}.sqdb.tmp", database_name));
        let _ = fs::remove_file(format!("{}.sqdb.journal", database_name));
    }

    #[test]
    fn stack_reads_last_inserted_value() {
        let db_name = "test_stack_reads_last_inserted_value";
        cleanup_database_files(db_name);

        let mut engine = Engine::new();

        run(&mut engine, &format!("create db {};", db_name));
        run(&mut engine, "create table numbers stack int;");
        run(&mut engine, "insert numbers 10;");
        run(&mut engine, "insert numbers 20;");

        let output = run(&mut engine, "read numbers;");

        assert_eq!(output, "20");

        cleanup_database_files(db_name);
    }

    #[test]
    fn queue_reads_first_inserted_value() {
        let db_name = "test_queue_reads_first_inserted_value";
        cleanup_database_files(db_name);

        let mut engine = Engine::new();

        run(&mut engine, &format!("create db {};", db_name));
        run(&mut engine, "create table names queue string;");
        run(&mut engine, "insert names Sourav;");
        run(&mut engine, "insert names Rahul;");

        let output = run(&mut engine, "read names;");

        assert_eq!(output, "Sourav");

        cleanup_database_files(db_name);
    }

    #[test]
    fn empty_table_returns_global_none_object() {
        let db_name = "test_empty_table_returns_global_none_object";
        cleanup_database_files(db_name);

        let mut engine = Engine::new();

        run(&mut engine, &format!("create db {};", db_name));
        run(&mut engine, "create table empty_stack stack int;");

        let output = run(&mut engine, "read empty_stack;");

        assert_eq!(output, format!("{}.None", db_name));

        cleanup_database_files(db_name);
    }

    #[test]
    fn rollback_restores_last_committed_state() {
        let db_name = "test_rollback_restores_last_committed_state";
        cleanup_database_files(db_name);

        let mut engine = Engine::new();

        run(&mut engine, &format!("create db {};", db_name));
        run(&mut engine, "create table numbers stack int;");
        run(&mut engine, "insert numbers 100;");
        run(&mut engine, "commit;");
        run(&mut engine, "insert numbers 200;");

        assert_eq!(run(&mut engine, "read numbers;"), "200");

        run(&mut engine, "rollback;");

        assert_eq!(run(&mut engine, "read numbers;"), "100");

        cleanup_database_files(db_name);
    }

    #[test]
    fn json_accepts_valid_json() {
        let db_name = "test_json_accepts_valid_json";
        cleanup_database_files(db_name);

        let mut engine = Engine::new();

        run(&mut engine, &format!("create db {};", db_name));
        run(&mut engine, "create table users queue json;");
        run(&mut engine, r#"insert users {"name":"Sourav"};"#);

        let output = run(&mut engine, "read users;");

        assert_eq!(output, r#"{"name":"Sourav"}"#);

        cleanup_database_files(db_name);
    }

    #[test]
    fn json_rejects_invalid_json() {
        let db_name = "test_json_rejects_invalid_json";
        cleanup_database_files(db_name);

        let mut engine = Engine::new();

        run(&mut engine, &format!("create db {};", db_name));
        run(&mut engine, "create table users queue json;");

        let command = parse_command("insert users {name:Sourav};").unwrap();
        let result = engine.execute(command);

        assert!(result.is_err());

        cleanup_database_files(db_name);
    }

    #[test]
    fn quoted_string_supports_spaces() {
        let db_name = "test_quoted_string_supports_spaces";
        cleanup_database_files(db_name);

        let mut engine = Engine::new();

        run(&mut engine, &format!("create db {};", db_name));
        run(&mut engine, "create table messages queue string;");
        run(&mut engine, r#"insert messages "hello world";"#);

        let output = run(&mut engine, "read messages;");

        assert_eq!(output, "hello world");

        cleanup_database_files(db_name);
    }
}