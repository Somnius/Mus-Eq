// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
//! Transcoding through ffmpeg. Needs ffmpeg on PATH (the CI image and the container have it).
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use museq_server::{api, db};
use serde_json::{Value, json};
use std::path::PathBuf;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789";

fn have_ffmpeg() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok()
}

async fn setup() -> (Router, String) {
    let lib = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dups");
    let state = api::AppState::new(
        db::open_in_memory().unwrap(),
        lib,
        TOKEN.into(),
        PathBuf::from("/nonexistent"),
        None,
    );
    let app = api::router(state);
    let (_, job) = call(&app, "/v1/jobs", Some(json!({"kind": "scan"}))).await;
    let uri = format!("/v1/jobs/{}", job.1["id"].as_str().unwrap());
    for _ in 0..200 {
        let (_, j) = call(&app, &uri, None).await;
        if j.1["state"] == "done" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let (_, files) = call(&app, "/v1/tracks", None).await;
    let fid = files.1["items"][0]["file_id"]
        .as_str()
        .or_else(|| files.1["items"][0]["files"][0]["id"].as_str())
        .unwrap_or("1")
        .to_string();
    (app, fid)
}

type Reply = (StatusCode, Value);

async fn raw(app: &Router, uri: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let req = Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (p, body) = resp.into_parts();
    let ct = p
        .headers
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string());
    (
        p.status,
        ct,
        body.collect().await.unwrap().to_bytes().to_vec(),
    )
}

async fn call(app: &Router, uri: &str, body: Option<Value>) -> ((), Reply) {
    let b = Request::builder()
        .method(if body.is_some() {
            Method::POST
        } else {
            Method::GET
        })
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
        (),
        (
            p.status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        ),
    )
}

#[tokio::test]
async fn bad_requests_are_refused_before_ffmpeg_runs() {
    let (app, fid) = setup().await;
    for q in [
        "codec=wav",
        "kbps=128",
        "codec=opus&kbps=8",
        "codec=mp3&kbps=999",
        "codec=opus&kbps=abc",
    ] {
        let (s, _, _) = raw(&app, &format!("/v1/audio/{fid}?{q}")).await;
        assert!(s == StatusCode::BAD_REQUEST, "{q}: {s}");
    }
}

#[tokio::test]
async fn each_codec_produces_real_audio_of_that_kind() {
    if !have_ffmpeg() {
        eprintln!("ffmpeg missing: skipped");
        return;
    }
    let (app, fid) = setup().await;
    for (codec, magic, mime) in [
        ("opus", &b"OggS"[..], "audio/ogg"),
        ("mp3", &b"ID3"[..], "audio/mpeg"),
        ("aac", &[0xFF, 0xF1][..], "audio/aac"),
    ] {
        let (s, ct, bytes) = raw(&app, &format!("/v1/audio/{fid}?codec={codec}&kbps=64")).await;
        assert_eq!(s, StatusCode::OK, "{codec}");
        assert_eq!(ct.as_deref(), Some(mime));
        assert!(bytes.len() > 2000, "{codec}: only {} bytes", bytes.len());
        let ok = bytes.starts_with(magic) || (codec == "mp3" && bytes[0] == 0xFF);
        assert!(ok, "{codec}: starts {:02x?}", &bytes[..4]);
    }
    // the plain answer is unchanged: the original bytes with Range support
    let (s, _, orig) = raw(&app, &format!("/v1/audio/{fid}")).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!orig.is_empty());
}

#[tokio::test]
async fn a_transcode_is_audibly_the_same_recording() {
    if !have_ffmpeg() {
        eprintln!("ffmpeg missing: skipped");
        return;
    }
    // fingerprint the opus transcode and compare with the original's: it must match
    let (app, fid) = setup().await;
    let (_, _, bytes) = raw(&app, &format!("/v1/audio/{fid}?codec=opus&kbps=96")).await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("t.opus");
    std::fs::write(&out, &bytes).unwrap();
    let (a, _) = museq_server::fingerprint::fingerprint_file(&out).expect("transcode decodes");
    let (_, _, orig) = raw(&app, &format!("/v1/audio/{fid}")).await;
    let ext = if orig.starts_with(b"fLaC") {
        "flac"
    } else if orig.starts_with(b"OggS") {
        "ogg"
    } else {
        "mp3"
    };
    let o = dir.path().join(format!("o.{ext}"));
    std::fs::write(&o, &orig).unwrap();
    let (b, _) = museq_server::fingerprint::fingerprint_file(&o).unwrap();
    let c = museq_server::duplicates::compare(&a, &b).expect("comparable");
    assert!(
        c.covered_score <= museq_server::duplicates::MAX_SCORE,
        "score {}",
        c.covered_score
    );
}

