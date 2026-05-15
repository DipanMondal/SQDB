use std::convert::TryInto;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;

use crate::error::SqdbError;
use crate::language::ast::{DataType, TableType};

const PAGE_SIZE: usize = 4096;
const PAGE_SIZE_U64: u64 = PAGE_SIZE as u64;

const MAGIC: &[u8; 8] = b"SQDBPG01";
const VERSION: u32 = 1;

const HEADER_PAGE_ID: u64 = 0;
const TABLE_DIRECTORY_PAGE_ID: u64 = 1;
const FIRST_DATA_PAGE_ID: u64 = 2;

const PAGE_KIND_TABLE_DIRECTORY: u8 = 1;
const PAGE_KIND_DATA: u8 = 2;
const PAGE_KIND_FREE: u8 = 255;

const HEADER_MAGIC_OFFSET: usize = 0;
const HEADER_VERSION_OFFSET: usize = 8;
const HEADER_PAGE_SIZE_OFFSET: usize = 12;
const HEADER_NEXT_PAGE_ID_OFFSET: usize = 16;
const HEADER_FREE_PAGE_HEAD_OFFSET: usize = 24;
const HEADER_TABLE_COUNT_OFFSET: usize = 32;

const TABLE_ENTRY_START_OFFSET: usize = 64;
const TABLE_ENTRY_SIZE: usize = 128;
const MAX_TABLES: usize = (PAGE_SIZE - TABLE_ENTRY_START_OFFSET) / TABLE_ENTRY_SIZE;

const ENTRY_ACTIVE_OFFSET: usize = 0;
const ENTRY_TABLE_TYPE_OFFSET: usize = 1;
const ENTRY_DATA_TYPE_OFFSET: usize = 2;
const ENTRY_TABLE_ID_OFFSET: usize = 4;
const ENTRY_ITEM_COUNT_OFFSET: usize = 8;
const ENTRY_FIRST_PAGE_OFFSET: usize = 16;
const ENTRY_LAST_PAGE_OFFSET: usize = 24;
const ENTRY_NAME_LEN_OFFSET: usize = 32;
const ENTRY_NAME_OFFSET: usize = 40;
const ENTRY_NAME_MAX_BYTES: usize = 80;

const FREE_PAGE_NEXT_OFFSET: usize = 8;

const DATA_TABLE_ID_OFFSET: usize = 4;
const DATA_PREV_PAGE_OFFSET: usize = 8;
const DATA_NEXT_PAGE_OFFSET: usize = 16;
const DATA_ITEM_COUNT_OFFSET: usize = 24;
const DATA_START_OFFSET_OFFSET: usize = 28;
const DATA_END_OFFSET_OFFSET: usize = 30;
const DATA_HEADER_SIZE: usize = 64;

const RECORD_LENGTH_SIZE: usize = 4;
const RECORD_OVERHEAD: usize = RECORD_LENGTH_SIZE * 2;

const JOURNAL_MAGIC: &[u8; 8] = b"SQDBJNL1";
const JOURNAL_VERSION: u32 = 1;
const JOURNAL_HEADER_SIZE: usize = PAGE_SIZE;
const JOURNAL_HEADER_SIZE_U64: u64 = JOURNAL_HEADER_SIZE as u64;
const JOURNAL_ENTRY_SIZE: u64 = 8 + PAGE_SIZE_U64;

const JOURNAL_MAGIC_OFFSET: usize = 0;
const JOURNAL_VERSION_OFFSET: usize = 8;
const JOURNAL_PAGE_SIZE_OFFSET: usize = 12;
const JOURNAL_ENTRY_COUNT_OFFSET: usize = 16;
const JOURNAL_ORIGINAL_FILE_LEN_OFFSET: usize = 24;
const JOURNAL_DB_NAME_LEN_OFFSET: usize = 32;
const JOURNAL_DB_NAME_OFFSET: usize = 64;
const JOURNAL_DB_NAME_MAX_BYTES: usize = 256;

#[derive(Debug, Clone)]
pub struct BinaryPageStorage {
    base_dir: PathBuf,
}

pub struct BinaryDatabaseHandle {
    db_name: String,
    path: PathBuf,
    file: File,
}

#[derive(Debug, Clone)]
pub struct TableInfo {
    pub name: String,
    pub table_type: TableType,
    pub data_type: DataType,
    pub item_count: u64,
    pub first_page: u64,
    pub last_page: u64,
    slot_index: usize,
}

#[derive(Debug, Clone, Copy)]
struct BinaryHeader {
    next_page_id: u64,
    free_page_head: u64,
    table_count: u32,
}

#[derive(Debug, Clone)]
struct JournalHeader {
    entry_count: u64,
    original_file_len: u64,
    db_name: String,
}

impl BinaryPageStorage {
    pub fn new() -> Self {
        Self {
            base_dir: PathBuf::from("."),
        }
    }

    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn database_exists(&self, database_name: &str) -> bool {
        self.database_path(database_name).exists()
    }

    pub fn create_database(&self, database_name: &str) -> Result<(), SqdbError> {
        validate_database_name(database_name)?;

        let path = self.database_path(database_name);

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|err| {
                SqdbError::IoError(format!(
                    "Could not create binary database `{}`: {}",
                    path.display(),
                    err
                ))
            })?;

        let header = BinaryHeader {
            next_page_id: FIRST_DATA_PAGE_ID,
            free_page_head: 0,
            table_count: 0,
        };

        write_header_to_file(&mut file, header)?;

        let mut table_directory_page = [0u8; PAGE_SIZE];
        table_directory_page[0] = PAGE_KIND_TABLE_DIRECTORY;

        write_page_to_file(&mut file, TABLE_DIRECTORY_PAGE_ID, &table_directory_page)?;

        file.sync_all().map_err(|err| {
            SqdbError::IoError(format!(
                "Could not sync binary database `{}`: {}",
                path.display(),
                err
            ))
        })?;

        Ok(())
    }

    pub fn open_database(&self, database_name: &str) -> Result<BinaryDatabaseHandle, SqdbError> {
        validate_database_name(database_name)?;
		
		self.recover_if_needed(database_name)?;
		
        let path = self.database_path(database_name);

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|err| {
                SqdbError::IoError(format!(
                    "Could not open binary database `{}`: {}",
                    path.display(),
                    err
                ))
            })?;

        let _header = read_header_from_file(&mut file)?;

        let table_directory_page = read_page_from_file(&mut file, TABLE_DIRECTORY_PAGE_ID)?;

        if table_directory_page[0] != PAGE_KIND_TABLE_DIRECTORY {
            return Err(SqdbError::IoError(format!(
                "Database `{}` is invalid: table directory page is missing.",
                database_name
            )));
        }

        Ok(BinaryDatabaseHandle {
            db_name: database_name.to_string(),
            path,
            file,
        })
    }

    pub fn open_or_create_database(
        &self,
        database_name: &str,
    ) -> Result<BinaryDatabaseHandle, SqdbError> {
        if !self.database_exists(database_name) {
            self.create_database(database_name)?;
        }

        self.open_database(database_name)
    }

    pub fn drop_database(&self, database_name: &str) -> Result<(), SqdbError> {
		validate_database_name(database_name)?;

		let path = self.database_path(database_name);
		let journal_path = self.journal_path(database_name);

		if path.exists() {
			fs::remove_file(&path).map_err(|err| {
				SqdbError::IoError(format!(
					"Could not drop binary database `{}`: {}",
					path.display(),
					err
				))
			})?;
		}

		if journal_path.exists() {
			fs::remove_file(&journal_path).map_err(|err| {
				SqdbError::IoError(format!(
					"Could not remove binary journal `{}`: {}",
					journal_path.display(),
					err
				))
			})?;
		}

		Ok(())
	}

    fn database_path(&self, database_name: &str) -> PathBuf {
        self.base_dir.join(format!("{}.sqdb", database_name))
    }
	
	fn journal_path(&self, database_name: &str) -> PathBuf {
		self.base_dir.join(format!("{}.sqdb.journal", database_name))
	}
	
	pub fn recover_if_needed(&self, database_name: &str) -> Result<(), SqdbError> {
        validate_database_name(database_name)?;

        let journal_path = self.journal_path(database_name);

        if !journal_path.exists() {
            return Ok(());
        }

        let database_path = self.database_path(database_name);

        if !database_path.exists() {
            return Err(SqdbError::IoError(format!(
                "Journal exists for `{}`, but database file is missing.",
                database_name
            )));
        }

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&database_path)
            .map_err(|err| {
                SqdbError::IoError(format!(
                    "Could not open database `{}` for journal recovery: {}",
                    database_path.display(),
                    err
                ))
            })?;

        rollback_journal_file(&mut file, &journal_path, database_name)?;

        file.sync_all().map_err(|err| {
            SqdbError::IoError(format!(
                "Could not sync recovered database `{}`: {}",
                database_path.display(),
                err
            ))
        })?;

        fs::remove_file(&journal_path).map_err(|err| {
            SqdbError::IoError(format!(
                "Could not remove recovered journal `{}`: {}",
                journal_path.display(),
                err
            ))
        })?;

        Ok(())
    }
}

