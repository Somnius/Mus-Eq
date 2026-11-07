// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, Method, StatusCode, header, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::scan;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub root: Arc<PathBuf>,
    pub token: Arc<String>,
    pub skins_dir: Arc<PathBuf>,
    pub web_dir: Option<Arc<PathBuf>>,
    pub events: tokio::sync::broadcast::Sender<String>,
    pub session: Arc<Mutex<crate::session::Session>>,
    /// Browser login sessions: random cookie value -> when it was issued. Lost on restart (log in again).
    pub cookies: Arc<Mutex<HashMap<String, Instant>>>,
    /// (window start, failed attempts) for the login rate limit.
    pub login_fails: Arc<Mutex<(Instant, u32)>>,
    /// Connected WebSocket clients by client id (a player must be connected to receive commands).
    pub players: Arc<Mutex<HashMap<String, usize>>>,
    /// Allow station URLs on loopback/private addresses (`MUSEQ_ALLOW_PRIVATE_STATIONS`).
    pub allow_private_stations: bool,
    pub services: Arc<crate::metadata::Services>,
    pub limiters: Arc<crate::metadata::Limiters>,
    pub fetch: Arc<crate::fetch::FetchConfig>,
    /// External programs (ffmpeg, yt-dlp): found, or installed on request after a confirmation.
    pub tools: Arc<crate::tools::Tools>,
    /// Running library radios.
    pub radios: crate::radio::Hubs,
    /// At most this many transcodes at once.
    pub transcodes: Arc<tokio::sync::Semaphore>,
}

/// A folder name nobody else can guess, for callers that never set a tools folder (tests); `main` always does.
fn unique_tools_dir() -> PathBuf {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    let tag: String = b.iter().map(|x| format!("{x:02x}")).collect();
    std::env::temp_dir().join(format!("museq-tools-{}-{tag}", std::process::id()))
}

const COOKIE_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
const MAX_COOKIES: usize = 64;
const MAX_LOGIN_FAILS: u32 = 10;
const LOGIN_WINDOW: Duration = Duration::from_secs(60);

impl AppState {
    pub fn new(
        db: Connection,
        root: PathBuf,
        token: String,
        skins_dir: PathBuf,
        web_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
            root: Arc::new(root),
            token: Arc::new(token),
            skins_dir: Arc::new(skins_dir),
            web_dir: web_dir.map(Arc::new),
            events: tokio::sync::broadcast::channel(64).0,
            session: Arc::new(Mutex::new(crate::session::Session::default())),
            cookies: Arc::new(Mutex::new(HashMap::new())),
            login_fails: Arc::new(Mutex::new((Instant::now(), 0))),
            players: Arc::new(Mutex::new(HashMap::new())),
            allow_private_stations: false,
            services: Arc::new(crate::metadata::Services::default()),
            limiters: Arc::new(crate::metadata::Limiters::default()),
            fetch: Arc::new(crate::fetch::FetchConfig::default()),
            tools: Arc::new(crate::tools::Tools::builtin(unique_tools_dir())),
            radios: Default::default(),
            transcodes: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }

    /// Use this ffmpeg instead of any downloaded one or the one on PATH.
    pub fn ffmpeg(self, path: String) -> Self {
        self.tools.set_explicit("ffmpeg", path);
        self
    }

    pub fn tools(mut self, tools: crate::tools::Tools) -> Self {
        self.tools = Arc::new(tools);
        self
    }

    /// Like `tools`, for a caller that keeps its own handle (tests).
    pub fn tools_shared(mut self, tools: Arc<crate::tools::Tools>) -> Self {
        self.tools = tools;
        self
    }

    /// The yt-dlp the fetch feature runs: the path named by `MUSEQ_FETCH_YTDLP`, else the copy Mus-Eq installed.
    /// It is never looked up on PATH.
    pub fn ytdlp_path(&self) -> Option<PathBuf> {
        if !self.fetch.ytdlp.as_os_str().is_empty() {
            return Some(self.fetch.ytdlp.clone());
        }
        self.tools.installed_binary("yt-dlp")
    }

    /// The ffmpeg to run: the configured one, else the installed one, else `ffmpeg` from PATH.
    pub fn ffmpeg_path(&self) -> String {
        self.tools.resolve("ffmpeg")
    }

