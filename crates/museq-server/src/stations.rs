use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

use crate::api::{AppState, err, internal};
use crate::db;

const CATEGORIES: &[&str] = &["Music", "News", "Sports", "Other"];

#[derive(Debug, PartialEq)]
pub struct SeedStation {
    pub name: String,
    pub category: String,
    pub url: String,
}

/// A stream address we accept: http(s), a real host, no embedded credentials, no whitespace.
pub fn parse_stream_url(u: &str) -> Option<url::Url> {
    if u.len() > 2048 || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let parsed = url::Url::parse(u).ok()?;
    let ok = matches!(parsed.scheme(), "http" | "https")
        && parsed.host().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none();
    ok.then_some(parsed)
}

fn valid_url(u: &str) -> bool {
    parse_stream_url(u).is_some()
}

/// True for addresses on the public internet. Loopback, private ranges, link-local (which
/// includes the cloud metadata address), carrier-grade NAT, multicast and unspecified are not.
pub fn is_public(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_loopback()
                || v.is_private()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_broadcast()
                || v.is_multicast()
                || (o[0] == 100 && (o[1] & 0xC0) == 64) // 100.64.0.0/10
                || o[0] == 0)
        }
        IpAddr::V6(v) => {
            if let Some(mapped) = v.to_ipv4_mapped() {
                return is_public(IpAddr::V4(mapped));
            }
            let seg = v.segments();
            !(v.is_loopback()
                || v.is_unspecified()
                || v.is_multicast()
                || (seg[0] & 0xFE00) == 0xFC00 // fc00::/7 unique local
                || (seg[0] & 0xFFC0) == 0xFE80) // fe80::/10 link-local
        }
    }
}

/// Refuse stream hosts that are not on the public internet, unless the owner allowed it
/// (`MUSEQ_ALLOW_PRIVATE_STATIONS`). Domain names are resolved and every address must be public.
/// Note: the connection resolves the name again, so a hostile DNS server could still answer
/// differently the second time; stations are added by the owner only.
pub async fn host_allowed(url: &url::Url, allow_private: bool) -> Result<(), String> {
    if allow_private {
        return Ok(());
    }
    let port = url.port_or_known_default().unwrap_or(80);
    match url.host() {
        Some(url::Host::Ipv4(ip)) => is_public(ip.into())
            .then_some(())
            .ok_or_else(|| "address is not public".to_string()),
        Some(url::Host::Ipv6(ip)) => is_public(ip.into())
            .then_some(())
            .ok_or_else(|| "address is not public".to_string()),
        Some(url::Host::Domain(d)) => {
            let addrs: Vec<_> =
                tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((d, port)))
                    .await
                    .map_err(|_| "DNS lookup timed out".to_string())?
                    .map_err(|e| format!("DNS lookup failed: {e}"))?
                    .collect();
            if addrs.is_empty() {
                Err("host did not resolve".into())
            } else if addrs.iter().all(|a| is_public(a.ip())) {
                Ok(())
            } else {
                Err("address is not public".into())
            }
        }
        None => Err("no host".into()),
    }
}

fn category_of(c: &str) -> String {
    CATEGORIES
        .iter()
        .find(|k| k.eq_ignore_ascii_case(c.trim()))
        .copied()
        .unwrap_or("Other")
        .to_string()
}

/// Parse the `Name|Category|URL` list format (`#` starts a comment). Bad lines are skipped.
pub fn parse_list(text: &str) -> Vec<SeedStation> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let mut it = line.splitn(3, '|');
            let (name, cat, url) = (it.next()?.trim(), it.next()?.trim(), it.next()?.trim());
            if name.is_empty() || !valid_url(url) {
                return None;
            }
            Some(SeedStation {
                name: name.to_string(),
                category: category_of(cat),
                url: url.to_string(),
            })
        })
        .collect()
}

/// Add stations that aren't there yet (matched by URL). Returns how many were new.
pub fn import(conn: &Connection, source: &str, list: &[SeedStation]) -> rusqlite::Result<usize> {
    let mut added = 0;
    for s in list {
        added += conn.execute(
            "INSERT INTO station(name, category, url, source, search_text) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(url) DO NOTHING",
            params![
                s.name,
                s.category,
                s.url,
                source,
                crate::text::fold(&s.name)
            ],
        )?;
    }
    Ok(added)
}

#[derive(Serialize)]
struct Station {
    id: String,
    name: String,
    kind: &'static str,
    category: String,
    url: String,
    source: String,
    /// null = never checked
    alive: Option<bool>,
    checked_at: Option<i64>,
    error: Option<String>,
}