impl Default for BinaryPageStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl BinaryDatabaseHandle {
    pub fn database_name(&self) -> &str {
        &self.db_name
    }

    pub fn database_path(&self) -> &PathBuf {
        &self.path
    }
	
	pub fn commit_transaction(&mut self) -> Result<(), SqdbError> {
        self.file.sync_all().map_err(|err| {
            SqdbError::IoError(format!(
                "Could not sync database `{}` before commit: {}",
                self.path.display(),
                err
            ))
        })?;

        let journal_path = self.journal_path();

        if journal_path.exists() {
            fs::remove_file(&journal_path).map_err(|err| {
                SqdbError::IoError(format!(
                    "Could not remove journal `{}` during commit: {}",
                    journal_path.display(),
                    err
                ))
            })?;
        }

        Ok(())
    }

    pub fn rollback_transaction(&mut self) -> Result<(), SqdbError> {
        let journal_path = self.journal_path();

        if !journal_path.exists() {
            return Ok(());
        }

        rollback_journal_file(&mut self.file, &journal_path, &self.db_name)?;

        self.file.sync_all().map_err(|err| {
            SqdbError::IoError(format!(
                "Could not sync database `{}` after rollback: {}",
                self.path.display(),
                err
            ))
        })?;

        fs::remove_file(&journal_path).map_err(|err| {
            SqdbError::IoError(format!(
                "Could not remove journal `{}` after rollback: {}",
                journal_path.display(),
                err
            ))
        })?;

        Ok(())
    }

    fn journal_path(&self) -> PathBuf {
        self.path
            .with_file_name(format!("{}.sqdb.journal", self.db_name))
    }

    fn create_journal_if_needed(&mut self) -> Result<(), SqdbError> {
        let journal_path = self.journal_path();

        if journal_path.exists() {
            return Ok(());
        }

        let original_file_len = self.file.metadata().map_err(|err| {
            SqdbError::IoError(format!(
                "Could not read database file metadata for `{}`: {}",
                self.path.display(),
                err
            ))
        })?.len();

        let header = JournalHeader {
            entry_count: 0,
            original_file_len,
            db_name: self.db_name.clone(),
        };

        write_new_journal_file(&journal_path, &header)?;

        Ok(())
    }

    fn journal_page_if_needed(&mut self, page_id: u64) -> Result<(), SqdbError> {
        self.create_journal_if_needed()?;

        let journal_path = self.journal_path();
        let journal_header = read_journal_header_from_path(&journal_path)?;

        if journal_header.db_name != self.db_name {
            return Err(SqdbError::IoError(format!(
                "Journal database mismatch. Expected `{}`, found `{}`.",
                self.db_name, journal_header.db_name
            )));
        }

        let page_start = page_id * PAGE_SIZE_U64;

        // If this page did not exist when the transaction started,
        // rollback will remove it by truncating the file.
        if page_start >= journal_header.original_file_len {
            return Ok(());
        }

        if journal_contains_page(&journal_path, page_id)? {
            return Ok(());
        }

        let old_page = read_page_from_file(&mut self.file, page_id)?;

        append_journal_entry(&journal_path, page_id, &old_page)?;

        Ok(())
    }

    pub fn create_table(
        &mut self,
        table_name: &str,
        table_type: TableType,
        data_type: DataType,
    ) -> Result<(), SqdbError> {
        validate_table_name(table_name)?;

        if self.find_table(table_name)?.is_some() {
            return Err(SqdbError::RuntimeError(format!(
                "Table `{}` already exists.",
                table_name
            )));
        }

        let free_slot = self.find_free_table_slot()?.ok_or_else(|| {
            SqdbError::RuntimeError(format!(
                "Maximum table limit reached. Current binary format supports {} tables.",
                MAX_TABLES
            ))
        })?;

        let table_info = TableInfo {
            name: table_name.to_string(),
            table_type,
            data_type,
            item_count: 0,
            first_page: 0,
            last_page: 0,
            slot_index: free_slot,
        };

        self.write_table_entry(&table_info)?;

        let mut header = self.read_header()?;
        header.table_count += 1;
        self.write_header(header)?;

        self.file.sync_all().map_err(|err| {
            SqdbError::IoError(format!(
                "Could not sync table creation for `{}`: {}",
                table_name, err
            ))
        })?;

        Ok(())
    }

    pub fn drop_table(&mut self, table_name: &str) -> Result<(), SqdbError> {
        let table = self.find_table(table_name)?.ok_or_else(|| {
            SqdbError::RuntimeError(format!("Table `{}` does not exist.", table_name))
        })?;

        let mut current_page_id = table.first_page;

        while current_page_id != 0 {
            let page = self.read_page(current_page_id)?;

            validate_data_page_for_table(&page, table.slot_index)?;

            let next_page_id = data_next_page(&page);

            self.free_page(current_page_id)?;

            current_page_id = next_page_id;
        }

        self.clear_table_entry(table.slot_index)?;

        let mut header = self.read_header()?;
        header.table_count = header.table_count.saturating_sub(1);
        self.write_header(header)?;

        self.file.sync_all().map_err(|err| {
            SqdbError::IoError(format!(
                "Could not sync table drop for `{}`: {}",
                table_name, err
            ))
        })?;

        Ok(())
    }

    pub fn show_tables(&mut self) -> Result<String, SqdbError> {
        let tables = self.load_tables()?;

        if tables.is_empty() {
            return Ok("No tables found.".to_string());
        }

        let mut output = String::new();

        output.push_str("Tables:\n");
        output.push_str("----------------------------------------------------------\n");
        output.push_str("Name                 Type      DType     Items     Pages\n");
        output.push_str("----------------------------------------------------------\n");

        for table in tables {
            let page_summary = if table.first_page == 0 {
                "-".to_string()
            } else if table.first_page == table.last_page {
                format!("{}", table.first_page)
            } else {
                format!("{}..{}", table.first_page, table.last_page)
            };

            output.push_str(&format!(
                "{:<20} {:<9} {:<9} {:<9} {}\n",
                table.name,
                table.table_type,
                table.data_type,
                table.item_count,
                page_summary
            ));
        }

        Ok(output)
    }

    pub fn get_table_type(&mut self, table_name: &str) -> Result<TableType, SqdbError> {
        let table = self.find_table(table_name)?.ok_or_else(|| {
            SqdbError::RuntimeError(format!("Table `{}` does not exist.", table_name))
        })?;

        Ok(table.table_type)
    }

    pub fn get_table_dtype(&mut self, table_name: &str) -> Result<DataType, SqdbError> {
        let table = self.find_table(table_name)?.ok_or_else(|| {
            SqdbError::RuntimeError(format!("Table `{}` does not exist.", table_name))
        })?;

        Ok(table.data_type)
    }

