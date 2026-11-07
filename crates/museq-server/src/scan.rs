use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::TrackType;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTag};

/// Test hook: with `MUSEQ_TEST_HOOKS=1`, a file whose name contains a marker makes that stage panic
/// on purpose, so the tests can prove a crash never wedges the job queue. Never set in production.
pub fn test_hook(path: &str, marker: &str) {
    if std::env::var("MUSEQ_TEST_HOOKS").is_ok_and(|v| v == "1") && path.contains(marker) {
        panic!("test hook: {marker}");
    }
}

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "wav", "aif", "aiff",
];

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct ScanStats {
    pub added: u32,
    pub updated: u32,
    pub unchanged: u32,
    pub removed: u32,
    pub errors: u32,
}

#[derive(Debug, Default)]
pub struct Probed {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub duration_ms: Option<i64>,
    pub sample_rate: Option<i64>,
    pub channels: Option<i64>,
    pub alac: bool,
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        // file_type() does not follow symlinks, so a link can't send the scan outside the library
        match e.file_type() {
            // fetch's half-finished downloads live in ".work-*" folders: never catalogue them
            Ok(t) if t.is_dir() && e.file_name().to_string_lossy().starts_with(".work-") => {}
            Ok(t) if t.is_dir() => walk(&p, out),
            Ok(t) if t.is_file() && is_audio(&p) => out.push(p),
            _ => {}
        }
    }
}

