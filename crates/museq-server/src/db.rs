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

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    init(conn)
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
    // A job left "running" by a crash will never finish.
    conn.execute("UPDATE job SET state='failed', error='interrupted by restart' WHERE state IN ('queued','running')", [])?;
    Ok(conn)
}
