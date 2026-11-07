//! The fetch module, tested with a fake yt-dlp and a fake Spotify page. Nothing here touches the internet.
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
    routing::get,
};
use http_body_util::BodyExt;
use museq_server::fetch::{self, Candidate, FetchConfig, Source, TrackMeta};
use museq_server::{api, db};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789";

fn fixtures(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(sub)
}

// ---- pure functions ---------------------------------------------------------------------------

#[test]
fn only_known_kinds_of_link_are_accepted() {
    use Source::*;
    assert_eq!(
        fetch::parse_source("https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RDAMVM").unwrap(),
        YouTube("dQw4w9WgXcQ".into())
    );
    assert_eq!(
        fetch::parse_source("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap(),
        YouTube("dQw4w9WgXcQ".into())
    );
    assert_eq!(
        fetch::parse_source("https://youtu.be/dQw4w9WgXcQ?t=5").unwrap(),
        YouTube("dQw4w9WgXcQ".into())
    );
    assert_eq!(
        fetch::parse_source("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=abc")
            .unwrap(),
        SpotifyTrack("4uLU6hMCjMI75M1A2tKUQC".into())
    );
    assert_eq!(
        fetch::parse_source("https://open.spotify.com/intl-gr/track/4uLU6hMCjMI75M1A2tKUQC")
            .unwrap(),
        SpotifyTrack("4uLU6hMCjMI75M1A2tKUQC".into())
    );
    for bad in [
        "http://music.youtube.com/watch?v=dQw4w9WgXcQ", // not https
        "https://music.youtube.com/watch?v=short",      // not an id
        "https://music.youtube.com/watch?v=dQw4w9WgXcQ%0A--exec=rm", // whitespace/control
        "https://music.youtube.com/playlist?list=PLabc", // playlists: not yet
        "https://open.spotify.com/album/4uLU6hMCjMI75M1A2tKUQC", // albums: not yet
        "https://evil.example/watch?v=dQw4w9WgXcQ",     // not on the list
        "https://user:pw@music.youtube.com/watch?v=dQw4w9WgXcQ", // credentials
        "https://music.youtube.com.evil.example/watch?v=dQw4w9WgXcQ", // look-alike host
        "https://127.0.0.1/watch?v=dQw4w9WgXcQ",
        "-o /etc/passwd",
        "",
    ] {
        assert!(fetch::parse_source(bad).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn file_names_cannot_escape_the_inbox() {
    for (input, expected) in [
        ("../../etc/passwd", "etc passwd"),
        ("AC/DC: Back In Black?", "AC DC Back In Black"),
        ("..", "track"),
        ("", "track"),
        ("   ...   ", "track"),
        ("normal name", "normal name"),
        ("τίτλος ✓ ελληνικά", "τίτλος ✓ ελληνικά"),
        ("with\0nul\nand\ttabs", "with nul and tabs"),
    ] {
        let got = fetch::sanitize_name(input);
        assert_eq!(got, expected, "{input:?}");
        assert!(
            !got.contains('/')
                && !got.contains('\\')
                && !got.contains('\0')
                && got != "."
                && got != ".."
        );
    }
    assert!(fetch::sanitize_name(&"x".repeat(500)).chars().count() <= 120);
}

#[test]
fn a_spotify_page_is_read_from_its_meta_tags_or_refused() {
    let page = r#"<html><head>
      <meta property="og:title" content="Caf&eacute; &amp; Co">
      <meta property="music:musician_description" content="The Artist">
      <meta property="music:duration" content="215">
      <meta property="og:description" content="The Artist · Some Album · Song · 2020">
    </head></html>"#;
    let m = fetch::parse_spotify_page(page).unwrap();
    assert_eq!((m.artist.as_str(), m.duration_s), ("The Artist", Some(215)));
    assert!(m.title.contains('&'), "entities are decoded: {}", m.title);
    // artist only in the description line
    let page2 = r#"<meta property="og:title" content="Song"><meta property="og:description" content="Band Name · Album · Song · 1999">"#;
    assert_eq!(
        fetch::parse_spotify_page(page2).unwrap().artist,
        "Band Name"
    );
    // a changed page is an error, never a guess
    assert!(fetch::parse_spotify_page("<html>nothing useful</html>").is_err());
    assert!(
        fetch::parse_spotify_page(r#"<meta property="og:title" content="Only a title">"#).is_err()
    );
}

fn cand(id: &str, title: &str, channel: &str, dur: i64) -> Candidate {
    Candidate {
        id: id.into(),
        title: title.into(),
        channel: channel.into(),
        duration_s: Some(dur),
    }
}

#[test]
fn matching_prefers_the_studio_upload_and_rejects_other_versions() {
    let meta = TrackMeta {
        title: "Real Song".into(),
        artist: "Real Artist".into(),
        album: None,
        duration_s: Some(200),
    };
    let cands = [
        cand(
            "LIVEAAAAAAA",
            "Real Song (Live at the Arena)",
            "Real Artist",
            201,
        ),
        cand("REMIXAAAAAA", "Real Song - Remix", "Some DJ - Topic", 200),
        cand(
            "SLOWEDAAAAA",
            "Real Song slowed + reverb",
            "Real Artist",
            200,
        ),
        cand(
            "LYRICAAAAAA",
            "Real Artist - Real Song (Lyrics)",
            "Lyric Channel",
            200,
        ),
        cand("TOPICAAAAAA", "Real Song", "Real Artist - Topic", 200),
        cand("WRONGLEN000", "Real Song", "Real Artist - Topic", 260),
    ];
    assert_eq!(
        fetch::pick(&meta, &cands).unwrap().id,
        "TOPICAAAAAA",
        "the Topic upload of the right length wins"
    );
    // nothing acceptable: a reasoned failure naming the closest, never a guess
    let err = fetch::pick(
        &meta,
        &[cand("XXXXXXXXXXX", "Something Else Entirely", "Other", 200)],
    )
    .unwrap_err();
    assert!(err.contains("closest"), "{err}");
    assert!(
        fetch::pick(&meta, &[])
            .unwrap_err()
            .contains("nothing was found")
    );
    // a version the request asked for is allowed
    let live = TrackMeta {
        title: "Real Song (Live)".into(),
        ..meta.clone()
    };
    assert_eq!(
        fetch::pick(
            &live,
            &[cand("LIVEAAAAAAA", "Real Song (Live)", "Real Artist", 201)]
        )
        .unwrap()
        .id,
        "LIVEAAAAAAA"
    );
}

#[test]
fn error_text_from_the_tool_is_sorted_into_causes() {
    use fetch::Failure::*;
    for (text, code, want) in [
        (
            "ERROR: HTTP Error 429: Too Many Requests",
            Some(1),
            RateLimited,
        ),
        (
            "ERROR: Video unavailable. This video is private",
            Some(1),
            Gone,
        ),
        (
            "ERROR: Sign in to confirm you're not a bot",
            Some(1),
            LoginRequired,
        ),
        (
            "ERROR: This video is not available in your country",
            Some(1),
            Geo,
        ),
        ("", Some(101), Live),
        ("something odd", Some(1), Other),
    ] {
        assert_eq!(fetch::classify(text, code), want, "{text}");
    }
}

// ---- the module through the API, with a fake yt-dlp ---------------------------------------------

/// Write a fake yt-dlp. `mode`: ok | ratelimit | login | big | evil | live | slow
fn fake_ytdlp(dir: &Path, mode: &str) -> PathBuf {
    let audio = fixtures("dups").join("src1.mp3");
    let script = format!(
        r#"#!/bin/sh
MODE={mode}
case "$*" in *ytsearch*) search=1;; esac
if [ -n "$search" ]; then
  echo '{{"entries":[{{"id":"WRONGLIVE00","title":"Real Song (Live)","channel":"Fan","duration":201}},{{"id":"RIGHTTOPIC0","title":"Real Song","channel":"Real Artist - Topic","duration":200}}]}}'
  exit 0
fi
[ "$MODE" = ratelimit ] && {{ echo "ERROR: HTTP Error 429: Too Many Requests" >&2; exit 1; }}
[ "$MODE" = live ] && {{ exit 101; }}
[ "$MODE" = slow ] && {{ d=$(dirname "$0"); echo $$ > "$d/slow.pid"; sleep 30 & echo $! > "$d/slow-child.pid"; wait; }}
# find the -o template
while [ $# -gt 0 ]; do [ "$1" = "-o" ] && tmpl="$2"; [ "$1" = "--cookies" ] && cookies="$2"; shift; done
if [ "$MODE" = login ] && [ -z "$cookies" ]; then echo "ERROR: Sign in to confirm you're not a bot" >&2; exit 1; fi
dir=$(dirname "$tmpl")
if [ "$MODE" = evil ]; then
  cp "{audio}" "$dir/media.mp3"
  echo '{{"title":"../../../escape","track":"../../../escape","artists":["../Evil/Artist"],"channel":"X - Topic","_version":{{"version":"2026.01.01"}}}}' > "$dir/media.info.json"
else
  cp "{audio}" "$dir/media.mp3"
  echo '{{"title":"Real Song","track":"Real Song","artists":["Real Artist"],"album":"Real Album","channel":"Real Artist - Topic","_version":{{"version":"2026.01.01"}}}}' > "$dir/media.info.json"
fi
echo "$dir/media.mp3"
"#,
        audio = audio.display()
    );
    let path = dir.join(format!("fake-ytdlp-{mode}.sh"));
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

struct Env {
    app: Router,
    state: api::AppState,
    library: tempfile::TempDir,
}

fn enabled(library: &Path, ytdlp: PathBuf, spotify_base: &str) -> FetchConfig {
    FetchConfig {
        enabled: true,
        disabled_reason: None,
        ytdlp,
        inbox: library.join("_fetched"),
        timeout: Duration::from_secs(10),
        spotify_base: spotify_base.into(),
        allow_private: true,
        ..FetchConfig::default()
    }
}

fn env_with(mode: &str, cookies: bool, spotify_base: &str) -> Env {
    let library = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap().keep();
    let mut cfg = enabled(library.path(), fake_ytdlp(&tools, mode), spotify_base);
    if cookies {
        let ck = tools.join("cookies.txt");
        std::fs::write(&ck, "# Netscape HTTP Cookie File\n.example.test\tTRUE\t/\tTRUE\t0\tSECRETCOOKIE\tSECRETVALUE\n").unwrap();
        std::fs::set_permissions(&ck, std::fs::Permissions::from_mode(0o600)).unwrap();
        cfg.cookies = Some(ck);
    }
    let state = api::AppState::new(
        db::open_in_memory().unwrap(),
        library.path().to_path_buf(),
        TOKEN.into(),
        library.path().join("../skins"),
        None,
    )
    .fetch(cfg);
    Env {
        app: api::router(state.clone()),
        state,
        library,
    }
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

async fn fetch_job(app: &Router, url: &str) -> (String, Value, Value) {
    let (s, job) = call(
        app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "fetch", "input": {"url": url}})),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{job}");
    let uri = format!("/v1/jobs/{}", job["id"].as_str().unwrap());
    for _ in 0..800 {
        let (_, j) = call(app, Method::GET, &uri, None).await;
        if matches!(j["state"].as_str(), Some("done" | "failed")) {
            return (
                j["state"].as_str().unwrap().into(),
                j["result"].clone(),
                j["error"].clone(),
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("fetch job did not end");
}

const YT: &str = "https://music.youtube.com/watch?v=RIGHTTOPIC0";

#[tokio::test]
async fn fetch_is_off_by_default_and_says_why() {
    let library = tempfile::tempdir().unwrap();
    let st = api::AppState::new(
        db::open_in_memory().unwrap(),
        library.path().to_path_buf(),
        TOKEN.into(),
        library.path().join("s"),
        None,
    );
    let app = api::router(st);
    let (s, body) = call(
        &app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "fetch", "input": {"url": YT}})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(
        body["error"].as_str().unwrap().contains("MUSEQ_FETCH"),
        "{body}"
    );
    let (_, status) = call(&app, Method::GET, "/v1/fetch/status", None).await;
    assert_eq!(status["enabled"], false);
    assert!(
        !library.path().join("_fetched").exists(),
        "nothing is created while it is off"
    );
}

#[test]
fn turning_fetch_on_needs_the_typed_acknowledgement_and_a_real_tool() {
    // exercised through the environment, so run these checks in one test (the variables are process-wide)
    let lib = tempfile::tempdir().unwrap();
    let tool = fake_ytdlp(lib.path(), "ok");
    unsafe {
        std::env::set_var("MUSEQ_FETCH", "on");
        std::env::remove_var("MUSEQ_FETCH_ACK");
        std::env::set_var("MUSEQ_FETCH_YTDLP", &tool);
    }
    let c = FetchConfig::from_env(lib.path());
    assert!(
        !c.enabled && c.disabled_reason.unwrap().contains("MUSEQ_FETCH_ACK"),
        "no acknowledgement: stays off"
    );
    unsafe { std::env::set_var("MUSEQ_FETCH_ACK", "yes") };
    assert!(
        !FetchConfig::from_env(lib.path()).enabled,
        "a wrong acknowledgement does not count"
    );
    unsafe { std::env::set_var("MUSEQ_FETCH_ACK", fetch::ACK_PHRASE) };
    assert!(FetchConfig::from_env(lib.path()).enabled);
    unsafe { std::env::set_var("MUSEQ_FETCH_YTDLP", "yt-dlp") };
    let c = FetchConfig::from_env(lib.path());
    assert!(
        !c.enabled && c.disabled_reason.unwrap().contains("absolute"),
        "no PATH lookup"
    );
    unsafe { std::env::set_var("MUSEQ_FETCH_YTDLP", &tool) };
    // cookies must be private
    let loose = lib.path().join("cookies.txt");
    std::fs::write(&loose, "x").unwrap();
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();
    unsafe { std::env::set_var("MUSEQ_FETCH_COOKIES", &loose) };
    let c = FetchConfig::from_env(lib.path());
    assert!(
        !c.enabled && c.disabled_reason.as_ref().unwrap().contains("mode 600"),
        "{:?}",
        c.disabled_reason
    );
    assert!(
        !c.disabled_reason.unwrap().contains(loose.to_str().unwrap()),
        "the cookie path is never in a message"
    );
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(FetchConfig::from_env(lib.path()).enabled);
    unsafe {
        for k in [
            "MUSEQ_FETCH",
            "MUSEQ_FETCH_ACK",
            "MUSEQ_FETCH_YTDLP",
            "MUSEQ_FETCH_COOKIES",
        ] {
            std::env::remove_var(k);
        }
    }
}

#[tokio::test]
async fn bad_links_are_refused_before_anything_runs() {
    let env = env_with("ok", false, "http://unused.example");
    for bad in [
        "https://evil.example/x",
        "https://open.spotify.com/album/4uLU6hMCjMI75M1A2tKUQC",
        "not a url",
        "",
    ] {
        let (s, body) = call(
            &env.app,
            Method::POST,
            "/v1/jobs",
            Some(json!({"kind": "fetch", "input": {"url": bad}})),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    assert_eq!(
        call(
            &env.app,
            Method::POST,
            "/v1/jobs",
            Some(json!({"kind": "fetch"}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(!env.library.path().join("_fetched").exists());
}

#[tokio::test]
async fn a_youtube_link_becomes_a_track_with_provenance() {
    let env = env_with("ok", false, "http://unused.example");
    let (state, result, error) = fetch_job(&env.app, YT).await;
    assert_eq!(state, "done", "{error}");
    assert_eq!(result["outcome"], "imported", "{result}");
    assert_eq!(
        (
            result["title"].as_str(),
            result["artist"].as_str(),
            result["album"].as_str()
        ),
        (Some("Real Song"), Some("Real Artist"), Some("Real Album"))
    );
    // it is in the library, with the source's tags, and its own file under the inbox
    let (_, tracks) = call(&env.app, Method::GET, "/v1/tracks?q=real%20song", None).await;
    assert_eq!(tracks["total"], 1, "{tracks}");
    assert!(
        env.library
            .path()
            .join("_fetched/Real Artist - Real Song.mp3")
            .exists()
    );
    // provenance is stored, the work folder is gone, and fingerprinting ran
    let (url, vid, ver): (String, String, Option<String>) = {
        let c = db::lock(&env.state.db);
        c.query_row(
            "SELECT source_url, video_id, tool_version FROM provenance",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    assert_eq!(
        (url.as_str(), vid.as_str(), ver.as_deref()),
        (
            "https://music.youtube.com/watch?v=RIGHTTOPIC0",
            "RIGHTTOPIC0",
            Some("2026.01.01")
        )
    );
    let leftovers: Vec<_> = std::fs::read_dir(env.library.path().join("_fetched"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(
        leftovers.is_empty(),
        "the work folder is removed: {leftovers:?}"
    );
    assert_eq!(result["fingerprint"]["fingerprinted"], 1, "{result}");
    // asking again does not download it twice
    let (_, again, _) = fetch_job(&env.app, YT).await;
    assert_eq!(again["outcome"], "skipped", "{again}");
    // a rescan keeps the tags the source gave it
    let (_, scan) = call(
        &env.app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "scan"})),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = scan;
    assert_eq!(
        call(&env.app, Method::GET, "/v1/tracks?q=real%20artist", None)
            .await
            .1["total"],
        1
    );
}

#[tokio::test]
async fn hostile_titles_cannot_leave_the_inbox() {
    let env = env_with("evil", false, "http://unused.example");
    let (state, result, error) = fetch_job(&env.app, YT).await;
    assert_eq!(state, "done", "{error}");
    assert_eq!(result["outcome"], "imported");
    let inbox = env.library.path().join("_fetched");
    let files: Vec<_> = std::fs::read_dir(&inbox)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file())
        .collect();
    assert_eq!(files.len(), 1, "exactly one file, inside the inbox");
    let name = files[0].file_name().to_string_lossy().into_owned();
    assert!(!name.contains('/') && !name.contains('\\'), "{name}");
    assert!(
        !env.library
            .path()
            .parent()
            .unwrap()
            .join("escape.mp3")
            .exists()
    );
    assert!(!env.library.path().join("escape.mp3").exists());
}

#[tokio::test]
async fn tool_failures_fail_the_job_with_a_plain_reason_and_leave_nothing_behind() {
    for (mode, expect) in [
        ("ratelimit", "rate limiting"),
        ("live", "live"),
        ("login", "sign-in"),
    ] {
        let env = env_with(mode, false, "http://unused.example");
        let (state, _, error) = fetch_job(&env.app, YT).await;
        assert_eq!(state, "failed", "{mode}");
        assert!(error.as_str().unwrap().contains(expect), "{mode}: {error}");
        let files = std::fs::read_dir(env.library.path().join("_fetched"))
            .map(|d| d.flatten().count())
            .unwrap_or(0);
        assert_eq!(files, 0, "{mode}: no files, no work folders left");
        assert_eq!(
            call(&env.app, Method::GET, "/v1/tracks", None).await.1["total"],
            0
        );
    }
}

#[tokio::test]
async fn cookies_are_used_only_after_a_sign_in_wall_and_only_as_a_copy() {
    let env = env_with("login", true, "http://unused.example");
    let original = std::fs::read(env.state.fetch.cookies.as_ref().unwrap()).unwrap();
    let (state, result, error) = fetch_job(&env.app, YT).await;
    assert_eq!(state, "done", "{error}");
    assert_eq!(result["outcome"], "imported");
    assert_eq!(
        std::fs::read(env.state.fetch.cookies.as_ref().unwrap()).unwrap(),
        original,
        "the original cookie file is never written"
    );
    let everything = format!(
        "{result}{}",
        call(&env.app, Method::GET, "/v1/fetch/status", None)
            .await
            .1
    );
    assert!(
        !everything.contains("SECRETVALUE") && !everything.contains("cookies.txt"),
        "no cookie content or path in any answer: {everything}"
    );
}

#[tokio::test]
async fn a_download_that_takes_too_long_is_stopped() {
    let env = env_with("slow", false, "http://unused.example");
    let mut cfg = (*env.state.fetch).clone();
    cfg.timeout = Duration::from_millis(600);
    let state = env.state.clone().fetch(cfg);
    let app = api::router(state);
    let started = std::time::Instant::now();
    let (st, _, error) = fetch_job(&app, YT).await;
    assert_eq!(st, "failed");
    assert!(error.as_str().unwrap().contains("too long"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "stopped by the timeout, not by the 30 s sleep"
    );
    // both the script and its child are really gone (the whole process group was killed)
    let tools = env.state.fetch.ytdlp.parent().unwrap().to_path_buf();
    for name in ["slow.pid", "slow-child.pid"] {
        let pid = std::fs::read_to_string(tools.join(name)).unwrap();
        let proc = format!("/proc/{}", pid.trim());
        let mut gone = false;
        for _ in 0..50 {
            gone = !std::path::Path::new(&proc).exists();
            if gone {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(gone, "{name} ({proc}) is still alive");
    }
}

#[tokio::test]
async fn a_spotify_link_is_resolved_matched_and_downloaded() {
    let page = r#"<meta property="og:title" content="Real Song"><meta property="music:musician_description" content="Real Artist"><meta property="music:duration" content="200">"#;
    let spotify = Router::new().route("/track/{id}", get(move || async move { page }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, spotify).await.unwrap() });
    let env = env_with("ok", false, &base);
    let (state, result, error) = fetch_job(
        &env.app,
        "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC",
    )
    .await;
    assert_eq!(state, "done", "{error}");
    assert_eq!(result["outcome"], "imported", "{result}");
    assert_eq!(
        result["matched"]["channel"], "Real Artist - Topic",
        "the live upload is rejected, the Topic upload chosen: {result}"
    );
    assert_eq!(result["video_id"], "RIGHTTOPIC0");
    let src: String = {
        let c = db::lock(&env.state.db);
        c.query_row("SELECT source FROM provenance", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(src, "spotify");
}

#[tokio::test]
async fn a_spotify_page_that_changed_fails_without_downloading_anything() {
    let spotify = Router::new().route("/track/{id}", get(|| async { "<html>a login wall</html>" }));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, spotify).await.unwrap() });
    let env = env_with("ok", false, &base);
    let (state, _, error) = fetch_job(
        &env.app,
        "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC",
    )
    .await;
    assert_eq!(state, "failed");
    assert!(error.as_str().unwrap().contains("Spotify page"), "{error}");
    assert!(
        !env.library.path().join("_fetched").exists()
            || std::fs::read_dir(env.library.path().join("_fetched"))
                .unwrap()
                .count()
                == 0
    );
}

#[tokio::test]
async fn private_hosts_are_refused_for_the_spotify_page() {
    let env = env_with("ok", false, "http://127.0.0.1:9");
    let mut cfg = (*env.state.fetch).clone();
    cfg.allow_private = false;
    let app = api::router(env.state.clone().fetch(cfg));
    let (state, _, error) = fetch_job(
        &app,
        "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC",
    )
    .await;
    assert_eq!(state, "failed");
    assert!(error.as_str().unwrap().contains("not public"), "{error}");
}

#[tokio::test]
async fn only_one_fetch_runs_and_never_beside_a_scan() {
    let env = env_with("ok", false, "http://unused.example");
    db::lock(&env.state.db)
        .execute("INSERT INTO job(kind, state) VALUES('scan', 'running')", [])
        .unwrap();
    let (s, _) = call(
        &env.app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "fetch", "input": {"url": YT}})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
}
