// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
//! Bad input and crashes must never take the job queue or the server down.
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use museq_server::{api, db};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789";

fn fixtures(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(sub)
}

fn app_for(root: &Path) -> Router {
    // the panic hooks only act when this is set; no other test uses the marker names
    // SAFETY: set once, before any server code in this process reads it
    unsafe { std::env::set_var("MUSEQ_TEST_HOOKS", "1") };
    api::router(api::AppState::new(
        db::open_in_memory().unwrap(),
        root.to_path_buf(),
        TOKEN.into(),
        root.join("../skins-unused"),
        None,
    ))
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

/// Start a job and wait for it to end; returns (state, result, error).
async fn run(app: &Router, kind: &str) -> (String, Value, Value) {
    let (s, job) = call(app, Method::POST, "/v1/jobs", Some(json!({"kind": kind}))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{job}");
    let uri = format!("/v1/jobs/{}", job["id"].as_str().unwrap());
    for _ in 0..600 {
        let (_, j) = call(app, Method::GET, &uri, None).await;
        if matches!(j["state"].as_str(), Some("done" | "failed")) {
            return (
                j["state"].as_str().unwrap().into(),
                j["result"].clone(),
                j["error"].clone(),
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("job {kind} did not end");
}

fn tiny(dir: &Path, name: &str) {
    std::fs::copy(fixtures("lib").join("sub/beta.mp3"), dir.join(name)).unwrap();
}

#[tokio::test]
async fn a_decoder_panic_on_one_file_costs_that_file_nothing_more() {
    let dir = tempfile::tempdir().unwrap();
    tiny(dir.path(), "good.mp3");
    tiny(dir.path(), "a__museq_panic_probe__.mp3");
    let app = app_for(dir.path());
    let (state, result, _) = run(&app, "scan").await;
    assert_eq!(state, "done");
    assert_eq!(
        result["added"], 2,
        "the file whose probe panicked is still catalogued, without tags: {result}"
    );
    assert_eq!(
        call(&app, Method::GET, "/v1/tracks", None).await.1["total"],
        2
    );
}

#[tokio::test]
async fn a_panic_outside_every_guard_fails_the_job_but_never_blocks_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    tiny(dir.path(), "good.mp3");
    let bomb = dir.path().join("b__museq_panic_scan__.mp3");
    tiny(dir.path(), "b__museq_panic_scan__.mp3");
    let app = app_for(dir.path());
    let (state, _, error) = run(&app, "scan").await;
    assert_eq!(state, "failed");
    assert!(error.as_str().unwrap().contains("crash"), "{error}");
    // not stuck behind a "running" row, and the database lock is not poisoned
    assert_eq!(
        call(&app, Method::GET, "/v1/tracks", None).await.0,
        StatusCode::OK
    );
    std::fs::remove_file(bomb).unwrap();
    let (state, result, _) = run(&app, "scan").await;
    assert_eq!(
        (state.as_str(), result["added"].as_i64()),
        ("done", Some(1)),
        "{result}"
    );
}

#[tokio::test]
async fn a_fingerprint_panic_marks_that_file_and_the_job_finishes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        fixtures("dups").join("src1.flac"),
        dir.path().join("ok.flac"),
    )
    .unwrap();
    std::fs::copy(
        fixtures("dups").join("src2.mp3"),
        dir.path().join("x__museq_panic_fp__.mp3"),
    )
    .unwrap();
    let app = app_for(dir.path());
    run(&app, "scan").await;
    let (state, result, _) = run(&app, "fingerprint").await;
    assert_eq!(state, "done", "{result}");
    assert_eq!(
        (result["fingerprinted"].as_i64(), result["failed"].as_i64()),
        (Some(1), Some(1)),
        "{result}"
    );
}

#[tokio::test]
async fn damaged_and_empty_files_are_survived() {
    let dir = tempfile::tempdir().unwrap();
    let flac = std::fs::read(fixtures("dups").join("src1.flac")).unwrap();
    std::fs::write(dir.path().join("truncated.flac"), &flac[..flac.len() / 20]).unwrap();
    std::fs::write(dir.path().join("empty.mp3"), b"").unwrap();
    std::fs::write(
        dir.path().join("garbage.opus"),
        b"OggS\0\x02 not really an ogg stream, just noise \xff\xfe\xfd",
    )
    .unwrap();
    std::fs::write(dir.path().join("zeros.ogg"), vec![0u8; 4096]).unwrap();
    std::fs::copy(
        fixtures("dups").join("src2.mp3"),
        dir.path().join("fine.mp3"),
    )
    .unwrap();
    let app = app_for(dir.path());
    let (state, result, _) = run(&app, "scan").await;
    assert_eq!(state, "done", "{result}");
    let (state, result, _) = run(&app, "fingerprint").await;
    assert_eq!(
        state, "done",
        "damaged files must not fail the job: {result}"
    );
    assert_eq!(
        result["fingerprinted"], 1,
        "only the intact file is fingerprinted: {result}"
    );
    assert_eq!(
        call(&app, Method::GET, "/v1/duplicates", None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, Method::GET, "/v1/tracks", None).await.0,
        StatusCode::OK
    );
}
