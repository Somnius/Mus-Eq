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

async fn run_job(app: &Router, kind: &str) -> Value {
    let (s, job) = call(app, Method::POST, "/v1/jobs", Some(json!({"kind": kind}))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{job}");
    let uri = format!("/v1/jobs/{}", job["id"].as_str().unwrap());
    for _ in 0..600 {
        let (_, j) = call(app, Method::GET, &uri, None).await;
        match j["state"].as_str() {
            Some("done") => return j["result"].clone(),
            Some("failed") => panic!("job failed: {j}"),
            _ => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    }
    panic!("job {kind} did not finish");
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

fn file_id(files: &Value, ends_with: &str) -> String {
    files
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"].as_str().unwrap().ends_with(ends_with))
        .unwrap_or_else(|| panic!("no {ends_with} in {files}"))["file_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn the_same_recording_in_three_formats_is_one_group_and_merging_keeps_everything_working() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixtures("dups"), dir.path());
    std::fs::copy(
        fixtures("lib").join("sub/gamma.opus"),
        dir.path().join("gamma.opus"),
    )
    .unwrap();
    let app = app_for(dir.path());

    assert_eq!(run_job(&app, "scan").await["added"], 6);
    let fp = run_job(&app, "fingerprint").await;
    assert_eq!(
        (fp["fingerprinted"].as_i64(), fp["failed"].as_i64()),
        (Some(5), Some(1)),
        "five files fingerprint (Opus included); the one-second tone is too short and must not fail the job: {fp}"
    );

    let (_, dups) = call(&app, Method::GET, "/v1/duplicates", None).await;
    let groups = dups["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{dups}");
    let g = &groups[0];
    let flac = file_id(&g["files"], "src1.flac");
    let mp3 = file_id(&g["files"], "src1.mp3");
    let ogg = file_id(&g["files"], "src1.ogg");
    let opus = file_id(&g["files"], "src1.opus");
    assert_eq!(
        g["files"].as_array().unwrap().len(),
        4,
        "flac, mp3, ogg and Opus of one source; the other noise source and the short tone stay out: {g}"
    );
    assert_eq!(
        g["keep_file_id"],
        flac.as_str(),
        "the lossless file is the proposed keeper"
    );
    assert!(
        g["proof"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["kind"] == "audio" && p["score"].as_f64().unwrap() <= 2.0),
        "{g}"
    );

    // a playlist holding the mp3's track
    let (_, mp3_track) = call(&app, Method::GET, "/v1/tracks?q=Noise%20One", None).await;
    assert_eq!(
        mp3_track["total"], 4,
        "search is a substring match: flac, mp3, opus and the ogg rip all contain it: {mp3_track}"
    );
    let all = call(&app, Method::GET, "/v1/tracks?limit=50", None).await.1;
    assert_eq!(all["total"], 6);
    let mp3_track_id = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["preferred_file_id"] == mp3.as_str())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, pl) = call(
        &app,
        Method::POST,
        "/v1/playlists",
        Some(json!({"name": "mix"})),
    )
    .await;
    let pl_uri = format!("/v1/playlists/{}", pl["id"].as_str().unwrap());
    call(
        &app,
        Method::POST,
        &format!("{pl_uri}/items"),
        Some(json!({"track_id": mp3_track_id})),
    )
    .await;

    // merge
    let (s, m) = call(
        &app,
        Method::POST,
        "/v1/duplicates/merge",
        Some(json!({"file_ids": [flac, mp3, ogg, opus], "keep_file_id": flac})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{m}");
    let kept_track = m["track_id"].as_str().unwrap().to_string();
    assert_eq!(
        call(&app, Method::GET, "/v1/tracks?limit=50", None).await.1["total"],
        3,
        "flac+mp3+ogg are now one track"
    );
    let (_, t) = call(&app, Method::GET, &format!("/v1/tracks/{kept_track}"), None).await;
    assert_eq!(t["preferred_file_id"], flac.as_str());
    let (_, pl_now) = call(&app, Method::GET, &pl_uri, None).await;
    assert_eq!(
        pl_now["track_ids"],
        json!([kept_track]),
        "the playlist entry moved to the kept track"
    );
    // nothing was deleted: every file is still on disk and still streams
    for f in ["src1.flac", "src1.mp3", "src1.ogg", "src1.opus"] {
        assert!(dir.path().join(f).exists(), "{f} must still be on disk");
    }
    assert_eq!(
        call(&app, Method::GET, &format!("/v1/audio/{ogg}"), None)
            .await
            .0,
        StatusCode::OK
    );
    // the group is settled: not offered again, and a rescan changes nothing
    assert_eq!(
        call(&app, Method::GET, "/v1/duplicates", None).await.1["groups"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let r = run_job(&app, "scan").await;
    assert_eq!(
        (r["added"].as_i64(), r["removed"].as_i64()),
        (Some(0), Some(0)),
        "{r}"
    );
    assert_eq!(
        call(&app, Method::GET, "/v1/tracks?limit=50", None).await.1["total"],
        3
    );

    // lose the preferred file: the track survives on one of the others
    std::fs::remove_file(dir.path().join("src1.flac")).unwrap();
    let r = run_job(&app, "scan").await;
    assert_eq!(r["removed"], 1, "{r}");
    assert_eq!(
        call(&app, Method::GET, "/v1/tracks?limit=50", None).await.1["total"],
        3,
        "the merged track must not vanish with its preferred file"
    );
    let (_, t) = call(&app, Method::GET, &format!("/v1/tracks/{kept_track}"), None).await;
    assert_ne!(t["preferred_file_id"], flac.as_str());
    let new_pref = t["preferred_file_id"].as_str().unwrap();
    assert_eq!(
        call(&app, Method::GET, &format!("/v1/audio/{new_pref}"), None)
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn identical_files_are_found_without_fingerprints_and_a_different_source_is_not_a_duplicate()
{
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(fixtures("dups").join("src2.mp3"), dir.path().join("a.mp3")).unwrap();
    std::fs::copy(
        fixtures("dups").join("src2.mp3"),
        dir.path().join("b-copy.mp3"),
    )
    .unwrap();
    std::fs::copy(
        fixtures("dups").join("src1.mp3"),
        dir.path().join("other.mp3"),
    )
    .unwrap();
    let app = app_for(dir.path());
    run_job(&app, "scan").await;
    let (_, dups) = call(&app, Method::GET, "/v1/duplicates", None).await;
    let groups = dups["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{dups}");
    assert_eq!(groups[0]["proof"][0]["kind"], "hash");
    assert_eq!(groups[0]["files"].as_array().unwrap().len(), 2);
    // after fingerprinting, different recordings still don't match
    run_job(&app, "fingerprint").await;
    let (_, dups) = call(&app, Method::GET, "/v1/duplicates", None).await;
    assert_eq!(
        dups["groups"].as_array().unwrap().len(),
        1,
        "the two different noise sources must not be grouped: {dups}"
    );
}

#[tokio::test]
async fn merge_validates_its_input() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixtures("dups"), dir.path());
    let app = app_for(dir.path());
    run_job(&app, "scan").await;
    let ids: Vec<String> = call(&app, Method::GET, "/v1/tracks?limit=50", None).await.1["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["preferred_file_id"].as_str().unwrap().to_string())
        .collect();
    for (body, want) in [
        (
            json!({"file_ids": [ids[0]], "keep_file_id": ids[0]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"file_ids": [ids[0], ids[1]], "keep_file_id": ids[2]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"file_ids": [ids[0], "x"], "keep_file_id": ids[0]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"file_ids": [ids[0], "99999"], "keep_file_id": ids[0]}),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (s, b) = call(
            &app,
            Method::POST,
            "/v1/duplicates/merge",
            Some(body.clone()),
        )
        .await;
        assert_eq!(s, want, "{body}: {b}");
    }
    assert_eq!(
        call(&app, Method::GET, "/v1/tracks?limit=50", None).await.1["total"],
        5,
        "a refused merge changes nothing"
    );
}

#[tokio::test]
async fn fingerprinting_and_scanning_never_run_together() {
    let st = api::AppState::new(
        db::open_in_memory().unwrap(),
        fixtures("dups"),
        TOKEN.into(),
        PathBuf::from("."),
        None,
    );
    db::lock(&st.db)
        .execute("INSERT INTO job(kind, state) VALUES('scan', 'running')", [])
        .unwrap();
    let app = api::router(st);
    let (s, _) = call(
        &app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "fingerprint"})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_scanner_records_what_quality_ranking_needs() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixtures("dups"), dir.path());
    let app = app_for(dir.path());
    run_job(&app, "scan").await;
    let (_, dups) = call(&app, Method::GET, "/v1/duplicates", None).await;
    assert_eq!(
        dups["groups"].as_array().unwrap().len(),
        0,
        "without fingerprints only identical bytes can be found"
    );
    // genre is stored for playlist extension
    let conn_state = api::AppState::new(
        db::open_in_memory().unwrap(),
        dir.path().to_path_buf(),
        TOKEN.into(),
        PathBuf::from("."),
        None,
    );
    let app2 = api::router(conn_state.clone());
    run_job(&app2, "scan").await;
    let c = db::lock(&conn_state.db);
    let (genre, rate, lossless): (Option<String>, Option<i64>, i64) = c
        .query_row("SELECT t.genre, f.sample_rate, f.lossless FROM file f JOIN track t ON t.id=f.track_id WHERE f.path='src1.flac'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert_eq!(
        (genre.as_deref(), rate, lossless),
        (Some("Electronic"), Some(11025), 1)
    );
    let (mp3_lossless, mp3_kbps): (i64, Option<i64>) = c
        .query_row(
            "SELECT lossless, bitrate_kbps FROM file WHERE path='src1.mp3'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(mp3_lossless, 0);
    assert!(
        mp3_kbps.is_some_and(|k| (30..70).contains(&k)),
        "average bitrate of a 48 kbps file: {mp3_kbps:?}"
    );
}

#[tokio::test]
async fn merging_keeps_the_owners_correction_and_pending_suggestions() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixtures("dups"), dir.path());
    let state = api::AppState::new(
        db::open_in_memory().unwrap(),
        dir.path().to_path_buf(),
        TOKEN.into(),
        PathBuf::from("."),
        None,
    );
    let app = api::router(state.clone());
    run_job(&app, "scan").await;
    let (_, all) = call(&app, Method::GET, "/v1/tracks?limit=50", None).await;
    let by_path = |p: &str| {
        let fid: i64 = db::lock(&state.db)
            .query_row("SELECT id FROM file WHERE path=?1", [p], |r| r.get(0))
            .unwrap();
        fid
    };
    let (flac, ogg) = (by_path("src1.flac"), by_path("src1.ogg"));
    let _ = all;
    {
        // the owner corrected the ogg's track and left a suggestion pending on it
        let c = db::lock(&state.db);
        let ogg_track: i64 = c
            .query_row("SELECT track_id FROM file WHERE id=?1", [ogg], |r| r.get(0))
            .unwrap();
        c.execute("UPDATE track SET title='My Own Title', artist='Me', corrected=1, search_text='my own title me' WHERE id=?1", [ogg_track]).unwrap();
        c.execute("INSERT INTO candidate(track_id, source, score, title, mbid, created_at) VALUES(?1,'musicbrainz',90,'Suggestion','m1',1)", [ogg_track]).unwrap();
    }
    let (s, m) = call(&app, Method::POST, "/v1/duplicates/merge", Some(json!({"file_ids": [flac.to_string(), ogg.to_string()], "keep_file_id": flac.to_string()}))).await;
    assert_eq!(s, StatusCode::OK, "{m}");
    let tid = m["track_id"].as_str().unwrap();
    let (_, t) = call(&app, Method::GET, &format!("/v1/tracks/{tid}"), None).await;
    assert_eq!(
        (t["title"].as_str(), t["artist"].as_str()),
        (Some("My Own Title"), Some("Me")),
        "the correction must survive the merge: {t}"
    );
    let (_, c) = call(
        &app,
        Method::GET,
        &format!("/v1/tracks/{tid}/candidates"),
        None,
    )
    .await;
    assert_eq!(
        c["items"].as_array().unwrap().len(),
        1,
        "the pending suggestion moved to the kept track: {c}"
    );
    assert_eq!(
        call(&app, Method::GET, "/v1/tracks?q=my%20own", None)
            .await
            .1["total"],
        1,
        "search follows"
    );
}

#[tokio::test]
async fn tracks_carry_their_genre() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixtures("dups"), dir.path());
    let app = app_for(dir.path());
    run_job(&app, "scan").await;
    let (_, r) = call(&app, Method::GET, "/v1/tracks?q=noise%20two", None).await;
    assert_eq!(r["items"][0]["genre"], "Ambient", "{r}");
}
