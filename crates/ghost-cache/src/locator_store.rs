//! SQLite-backed locator store. Stable cross-run cache of (app, window, role, name) -> rect.

use crate::error::CacheError;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA_V1: &str = r"
CREATE TABLE IF NOT EXISTS locators (
    id INTEGER PRIMARY KEY,
    app_id TEXT NOT NULL,
    window_class TEXT NOT NULL,
    title_pattern TEXT NOT NULL,
    role TEXT NOT NULL,
    name TEXT NOT NULL,
    rect_left INTEGER NOT NULL,
    rect_top INTEGER NOT NULL,
    rect_right INTEGER NOT NULL,
    rect_bottom INTEGER NOT NULL,
    ax_checksum BLOB NOT NULL,
    last_verified_ms INTEGER NOT NULL,
    hit_count INTEGER NOT NULL DEFAULT 0,
    UNIQUE(app_id, window_class, title_pattern, role, name)
);
CREATE INDEX IF NOT EXISTS idx_locators_lookup ON locators(app_id, window_class);
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatorKey {
    pub app_id: String,
    pub window_class: String,
    pub title_pattern: String,
    pub role: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct LocatorRow {
    pub id: i64,
    pub key: LocatorKey,
    pub rect: (i32, i32, i32, i32),
    pub ax_checksum: [u8; 16],
    pub last_verified_ms: i64,
    pub hit_count: i64,
}

pub struct LocatorStore {
    conn: Mutex<Connection>,
}

impl LocatorStore {
    pub fn open(dir: &Path) -> Result<Self, CacheError> {
        std::fs::create_dir_all(dir).map_err(|e| CacheError::Io(e.to_string()))?;
        let db = dir.join("locators.db");
        let conn = Connection::open(&db).map_err(|e| CacheError::Sqlite(e.to_string()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| CacheError::Sqlite(e.to_string()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| CacheError::Sqlite(e.to_string()))?;
        conn.pragma_update(None, "mmap_size", 268_435_456i64)
            .map_err(|e| CacheError::Sqlite(e.to_string()))?;
        let user_version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|e| CacheError::Sqlite(e.to_string()))?;
        if user_version == 0 {
            conn.execute_batch(SCHEMA_V1)
                .map_err(|e| CacheError::Sqlite(e.to_string()))?;
            conn.pragma_update(None, "user_version", 1i64)
                .map_err(|e| CacheError::Sqlite(e.to_string()))?;
        }
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn schema_version(&self) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap_or(0)
    }

    pub fn upsert(
        &self,
        key: &LocatorKey,
        rect: (i32, i32, i32, i32),
        ax_checksum: [u8; 16],
    ) -> Result<i64, CacheError> {
        let now = now_ms();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO locators
                (app_id, window_class, title_pattern, role, name,
                 rect_left, rect_top, rect_right, rect_bottom,
                 ax_checksum, last_verified_ms, hit_count)
             VALUES (?1,?2,?3,?4,?5, ?6,?7,?8,?9, ?10, ?11, 0)
             ON CONFLICT(app_id, window_class, title_pattern, role, name)
             DO UPDATE SET
                rect_left=excluded.rect_left,
                rect_top=excluded.rect_top,
                rect_right=excluded.rect_right,
                rect_bottom=excluded.rect_bottom,
                ax_checksum=excluded.ax_checksum,
                last_verified_ms=excluded.last_verified_ms",
            params![
                key.app_id,
                key.window_class,
                key.title_pattern,
                key.role,
                key.name,
                rect.0,
                rect.1,
                rect.2,
                rect.3,
                ax_checksum.to_vec(),
                now,
            ],
        )
        .map_err(|e| CacheError::Sqlite(e.to_string()))?;
        let id = conn
            .query_row(
                "SELECT id FROM locators WHERE app_id=?1 AND window_class=?2
                 AND title_pattern=?3 AND role=?4 AND name=?5",
                params![key.app_id, key.window_class, key.title_pattern, key.role, key.name],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| CacheError::Sqlite(e.to_string()))?;
        Ok(id)
    }

    /// Look up a row. If `live_checksum` is supplied and mismatches, evict and return `None`.
    /// On hit, bump `hit_count` and `last_verified_ms`.
    pub fn lookup(
        &self,
        key: &LocatorKey,
        live_checksum: Option<[u8; 16]>,
    ) -> Result<Option<LocatorRow>, CacheError> {
        let conn = self.conn.lock().unwrap();
        let row: Option<LocatorRow> = conn
            .query_row(
                "SELECT id, app_id, window_class, title_pattern, role, name,
                        rect_left, rect_top, rect_right, rect_bottom,
                        ax_checksum, last_verified_ms, hit_count
                 FROM locators
                 WHERE app_id=?1 AND window_class=?2
                   AND title_pattern=?3 AND role=?4 AND name=?5",
                params![key.app_id, key.window_class, key.title_pattern, key.role, key.name],
                row_to_locator,
            )
            .optional()
            .map_err(|e| CacheError::Sqlite(e.to_string()))?;
        let Some(row) = row else { return Ok(None) };
        if let Some(live) = live_checksum {
            if live != row.ax_checksum {
                conn.execute("DELETE FROM locators WHERE id=?1", params![row.id])
                    .map_err(|e| CacheError::Sqlite(e.to_string()))?;
                return Ok(None);
            }
        }
        conn.execute(
            "UPDATE locators SET hit_count=hit_count+1, last_verified_ms=?1 WHERE id=?2",
            params![now_ms(), row.id],
        )
        .map_err(|e| CacheError::Sqlite(e.to_string()))?;
        Ok(Some(row))
    }

    pub fn evict(&self, id: i64) -> Result<(), CacheError> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM locators WHERE id=?1", params![id])
            .map_err(|e| CacheError::Sqlite(e.to_string()))?;
        Ok(())
    }

    pub fn row_count(&self) -> Result<i64, CacheError> {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM locators", [], |r| r.get(0))
            .map_err(|e| CacheError::Sqlite(e.to_string()))
    }
}

