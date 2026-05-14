use std::convert::TryInto;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

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

        write_page_to_file(
            &mut file,
            TABLE_DIRECTORY_PAGE_ID,
            &table_directory_page,
        )?;

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

        if path.exists() {
            fs::remove_file(&path).map_err(|err| {
                SqdbError::IoError(format!(
                    "Could not drop binary database `{}`: {}",
                    path.display(),
                    err
                ))
            })?;
        }

        Ok(())
    }

    fn database_path(&self, database_name: &str) -> PathBuf {
        self.base_dir.join(format!("{}.sqdb", database_name))
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

        if table.first_page != 0 || table.last_page != 0 || table.item_count != 0 {
            return Err(SqdbError::RuntimeError(
                "This table contains data pages. Data-page freeing will be added in the next binary step."
                    .to_string(),
            ));
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

    fn read_header(&mut self) -> Result<BinaryHeader, SqdbError> {
        read_header_from_file(&mut self.file)
    }

    fn write_header(&mut self, header: BinaryHeader) -> Result<(), SqdbError> {
        write_header_to_file(&mut self.file, header)
    }

    fn read_page(&mut self, page_id: u64) -> Result<[u8; PAGE_SIZE], SqdbError> {
        read_page_from_file(&mut self.file, page_id)
    }

    fn write_page(&mut self, page_id: u64, page: &[u8; PAGE_SIZE]) -> Result<(), SqdbError> {
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

    file.read_exact(&mut page).map_err(|err| {
        SqdbError::IoError(format!("Could not read page {}: {}", page_id, err))
    })?;

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

    file.write_all(page).map_err(|err| {
        SqdbError::IoError(format!("Could not write page {}: {}", page_id, err))
    })?;

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
        (table.slot_index + 1) as u32,
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
}