use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode, Uri, header},
    routing::get,
};
use http_body_util::BodyExt;
use museq_server::metadata::{RateLimiter, Services};
use museq_server::{api, db};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789";

fn fixtures(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(sub)
}

/// What the fake MusicBrainz / AcoustID saw.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<(String, String)>>>); // (query string, user agent)

async fn fake_service(mb_status: u16, mb_body: Value, ac_body: Value) -> (String, Seen) {
    let seen = Seen::default();
    async fn handle(
        State((seen, status, body)): State<(Seen, u16, Value)>,
        uri: Uri,
        headers: HeaderMap,
    ) -> (StatusCode, axum::Json<Value>) {
        let ua = headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        seen.0
            .lock()
            .unwrap()
            .push((format!("{}?{}", uri.path(), uri.query().unwrap_or("")), ua));
        (StatusCode::from_u16(status).unwrap(), axum::Json(body))
    }
    let app = Router::new()
        .route(
            "/mb/recording",
            get(handle).with_state((seen.clone(), mb_status, mb_body)),
        )
        .route(
            "/ac/lookup",
            get(handle).with_state((seen.clone(), 200, ac_body)),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

fn services(base: &str, key: Option<&str>) -> Services {
    Services {
        musicbrainz_url: format!("{base}/mb"),
        acoustid_url: format!("{base}/ac"),
        acoustid_key: key.map(String::from),
        mb_interval: Duration::ZERO,
        acoustid_interval: Duration::ZERO,
        ..Services::default()
    }
}

fn app_for(root: &Path, svc: Services) -> Router {
    api::router(
        api::AppState::new(
            db::open_in_memory().unwrap(),
            root.to_path_buf(),
            TOKEN.into(),
            root.join("../skins-unused"),
            None,
        )
        .services(svc),
    )
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

async fn run_job(app: &Router, body: Value) -> Value {
    let (s, job) = call(app, Method::POST, "/v1/jobs", Some(body)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{job}");
    let uri = format!("/v1/jobs/{}", job["id"].as_str().unwrap());
    for _ in 0..400 {
        let (_, j) = call(app, Method::GET, &uri, None).await;
        match j["state"].as_str() {
            Some("done") => return j["result"].clone(),
            Some("failed") => panic!("job failed: {j}"),
            _ => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    }
    panic!("job did not finish");
}

fn mb_reply() -> Value {
    json!({"recordings": [
        {"id": "mbid-1", "score": 98, "title": "Real Title", "artist-credit": [{"name": "Real Artist"}], "releases": [{"title": "Real Album", "date": "2009-03-01"}]},
        {"id": "mbid-2", "score": 40, "title": "Weak Guess", "artist-credit": [{"name": "Nobody"}]}
    ]})
}

fn untagged_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        fixtures("meta").join("notags.mp3"),
        dir.path().join("notags.mp3"),
    )
    .unwrap();
    dir
}

async fn first_track(app: &Router) -> Value {
    call(app, Method::GET, "/v1/tracks", None).await.1["items"][0].clone()
}

#[tokio::test]
async fn weak_tags_get_musicbrainz_candidates_that_change_nothing_until_accepted() {
    let (base, seen) = fake_service(200, mb_reply(), json!({})).await;
    let dir = untagged_dir();
    let app = app_for(dir.path(), services(&base, None));
    run_job(&app, json!({"kind": "scan"})).await;
    let t = first_track(&app).await;
    assert_eq!(t["artist"], Value::Null);
    let tid = t["id"].as_str().unwrap().to_string();

    let r = run_job(&app, json!({"kind": "tag"})).await;
    assert_eq!(
        (
            r["tracks_asked"].as_i64(),
            r["candidates_stored"].as_i64(),
            r["acoustid"].clone()
        ),
        (Some(1), Some(1), json!(false)),
        "the weak 40-score guess is dropped: {r}"
    );
    {
        let calls = seen.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].0.contains("recording%3A%22notags%22")
                || calls[0].0.contains("recording:\"notags\""),
            "{}",
            calls[0].0
        );
        assert!(
            calls[0].1.starts_with("Mus-Eq/"),
            "a descriptive User-Agent is required by MusicBrainz: {}",
            calls[0].1
        );
    }
    // stored, but the track is untouched
    let (_, c) = call(
        &app,
        Method::GET,
        &format!("/v1/tracks/{tid}/candidates"),
        None,
    )
    .await;
    assert_eq!(c["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        (
            c["items"][0]["title"].as_str(),
            c["items"][0]["status"].as_str()
        ),
        (Some("Real Title"), Some("pending"))
    );
    assert_eq!(first_track(&app).await["artist"], Value::Null);
    // asking again must not pile up duplicates
    let r2 = run_job(&app, json!({"kind": "tag"})).await;
    assert_eq!(r2["candidates_stored"], 0, "{r2}");

    // accept: the track changes in the database, the file does not
    let before = std::fs::read(dir.path().join("notags.mp3")).unwrap();
    let cid = c["items"][0]["id"].as_str().unwrap();
    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/v1/candidates/{cid}/accept"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let t = first_track(&app).await;
    assert_eq!(
        (
            t["title"].as_str(),
            t["artist"].as_str(),
            t["album"].as_str()
        ),
        (Some("Real Title"), Some("Real Artist"), Some("Real Album"))
    );
    assert_eq!(
        std::fs::read(dir.path().join("notags.mp3")).unwrap(),
        before,
        "the audio file must never be rewritten"
    );
    assert_eq!(
        call(&app, Method::GET, "/v1/tracks?q=real%20artist", None)
            .await
            .1["total"],
        1,
        "search follows the correction"
    );
    assert_eq!(
        call(
            &app,
            Method::POST,
            &format!("/v1/candidates/{cid}/accept"),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    // a rescan of the changed file must not undo the correction
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.path().join("notags.mp3"))
        .unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(120))
        .unwrap();
    drop(f);
    assert_eq!(run_job(&app, json!({"kind": "scan"})).await["updated"], 1);
    assert_eq!(first_track(&app).await["title"], "Real Title");
    // and corrected tracks are not asked about again
    assert_eq!(
        run_job(&app, json!({"kind": "tag"})).await["tracks_asked"],
        0
    );
}

#[tokio::test]
async fn rejecting_a_candidate_and_unknown_ids() {
    let (base, _) = fake_service(200, mb_reply(), json!({})).await;
    let dir = untagged_dir();
    let app = app_for(dir.path(), services(&base, None));
    run_job(&app, json!({"kind": "scan"})).await;
    run_job(&app, json!({"kind": "tag"})).await;
    let tid = first_track(&app).await["id"].as_str().unwrap().to_string();
    let cid = call(
        &app,
        Method::GET,
        &format!("/v1/tracks/{tid}/candidates"),
        None,
    )
    .await
    .1["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        call(
            &app,
            Method::POST,
            &format!("/v1/candidates/{cid}/reject"),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &app,
            Method::POST,
            &format!("/v1/candidates/{cid}/reject"),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            Method::POST,
            &format!("/v1/candidates/{cid}/accept"),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, Method::POST, "/v1/candidates/99999/accept", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, Method::GET, "/v1/tracks/99999/candidates", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        first_track(&app).await["artist"],
        Value::Null,
        "rejecting changes nothing"
    );
}

#[tokio::test]
async fn a_failing_service_is_reported_not_fatal() {
    let (base, _) = fake_service(503, json!({}), json!({})).await;
    let dir = untagged_dir();
    let app = app_for(dir.path(), services(&base, None));
    run_job(&app, json!({"kind": "scan"})).await;
    let r = run_job(&app, json!({"kind": "tag"})).await;
    assert_eq!(
        (
            r["lookups_failed"].as_i64(),
            r["last_error"].as_str(),
            r["candidates_stored"].as_i64()
        ),
        (Some(1), Some("HTTP 503"), Some(0)),
        "{r}"
    );
}

#[tokio::test]
async fn identify_by_audio_needs_a_key_and_a_fingerprint() {
    let ac = json!({"status": "ok", "results": [{"id": "ac-1", "score": 0.93, "recordings": [{"id": "rec-1", "title": "Found It", "artists": [{"name": "Heard Band"}], "releasegroups": [{"title": "RG Album"}]}]}]});
    let (base, seen) = fake_service(200, json!({"recordings": []}), ac).await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        fixtures("dups").join("src1.flac"),
        dir.path().join("a.flac"),
    )
    .unwrap();

    // no key: 501
    let app = app_for(dir.path(), services(&base, None));
    run_job(&app, json!({"kind": "scan"})).await;
    let tid = first_track(&app).await["id"].as_str().unwrap().to_string();
    assert_eq!(
        call(
            &app,
            Method::POST,
            &format!("/v1/tracks/{tid}/identify"),
            None
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );

    // key but no fingerprint yet: 409
    let app = app_for(dir.path(), services(&base, Some("my-app-key")));
    run_job(&app, json!({"kind": "scan"})).await;
    assert_eq!(
        call(
            &app,
            Method::POST,
            &format!("/v1/tracks/{tid}/identify"),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );

    // fingerprinted: AcoustID is asked with our key, the duration and a fingerprint
    run_job(&app, json!({"kind": "fingerprint"})).await;
    let (s, r) = call(
        &app,
        Method::POST,
        &format!("/v1/tracks/{tid}/identify"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    {
        let calls = seen.0.lock().unwrap();
        let q = &calls.last().unwrap().0;
        assert!(
            q.contains("client=my-app-key")
                && q.contains("duration=6")
                && q.contains("fingerprint=")
                && !q.contains("fingerprint=&"),
            "{q}"
        );
    }
    let (_, c) = call(
        &app,
        Method::GET,
        &format!("/v1/tracks/{tid}/candidates"),
        None,
    )
    .await;
    let first = &c["items"][0];
    assert_eq!(
        (
            first["source"].as_str(),
            first["title"].as_str(),
            first["artist"].as_str(),
            first["album"].as_str()
        ),
        (
            Some("acoustid"),
            Some("Found It"),
            Some("Heard Band"),
            Some("RG Album")
        )
    );
    assert!((first["score"].as_f64().unwrap() - 93.0).abs() < 0.01);
}

#[tokio::test]
async fn calls_to_one_service_are_spaced_out() {
    let limiter = RateLimiter::default();
    let start = Instant::now();
    for _ in 0..3 {
        limiter.wait(Duration::from_millis(120)).await;
    }
    assert!(
        start.elapsed() >= Duration::from_millis(230),
        "three calls must take at least two intervals, took {:?}",
        start.elapsed()
    );
}

#[test]
fn replies_are_parsed_defensively() {
    use museq_server::metadata::{parse_acoustid, parse_musicbrainz};
    assert!(parse_musicbrainz(&json!({})).is_empty());
    assert!(
        parse_musicbrainz(&json!({"recordings": [{"id": "x"}]})).is_empty(),
        "no title: skipped"
    );
    assert!(parse_acoustid(&json!({"results": [{"score": 1.0}]})).is_empty());
    let c = parse_musicbrainz(
        &json!({"recordings": [{"id": "m", "score": 90, "title": "T", "artist-credit": [{"name": "A"}, {"name": "B"}]}]}),
    );
    assert_eq!(c[0].artist.as_deref(), Some("A, B"));
}

#[tokio::test]
async fn accepting_a_sparse_candidate_never_blanks_existing_tags() {
    // a recording with a title but no artist and no release
    let sparse = json!({"recordings": [{"id": "mbid-9", "score": 95, "title": "Only A Title"}]});
    let (base, _) = fake_service(200, sparse, json!({})).await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        fixtures("lib").join("sub/beta.mp3"),
        dir.path().join("beta.mp3"),
    )
    .unwrap(); // has artist and album, no genre
    let st = api::AppState::new(
        db::open_in_memory().unwrap(),
        dir.path().to_path_buf(),
        TOKEN.into(),
        PathBuf::from("."),
        None,
    )
    .services(services(&base, None));
    let app = api::router(st.clone());
    run_job(&app, json!({"kind": "scan"})).await;
    let tid = first_track(&app).await["id"].as_str().unwrap().to_string();
    // the owner names the track explicitly, so it is asked about even though its tags are fine
    let r = run_job(&app, json!({"kind": "tag", "track_ids": [tid]})).await;
    assert_eq!(r["candidates_stored"], 1, "{r}");
    let c = call(
        &app,
        Method::GET,
        &format!("/v1/tracks/{tid}/candidates"),
        None,
    )
    .await
    .1;
    assert_eq!(c["items"][0]["mbid"], "mbid-9");
    let cid = c["items"][0]["id"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            Method::POST,
            &format!("/v1/candidates/{cid}/accept"),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    let t = first_track(&app).await;
    assert_eq!(
        (
            t["title"].as_str(),
            t["artist"].as_str(),
            t["album"].as_str()
        ),
        (Some("Only A Title"), Some("Artist Two"), Some("Second")),
        "missing fields keep the track's own values: {t}"
    );
    // asking about a corrected track by name is allowed, and too long a list is refused
    assert_eq!(
        run_job(&app, json!({"kind": "tag", "track_ids": [tid]})).await["tracks_asked"],
        1
    );
    let many: Vec<String> = (0..1001).map(|i| i.to_string()).collect();
    assert_eq!(
        call(
            &app,
            Method::POST,
            "/v1/jobs",
            Some(json!({"kind": "tag", "track_ids": many}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn the_acoustid_key_never_appears_in_results_or_errors() {
    let (base, _) = fake_service(
        200,
        json!({"recordings": []}),
        json!({"status": "error", "error": {"message": "invalid API key"}}),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        fixtures("dups").join("src1.flac"),
        dir.path().join("a.flac"),
    )
    .unwrap();
    let key = "SECRETKEY-1234567890";
    let app = app_for(dir.path(), services(&base, Some(key)));
    run_job(&app, json!({"kind": "scan"})).await;
    run_job(&app, json!({"kind": "fingerprint"})).await;
    let tid = first_track(&app).await["id"].as_str().unwrap().to_string();
    let r = run_job(&app, json!({"kind": "tag", "track_ids": [tid.clone()]})).await;
    assert_eq!(r["lookups_failed"], 1, "{r}");
    assert!(
        !r.to_string().contains(key),
        "job result leaked the key: {r}"
    );
    let (s, body) = call(
        &app,
        Method::POST,
        &format!("/v1/tracks/{tid}/identify"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
    assert!(
        !body.to_string().contains(key),
        "error body leaked the key: {body}"
    );
}
