use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::scan;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub root: Arc<PathBuf>,
    pub token: Arc<String>,
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn internal<E: std::fmt::Display>(e: E) -> Response {
    eprintln!("internal error: {e}");
    err(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/tracks", get(search_tracks))
        .route("/v1/tracks/{id}", get(get_track))
        .route("/v1/audio/{file_id}", get(audio))
        .route("/v1/playlists", get(list_playlists).post(create_playlist))
        .route(
            "/v1/playlists/{id}",
            get(get_playlist).delete(delete_playlist),
        )
        .route("/v1/playlists/{id}/items", post(add_item).put(set_items))
        .route("/v1/jobs", post(create_job))
        .route("/v1/jobs/{id}", get(get_job))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));
    Router::new()
        .route("/v1/health", get(health))
        .merge(protected)
        .with_state(state)
}

// ---- auth -------------------------------------------------------------

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn auth(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| ct_eq(t.as_bytes(), s.token.as_bytes()))
        .unwrap_or(false);
    if ok {
        next.run(req).await
    } else {
        err(StatusCode::UNAUTHORIZED, "missing or invalid token")
    }
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
struct Track {
    id: String,
    title: String,
    artist: Option<String>,
    album: Option<String>,
    duration_ms: Option<i64>,
    preferred_file_id: String,
}

const TRACK_COLS: &str = "id, title, artist, album, duration_ms, preferred_file_id";

fn track_from_row(r: &rusqlite::Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: r.get::<_, i64>(0)?.to_string(),
        title: r.get(1)?,
        artist: r.get(2)?,
        album: r.get(3)?,
        duration_ms: r.get(4)?,
        preferred_file_id: r.get::<_, i64>(5)?.to_string(),
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
                format!(
                    "%{}%",
                    t.replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )
            })
            .unwrap_or_else(|| "%".to_string());
    let conn = s.db.lock().unwrap();
    let where_ =
        "(title LIKE ?1 ESCAPE '\\' OR artist LIKE ?1 ESCAPE '\\' OR album LIKE ?1 ESCAPE '\\')";
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
    let conn = s.db.lock().unwrap();
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
    if q.codec.is_some() || q.kbps.is_some() {
        return err(
            StatusCode::NOT_IMPLEMENTED,
            "transcoding is not available yet",
        );
    }
    let rel: Option<String> = {
        let conn = s.db.lock().unwrap();
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
    let conn = s.db.lock().unwrap();
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
    let conn = s.db.lock().unwrap();
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
    let conn = s.db.lock().unwrap();
    match load_playlist(&conn, id) {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such playlist"),
        Err(e) => internal(e),
    }
}

async fn delete_playlist(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    let conn = s.db.lock().unwrap();
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
    let conn = s.db.lock().unwrap();
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
    let mut conn = s.db.lock().unwrap();
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
}

async fn create_job(State(s): State<AppState>, Json(body): Json<NewJob>) -> Response {
    match body.kind.as_str() {
        "scan" => {}
        "fetch" => {
            return err(
                StatusCode::FORBIDDEN,
                "fetch is disabled in the configuration",
            );
        }
        "tag" | "fingerprint" => {
            return err(
                StatusCode::NOT_IMPLEMENTED,
                "this job kind is not implemented yet",
            );
        }
        _ => return err(StatusCode::BAD_REQUEST, "unknown job kind"),
    }
    let id = {
        let conn = s.db.lock().unwrap();
        let busy: rusqlite::Result<bool> = conn
            .query_row(
                "SELECT 1 FROM job WHERE kind='scan' AND state IN ('queued','running')",
                [],
                |_| Ok(()),
            )
            .optional()
            .map(|v| v.is_some());
        match busy {
            Ok(false) => {}
            Ok(true) => return err(StatusCode::CONFLICT, "a scan is already running"),
            Err(e) => return internal(e),
        }
        if let Err(e) = conn.execute("INSERT INTO job(kind, state) VALUES('scan', 'queued')", []) {
            return internal(e);
        }
        conn.last_insert_rowid()
    };
    let (db, root) = (s.db.clone(), s.root.clone());
    tokio::task::spawn_blocking(move || {
        let _ = db.lock().unwrap().execute(
            "UPDATE job SET state='running', attempts=attempts+1 WHERE id=?1",
            [id],
        );
        let stats = scan::scan(&db, &root);
        let result = serde_json::to_string(&stats).unwrap_or_default();
        let _ = db.lock().unwrap().execute(
            "UPDATE job SET state='done', result=?2 WHERE id=?1",
            params![id, result],
        );
    });
    let conn = s.db.lock().unwrap();
    match load_job(&conn, id) {
        Ok(Some(j)) => (StatusCode::ACCEPTED, Json(j)).into_response(),
        Ok(None) => internal("job vanished"),
        Err(e) => internal(e),
    }
}

async fn get_job(State(s): State<AppState>, Path(id): Path<i64>) -> Response {
    let conn = s.db.lock().unwrap();
    match load_job(&conn, id) {
        Ok(Some(j)) => Json(j).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no such job"),
        Err(e) => internal(e),
    }
}
