use crate::error::SqdbError;
use crate::language::ast::Command;

pub struct Engine;

impl Engine {
    pub fn new() -> Self {
        Self
    }

    pub fn execute(&mut self, command: Command) -> Result<String, SqdbError> {
		match command {
			Command::Help => Ok(help_text()),

			Command::ShowTables => Ok(
				"Show tables command parsed successfully.\nActual table listing will be added when we build the in-memory engine."
					.to_string(),
			),

			other => Ok(format!(
				"Command parsed successfully: {:?}\nEngine execution will be added in the next step.",
				other
			)),
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