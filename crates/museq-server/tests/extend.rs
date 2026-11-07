// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use museq_server::extend::{Row, pick};
use museq_server::{api, db};
use serde_json::{Value, json};
use std::path::PathBuf;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789";

fn row(id: i64, artist: &str, genre: &str) -> Row {
    Row {
        id,
        artist: Some(artist.into()),
        genre: Some(genre.into()),
    }
}

#[test]
fn picks_new_artists_in_the_same_style_only() {
    let playlist = [row(1, "Alpha", "Rock"), row(2, "Beta", "rock")];
    let pool = [
        row(1, "Alpha", "Rock"),
        row(3, "ALPHA", "Rock"), // an artist already in the playlist (case differs)
        row(4, "Gamma", "Rock"),
        row(5, "Gamma", "Rock"), // the same new artist again: one track per artist
        row(6, "Delta", "Jazz"), // another style
        row(7, "Epsilon", "ROCK"),
        row(8, "Zeta", "Rock"),
    ];
    let (ids, available) = pick(&playlist, &pool, 5, 0);
    assert_eq!(available, 3);
    let mut got = ids.clone();
    got.sort();
    assert_eq!(
        got,
        vec![4, 7, 8],
        "Gamma once, Epsilon, Zeta; never Alpha, Beta or Delta"
    );
    let (two, _) = pick(&playlist, &pool, 2, 0);
    assert_eq!(two.len(), 2);
}

#[test]
fn accents_and_case_do_not_make_a_new_artist() {
    let playlist = [row(1, "Beyoncé", "Pop")];
    let pool = [
        row(2, "BEYONCE", "pop"),
        row(3, "Σταύρος", "Pop"),
        row(4, "ΣΤΑΥΡΟΣ", "pop"),
    ];
    let (ids, available) = pick(&playlist, &pool, 5, 0);
    assert_eq!(
        (ids.len(), available),
        (1, 1),
        "BEYONCE is the same artist, the two Greek spellings are one new artist: {ids:?}"
    );
}

#[test]
fn repeated_extensions_rotate_through_the_pool() {
    let playlist = [row(1, "A", "Rock")];
    let pool: Vec<Row> = (2..=6)
        .map(|i| row(i, &format!("Artist{i}"), "Rock"))
        .collect();
    let first = pick(&playlist, &pool, 2, 1).0;
    let second = pick(&playlist, &pool, 2, 3).0;
    assert_ne!(first, second);
}

#[test]
fn heavier_styles_come_first() {
    let playlist = [
        row(1, "A", "Jazz"),
        row(2, "B", "Rock"),
        row(3, "C", "Rock"),
    ];
    let pool = [row(10, "Jazzer", "Jazz"), row(11, "Rocker", "Rock")];
    assert_eq!(
        pick(&playlist, &pool, 1, 0).0,
        vec![11],
        "the playlist is mostly Rock"
    );
}

fn state() -> api::AppState {
    api::AppState::new(
        db::open_in_memory().unwrap(),
        PathBuf::from("."),
        TOKEN.into(),
        PathBuf::from("."),
        None,
    )
}

fn add_track(st: &api::AppState, title: &str, artist: Option<&str>, genre: Option<&str>) -> i64 {
    let c = db::lock(&st.db);
    c.execute(
        "INSERT INTO file(sha256, path, bytes, mtime) VALUES(?1, ?2, 1, 1)",
        rusqlite::params![title, format!("{title}.mp3")],
    )
    .unwrap();
    let fid = c.last_insert_rowid();
    c.execute(
        "INSERT INTO track(title, artist, genre, preferred_file_id, search_text) VALUES(?1,?2,?3,?4,?1)",
        rusqlite::params![title, artist, genre, fid],
    )
    .unwrap();
    let tid = c.last_insert_rowid();
    c.execute(
        "UPDATE file SET track_id=?1 WHERE id=?2",
        rusqlite::params![tid, fid],
    )
    .unwrap();
    tid
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let (p, body) = resp.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (
        p.status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn extending_a_playlist_through_the_api() {
    let st = state();
    let a = add_track(&st, "A1", Some("Alpha"), Some("Rock"));
    let b = add_track(&st, "B1", Some("Beta"), Some("Rock"));
    let _same_artist = add_track(&st, "A2", Some("Alpha"), Some("Rock"));
    let _jazz = add_track(&st, "J1", Some("Jazzer"), Some("Jazz"));
    let _no_artist = add_track(&st, "N1", None, Some("Rock"));
    let _no_genre = add_track(&st, "G1", Some("Plain"), None);
    let c = add_track(&st, "C1", Some("Gamma"), Some("Rock"));
    let d = add_track(&st, "D1", Some("Delta"), Some("Rock"));
    let app = api::router(st);

    let (_, pl) = call(
        &app,
        Method::POST,
        "/v1/playlists",
        Some(json!({"name": "mine", "extend": true})),
    )
    .await;
    let uri = format!("/v1/playlists/{}", pl["id"].as_str().unwrap());

    // empty playlist: nothing to base it on
    assert_eq!(
        call(&app, Method::POST, &format!("{uri}/extend"), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, Method::POST, "/v1/playlists/99999/extend", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    call(
        &app,
        Method::PUT,
        &format!("{uri}/items"),
        Some(json!({"track_ids": [a.to_string(), b.to_string()]})),
    )
    .await;
    for bad in [0, 21, -1] {
        assert_eq!(
            call(
                &app,
                Method::POST,
                &format!("{uri}/extend"),
                Some(json!({"count": bad}))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "count {bad}"
        );
    }

    // the pool holds exactly two new Rock artists: Gamma and Delta
    let (s, r) = call(
        &app,
        Method::POST,
        &format!("{uri}/extend"),
        Some(json!({"count": 5})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let mut added: Vec<String> = r["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    added.sort();
    let mut want = vec![c.to_string(), d.to_string()];
    want.sort();
    assert_eq!(added, want, "{r}");
    assert_eq!(r["pool_empty"], false);
    let (_, now) = call(&app, Method::GET, &uri, None).await;
    assert_eq!(
        now["track_ids"].as_array().unwrap().len(),
        4,
        "the playlist grew and kept its order"
    );
    assert_eq!(&now["track_ids"][0], &json!(a.to_string()));

    // a second time there is nothing left, and it says so instead of guessing
    let (s, r) = call(&app, Method::POST, &format!("{uri}/extend"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (
            r["added"].as_array().unwrap().len(),
            r["pool_empty"].clone()
        ),
        (0, json!(true)),
        "{r}"
    );
    assert!(
        r["reason"]
            .as_str()
            .unwrap()
            .contains("not already in the playlist")
    );
    assert_eq!(
        call(&app, Method::GET, &uri, None).await.1["track_ids"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[tokio::test]
async fn a_playlist_without_any_genre_cannot_be_matched() {
    let st = state();
    let a = add_track(&st, "A1", Some("Alpha"), None);
    add_track(&st, "B1", Some("Beta"), Some("Rock"));
    let app = api::router(st);
    let (_, pl) = call(
        &app,
        Method::POST,
        "/v1/playlists",
        Some(json!({"name": "x"})),
    )
    .await;
    let uri = format!("/v1/playlists/{}", pl["id"].as_str().unwrap());
    call(
        &app,
        Method::POST,
        &format!("{uri}/items"),
        Some(json!({"track_id": a.to_string()})),
    )
    .await;
    let (s, r) = call(&app, Method::POST, &format!("{uri}/extend"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(r["pool_empty"], true);
    assert!(r["reason"].as_str().unwrap().contains("genre"), "{r}");
}
