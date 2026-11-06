use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::api::{AppState, err, internal};
use crate::db;

pub const MAX_SKIN_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize)]
struct Skin {
    id: String,
    name: String,
}

/// Keep a display name only: no paths, no odd characters, no `.wsz` suffix.
fn clean_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let base = base
        .strip_suffix(".wsz")
        .or_else(|| base.strip_suffix(".WSZ"))
        .unwrap_or(base);
    let cleaned: String = base
        .chars()
        .filter(|c| c.is_alphanumeric() || " ._-()".contains(*c))
        .take(80)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "skin".into()
    } else {
        cleaned
    }
}

pub async fn list_skins(State(s): State<AppState>) -> Response {
    let conn = db::lock(&s.db);
    let rows: rusqlite::Result<Vec<Skin>> = conn
        .prepare("SELECT id, name FROM skin ORDER BY name")
        .and_then(|mut st| {
            st.query_map([], |r| {
                Ok(Skin {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?
            .collect()
        });
    match rows {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct UploadQuery {
    name: Option<String>,
}

/// The raw `.wsz` is stored as-is and never unpacked here: the browser (Webamp) parses it.
pub async fn upload_skin(
    State(s): State<AppState>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Response {
    if body.len() > MAX_SKIN_BYTES {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "skin is too large");
    }
    if !body.starts_with(b"PK\x03\x04") {
        return err(StatusCode::BAD_REQUEST, "not a zip file (.wsz)");
    }
    let digest: String = Sha256::digest(&body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let id = digest[..16].to_string();
    let name = clean_name(q.name.as_deref().unwrap_or("skin"));
    if let Err(e) = std::fs::create_dir_all(&*s.skins_dir) {
        return internal(e);
    }
    if let Err(e) = std::fs::write(s.skins_dir.join(format!("{id}.wsz")), &body) {
        return internal(e);
    }
    let conn = db::lock(&s.db);
    if let Err(e) = conn.execute(
        "INSERT INTO skin(id, name, bytes) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET name=excluded.name",
        params![id, name, body.len() as i64],
    ) {
        return internal(e);
    }
    (StatusCode::CREATED, Json(Skin { id, name })).into_response()
}

pub async fn get_skin(State(s): State<AppState>, Path(id): Path<String>, req: Request) -> Response {
    // ids are 16 hex characters we generated: anything else can't name a file
    if id.len() != 16 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return err(StatusCode::NOT_FOUND, "no such skin");
    }
    let known = db::lock(&s.db)
        .query_row("SELECT 1 FROM skin WHERE id=?1", [&id], |_| Ok(()))
        .optional();
    match known {
        Ok(Some(())) => match ServeFile::new(s.skins_dir.join(format!("{id}.wsz")))
            .oneshot(req)
            .await
        {
            Ok(resp) => resp.into_response(),
            Err(e) => match e {},
        },
        Ok(None) => err(StatusCode::NOT_FOUND, "no such skin"),
        Err(e) => internal(e),
    }
}
