// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use museq_server::radio::{Cand, pick_next};
use museq_server::{api, db};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789";

fn cand(id: i64, artist: &str) -> Cand {
    Cand {
        file_id: id,
        artist: Some(artist.into()),
    }
}

#[test]
fn rotation_avoids_recent_files_and_the_last_artist() {
    let cands = [
        cand(1, "A"),
        cand(2, "A"),
        cand(3, "B"),
        cand(4, "C"),
        cand(5, "C"),
    ];
    let recent: VecDeque<Cand> = VecDeque::from([cand(1, "A")]);
    for roll in 0..50u64 {
        let n = pick_next(&cands, &recent, 3, roll).unwrap();
        assert_ne!(n.file_id, 1, "just played");
        assert_ne!(n.artist.as_deref(), Some("A"), "same artist twice in a row");
    }
    // no other artist and nothing unplayed: repeats beat silence
    let only = [cand(1, "A")];
    assert_eq!(
        pick_next(&only, &VecDeque::from([cand(1, "A")]), 5, 7)
            .unwrap()
            .file_id,
        1
    );
    // a long run never plays a file again inside the gap while others exist
    let many: Vec<Cand> = (1..=10).map(|i| cand(i, &format!("artist{i}"))).collect();
    let mut recent = VecDeque::new();
    for roll in 0..200u64 {
        let n = pick_next(&many, &recent, 5, roll.wrapping_mul(2654435761)).unwrap();
        assert!(
            !recent.iter().rev().take(5).any(|c| c.file_id == n.file_id),
            "repeat inside the gap"
        );
        recent.push_back(n);
    }
    assert!(pick_next(&[], &VecDeque::new(), 5, 1).is_none());
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

async fn app_with_library() -> Router {
    let lib = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dups");
    let state = api::AppState::new(
        db::open_in_memory().unwrap(),
        lib,
        TOKEN.into(),
        PathBuf::from("/nonexistent"),
        None,
    );
    let app = api::router(state);
    let (_, job) = call(
        &app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "scan"})),
    )
    .await;
    let uri = format!("/v1/jobs/{}", job["id"].as_str().unwrap());
    for _ in 0..200 {
        if call(&app, Method::GET, &uri, None).await.1["state"] == "done" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    app
}

