use std::fs;
use std::path::PathBuf;

use crate::error::SqdbError;
use crate::language::ast::{Command, DataType, TableType};
use crate::storage::binary::{BinaryDatabaseHandle, BinaryPageStorage};

pub struct Engine {
    storage: BinaryPageStorage,
    current_db: Option<BinaryDatabaseHandle>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            storage: BinaryPageStorage::new(),
            current_db: None,
        }
    }

    pub fn with_binary_storage(storage: BinaryPageStorage) -> Self {
        Self {
            storage,
            current_db: None,
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
        if self.current_db.is_some() {
            return Err(SqdbError::RuntimeError(
                "A database is already open. Drop it first or restart SQDB.".to_string(),
            ));
        }

        let existed_before = self.storage.database_exists(&name);

        let handle = self.storage.open_or_create_database(&name)?;

        self.current_db = Some(handle);

        if existed_before {
            Ok(format!("Binary database `{}` opened.", name))
        } else {
            Ok(format!("Binary database `{}` created.", name))
        }
    }

    fn drop_db(&mut self, name: String) -> Result<String, SqdbError> {
        let current_db_name = self
            .current_db
            .as_ref()
            .ok_or_else(|| {
                SqdbError::RuntimeError(
                    "No database is currently open. Use `create db <database_name>;` first."
                        .to_string(),
                )
            })?
            .database_name()
            .to_string();

        if current_db_name != name {
            return Err(SqdbError::RuntimeError(format!(
                "Cannot drop database `{}` because current database is `{}`.",
                name, current_db_name
            )));
        }

        // Important on Windows:
        // Close the open file handle before deleting the database file.
        self.current_db = None;

        self.storage.drop_database(&name)?;

        Ok(format!("Binary database `{}` dropped.", name))
    }

    fn create_table(
        &mut self,
        name: String,
        table_type: TableType,
        data_type: DataType,
    ) -> Result<String, SqdbError> {
        let db = self.get_db_mut()?;

        db.create_table(&name, table_type.clone(), data_type.clone())?;

        Ok(format!(
            "Table `{}` created as {} {}.",
            name, table_type, data_type
        ))
    }

    fn drop_table(&mut self, name: String) -> Result<String, SqdbError> {
        let db = self.get_db_mut()?;

        db.drop_table(&name)?;

        Ok(format!("Table `{}` dropped.", name))
    }

    fn show_tables(&mut self) -> Result<String, SqdbError> {
        let db = self.get_db_mut()?;

        db.show_tables()
    }

    fn get_table_type(&mut self, table_name: String) -> Result<String, SqdbError> {
        let db = self.get_db_mut()?;

        let table_type = db.get_table_type(&table_name)?;

        Ok(table_type.to_string())
    }

    fn get_table_dtype(&mut self, table_name: String) -> Result<String, SqdbError> {
        let db = self.get_db_mut()?;

        let data_type = db.get_table_dtype(&table_name)?;

        Ok(data_type.to_string())
    }

    fn insert_value(&mut self, table_name: String, raw_value: String) -> Result<String, SqdbError> {
        let db = self.get_db_mut()?;

        db.insert_raw(&table_name, &raw_value)?;

        Ok(format!("Value inserted into `{}`.", table_name))
    }

    fn read_value(&mut self, table_name: String) -> Result<String, SqdbError> {
        let db = self.get_db_mut()?;

        db.read_value(&table_name)
    }

    fn delete_value(&mut self, table_name: String) -> Result<String, SqdbError> {
        let db = self.get_db_mut()?;

        db.delete_value(&table_name)
    }

    fn commit(&mut self) -> Result<String, SqdbError> {
        self.get_db_mut()?;

        Ok(
            "Commit acknowledged. Current binary backend writes each operation directly to disk. Full transaction journaling will be added next."
                .to_string(),
        )
    }

    fn rollback(&mut self) -> Result<String, SqdbError> {
        self.get_db_mut()?;

        Err(SqdbError::RuntimeError(
            "Rollback is not available yet for BinaryPageStorage. Disk-based page journal rollback will be added in the next step."
                .to_string(),
        ))
    }

    fn get_db_mut(&mut self) -> Result<&mut BinaryDatabaseHandle, SqdbError> {
        self.current_db.as_mut().ok_or_else(|| {
            SqdbError::RuntimeError(
                "No database is currently open. Use `create db <database_name>;` first."
                    .to_string(),
            )
        })
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::language::parser::parse_command;

    fn unique_test_dir() -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!("sqdb_engine_binary_test_{}", timestamp))
    }

    fn run(engine: &mut Engine, input: &str) -> String {
        let command = parse_command(input).expect("Command should parse");
        engine.execute(command).expect("Command should execute")
    }

    #[test]
    fn cli_engine_uses_binary_stack_behavior() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let mut engine = Engine::with_binary_storage(storage);

        run(&mut engine, "create db test_stack;");
        run(&mut engine, "create table numbers stack int;");
        run(&mut engine, "insert numbers 10;");
        run(&mut engine, "insert numbers 20;");
        run(&mut engine, "insert numbers 30;");

        assert_eq!(run(&mut engine, "read numbers;"), "30");
        assert_eq!(run(&mut engine, "delete numbers;"), "30");
        assert_eq!(run(&mut engine, "read numbers;"), "20");

        run(&mut engine, "drop db test_stack;");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cli_engine_uses_binary_queue_behavior() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let mut engine = Engine::with_binary_storage(storage);

        run(&mut engine, "create db test_queue;");
        run(&mut engine, "create table names queue string;");
        run(&mut engine, "insert names Sourav;");
        run(&mut engine, "insert names Rahul;");
        run(&mut engine, "insert names Amit;");

        assert_eq!(run(&mut engine, "read names;"), "Sourav");
        assert_eq!(run(&mut engine, "delete names;"), "Sourav");
        assert_eq!(run(&mut engine, "read names;"), "Rahul");

        run(&mut engine, "drop db test_queue;");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cli_engine_persists_binary_database_after_restart() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        {
            let storage = BinaryPageStorage::with_base_dir(dir.clone());
            let mut engine = Engine::with_binary_storage(storage);

            run(&mut engine, "create db test_persist;");
            run(&mut engine, "create table numbers stack int;");
            run(&mut engine, "insert numbers 100;");
            run(&mut engine, "insert numbers 200;");
        }

        {
            let storage = BinaryPageStorage::with_base_dir(dir.clone());
            let mut engine = Engine::with_binary_storage(storage);

            run(&mut engine, "create db test_persist;");

            assert_eq!(run(&mut engine, "read numbers;"), "200");

            run(&mut engine, "drop db test_persist;");
        }

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cli_engine_supports_show_tables_type_and_dtype() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let mut engine = Engine::with_binary_storage(storage);

        run(&mut engine, "create db test_info;");
        run(&mut engine, "create table events queue json;");

        let output = run(&mut engine, "show tables;");

        assert!(output.contains("events"));
        assert!(output.contains("queue"));
        assert!(output.contains("json"));

        assert_eq!(run(&mut engine, "type events;"), "queue");
        assert_eq!(run(&mut engine, "dtype events;"), "json");

        run(&mut engine, "drop db test_info;");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cli_engine_returns_none_for_empty_table() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let mut engine = Engine::with_binary_storage(storage);

        run(&mut engine, "create db test_none;");
        run(&mut engine, "create table empty_stack stack int;");

        assert_eq!(run(&mut engine, "read empty_stack;"), "test_none.None");
        assert_eq!(run(&mut engine, "delete empty_stack;"), "test_none.None");

        run(&mut engine, "drop db test_none;");

        let _ = fs::remove_dir_all(dir);
    }
}