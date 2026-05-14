use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::Database;
use crate::error::SqdbError;

const SQDB_EXTENSION: &str = "sqdb";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Journal {
    database_name: String,
    stage: JournalStage,
    previous_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum JournalStage {
    SaveStarted,
    ReplaceStarted,
    DeleteStarted,
}

pub fn database_exists(database_name: &str) -> bool {
    database_path(database_name).exists()
}

pub fn recover_if_needed(database_name: &str) -> Result<(), SqdbError> {
    let final_path = database_path(database_name);
    let temp_path = temp_database_path(database_name);
    let journal_path = journal_database_path(database_name);

    if !journal_path.exists() {
        remove_file_if_exists(&temp_path)?;
        return Ok(());
    }

    let journal = read_journal(database_name)?;

    if journal.database_name != database_name {
        return Err(SqdbError::IoError(format!(
            "Journal database mismatch. Expected `{}`, found `{}`.",
            database_name, journal.database_name
        )));
    }

    match journal.stage {
        JournalStage::SaveStarted => {
            remove_file_if_exists(&temp_path)?;
            restore_previous_or_delete(&final_path, journal.previous_content)?;
            remove_file_if_exists(&journal_path)?;
        }

        JournalStage::ReplaceStarted => {
            if temp_path.exists() {
                remove_file_if_exists(&temp_path)?;
                restore_previous_or_delete(&final_path, journal.previous_content)?;
            } else if final_path.exists() && is_valid_database_file(&final_path, database_name) {
                remove_file_if_exists(&journal_path)?;
            } else {
                restore_previous_or_delete(&final_path, journal.previous_content)?;
            }

            remove_file_if_exists(&journal_path)?;
        }

        JournalStage::DeleteStarted => {
            if !final_path.exists() {
                restore_previous_or_delete(&final_path, journal.previous_content)?;
            }

            remove_file_if_exists(&journal_path)?;
        }
    }

    Ok(())
}

pub fn load_database(database_name: &str) -> Result<Database, SqdbError> {
    recover_if_needed(database_name)?;

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
    recover_if_needed(&database.name)?;

    let final_path = database_path(&database.name);
    let temp_path = temp_database_path(&database.name);
    let journal_path = journal_database_path(&database.name);

    let previous_content = if final_path.exists() {
        Some(fs::read_to_string(&final_path).map_err(|err| {
            SqdbError::IoError(format!(
                "Could not read existing database file `{}`: {}",
                final_path.display(),
                err
            ))
        })?)
    } else {
        None
    };

    let journal = Journal {
        database_name: database.name.clone(),
        stage: JournalStage::SaveStarted,
        previous_content: previous_content.clone(),
    };

    write_journal(&journal_path, &journal)?;

    let new_content = serde_json::to_string_pretty(database).map_err(|err| {
        SqdbError::IoError(format!("Could not serialize database: {}", err))
    })?;

    write_text_file_synced(&temp_path, &new_content)?;

    let journal = Journal {
        database_name: database.name.clone(),
        stage: JournalStage::ReplaceStarted,
        previous_content,
    };

    write_journal(&journal_path, &journal)?;

    if final_path.exists() {
        fs::remove_file(&final_path).map_err(|err| {
            SqdbError::IoError(format!(
                "Could not remove old database file `{}`: {}",
                final_path.display(),
                err
            ))
        })?;
    }

    fs::rename(&temp_path, &final_path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not rename temporary file `{}` to `{}`: {}",
            temp_path.display(),
            final_path.display(),
            err
        ))
    })?;

    verify_database_file(&final_path, &database.name)?;

    remove_file_if_exists(&journal_path)?;

    Ok(())
}

pub fn delete_database_file_with_journal(database_name: &str) -> Result<(), SqdbError> {
    recover_if_needed(database_name)?;

    let final_path = database_path(database_name);
    let journal_path = journal_database_path(database_name);

    if !final_path.exists() {
        return Ok(());
    }

    let previous_content = Some(fs::read_to_string(&final_path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not read database file `{}` before delete: {}",
            final_path.display(),
            err
        ))
    })?);

    let journal = Journal {
        database_name: database_name.to_string(),
        stage: JournalStage::DeleteStarted,
        previous_content,
    };

    write_journal(&journal_path, &journal)?;

    fs::remove_file(&final_path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not delete database file `{}`: {}",
            final_path.display(),
            err
        ))
    })?;

    remove_file_if_exists(&journal_path)?;

    Ok(())
}

pub fn database_path(database_name: &str) -> PathBuf {
    PathBuf::from(format!("{}.{}", database_name, SQDB_EXTENSION))
}

fn temp_database_path(database_name: &str) -> PathBuf {
    PathBuf::from(format!("{}.{}.tmp", database_name, SQDB_EXTENSION))
}

fn journal_database_path(database_name: &str) -> PathBuf {
    PathBuf::from(format!("{}.{}.journal", database_name, SQDB_EXTENSION))
}

fn read_journal(database_name: &str) -> Result<Journal, SqdbError> {
    let path = journal_database_path(database_name);

    let content = fs::read_to_string(&path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not read journal file `{}`: {}",
            path.display(),
            err
        ))
    })?;

    let journal: Journal = serde_json::from_str(&content).map_err(|err| {
        SqdbError::IoError(format!(
            "Journal file `{}` is corrupted or invalid: {}",
            path.display(),
            err
        ))
    })?;

    Ok(journal)
}

fn write_journal(path: &Path, journal: &Journal) -> Result<(), SqdbError> {
    let content = serde_json::to_string_pretty(journal).map_err(|err| {
        SqdbError::IoError(format!("Could not serialize journal: {}", err))
    })?;

    write_text_file_synced(path, &content)
}

fn write_text_file_synced(path: &Path, content: &str) -> Result<(), SqdbError> {
    let mut file = File::create(path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not create file `{}`: {}",
            path.display(),
            err
        ))
    })?;

    file.write_all(content.as_bytes()).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not write file `{}`: {}",
            path.display(),
            err
        ))
    })?;

    file.sync_all().map_err(|err| {
        SqdbError::IoError(format!(
            "Could not sync file `{}` to disk: {}",
            path.display(),
            err
        ))
    })?;

    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<(), SqdbError> {
    if path.exists() {
        fs::remove_file(path).map_err(|err| {
            SqdbError::IoError(format!(
                "Could not remove file `{}`: {}",
                path.display(),
                err
            ))
        })?;
    }

    Ok(())
}

fn restore_previous_or_delete(
    final_path: &Path,
    previous_content: Option<String>,
) -> Result<(), SqdbError> {
    match previous_content {
        Some(content) => {
            write_text_file_synced(final_path, &content)?;
        }

        None => {
            remove_file_if_exists(final_path)?;
        }
    }

    Ok(())
}

fn verify_database_file(path: &Path, expected_database_name: &str) -> Result<(), SqdbError> {
    let content = fs::read_to_string(path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not verify database file `{}`: {}",
            path.display(),
            err
        ))
    })?;

    let database: Database = serde_json::from_str(&content).map_err(|err| {
        SqdbError::IoError(format!(
            "Database verification failed for `{}`: {}",
            path.display(),
            err
        ))
    })?;

    if database.name != expected_database_name {
        return Err(SqdbError::IoError(format!(
            "Database verification failed. Expected `{}`, found `{}`.",
            expected_database_name, database.name
        )));
    }

    Ok(())
}

fn is_valid_database_file(path: &Path, expected_database_name: &str) -> bool {
    verify_database_file(path, expected_database_name).is_ok()
}