    pub fn insert_raw(&mut self, table_name: &str, raw_value: &str) -> Result<(), SqdbError> {
        let mut table = self.find_table(table_name)?.ok_or_else(|| {
            SqdbError::RuntimeError(format!("Table `{}` does not exist.", table_name))
        })?;

        let payload = encode_raw_value(&table.data_type, raw_value)?;

        let required_record_size = payload.len() + RECORD_OVERHEAD;

        if required_record_size > PAGE_SIZE - DATA_HEADER_SIZE {
            return Err(SqdbError::RuntimeError(format!(
                "Value is too large for one page. Maximum payload is {} bytes.",
                PAGE_SIZE - DATA_HEADER_SIZE - RECORD_OVERHEAD
            )));
        }

        let table_id = table_id_from_slot(table.slot_index);

        if table.last_page == 0 {
            let new_page_id = self.allocate_page()?;

            let mut page = new_data_page(table_id, 0, 0);
            append_record_to_data_page(&mut page, &payload)?;

            self.write_page(new_page_id, &page)?;

            table.first_page = new_page_id;
            table.last_page = new_page_id;
        } else {
            let mut last_page = self.read_page(table.last_page)?;

            validate_data_page_for_table(&last_page, table.slot_index)?;

            if data_available_space(&last_page) < required_record_size {
                compact_data_page(&mut last_page)?;

                if data_available_space(&last_page) < required_record_size {
                    let new_page_id = self.allocate_page()?;

                    set_data_next_page(&mut last_page, new_page_id);
                    self.write_page(table.last_page, &last_page)?;

                    let mut new_page = new_data_page(table_id, table.last_page, 0);
                    append_record_to_data_page(&mut new_page, &payload)?;

                    self.write_page(new_page_id, &new_page)?;

                    table.last_page = new_page_id;
                } else {
                    append_record_to_data_page(&mut last_page, &payload)?;
                    self.write_page(table.last_page, &last_page)?;
                }
            } else {
                append_record_to_data_page(&mut last_page, &payload)?;
                self.write_page(table.last_page, &last_page)?;
            }
        }

        table.item_count += 1;
        self.write_table_entry(&table)?;

        self.file.sync_all().map_err(|err| {
            SqdbError::IoError(format!(
                "Could not sync insert into `{}`: {}",
                table_name, err
            ))
        })?;

        Ok(())
    }

    pub fn read_value(&mut self, table_name: &str) -> Result<String, SqdbError> {
        let table = self.find_table(table_name)?.ok_or_else(|| {
            SqdbError::RuntimeError(format!("Table `{}` does not exist.", table_name))
        })?;

        if table.item_count == 0 {
            return Ok(format!("{}.None", self.db_name));
        }

        let page_id = match table.table_type {
            TableType::Stack => table.last_page,
            TableType::Queue => table.first_page,
        };

        if page_id == 0 {
            return Err(SqdbError::IoError(format!(
                "Table `{}` is corrupted: item count is non-zero but page pointer is zero.",
                table_name
            )));
        }

        let page = self.read_page(page_id)?;

        validate_data_page_for_table(&page, table.slot_index)?;

        let payload = match table.table_type {
            TableType::Stack => read_last_record_payload(&page)?,
            TableType::Queue => {
                let (payload, _) = read_first_record_payload(&page)?;
                payload
            }
        };

        decode_payload_value(&table.data_type, &payload)
    }

    pub fn delete_value(&mut self, table_name: &str) -> Result<String, SqdbError> {
        let mut table = self.find_table(table_name)?.ok_or_else(|| {
            SqdbError::RuntimeError(format!("Table `{}` does not exist.", table_name))
        })?;

        if table.item_count == 0 {
            return Ok(format!("{}.None", self.db_name));
        }

        let result = match table.table_type {
            TableType::Stack => self.delete_from_stack(&mut table)?,
            TableType::Queue => self.delete_from_queue(&mut table)?,
        };

        table.item_count = table.item_count.saturating_sub(1);

        if table.item_count == 0 {
            table.first_page = 0;
            table.last_page = 0;
        }

        self.write_table_entry(&table)?;

        self.file.sync_all().map_err(|err| {
            SqdbError::IoError(format!(
                "Could not sync delete from `{}`: {}",
                table_name, err
            ))
        })?;

        Ok(result)
    }

    pub fn allocate_page(&mut self) -> Result<u64, SqdbError> {
        let mut header = self.read_header()?;

        if header.free_page_head != 0 {
            let allocated_page_id = header.free_page_head;
            let free_page = self.read_page(allocated_page_id)?;

            if free_page[0] != PAGE_KIND_FREE {
                return Err(SqdbError::IoError(format!(
                    "Free page list is corrupted. Page {} is not marked free.",
                    allocated_page_id
                )));
            }

            let next_free_page = read_u64(&free_page, FREE_PAGE_NEXT_OFFSET);

            header.free_page_head = next_free_page;
            self.write_header(header)?;

            let empty_page = [0u8; PAGE_SIZE];
            self.write_page(allocated_page_id, &empty_page)?;

            Ok(allocated_page_id)
        } else {
            let allocated_page_id = header.next_page_id;

            header.next_page_id += 1;
            self.write_header(header)?;

            let empty_page = [0u8; PAGE_SIZE];
            self.write_page(allocated_page_id, &empty_page)?;

            Ok(allocated_page_id)
        }
    }

    pub fn free_page(&mut self, page_id: u64) -> Result<(), SqdbError> {
        if page_id <= TABLE_DIRECTORY_PAGE_ID {
            return Err(SqdbError::RuntimeError(format!(
                "Cannot free reserved page {}.",
                page_id
            )));
        }

        let mut header = self.read_header()?;

        let mut free_page = [0u8; PAGE_SIZE];
        free_page[0] = PAGE_KIND_FREE;
        write_u64(&mut free_page, FREE_PAGE_NEXT_OFFSET, header.free_page_head);

        self.write_page(page_id, &free_page)?;

        header.free_page_head = page_id;
        self.write_header(header)?;

        Ok(())
    }

    fn delete_from_stack(&mut self, table: &mut TableInfo) -> Result<String, SqdbError> {
        let page_id = table.last_page;

        if page_id == 0 {
            return Err(SqdbError::IoError(format!(
                "Table `{}` is corrupted: stack last page is zero.",
                table.name
            )));
        }

        let mut page = self.read_page(page_id)?;

        validate_data_page_for_table(&page, table.slot_index)?;

        let previous_page_id = data_prev_page(&page);

        let payload = remove_last_record_from_data_page(&mut page)?;

        let result = decode_payload_value(&table.data_type, &payload)?;

        if data_item_count(&page) == 0 {
            self.free_page(page_id)?;

            if previous_page_id != 0 {
                let mut previous_page = self.read_page(previous_page_id)?;
                validate_data_page_for_table(&previous_page, table.slot_index)?;
                set_data_next_page(&mut previous_page, 0);
                self.write_page(previous_page_id, &previous_page)?;
            }

            table.last_page = previous_page_id;

            if table.first_page == page_id {
                table.first_page = 0;
            }
        } else {
            self.write_page(page_id, &page)?;
        }

        Ok(result)
    }

    fn delete_from_queue(&mut self, table: &mut TableInfo) -> Result<String, SqdbError> {
        let page_id = table.first_page;

        if page_id == 0 {
            return Err(SqdbError::IoError(format!(
                "Table `{}` is corrupted: queue first page is zero.",
                table.name
            )));
        }

        let mut page = self.read_page(page_id)?;

        validate_data_page_for_table(&page, table.slot_index)?;

        let next_page_id = data_next_page(&page);

        let payload = remove_first_record_from_data_page(&mut page)?;

        let result = decode_payload_value(&table.data_type, &payload)?;

        if data_item_count(&page) == 0 {
            self.free_page(page_id)?;

            if next_page_id != 0 {
                let mut next_page = self.read_page(next_page_id)?;
                validate_data_page_for_table(&next_page, table.slot_index)?;
                set_data_prev_page(&mut next_page, 0);
                self.write_page(next_page_id, &next_page)?;
            }

            table.first_page = next_page_id;

            if table.last_page == page_id {
                table.last_page = 0;
            }
        } else {
            self.write_page(page_id, &page)?;
        }

        Ok(result)
    }

    fn read_header(&mut self) -> Result<BinaryHeader, SqdbError> {
        read_header_from_file(&mut self.file)
    }

    fn write_header(&mut self, header: BinaryHeader) -> Result<(), SqdbError> {
		self.journal_page_if_needed(HEADER_PAGE_ID)?;
		write_header_to_file(&mut self.file, header)
	}

    fn read_page(&mut self, page_id: u64) -> Result<[u8; PAGE_SIZE], SqdbError> {
        read_page_from_file(&mut self.file, page_id)
    }

