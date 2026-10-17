//! The avatar cache index (`dp_avatar`, DASHPAY §3.4; DP4-02): one row per
//! avatar URL with the outcome of fetching and decoding it, and the
//! thumbnails it left in the avatars directory. A row says nothing about
//! any profile: the engine compares `content_sha` and `dhash` with each
//! profile when it reads. Rows of different URLs with the same content
//! share their files, so rotation counts a file once and reports it for
//! unlinking only when no row names it any more.

use rusqlite::{OptionalExtension, params};

use crate::{AppDb, Result};

/// One `dp_avatar` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarRow {
    /// SHA-256 of the URL, hex.
    pub url_sha: String,
    /// SHA-256 of the bytes fetched, hex (`ok` rows).
    pub content_sha: Option<String>,
    /// The dHash of the bytes fetched, 16 hex digits (`ok` rows).
    pub dhash: Option<String>,
    /// `ok`, or the failure's code (`fetch_failed`, `too_large`,
    /// `unsupported`).
    pub status: String,
    /// The thumbnails' file stem (`ok` rows with a cache directory).
    pub file: Option<String>,
    /// When the URL was last fetched, UNIX seconds.
    pub fetched_at: u64,
    /// The thumbnails' bytes on disk.
    pub bytes: Option<u64>,
}

impl AppDb {
    /// The row of the URL whose SHA-256 is `url_sha`.
    pub fn avatar_row(&self, url_sha: &str) -> Result<Option<AvatarRow>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT url_sha, content_sha, dhash, status, file, fetched_at, bytes
                 FROM dp_avatar WHERE url_sha = ?1",
                params![url_sha],
                |r| {
                    Ok(AvatarRow {
                        url_sha: r.get(0)?,
                        content_sha: r.get(1)?,
                        dhash: r.get(2)?,
                        status: r.get(3)?,
                        file: r.get(4)?,
                        fetched_at: r.get::<_, i64>(5)?.max(0) as u64,
                        bytes: r.get::<_, Option<i64>>(6)?.map(|b| b.max(0) as u64),
                    })
                },
            )
            .optional()?)
    }

    /// Writes `row`, replacing the URL's previous one. Returns the file the
    /// previous row named if no row names it now (to be unlinked).
    pub fn put_avatar_row(&self, row: &AvatarRow) -> Result<Option<String>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT file FROM dp_avatar WHERE url_sha = ?1",
                params![row.url_sha],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        tx.execute(
            "INSERT INTO dp_avatar (url_sha, content_sha, dhash, status, file, fetched_at, bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (url_sha) DO UPDATE SET
                 content_sha = excluded.content_sha, dhash = excluded.dhash,
                 status = excluded.status, file = excluded.file,
                 fetched_at = excluded.fetched_at, bytes = excluded.bytes",
            params![
                row.url_sha,
                row.content_sha,
                row.dhash,
                row.status,
                row.file,
                i64::try_from(row.fetched_at).unwrap_or(i64::MAX),
                row.bytes.map(|b| i64::try_from(b).unwrap_or(i64::MAX)),
            ],
        )?;
        let orphan = match previous {
            Some(file) if !named(&tx, &file)? => Some(file),
            _ => None,
        };
        tx.commit()?;
        Ok(orphan)
    }

    /// Deletes the oldest rows (by `fetched_at`) while the files the rows
    /// name hold more than `max_bytes` (a shared file counted once) or there
    /// are more than `max_rows` rows. Returns the files no row names any
    /// more, to be unlinked.
    pub fn rotate_avatars(&self, max_bytes: u64, max_rows: u64) -> Result<Vec<String>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut total: u64 = tx
            .query_row(
                "SELECT COALESCE(SUM(b), 0) FROM
                     (SELECT MAX(bytes) AS b FROM dp_avatar WHERE file IS NOT NULL GROUP BY file)",
                [],
                |r| r.get::<_, i64>(0),
            )?
            .max(0) as u64;
        let mut rows: u64 = tx
            .query_row("SELECT COUNT(*) FROM dp_avatar", [], |r| r.get::<_, i64>(0))?
            .max(0) as u64;
        let mut orphans = Vec::new();
        while total > max_bytes || rows > max_rows {
            let oldest: Option<(String, Option<String>, Option<i64>)> = tx
                .query_row(
                    "SELECT url_sha, file, bytes FROM dp_avatar
                     ORDER BY fetched_at, url_sha LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((url_sha, file, bytes)) = oldest else {
                break;
            };
            tx.execute("DELETE FROM dp_avatar WHERE url_sha = ?1", params![url_sha])?;
            rows -= 1;
            if let Some(file) = file
                && !named(&tx, &file)?
            {
                total = total.saturating_sub(bytes.unwrap_or(0).max(0) as u64);
                orphans.push(file);
            }
        }
        tx.commit()?;
        Ok(orphans)
    }
}