fn station_from_row(r: &rusqlite::Row) -> rusqlite::Result<Station> {
    Ok(Station {
        id: r.get::<_, i64>(0)?.to_string(),
        name: r.get(1)?,
        kind: "external",
        category: r.get(2)?,
        url: r.get(3)?,
        source: r.get(4)?,
        alive: r.get::<_, Option<i64>>(5)?.map(|v| v != 0),
        checked_at: r.get(6)?,
        error: r.get(7)?,
    })
}

const COLS: &str = "id, name, category, url, source, alive, checked_at, error";

#[derive(Deserialize)]
pub struct ListQuery {
    q: Option<String>,
    category: Option<String>,
    /// `true`, `false` or `unknown`
    alive: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

pub async fn list_stations(State(s): State<AppState>, Query(q): Query<ListQuery>) -> Response {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let pattern =
        q.q.filter(|t| !t.is_empty())
            .map(|t| {
                let t = crate::text::fold(&t);
                format!(
                    "%{}%",
                    t.replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )
            })
            .unwrap_or_else(|| "%".into());
    let category = q
        .category
        .filter(|c| !c.is_empty())
        .map(|c| category_of(&c));
    let alive_clause = match q.alive.as_deref() {
        Some("true") => " AND alive = 1",
        Some("false") => " AND alive = 0",
        Some("unknown") => " AND alive IS NULL",
        None | Some("") => "",
        Some(_) => {
            return err(
                StatusCode::BAD_REQUEST,
                "alive must be true, false or unknown",
            );
        }
    };
    let where_ =
        format!("search_text LIKE ?1 ESCAPE '\\' AND (?2 IS NULL OR category = ?2){alive_clause}");
    let conn = db::lock(&s.db);
    let total: i64 = match conn.query_row(
        &format!("SELECT COUNT(*) FROM station WHERE {where_}"),
        params![pattern, category],
        |r| r.get(0),
    ) {
        Ok(n) => n,
        Err(e) => return internal(e),
    };
    let items: rusqlite::Result<Vec<Station>> = conn
        .prepare(&format!(
            "SELECT {COLS} FROM station WHERE {where_} ORDER BY name, id LIMIT ?3 OFFSET ?4"
        ))
        .and_then(|mut st| {
            st.query_map(params![pattern, category, limit, offset], station_from_row)?
                .collect()
        });
    match items {
        Ok(items) => Json(json!({"items": items, "total": total})).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct NewStation {
    name: String,
    url: String,
    category: Option<String>,
}

pub async fn add_station(State(s): State<AppState>, Json(b): Json<NewStation>) -> Response {
    let (name, url) = (b.name.trim().to_string(), b.url.trim().to_string());
    if name.is_empty() || name.len() > 200 {
        return err(StatusCode::BAD_REQUEST, "name must be 1-200 characters");
    }
    if !valid_url(&url) {
        return err(StatusCode::BAD_REQUEST, "url must be an http(s) stream URL");
    }
    let category = category_of(b.category.as_deref().unwrap_or("Other"));
    if let Some(parsed) = parse_stream_url(&url)
        && let Err(why) = host_allowed(&parsed, s.allow_private_stations).await
    {
        return err(
            StatusCode::BAD_REQUEST,
            &format!(
                "that address is not allowed ({why}); set MUSEQ_ALLOW_PRIVATE_STATIONS=1 to use stations on your own network"
            ),
        );
    }
    let conn = db::lock(&s.db);
    let inserted = conn.execute(
        "INSERT INTO station(name, category, url, source, search_text) VALUES(?1,?2,?3,'manual',?4)
         ON CONFLICT(url) DO NOTHING",
        params![name, category, url, crate::text::fold(&name)],
    );
    match inserted {
        Ok(0) => err(
            StatusCode::CONFLICT,
            "a station with that URL already exists",
        ),
        Ok(_) => {
            let id = conn.last_insert_rowid();
            match conn.query_row(
                &format!("SELECT {COLS} FROM station WHERE id=?1"),
                [id],
                station_from_row,
            ) {
                Ok(st) => (StatusCode::CREATED, Json(st)).into_response(),
                Err(e) => internal(e),
            }
        }
        Err(e) => internal(e),
    }
}

pub async fn get_station(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    use rusqlite::OptionalExtension;
    match db::lock(&s.db)
        .query_row(
            &format!("SELECT {COLS} FROM station WHERE id=?1"),
            [id],
            station_from_row,
        )
        .optional()
    {
        Ok(Some(st)) => Json(st).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such station"),
        Err(e) => internal(e),
    }
}

pub async fn delete_station(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    match db::lock(&s.db).execute("DELETE FROM station WHERE id=?1", [id]) {
        Ok(0) => err(StatusCode::NOT_FOUND, "no such station"),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

// ---- liveness check ---------------------------------------------------------------------

/// One whole check (all redirect hops and the fallback) must finish within this.
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const CONCURRENCY: usize = 8;
const MAX_REDIRECTS: usize = 3;

#[derive(Debug, PartialEq)]
pub struct Verdict {
    pub alive: bool,
    pub error: Option<String>,
}

fn dead(msg: impl Into<String>) -> Verdict {
    Verdict {
        alive: false,
        error: Some(msg.into()),
    }
}

fn alive() -> Verdict {
    Verdict {
        alive: true,
        error: None,
    }
}

enum CtKind {
    Audio,   // clearly audio or a playlist: trust it
    Unknown, // empty or generic binary: the first bytes must look like audio
    Other,   // text/html and friends
}

fn classify(content_type: &str, url: &str) -> CtKind {
    let ct = content_type.to_ascii_lowercase();
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    if [
        "audio/",
        "application/ogg",
        "mpegurl",
        "x-scpls",
        "application/vnd.apple",
    ]
    .iter()
    .any(|k| ct.contains(k))
        || path.ends_with(".m3u8")
        || path.ends_with(".pls")
        || path.ends_with(".m3u")
    {
        CtKind::Audio
    } else if ct.is_empty() || ct.contains("octet-stream") || ct.contains("video/mp2t") {
        CtKind::Unknown
    } else {
        CtKind::Other
    }
}

/// Do these first bytes look like an audio stream or a playlist?
pub fn sniff_audio(b: &[u8]) -> bool {
    b.starts_with(b"ID3")
        || b.starts_with(b"OggS")
        || b.starts_with(b"fLaC")
        || b.starts_with(b"RIFF")
        || b.starts_with(b"#EXTM3U")
        || b.starts_with(b"[playlist")
        || b.starts_with(b"FLV")
        || (b.len() >= 2 && b[0] == 0xFF && (b[1] & 0xE0) == 0xE0) // MP3/AAC frame sync
        || (b.len() >= 8 && &b[4..8] == b"ftyp")
        || b.first() == Some(&0x47) // MPEG transport stream
}

fn verdict_for(ct: &str, url: &str, first: &[u8]) -> Verdict {
    if first.is_empty() {
        return dead("connected but sent no audio");
    }
    match classify(ct, url) {
        CtKind::Audio => alive(),
        CtKind::Unknown if sniff_audio(first) => alive(),
        CtKind::Unknown => dead("sent bytes that do not look like audio"),
        CtKind::Other => dead(format!("not audio (content-type {ct})")),
    }
}

/// Old Shoutcast servers answer `ICY 200 OK`, which HTTP clients reject; read those by hand.
/// Only used for plain http, and only when the HTTP client could not parse the reply.
async fn check_raw_http(url: &url::Url) -> Verdict {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Some(host) = url.host_str() else {
        return dead("no host");
    };
    let port = url.port_or_known_default().unwrap_or(80);
    let target = &url[url::Position::BeforePath..url::Position::AfterQuery];
    let run = async {
        let mut sock = tokio::net::TcpStream::connect((host.trim_matches(['[', ']']), port))
            .await
            .map_err(|e| e.to_string())?;
        let req = format!(
            "GET {target} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: museq-station-check\r\nAccept: */*\r\n\r\n"
        );
        sock.write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        // read until the end of the headers plus at least one body byte, or 4 KB
        while buf.len() < 4096 {
            let n = sock.read(&mut chunk).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n")
                && buf.len() > end + 4
            {
                break;
            }
        }
        Ok::<Vec<u8>, String>(buf)
    };
    let buf = match run.await {
        Ok(b) => b,
        Err(e) => return dead(e),
    };
    let end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let body = &buf[end.min(buf.len())..];
    let first = head.lines().next().unwrap_or("");
    let ok_status = first.starts_with("ICY 200")
        || first.starts_with("HTTP/1.0 200")
        || first.starts_with("HTTP/1.1 200");
    if !ok_status {
        return dead(format!(
            "unexpected reply: {}",
            first.chars().take(40).collect::<String>()
        ));
    }
    if first.starts_with("ICY") {
        return if body.is_empty() {
            dead("connected but sent no audio")
        } else {
            alive()
        };
    }
    let ct = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-type:")
                .map(|v| v.trim().to_string())
        })
        .unwrap_or_default();
    verdict_for(&ct, url.as_str(), body)
}

enum Hop {
    Done(Verdict),
    Redirect(String),
    ParseTrouble(String), // the reply was not valid HTTP: maybe an ICY server
}

fn error_chain(e: &reqwest::Error) -> String {
    use std::error::Error;
    let mut parts = vec![e.to_string()];
    let mut src = e.source();
    while let Some(s) = src {
        let t = s.to_string();
        if parts.last() != Some(&t) {
            parts.push(t);
        }
        src = s.source();
    }
    parts.join(": ").chars().take(160).collect()
}

async fn fetch_once(client: &reqwest::Client, url: &str) -> Hop {
    let mut resp = match client
        .get(url)
        .header("User-Agent", "museq-station-check")
        .header("Icy-MetaData", "0")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let chain = error_chain(&e);
            let low = chain.to_ascii_lowercase();
            return if low.contains("invalid http version")
                || low.contains("invalid status")
                || low.contains("malformed")
            {
                Hop::ParseTrouble(chain)
            } else {
                Hop::Done(dead(chain))
            };
        }
    };
    let status = resp.status();
    if status.is_redirection() {
        return match resp.headers().get("location").and_then(|v| v.to_str().ok()) {
            Some(loc) => Hop::Redirect(loc.to_string()),
            None => Hop::Done(dead(format!("HTTP {} without a location", status.as_u16()))),
        };
    }
    if !status.is_success() {
        return Hop::Done(dead(format!("HTTP {}", status.as_u16())));
    }
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    match resp.chunk().await {
        Ok(Some(first)) => Hop::Done(verdict_for(&ct, url, &first)),
        Ok(None) => Hop::Done(dead("connected but sent no audio")),
        Err(e) => Hop::Done(dead(error_chain(&e))),
    }
}

/// Check one stream: follow up to three redirects by hand (each hop is checked against the
/// host policy), within one overall deadline.
pub async fn check_one(client: &reqwest::Client, start: &str, allow_private: bool) -> Verdict {
    let run = async {
        let mut current = start.to_string();
        for _ in 0..=MAX_REDIRECTS {
            let Some(parsed) = parse_stream_url(&current) else {
                return dead("not a usable stream address");
            };
            if let Err(why) = host_allowed(&parsed, allow_private).await {
                return dead(why);
            }
            match fetch_once(client, parsed.as_str()).await {
                Hop::Done(v) => return v,
                Hop::Redirect(loc) => match parsed.join(&loc) {
                    Ok(next) => current = next.to_string(),
                    Err(_) => return dead("bad redirect address"),
                },
                Hop::ParseTrouble(chain) => {
                    return if parsed.scheme() == "http" {
                        let raw = check_raw_http(&parsed).await;
                        if raw.alive { raw } else { dead(chain) }
                    } else {
                        dead(chain)
                    };
                }
            }
        }
        dead("too many redirects")
    };
    tokio::time::timeout(CHECK_TIMEOUT, run)
        .await
        .unwrap_or_else(|_| dead("timed out"))
}

/// Check every station (a few at a time), recording the verdict. Never panics on a bad URL.
pub async fn check_all(
    db: std::sync::Arc<std::sync::Mutex<Connection>>,
    allow_private: bool,
) -> serde_json::Value {
    let list: Vec<(i64, String)> = {
        let conn = db::lock(&db);
        conn.prepare("SELECT id, url FROM station ORDER BY id")
            .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .unwrap_or_default()
    };
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();
    for (id, url) in list {
        // take the permit before spawning, so a huge list never creates thousands of tasks at once
        let Ok(permit) = sem.clone().acquire_owned().await else {
            break;
        };
        let (client, db) = (client.clone(), db.clone());
        tasks.spawn(async move {
            let _permit = permit;
            let v = check_one(&client, &url, allow_private).await;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let _ = db::lock(&db).execute(
                "UPDATE station SET alive=?1, checked_at=?2, error=?3 WHERE id=?4",
                params![v.alive, now, v.error, id],
            );
            v.alive
        });
    }
    let (mut ok, mut bad) = (0u32, 0u32);
    while let Some(r) = tasks.join_next().await {
        match r {
            Ok(true) => ok += 1,
            _ => bad += 1,
        }
    }
    json!({"checked": ok + bad, "alive": ok, "dead": bad})
}
