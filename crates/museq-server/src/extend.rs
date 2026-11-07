// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
//! Playlist extension: when a playlist the owner built reaches its end, add tracks in the same
//! style by artists that are not in it yet. The pool is the local library only: this never calls
//! the network and never downloads anything.
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::api::{AppState, TRACK_COLS, Track, err, internal, track_from_row};
use crate::{db, text};

const DEFAULT_COUNT: i64 = 3;
const MAX_COUNT: i64 = 20;

#[derive(Deserialize, Default)]
pub struct ExtendBody {
    count: Option<i64>,
}

pub struct Row {
    pub id: i64,
    pub artist: Option<String>,
    pub genre: Option<String>,
}

/// Pick up to `count` tracks: genre shared with the playlist, an artist not in the playlist,
/// each new artist at most once. `rotate` shifts the choice so repeated calls differ.
pub fn pick(playlist: &[Row], pool: &[Row], count: usize, rotate: usize) -> (Vec<i64>, usize) {
    let norm = |s: &Option<String>| {
        s.as_deref()
            .map(text::fold)
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    let in_list: HashSet<i64> = playlist.iter().map(|r| r.id).collect();
    let artists: HashSet<String> = playlist.iter().filter_map(|r| norm(&r.artist)).collect();
    let mut genre_weight: HashMap<String, usize> = HashMap::new();
    for r in playlist {
        if let Some(g) = norm(&r.genre) {
            *genre_weight.entry(g).or_default() += 1;
        }
    }
    // the first track of each new artist, in a stable order (heavier genres first)
    let mut by_artist: BTreeMap<(usize, String), i64> = BTreeMap::new();
    for r in pool {
        let (Some(a), Some(g)) = (norm(&r.artist), norm(&r.genre)) else {
            continue;
        };
        let Some(w) = genre_weight.get(&g) else {
            continue;
        };
        if in_list.contains(&r.id) || artists.contains(&a) {
            continue;
        }
        by_artist.entry((usize::MAX - w, a)).or_insert(r.id);
    }
    let ids: Vec<i64> = by_artist.values().copied().collect();
    let total = ids.len();
    if total == 0 {
        return (Vec::new(), 0);
    }
    let start = rotate % total;
    let picked = (0..total.min(count))
        .map(|i| ids[(start + i) % total])
        .collect();
    (picked, total)
}

pub async fn extend_playlist(
    State(s): State<AppState>,
    Path(id): Path<i64>,
    body: Option<Json<ExtendBody>>,
) -> Response {
    let count = body.and_then(|b| b.0.count).unwrap_or(DEFAULT_COUNT);
    if !(1..=MAX_COUNT).contains(&count) {
        return err(
            StatusCode::BAD_REQUEST,
            &format!("count must be between 1 and {MAX_COUNT}"),
        );
    }
    let mut conn = db::lock(&s.db);
    let exists = conn
        .query_row("SELECT name FROM playlist WHERE id=?1", [id], |r| {
            r.get::<_, String>(0)
        })
        .optional();
    match exists {
        Ok(Some(_)) => {}
        Ok(None) => return err(StatusCode::NOT_FOUND, "no such playlist"),
        Err(e) => return internal(e),
    }
    let load = |sql: &str, arg: Option<i64>| -> rusqlite::Result<Vec<Row>> {
        let map = |r: &rusqlite::Row| {
            Ok(Row {
                id: r.get(0)?,
                artist: r.get(1)?,
                genre: r.get(2)?,
            })
        };
        let mut st = conn.prepare(sql)?;
        match arg {
            Some(a) => st.query_map([a], map)?.collect(),
            None => st.query_map([], map)?.collect(),
        }
    };
    let playlist = match load(
        "SELECT t.id, t.artist, t.genre FROM playlist_item i JOIN track t ON t.id = i.track_id WHERE i.playlist_id = ?1",
        Some(id),
    ) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    if playlist.is_empty() {
        return err(
            StatusCode::CONFLICT,
            "the playlist is empty: nothing to base the extension on",
        );
    }
    if playlist
        .iter()
        .all(|r| r.genre.as_deref().is_none_or(|g| g.trim().is_empty()))
    {
        return Json(json!({"added": [], "pool_empty": true, "reason": "none of the playlist's tracks has a genre tag, so there is no style to match"})).into_response();
    }
    let pool = match load(
        "SELECT id, artist, genre FROM track WHERE genre IS NOT NULL AND artist IS NOT NULL",
        None,
    ) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let (ids, available) = pick(&playlist, &pool, count as usize, playlist.len());
    if ids.is_empty() {
        return Json(json!({"added": [], "pool_empty": true, "reason": "no track in the library has this style from an artist that is not already in the playlist"})).into_response();
    }
    let res: rusqlite::Result<()> = (|| {
        let tx = conn.transaction()?;
        for tid in &ids {
            tx.execute(
                "INSERT INTO playlist_item(playlist_id, position, track_id)
                 VALUES(?1, COALESCE((SELECT MAX(position)+1 FROM playlist_item WHERE playlist_id=?1), 0), ?2)",
                params![id, tid],
            )?;
        }
        tx.commit()
    })();
    if let Err(e) = res {
        return internal(e);
    }
    let added: rusqlite::Result<Vec<Track>> = ids
        .iter()
        .map(|tid| {
            conn.query_row(
                &format!("SELECT {TRACK_COLS} FROM track WHERE id=?1"),
                [tid],
                track_from_row,
            )
        })
        .collect();
    match added {
        Ok(added) => {
            Json(json!({"added": added, "pool_empty": false, "available_artists": available}))
                .into_response()
        }
        Err(e) => internal(e),
    }
}