fn named(tx: &rusqlite::Transaction<'_>, file: &str) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM dp_avatar WHERE file = ?1)",
        params![file],
        |r| r.get(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(url: &str, file: Option<&str>, at: u64, bytes: u64) -> AvatarRow {
        AvatarRow {
            url_sha: url.into(),
            content_sha: file.map(|f| format!("c{f}")),
            dhash: file.map(|_| "00".repeat(8)),
            status: if file.is_some() { "ok" } else { "fetch_failed" }.into(),
            file: file.map(str::to_owned),
            fetched_at: at,
            bytes: file.map(|_| bytes),
        }
    }

    #[test]
    fn a_row_round_trips_and_is_replaced() {
        let db = AppDb::open_in_memory().unwrap();
        assert_eq!(db.avatar_row("u").unwrap(), None);
        let r = row("u", Some("f"), 10, 100);
        assert_eq!(db.put_avatar_row(&r).unwrap(), None);
        assert_eq!(db.avatar_row("u").unwrap(), Some(r));
        // Replacing it with a failure orphans its file.
        let failed = row("u", None, 20, 0);
        assert_eq!(db.put_avatar_row(&failed).unwrap(), Some("f".into()));
        assert_eq!(db.avatar_row("u").unwrap(), Some(failed));
    }

    #[test]
    fn a_shared_file_is_kept_while_a_row_names_it() {
        let db = AppDb::open_in_memory().unwrap();
        db.put_avatar_row(&row("u1", Some("f"), 10, 100)).unwrap();
        db.put_avatar_row(&row("u2", Some("f"), 11, 100)).unwrap();
        assert_eq!(
            db.put_avatar_row(&row("u1", Some("g"), 12, 50)).unwrap(),
            None
        );
        assert_eq!(
            db.put_avatar_row(&row("u2", Some("g"), 13, 50)).unwrap(),
            Some("f".into())
        );
    }

    #[test]
    fn rotation_drops_the_oldest_and_counts_a_shared_file_once() {
        let db = AppDb::open_in_memory().unwrap();
        db.put_avatar_row(&row("a", Some("f1"), 1, 100)).unwrap();
        db.put_avatar_row(&row("b", Some("f2"), 2, 100)).unwrap();
        db.put_avatar_row(&row("c", Some("f2"), 3, 100)).unwrap();
        db.put_avatar_row(&row("d", None, 4, 0)).unwrap();
        // Two distinct files, 200 bytes: within 200.
        assert_eq!(db.rotate_avatars(200, 10).unwrap(), Vec::<String>::new());
        // Over 150: the oldest row (a) goes, and with it f1.
        assert_eq!(db.rotate_avatars(150, 10).unwrap(), vec!["f1".to_string()]);
        assert_eq!(db.avatar_row("a").unwrap(), None);
        // Over 50: b goes but c still names f2, so nothing to unlink yet;
        // then c goes and f2 with it.
        assert_eq!(db.rotate_avatars(50, 10).unwrap(), vec!["f2".to_string()]);
        assert!(db.avatar_row("b").unwrap().is_none() && db.avatar_row("c").unwrap().is_none());
        // The row cap alone.
        assert_eq!(
            db.avatar_row("d").unwrap().map(|r| r.status),
            Some("fetch_failed".into())
        );
        assert_eq!(db.rotate_avatars(0, 0).unwrap(), Vec::<String>::new());
        assert_eq!(db.avatar_row("d").unwrap(), None);
    }
}
