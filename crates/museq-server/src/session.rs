use axum::{
    Json,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::{AppState, TRACK_COLS, err, internal, track_from_row};
use crate::db;

/// The one global playback session. The browser that is playing reports into it;
/// widgets and other clients read it and send commands to the reporting client.
#[derive(Clone, Debug, Serialize)]
pub struct Session {
    pub state: String,
    pub track_id: Option<i64>,
    pub position_ms: i64,
    pub volume: f64,
    /// Id of the client that last reported; commands are addressed to it.
    pub client_id: Option<String>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            state: "stopped".into(),
            track_id: None,
            position_ms: 0,
            volume: 1.0,
            client_id: None,
        }
    }
}

fn session_json(s: &AppState) -> Value {
    let sess = s.session.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let track = sess.track_id.and_then(|id| {
        db::lock(&s.db)
            .query_row(
                &format!("SELECT {TRACK_COLS} FROM track WHERE id=?1"),
                [id],
                track_from_row,
            )
            .optional()
            .ok()
            .flatten()
    });
    json!({
        "state": sess.state,
        "track": track,
        "position_ms": sess.position_ms,
        "volume": sess.volume,
        "client_id": sess.client_id,
    })
}

pub async fn get_session(State(s): State<AppState>) -> Response {
    Json(session_json(&s)).into_response()
}

#[derive(Deserialize)]
pub struct SessionBody {
    action: String,
    track_id: Option<String>,
    volume: Option<f64>,
    position_ms: Option<i64>,
    state: Option<String>,
    client_id: Option<String>,
}

pub async fn post_session(State(s): State<AppState>, Json(b): Json<SessionBody>) -> Response {
    match b.action.as_str() {
        // The playing client says what it is doing.
        "report" => {
            let Some(client_id) = b.client_id.filter(|c| !c.is_empty() && c.len() <= 64) else {
                return err(StatusCode::BAD_REQUEST, "report needs a client_id");
            };
            if let Some(st) = &b.state
                && !matches!(st.as_str(), "stopped" | "playing" | "paused")
            {
                return err(
                    StatusCode::BAD_REQUEST,
                    "state must be stopped, playing or paused",
                );
            }
            if let Some(v) = b.volume
                && !(0.0..=1.0).contains(&v)
            {
                return err(StatusCode::BAD_REQUEST, "volume must be between 0 and 1");
            }
            let track_id = match &b.track_id {
                None => None,
                Some(t) => {
                    let Ok(id) = t.parse::<i64>() else {
                        return err(StatusCode::BAD_REQUEST, "track_id is not a valid id");
                    };
                    match db::lock(&s.db)
                        .query_row("SELECT 1 FROM track WHERE id=?1", [id], |_| Ok(()))
                        .optional()
                    {
                        Ok(Some(())) => Some(id),
                        Ok(None) => return err(StatusCode::NOT_FOUND, "no such track"),
                        Err(e) => return internal(e),
                    }
                }
            };
            {
                let mut sess = s.session.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(st) = b.state {
                    sess.state = st;
                }
                if b.track_id.is_some() {
                    sess.track_id = track_id;
                }
                if let Some(p) = b.position_ms {
                    sess.position_ms = p.max(0);
                }
                if let Some(v) = b.volume {
                    sess.volume = v;
                }
                sess.client_id = Some(client_id);
            }
            let snap = session_json(&s);
            let _ = s
                .events
                .send(json!({"type": "session", "session": snap}).to_string());
            Json(snap).into_response()
        }
        // Commands from widgets or other clients, forwarded to the playing client.
        "play" | "pause" | "next" | "previous" | "volume" => {
            let target = s
                .session
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .client_id
                .clone();
            if target.is_none() || s.events.receiver_count() == 0 {
                return err(
                    StatusCode::CONFLICT,
                    "no player is connected; start playback in a client first",
                );
            }
            if b.action == "volume" && !b.volume.is_some_and(|v| (0.0..=1.0).contains(&v)) {
                return err(StatusCode::BAD_REQUEST, "volume must be between 0 and 1");
            }
            let _ = s.events.send(
                json!({"type": "command", "action": b.action, "track_id": b.track_id, "volume": b.volume, "target": target})
                    .to_string(),
            );
            (StatusCode::ACCEPTED, Json(session_json(&s))).into_response()
        }
        _ => err(StatusCode::BAD_REQUEST, "unknown action"),
    }
}

/// A browser WebSocket must come from the page we serve: reject a foreign Origin.
fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true; // not a browser (curl, widget)
    };
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    origin.split_once("://").map(|(_, h)| h) == Some(host)
}

pub async fn events(
    State(s): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !origin_ok(&headers) {
        return err(StatusCode::FORBIDDEN, "cross-origin WebSocket refused");
    }
    let rx = s.events.subscribe();
    let first = json!({"type": "session", "session": session_json(&s)}).to_string();
    ws.on_upgrade(move |socket| pump(socket, rx, first))
}

async fn pump(
    mut socket: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<String>,
    first: String,
) {
    if socket.send(Message::Text(first.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Ok(text) => if socket.send(Message::Text(text.into())).await.is_err() { break },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                _ => {}
            },
        }
    }
}
