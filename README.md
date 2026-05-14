# SQ DB
---

## Project Structure
```
sqdb/
├── Cargo.toml
└── src/
    ├── main.rs
    ├── error.rs
    ├── language/
    │   ├── mod.rs
    │   ├── ast.rs
    │   └── parser.rs
    ├── engine/
    │   └── mod.rs
    └── storage/
        └── mod.rs
```

---

## `.sqdb` Layout
```
database.sqdb
┌────────────────────────────┐
│ Page 0: Database Header    │
├────────────────────────────┤
│ Page 1: Table Directory    │
├────────────────────────────┤
│ Page 2: Data Page          │
├────────────────────────────┤
│ Page 3: Data Page          │
├────────────────────────────┤
│ Page N: Free/Data Page     │
└────────────────────────────┘

```

Each Page is 
```
4096 bytes
```

---

## Storage Rules
```
create table  -> update table directory page
drop table    -> free table pages and reuse them later
insert        -> write value into disk page
read          -> read only stack top or queue front page
delete        -> update page metadata and free page if empty
show tables   -> read table directory only
```

### Free Page Reuse
When Data is deleted
```
used page
   ↓
marked as free
   ↓
added to global free page list
   ↓
future inserts from any table can reuse it
```

---

## Binary Value Page Design

A table has
```
first_page
last_page
item_count
```

Each data page stores multiple variable-length records
```
Data Page
┌──────────────────────────────┐
│ Page Header                  │
├──────────────────────────────┤
│ Record 1                     │
│ Record 2                     │
│ Record 3                     │
│ ...                          │
└──────────────────────────────┘
```

Each record is stored as:
```
[payload_length][payload_bytes][payload_length]
```

Why store length twice?

Because:
```
Queue read/delete needs first record
Stack read/delete needs last record
```
The ending length lets us find the last record quickly.

---