    pub fn fetch(mut self, cfg: crate::fetch::FetchConfig) -> Self {
        self.fetch = Arc::new(cfg);
        self
    }

    pub fn services(mut self, services: crate::metadata::Services) -> Self {
        self.services = Arc::new(services);
        self
    }

    pub fn allow_private_stations(mut self, allow: bool) -> Self {
        self.allow_private_stations = allow;
        self
    }
}

pub(crate) fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

pub(crate) fn internal<E: std::fmt::Display>(e: E) -> Response {
    eprintln!("internal error: {e}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/tracks", get(search_tracks))
        .route("/v1/tracks/manifest", get(tracks_manifest))
        .route("/v1/tracks/{id}", get(get_track))
        .route("/v1/audio/{file_id}", get(audio))
        .route("/v1/tools", get(list_tools))
        .route("/v1/tools/{name}", axum::routing::delete(remove_tool))
        .route("/v1/tools/{name}/install", post(install_tool))
        .route("/v1/playlists", get(list_playlists).post(create_playlist))
        .route(
            "/v1/playlists/{id}",
            get(get_playlist).delete(delete_playlist),
        )
        .route("/v1/playlists/{id}/items", post(add_item).put(set_items))
        .route(
            "/v1/playlists/{id}/extend",
            post(crate::extend::extend_playlist),
        )
        .route("/v1/logout", post(logout))
        .route("/v1/jobs", post(create_job))
        .route("/v1/jobs/{id}", get(get_job))
        .route(
            "/v1/session",
            get(crate::session::get_session).post(crate::session::post_session),
        )
        .route("/v1/events", get(crate::session::events))
        .route(
            "/v1/skins",
            get(crate::skins::list_skins).post(crate::skins::upload_skin),
        )
        .route("/v1/skins/{id}", get(crate::skins::get_skin))
        .route(
            "/v1/tracks/{id}/candidates",
            get(crate::metadata::list_candidates),
        )
        .route("/v1/tracks/{id}/identify", post(crate::metadata::identify))
        .route("/v1/candidates/{id}/accept", post(crate::metadata::accept))
        .route("/v1/candidates/{id}/reject", post(crate::metadata::reject))
        .route("/v1/fetch/status", get(fetch_status))
        .route(
            "/v1/radios",
            get(crate::radio::list_radios).post(crate::radio::add_radio),
        )
        .route(
            "/v1/radios/{id}",
            axum::routing::delete(crate::radio::delete_radio),
        )
        .route("/v1/radios/{id}/stream", get(crate::radio::stream))
        .route("/v1/radios/{id}/rekey", post(crate::radio::rekey))
        .route("/v1/duplicates", get(crate::duplicates::list_duplicates))
        .route("/v1/duplicates/merge", post(crate::duplicates::merge))
        .route(
            "/v1/stations",
            get(crate::stations::list_stations).post(crate::stations::add_station),
        )
        .route(
            "/v1/stations/{id}",
            get(crate::stations::get_station).delete(crate::stations::delete_station),
        )
        .layer(DefaultBodyLimit::max(crate::skins::MAX_SKIN_BYTES))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));
    let mut app = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/login", post(login))
        .merge(protected)
        // an unknown API path must not fall through to the web client's index.html
        .route(
            "/v1/{*rest}",
            axum::routing::any(|| async { err(StatusCode::NOT_FOUND, "no such endpoint") }),
        );
    if let Some(dir) = state.web_dir.clone() {
        // The web client is served from the same origin as the API, so cookies need no CORS.
        let index = dir.join("index.html");
        app = app.fallback_service(
            tower_http::services::ServeDir::new(&*dir).fallback(ServeFile::new(index)),
        );
    }
    app.with_state(state)
}

// ---- auth -------------------------------------------------------------

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

const COOKIE: &str = "museq_token";

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|p| p.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

fn radio_stream_id(path: &str) -> Option<i64> {
    path.strip_prefix("/v1/radios/")?
        .strip_suffix("/stream")?
        .parse()
        .ok()
}

