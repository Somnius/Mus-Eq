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

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lib")
}

fn app_for(root: PathBuf) -> Router {
    api::router(api::AppState::new(
        db::open_in_memory().unwrap(),
        root,
        TOKEN.into(),
        std::env::temp_dir().join(format!("museq-test-skins-{}", std::process::id())),
        None,
    ))
}

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = if let Some(v) = body {
        b.header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap()
    } else {
        b.body(Body::empty()).unwrap()
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    (
        parts.status,
        parts.headers,
        body.collect().await.unwrap().to_bytes().to_vec(),
    )
}

async fn json_call(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (s, _, b) = call(app, method, uri, Some(TOKEN), body).await;
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

async fn scan_and_wait(app: &Router) -> Value {
    let (s, job) = json_call(app, Method::POST, "/v1/jobs", Some(json!({"kind": "scan"}))).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let uri = format!("/v1/jobs/{}", job["id"].as_str().unwrap());
    for _ in 0..200 {
        let (_, j) = json_call(app, Method::GET, &uri, None).await;
        if j["state"] == "done" {
            return j["result"].clone();
        }
        assert_ne!(j["state"], "failed", "scan failed: {j}");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("scan did not finish");
}

#[tokio::test]
async fn health_is_open_everything_else_needs_the_token() {
    let app = app_for(fixtures());
    let (s, _, _) = call(&app, Method::GET, "/v1/health", None, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = call(&app, Method::GET, "/v1/tracks", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = call(
        &app,
        Method::GET,
        "/v1/tracks",
        Some("wrong-token-0123456789"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = call(&app, Method::GET, "/v1/tracks", Some(TOKEN), None).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn scan_finds_tags_and_durations_and_search_works() {
    let app = app_for(fixtures());
    let r = scan_and_wait(&app).await;
    assert_eq!(r["added"], 4, "{r}");
    assert_eq!(r["errors"], 0);

    let (_, all) = json_call(&app, Method::GET, "/v1/tracks", None).await;
    assert_eq!(all["total"], 4);
    let items = all["items"].as_array().unwrap();
    let titles: Vec<&str> = items.iter().map(|t| t["title"].as_str().unwrap()).collect();
    assert!(
        titles.contains(&"Alpha") && titles.contains(&"Beta") && titles.contains(&"Gamma"),
        "{titles:?}"
    );
    for t in items {
        let d = t["duration_ms"]
            .as_i64()
            .unwrap_or_else(|| panic!("no duration for {t}"));
        assert!((900..=1200).contains(&d), "duration {d} ms for {t}");
    }

    let (_, one) = json_call(&app, Method::GET, "/v1/tracks?q=artist%20one", None).await;
    assert_eq!(one["total"], 2);
    let (_, none) = json_call(&app, Method::GET, "/v1/tracks?q=%25", None).await;
    assert_eq!(none["total"], 0, "a literal % must not act as a wildcard");
    // Greek: case and accents don't matter, final sigma folds
    for q in [
        "%CE%BB%CE%AC%CE%BC%CF%88%CE%B7",
        "%CE%9B%CE%91%CE%9C%CE%A8%CE%97",
        "%CE%BB%CE%B1%CE%BC%CF%88%CE%B7",
        "%CE%B5%CE%BB%CE%BB%CE%B7%CE%BD%CE%B9%CE%BA%CE%BF%CF%82",
    ] {
        let (_, g) = json_call(&app, Method::GET, &format!("/v1/tracks?q={q}"), None).await;
        assert_eq!(g["total"], 1, "query {q}: {g}");
    }
    let (_, page) = json_call(&app, Method::GET, "/v1/tracks?limit=1&offset=1", None).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["total"], 4);
}

#[tokio::test]
async fn audio_supports_range_and_transcodes() {
    let app = app_for(fixtures());
    scan_and_wait(&app).await;
    let (_, all) = json_call(&app, Method::GET, "/v1/tracks", None).await;
    let fid = all["items"][0]["preferred_file_id"]
        .as_str()
        .unwrap()
        .to_string();

    let (s, _, full) = call(
        &app,
        Method::GET,
        &format!("/v1/audio/{fid}"),
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(full.len() > 100);

    let req = Request::builder()
        .uri(format!("/v1/audio/{fid}"))
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(header::RANGE, "bytes=0-9")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert!(resp.headers().contains_key(header::CONTENT_RANGE));
    let part = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&part[..], &full[..10]);

    let (s, _, _) = call(
        &app,
        Method::GET,
        &format!("/v1/audio/{fid}?codec=opus"),
        Some(TOKEN),
        None,
    )
    .await;
    // transcoding is covered in tests/transcode.rs; here only that it is no longer refused outright
    assert!(
        s == StatusCode::OK || s == StatusCode::NOT_IMPLEMENTED,
        "{s}"
    );
    let (s, _, _) = call(&app, Method::GET, "/v1/audio/99999", Some(TOKEN), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (p, q) = (e.path(), to.join(e.file_name()));
        if p.is_dir() {
            copy_dir(&p, &q)
        } else {
            std::fs::copy(&p, &q).unwrap();
        }
    }
}

#[tokio::test]
async fn rescan_skips_unchanged_and_drops_vanished_files() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixtures(), dir.path());
    let app = app_for(dir.path().to_path_buf());
    scan_and_wait(&app).await;

    let r = scan_and_wait(&app).await;
    assert_eq!(
        (
            r["added"].as_i64(),
            r["unchanged"].as_i64(),
            r["removed"].as_i64()
        ),
        (Some(0), Some(4), Some(0)),
        "{r}"
    );

    std::fs::remove_file(dir.path().join("sub/beta.mp3")).unwrap();
    let r = scan_and_wait(&app).await;
    assert_eq!(r["removed"], 1, "{r}");
    let (_, all) = json_call(&app, Method::GET, "/v1/tracks", None).await;
    assert_eq!(all["total"], 3);
}

#[tokio::test]
async fn playlists_create_add_reorder_delete() {
    let app = app_for(fixtures());
    scan_and_wait(&app).await;
    let (_, all) = json_call(&app, Method::GET, "/v1/tracks", None).await;
    let ids: Vec<String> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();

    let (s, _) = json_call(
        &app,
        Method::POST,
        "/v1/playlists",
        Some(json!({"name": "  "})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, pl) = json_call(
        &app,
        Method::POST,
        "/v1/playlists",
        Some(json!({"name": "Mix", "extend": true})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(pl["extend"], true);
    let uri = format!("/v1/playlists/{}", pl["id"].as_str().unwrap());

    for id in &ids[..2] {
        let (s, _) = json_call(
            &app,
            Method::POST,
            &format!("{uri}/items"),
            Some(json!({"track_id": id})),
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }
    let (s, _) = json_call(
        &app,
        Method::POST,
        &format!("{uri}/items"),
        Some(json!({"track_id": "99999"})),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, got) = json_call(&app, Method::GET, &uri, None).await;
    assert_eq!(got["track_ids"], json!([ids[0], ids[1]]));

    let (s, got) = json_call(
        &app,
        Method::PUT,
        &format!("{uri}/items"),
        Some(json!({"track_ids": [ids[2], ids[1], ids[0]]})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(got["track_ids"], json!([ids[2], ids[1], ids[0]]));
    let (s, got) = json_call(
        &app,
        Method::PUT,
        &format!("{uri}/items"),
        Some(json!({"track_ids": [ids[1]]})),
    )
    .await;
    assert_eq!((s, &got["track_ids"]), (StatusCode::OK, &json!([ids[1]])));

    let (_, list) = json_call(&app, Method::GET, "/v1/playlists", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (s, _) = json_call(&app, Method::DELETE, &uri, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = json_call(&app, Method::GET, &uri, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn job_kinds_are_gated() {
    let app = app_for(fixtures());
    let (s, _) = json_call(
        &app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "fetch"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = json_call(
        &app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "nope"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rescan_updates_changed_files_and_keeps_moved_files_in_their_playlists() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixtures(), dir.path());
    let app = app_for(dir.path().to_path_buf());
    scan_and_wait(&app).await;

    // a playlist containing the Opus track
    let (_, found) = json_call(&app, Method::GET, "/v1/tracks?q=gamma", None).await;
    let tid = found["items"][0]["id"].as_str().unwrap().to_string();
    let (_, pl) = json_call(
        &app,
        Method::POST,
        "/v1/playlists",
        Some(json!({"name": "keep"})),
    )
    .await;
    let uri = format!("/v1/playlists/{}", pl["id"].as_str().unwrap());
    json_call(
        &app,
        Method::POST,
        &format!("{uri}/items"),
        Some(json!({"track_id": tid})),
    )
    .await;

    // move it: same bytes, new path
    std::fs::rename(
        dir.path().join("sub/gamma.opus"),
        dir.path().join("moved.opus"),
    )
    .unwrap();
    let r = scan_and_wait(&app).await;
    assert_eq!(
        (
            r["added"].as_i64(),
            r["updated"].as_i64(),
            r["removed"].as_i64()
        ),
        (Some(0), Some(1), Some(0)),
        "{r}"
    );
    let (_, got) = json_call(&app, Method::GET, &uri, None).await;
    assert_eq!(
        got["track_ids"],
        json!([tid]),
        "the moved track must stay in the playlist"
    );
    let (_, all) = json_call(&app, Method::GET, "/v1/tracks", None).await;
    assert_eq!(all["total"], 4, "no duplicate track after a move");

    // change a file's content: counted as updated
    let alpha = dir.path().join("alpha.flac");
    let mut bytes = std::fs::read(&alpha).unwrap();
    bytes.extend_from_slice(&[0u8; 16]);
    std::fs::write(&alpha, bytes).unwrap();
    let r = scan_and_wait(&app).await;
    assert_eq!(r["updated"], 1, "{r}");
}

#[tokio::test]
async fn a_second_scan_is_refused_while_one_runs() {
    let state = api::AppState::new(
        db::open_in_memory().unwrap(),
        fixtures(),
        TOKEN.into(),
        std::env::temp_dir(),
        None,
    );
    db::lock(&state.db)
        .execute("INSERT INTO job(kind, state) VALUES('scan', 'running')", [])
        .unwrap();
    let app = api::router(state);
    let (s, _) = json_call(
        &app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "scan"})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
}