#[tokio::test]
async fn a_missing_ffmpeg_is_a_clear_501_not_a_crash() {
    let lib = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dups");
    let state = api::AppState::new(
        db::open_in_memory().unwrap(),
        lib,
        TOKEN.into(),
        PathBuf::from("/nonexistent"),
        None,
    )
    .ffmpeg("/nonexistent/ffmpeg".into());
    let app = api::router(state);
    let (_, job) = call(&app, "/v1/jobs", Some(json!({"kind": "scan"}))).await;
    let uri = format!("/v1/jobs/{}", job.1["id"].as_str().unwrap());
    for _ in 0..200 {
        if call(&app, &uri, None).await.1.1["state"] == "done" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let (s, _, body) = raw(&app, "/v1/audio/1?codec=mp3").await;
    assert_eq!(
        s,
        StatusCode::NOT_IMPLEMENTED,
        "{}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn only_two_transcodes_run_at_once() {
    if !have_ffmpeg() {
        return;
    }
    let (app, fid) = setup().await;
    // hold two bodies open without reading them: the third request must be turned away
    let open = |app: Router, uri: String| async move {
        let req = Request::builder()
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap();
        app.oneshot(req).await.unwrap()
    };
    let r1 = open(app.clone(), format!("/v1/audio/{fid}?codec=mp3")).await;
    let r2 = open(app.clone(), format!("/v1/audio/{fid}?codec=mp3")).await;
    assert_eq!((r1.status(), r2.status()), (StatusCode::OK, StatusCode::OK));
    let (s, _, _) = raw(&app, &format!("/v1/audio/{fid}?codec=mp3")).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    drop((r1, r2));
    // once the bodies are dropped the permits come back
    let mut ok = false;
    for _ in 0..40 {
        let (s, _, _) = raw(&app, &format!("/v1/audio/{fid}?codec=mp3")).await;
        if s == StatusCode::OK {
            ok = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(ok, "permits were not released");
}

/// Scripts that stand in for ffmpeg, written once for all the tests of this file.
fn fake_ffmpeg(kind: &str) -> PathBuf {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let dir = DIR.get_or_init(|| {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        for (name, body) in [
            ("fail_at_once", "#!/bin/sh\nexit 1\n"),
            (
                "fail_part_way",
                "#!/bin/sh\nhead -c 65536 /dev/zero\nexit 1\n",
            ),
            ("succeed", "#!/bin/sh\nhead -c 1000 /dev/zero\nexit 0\n"),
            (
                "close_then_hang",
                "#!/bin/sh\nhead -c 100 /dev/zero\nexec 1>&-\nsleep 30\n",
            ),
        ] {
            let p = d.path().join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // let any fork that raced with the writes finish before the scripts are run
        std::thread::sleep(std::time::Duration::from_millis(100));
        d
    });
    dir.path().join(kind)
}

async fn with_fake(kind: &str) -> (Router, String) {
    let lib = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dups");
    let state = api::AppState::new(
        db::open_in_memory().unwrap(),
        lib,
        TOKEN.into(),
        PathBuf::from("/nonexistent"),
        None,
    )
    .ffmpeg(fake_ffmpeg(kind).to_string_lossy().into_owned());
    let app = api::router(state);
    let (_, job) = call(&app, "/v1/jobs", Some(json!({"kind": "scan"}))).await;
    let uri = format!("/v1/jobs/{}", job.1["id"].as_str().unwrap());
    for _ in 0..200 {
        let (_, j) = call(&app, &uri, None).await;
        if j.1["state"] == "done" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    (app, "1".to_string())
}

#[tokio::test]
async fn a_file_ffmpeg_cannot_convert_is_a_502_not_an_empty_200() {
    let (app, fid) = with_fake("fail_at_once").await;
    let (status, _, body) = raw(&app, &format!("/v1/audio/{fid}?codec=mp3")).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(String::from_utf8_lossy(&body).contains("could not convert"));
    assert!(
        !String::from_utf8_lossy(&body).contains("fixtures"),
        "the answer must not name file paths"
    );
}

#[tokio::test]
async fn an_ffmpeg_that_dies_part_way_breaks_the_transfer_instead_of_looking_finished() {
    let (app, fid) = with_fake("fail_part_way").await;
    let req = Request::builder()
        .uri(format!("/v1/audio/{fid}?codec=mp3"))
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the answer had already started"
    );
    let mut body = resp.into_body();
    let (mut bytes, mut failed) = (0usize, false);
    while let Some(frame) = body.frame().await {
        match frame {
            Ok(f) => bytes += f.into_data().map(|d| d.len()).unwrap_or(0),
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    assert!(
        bytes >= 65536,
        "the audio before the failure was delivered ({bytes})"
    );
    assert!(failed, "the body must end with an error, not a clean end");
}

#[tokio::test]
async fn an_ffmpeg_that_finishes_normally_ends_the_body_cleanly() {
    let (app, fid) = with_fake("succeed").await;
    let (status, ct, body) = raw(&app, &format!("/v1/audio/{fid}?codec=mp3")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct.as_deref(), Some("audio/mpeg"));
    assert_eq!(body.len(), 1000);
}

#[tokio::test]
async fn an_ffmpeg_that_closes_its_output_and_then_hangs_does_not_hold_the_request() {
    let (app, fid) = with_fake("close_then_hang").await;
    let req = Request::builder()
        .uri(format!("/v1/audio/{fid}?codec=mp3"))
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.into_body();
    let started = std::time::Instant::now();
    let mut failed = false;
    // the body must end (with an error, since ffmpeg did not exit cleanly) well before the 30 s the script sleeps
    while let Ok(Some(frame)) =
        tokio::time::timeout(std::time::Duration::from_secs(15), body.frame()).await
    {
        if frame.is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed, "the body ended without an error");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(12),
        "the wait for ffmpeg was not bounded ({:?})",
        started.elapsed()
    );
}
