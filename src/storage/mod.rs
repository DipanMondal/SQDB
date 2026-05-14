use std::fs;
use std::path::PathBuf;

use crate::engine::Database;
use crate::error::SqdbError;

const SQDB_EXTENSION: &str = "sqdb";

pub fn database_exists(database_name: &str) -> bool {
    database_path(database_name).exists()
}

pub fn load_database(database_name: &str) -> Result<Database, SqdbError> {
    let path = database_path(database_name);

    let content = fs::read_to_string(&path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not read database file `{}`: {}",
            path.display(),
            err
        ))
    })?;

    let database: Database = serde_json::from_str(&content).map_err(|err| {
        SqdbError::IoError(format!(
            "Database file `{}` is corrupted or invalid: {}",
            path.display(),
            err
        ))
    })?;

    if database.name != database_name {
        return Err(SqdbError::IoError(format!(
            "Database file name mismatch. Expected database `{}`, but file contains `{}`.",
            database_name, database.name
        )));
    }

    Ok(database)
}

pub fn save_database_atomic(database: &Database) -> Result<(), SqdbError> {
    let final_path = database_path(&database.name);
    let temp_path = temp_database_path(&database.name);

    let content = serde_json::to_string_pretty(database).map_err(|err| {
        SqdbError::IoError(format!("Could not serialize database: {}", err))
    })?;

    fs::write(&temp_path, content).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not write temporary database file `{}`: {}",
            temp_path.display(),
            err
        ))
    })?;

    fs::rename(&temp_path, &final_path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not rename temporary file `{}` to `{}`: {}",
            temp_path.display(),
            final_path.display(),
            err
        ))
    })?;

    Ok(())
}

pub fn delete_database_file(database_name: &str) -> Result<(), SqdbError> {
    let path = database_path(database_name);

    if path.exists() {
        fs::remove_file(&path).map_err(|err| {
            SqdbError::IoError(format!(
                "Could not delete database file `{}`: {}",
                path.display(),
                err
            ))
        })?;
    }

    Ok(())
}

pub fn database_path(database_name: &str) -> PathBuf {
    PathBuf::from(format!("{}.{}", database_name, SQDB_EXTENSION))
}

fn temp_database_path(database_name: &str) -> PathBuf {
    PathBuf::from(format!("{}.{}.tmp", database_name, SQDB_EXTENSION))
}