    fn write_page(&mut self, page_id: u64, page: &[u8; PAGE_SIZE]) -> Result<(), SqdbError> {
		self.journal_page_if_needed(page_id)?;
		write_page_to_file(&mut self.file, page_id, page)
	}

    fn load_tables(&mut self) -> Result<Vec<TableInfo>, SqdbError> {
        let directory_page = self.read_page(TABLE_DIRECTORY_PAGE_ID)?;

        let mut tables = Vec::new();

        for slot_index in 0..MAX_TABLES {
            let entry_offset = table_entry_offset(slot_index);

            if directory_page[entry_offset + ENTRY_ACTIVE_OFFSET] != 1 {
                continue;
            }

            let table = decode_table_entry(&directory_page, slot_index)?;

            tables.push(table);
        }

        tables.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(tables)
    }

    fn find_table(&mut self, table_name: &str) -> Result<Option<TableInfo>, SqdbError> {
        let directory_page = self.read_page(TABLE_DIRECTORY_PAGE_ID)?;

        for slot_index in 0..MAX_TABLES {
            let entry_offset = table_entry_offset(slot_index);

            if directory_page[entry_offset + ENTRY_ACTIVE_OFFSET] != 1 {
                continue;
            }

            let table = decode_table_entry(&directory_page, slot_index)?;

            if table.name == table_name {
                return Ok(Some(table));
            }
        }

        Ok(None)
    }

    fn find_free_table_slot(&mut self) -> Result<Option<usize>, SqdbError> {
        let directory_page = self.read_page(TABLE_DIRECTORY_PAGE_ID)?;

        for slot_index in 0..MAX_TABLES {
            let entry_offset = table_entry_offset(slot_index);

            if directory_page[entry_offset + ENTRY_ACTIVE_OFFSET] == 0 {
                return Ok(Some(slot_index));
            }
        }

        Ok(None)
    }

    fn write_table_entry(&mut self, table: &TableInfo) -> Result<(), SqdbError> {
        let mut directory_page = self.read_page(TABLE_DIRECTORY_PAGE_ID)?;

        encode_table_entry(&mut directory_page, table)?;

        self.write_page(TABLE_DIRECTORY_PAGE_ID, &directory_page)
    }

    fn clear_table_entry(&mut self, slot_index: usize) -> Result<(), SqdbError> {
        let mut directory_page = self.read_page(TABLE_DIRECTORY_PAGE_ID)?;

        let entry_offset = table_entry_offset(slot_index);

        for index in entry_offset..entry_offset + TABLE_ENTRY_SIZE {
            directory_page[index] = 0;
        }

        self.write_page(TABLE_DIRECTORY_PAGE_ID, &directory_page)
    }
}

fn read_header_from_file(file: &mut File) -> Result<BinaryHeader, SqdbError> {
    let page = read_page_from_file(file, HEADER_PAGE_ID)?;

    if &page[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 8] != MAGIC {
        return Err(SqdbError::IoError(
            "Invalid SQDB binary file: magic number mismatch.".to_string(),
        ));
    }

    let version = read_u32(&page, HEADER_VERSION_OFFSET);

    if version != VERSION {
        return Err(SqdbError::IoError(format!(
            "Unsupported SQDB binary version {}. Expected {}.",
            version, VERSION
        )));
    }

    let page_size = read_u32(&page, HEADER_PAGE_SIZE_OFFSET);

    if page_size != PAGE_SIZE as u32 {
        return Err(SqdbError::IoError(format!(
            "Invalid page size {}. Expected {}.",
            page_size, PAGE_SIZE
        )));
    }

    Ok(BinaryHeader {
        next_page_id: read_u64(&page, HEADER_NEXT_PAGE_ID_OFFSET),
        free_page_head: read_u64(&page, HEADER_FREE_PAGE_HEAD_OFFSET),
        table_count: read_u32(&page, HEADER_TABLE_COUNT_OFFSET),
    })
}

fn write_header_to_file(file: &mut File, header: BinaryHeader) -> Result<(), SqdbError> {
    let mut page = [0u8; PAGE_SIZE];

    page[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 8].copy_from_slice(MAGIC);

    write_u32(&mut page, HEADER_VERSION_OFFSET, VERSION);
    write_u32(&mut page, HEADER_PAGE_SIZE_OFFSET, PAGE_SIZE as u32);
    write_u64(&mut page, HEADER_NEXT_PAGE_ID_OFFSET, header.next_page_id);
    write_u64(&mut page, HEADER_FREE_PAGE_HEAD_OFFSET, header.free_page_head);
    write_u32(&mut page, HEADER_TABLE_COUNT_OFFSET, header.table_count);

    write_page_to_file(file, HEADER_PAGE_ID, &page)
}

fn read_page_from_file(file: &mut File, page_id: u64) -> Result<[u8; PAGE_SIZE], SqdbError> {
    let mut page = [0u8; PAGE_SIZE];

    file.seek(SeekFrom::Start(page_id * PAGE_SIZE_U64))
        .map_err(|err| {
            SqdbError::IoError(format!("Could not seek to page {}: {}", page_id, err))
        })?;

    file.read_exact(&mut page)
        .map_err(|err| SqdbError::IoError(format!("Could not read page {}: {}", page_id, err)))?;

    Ok(page)
}

fn write_page_to_file(
    file: &mut File,
    page_id: u64,
    page: &[u8; PAGE_SIZE],
) -> Result<(), SqdbError> {
    file.seek(SeekFrom::Start(page_id * PAGE_SIZE_U64))
        .map_err(|err| {
            SqdbError::IoError(format!("Could not seek to page {}: {}", page_id, err))
        })?;

    file.write_all(page)
        .map_err(|err| SqdbError::IoError(format!("Could not write page {}: {}", page_id, err)))?;

    Ok(())
}

fn table_entry_offset(slot_index: usize) -> usize {
    TABLE_ENTRY_START_OFFSET + slot_index * TABLE_ENTRY_SIZE
}

fn encode_table_entry(page: &mut [u8; PAGE_SIZE], table: &TableInfo) -> Result<(), SqdbError> {
    if table.slot_index >= MAX_TABLES {
        return Err(SqdbError::RuntimeError(format!(
            "Invalid table slot index {}.",
            table.slot_index
        )));
    }

    let name_bytes = table.name.as_bytes();

    if name_bytes.len() > ENTRY_NAME_MAX_BYTES {
        return Err(SqdbError::RuntimeError(format!(
            "Table name `{}` is too long. Maximum is {} bytes.",
            table.name, ENTRY_NAME_MAX_BYTES
        )));
    }

    let entry_offset = table_entry_offset(table.slot_index);

    for index in entry_offset..entry_offset + TABLE_ENTRY_SIZE {
        page[index] = 0;
    }

    page[entry_offset + ENTRY_ACTIVE_OFFSET] = 1;
    page[entry_offset + ENTRY_TABLE_TYPE_OFFSET] = table_type_to_u8(&table.table_type);
    page[entry_offset + ENTRY_DATA_TYPE_OFFSET] = data_type_to_u8(&table.data_type);

    write_u32(
        page,
        entry_offset + ENTRY_TABLE_ID_OFFSET,
        table_id_from_slot(table.slot_index),
    );

    write_u64(
        page,
        entry_offset + ENTRY_ITEM_COUNT_OFFSET,
        table.item_count,
    );

    write_u64(
        page,
        entry_offset + ENTRY_FIRST_PAGE_OFFSET,
        table.first_page,
    );

    write_u64(
        page,
        entry_offset + ENTRY_LAST_PAGE_OFFSET,
        table.last_page,
    );

    write_u16(
        page,
        entry_offset + ENTRY_NAME_LEN_OFFSET,
        name_bytes.len() as u16,
    );

    let name_start = entry_offset + ENTRY_NAME_OFFSET;
    let name_end = name_start + name_bytes.len();

    page[name_start..name_end].copy_from_slice(name_bytes);

    Ok(())
}

