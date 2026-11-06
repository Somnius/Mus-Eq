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

fn valid_url(u: &str) -> bool {
    (u.starts_with("http://") || u.starts_with("https://"))
        && u.len() <= 2048
        && !u.chars().any(|c| c.is_whitespace() || c.is_control())
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

const CHECK_TIMEOUT: Duration = Duration::from_secs(8);
const CONCURRENCY: usize = 8;

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

fn audio_like(content_type: &str, url: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    ct.is_empty()
        || [
            "audio/",
            "application/ogg",
            "application/octet-stream",
            "mpegurl",
            "binary/octet-stream",
            "video/mp2t",
            "x-scpls",
            "application/vnd.apple",
        ]
        .iter()
        .any(|k| ct.contains(k))
        || path.ends_with(".m3u8")
        || path.ends_with(".pls")
        || path.ends_with(".m3u")
}

/// Old Shoutcast servers answer `ICY 200 OK`, which HTTP clients reject; check those by hand.
async fn check_raw_http(url: &str) -> Verdict {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Some(rest) = url.strip_prefix("http://") else {
        return dead("not plain http");
    };
    let (hostport, path) = rest
        .split_once('/')
        .map(|(h, p)| (h, format!("/{p}")))
        .unwrap_or((rest, "/".into()));
    let addr = if hostport.contains(':') {
        hostport.to_string()
    } else {
        format!("{hostport}:80")
    };
    let host = hostport.split(':').next().unwrap_or(hostport);
    let run = async {
        let mut sock = tokio::net::TcpStream::connect(&addr)
            .await
            .map_err(|e| e.to_string())?;
        let req = format!("GET {path} HTTP/1.0\\r\\nHost: {host}\\r\\nUser-Agent: museq-station-check\\r\\nAccept: */*\\r\\n\\r\\n").replace("\\r\\n", "\r\n");
        sock.write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut buf = [0u8; 256];
        let n = sock.read(&mut buf).await.map_err(|e| e.to_string())?;
        Ok::<String, String>(String::from_utf8_lossy(&buf[..n]).into_owned())
    };
    match tokio::time::timeout(CHECK_TIMEOUT, run).await {
        Err(_) => dead("timed out"),
        Ok(Err(e)) => dead(e),
        Ok(Ok(head)) => {
            let first = head.lines().next().unwrap_or("");
            if first.starts_with("ICY 200") || first.contains(" 200") {
                Verdict {
                    alive: true,
                    error: None,
                }
            } else {
                dead(format!(
                    "unexpected reply: {}",
                    first.chars().take(60).collect::<String>()
                ))
            }
        }
    }
}

/// The reqwest message alone is "error sending request"; the useful part is further down the chain.
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
    let joined = parts.join(": ");
    // keep it short and drop the URL (it is already in the row)
    joined.chars().take(160).collect()
}

pub async fn check_one(client: &reqwest::Client, url: &str) -> Verdict {
    let fetch = async {
        let mut resp = client
            .get(url)
            .header("User-Agent", "museq-station-check")
            .header("Icy-MetaData", "0")
            .send()
            .await?;
        let status = resp.status();
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !(status.is_success()) {
            return Ok::<Verdict, reqwest::Error>(dead(format!("HTTP {}", status.as_u16())));
        }
        if !audio_like(&ct, url) {
            return Ok(dead(format!("not audio (content-type {ct})")));
        }
        // A live stream must actually send bytes.
        match resp.chunk().await? {
            Some(c) if !c.is_empty() => Ok(Verdict {
                alive: true,
                error: None,
            }),
            _ => Ok(dead("connected but sent no audio")),
        }
    };
    match tokio::time::timeout(CHECK_TIMEOUT, fetch).await {
        Err(_) => dead("timed out"),
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            if url.starts_with("http://") {
                let raw = check_raw_http(url).await;
                if raw.alive {
                    return raw;
                }
            }
            dead(error_chain(&e))
        }
    }
}

/// Check every station (a few at a time), recording the verdict. Never panics on a bad URL.
pub async fn check_all(db: std::sync::Arc<std::sync::Mutex<Connection>>) -> serde_json::Value {
    let list: Vec<(i64, String)> = {
        let conn = db::lock(&db);
        conn.prepare("SELECT id, url FROM station ORDER BY id")
            .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .unwrap_or_default()
    };
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
    {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();
    for (id, url) in list {
        let (client, sem, db) = (client.clone(), sem.clone(), db.clone());
        tasks.spawn(async move {
            let _permit = sem.acquire().await;
            let v = check_one(&client, &url).await;
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
    let (mut alive, mut dead_n) = (0u32, 0u32);
    while let Some(r) = tasks.join_next().await {
        match r {
            Ok(true) => alive += 1,
            _ => dead_n += 1,
        }
    }
    json!({"checked": alive + dead_n, "alive": alive, "dead": dead_n})
}