fn sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Read tags and duration. Never fails the scan: unreadable files just get no metadata.
pub fn probe(path: &Path) -> Probed {
    let mut out = Probed::default();
    let Ok(file) = File::open(path) else {
        return out;
    };
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let Ok(mut format) = symphonia::default::get_probe().probe(
        &hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    ) else {
        return out;
    };
    if let Some(track) = format.default_track(TrackType::Audio) {
        if let Some(a) = track.codec_params.as_ref().and_then(|p| p.audio()) {
            out.sample_rate = a.sample_rate.map(i64::from);
            out.channels = a.channels.as_ref().map(|c| c.count() as i64);
            out.alac = a.codec == symphonia::core::codecs::audio::well_known::CODEC_ID_ALAC;
        }
        if let (Some(tb), Some(d)) = (track.time_base, track.duration) {
            out.duration_ms = tb.calc_duration(d).map(|t| t.as_millis() as i64);
        } else if let (Some(frames), Some(rate)) = (
            track.num_frames,
            track
                .codec_params
                .as_ref()
                .and_then(|p| p.audio())
                .and_then(|a| a.sample_rate)
                .filter(|r| *r > 0),
        ) {
            out.duration_ms = Some((frames * 1000 / rate as u64) as i64);
        }
    }
    if let Some(rev) = format.metadata().current() {
        for tag in &rev.media.tags {
            match &tag.std {
                Some(StandardTag::TrackTitle(v)) if out.title.is_none() => {
                    out.title = Some(v.to_string())
                }
                Some(StandardTag::Artist(v)) if out.artist.is_none() => {
                    out.artist = Some(v.to_string())
                }
                Some(StandardTag::Album(v)) if out.album.is_none() => {
                    out.album = Some(v.to_string())
                }
                Some(StandardTag::Genre(v)) if out.genre.is_none() => {
                    out.genre = Some(v.to_string())
                }
                _ => {}
            }
        }
    }
    out
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Walk the library, add new files, refresh changed ones, drop vanished ones.
/// Hashing and probing happen without holding the database lock.
pub fn scan(db: &Mutex<Connection>, root: &Path) -> ScanStats {
    let mut stats = ScanStats::default();
    let mut files = Vec::new();
    walk(root, &mut files);
    let mut seen: HashSet<String> = HashSet::new();

    for path in files {
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        test_hook(&rel, "__museq_panic_scan__"); // outside every guard: the job itself must survive it
        seen.insert(rel.clone());
        let Ok(meta) = std::fs::metadata(&path) else {
            stats.errors += 1;
            continue;
        };
        let (bytes, mtime) = (meta.len() as i64, mtime_secs(&meta));

        let existing: Option<(i64, i64, i64)> = crate::db::lock(db)
            .query_row(
                "SELECT id, bytes, mtime FROM file WHERE path=?1",
                [&rel],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .unwrap_or(None);
        if let Some((_, b, m)) = existing
            && b == bytes
            && m == mtime
        {
            stats.unchanged += 1;
            continue;
        }

        let Ok(sha) = sha256_hex(&path) else {
            stats.errors += 1;
            continue;
        };
        let p = std::panic::catch_unwind(|| {
            test_hook(&path.to_string_lossy(), "__museq_panic_probe__");
            probe(&path)
        })
        .unwrap_or_default();
        let codec = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        let title = p.title.clone().unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| rel.clone())
        });
        let search = crate::db::search_text(&title, p.artist.as_deref(), p.album.as_deref());
        let lossless = p.alac || matches!(codec.as_deref(), Some("flac" | "wav" | "aif" | "aiff"));
        let bitrate_kbps = p
            .duration_ms
            .filter(|d| *d > 0)
            .map(|d| bytes.saturating_mul(8) / d);

        let mut conn = crate::db::lock(db);
        // A moved or renamed file has the same content hash as a row whose path is gone:
        // keep that row (and its playlist entries) instead of removing and re-adding it.
        let moved_from: Option<i64> = if existing.is_none() {
            conn.prepare("SELECT id, path FROM file WHERE sha256=?1 ORDER BY id")
                .and_then(|mut st| {
                    st.query_map([&sha], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap_or_default()
                .into_iter()
                .find(|(_, old)| !root.join(old).exists())
                .map(|(id, _)| id)
        } else {
            None
        };
        let target = existing.map(|(id, _, _)| id).or(moved_from);
        let res: rusqlite::Result<()> = (|| {
            let tx = conn.transaction()?;
            match target {
                Some(id) => {
                    tx.execute(
                        "UPDATE file SET sha256=?1, path=?7, bytes=?2, mtime=?3, codec=?4, duration_ms=?5,
                           sample_rate=?8, channels=?9, bitrate_kbps=?10, lossless=?11 WHERE id=?6",
                        params![sha, bytes, mtime, codec, p.duration_ms, id, rel, p.sample_rate, p.channels, bitrate_kbps, lossless],
                    )?;
                    // new content: comparisons made against the old audio no longer apply
                    tx.execute(
                        "DELETE FROM audio_match WHERE (a=?1 OR b=?1)
                           AND EXISTS (SELECT 1 FROM fingerprint WHERE file_id=?1 AND sha256<>?2)",
                        params![id, sha],
                    )?;
                    // the track follows its preferred file's tags
                    // a track the owner corrected keeps its corrected title, artist and album
                    tx.execute(
                        "UPDATE track SET title=CASE WHEN corrected=1 THEN title ELSE ?1 END,
                           artist=CASE WHEN corrected=1 THEN artist ELSE ?2 END,
                           album=CASE WHEN corrected=1 THEN album ELSE ?3 END,
                           search_text=CASE WHEN corrected=1 THEN search_text ELSE ?6 END,
                           duration_ms=?4, genre=?7 WHERE preferred_file_id=?5",
                        params![title, p.artist, p.album, p.duration_ms, id, search, p.genre],
                    )?;
                    tx.execute("UPDATE file SET track_id = (SELECT id FROM track WHERE preferred_file_id = ?1) WHERE id = ?1 AND track_id IS NULL", [id])?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO file(sha256, path, bytes, mtime, codec, duration_ms, sample_rate, channels, bitrate_kbps, lossless)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                        params![sha, rel, bytes, mtime, codec, p.duration_ms, p.sample_rate, p.channels, bitrate_kbps, lossless],
                    )?;
                    let id = tx.last_insert_rowid();
                    tx.execute(
                        "INSERT INTO track(title, artist, album, duration_ms, preferred_file_id, search_text, genre) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                        params![title, p.artist, p.album, p.duration_ms, id, search, p.genre],
                    )?;
                    let track_id = tx.last_insert_rowid();
                    tx.execute(
                        "UPDATE file SET track_id=?1 WHERE id=?2",
                        params![track_id, id],
                    )?;
                }
            }
            tx.commit()
        })();
        match (res, target) {
            (Ok(()), Some(_)) => stats.updated += 1,
            (Ok(()), None) => stats.added += 1,
            (Err(_), _) => stats.errors += 1,
        }
    }

    // Drop files that vanished from disk (their tracks go with them).
    let conn = crate::db::lock(db);
    let known: Vec<(i64, String)> = conn
        .prepare("SELECT id, path FROM file")
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .unwrap_or_default();
    for (id, path) in known {
        if seen.contains(&path) {
            continue;
        }
        // If the vanished file was the preferred one of a track that still has other files
        // (a merged duplicate), hand the track to another file instead of dropping it.
        let _ = conn.execute(
            "UPDATE track SET preferred_file_id = (SELECT id FROM file WHERE track_id = track.id AND id <> ?1 ORDER BY lossless DESC, bitrate_kbps DESC, id LIMIT 1)
             WHERE preferred_file_id = ?1 AND EXISTS (SELECT 1 FROM file WHERE track_id = track.id AND id <> ?1)",
            [id],
        );
        if conn.execute("DELETE FROM file WHERE id=?1", [id]).is_ok() {
            stats.removed += 1;
        }
    }
    stats
}