async fn auth(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let bearer = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| ct_eq(t.as_bytes(), s.token.as_bytes()))
        .unwrap_or(false);
    // a radio's own listen key opens that radio's stream, and nothing else
    let radio_key = !bearer
        && req.method() == Method::GET
        && radio_stream_id(req.uri().path()).is_some_and(|id| {
            req.uri()
                .query()
                .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("key=")))
                .is_some_and(|k| {
                    let conn = crate::db::lock(&s.db);
                    crate::radio::listen_key(&conn, id)
                        .is_some_and(|real| ct_eq(k.as_bytes(), real.as_bytes()))
                })
        });
    if !bearer && !radio_key {
        let cookie = cookie_token(req.headers())
            .map(|id| {
                let jar = s.cookies.lock().unwrap_or_else(|e| e.into_inner());
                jar.get(&id)
                    .is_some_and(|issued| issued.elapsed() < COOKIE_TTL)
            })
            .unwrap_or(false);
        if !cookie {
            return err(StatusCode::UNAUTHORIZED, "missing or invalid token");
        }
        // Cookie auth is ambient, so mutating requests must be JSON or a raw upload:
        // a cross-site HTML form cannot send those without a CORS preflight.
        if !matches!(*req.method(), Method::GET | Method::HEAD) {
            let ct = req
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !(ct.starts_with("application/json") || ct.starts_with("application/octet-stream")) {
                return err(StatusCode::UNSUPPORTED_MEDIA_TYPE, "use application/json");
            }
        }
    }
    next.run(req).await
}

#[derive(Deserialize)]
struct Login {
    token: String,
}

async fn login(State(s): State<AppState>, Json(body): Json<Login>) -> Response {
    {
        // Brute-force guard: too many failures in a minute and logins pause for the rest of it.
        let mut f = s.login_fails.lock().unwrap_or_else(|e| e.into_inner());
        if f.0.elapsed() > LOGIN_WINDOW {
            *f = (Instant::now(), 0);
        }
        if f.1 >= MAX_LOGIN_FAILS {
            return err(
                StatusCode::TOO_MANY_REQUESTS,
                "too many failed logins, try again in a minute",
            );
        }
        if !ct_eq(body.token.as_bytes(), s.token.as_bytes()) {
            f.1 += 1;
            return err(StatusCode::UNAUTHORIZED, "invalid token");
        }
    }
    // The cookie is a random session id, never the token itself, so logout really revokes it.
    let mut raw = [0u8; 32];
    if getrandom::fill(&mut raw).is_err() {
        return internal("no randomness available");
    }
    let id: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    {
        let mut jar = s.cookies.lock().unwrap_or_else(|e| e.into_inner());
        jar.retain(|_, issued| issued.elapsed() < COOKIE_TTL);
        while jar.len() >= MAX_COOKIES {
            let Some(oldest) = jar.iter().min_by_key(|(_, t)| **t).map(|(k, _)| k.clone()) else {
                break;
            };
            jar.remove(&oldest);
        }
        jar.insert(id.clone(), Instant::now());
    }
    // No `Secure` flag on purpose: the server is often reached over plain HTTP on a LAN or
    // Tailscale. Put it behind TLS for a public endpoint.
    let cookie = format!("{COOKIE}={id}; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}

async fn logout(State(s): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(id) = cookie_token(&headers) {
        s.cookies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }
    let cookie = format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}

// ---- health -----------------------------------------------------------

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

// ---- tracks -----------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct Track {
    id: String,
    title: String,
    artist: Option<String>,
    album: Option<String>,
    duration_ms: Option<i64>,
    preferred_file_id: String,
    genre: Option<String>,
}

pub(crate) const TRACK_COLS: &str =
    "id, title, artist, album, duration_ms, preferred_file_id, genre";

pub(crate) fn track_from_row(r: &rusqlite::Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: r.get::<_, i64>(0)?.to_string(),
        title: r.get(1)?,
        artist: r.get(2)?,
        album: r.get(3)?,
        duration_ms: r.get(4)?,
        preferred_file_id: r.get::<_, i64>(5)?.to_string(),
        genre: r.get(6)?,
    })
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn search_tracks(State(s): State<AppState>, Query(q): Query<SearchQuery>) -> Response {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
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
            .unwrap_or_else(|| "%".to_string());
    let conn = crate::db::lock(&s.db);
    let where_ = "search_text LIKE ?1 ESCAPE '\\'";
    let total: i64 = match conn.query_row(
        &format!("SELECT COUNT(*) FROM track WHERE {where_}"),
        [&pattern],
        |r| r.get(0),
    ) {
        Ok(n) => n,
        Err(e) => return internal(e),
    };
    let sql = format!(
        "SELECT {TRACK_COLS} FROM track WHERE {where_} ORDER BY artist, album, title, id LIMIT ?2 OFFSET ?3"
    );
    let items: rusqlite::Result<Vec<Track>> = conn.prepare(&sql).and_then(|mut st| {
        st.query_map(params![pattern, limit, offset], track_from_row)?
            .collect()
    });
    match items {
        Ok(items) => Json(json!({ "items": items, "total": total })).into_response(),
        Err(e) => internal(e),
    }
}

