// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use museq_server::{api, db, stations};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789";

fn state(allow_private: bool) -> api::AppState {
    api::AppState::new(
        db::open_in_memory().unwrap(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lib"),
        TOKEN.into(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.work/tmp/test-skins"),
        None,
    )
    .allow_private_stations(allow_private)
}

/// Most tests talk to fake radio servers on 127.0.0.1, so they run with private addresses allowed.
fn app() -> Router {
    api::router(state(true))
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

/// A fake radio server: answers every connection with `head`, then streams `body` bytes and waits.
async fn fake(head: &'static str, body: usize) -> String {
    fake_bytes(head, vec![0x55u8; body]).await
}

async fn fake_bytes(head: &'static str, body: Vec<u8>) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&body).await;
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            });
        }
    });
    format!("http://{addr}/stream")
}

#[test]
fn parse_list_skips_comments_and_bad_lines_and_normalises_categories() {
    let text = "# header\n\nGood FM|Music|http://a.example/s\nNo Url|Music|\nBad Scheme|Music|ftp://x/y\nSpaces|Music|http://a b/c\nnews station|news|https://n.example/live\nOdd|Weird|http://o.example/x\nPiped|Music|http://p.example/a|b\n";
    let got = stations::parse_list(text);
    let names: Vec<_> = got
        .iter()
        .map(|s| (s.name.as_str(), s.category.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("Good FM", "Music"),
            ("news station", "News"),
            ("Odd", "Other"),
            ("Piped", "Music")
        ]
    );
    assert_eq!(
        got[3].url, "http://p.example/a|b",
        "a '|' inside the URL stays part of it"
    );
}

#[test]
fn the_bundled_greek_list_imports_once() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/stations/trayfm-stations.txt");
    let list = stations::parse_list(&std::fs::read_to_string(path).unwrap());
    assert!(list.len() >= 120, "only {} stations parsed", list.len());
    let conn = db::open_in_memory().unwrap();
    let first = stations::import(&conn, "trayfm", &list).unwrap();
    assert!(first >= 120);
    assert_eq!(
        stations::import(&conn, "trayfm", &list).unwrap(),
        0,
        "re-importing must add nothing"
    );
}

