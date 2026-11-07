// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use museq_server::radio::{Cand, CaptureState, OggHeaderCapture, pick_next};
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

/// One Ogg page with a body of at most 254 bytes (a single lacing value); the checksum is not needed here.
fn ogg_page(granule: i64, seq: u32, body: &[u8]) -> Vec<u8> {
    assert!(body.len() < 255);
    let mut p = b"OggS".to_vec();
    p.push(0); // version
    p.push(0); // header type
    p.extend_from_slice(&granule.to_le_bytes());
    p.extend_from_slice(&7u32.to_le_bytes()); // stream serial
    p.extend_from_slice(&seq.to_le_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]); // crc
    p.push(1); // one segment
    p.push(body.len() as u8);
    p.extend_from_slice(body);
    p
}

fn opus_stream() -> (Vec<u8>, Vec<u8>) {
    let head = ogg_page(0, 0, b"OpusHead\x01\x02");
    let tags = ogg_page(0, 1, b"OpusTagsvendor");
    let audio1 = ogg_page(960, 2, &[1u8; 40]);
    let audio2 = ogg_page(1920, 3, &[2u8; 40]);
    let header = [head.clone(), tags.clone()].concat();
    let all = [head, tags, audio1, audio2].concat();
    (header, all)
}

#[test]
fn ogg_capture_keeps_the_pages_before_the_first_audio_page() {
    let (header, all) = opus_stream();
    let mut c = OggHeaderCapture::new();
    assert_eq!(c.feed(&all).as_deref(), Some(header.as_slice()));
    assert!(c.feed(&all).is_none(), "the header is reported once");

    // the same when the bytes arrive one at a time
    let mut c = OggHeaderCapture::new();
    let mut found = Vec::new();
    for b in &all {
        if let Some(h) = c.feed(&[*b]) {
            found.push(h);
        }
    }
    assert_eq!(found.len(), 1);
    assert_eq!(&found[0][..], header.as_slice());

    // nothing is reported until an audio page has been seen
    let mut c = OggHeaderCapture::new();
    assert!(c.feed(&all[..header.len() + 10]).is_none());
}

#[test]
fn ogg_capture_counts_continuation_pages_as_header_and_ignores_other_data() {
    // granule -1: a packet that goes on in the next page, still header
    let a = ogg_page(0, 0, b"OpusHead\x01\x02");
    let b = ogg_page(-1, 1, b"OpusTags...");
    let c2 = ogg_page(0, 2, b"...end");
    let audio = ogg_page(960, 3, &[9u8; 30]);
    let header = [a.clone(), b.clone(), c2.clone()].concat();
    let mut cap = OggHeaderCapture::new();
    assert_eq!(
        cap.feed(&[header.clone(), audio].concat()).as_deref(),
        Some(header.as_slice())
    );

    // not Ogg at all (an mp3 stream, say): nothing is kept, and it never panics
    let mut cap = OggHeaderCapture::new();
    assert!(cap.feed(&b"ID3".repeat(40)).is_none());
    assert!(
        cap.feed(&opus_stream().1).is_none(),
        "once out of step it stops looking"
    );

    // header pages that never end are not kept without limit
    let mut cap = OggHeaderCapture::new();
    let big = ogg_page(0, 0, &[0u8; 250]);
    let mut kept = None;
    for _ in 0..400 {
        kept = kept.or(cap.feed(&big));
    }
    assert!(kept.is_none());
    assert!(cap.feed(&ogg_page(960, 1, &[1u8; 10])).is_none());
}

#[tokio::test]
async fn a_listener_who_joins_an_opus_radio_mid_track_gets_the_header_pages_first() {
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
        Some(json!({"name": "Opus FM", "codec": "opus", "kbps": 64})),
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
    let first = open().await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()[header::CONTENT_TYPE], "audio/ogg");

    // the first listener hears the track from its start: it begins with the header pages
    let mut a = first.into_body();
    let mut heard = Vec::new();
    let started = std::time::Instant::now();
    while heard.len() < 12_000 && started.elapsed() < Duration::from_secs(15) {
        if let Some(Ok(f)) = tokio::time::timeout(Duration::from_secs(5), a.frame())
            .await
            .expect("audio arrives")
            && let Ok(d) = f.into_data()
        {
            heard.extend_from_slice(&d);
        }
    }
    assert!(heard.len() >= 12_000, "the first listener got audio");
    assert_eq!(&heard[..4], b"OggS");
    assert_eq!(&heard[28..36], b"OpusHead");

    // a second listener joins now, well into the track: it must start with the same header pages
    let mut b = open().await.unwrap().into_body();
    let mut got = Vec::new();
    while got.len() < 400 {
        let f = tokio::time::timeout(Duration::from_secs(5), b.frame())
            .await
            .expect("audio arrives")
            .expect("the stream goes on")
            .unwrap();
        if let Ok(d) = f.into_data() {
            got.extend_from_slice(&d);
        }
    }
    assert_eq!(&got[..4], b"OggS", "a joiner starts on a page boundary");
    assert_eq!(
        &got[28..36],
        b"OpusHead",
        "the first page is the stream header"
    );
    assert_eq!(
        &got[75..83],
        b"OpusTags",
        "the second page is the comment header"
    );
}

