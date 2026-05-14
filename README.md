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