async fn get_track(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    let conn = crate::db::lock(&s.db);
    match conn
        .query_row(
            &format!("SELECT {TRACK_COLS} FROM track WHERE id=?1"),
            [id],
            track_from_row,
        )
        .optional()
    {
        Ok(Some(t)) => Json(t).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such track"),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct ManifestQuery {
    ids: Option<String>,
}

/// Most track ids one manifest request may ask for.
const MANIFEST_MAX: usize = 500;

/// For each asked track: its details plus the size and SHA-256 of the file that plays, so a client can
/// build offline pins (and later check that a pinned copy still equals the server's). Ids that name no
/// track come back in `missing`.
async fn tracks_manifest(State(s): State<AppState>, Query(q): Query<ManifestQuery>) -> Response {
    let mut ids = Vec::new();
    for part in q
        .ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|p| !p.is_empty())
    {
        match part.parse::<i64>() {
            Ok(id) => ids.push(id),
            Err(_) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    "ids must be comma-separated numbers",
                );
            }
        }
    }
    if ids.len() > MANIFEST_MAX {
        return err(StatusCode::BAD_REQUEST, "at most 500 ids per request");
    }
    let conn = crate::db::lock(&s.db);
    let mut items = Vec::new();
    let mut missing = Vec::new();
    for id in ids {
        let row = conn
            .query_row(
                "SELECT t.id, t.title, t.artist, t.album, t.duration_ms, t.preferred_file_id, t.genre, \
                        f.bytes, f.sha256 \
                 FROM track t JOIN file f ON f.id = t.preferred_file_id WHERE t.id=?1",
                [id],
                |r| Ok((track_from_row(r)?, r.get::<_, i64>(7)?, r.get::<_, String>(8)?)),
            )
            .optional();
        match row {
            Ok(Some((t, bytes, sha256))) => items.push(json!({
                "track": t, "file_id": t.preferred_file_id, "bytes": bytes, "sha256": sha256,
            })),
            Ok(None) => missing.push(id.to_string()),
            Err(e) => return internal(e),
        }
    }
    Json(json!({ "items": items, "missing": missing })).into_response()
}

// ---- audio ------------------------------------------------------------

#[derive(Deserialize)]
struct AudioQuery {
    codec: Option<String>,
    kbps: Option<i64>,
}

async fn audio(
    State(s): State<AppState>,
    Path(file_id): Path<i64>,
    Query(q): Query<AudioQuery>,
    req: Request,
) -> Response {
    let wanted = match (q.codec.as_deref(), q.kbps) {
        (None, None) => None,
        (None, Some(_)) => return err(StatusCode::BAD_REQUEST, "kbps needs a codec"),
        (Some(c), k) => {
            let Some(codec) = crate::transcode::Codec::parse(c) else {
                return err(StatusCode::BAD_REQUEST, "codec must be opus, mp3 or aac");
            };
            let kbps = k.unwrap_or(codec.default_kbps());
            if !(32..=320).contains(&kbps) {
                return err(StatusCode::BAD_REQUEST, "kbps must be 32 to 320");
            }
            Some((codec, kbps))
        }
    };
    let rel: Option<String> = {
        let conn = crate::db::lock(&s.db);
        match conn
            .query_row("SELECT path FROM file WHERE id=?1", [file_id], |r| r.get(0))
            .optional()
        {
            Ok(v) => v,
            Err(e) => return internal(e),
        }
    };
    let Some(rel) = rel else {
        return err(StatusCode::NOT_FOUND, "no such file");
    };
    if let Some((codec, kbps)) = wanted {
        if req.method() == axum::http::Method::HEAD {
            // headers only: never start ffmpeg for a HEAD request
            let mime = match codec {
                crate::transcode::Codec::Opus => "audio/ogg",
                crate::transcode::Codec::Mp3 => "audio/mpeg",
                crate::transcode::Codec::Aac => "audio/aac",
            };
            return ([
                (header::CONTENT_TYPE, mime),
                (header::ACCEPT_RANGES, "none"),
            ])
            .into_response();
        }
        let permit = match s.transcodes.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                return err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "too many transcodes running, try again",
                );
            }
        };
        return crate::transcode::stream(&s.ffmpeg_path(), &s.root.join(rel), codec, kbps, permit);
    }
    // ServeFile answers Range requests, ETag/Last-Modified and the content type.
    match ServeFile::new(s.root.join(rel)).oneshot(req).await {
        Ok(resp) => resp.into_response(),
        Err(e) => match e {},
    }
}

