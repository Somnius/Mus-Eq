// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use museq_server::radio::{
    Cand, CaptureState, ICY_METAINT, IcyInjector, OggHeaderCapture, icy_block, icy_title, pick_next,
};
use museq_server::transcode::Tags;
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
    // HEAD with the right key gets the headers and nothing else: no body, and no encoder is started
    let head = |uri: String, method: Method| {
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    };
    let r = app
        .clone()
        .oneshot(head(new.clone(), Method::HEAD))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert!(r.into_body().collect().await.unwrap().to_bytes().is_empty());
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_, list) = call(&app, Method::GET, "/v1/radios", None).await;
    let mine = list
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == id.as_str())
        .unwrap();
    assert_eq!(mine["listeners"], 0);
    assert!(
        mine["now_track_id"].is_null(),
        "HEAD started a station: {mine}"
    );
    // a wrong key, or any method but GET and HEAD, is refused
    let wrong = new.replace("key=", "key=0");
    assert_eq!(
        app.clone()
            .oneshot(head(wrong, Method::HEAD))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_ne!(
        app.clone()
            .oneshot(head(new.clone(), Method::POST))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    // a valid key on a deleted radio opens nothing
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

#[test]
fn icy_blocks_are_sixteen_byte_units_and_titles_cannot_break_the_field() {
    assert_eq!(icy_block(None), vec![0], "no title: a single zero byte");
    let b = icy_block(Some("Artist - Title"));
    assert_eq!((b.len() - 1) % 16, 0, "padded to a multiple of 16");
    assert_eq!(b[0] as usize * 16, b.len() - 1);
    let text = String::from_utf8_lossy(&b[1..])
        .trim_end_matches('\0')
        .to_string();
    assert_eq!(text, "StreamTitle='Artist - Title';");

    // quotes, semicolons and control characters are removed; the result is capped
    let t = Tags {
        title: "It's; a \n\u{7}title".into(),
        artist: Some("A\u{0}rtist".into()),
    };
    assert_eq!(icy_title(&t).as_deref(), Some("Artist - Its a title"));
    let long = Tags {
        title: "x".repeat(5000),
        artist: None,
    };
    assert!(icy_title(&long).unwrap().chars().count() <= 200);
    assert!(icy_block(icy_title(&long).as_deref()).len() < 255 * 16);
    assert_eq!(
        icy_title(&Tags {
            title: "  ".into(),
            artist: None
        }),
        None
    );
    assert_eq!(
        icy_title(&Tags {
            title: "T".into(),
            artist: Some("A".into())
        })
        .as_deref(),
        Some("A - T")
    );
}

#[test]
fn the_injector_puts_blocks_at_exact_offsets_whatever_the_chunk_sizes() {
    // the same audio in odd-sized chunks must produce the same bytes
    let audio: Vec<u8> = (0..(ICY_METAINT * 3 + 123))
        .map(|i| (i % 251) as u8)
        .collect();
    let run = |sizes: &[usize]| {
        let mut inj = IcyInjector::default();
        let mut out = Vec::new();
        let (mut at, mut k) = (0, 0);
        while at < audio.len() {
            let n = sizes[k % sizes.len()].min(audio.len() - at);
            out.extend(inj.inject(&audio[at..at + n], Some("A - T")));
            at += n;
            k += 1;
        }
        out
    };
    let a = run(&[8192]);
    assert_eq!(a, run(&[1]), "byte by byte");
    assert_eq!(
        a,
        run(&[7, 4000, 15999, 16001, 33]),
        "odd sizes, also across a boundary"
    );

    // parse it back: audio, block, audio, block ...
    let mut pos = 0;
    let mut got = Vec::new();
    let mut titles = Vec::new();
    while pos + ICY_METAINT <= a.len() {
        got.extend_from_slice(&a[pos..pos + ICY_METAINT]);
        pos += ICY_METAINT;
        let len = a[pos] as usize * 16;
        titles.push(
            String::from_utf8_lossy(&a[pos + 1..pos + 1 + len])
                .trim_end_matches('\0')
                .to_string(),
        );
        pos += 1 + len;
    }
    got.extend_from_slice(&a[pos..]);
    assert_eq!(
        got, audio,
        "removing the blocks gives back the audio exactly"
    );
    assert_eq!(
        titles,
        vec![
            "StreamTitle='A - T';".to_string(),
            String::new(),
            String::new()
        ],
        "sent once, then 'no change'"
    );
}

#[test]
fn the_injector_sends_a_changed_title_and_forgets_nothing_between_chunks() {
    let mut inj = IcyInjector::default();
    let chunk = vec![7u8; ICY_METAINT];
    let one = inj.inject(&chunk, Some("First"));
    assert!(String::from_utf8_lossy(&one[ICY_METAINT + 1..]).starts_with("StreamTitle='First';"));
    let two = inj.inject(&chunk, Some("Second"));
    assert!(String::from_utf8_lossy(&two[ICY_METAINT + 1..]).starts_with("StreamTitle='Second';"));
    let three = inj.inject(&chunk, Some("Second"));
    assert_eq!(three[ICY_METAINT], 0, "unchanged title: one zero byte");
    let four = inj.inject(&chunk, None);
    assert!(
        four[ICY_METAINT] > 0,
        "no title any more is a change, sent as an empty field or a cleared one"
    );
}

async fn first_bytes(mut body: Body, want: usize) -> Vec<u8> {
    let mut got = Vec::new();
    while got.len() < want {
        let f = tokio::time::timeout(Duration::from_secs(10), body.frame())
            .await
            .expect("audio arrives")
            .expect("the stream goes on")
            .unwrap();
        if let Ok(d) = f.into_data() {
            got.extend_from_slice(&d);
        }
    }
    got
}

async fn make_radio(app: &Router, codec: &str) -> String {
    let (_, r) = call(
        app,
        Method::POST,
        "/v1/radios",
        Some(json!({"name": format!("{codec} FM"), "codec": codec, "kbps": 64})),
    )
    .await;
    r["id"].as_str().unwrap().to_string()
}

fn open_with(
    app: &Router,
    id: &str,
    method: Method,
    icy: bool,
) -> impl std::future::Future<Output = axum::response::Response> + use<> {
    let mut b = Request::builder()
        .method(method)
        .uri(format!("/v1/radios/{id}/stream"))
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    if icy {
        b = b.header("icy-metadata", "1");
    }
    let req = b.body(Body::empty()).unwrap();
    let app = app.clone();
    async move { app.oneshot(req).await.unwrap() }
}

fn have_ffmpeg() -> bool {
    let ok = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok();
    if !ok {
        assert!(std::env::var_os("CI").is_none(), "ffmpeg is required in CI");
    }
    ok
}

#[tokio::test]
async fn an_mp3_radio_sends_the_track_title_to_players_that_ask_for_it() {
    if !have_ffmpeg() {
        return;
    }
    let app = app_with_library().await;
    let id = make_radio(&app, "mp3").await;

    // asked for: icy-metaint is announced and the blocks sit at exact offsets
    let resp = open_with(&app, &id, Method::GET, true).await;
    assert_eq!(
        resp.headers()["icy-metaint"],
        ICY_METAINT.to_string().as_str()
    );
    let got = first_bytes(resp.into_body(), ICY_METAINT * 2 + 400).await;
    assert_eq!(
        &got[..3],
        b"ID3",
        "the stream itself starts as mp3 always did"
    );
    let len = got[ICY_METAINT] as usize * 16;
    assert!(len > 0, "the first block carries the title");
    let text = String::from_utf8_lossy(&got[ICY_METAINT + 1..ICY_METAINT + 1 + len])
        .trim_end_matches('\0')
        .to_string();
    assert!(
        text.starts_with("StreamTitle='Synth Lab - Noise") && text.ends_with("';"),
        "{text}"
    );
    // the second block, one interval later: still the same title, so one zero byte
    let second = ICY_METAINT + 1 + len + ICY_METAINT;
    assert_eq!(got[second], 0, "unchanged title: no change byte");

    // not asked for: nothing announced. This listener joins mid-track, so it gets audio from wherever the
    // stream is, not the start of a file
    let plain = open_with(&app, &id, Method::GET, false).await;
    assert!(plain.headers().get("icy-metaint").is_none());
    let bytes = first_bytes(plain.into_body(), 300).await;
    assert!(bytes.len() >= 300);
}

#[tokio::test]
async fn an_opus_radio_is_never_given_icy_blocks_but_carries_the_title_in_its_own_tags() {
    if !have_ffmpeg() {
        return;
    }
    let app = app_with_library().await;
    let id = make_radio(&app, "opus").await;
    let resp = open_with(&app, &id, Method::GET, true).await;
    assert!(
        resp.headers().get("icy-metaint").is_none(),
        "inserting bytes would corrupt an Ogg stream"
    );
    let got = first_bytes(resp.into_body(), 400).await;
    assert_eq!(&got[..4], b"OggS");
    assert_eq!(&got[28..36], b"OpusHead");
    let tags = String::from_utf8_lossy(&got[..400]).to_lowercase();
    assert!(
        tags.contains("title=noise"),
        "the OpusTags page names the track: {tags:?}"
    );
    assert!(
        tags.contains("artist=synth lab"),
        "and its artist: {tags:?}"
    );
}

#[tokio::test]
async fn a_head_request_does_not_join_the_station_or_start_an_encoder() {
    if !have_ffmpeg() {
        return;
    }
    let app = app_with_library().await;
    let id = make_radio(&app, "mp3").await;
    let resp = open_with(&app, &id, Method::HEAD, true).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()[header::CONTENT_TYPE], "audio/mpeg");
    assert_eq!(resp.headers()["icy-name"], "mp3 FM");
    assert_eq!(
        resp.headers()["icy-metaint"],
        ICY_METAINT.to_string().as_str()
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(body.is_empty(), "no body for HEAD");
    // give an encoder time to start if one wrongly did
    tokio::time::sleep(Duration::from_millis(700)).await;
    let (_, list) = call(&app, Method::GET, "/v1/radios", None).await;
    let mine = list
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == id.as_str())
        .unwrap();
    assert_eq!(mine["listeners"], 0);
    assert!(
        mine["now_track_id"].is_null(),
        "no track is on air: nothing started: {mine}"
    );
}
