// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
//! Metadata suggestions. Candidates come from MusicBrainz (text search) and AcoustID (by audio,
//! needs the owner's key). They are stored as candidates and only change a track when the owner
//! accepts one, and then only in the database: the audio files are never rewritten.
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, OptionalExtension, params};
use rusty_chromaprint::FingerprintCompressor;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::api::{AppState, err, internal};
use crate::{db, fingerprint};

const MAX_BODY: usize = 1 << 20;
const MIN_MB_SCORE: f64 = 70.0;
const CANDIDATES_PER_TRACK: usize = 3;

/// Where the metadata services live and how politely to talk to them.
#[derive(Clone, Debug)]
pub struct Services {
    pub musicbrainz_url: String,
    pub acoustid_url: String,
    pub acoustid_key: Option<String>,
    pub user_agent: String,
    pub mb_interval: Duration,
    pub acoustid_interval: Duration,
    /// How long to wait before retrying a 429/503 (doubles once; tests use zero)
    pub retry_backoff: Duration,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            musicbrainz_url: "https://musicbrainz.org/ws/2".into(),
            acoustid_url: "https://api.acoustid.org/v2".into(),
            acoustid_key: None,
            user_agent: format!(
                "Mus-Eq/{} (https://github.com/Somnius/Mus-Eq)",
                env!("CARGO_PKG_VERSION")
            ),
            mb_interval: Duration::from_millis(1100), // MusicBrainz: at most one request per second
            acoustid_interval: Duration::from_millis(350), // AcoustID: at most three per second
            retry_backoff: Duration::from_secs(4),
        }
    }
}

/// Spaces calls to one service. Shared by every job so two jobs can't double the rate.
#[derive(Default)]
pub struct RateLimiter(tokio::sync::Mutex<Option<Instant>>);

impl RateLimiter {
    pub async fn wait(&self, interval: Duration) {
        let mut last = self.0.lock().await;
        if let Some(prev) = *last {
            let ready = prev + interval;
            if let Some(d) = ready.checked_duration_since(Instant::now()) {
                tokio::time::sleep(d).await;
            }
        }
        *last = Some(Instant::now());
    }
}

#[derive(Default)]
pub struct Limiters {
    pub musicbrainz: RateLimiter,
    pub acoustid: RateLimiter,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub source: &'static str,
    pub score: f64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<String>,
    pub mbid: Option<String>,
}

fn client(svc: &Services) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(svc.user_agent.clone())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

async fn get_json(client: &reqwest::Client, url: reqwest::Url) -> Result<Value, String> {
    let mut resp = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| e.without_url().to_string())?
    {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY {
            return Err("reply too large".into());
        }
    }
    serde_json::from_slice(&body).map_err(|e| format!("not JSON: {e}"))
}

/// Pick the release that best stands for a recording: an official album that is not a live
/// recording or compilation, else any official release, else the first; earliest date wins.
fn best_release(releases: &[Value]) -> Option<&Value> {
    let rank = |r: &&Value| {
        let official = r["status"] == "Official";
        let group = &r["release-group"];
        let plain_album = group["primary-type"] == "Album"
            && group["secondary-types"]
                .as_array()
                .is_none_or(|a| a.is_empty());
        let date = r["date"]
            .as_str()
            .filter(|d| !d.is_empty())
            .unwrap_or("9999")
            .to_string();
        (!official, !plain_album, date)
    };
    releases.iter().min_by_key(rank)
}