fn decode_table_entry(page: &[u8; PAGE_SIZE], slot_index: usize) -> Result<TableInfo, SqdbError> {
    let entry_offset = table_entry_offset(slot_index);

    if page[entry_offset + ENTRY_ACTIVE_OFFSET] != 1 {
        return Err(SqdbError::RuntimeError(format!(
            "Table slot {} is not active.",
            slot_index
        )));
    }

    let table_type = u8_to_table_type(page[entry_offset + ENTRY_TABLE_TYPE_OFFSET])?;
    let data_type = u8_to_data_type(page[entry_offset + ENTRY_DATA_TYPE_OFFSET])?;

    let item_count = read_u64(page, entry_offset + ENTRY_ITEM_COUNT_OFFSET);
    let first_page = read_u64(page, entry_offset + ENTRY_FIRST_PAGE_OFFSET);
    let last_page = read_u64(page, entry_offset + ENTRY_LAST_PAGE_OFFSET);

    let name_len = read_u16(page, entry_offset + ENTRY_NAME_LEN_OFFSET) as usize;

    if name_len > ENTRY_NAME_MAX_BYTES {
        return Err(SqdbError::IoError(format!(
            "Corrupted table directory: table name length {} is invalid.",
            name_len
        )));
    }

    let name_start = entry_offset + ENTRY_NAME_OFFSET;
    let name_end = name_start + name_len;

    let name = String::from_utf8(page[name_start..name_end].to_vec()).map_err(|err| {
        SqdbError::IoError(format!(
            "Corrupted table directory: invalid UTF-8 table name: {}",
            err
        ))
    })?;

    Ok(TableInfo {
        name,
        table_type,
        data_type,
        item_count,
        first_page,
        last_page,
        slot_index,
    })
}

fn table_id_from_slot(slot_index: usize) -> u32 {
    (slot_index + 1) as u32
}

fn new_data_page(table_id: u32, previous_page: u64, next_page: u64) -> [u8; PAGE_SIZE] {
    let mut page = [0u8; PAGE_SIZE];

    page[0] = PAGE_KIND_DATA;

    write_u32(&mut page, DATA_TABLE_ID_OFFSET, table_id);
    write_u64(&mut page, DATA_PREV_PAGE_OFFSET, previous_page);
    write_u64(&mut page, DATA_NEXT_PAGE_OFFSET, next_page);
    write_u32(&mut page, DATA_ITEM_COUNT_OFFSET, 0);
    write_u16(&mut page, DATA_START_OFFSET_OFFSET, DATA_HEADER_SIZE as u16);
    write_u16(&mut page, DATA_END_OFFSET_OFFSET, DATA_HEADER_SIZE as u16);

    page
}

fn validate_data_page_for_table(
    page: &[u8; PAGE_SIZE],
    slot_index: usize,
) -> Result<(), SqdbError> {
    if page[0] != PAGE_KIND_DATA {
        return Err(SqdbError::IoError(
            "Expected data page, but page kind is different.".to_string(),
        ));
    }

    let expected_table_id = table_id_from_slot(slot_index);
    let actual_table_id = read_u32(page, DATA_TABLE_ID_OFFSET);

    if actual_table_id != expected_table_id {
        return Err(SqdbError::IoError(format!(
            "Data page belongs to table id {}, expected table id {}.",
            actual_table_id, expected_table_id
        )));
    }

    Ok(())
}

fn data_prev_page(page: &[u8; PAGE_SIZE]) -> u64 {
    read_u64(page, DATA_PREV_PAGE_OFFSET)
}

fn data_next_page(page: &[u8; PAGE_SIZE]) -> u64 {
    read_u64(page, DATA_NEXT_PAGE_OFFSET)
}

fn set_data_prev_page(page: &mut [u8; PAGE_SIZE], previous_page: u64) {
    write_u64(page, DATA_PREV_PAGE_OFFSET, previous_page);
}

fn set_data_next_page(page: &mut [u8; PAGE_SIZE], next_page: u64) {
    write_u64(page, DATA_NEXT_PAGE_OFFSET, next_page);
}

fn data_item_count(page: &[u8; PAGE_SIZE]) -> u32 {
    read_u32(page, DATA_ITEM_COUNT_OFFSET)
}

fn set_data_item_count(page: &mut [u8; PAGE_SIZE], count: u32) {
    write_u32(page, DATA_ITEM_COUNT_OFFSET, count);
}

fn data_start_offset(page: &[u8; PAGE_SIZE]) -> usize {
    read_u16(page, DATA_START_OFFSET_OFFSET) as usize
}

fn data_end_offset(page: &[u8; PAGE_SIZE]) -> usize {
    read_u16(page, DATA_END_OFFSET_OFFSET) as usize
}

fn set_data_start_offset(page: &mut [u8; PAGE_SIZE], offset: usize) {
    write_u16(page, DATA_START_OFFSET_OFFSET, offset as u16);
}

fn set_data_end_offset(page: &mut [u8; PAGE_SIZE], offset: usize) {
    write_u16(page, DATA_END_OFFSET_OFFSET, offset as u16);
}

fn data_available_space(page: &[u8; PAGE_SIZE]) -> usize {
    PAGE_SIZE - data_end_offset(page)
}

fn append_record_to_data_page(
    page: &mut [u8; PAGE_SIZE],
    payload: &[u8],
) -> Result<(), SqdbError> {
    let count = data_item_count(page);
    let mut end_offset = data_end_offset(page);

    let record_size = payload.len() + RECORD_OVERHEAD;

    if PAGE_SIZE - end_offset < record_size {
        return Err(SqdbError::RuntimeError(
            "Not enough space in data page.".to_string(),
        ));
    }

    if count == 0 {
        set_data_start_offset(page, DATA_HEADER_SIZE);
        end_offset = DATA_HEADER_SIZE;
    }

    write_u32(page, end_offset, payload.len() as u32);

    let payload_start = end_offset + RECORD_LENGTH_SIZE;
    let payload_end = payload_start + payload.len();

    page[payload_start..payload_end].copy_from_slice(payload);

    write_u32(page, payload_end, payload.len() as u32);

    let new_end_offset = payload_end + RECORD_LENGTH_SIZE;

    set_data_end_offset(page, new_end_offset);
    set_data_item_count(page, count + 1);

    Ok(())
}

fn compact_data_page(page: &mut [u8; PAGE_SIZE]) -> Result<(), SqdbError> {
    let count = data_item_count(page);

    if count == 0 {
        set_data_start_offset(page, DATA_HEADER_SIZE);
        set_data_end_offset(page, DATA_HEADER_SIZE);
        return Ok(());
    }

    let start_offset = data_start_offset(page);
    let end_offset = data_end_offset(page);

    if start_offset < DATA_HEADER_SIZE || end_offset < start_offset || end_offset > PAGE_SIZE {
        return Err(SqdbError::IoError(
            "Cannot compact corrupted data page offsets.".to_string(),
        ));
    }

    if start_offset == DATA_HEADER_SIZE {
        return Ok(());
    }

    let live_len = end_offset - start_offset;

    page.copy_within(start_offset..end_offset, DATA_HEADER_SIZE);

    let new_end_offset = DATA_HEADER_SIZE + live_len;

    for byte in &mut page[new_end_offset..end_offset] {
        *byte = 0;
    }

    set_data_start_offset(page, DATA_HEADER_SIZE);
    set_data_end_offset(page, new_end_offset);

    Ok(())
}

fn read_first_record_payload(page: &[u8; PAGE_SIZE]) -> Result<(Vec<u8>, usize), SqdbError> {
    let count = data_item_count(page);

    if count == 0 {
        return Err(SqdbError::RuntimeError(
            "Cannot read first record from empty data page.".to_string(),
        ));
    }

    let start_offset = data_start_offset(page);

    read_record_at(page, start_offset)
}

fn read_last_record_payload(page: &[u8; PAGE_SIZE]) -> Result<Vec<u8>, SqdbError> {
    let count = data_item_count(page);

    if count == 0 {
        return Err(SqdbError::RuntimeError(
            "Cannot read last record from empty data page.".to_string(),
        ));
    }

    let end_offset = data_end_offset(page);

    if end_offset < DATA_HEADER_SIZE + RECORD_OVERHEAD {
        return Err(SqdbError::IoError(
            "Corrupted data page: invalid end offset.".to_string(),
        ));
    }

    let length_offset = end_offset - RECORD_LENGTH_SIZE;
    let payload_len = read_u32(page, length_offset) as usize;

    let record_start = end_offset
        .checked_sub(RECORD_LENGTH_SIZE + payload_len + RECORD_LENGTH_SIZE)
        .ok_or_else(|| {
            SqdbError::IoError("Corrupted data page: invalid last record length.".to_string())
        })?;

    let (payload, record_end) = read_record_at(page, record_start)?;

    if record_end != end_offset {
        return Err(SqdbError::IoError(
            "Corrupted data page: last record boundary mismatch.".to_string(),
        ));
    }

    Ok(payload)
}

