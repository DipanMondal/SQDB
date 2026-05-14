use crate::error::SqdbError;
use crate::language::ast::{Command, DataType, TableType};

pub fn parse_command(input: &str) -> Result<Command, SqdbError> {
    let input = clean_input(input);

    if input.is_empty() {
        return Err(SqdbError::ParseError("Empty command.".to_string()));
    }

    let (first_word, rest) = take_word(input)
        .ok_or_else(|| SqdbError::ParseError("Could not read command.".to_string()))?;

    match first_word.to_lowercase().as_str() {
        "create" => parse_create(rest),
        "drop" => parse_drop(rest),
        "type" => parse_type(rest),
        "dtype" => parse_dtype(rest),
        "insert" => parse_insert(rest),
        "read" => parse_read(rest),
        "delete" => parse_delete(rest),
        "commit" => parse_no_arg_command(rest, Command::Commit),
        "rollback" => parse_no_arg_command(rest, Command::Rollback),
        "help" => parse_no_arg_command(rest, Command::Help),
        "exit" => parse_no_arg_command(rest, Command::Exit),
        "quit" => parse_no_arg_command(rest, Command::Exit),
        other => Err(SqdbError::ParseError(format!(
            "Unknown command `{}`.",
            other
        ))),
    }
}

fn parse_create(rest: &str) -> Result<Command, SqdbError> {
    let (second_word, rest) = take_word(rest)
        .ok_or_else(|| SqdbError::ParseError("Expected `db` or `table` after `create`.".to_string()))?;

    match second_word.to_lowercase().as_str() {
        "db" => {
            let name = read_single_identifier(rest, "database name")?;

            Ok(Command::CreateDb { name })
        }

        "table" => {
            let (table_name, rest) = take_word(rest)
                .ok_or_else(|| SqdbError::ParseError("Expected table name.".to_string()))?;

            validate_identifier(table_name)?;

            let (table_type_raw, rest) = take_word(rest)
                .ok_or_else(|| SqdbError::ParseError("Expected table type.".to_string()))?;

            let table_type = TableType::from_str(table_type_raw)?;

            let (data_type_raw, rest) = take_word(rest)
                .ok_or_else(|| SqdbError::ParseError("Expected data type.".to_string()))?;

            let data_type = DataType::from_str(data_type_raw)?;

            ensure_no_extra_text(rest)?;

            Ok(Command::CreateTable {
                name: table_name.to_string(),
                table_type,
                data_type,
            })
        }

        other => Err(SqdbError::ParseError(format!(
            "Expected `db` or `table` after `create`, found `{}`.",
            other
        ))),
    }
}

fn parse_drop(rest: &str) -> Result<Command, SqdbError> {
    let (first_arg, rest_after_first_arg) = take_word(rest)
        .ok_or_else(|| SqdbError::ParseError("Expected name after `drop`.".to_string()))?;

    match first_arg.to_lowercase().as_str() {
        "db" => {
            let name = read_single_identifier(rest_after_first_arg, "database name")?;

            Ok(Command::DropDb { name })
        }

        "table" => {
            let name = read_single_identifier(rest_after_first_arg, "table name")?;

            Ok(Command::DropTable { name })
        }

        table_name => {
            validate_identifier(table_name)?;
            ensure_no_extra_text(rest_after_first_arg)?;

            Ok(Command::DropTable {
                name: table_name.to_string(),
            })
        }
    }
}

fn parse_type(rest: &str) -> Result<Command, SqdbError> {
    let table_name = read_single_identifier(rest, "table name")?;

    Ok(Command::Type { table_name })
}

fn parse_dtype(rest: &str) -> Result<Command, SqdbError> {
    let table_name = read_single_identifier(rest, "table name")?;

    Ok(Command::DType { table_name })
}

fn parse_insert(rest: &str) -> Result<Command, SqdbError> {
    let (table_name, value_rest) = take_word(rest)
        .ok_or_else(|| SqdbError::ParseError("Expected table name after `insert`.".to_string()))?;

    validate_identifier(table_name)?;

    let raw_value = value_rest.trim();

    if raw_value.is_empty() {
        return Err(SqdbError::ParseError(
            "Expected value after table name.".to_string(),
        ));
    }

    Ok(Command::Insert {
        table_name: table_name.to_string(),
        raw_value: raw_value.to_string(),
    })
}

fn parse_read(rest: &str) -> Result<Command, SqdbError> {
    let table_name = read_single_identifier(rest, "table name")?;

    Ok(Command::Read { table_name })
}

fn parse_delete(rest: &str) -> Result<Command, SqdbError> {
    let table_name = read_single_identifier(rest, "table name")?;

    Ok(Command::Delete { table_name })
}

fn parse_no_arg_command(rest: &str, command: Command) -> Result<Command, SqdbError> {
    ensure_no_extra_text(rest)?;

    Ok(command)
}

fn clean_input(input: &str) -> &str {
    input.trim().trim_end_matches(';').trim()
}

fn take_word(input: &str) -> Option<(&str, &str)> {
    let input = input.trim_start();

    if input.is_empty() {
        return None;
    }

    for (index, ch) in input.char_indices() {
        if ch.is_whitespace() {
            let word = &input[..index];
            let rest = &input[index..];
            return Some((word, rest));
        }
    }

    Some((input, ""))
}

fn read_single_identifier(input: &str, expected: &str) -> Result<String, SqdbError> {
    let (identifier, rest) = take_word(input)
        .ok_or_else(|| SqdbError::ParseError(format!("Expected {}.", expected)))?;

    validate_identifier(identifier)?;
    ensure_no_extra_text(rest)?;

    Ok(identifier.to_string())
}

fn validate_identifier(identifier: &str) -> Result<(), SqdbError> {
    if identifier.is_empty() {
        return Err(SqdbError::ParseError(
            "Identifier cannot be empty.".to_string(),
        ));
    }

    let mut chars = identifier.chars();

    let first = chars.next().unwrap();

    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(SqdbError::ParseError(format!(
            "Invalid identifier `{}`. Identifier must start with a letter or underscore.",
            identifier
        )));
    }

    for ch in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '_') {
            return Err(SqdbError::ParseError(format!(
                "Invalid identifier `{}`. Only letters, numbers, and underscores are allowed.",
                identifier
            )));
        }
    }

    Ok(())
}

fn ensure_no_extra_text(input: &str) -> Result<(), SqdbError> {
    if input.trim().is_empty() {
        Ok(())
    } else {
        Err(SqdbError::ParseError(format!(
            "Unexpected extra text `{}`.",
            input.trim()
        )))
    }
}