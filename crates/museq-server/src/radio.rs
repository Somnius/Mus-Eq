//! Library radio: a station plays the owner's own tracks in a rotation, one encode shared by every
//! listener. The encoder runs only while somebody listens.
use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::broadcast;

use crate::api::{AppState, err, internal};
use crate::db;
use crate::transcode::{self, Codec};

/// What one running station shares between its encoder task and its listeners.
pub struct Hub {
    tx: broadcast::Sender<Bytes>,
    now: Mutex<Option<i64>>,             // track id on air
    stop: std::sync::atomic::AtomicBool, // set when the station is deleted
}

/// At most this many stations encode at the same time (each is one ffmpeg process).
const MAX_RUNNING: usize = 4;
/// At most this many stations can be defined.
const MAX_RADIOS: i64 = 50;

pub type Hubs = Arc<Mutex<HashMap<i64, Arc<Hub>>>>;

#[derive(Clone, Debug)]
pub struct Cand {
    pub file_id: i64,
    pub artist: Option<String>,
}

fn norm(a: &Option<String>) -> Option<String> {
    a.as_ref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
}

/// The next track: never one of the last `gap` played files, and not the same artist as the one
/// just played when there is any other choice. `roll` is a random number.
pub fn pick_next(cands: &[Cand], recent: &VecDeque<Cand>, gap: usize, roll: u64) -> Option<Cand> {
    let recent_files: Vec<i64> = recent.iter().rev().take(gap).map(|c| c.file_id).collect();
    let mut pool: Vec<&Cand> = cands
        .iter()
        .filter(|c| !recent_files.contains(&c.file_id))
        .collect();
    if pool.is_empty() {
        pool = cands.iter().collect(); // a small library: repeats are better than silence
    }
    if let Some(last) = recent.back().and_then(|c| norm(&c.artist)) {
        let other: Vec<&Cand> = pool
            .iter()
            .copied()
            .filter(|c| norm(&c.artist).as_ref() != Some(&last))
            .collect();
        if !other.is_empty() {
            pool = other;
        }
    }
    if pool.is_empty() {
        return None;
    }
    Some(pool[(roll % pool.len() as u64) as usize].clone())
}