#[test]
fn ogg_capture_reports_everything_so_far_while_the_header_is_still_arriving() {
    let (header, all) = opus_stream();
    let first_len = ogg_page(0, 0, b"OpusHead\x01\x02").len();
    let mut c = OggHeaderCapture::new();
    assert!(c.pending().is_empty());

    // only the first page so far: a listener who joins now must get that page, and the rest live
    assert!(c.feed(&all[..first_len]).is_none());
    assert_eq!(c.state(), CaptureState::Capturing);
    assert_eq!(&c.pending()[..], &all[..first_len]);

    // half of the second page: still everything received, including the part page
    assert!(c.feed(&all[first_len..first_len + 10]).is_none());
    assert_eq!(&c.pending()[..], &all[..first_len + 10]);

    // the rest of the header and the first audio page: the header alone is handed over, and the state is final
    let done = c.feed(&all[first_len + 10..]).expect("header found");
    assert_eq!(&done[..], header.as_slice());
    assert_eq!(c.state(), CaptureState::Done);
    assert!(c.feed(&all).is_none());
}

#[test]
fn ogg_capture_forgets_everything_when_it_gives_up() {
    let mut c = OggHeaderCapture::new();
    assert!(c.feed(&b"ID3".repeat(20)).is_none());
    assert_eq!(c.state(), CaptureState::Failed);
    assert!(c.pending().is_empty(), "nothing is kept after a failure");

    // an enormous header never ends: it fails and frees what it held
    let mut c = OggHeaderCapture::new();
    let big = ogg_page(0, 0, &[0u8; 250]);
    for _ in 0..400 {
        c.feed(&big);
    }
    assert_eq!(c.state(), CaptureState::Failed);
    assert!(c.pending().is_empty());
}

#[tokio::test]
async fn joining_an_opus_radio_at_any_moment_still_starts_with_the_header_pages() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        assert!(std::env::var_os("CI").is_none(), "ffmpeg is required in CI");
        return;
    }
    let app = app_with_library().await;
    // The second listener joins at different moments after the station starts, including the first instants, when
    // the header pages have been sent but the first audio page has not been seen yet.
    for (round, delay_ms) in [0u64, 40, 120, 300].into_iter().enumerate() {
        let (_, r) = call(
            &app,
            Method::POST,
            "/v1/radios",
            Some(json!({"name": format!("Join {round}"), "codec": "opus", "kbps": 64})),
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
        let first = open().await.unwrap().into_body();
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        let mut second = open().await.unwrap().into_body();
        let mut got = Vec::new();
        while got.len() < 120 {
            let f = tokio::time::timeout(Duration::from_secs(8), second.frame())
                .await
                .expect("audio arrives")
                .expect("the stream goes on")
                .unwrap();
            if let Ok(d) = f.into_data() {
                got.extend_from_slice(&d);
            }
        }
        assert_eq!(
            &got[..4],
            b"OggS",
            "joined after {delay_ms} ms: starts on a page"
        );
        assert_eq!(
            &got[28..36],
            b"OpusHead",
            "joined after {delay_ms} ms: first page is the stream header"
        );
        assert_eq!(
            &got[75..83],
            b"OpusTags",
            "joined after {delay_ms} ms: second page is the comment header"
        );
        drop((first, second));
        // wait for this station to go quiet so the next round has a free encoder slot
        for _ in 0..80 {
            let (_, list) = call(&app, Method::GET, "/v1/radios", None).await;
            let mine = list
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["id"] == id.as_str())
                .unwrap();
            if mine["listeners"] == 0 && mine["now_track_id"].is_null() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