// ---- playlists --------------------------------------------------------

#[derive(Serialize)]
struct Playlist {
    id: String,
    name: String,
    extend: bool,
    track_ids: Vec<String>,
}

fn load_playlist(conn: &Connection, id: i64) -> rusqlite::Result<Option<Playlist>> {
    let head: Option<(String, bool)> = conn
        .query_row("SELECT name, extend FROM playlist WHERE id=?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    let Some((name, extend)) = head else {
        return Ok(None);
    };
    let track_ids = conn
        .prepare("SELECT track_id FROM playlist_item WHERE playlist_id=?1 ORDER BY position")?
        .query_map([id], |r| Ok(r.get::<_, i64>(0)?.to_string()))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(Playlist {
        id: id.to_string(),
        name,
        extend,
        track_ids,
    }))
}

async fn list_playlists(State(s): State<AppState>) -> Response {
    let conn = crate::db::lock(&s.db);
    let ids: rusqlite::Result<Vec<i64>> = conn
        .prepare("SELECT id FROM playlist ORDER BY id")
        .and_then(|mut st| st.query_map([], |r| r.get(0))?.collect());
    let ids = match ids {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let mut out = Vec::new();
    for id in ids {
        match load_playlist(&conn, id) {
            Ok(Some(p)) => out.push(p),
            Ok(None) => {}
            Err(e) => return internal(e),
        }
    }
    Json(out).into_response()
}

#[derive(Deserialize)]
struct NewPlaylist {
    name: String,
    #[serde(default)]
    extend: bool,
}

async fn create_playlist(State(s): State<AppState>, Json(body): Json<NewPlaylist>) -> Response {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "name must not be empty");
    }
    let conn = crate::db::lock(&s.db);
    if let Err(e) = conn.execute(
        "INSERT INTO playlist(name, extend) VALUES(?1, ?2)",
        params![name, body.extend],
    ) {
        return internal(e);
    }
    let id = conn.last_insert_rowid();
    match load_playlist(&conn, id) {
        Ok(Some(p)) => (StatusCode::CREATED, Json(p)).into_response(),
        Ok(None) => internal("playlist vanished"),
        Err(e) => internal(e),
    }
}

async fn get_playlist(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    let conn = crate::db::lock(&s.db);
    match load_playlist(&conn, id) {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such playlist"),
        Err(e) => internal(e),
    }
}