fn row_to_locator(r: &rusqlite::Row) -> rusqlite::Result<LocatorRow> {
    let cs_vec: Vec<u8> = r.get(10)?;
    let mut checksum = [0u8; 16];
    if cs_vec.len() >= 16 {
        checksum.copy_from_slice(&cs_vec[..16]);
    }
    Ok(LocatorRow {
        id: r.get(0)?,
        key: LocatorKey {
            app_id: r.get(1)?,
            window_class: r.get(2)?,
            title_pattern: r.get(3)?,
            role: r.get(4)?,
            name: r.get(5)?,
        },
        rect: (r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?),
        ax_checksum: checksum,
        last_verified_ms: r.get(11)?,
        hit_count: r.get(12)?,
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> LocatorKey {
        LocatorKey {
            app_id: "msedge".into(),
            window_class: "Chrome_WidgetWin_1".into(),
            title_pattern: "kimi".into(),
            role: "edit".into(),
            name: "Ask Anything...".into(),
        }
    }

    #[test]
    fn store_opens_creates_schema_and_reports_v1() {
        let tmp = tempfile::tempdir().unwrap();
        let store = LocatorStore::open(tmp.path()).unwrap();
        assert_eq!(store.schema_version(), 1);
    }

    #[test]
    fn upsert_then_lookup_returns_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let store = LocatorStore::open(tmp.path()).unwrap();
        let cs = [1u8; 16];
        let id = store.upsert(&key(), (10, 20, 30, 40), cs).unwrap();
        let row = store.lookup(&key(), Some(cs)).unwrap().unwrap();
        assert_eq!(row.id, id);
        assert_eq!(row.rect, (10, 20, 30, 40));
    }

    #[test]
    fn lookup_with_stale_checksum_returns_miss_and_evicts() {
        let tmp = tempfile::tempdir().unwrap();
        let store = LocatorStore::open(tmp.path()).unwrap();
        store.upsert(&key(), (10, 20, 30, 40), [1u8; 16]).unwrap();
        assert!(store.lookup(&key(), Some([2u8; 16])).unwrap().is_none());
        assert_eq!(store.row_count().unwrap(), 0);
    }

    #[test]
    fn hit_count_increments_on_verified_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let store = LocatorStore::open(tmp.path()).unwrap();
        let cs = [3u8; 16];
        store.upsert(&key(), (0, 0, 1, 1), cs).unwrap();
        let r1 = store.lookup(&key(), Some(cs)).unwrap().unwrap();
        let r2 = store.lookup(&key(), Some(cs)).unwrap().unwrap();
        assert!(r2.hit_count > r1.hit_count);
    }

    #[test]
    fn store_survives_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let cs = [4u8; 16];
        {
            let s = LocatorStore::open(tmp.path()).unwrap();
            s.upsert(&key(), (5, 6, 7, 8), cs).unwrap();
        }
        let s = LocatorStore::open(tmp.path()).unwrap();
        let row = s.lookup(&key(), Some(cs)).unwrap().unwrap();
        assert_eq!(row.rect, (5, 6, 7, 8));
    }
}
