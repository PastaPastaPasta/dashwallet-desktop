use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::error::StoreUpgradeError;

/// A `sqlite_master` row: type, name, and its statement with comments and
/// layout removed (`None` for automatic indexes).
pub(crate) type SchemaObject = (String, String, Option<String>);

/// `sql` with comments removed, runs of whitespace collapsed, and no
/// whitespace next to `(`, `)` or `,`. Quoted text is kept byte for byte.
pub(crate) fn normalize_sql(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut space = false;
    let tight = |c: char| matches!(c, '(' | ')' | ',');
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '-' && next == Some('-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            space = true;
            continue;
        }
        if c == '/' && next == Some('*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
            space = true;
            continue;
        }
        if c.is_whitespace() {
            space = true;
            i += 1;
            continue;
        }
        if space && out.chars().last().is_some_and(|last| !tight(last)) && !tight(c) {
            out.push(' ');
        }
        space = false;
        if matches!(c, '\'' | '"' | '`' | '[') {
            let close = if c == '[' { ']' } else { c };
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == close {
                    if close != ']' && chars.get(i + 1) == Some(&close) {
                        out.push(close);
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

pub(crate) fn has_table(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |r| r.get(0),
    )
}

pub(crate) fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        [table, column],
        |r| r.get(0),
    )
}

/// Every schema object on `table` (the table, its indexes and triggers),
/// sorted, with normalized statements.
pub(crate) fn objects_on(conn: &Connection, table: &str) -> rusqlite::Result<Vec<SchemaObject>> {
    let mut stmt = conn.prepare(
        "SELECT type, name, sql FROM sqlite_master WHERE tbl_name = ?1 ORDER BY type, name",
    )?;
    let rows = stmt.query_map([table], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?.map(|s| normalize_sql(&s)),
        ))
    })?;
    rows.collect()
}

pub(crate) fn table_sql(conn: &Connection, table: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map(|s| s.map(|s| normalize_sql(&s)))
}

pub(crate) fn count_rows(conn: &Connection, table: &str) -> rusqlite::Result<u64> {
    let n: i64 = conn.query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |r| {
        r.get(0)
    })?;
    Ok(n.max(0) as u64)
}

/// A `file:` URI for `path` with `query`, percent-encoding what SQLite's URI
/// parser would read as syntax.
fn file_uri(path: &Path, query: &str) -> String {
    let raw = path.to_string_lossy();
    let mut s = String::from("file:");
    if cfg!(windows) {
        s.push('/');
    }
    for b in raw.bytes() {
        let c = b as char;
        if cfg!(windows) && c == '\\' {
            s.push('/');
        } else if c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | '~' | ':') {
            s.push(c);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s.push('?');
    s.push_str(query);
    s
}

/// A read-only connection that ignores `-wal`/`-shm`, takes no lock and
/// creates no file (`immutable=1`).
pub(crate) fn open_immutable(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        file_uri(path, "immutable=1"),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// A normal read-write connection on an existing file.
pub(crate) fn open_existing(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

fn put_value(h: &mut Sha256, v: rusqlite::types::ValueRef<'_>) {
    use rusqlite::types::ValueRef;
    let (tag, bytes): (u8, Vec<u8>) = match v {
        ValueRef::Null => (0, Vec::new()),
        ValueRef::Integer(i) => (1, i.to_le_bytes().to_vec()),
        ValueRef::Real(f) => (2, f.to_le_bytes().to_vec()),
        ValueRef::Text(t) => (3, t.to_vec()),
        ValueRef::Blob(b) => (4, b.to_vec()),
    };
    h.update([tag]);
    h.update((bytes.len() as u64).to_le_bytes());
    h.update(&bytes);
}

/// Each table's rows hashed in a row-order-independent way: name → sha256.
/// `skip` names `(table, column)` pairs left out, and tables named by
/// `(table, "*")` are left out whole.
pub(crate) fn table_digests(
    conn: &Connection,
    skip: &[(&str, &str)],
) -> rusqlite::Result<BTreeMap<String, String>> {
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = BTreeMap::new();
    for table in tables {
        if skip.iter().any(|(t, c)| *t == table && *c == "*") {
            continue;
        }
        let columns: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")?
            .query_map([&table], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?
            .into_iter()
            .filter(|c| !skip.iter().any(|(t, col)| *t == table && col == c))
            .collect();
        let list = columns
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let mut rows: Vec<[u8; 32]> = Vec::new();
        let mut stmt = conn.prepare(&format!("SELECT {list} FROM \"{table}\""))?;
        let mut q = stmt.query([])?;
        while let Some(row) = q.next()? {
            let mut h = Sha256::new();
            for i in 0..columns.len() {
                put_value(&mut h, row.get_ref(i)?);
            }
            rows.push(h.finalize().into());
        }
        rows.sort_unstable();
        let mut h = Sha256::new();
        h.update(columns.join(",").as_bytes());
        for r in &rows {
            h.update(r);
        }
        out.insert(table, hex(&h.finalize()));
    }
    Ok(out)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex(&h.finalize()))
}

pub(crate) fn integrity_ok(conn: &Connection) -> rusqlite::Result<bool> {
    let report: Vec<String> = conn
        .prepare("PRAGMA integrity_check")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(report == ["ok"])
}

pub(crate) fn map_sqlite(context: &'static str) -> impl Fn(rusqlite::Error) -> StoreUpgradeError {
    StoreUpgradeError::sqlite(context, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_drops_comments_and_layout_but_keeps_quotes() {
        let a = "CREATE TABLE t (\n    a INTEGER, -- note\n    b TEXT DEFAULT 'x  -- y'\n)";
        let b = "CREATE TABLE t(a INTEGER,b TEXT DEFAULT 'x  -- y')";
        assert_eq!(normalize_sql(a), normalize_sql(b));
        assert_eq!(
            normalize_sql(a),
            "CREATE TABLE t(a INTEGER,b TEXT DEFAULT 'x  -- y')"
        );
        assert_ne!(normalize_sql("a INTEGER"), normalize_sql("a TEXT"));
        assert_eq!(normalize_sql("x /* c */ y"), "x y");
    }

    #[test]
    fn file_uri_escapes_uri_syntax() {
        let uri = file_uri(Path::new("/a b/c?d#e%f"), "immutable=1");
        assert_eq!(uri, "file:/a%20b/c%3Fd%23e%25f?immutable=1");
    }
}
