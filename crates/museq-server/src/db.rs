use rusqlite::Connection;
use std::path::Path;

const SCHEMA_V1: &str = "
CREATE TABLE file(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  sha256 TEXT NOT NULL,
  path TEXT NOT NULL UNIQUE,      -- relative to the library folder
  bytes INTEGER NOT NULL,
  mtime INTEGER NOT NULL,
  codec TEXT,
  duration_ms INTEGER
);
CREATE INDEX file_sha256 ON file(sha256);
CREATE TABLE track(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL,
  artist TEXT,
  album TEXT,
  duration_ms INTEGER,
  preferred_file_id INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX track_file ON track(preferred_file_id);
CREATE TABLE playlist(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  extend INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE playlist_item(
  playlist_id INTEGER NOT NULL REFERENCES playlist(id) ON DELETE CASCADE,
  position INTEGER NOT NULL,
  track_id INTEGER NOT NULL REFERENCES track(id) ON DELETE CASCADE,
  PRIMARY KEY(playlist_id, position)
);
CREATE TABLE job(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,
  state TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  payload TEXT,
  result TEXT,
  error TEXT
);
";

pub fn search_text(title: &str, artist: Option<&str>, album: Option<&str>) -> String {
    crate::text::fold(&format!(
        "{title}\n{}\n{}",
        artist.unwrap_or(""),
        album.unwrap_or("")
    ))
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    init(conn)
}

/// Lock the database; a panic elsewhere must not poison every later request.
pub fn lock(m: &std::sync::Mutex<Connection>) -> std::sync::MutexGuard<'_, Connection> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn open_in_memory() -> rusqlite::Result<Connection> {
    init(Connection::open_in_memory()?)
}

fn init(conn: Connection) -> rusqlite::Result<Connection> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(SCHEMA_V1)?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    if version < 2 {
        // Accent- and case-insensitive search text (SQLite LIKE only folds ASCII case).
        conn.execute_batch("ALTER TABLE track ADD COLUMN search_text TEXT NOT NULL DEFAULT '';")?;
        let rows: Vec<(i64, String, Option<String>, Option<String>)> = conn
            .prepare("SELECT id, title, artist, album FROM track")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, title, artist, album) in rows {
            conn.execute(
                "UPDATE track SET search_text=?1 WHERE id=?2",
                rusqlite::params![search_text(&title, artist.as_deref(), album.as_deref()), id],
            )?;
        }
        conn.pragma_update(None, "user_version", 2)?;
    }
    if version < 3 {
        conn.execute_batch(
            "CREATE TABLE skin(id TEXT PRIMARY KEY, name TEXT NOT NULL, bytes INTEGER NOT NULL);",
        )?;
        conn.pragma_update(None, "user_version", 3)?;
    }
    if version < 4 {
        conn.execute_batch(
            "CREATE TABLE station(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               name TEXT NOT NULL,
               category TEXT NOT NULL,
               url TEXT NOT NULL UNIQUE,
               source TEXT NOT NULL,
               search_text TEXT NOT NULL,
               alive INTEGER,        -- NULL = never checked
               checked_at INTEGER,
               error TEXT
             );",
        )?;
        conn.pragma_update(None, "user_version", 4)?;
    }
    // A job left "running" by a crash will never finish.
    conn.execute("UPDATE job SET state='failed', error='interrupted by restart' WHERE state IN ('queued','running')", [])?;
    Ok(conn)
}