#[tokio::test]
async fn add_list_filter_and_delete_stations() {
    let app = app();
    for bad in [
        json!({"name": "", "url": "http://a/b"}),
        json!({"name": "x", "url": "ftp://a/b"}),
        json!({"name": "x", "url": "javascript:alert(1)"}),
        json!({"name": "x", "url": "http://a b"}),
    ] {
        let (s, _) = call(&app, Method::POST, "/v1/stations", Some(bad.clone())).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (s, st) = call(
        &app,
        Method::POST,
        "/v1/stations",
        Some(json!({"name": "Λαϊκός FM", "url": "http://a.example/1", "category": "music"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(
        (
            st["category"].as_str(),
            st["source"].as_str(),
            st["alive"].clone()
        ),
        (Some("Music"), Some("manual"), Value::Null)
    );
    let (s, _) = call(
        &app,
        Method::POST,
        "/v1/stations",
        Some(json!({"name": "dup", "url": "http://a.example/1"})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    call(
        &app,
        Method::POST,
        "/v1/stations",
        Some(json!({"name": "Sport X", "url": "http://a.example/2", "category": "Sports"})),
    )
    .await;

    let (_, all) = call(&app, Method::GET, "/v1/stations", None).await;
    assert_eq!(all["total"], 2);
    let (_, g) = call(
        &app,
        Method::GET,
        "/v1/stations?q=%CE%BB%CE%B1%CE%B9%CE%BA%CE%BF%CF%82",
        None,
    )
    .await; // "λαικοσ"
    assert_eq!(g["total"], 1, "accent/case-insensitive Greek search: {g}");
    let (_, sp) = call(&app, Method::GET, "/v1/stations?category=sports", None).await;
    assert_eq!(sp["items"][0]["name"], "Sport X");
    let (_, unk) = call(&app, Method::GET, "/v1/stations?alive=unknown", None).await;
    assert_eq!(unk["total"], 2);
    let (s, _) = call(&app, Method::GET, "/v1/stations?alive=maybe", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let id = st["id"].as_str().unwrap();
    assert_eq!(
        call(&app, Method::DELETE, &format!("/v1/stations/{id}"), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, Method::DELETE, &format!("/v1/stations/{id}"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_liveness_check_sorts_live_from_dead_without_crashing() {
    let live = fake(
        "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nConnection: close\r\n\r\n",
        4096,
    )
    .await;
    let icy = fake("ICY 200 OK\r\nicy-name: Old Shoutcast\r\n\r\n", 4096).await;
    let html = fake(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n",
        64,
    )
    .await;
    let gone = fake("HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n", 0).await;
    let refused = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}/x", l.local_addr().unwrap()) // listener dropped: connection refused
    };
    // a plain-http address that redirects to the real stream (as the Omarchy one does) is alive
    let redirect_head: &'static str = Box::leak(
        format!("HTTP/1.1 301 Moved Permanently\r\nLocation: {live}\r\nConnection: close\r\n\r\n")
            .into_boxed_str(),
    );
    let redirect = fake(redirect_head, 0).await;
    let app = app();
    for (name, url) in [
        ("live", &live),
        ("icy", &icy),
        ("html", &html),
        ("gone", &gone),
        ("refused", &refused),
        ("redirect", &redirect),
    ] {
        let (s, _) = call(
            &app,
            Method::POST,
            "/v1/stations",
            Some(json!({"name": name, "url": url})),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{name}");
    }
    let (s, job) = call(
        &app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "check_stations"})),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let uri = format!("/v1/jobs/{}", job["id"].as_str().unwrap());
    let mut result = Value::Null;
    for _ in 0..400 {
        let (_, j) = call(&app, Method::GET, &uri, None).await;
        if j["state"] == "done" {
            result = j["result"].clone();
            break;
        }
        assert_ne!(j["state"], "failed", "{j}");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        result,
        json!({"checked": 6, "alive": 3, "dead": 3}),
        "{result}"
    );

    let (_, all) = call(&app, Method::GET, "/v1/stations?limit=10", None).await;
    assert!(all["items"].is_array(), "list answered: {all}");
    let by_name = |n: &str| {
        all["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == n)
            .unwrap()
            .clone()
    };
    assert_eq!(by_name("live")["alive"], true);
    assert_eq!(
        by_name("icy")["alive"],
        true,
        "ICY (old Shoutcast) servers count as alive"
    );
    assert_eq!(
        by_name("redirect")["alive"],
        true,
        "a redirect to a live stream is alive"
    );
    assert_eq!(by_name("html")["alive"], false);
    assert!(
        by_name("html")["error"]
            .as_str()
            .unwrap()
            .contains("not audio")
    );
    assert_eq!(by_name("gone")["error"], "HTTP 404");
    assert_eq!(by_name("refused")["alive"], false);
    assert!(by_name("live")["checked_at"].as_i64().unwrap() > 0);
    let (_, alive) = call(&app, Method::GET, "/v1/stations?alive=true", None).await;
    assert_eq!(alive["total"], 3);
}

#[tokio::test]
async fn a_second_station_check_is_refused_while_one_runs() {
    let state = api::AppState::new(
        db::open_in_memory().unwrap(),
        PathBuf::from("."),
        TOKEN.into(),
        PathBuf::from("."),
        None,
    );
    db::lock(&state.db)
        .execute(
            "INSERT INTO job(kind, state) VALUES('check_stations', 'running')",
            [],
        )
        .unwrap();
    let app = api::router(state);
    let (s, _) = call(
        &app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "check_stations"})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[test]
fn only_public_addresses_count_as_public() {
    use std::net::IpAddr;
    for (ip, public) in [
        ("93.184.216.34", true),
        ("8.8.8.8", true),
        ("2606:4700:4700::1111", true),
        ("127.0.0.1", false),
        ("10.1.2.3", false),
        ("172.16.0.1", false),
        ("192.168.1.10", false),
        ("169.254.169.254", false),
        ("100.64.0.1", false),
        ("0.0.0.0", false),
        ("224.0.0.1", false),
        ("::1", false),
        ("fe80::1", false),
        ("fd00::1", false),
        ("::ffff:127.0.0.1", false),
        ("::ffff:93.184.216.34", true),
    ] {
        assert_eq!(
            stations::is_public(ip.parse::<IpAddr>().unwrap()),
            public,
            "{ip}"
        );
    }
}

#[test]
fn stream_urls_need_a_host_and_no_credentials() {
    for bad in [
        "http://",
        "https://",
        "ftp://a/b",
        "javascript:alert(1)",
        "http://user:pw@a.example/x",
        "http://u@a.example/x",
        "http://a b/c",
        "//a.example/x",
    ] {
        assert!(stations::parse_stream_url(bad).is_none(), "{bad}");
    }
    assert!(stations::parse_stream_url("https://radio.example/stream?x=1").is_some());
    assert!(stations::parse_stream_url("http://[2606:4700::1111]:8000/live").is_some());
}

#[test]
fn audio_sniffing_recognises_streams_and_rejects_pages() {
    assert!(stations::sniff_audio(b"ID3\x04\x00"));
    assert!(stations::sniff_audio(&[0xFF, 0xFB, 0x90, 0x00]));
    assert!(stations::sniff_audio(b"OggS\x00"));
    assert!(stations::sniff_audio(b"#EXTM3U\n"));
    assert!(!stations::sniff_audio(b"<html><body>"));
    assert!(!stations::sniff_audio(b"UUUUUUUU"));
    assert!(!stations::sniff_audio(b""));
}

#[tokio::test]
async fn by_default_private_and_loopback_stations_are_refused() {
    let app = api::router(state(false));
    for bad in [
        "http://127.0.0.1:8000/x",
        "http://localhost/x",
        "http://10.0.0.5/x",
        "http://169.254.169.254/latest/meta-data",
        "http://[::1]:8000/x",
        "http://192.168.1.2/radio",
    ] {
        let (s, body) = call(
            &app,
            Method::POST,
            "/v1/stations",
            Some(json!({"name": "x", "url": bad})),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    let (s, body) = call(
        &app,
        Method::POST,
        "/v1/stations",
        Some(json!({"name": "x", "url": "http://user:pw@8.8.8.8/x"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    // a public literal address is fine (no DNS needed)
    let (s, _) = call(
        &app,
        Method::POST,
        "/v1/stations",
        Some(json!({"name": "ok", "url": "http://93.184.216.34/stream"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
}

#[tokio::test]
async fn the_check_will_not_probe_private_hosts_even_if_one_got_into_the_database() {
    let live = fake(
        "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nConnection: close\r\n\r\n",
        2048,
    )
    .await;
    let st = state(false);
    db::lock(&st.db)
        .execute("INSERT INTO station(name, category, url, source, search_text) VALUES('inside','Music',?1,'manual','inside')", [&live])
        .unwrap();
    let app = api::router(st);
    let (_, job) = call(
        &app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "check_stations"})),
    )
    .await;
    let uri = format!("/v1/jobs/{}", job["id"].as_str().unwrap());
    for _ in 0..200 {
        let (_, j) = call(&app, Method::GET, &uri, None).await;
        if j["state"] == "done" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let (_, all) = call(&app, Method::GET, "/v1/stations", None).await;
    assert_eq!(all["items"][0]["alive"], false);
    assert_eq!(all["items"][0]["error"], "address is not public");
}

#[tokio::test]
async fn streams_without_a_content_type_must_look_like_audio() {
    let mp3 = fake_bytes(
        "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n",
        vec![0xFF, 0xFB, 0x90, 0x00, 1, 2, 3],
    )
    .await;
    let junk = fake("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n", 512).await; // 0x55 bytes
    let app = app();
    for (name, url) in [("mp3", &mp3), ("junk", &junk)] {
        call(
            &app,
            Method::POST,
            "/v1/stations",
            Some(json!({"name": name, "url": url})),
        )
        .await;
    }
    let (_, job) = call(
        &app,
        Method::POST,
        "/v1/jobs",
        Some(json!({"kind": "check_stations"})),
    )
    .await;
    let uri = format!("/v1/jobs/{}", job["id"].as_str().unwrap());
    for _ in 0..200 {
        let (_, j) = call(&app, Method::GET, &uri, None).await;
        if j["state"] == "done" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let (_, all) = call(&app, Method::GET, "/v1/stations", None).await;
    let get = |n: &str| {
        all["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == n)
            .unwrap()
            .clone()
    };
    assert_eq!(get("mp3")["alive"], true);
    assert_eq!(get("junk")["alive"], false);
    assert!(
        get("junk")["error"]
            .as_str()
            .unwrap()
            .contains("do not look like audio")
    );
}
