//! Storage stack foundation for the P-MEM milestone: rusqlite with bundled
//! SQLite (FTS5 enabled) plus the sqlite-vec vector extension.
//!
//! This crate is a CI spike: it proves the native C build (libsqlite3-sys +
//! sqlite-vec via `cc`) compiles and runs on every gate target before any
//! P-MEM schema work starts. It carries smoke tests only — no schema design,
//! no memory logic.

use rusqlite::Connection;

/// Registers sqlite-vec as an auto-extension so every subsequently opened
/// connection has the `vec0` virtual table and `vec_*` functions available.
///
/// Call once at process startup, before opening any connection that needs
/// vector search.
pub fn register_sqlite_vec() {
    type EntryPoint = unsafe extern "C" fn(
        *mut rusqlite::ffi::sqlite3,
        *mut *mut std::ffi::c_char,
        *const rusqlite::ffi::sqlite3_api_routines,
    ) -> std::ffi::c_int;
    // SAFETY: `sqlite3_vec_init` is the extension's C entrypoint with exactly
    // the signature SQLite expects for an auto-extension; the transmute only
    // reinterprets the function pointer to the type the rusqlite FFI declares.
    // Registration pattern per sqlite-vec's official rusqlite example.
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<*const (), EntryPoint>(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }
}

/// Opens an in-memory database. Smoke-test surface only; real connection
/// management lands with the P-MEM schema work.
pub fn open_in_memory() -> rusqlite::Result<Connection> {
    Connection::open_in_memory()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn sqlite_bundled_basic_roundtrip() {
        let db = open_in_memory().expect("open in-memory db");
        db.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
            .expect("create table");
        db.execute("INSERT INTO t(name) VALUES (?1)", params!["nano"])
            .expect("insert row");
        let name: String = db
            .query_row("SELECT name FROM t WHERE id = 1", [], |row| row.get(0))
            .expect("select row");
        assert_eq!(name, "nano");
    }

    #[test]
    fn sqlite_vec_knn_query() {
        register_sqlite_vec();
        let db = open_in_memory().expect("open in-memory db");

        let version: String = db
            .query_row("SELECT vec_version()", [], |row| row.get(0))
            .expect("vec_version()");
        assert!(
            version.starts_with("v"),
            "unexpected vec_version: {version}"
        );

        db.execute_batch(
            "CREATE VIRTUAL TABLE vec_items USING vec0(embedding float[3]);
             INSERT INTO vec_items(rowid, embedding) VALUES
               (1, '[0.1, 0.2, 0.3]'),
               (2, '[0.9, 0.9, 0.9]'),
               (3, '[0.2, 0.2, 0.2]');",
        )
        .expect("create vec0 table and insert rows");

        let mut stmt = db
            .prepare(
                "SELECT rowid FROM vec_items
                 WHERE embedding MATCH '[0.15, 0.2, 0.25]' AND k = 2
                 ORDER BY distance",
            )
            .expect("prepare KNN query");
        let hits: Vec<i64> = stmt
            .query_map([], |row| row.get(0))
            .expect("run KNN query")
            .collect::<rusqlite::Result<Vec<i64>>>()
            .expect("collect KNN results");
        assert_eq!(hits, vec![3, 1], "nearest rows must be 3 then 1");
    }

    #[test]
    fn fts5_match_query() {
        let db = open_in_memory().expect("open in-memory db");
        db.execute_batch(
            "CREATE VIRTUAL TABLE docs USING fts5(body);
             INSERT INTO docs(rowid, body) VALUES
               (1, 'the quick brown fox'),
               (2, 'lazy dogs sleep all day');",
        )
        .expect("create FTS5 table and insert rows");
        let rowid: i64 = db
            .query_row(
                "SELECT rowid FROM docs WHERE docs MATCH 'quick'",
                [],
                |row| row.get(0),
            )
            .expect("FTS5 MATCH query");
        assert_eq!(rowid, 1);
    }
}
