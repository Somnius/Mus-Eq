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

fn app() -> Router {
    api::router(api::AppState::new(
        db::open_in_memory().unwrap(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lib"),
        TOKEN.into(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.work/tmp/test-skins"),
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

/// A fake radio server: answers every connection with `head`, then streams `body` bytes and waits.
async fn fake(head: &'static str, body: usize) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&vec![0x55u8; body]).await;
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