fn read_record_at(page: &[u8; PAGE_SIZE], offset: usize) -> Result<(Vec<u8>, usize), SqdbError> {
    if offset + RECORD_LENGTH_SIZE > PAGE_SIZE {
        return Err(SqdbError::IoError(
            "Corrupted data page: record offset is out of bounds.".to_string(),
        ));
    }

    let payload_len = read_u32(page, offset) as usize;

    let payload_start = offset + RECORD_LENGTH_SIZE;
    let payload_end = payload_start + payload_len;
    let trailing_len_offset = payload_end;

    if trailing_len_offset + RECORD_LENGTH_SIZE > PAGE_SIZE {
        return Err(SqdbError::IoError(
            "Corrupted data page: record length is out of bounds.".to_string(),
        ));
    }

    let trailing_len = read_u32(page, trailing_len_offset) as usize;

    if trailing_len != payload_len {
        return Err(SqdbError::IoError(
            "Corrupted data page: record length markers do not match.".to_string(),
        ));
    }

    let next_offset = trailing_len_offset + RECORD_LENGTH_SIZE;

    Ok((page[payload_start..payload_end].to_vec(), next_offset))
}

fn remove_last_record_from_data_page(page: &mut [u8; PAGE_SIZE]) -> Result<Vec<u8>, SqdbError> {
    let count = data_item_count(page);

    if count == 0 {
        return Err(SqdbError::RuntimeError(
            "Cannot remove last record from empty data page.".to_string(),
        ));
    }

    let end_offset = data_end_offset(page);
    let length_offset = end_offset - RECORD_LENGTH_SIZE;
    let payload_len = read_u32(page, length_offset) as usize;

    let record_start = end_offset
        .checked_sub(RECORD_LENGTH_SIZE + payload_len + RECORD_LENGTH_SIZE)
        .ok_or_else(|| {
            SqdbError::IoError("Corrupted data page: invalid last record length.".to_string())
        })?;

    let (payload, record_end) = read_record_at(page, record_start)?;

    if record_end != end_offset {
        return Err(SqdbError::IoError(
            "Corrupted data page: last record boundary mismatch.".to_string(),
        ));
    }

    for byte in &mut page[record_start..end_offset] {
        *byte = 0;
    }

    let new_count = count - 1;

    set_data_item_count(page, new_count);

    if new_count == 0 {
        set_data_start_offset(page, DATA_HEADER_SIZE);
        set_data_end_offset(page, DATA_HEADER_SIZE);
    } else {
        set_data_end_offset(page, record_start);
    }

    Ok(payload)
}

fn remove_first_record_from_data_page(page: &mut [u8; PAGE_SIZE]) -> Result<Vec<u8>, SqdbError> {
    let count = data_item_count(page);

    if count == 0 {
        return Err(SqdbError::RuntimeError(
            "Cannot remove first record from empty data page.".to_string(),
        ));
    }

    let start_offset = data_start_offset(page);
    let (payload, next_offset) = read_record_at(page, start_offset)?;

    for byte in &mut page[start_offset..next_offset] {
        *byte = 0;
    }

    let new_count = count - 1;

    set_data_item_count(page, new_count);

    if new_count == 0 {
        set_data_start_offset(page, DATA_HEADER_SIZE);
        set_data_end_offset(page, DATA_HEADER_SIZE);
    } else {
        set_data_start_offset(page, next_offset);
    }

    Ok(payload)
}

fn encode_raw_value(data_type: &DataType, raw_value: &str) -> Result<Vec<u8>, SqdbError> {
    match data_type {
        DataType::Int => {
            let value = raw_value.trim().parse::<i64>().map_err(|_| {
                SqdbError::RuntimeError(format!("Expected int value, found `{}`.", raw_value))
            })?;

            Ok(value.to_le_bytes().to_vec())
        }

        DataType::Real => {
            let value = raw_value.trim().parse::<f64>().map_err(|_| {
                SqdbError::RuntimeError(format!("Expected real value, found `{}`.", raw_value))
            })?;

            Ok(value.to_le_bytes().to_vec())
        }

        DataType::String => {
            let value = parse_string_value(raw_value)?;

            Ok(value.into_bytes())
        }

        DataType::Json => {
            let value = serde_json::from_str::<JsonValue>(raw_value.trim()).map_err(|err| {
                SqdbError::RuntimeError(format!(
                    "Expected valid json value, found `{}`. JSON error: {}",
                    raw_value, err
                ))
            })?;

            serde_json::to_vec(&value).map_err(|err| {
                SqdbError::RuntimeError(format!("Could not encode json value: {}", err))
            })
        }
    }
}

fn decode_payload_value(data_type: &DataType, payload: &[u8]) -> Result<String, SqdbError> {
    match data_type {
        DataType::Int => {
            if payload.len() != 8 {
                return Err(SqdbError::IoError(
                    "Corrupted int payload: expected 8 bytes.".to_string(),
                ));
            }

            let value = i64::from_le_bytes(payload.try_into().unwrap());

            Ok(value.to_string())
        }

        DataType::Real => {
            if payload.len() != 8 {
                return Err(SqdbError::IoError(
                    "Corrupted real payload: expected 8 bytes.".to_string(),
                ));
            }

            let value = f64::from_le_bytes(payload.try_into().unwrap());

            Ok(value.to_string())
        }

        DataType::String => {
            let value = String::from_utf8(payload.to_vec()).map_err(|err| {
                SqdbError::IoError(format!("Corrupted string payload: {}", err))
            })?;

            Ok(value)
        }

        DataType::Json => {
            let value = serde_json::from_slice::<JsonValue>(payload).map_err(|err| {
                SqdbError::IoError(format!("Corrupted json payload: {}", err))
            })?;

            Ok(value.to_string())
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

fn table_type_to_u8(table_type: &TableType) -> u8 {
    match table_type {
        TableType::Stack => 1,
        TableType::Queue => 2,
    }
}

fn u8_to_table_type(value: u8) -> Result<TableType, SqdbError> {
    match value {
        1 => Ok(TableType::Stack),
        2 => Ok(TableType::Queue),
        other => Err(SqdbError::IoError(format!(
            "Invalid table type byte `{}` in binary database.",
            other
        ))),
    }
}

fn data_type_to_u8(data_type: &DataType) -> u8 {
    match data_type {
        DataType::Int => 1,
        DataType::Real => 2,
        DataType::String => 3,
        DataType::Json => 4,
    }
}

fn u8_to_data_type(value: u8) -> Result<DataType, SqdbError> {
    match value {
        1 => Ok(DataType::Int),
        2 => Ok(DataType::Real),
        3 => Ok(DataType::String),
        4 => Ok(DataType::Json),
        other => Err(SqdbError::IoError(format!(
            "Invalid data type byte `{}` in binary database.",
            other
        ))),
    }
}

fn validate_database_name(name: &str) -> Result<(), SqdbError> {
    validate_identifier_like_name(name, "database")
}

fn validate_table_name(name: &str) -> Result<(), SqdbError> {
    validate_identifier_like_name(name, "table")
}

fn validate_identifier_like_name(name: &str, label: &str) -> Result<(), SqdbError> {
    if name.is_empty() {
        return Err(SqdbError::RuntimeError(format!(
            "{} name cannot be empty.",
            label
        )));
    }

    let mut chars = name.chars();

    let first = chars.next().unwrap();

    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(SqdbError::RuntimeError(format!(
            "Invalid {} name `{}`. It must start with a letter or underscore.",
            label, name
        )));
    }

    for ch in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '_') {
            return Err(SqdbError::RuntimeError(format!(
                "Invalid {} name `{}`. Only letters, numbers, and underscores are allowed.",
                label, name
            )));
        }
    }

    Ok(())
}

fn read_u16(buffer: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(buffer[offset..offset + 2].try_into().unwrap())
}