#[tokio::test]
async fn stations_are_validated_listed_and_deleted() {
    let app = app_with_library().await;
    for bad in [
        json!({"name": ""}),
        json!({"name": "x", "codec": "wav"}),
        json!({"name": "x", "kbps": 5}),
        json!({"name": "x", "artist_gap": 999}),
    ] {
        assert_eq!(
            call(&app, Method::POST, "/v1/radios", Some(bad)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let (s, r) = call(
        &app,
        Method::POST,
        "/v1/radios",
        Some(json!({"name": "Night Mix", "genre": " Rock "})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_eq!(
        (
            r["codec"].as_str(),
            r["kbps"].as_i64(),
            r["listeners"].as_i64()
        ),
        (Some("mp3"), Some(192), Some(0))
    );
    let id = r["id"].as_str().unwrap().to_string();
    assert_eq!(
        call(&app, Method::GET, "/v1/radios", None)
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        call(&app, Method::DELETE, &format!("/v1/radios/{id}"), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, Method::DELETE, &format!("/v1/radios/{id}"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let req = Request::builder()
        .uri("/v1/radios/999/stream")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn listeners_share_one_encoder_that_stops_when_they_leave() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        assert!(std::env::var_os("CI").is_none(), "ffmpeg is required in CI");
        return;
    }
    let app = app_with_library().await;
    let (_, r) = call(
        &app,
        Method::POST,
        "/v1/radios",
        Some(json!({"name": "Test FM", "kbps": 64})),
    )
    .await;
    let id = r["id"].as_str().unwrap().to_string();
    let open = || {
        let req = Request::builder()
            .uri(format!("/v1/radios/{id}/stream"))
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap();
        app.clone().oneshot(req)
    };
    let one = open().await.unwrap();
    assert_eq!(one.status(), StatusCode::OK);
    assert_eq!(one.headers()[header::CONTENT_TYPE], "audio/mpeg");
    assert_eq!(one.headers()["icy-name"], "Test FM");
    let two = open().await.unwrap();
    let (_, list) = call(&app, Method::GET, "/v1/radios", None).await;
    assert_eq!(
        list[0]["listeners"], 2,
        "two listeners on one station: {list}"
    );
    let mut on_air = false;
    for _ in 0..40 {
        let (_, l) = call(&app, Method::GET, "/v1/radios", None).await;
        if !l[0]["now_track_id"].is_null() {
            on_air = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(on_air, "a track is reported on air");
    // no token, no stream
    let anon = Request::builder()
        .uri(format!("/v1/radios/{id}/stream"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(anon).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    // both hear audio
    for resp in [one, two] {
        let mut body = resp.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(10), body.frame())
            .await
            .expect("audio arrives")
            .unwrap()
            .unwrap();
        let data = frame.into_data().unwrap();
        assert!(!data.is_empty());
    }
    // (both dropped) the station goes quiet and is removed
    let mut quiet = false;
    for _ in 0..60 {
        let (_, list) = call(&app, Method::GET, "/v1/radios", None).await;
        if list[0]["listeners"] == 0 && list[0]["now_track_id"].is_null() {
            quiet = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        quiet,
        "the encoder did not stop after the last listener left"
    );
}

#[tokio::test]
async fn deleting_a_station_ends_its_stream_at_once() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        assert!(std::env::var_os("CI").is_none(), "ffmpeg is required in CI");
        return;
    }
    let app = app_with_library().await;
    let (_, r) = call(
        &app,
        Method::POST,
        "/v1/radios",
        Some(json!({"name": "Gone FM", "kbps": 64})),
    )
    .await;
    let id = r["id"].as_str().unwrap().to_string();
    let req = Request::builder()
        .uri(format!("/v1/radios/{id}/stream"))
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let mut body = app.clone().oneshot(req).await.unwrap().into_body();
    tokio::time::timeout(Duration::from_secs(10), body.frame())
        .await
        .expect("audio arrives");
    assert_eq!(
        call(&app, Method::DELETE, &format!("/v1/radios/{id}"), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    // the stream ends (None) well before the track would have
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while body.frame().await.is_some() {}
    })
    .await;
    assert!(
        ended.is_ok(),
        "the stream kept going after the station was deleted"
    );
}

#[tokio::test]
async fn a_listen_key_opens_only_its_own_stream() {
    let app = app_with_library().await;
    let (_, a) = call(&app, Method::POST, "/v1/radios", Some(json!({"name": "A"}))).await;
    let (_, b) = call(&app, Method::POST, "/v1/radios", Some(json!({"name": "B"}))).await;
    let path_a = a["listen_path"].as_str().unwrap().to_string();
    let path_b = b["listen_path"].as_str().unwrap().to_string();
    assert!(
        path_a.contains("?key=") && !path_a.contains(TOKEN),
        "the token itself never appears in the path"
    );
    let anon = |uri: String, method: Method| {
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    };
    let id_b = b["id"].as_str().unwrap();
    // b's key does not open a's stream, a wrong key does not open anything, other routes stay closed
    let swapped = format!(
        "/v1/radios/{}/stream?key={}",
        a["id"].as_str().unwrap(),
        path_b.split("key=").nth(1).unwrap()
    );
    assert_eq!(
        app.clone()
            .oneshot(anon(swapped, Method::GET))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.clone()
            .oneshot(anon(
                format!("/v1/radios/{id_b}/stream?key=nope"),
                Method::GET
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.clone()
            .oneshot(anon(
                format!("/v1/radios?key={}", path_a.split("key=").nth(1).unwrap()),
                Method::GET
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.clone()
            .oneshot(anon(
                format!(
                    "/v1/radios/{id_b}?key={}",
                    path_b.split("key=").nth(1).unwrap()
                ),
                Method::DELETE
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    // the right key reaches the handler (200 with ffmpeg; the station may end at once without it)
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok()
    {
        let ok = app
            .clone()
            .oneshot(anon(path_a, Method::GET))
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn keys_are_random_per_radio_and_can_be_replaced() {
    let app = app_with_library().await;
    let (_, a) = call(&app, Method::POST, "/v1/radios", Some(json!({"name": "A"}))).await;
    let id = a["id"].as_str().unwrap().to_string();
    let old = a["listen_path"].as_str().unwrap().to_string();
    assert_eq!(
        call(&app, Method::GET, "/v1/radios", None).await.1[0]["listen_path"],
        old,
        "stable between reads"
    );
    let anon = |uri: &str| {
        Request::builder()
            .uri(uri.to_string())
            .body(Body::empty())
            .unwrap()
    };
    // a missing station with a made-up key, trailing slash and encoded id all stay closed
    for bad in [
        "/v1/radios/999/stream?key=abc",
        &format!("/v1/radios/{id}/stream/?key=x"),
        &format!(
            "/v1/radios/%31/stream?key={}",
            old.split("key=").nth(1).unwrap()
        ),
    ] {
        let st = app.clone().oneshot(anon(bad)).await.unwrap().status();
        assert!(
            st == StatusCode::UNAUTHORIZED || st == StatusCode::NOT_FOUND,
            "{bad}: {st}"
        );
    }
    let (s, r) = call(
        &app,
        Method::POST,
        &format!("/v1/radios/{id}/rekey"),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let new = r["listen_path"].as_str().unwrap().to_string();
    assert_ne!(new, old);
    assert_eq!(
        app.clone().oneshot(anon(&old)).await.unwrap().status(),
        StatusCode::UNAUTHORIZED,
        "the old key is dead"
    );
    assert_eq!(
        call(&app, Method::POST, "/v1/radios/999/rekey", Some(json!({})))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // HEAD with a key is refused, and a valid key on a deleted radio opens nothing
    let head = Request::builder()
        .method(Method::HEAD)
        .uri(new.clone())
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(head).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    call(&app, Method::DELETE, &format!("/v1/radios/{id}"), None).await;
    assert_eq!(
        app.clone().oneshot(anon(&new)).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}
