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

/// Run one migration step atomically: either all of its statements and the new `user_version`
/// are stored, or none are, so a failure halfway never leaves a database that cannot start again.
fn step(conn: &Connection, to: i64, sql: &str) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "BEGIN IMMEDIATE;\n{sql}\nPRAGMA user_version = {to};\nCOMMIT;"
    ))
    .inspect_err(|_| {
        let _ = conn.execute_batch("ROLLBACK");
    })
}

fn init(conn: Connection) -> rusqlite::Result<Connection> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 1 {
        step(&conn, 1, SCHEMA_V1)?;
    }
    if version < 2 {
        // Accent- and case-insensitive search text (SQLite LIKE only folds ASCII case).
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let migrated = (|| -> rusqlite::Result<()> {
            conn.execute_batch(
                "ALTER TABLE track ADD COLUMN search_text TEXT NOT NULL DEFAULT '';",
            )?;
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
            conn.execute_batch("PRAGMA user_version = 2;")
        })();
        match migrated {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
    }
    if version < 3 {
        step(
            &conn,
            3,
            "CREATE TABLE skin(id TEXT PRIMARY KEY, name TEXT NOT NULL, bytes INTEGER NOT NULL);",
        )?;
    }
    if version < 4 {
        step(
            &conn,
            4,
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
    }
    if version < 5 {
        // One track can own several files (duplicates merged by the owner), and files now carry
        // the facts needed to rank their quality. Every file is re-probed on the next scan.
        step(
            &conn,
            5,
            "ALTER TABLE file ADD COLUMN track_id INTEGER REFERENCES track(id) ON DELETE SET NULL;
             ALTER TABLE file ADD COLUMN sample_rate INTEGER;
             ALTER TABLE file ADD COLUMN channels INTEGER;
             ALTER TABLE file ADD COLUMN bitrate_kbps INTEGER;
             ALTER TABLE file ADD COLUMN lossless INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE track ADD COLUMN genre TEXT;
             UPDATE file SET track_id = (SELECT id FROM track WHERE preferred_file_id = file.id);
             UPDATE file SET mtime = 0;
             CREATE INDEX file_track ON file(track_id);
             CREATE TABLE fingerprint(
               file_id INTEGER PRIMARY KEY REFERENCES file(id) ON DELETE CASCADE,
               sha256 TEXT NOT NULL,
               status TEXT NOT NULL,  -- ok | unsupported | error
               error TEXT,
               duration_ms INTEGER,
               data BLOB
             );",
        )?;
    }
    if version < 6 {
        // Suggested metadata. A track the owner corrected keeps its corrected tags on rescans.
        step(&conn, 6, "ALTER TABLE track ADD COLUMN corrected INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE track ADD COLUMN mbid TEXT;
             CREATE TABLE candidate(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               track_id INTEGER NOT NULL REFERENCES track(id) ON DELETE CASCADE,
               source TEXT NOT NULL,          -- musicbrainz | acoustid
               score REAL,
               title TEXT, artist TEXT, album TEXT, year TEXT, mbid TEXT,
               status TEXT NOT NULL DEFAULT 'pending',   -- pending | accepted | rejected | superseded
               created_at INTEGER NOT NULL,
               UNIQUE(track_id, source, mbid)
             );")?;
    }
    // A job left "running" by a crash will never finish.
    conn.execute("UPDATE job SET state='failed', error='interrupted by restart' WHERE state IN ('queued','running')", [])?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A migration that fails halfway must leave the database exactly as it was.
    #[test]
    fn a_failed_migration_step_changes_nothing() {
        let dir = std::env::temp_dir().join(format!("museq-mig-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("half.db");
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.execute_batch(
                "ALTER TABLE track ADD COLUMN search_text TEXT NOT NULL DEFAULT '';
                 CREATE TABLE skin(id TEXT PRIMARY KEY, name TEXT NOT NULL, bytes INTEGER NOT NULL);
                 CREATE TABLE station(id INTEGER PRIMARY KEY, name TEXT, category TEXT, url TEXT, source TEXT, search_text TEXT, alive INTEGER, checked_at INTEGER, error TEXT);
                 ALTER TABLE file ADD COLUMN sample_rate INTEGER;", // v5 will trip over this one
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 4).unwrap();
        }
        assert!(open(&path).is_err(), "the broken step must fail loudly");
        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 4, "the version must not move");
        let has_track_id = conn.prepare("SELECT track_id FROM file LIMIT 1").is_ok();
        assert!(
            !has_track_id,
            "statements before the failing one must be rolled back too"
        );
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A database created by the first release, with a track in a playlist, must come through
    /// every later migration with its data and get the new structure.
    #[test]
    fn a_version_1_database_is_migrated_with_its_data() {
        let dir = std::env::temp_dir().join(format!("museq-mig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.db");
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute_batch(
                "INSERT INTO file(id, sha256, path, bytes, mtime, codec, duration_ms) VALUES(7,'abc','a/b.flac',100,5,'flac',1000);
                 INSERT INTO track(id, title, artist, album, duration_ms, preferred_file_id) VALUES(3,'Song','Art','Alb',1000,7);
                 INSERT INTO playlist(id, name) VALUES(1,'mine');
                 INSERT INTO playlist_item(playlist_id, position, track_id) VALUES(1,0,3);",
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 6);
        // data carried across, the new columns filled in, the new tables there
        let (track_id, mtime): (Option<i64>, i64) = conn
            .query_row("SELECT track_id, mtime FROM file WHERE id=7", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(track_id, Some(3), "the file must point at its track");
        assert_eq!(mtime, 0, "files are re-probed on the next scan");
        let items: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM playlist_item WHERE track_id=3",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(items, 1);
        let search: String = conn
            .query_row("SELECT search_text FROM track WHERE id=3", [], |r| r.get(0))
            .unwrap();
        assert!(
            search.contains("song"),
            "search text is backfilled: {search}"
        );
        for table in ["skin", "station", "fingerprint", "candidate"] {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
        }
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