fn write_u16(buffer: &mut [u8], offset: usize, value: u16) {
    buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn read_u32(buffer: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(buffer[offset..offset + 4].try_into().unwrap())
}

fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn read_u64(buffer: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap())
}

fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn write_new_journal_file(path: &Path, header: &JournalHeader) -> Result<(), SqdbError> {
    let mut file = File::create(path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not create journal file `{}`: {}",
            path.display(),
            err
        ))
    })?;

    write_journal_header_to_file(&mut file, header)?;

    file.sync_all().map_err(|err| {
        SqdbError::IoError(format!(
            "Could not sync new journal file `{}`: {}",
            path.display(),
            err
        ))
    })?;

    Ok(())
}

fn write_journal_header_to_file(
    file: &mut File,
    header: &JournalHeader,
) -> Result<(), SqdbError> {
    let db_name_bytes = header.db_name.as_bytes();

    if db_name_bytes.len() > JOURNAL_DB_NAME_MAX_BYTES {
        return Err(SqdbError::RuntimeError(format!(
            "Database name `{}` is too long for journal.",
            header.db_name
        )));
    }

    let mut page = [0u8; PAGE_SIZE];

    page[JOURNAL_MAGIC_OFFSET..JOURNAL_MAGIC_OFFSET + 8].copy_from_slice(JOURNAL_MAGIC);

    write_u32(&mut page, JOURNAL_VERSION_OFFSET, JOURNAL_VERSION);
    write_u32(&mut page, JOURNAL_PAGE_SIZE_OFFSET, PAGE_SIZE as u32);
    write_u64(&mut page, JOURNAL_ENTRY_COUNT_OFFSET, header.entry_count);
    write_u64(
        &mut page,
        JOURNAL_ORIGINAL_FILE_LEN_OFFSET,
        header.original_file_len,
    );
    write_u16(
        &mut page,
        JOURNAL_DB_NAME_LEN_OFFSET,
        db_name_bytes.len() as u16,
    );

    let name_start = JOURNAL_DB_NAME_OFFSET;
    let name_end = name_start + db_name_bytes.len();

    page[name_start..name_end].copy_from_slice(db_name_bytes);

    file.seek(SeekFrom::Start(0)).map_err(|err| {
        SqdbError::IoError(format!("Could not seek journal header: {}", err))
    })?;

    file.write_all(&page).map_err(|err| {
        SqdbError::IoError(format!("Could not write journal header: {}", err))
    })?;

    Ok(())
}

fn read_journal_header_from_path(path: &Path) -> Result<JournalHeader, SqdbError> {
    let mut file = File::open(path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not open journal file `{}`: {}",
            path.display(),
            err
        ))
    })?;

    read_journal_header_from_file(&mut file)
}

fn read_journal_header_from_file(file: &mut File) -> Result<JournalHeader, SqdbError> {
    let mut page = [0u8; PAGE_SIZE];

    file.seek(SeekFrom::Start(0)).map_err(|err| {
        SqdbError::IoError(format!("Could not seek journal header: {}", err))
    })?;

    file.read_exact(&mut page).map_err(|err| {
        SqdbError::IoError(format!("Could not read journal header: {}", err))
    })?;

    if &page[JOURNAL_MAGIC_OFFSET..JOURNAL_MAGIC_OFFSET + 8] != JOURNAL_MAGIC {
        return Err(SqdbError::IoError(
            "Invalid journal file: magic number mismatch.".to_string(),
        ));
    }

    let version = read_u32(&page, JOURNAL_VERSION_OFFSET);

    if version != JOURNAL_VERSION {
        return Err(SqdbError::IoError(format!(
            "Unsupported journal version {}. Expected {}.",
            version, JOURNAL_VERSION
        )));
    }

    let page_size = read_u32(&page, JOURNAL_PAGE_SIZE_OFFSET);

    if page_size != PAGE_SIZE as u32 {
        return Err(SqdbError::IoError(format!(
            "Invalid journal page size {}. Expected {}.",
            page_size, PAGE_SIZE
        )));
    }

    let entry_count = read_u64(&page, JOURNAL_ENTRY_COUNT_OFFSET);
    let original_file_len = read_u64(&page, JOURNAL_ORIGINAL_FILE_LEN_OFFSET);
    let db_name_len = read_u16(&page, JOURNAL_DB_NAME_LEN_OFFSET) as usize;

    if db_name_len > JOURNAL_DB_NAME_MAX_BYTES {
        return Err(SqdbError::IoError(format!(
            "Invalid journal database name length {}.",
            db_name_len
        )));
    }

    let name_start = JOURNAL_DB_NAME_OFFSET;
    let name_end = name_start + db_name_len;

    let db_name = String::from_utf8(page[name_start..name_end].to_vec()).map_err(|err| {
        SqdbError::IoError(format!("Invalid UTF-8 database name in journal: {}", err))
    })?;

    Ok(JournalHeader {
        entry_count,
        original_file_len,
        db_name,
    })
}

fn journal_contains_page(path: &Path, page_id: u64) -> Result<bool, SqdbError> {
    let mut file = File::open(path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not open journal file `{}`: {}",
            path.display(),
            err
        ))
    })?;

    let header = read_journal_header_from_file(&mut file)?;

    for index in 0..header.entry_count {
        let entry_offset = JOURNAL_HEADER_SIZE_U64 + index * JOURNAL_ENTRY_SIZE;

        file.seek(SeekFrom::Start(entry_offset)).map_err(|err| {
            SqdbError::IoError(format!(
                "Could not seek journal entry {}: {}",
                index, err
            ))
        })?;

        let mut page_id_buffer = [0u8; 8];

        file.read_exact(&mut page_id_buffer).map_err(|err| {
            SqdbError::IoError(format!(
                "Could not read journal entry {} page id: {}",
                index, err
            ))
        })?;

        let stored_page_id = u64::from_le_bytes(page_id_buffer);

        if stored_page_id == page_id {
            return Ok(true);
        }
    }

    Ok(false)
}

fn append_journal_entry(
    path: &Path,
    page_id: u64,
    page: &[u8; PAGE_SIZE],
) -> Result<(), SqdbError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|err| {
            SqdbError::IoError(format!(
                "Could not open journal file `{}` for append: {}",
                path.display(),
                err
            ))
        })?;

    let mut header = read_journal_header_from_file(&mut file)?;

    let entry_offset = JOURNAL_HEADER_SIZE_U64 + header.entry_count * JOURNAL_ENTRY_SIZE;

    file.seek(SeekFrom::Start(entry_offset)).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not seek journal append offset {}: {}",
            entry_offset, err
        ))
    })?;

    file.write_all(&page_id.to_le_bytes()).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not write journal page id {}: {}",
            page_id, err
        ))
    })?;

    file.write_all(page).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not write old page {} to journal: {}",
            page_id, err
        ))
    })?;

    // Make sure entry bytes are durable before increasing entry count.
    file.sync_all().map_err(|err| {
        SqdbError::IoError(format!(
            "Could not sync journal entry for page {}: {}",
            page_id, err
        ))
    })?;

    header.entry_count += 1;

    write_journal_header_to_file(&mut file, &header)?;

    file.sync_all().map_err(|err| {
        SqdbError::IoError(format!(
            "Could not sync updated journal header: {}",
            err
        ))
    })?;

    Ok(())
}

fn rollback_journal_file(
    database_file: &mut File,
    journal_path: &Path,
    expected_database_name: &str,
) -> Result<(), SqdbError> {
    let mut journal_file = File::open(journal_path).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not open journal `{}` for rollback: {}",
            journal_path.display(),
            err
        ))
    })?;

    let header = read_journal_header_from_file(&mut journal_file)?;

    if header.db_name != expected_database_name {
        return Err(SqdbError::IoError(format!(
            "Journal belongs to `{}`, but expected `{}`.",
            header.db_name, expected_database_name
        )));
    }

    for index in 0..header.entry_count {
        let entry_offset = JOURNAL_HEADER_SIZE_U64 + index * JOURNAL_ENTRY_SIZE;

        journal_file
            .seek(SeekFrom::Start(entry_offset))
            .map_err(|err| {
                SqdbError::IoError(format!(
                    "Could not seek journal rollback entry {}: {}",
                    index, err
                ))
            })?;

        let mut page_id_buffer = [0u8; 8];

        journal_file.read_exact(&mut page_id_buffer).map_err(|err| {
            SqdbError::IoError(format!(
                "Could not read rollback page id at entry {}: {}",
                index, err
            ))
        })?;

        let page_id = u64::from_le_bytes(page_id_buffer);

        let mut page = [0u8; PAGE_SIZE];

        journal_file.read_exact(&mut page).map_err(|err| {
            SqdbError::IoError(format!(
                "Could not read old page {} from journal: {}",
                page_id, err
            ))
        })?;

        write_page_to_file(database_file, page_id, &page)?;
    }

    database_file.set_len(header.original_file_len).map_err(|err| {
        SqdbError::IoError(format!(
            "Could not truncate database file during rollback: {}",
            err
        ))
    })?;

    database_file.sync_all().map_err(|err| {
        SqdbError::IoError(format!(
            "Could not sync database file during rollback: {}",
            err
        ))
    })?;

    Ok(())
}