/// A random key that opens one radio's stream and nothing else, for players that cannot send a
/// header (VLC, a phone's media player). Stored per radio, so it can be revoked on its own and
/// says nothing about the server token.
pub fn new_listen_key() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The radio's key, created on first use for radios made before keys existed.
pub fn listen_key(conn: &rusqlite::Connection, id: i64) -> Option<String> {
    let cur: Option<Option<String>> = conn
        .query_row("SELECT listen_key FROM radio WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .ok();
    match cur? {
        Some(k) => Some(k),
        None => {
            let k = new_listen_key();
            conn.execute("UPDATE radio SET listen_key=?2 WHERE id=?1", params![id, k])
                .ok()?;
            Some(k)
        }
    }
}

pub async fn rekey(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    let n = {
        let conn = db::lock(&s.db);
        conn.execute(
            "UPDATE radio SET listen_key=?2 WHERE id=?1",
            params![id, new_listen_key()],
        )
    };
    match n {
        Ok(0) => err(StatusCode::NOT_FOUND, "no such radio"),
        Ok(_) => match load(&s, Some(id)) {
            Ok(mut v) if !v.is_empty() => Json(v.remove(0)).into_response(),
            _ => internal("radio vanished"),
        },
        Err(e) => internal(e),
    }
}

fn roll() -> u64 {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    u64::from_le_bytes(b)
}

#[derive(Serialize)]
pub struct Radio {
    id: String,
    name: String,
    genre: Option<String>,
    codec: String,
    kbps: i64,
    artist_gap: i64,
    listeners: usize,
    /// Path (with key) a player without header support can open.
    listen_path: String,
    now_track_id: Option<String>,
}

fn load(s: &AppState, id: Option<i64>) -> rusqlite::Result<Vec<Radio>> {
    let conn = db::lock(&s.db);
    let mut st = conn.prepare(
        "SELECT id, name, genre, codec, kbps, artist_gap FROM radio WHERE (?1 IS NULL OR id=?1) ORDER BY id",
    )?;
    let rows = st
        .query_map([id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<(i64, String, Option<String>, String, i64, i64)>>>()?;
    let hubs = s.radios.lock().unwrap_or_else(|e| e.into_inner());
    Ok(rows
        .into_iter()
        .map(|(id, name, genre, codec, kbps, artist_gap)| {
            let hub = hubs.get(&id);
            Radio {
                id: id.to_string(),
                name,
                genre,
                codec,
                kbps,
                artist_gap,
                listen_path: format!(
                    "/v1/radios/{id}/stream?key={}",
                    listen_key(&conn, id).unwrap_or_default()
                ),
                listeners: hub.map_or(0, |h| h.tx.receiver_count()),
                now_track_id: hub
                    .and_then(|h| *h.now.lock().unwrap_or_else(|e| e.into_inner()))
                    .map(|v| v.to_string()),
            }
        })
        .collect())
}

pub async fn list_radios(State(s): State<AppState>) -> Response {
    match load(&s, None) {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct NewRadio {
    name: String,
    genre: Option<String>,
    codec: Option<String>,
    kbps: Option<i64>,
    artist_gap: Option<i64>,
}

pub async fn add_radio(State(s): State<AppState>, Json(b): Json<NewRadio>) -> Response {
    let name = b.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return err(StatusCode::BAD_REQUEST, "name must be 1 to 80 characters");
    }
    let codec = b.codec.as_deref().unwrap_or("mp3");
    let Some(c) = Codec::parse(codec) else {
        return err(StatusCode::BAD_REQUEST, "codec must be opus, mp3 or aac");
    };
    let kbps = b.kbps.unwrap_or(c.default_kbps());
    if !(32..=320).contains(&kbps) {
        return err(StatusCode::BAD_REQUEST, "kbps must be 32 to 320");
    }
    let gap = b.artist_gap.unwrap_or(5);
    if !(0..=200).contains(&gap) {
        return err(StatusCode::BAD_REQUEST, "artist_gap must be 0 to 200");
    }
    let count: i64 = db::lock(&s.db)
        .query_row("SELECT COUNT(*) FROM radio", [], |r| r.get(0))
        .unwrap_or(0);
    if count >= MAX_RADIOS {
        return err(StatusCode::CONFLICT, "too many radios defined");
    }
    let genre = b.genre.as_deref().map(str::trim).filter(|g| !g.is_empty());
    let id = {
        let conn = db::lock(&s.db);
        if let Err(e) = conn.execute(
            "INSERT INTO radio(name, genre, artist_gap, codec, kbps) VALUES(?1,?2,?3,?4,?5)",
            params![name, genre, gap, codec, kbps],
        ) {
            return internal(e);
        }
        conn.last_insert_rowid()
    };
    match load(&s, Some(id)) {
        Ok(mut v) if !v.is_empty() => (StatusCode::CREATED, Json(v.remove(0))).into_response(),
        Ok(_) => internal("radio vanished"),
        Err(e) => internal(e),
    }
}

pub async fn delete_radio(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    let n = {
        let conn = db::lock(&s.db);
        match conn.execute("DELETE FROM radio WHERE id=?1", [id]) {
            Ok(n) => n,
            Err(e) => return internal(e),
        }
    };
    if n == 0 {
        return err(StatusCode::NOT_FOUND, "no such radio");
    }
    // stop the encoder right now, mid-track
    let hubs = s.radios.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = hubs.get(&id) {
        h.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Everything a station could play now: the preferred file of each track, in the station's genre.
fn candidates(
    db: &Mutex<rusqlite::Connection>,
    genre: &Option<String>,
) -> Vec<(Cand, i64, String)> {
    let conn = db::lock(db);
    let Ok(mut st) = conn.prepare(
        "SELECT t.id, f.id, t.artist, f.path FROM track t JOIN file f ON f.id = t.preferred_file_id
         WHERE (?1 IS NULL OR lower(t.genre) = lower(?1))",
    ) else {
        return vec![];
    };
    st.query_map([genre], |r| {
        Ok((
            Cand {
                file_id: r.get(1)?,
                artist: r.get(2)?,
            },
            r.get::<_, i64>(0)?,
            r.get::<_, String>(3)?,
        ))
    })
    .map(|it| it.flatten().collect())
    .unwrap_or_default()
}

/// The station's encoder loop: pick a track, encode it at playing speed, send the bytes to every
/// listener, repeat. Ends when nobody listens, the station is deleted, or nothing can be played.
async fn run_station(s: AppState, id: i64, hub: Arc<Hub>) {
    let mut recent: VecDeque<Cand> = VecDeque::new();
    let mut failures = 0;
    loop {
        {
            // decide "nobody listens" and remove the hub under the lock listeners subscribe under,
            // so a listener can never join a station that is shutting down
            let mut hubs = s.radios.lock().unwrap_or_else(|e| e.into_inner());
            if hub.tx.receiver_count() == 0 {
                if hubs.get(&id).is_some_and(|h| Arc::ptr_eq(h, &hub)) {
                    hubs.remove(&id);
                }
                return;
            }
        }
        let station: Option<(Option<String>, i64, String, i64)> = {
            let conn = db::lock(&s.db);
            conn.query_row(
                "SELECT genre, artist_gap, codec, kbps FROM radio WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .ok()
        };
        let Some((genre, gap, codec, kbps)) = station else {
            break;
        };
        let Some(codec) = Codec::parse(&codec) else {
            break;
        };
        let all = candidates(&s.db, &genre);
        let cands: Vec<Cand> = all.iter().map(|(c, _, _)| c.clone()).collect();
        let Some(next) = pick_next(&cands, &recent, gap as usize, roll()) else {
            break;
        };
        let Some((_, track_id, rel)) = all.iter().find(|(c, _, _)| c.file_id == next.file_id)
        else {
            break;
        };
        *hub.now.lock().unwrap_or_else(|e| e.into_inner()) = Some(*track_id);
        recent.push_back(next);
        while recent.len() > 300 {
            recent.pop_front();
        }
        let mut child =
            match transcode::command(&s.ffmpeg, &s.root.join(rel), codec, kbps, true).spawn() {
                Ok(c) => c,
                Err(_) => break,
            };
        let mut out = child.stdout.take().expect("piped");
        let mut buf = vec![0u8; 8 * 1024];
        let mut sent = 0usize;
        let started = std::time::Instant::now();
        loop {
            if hub.stop.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            match out.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    sent += n;
                    // an error here means nobody is listening any more
                    if hub.tx.send(Bytes::copy_from_slice(&buf[..n])).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = child.kill().await;
        // a file that produced nothing (corrupt, vanished) is skipped; many in a row end the station
        if hub.stop.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        // a track that gave nothing, or ended within a second, counts as a failure: back off, and
        // give up after ten in a row (a broken ffmpeg must not spin)
        let quick = sent == 0 || started.elapsed() < Duration::from_secs(1);
        failures = if quick { failures + 1 } else { 0 };
        if failures >= 10 {
            break;
        }
        if quick {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    *hub.now.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let mut hubs = s.radios.lock().unwrap_or_else(|e| e.into_inner());
    if hubs.get(&id).is_some_and(|h| Arc::ptr_eq(h, &hub)) {
        hubs.remove(&id);
    }
}

pub async fn stream(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    let info: Option<(String, String)> = {
        let conn = db::lock(&s.db);
        conn.query_row("SELECT name, codec FROM radio WHERE id=?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .ok()
    };
    let Some((name, codec)) = info else {
        return err(StatusCode::NOT_FOUND, "no such radio");
    };
    let Some(codec) = Codec::parse(&codec) else {
        return internal("bad codec");
    };
    let (rx, start) = {
        let mut hubs = s.radios.lock().unwrap_or_else(|e| e.into_inner());
        match hubs.get(&id) {
            Some(h) => (h.tx.subscribe(), None),
            None if hubs.len() >= MAX_RUNNING => {
                return err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "too many radios are on air",
                );
            }
            None => {
                let (tx, rx) = broadcast::channel(64);
                let hub = Arc::new(Hub {
                    tx,
                    now: Mutex::new(None),
                    stop: Default::default(),
                });
                hubs.insert(id, hub.clone());
                (rx, Some(hub))
            }
        }
    };
    if let Some(hub) = start {
        tokio::spawn(run_station(s.clone(), id, hub));
    }
    let body = Body::from_stream(futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(b) => return Some((Ok::<_, std::io::Error>(b), rx)),
                Err(broadcast::error::RecvError::Lagged(_)) => continue, // a slow listener skips ahead
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }));
    let mut resp = Response::new(body);
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(transcode::mime(codec)),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("none"));
    if let Ok(v) = HeaderValue::from_str(&name.replace(['\r', '\n'], " ")) {
        h.insert("icy-name", v); // Icecast-style header that players show as the station name
    }
    resp
}