pub fn parse_musicbrainz(v: &Value) -> Vec<Candidate> {
    let mut out = Vec::new();
    for r in v["recordings"].as_array().into_iter().flatten() {
        let score = r["score"].as_f64().unwrap_or(0.0);
        let Some(title) = r["title"].as_str() else {
            continue;
        };
        let artist = r["artist-credit"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|c| c["name"].as_str().or_else(|| c["artist"]["name"].as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|s| !s.is_empty());
        let rel = r["releases"].as_array().and_then(|a| best_release(a));
        out.push(Candidate {
            source: "musicbrainz",
            score,
            title: title.to_string(),
            artist,
            album: rel.and_then(|x| x["title"].as_str()).map(String::from),
            year: rel
                .and_then(|x| x["date"].as_str())
                .filter(|d| !d.is_empty())
                .map(|d| d.chars().take(4).collect()),
            mbid: r["id"].as_str().map(String::from),
        });
    }
    out
}

pub fn parse_acoustid(v: &Value) -> Vec<Candidate> {
    let mut out = Vec::new();
    for res in v["results"].as_array().into_iter().flatten() {
        let score = res["score"].as_f64().unwrap_or(0.0) * 100.0;
        for r in res["recordings"].as_array().into_iter().flatten() {
            let Some(title) = r["title"].as_str() else {
                continue;
            };
            let artist = r["artists"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x["name"].as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty());
            out.push(Candidate {
                source: "acoustid",
                score,
                title: title.to_string(),
                artist,
                album: r["releasegroups"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|g| g["title"].as_str())
                    .map(String::from),
                year: None,
                mbid: r["id"].as_str().map(String::from),
            });
        }
    }
    out
}

/// One MusicBrainz request, spaced by the shared limiter and retried when the service is busy
/// (it answers 503 now and then even at one request per second).
async fn musicbrainz_get(
    s: &AppState,
    c: &reqwest::Client,
    url: reqwest::Url,
) -> Result<Value, String> {
    let mut wait = s.services.retry_backoff;
    let mut last = String::new();
    for attempt in 0..3 {
        s.limiters.musicbrainz.wait(s.services.mb_interval).await;
        match get_json(c, url.clone()).await {
            Ok(v) => return Ok(v),
            Err(e)
                if (e == "HTTP 503" || e == "HTTP 429" || e.contains("error sending request"))
                    && attempt < 2 =>
            {
                last = e;
                tokio::time::sleep(wait).await;
                wait *= 2;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

fn mb_url(s: &AppState, query: &str, limit: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(&format!(
        "{}/recording",
        s.services.musicbrainz_url.trim_end_matches('/')
    ))
    .map_err(|e| e.to_string())?;
    url.query_pairs_mut()
        .append_pair("query", query)
        .append_pair("fmt", "json")
        .append_pair("limit", limit);
    Ok(url)
}

/// Best candidates first: highest score, then the earliest release (usually the original).
fn rank(mut cands: Vec<Candidate>) -> Vec<Candidate> {
    cands.retain(|c| c.score >= MIN_MB_SCORE);
    cands.sort_by(|a, b| {
        b.score.total_cmp(&a.score).then_with(|| {
            a.year
                .as_deref()
                .unwrap_or("9999")
                .cmp(b.year.as_deref().unwrap_or("9999"))
        })
    });
    let mut seen = std::collections::HashSet::new();
    cands.retain(|c| seen.insert((c.title.to_lowercase(), c.artist.clone(), c.album.clone())));
    cands.truncate(CANDIDATES_PER_TRACK);
    cands
}

async fn musicbrainz_search(
    s: &AppState,
    c: &reqwest::Client,
    title: &str,
    artist: Option<&str>,
) -> Result<Vec<Candidate>, String> {
    let esc = |t: &str| t.replace('\\', "\\\\").replace('"', "\\\"");
    let mut base = format!("recording:\"{}\"", esc(title));
    if let Some(a) = artist {
        base.push_str(&format!(" AND artist:\"{}\"", esc(a)));
    }
    // First ask for official studio albums (live recordings, bootlegs and compilations bury the
    // original otherwise); fall back to the plain search for singles and odd releases.
    let strict = format!(
        "{base} AND status:official AND primarytype:album AND NOT secondarytype:live AND NOT secondarytype:compilation"
    );
    let found = rank(parse_musicbrainz(
        &musicbrainz_get(s, c, mb_url(s, &strict, "10")?).await?,
    ));
    if !found.is_empty() {
        return Ok(found);
    }
    Ok(rank(parse_musicbrainz(
        &musicbrainz_get(s, c, mb_url(s, &base, "10")?).await?,
    )))
}

async fn acoustid_lookup(
    s: &AppState,
    c: &reqwest::Client,
    key: &str,
    fp: &[u32],
    duration_s: i64,
) -> Result<Vec<Candidate>, String> {
    let compressed = FingerprintCompressor::from(&fingerprint::config()).compress(fp);
    use base64::Engine;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(compressed);
    let mut url = reqwest::Url::parse(&format!(
        "{}/lookup",
        s.services.acoustid_url.trim_end_matches('/')
    ))
    .map_err(|e| e.to_string())?;
    url.query_pairs_mut()
        .append_pair("client", key)
        .append_pair("duration", &duration_s.to_string())
        .append_pair("fingerprint", &encoded)
        .append_pair("meta", "recordings releasegroups");
    s.limiters.acoustid.wait(s.services.acoustid_interval).await;
    let v = get_json(c, url).await?;
    if v["status"] != "ok" {
        return Err(format!(
            "AcoustID said: {}",
            v["error"]["message"].as_str().unwrap_or("error")
        ));
    }
    let mut cands = parse_acoustid(&v);
    cands.sort_by(|a, b| b.score.total_cmp(&a.score));
    cands.truncate(CANDIDATES_PER_TRACK);
    Ok(cands)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn store(conn: &Connection, track: i64, cands: &[Candidate]) -> usize {
    let mut n = 0;
    for c in cands {
        n += conn
            .execute(
                "INSERT INTO candidate(track_id, source, score, title, artist, album, year, mbid, created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(track_id, source, mbid) DO NOTHING",
                params![track, c.source, c.score, c.title, c.artist, c.album, c.year, c.mbid.clone().unwrap_or_default(), now()],
            )
            .unwrap_or(0);
    }
    n
}

/// The `tag` job: ask MusicBrainz about tracks with weak tags (no artist or no album), or about
/// the tracks the caller named. With an AcoustID key, tracks that have a fingerprint are also
/// identified by audio.
pub async fn run_tag_job(s: AppState, only: Option<Vec<i64>>) -> Value {
    let tracks: Vec<(i64, String, Option<String>)> = {
        let conn = db::lock(&s.db);
        let sql = match &only {
            // tracks the owner names are asked about even if they were corrected before
            Some(_) => "SELECT id, title, artist FROM track ORDER BY id",
            None => {
                "SELECT id, title, artist FROM track WHERE corrected = 0 AND (artist IS NULL OR artist = '' OR album IS NULL OR album = '') ORDER BY id"
            }
        };
        conn.prepare(sql)
            .and_then(|mut st| {
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect()
            })
            .unwrap_or_default()
    };
    let tracks: Vec<_> = tracks
        .into_iter()
        .filter(|(id, _, _)| only.as_ref().is_none_or(|o| o.contains(id)))
        .collect();
    let c = match client(&s.services) {
        Ok(c) => c,
        Err(e) => return json!({"error": e}),
    };
    let (mut asked, mut stored, mut failed) = (0u32, 0usize, 0u32);
    let mut last_error: Option<String> = None;
    for (id, title, artist) in tracks {
        asked += 1;
        let mut found: Vec<Candidate> = Vec::new();
        match musicbrainz_search(&s, &c, &title, artist.as_deref().filter(|a| !a.is_empty())).await
        {
            Ok(v) => found.extend(v),
            Err(e) => {
                failed += 1;
                last_error = Some(e);
            }
        }
        if let Some(key) = s.services.acoustid_key.clone() {
            let fp: Option<(Vec<u8>, i64)> = db::lock(&s.db)
                .query_row(
                    "SELECT p.data, p.duration_ms FROM fingerprint p JOIN file f ON f.id = p.file_id AND f.sha256 = p.sha256
                     WHERE f.track_id = ?1 AND p.status = 'ok' ORDER BY f.lossless DESC LIMIT 1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .unwrap_or(None);
            if let Some((data, ms)) = fp {
                match acoustid_lookup(&s, &c, &key, &fingerprint::from_blob(&data), ms / 1000).await
                {
                    Ok(v) => found.extend(v),
                    Err(e) => {
                        failed += 1;
                        last_error = Some(e);
                    }
                }
            }
        }
        stored += store(&db::lock(&s.db), id, &found);
    }
    json!({"tracks_asked": asked, "candidates_stored": stored, "lookups_failed": failed, "acoustid": s.services.acoustid_key.is_some(), "last_error": last_error})
}

#[derive(Serialize)]
struct CandidateRow {
    id: String,
    track_id: String,
    source: String,
    score: Option<f64>,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    year: Option<String>,
    mbid: Option<String>,
    status: String,
}

fn candidate_from_row(r: &rusqlite::Row) -> rusqlite::Result<CandidateRow> {
    Ok(CandidateRow {
        id: r.get::<_, i64>(0)?.to_string(),
        track_id: r.get::<_, i64>(1)?.to_string(),
        source: r.get(2)?,
        score: r.get(3)?,
        title: r.get(4)?,
        artist: r.get(5)?,
        album: r.get(6)?,
        year: r.get(7)?,
        mbid: r.get::<_, Option<String>>(8)?.filter(|s| !s.is_empty()),
        status: r.get(9)?,
    })
}

/// track_id, title, artist, album, mbid, status of one candidate row
type CandidateFields = (
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

const CAND_COLS: &str = "id, track_id, source, score, title, artist, album, year, mbid, status";

pub async fn list_candidates(State(s): State<AppState>, Path(track): Path<i64>) -> Response {
    let conn = db::lock(&s.db);
    match conn
        .query_row("SELECT 1 FROM track WHERE id=?1", [track], |_| Ok(()))
        .optional()
    {
        Ok(Some(())) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, "no such track"),
        Err(e) => return internal(e),
    }
    let rows: rusqlite::Result<Vec<CandidateRow>> = conn
        .prepare(&format!("SELECT {CAND_COLS} FROM candidate WHERE track_id=?1 ORDER BY (status='pending') DESC, score DESC, id"))
        .and_then(|mut st| st.query_map([track], candidate_from_row)?.collect());
    match rows {
        Ok(items) => Json(json!({"items": items})).into_response(),
        Err(e) => internal(e),
    }
}

/// Apply a candidate to its track (database only; files are untouched). The track is then marked
/// corrected, so a later rescan does not overwrite it with the old tags.
pub async fn accept(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    let mut conn = db::lock(&s.db);
    let res: rusqlite::Result<Option<(i64, String)>> = (|| {
        let tx = conn.transaction()?;
        let row: Option<CandidateFields> = tx
            .query_row(
                "SELECT track_id, title, artist, album, mbid, status FROM candidate WHERE id=?1",
                [id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((track, title, artist, album, mbid, status)) = row else {
            return Ok(None);
        };
        if status != "pending" {
            return Ok(Some((track, format!("already {status}"))));
        }
        let title = title.unwrap_or_default();
        // a candidate that lacks a field must not blank what the track already has
        tx.execute(
            "UPDATE track SET title=COALESCE(NULLIF(?1,''), title), artist=COALESCE(NULLIF(?2,''), artist),
               album=COALESCE(NULLIF(?3,''), album), mbid=NULLIF(?4,''), corrected=1 WHERE id=?5",
            params![title, artist, album, mbid, track],
        )?;
        let (t2, a2, b2): (String, Option<String>, Option<String>) = tx.query_row(
            "SELECT title, artist, album FROM track WHERE id=?1",
            [track],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        tx.execute(
            "UPDATE track SET search_text=?1 WHERE id=?2",
            params![
                crate::db::search_text(&t2, a2.as_deref(), b2.as_deref()),
                track
            ],
        )?;
        tx.execute("UPDATE candidate SET status='accepted' WHERE id=?1", [id])?;
        tx.execute(
            "UPDATE candidate SET status='superseded' WHERE track_id=?1 AND status='pending'",
            [track],
        )?;
        tx.commit()?;
        Ok(Some((track, "accepted".into())))
    })();
    match res {
        Ok(None) => err(StatusCode::NOT_FOUND, "no such candidate"),
        Ok(Some((_, msg))) if msg != "accepted" => {
            err(StatusCode::CONFLICT, &format!("this candidate is {msg}"))
        }
        Ok(Some((track, _))) => {
            Json(json!({"track_id": track.to_string(), "status": "accepted"})).into_response()
        }
        Err(e) => internal(e),
    }
}

pub async fn reject(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    match db::lock(&s.db).execute(
        "UPDATE candidate SET status='rejected' WHERE id=?1 AND status='pending'",
        [id],
    ) {
        Ok(0) => err(StatusCode::NOT_FOUND, "no such pending candidate"),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

/// Identify one track by its audio through AcoustID. 501 without the owner's key.
pub async fn identify(State(s): State<AppState>, Path(track): Path<i64>) -> Response {
    let Some(key) = s.services.acoustid_key.clone() else {
        return err(
            StatusCode::NOT_IMPLEMENTED,
            "set MUSEQ_ACOUSTID_KEY (a free AcoustID application key) to identify tracks by audio",
        );
    };
    let fp: Option<(Vec<u8>, i64)> = match db::lock(&s.db)
        .query_row(
            "SELECT p.data, p.duration_ms FROM fingerprint p JOIN file f ON f.id = p.file_id AND f.sha256 = p.sha256
             WHERE f.track_id = ?1 AND p.status = 'ok' ORDER BY f.lossless DESC LIMIT 1",
            [track],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
    {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let Some((data, ms)) = fp else {
        return err(
            StatusCode::CONFLICT,
            "this track has no fingerprint yet: run the fingerprint job first",
        );
    };
    let c = match client(&s.services) {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    match acoustid_lookup(&s, &c, &key, &fingerprint::from_blob(&data), ms / 1000).await {
        Ok(cands) => {
            let n = store(&db::lock(&s.db), track, &cands);
            Json(json!({"found": cands.len(), "stored": n})).into_response()
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, &e),
    }
}

pub fn services_arc(s: Services) -> Arc<Services> {
    Arc::new(s)
}