async fn delete_playlist(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    let conn = crate::db::lock(&s.db);
    match conn.execute("DELETE FROM playlist WHERE id=?1", [id]) {
        Ok(0) => err(StatusCode::NOT_FOUND, "no such playlist"),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct NewItem {
    track_id: String,
}

fn parse_track_id(s: &str) -> Option<i64> {
    s.parse().ok()
}

fn track_exists(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    conn.query_row("SELECT 1 FROM track WHERE id=?1", [id], |_| Ok(()))
        .optional()
        .map(|v| v.is_some())
}

async fn add_item(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<NewItem>,
) -> Response {
    let Some(track_id) = parse_track_id(&body.track_id) else {
        return err(StatusCode::BAD_REQUEST, "track_id is not a valid id");
    };
    let conn = crate::db::lock(&s.db);
    match load_playlist(&conn, id) {
        Ok(Some(_)) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, "no such playlist"),
        Err(e) => return internal(e),
    }
    match track_exists(&conn, track_id) {
        Ok(true) => {}
        Ok(false) => return err(StatusCode::NOT_FOUND, "no such track"),
        Err(e) => return internal(e),
    }
    match conn.execute(
        "INSERT INTO playlist_item(playlist_id, position, track_id)
         VALUES(?1, COALESCE((SELECT MAX(position)+1 FROM playlist_item WHERE playlist_id=?1), 0), ?2)",
        params![id, track_id],
    ) {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct ItemList {
    track_ids: Vec<String>,
}

/// Replace the whole item list in one go: this is how clients remove and reorder.
async fn set_items(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<ItemList>,
) -> Response {
    let mut ids = Vec::with_capacity(body.track_ids.len());
    for t in &body.track_ids {
        match parse_track_id(t) {
            Some(n) => ids.push(n),
            None => return err(StatusCode::BAD_REQUEST, "track_ids contains an invalid id"),
        }
    }
    let mut conn = crate::db::lock(&s.db);
    match load_playlist(&conn, id) {
        Ok(Some(_)) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, "no such playlist"),
        Err(e) => return internal(e),
    }
    for tid in &ids {
        match track_exists(&conn, *tid) {
            Ok(true) => {}
            Ok(false) => return err(StatusCode::NOT_FOUND, "no such track"),
            Err(e) => return internal(e),
        }
    }
    let res: rusqlite::Result<()> = (|| {
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM playlist_item WHERE playlist_id=?1", [id])?;
        for (pos, tid) in ids.iter().enumerate() {
            tx.execute(
                "INSERT INTO playlist_item(playlist_id, position, track_id) VALUES(?1, ?2, ?3)",
                params![id, pos as i64, tid],
            )?;
        }
        tx.commit()
    })();
    match res {
        Ok(()) => match load_playlist(&conn, id) {
            Ok(Some(p)) => Json(p).into_response(),
            Ok(None) => internal("playlist vanished"),
            Err(e) => internal(e),
        },
        Err(e) => internal(e),
    }
}

// ---- jobs -------------------------------------------------------------

#[derive(Serialize)]
struct Job {
    id: String,
    kind: String,
    state: String,
    attempts: i64,
    result: Option<serde_json::Value>,
    error: Option<String>,
}

fn load_job(conn: &Connection, id: i64) -> rusqlite::Result<Option<Job>> {
    conn.query_row(
        "SELECT kind, state, attempts, result, error FROM job WHERE id=?1",
        [id],
        |r| {
            let result: Option<String> = r.get(3)?;
            Ok(Job {
                id: id.to_string(),
                kind: r.get(0)?,
                state: r.get(1)?,
                attempts: r.get(2)?,
                result: result.and_then(|s| serde_json::from_str(&s).ok()),
                error: r.get(4)?,
            })
        },
    )
    .optional()
}

#[derive(Deserialize)]
struct NewJob {
    kind: String,
    /// For `tag`: only these tracks (default: every track with a missing artist or album)
    #[serde(default)]
    track_ids: Option<Vec<String>>,
    /// For `fetch`: `{"url": "…"}`
    #[serde(default)]
    input: Option<Value>,
}

async fn create_job(State(s): State<AppState>, Json(body): Json<NewJob>) -> Response {
    if body.track_ids.as_ref().is_some_and(|v| v.len() > 1000) {
        return err(
            StatusCode::BAD_REQUEST,
            "track_ids may name at most 1000 tracks",
        );
    }
    let kind = match body.kind.as_str() {
        k @ ("scan" | "check_stations" | "fingerprint" | "tag") => k.to_string(),
        "fetch" => {
            if !s.fetch.enabled {
                let why = s.fetch.disabled_reason.clone().unwrap_or_default();
                return err(StatusCode::FORBIDDEN, &format!("fetch is off: {why}"));
            }
            if s.ytdlp_path().is_none() {
                return err(
                    StatusCode::CONFLICT,
                    "fetch needs yt-dlp, which is not installed: install it from the Programs panel (GET /v1/tools) or set MUSEQ_FETCH_YTDLP",
                );
            }
            "fetch".to_string()
        }
        _ => return err(StatusCode::BAD_REQUEST, "unknown job kind"),
    };
    let fetch_source = if kind == "fetch" {
        let url = body
            .input
            .as_ref()
            .and_then(|i| i["url"].as_str())
            .unwrap_or("");
        match crate::fetch::parse_source(url) {
            Ok(src) => Some(src),
            Err(msg) => return err(StatusCode::BAD_REQUEST, &msg),
        }
    } else {
        None
    };
    let id = {
        // Check and insert under one lock, so two requests can't both start the same kind of job.
        let conn = crate::db::lock(&s.db);
        let busy: rusqlite::Result<bool> = conn
            .query_row(
                // scan, fingerprint and fetch share the disk and the CPU: never run them together
                "SELECT 1 FROM job WHERE state IN ('queued','running') AND (kind=?1
                   OR (?1 IN ('scan','fingerprint','fetch') AND kind IN ('scan','fingerprint','fetch')))",
                [&kind],
                |_| Ok(()),
            )
            .optional()
            .map(|v| v.is_some());
        match busy {
            Ok(false) => {}
            Ok(true) => return err(StatusCode::CONFLICT, "that job is already running"),
            Err(e) => return internal(e),
        }
        if let Err(e) = conn.execute("INSERT INTO job(kind, state) VALUES(?1, 'queued')", [&kind]) {
            return internal(e);
        }
        conn.last_insert_rowid()
    };
    let db = s.db.clone();
    let task_db = db.clone();
    let root = s.root.clone();
    let allow_private = s.allow_private_stations;
    let app = s.clone();
    let only: Option<Vec<i64>> = body
        .track_ids
        .as_ref()
        .map(|v| v.iter().filter_map(|x| x.parse().ok()).collect());
    let work = tokio::spawn(async move {
        let _ = crate::db::lock(&task_db).execute(
            "UPDATE job SET state='running', attempts=attempts+1 WHERE id=?1",
            [id],
        );
        let result = if kind == "scan" {
            let d = task_db.clone();
            let stats = tokio::task::spawn_blocking(move || scan::scan(&d, &root))
                .await
                .map_err(|_| "scan crashed".to_string())?;
            serde_json::to_string(&stats).map_err(|_| "cannot encode result".to_string())?
        } else if kind == "fingerprint" {
            crate::fingerprint::run_job(task_db.clone(), root.clone())
                .await
                .to_string()
        } else if kind == "tag" {
            crate::metadata::run_tag_job(app.clone(), only.clone())
                .await
                .to_string()
        } else if let Some(source) = fetch_source {
            crate::fetch::run(app.clone(), source).await?.to_string()
        } else {
            crate::stations::check_all(task_db.clone(), allow_private)
                .await
                .to_string()
        };
        let _ = crate::db::lock(&task_db).execute(
            "UPDATE job SET state='done', result=?2 WHERE id=?1",
            params![id, result],
        );
        Ok::<(), String>(())
    });
    // If the job fails or panics, mark it failed so later runs aren't blocked by a ghost.
    tokio::spawn(async move {
        let failure = match work.await {
            Ok(Ok(())) => None,
            Ok(Err(msg)) => Some(msg),
            Err(_) => Some("job crashed".to_string()),
        };
        if let Some(msg) = failure {
            let _ = crate::db::lock(&db).execute(
                "UPDATE job SET state='failed', error=?2 WHERE id=?1",
                params![id, msg],
            );
        }
    });
    let conn = crate::db::lock(&s.db);
    match load_job(&conn, id) {
        Ok(Some(j)) => (StatusCode::ACCEPTED, Json(j)).into_response(),
        Ok(None) => internal("job vanished"),
        Err(e) => internal(e),
    }
}

// ---- tools ------------------------------------------------------------

async fn list_tools(State(s): State<AppState>) -> Response {
    Json(s.tools.status()).into_response()
}

#[derive(Deserialize)]
struct InstallBody {
    #[serde(default)]
    confirm: String,
}

/// Starts a download of a pinned tool. Needs the exact confirmation phrase from `GET /v1/tools`.
async fn install_tool(
    State(s): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<InstallBody>>,
) -> Response {
    let confirm = body.map(|b| b.0.confirm).unwrap_or_default();
    match s.tools.install(&name, &confirm) {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!(s.tools.status().into_iter().find(|t| t.name == name))),
        )
            .into_response(),
        Err((code, msg)) => err(
            StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
            &msg,
        ),
    }
}

async fn remove_tool(State(s): State<AppState>, Path(name): Path<String>) -> Response {
    match s.tools.uninstall(&name) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err((code, msg)) => err(
            StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
            &msg,
        ),
    }
}

async fn get_job(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    let conn = crate::db::lock(&s.db);
    match load_job(&conn, id) {
        Ok(Some(j)) => Json(j).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such job"),
        Err(e) => internal(e),
    }
}

async fn fetch_status(State(s): State<AppState>) -> Json<Value> {
    Json(crate::fetch::status(&s))
}