#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn unique_test_dir() -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!("sqdb_binary_test_{}", timestamp))
    }

    #[test]
    fn binary_storage_creates_opens_and_drops_database() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let db_name = "binary_create_open_drop";

        storage.create_database(db_name).unwrap();

        assert!(storage.database_exists(db_name));

        let handle = storage.open_database(db_name).unwrap();

        assert_eq!(handle.database_name(), db_name);

        drop(handle);

        storage.drop_database(db_name).unwrap();

        assert!(!storage.database_exists(db_name));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn binary_storage_creates_and_lists_tables() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let db_name = "binary_create_list_tables";

        storage.create_database(db_name).unwrap();

        let mut db = storage.open_database(db_name).unwrap();

        db.create_table("numbers", TableType::Stack, DataType::Int)
            .unwrap();

        db.create_table("names", TableType::Queue, DataType::String)
            .unwrap();

        let output = db.show_tables().unwrap();

        assert!(output.contains("numbers"));
        assert!(output.contains("stack"));
        assert!(output.contains("int"));

        assert!(output.contains("names"));
        assert!(output.contains("queue"));
        assert!(output.contains("string"));

        drop(db);

        storage.drop_database(db_name).unwrap();

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn binary_storage_reads_type_and_dtype_from_disk() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let db_name = "binary_type_dtype";

        storage.create_database(db_name).unwrap();

        let mut db = storage.open_database(db_name).unwrap();

        db.create_table("events", TableType::Queue, DataType::Json)
            .unwrap();

        assert_eq!(db.get_table_type("events").unwrap(), TableType::Queue);
        assert_eq!(db.get_table_dtype("events").unwrap(), DataType::Json);

        drop(db);

        storage.drop_database(db_name).unwrap();

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn binary_storage_reuses_freed_pages() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let db_name = "binary_reuses_pages";

        storage.create_database(db_name).unwrap();

        let mut db = storage.open_database(db_name).unwrap();

        let page_1 = db.allocate_page().unwrap();
        let page_2 = db.allocate_page().unwrap();

        assert_eq!(page_1, 2);
        assert_eq!(page_2, 3);

        db.free_page(page_1).unwrap();

        let reused_page = db.allocate_page().unwrap();

        assert_eq!(reused_page, page_1);

        drop(db);

        storage.drop_database(db_name).unwrap();

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn binary_stack_reads_and_deletes_last_inserted_value() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let db_name = "binary_stack_ops";

        storage.create_database(db_name).unwrap();

        let mut db = storage.open_database(db_name).unwrap();

        db.create_table("numbers", TableType::Stack, DataType::Int)
            .unwrap();

        db.insert_raw("numbers", "10").unwrap();
        db.insert_raw("numbers", "20").unwrap();
        db.insert_raw("numbers", "30").unwrap();

        assert_eq!(db.read_value("numbers").unwrap(), "30");
        assert_eq!(db.delete_value("numbers").unwrap(), "30");
        assert_eq!(db.read_value("numbers").unwrap(), "20");

        drop(db);

        storage.drop_database(db_name).unwrap();

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn binary_queue_reads_and_deletes_first_inserted_value() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let db_name = "binary_queue_ops";

        storage.create_database(db_name).unwrap();

        let mut db = storage.open_database(db_name).unwrap();

        db.create_table("names", TableType::Queue, DataType::String)
            .unwrap();

        db.insert_raw("names", "Sourav").unwrap();
        db.insert_raw("names", "Rahul").unwrap();
        db.insert_raw("names", "Amit").unwrap();

        assert_eq!(db.read_value("names").unwrap(), "Sourav");
        assert_eq!(db.delete_value("names").unwrap(), "Sourav");
        assert_eq!(db.read_value("names").unwrap(), "Rahul");

        drop(db);

        storage.drop_database(db_name).unwrap();

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
	fn binary_storage_persists_values_after_reopen() {
		let dir = unique_test_dir();
		fs::create_dir_all(&dir).unwrap();

		let storage = BinaryPageStorage::with_base_dir(dir.clone());
		let db_name = "binary_persistence";

		storage.create_database(db_name).unwrap();

		{
			let mut db = storage.open_database(db_name).unwrap();

			db.create_table("numbers", TableType::Stack, DataType::Int)
				.unwrap();

			db.insert_raw("numbers", "100").unwrap();
			db.insert_raw("numbers", "200").unwrap();

			db.commit_transaction().unwrap();
		}

		{
			let mut db = storage.open_database(db_name).unwrap();

			assert_eq!(db.read_value("numbers").unwrap(), "200");
		}

		storage.drop_database(db_name).unwrap();

		let _ = fs::remove_dir_all(dir);
	}

    #[test]
    fn binary_storage_supports_json_values() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let db_name = "binary_json";

        storage.create_database(db_name).unwrap();

        let mut db = storage.open_database(db_name).unwrap();

        db.create_table("users", TableType::Queue, DataType::Json)
            .unwrap();

        db.insert_raw("users", r#"{"name":"Sourav"}"#).unwrap();

        assert_eq!(db.read_value("users").unwrap(), r#"{"name":"Sourav"}"#);

        let invalid = db.insert_raw("users", "{name:Sourav}");

        assert!(invalid.is_err());

        drop(db);

        storage.drop_database(db_name).unwrap();

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn binary_storage_returns_none_for_empty_table() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let db_name = "binary_empty_none";

        storage.create_database(db_name).unwrap();

        let mut db = storage.open_database(db_name).unwrap();

        db.create_table("numbers", TableType::Stack, DataType::Int)
            .unwrap();

        assert_eq!(
            db.read_value("numbers").unwrap(),
            "binary_empty_none.None"
        );

        assert_eq!(
            db.delete_value("numbers").unwrap(),
            "binary_empty_none.None"
        );

        drop(db);

        storage.drop_database(db_name).unwrap();

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn binary_storage_uses_multiple_pages_for_many_values() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let db_name = "binary_many_values";

        storage.create_database(db_name).unwrap();

        let mut db = storage.open_database(db_name).unwrap();

        db.create_table("numbers", TableType::Stack, DataType::Int)
            .unwrap();

        for value in 0..400 {
            db.insert_raw("numbers", &value.to_string()).unwrap();
        }

        assert_eq!(db.read_value("numbers").unwrap(), "399");

        let output = db.show_tables().unwrap();

        assert!(output.contains("numbers"));
        assert!(output.contains("400"));
        assert!(output.contains("2.."));

        drop(db);

        storage.drop_database(db_name).unwrap();

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn binary_drop_table_frees_data_pages_for_reuse() {
        let dir = unique_test_dir();
        fs::create_dir_all(&dir).unwrap();

        let storage = BinaryPageStorage::with_base_dir(dir.clone());
        let db_name = "binary_drop_frees_pages";

        storage.create_database(db_name).unwrap();

        let mut db = storage.open_database(db_name).unwrap();

        db.create_table("numbers", TableType::Stack, DataType::Int)
            .unwrap();

        for value in 0..400 {
            db.insert_raw("numbers", &value.to_string()).unwrap();
        }

        db.drop_table("numbers").unwrap();

        let reused_page = db.allocate_page().unwrap();

        assert!(reused_page >= 2);

        drop(db);

        storage.drop_database(db_name).unwrap();

        let _ = fs::remove_dir_all(dir);
    }
}