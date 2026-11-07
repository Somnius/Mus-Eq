// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use museq_server::{api, db};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest, http::HeaderValue, protocol::Message,
};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lib")
}

fn state(web: Option<PathBuf>) -> api::AppState {
    let skins = tempfile::tempdir().unwrap().keep();
    // the tests use made-up station addresses, so allow private/unresolvable hosts here
    api::AppState::new(
        db::open_in_memory().unwrap(),
        fixtures(),
        TOKEN.into(),
        skins,
        web,
    )
    .allow_private_stations(true)
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let (p, b) = resp.into_parts();
    (
        p.status,
        p.headers,
        b.collect().await.unwrap().to_bytes().to_vec(),
    )
}

fn bearer(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

async fn serve(app: Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr.to_string()
}

async fn first_track_id(app: &Router) -> String {
    send(
        app,
        bearer(Method::POST, "/v1/jobs", Some(json!({"kind": "scan"}))),
    )
    .await;
    for _ in 0..200 {
        let (_, _, b) = send(app, bearer(Method::GET, "/v1/tracks", None)).await;
        let v: Value = serde_json::from_slice(&b).unwrap();
        if v["total"].as_i64().unwrap_or(0) > 0 {
            return v["items"][0]["id"].as_str().unwrap().to_string();
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("scan produced no tracks");
}

#[tokio::test]
async fn cookie_login_works_and_csrf_guard_applies_to_cookie_auth_only() {
    let app = api::router(state(None));
    let req = |body: &str| {
        Request::builder()
            .method(Method::POST)
            .uri("/v1/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let (s, _, _) = send(
        &app,
        req(&json!({"token": "wrong-wrong-wrong-1"}).to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, h, _) = send(&app, req(&json!({"token": TOKEN}).to_string())).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let set = h
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Strict"),
        "{set}"
    );
    let cookie = set.split(';').next().unwrap().to_string();

    // GET with the cookie works
    let get = Request::builder()
        .uri("/v1/tracks")
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, get).await.0, StatusCode::OK);
    // a form-typed POST with only the cookie is refused (CSRF), JSON is accepted
    let form = Request::builder()
        .method(Method::POST)
        .uri("/v1/playlists")
        .header(header::COOKIE, &cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("name=x"))
        .unwrap();
    assert_eq!(send(&app, form).await.0, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let ok = Request::builder()
        .method(Method::POST)
        .uri("/v1/playlists")
        .header(header::COOKIE, &cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"name": "x"}).to_string()))
        .unwrap();
    assert_eq!(send(&app, ok).await.0, StatusCode::CREATED);
    // a wrong cookie gets nothing
    let bad = Request::builder()
        .uri("/v1/tracks")
        .header(header::COOKIE, "museq_token=nope-nope-nope-nope")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, bad).await.0, StatusCode::UNAUTHORIZED);
    // logout clears it
    let out = Request::builder()
        .method(Method::POST)
        .uri("/v1/logout")
        .header(header::COOKIE, &cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();
    let (s, h, _) = send(&app, out).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(
        h.get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
}

#[tokio::test]
async fn session_report_validates_and_commands_need_a_player() {
    let app = api::router(state(None));
    let tid = first_track_id(&app).await;

    let (_, _, b) = send(&app, bearer(Method::GET, "/v1/session", None)).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&b).unwrap()["state"],
        "stopped"
    );

    // commands with no player
    let (s, _, _) = send(
        &app,
        bearer(
            Method::POST,
            "/v1/session",
            Some(json!({"action": "pause"})),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);

    // bad reports
    for bad in [
        json!({"action": "report", "state": "playing"}), // no client id
        json!({"action": "report", "client_id": "c1", "state": "dancing"}), // bad state
        json!({"action": "report", "client_id": "c1", "volume": 3}), // bad volume
        json!({"action": "report", "client_id": "c1", "track_id": "abc"}), // bad id
    ] {
        let (s, _, _) = send(&app, bearer(Method::POST, "/v1/session", Some(bad.clone()))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (s, _, _) = send(
        &app,
        bearer(
            Method::POST,
            "/v1/session",
            Some(json!({"action": "report", "client_id": "c1", "track_id": "99999"})),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // a good report is reflected, with the track expanded
    let (s, _, _) = send(&app, bearer(Method::POST, "/v1/session", Some(json!({"action": "report", "client_id": "c1", "state": "playing", "track_id": tid, "position_ms": 1234, "volume": 0.5})))).await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, b) = send(&app, bearer(Method::GET, "/v1/session", None)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(
        (
            v["state"].as_str(),
            v["position_ms"].as_i64(),
            v["client_id"].as_str()
        ),
        (Some("playing"), Some(1234), Some("c1"))
    );
    assert_eq!(v["track"]["id"], tid);
    let (s, _, _) = send(
        &app,
        bearer(
            Method::POST,
            "/v1/session",
            Some(json!({"action": "explode"})),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn websocket_gets_snapshot_events_and_commands_and_refuses_foreign_origin() {
    let app = api::router(state(None));
    let tid = first_track_id(&app).await;
    let addr = serve(app).await;
    let http = |method: &str, path: &str, body: Value| {
        let (addr, method, path) = (addr.clone(), method.to_string(), path.to_string());
        async move {
            let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
            let body = body.to_string();
            let req = format!(
                "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            tokio::io::AsyncWriteExt::write_all(&mut s, req.as_bytes())
                .await
                .unwrap();
            let mut out = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut s, &mut out)
                .await
                .unwrap();
            String::from_utf8_lossy(&out)
                .split(' ')
                .nth(1)
                .unwrap()
                .to_string()
        }
    };

    // foreign origin is refused
    let mut bad = format!("ws://{addr}/v1/events")
        .into_client_request()
        .unwrap();
    bad.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
    );
    bad.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_static("http://evil.example"),
    );
    assert!(tokio_tungstenite::connect_async(bad).await.is_err());
    // no token is refused
    assert!(
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/events"))
            .await
            .is_err()
    );

    let mut ok = format!("ws://{addr}/v1/events?client_id=c1")
        .into_client_request()
        .unwrap();
    ok.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
    );
    ok.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_str(&format!("http://{addr}")).unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(ok).await.unwrap();
    let next_json = |m: Message| serde_json::from_str::<Value>(m.to_text().unwrap()).unwrap();

    let snap = next_json(ws.next().await.unwrap().unwrap());
    assert_eq!(
        (snap["type"].as_str(), snap["session"]["state"].as_str()),
        (Some("session"), Some("stopped"))
    );

    assert_eq!(
        http(
            "POST",
            "/v1/session",
            json!({"action": "report", "client_id": "c1", "state": "playing", "track_id": tid})
        )
        .await,
        "200"
    );
    let ev = next_json(ws.next().await.unwrap().unwrap());
    assert_eq!(
        (ev["type"].as_str(), ev["session"]["state"].as_str()),
        (Some("session"), Some("playing"))
    );

    // a command from another client is forwarded to the reporter
    assert_eq!(
        http("POST", "/v1/session", json!({"action": "pause"})).await,
        "202"
    );
    let cmd = next_json(ws.next().await.unwrap().unwrap());
    assert_eq!(
        (
            cmd["type"].as_str(),
            cmd["action"].as_str(),
            cmd["target"].as_str()
        ),
        (Some("command"), Some("pause"), Some("c1"))
    );
}

fn zip_bytes() -> Vec<u8> {
    let mut v = b"PK\x03\x04".to_vec();
    v.extend_from_slice(&[0u8; 64]);
    v
}

#[tokio::test]
async fn skins_upload_list_download_and_validation() {
    let app = api::router(state(None));
    let up = |name: &str, body: Vec<u8>| {
        Request::builder()
            .method(Method::POST)
            .uri(format!("/v1/skins?name={name}"))
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(body))
            .unwrap()
    };
    let (s, _, _) = send(&app, up("x", b"not a zip".to_vec())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _, _) = send(
        &app,
        up(
            "x",
            vec![b'P', b'K', 3, 4]
                .into_iter()
                .chain(std::iter::repeat_n(0u8, 9 * 1024 * 1024))
                .collect(),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);

    let (s, _, b) = send(&app, up("..%2F..%2Fetc%2Fevil.wsz", zip_bytes())).await;
    assert_eq!(s, StatusCode::CREATED);
    let skin: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(skin["name"], "evil", "path parts must be stripped: {skin}");
    let id = skin["id"].as_str().unwrap();

    let (_, _, b) = send(&app, bearer(Method::GET, "/v1/skins", None)).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&b)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let (s, _, got) = send(&app, bearer(Method::GET, &format!("/v1/skins/{id}"), None)).await;
    assert_eq!((s, got), (StatusCode::OK, zip_bytes()));
    for bad in ["..%2F..%2FCargo.toml", "0123456789abcdeg", "short"] {
        let (s, _, _) = send(&app, bearer(Method::GET, &format!("/v1/skins/{bad}"), None)).await;
        assert!(
            s == StatusCode::NOT_FOUND || s == StatusCode::BAD_REQUEST,
            "{bad} -> {s}"
        );
    }
}

#[tokio::test]
async fn web_client_is_served_with_spa_fallback_and_api_stays_protected() {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<html>museq</html>").unwrap();
    std::fs::write(web.path().join("app.js"), "console.log(1)").unwrap();
    let app = api::router(state(Some(web.path().to_path_buf())));
    let get = |p: &str| {
        Request::builder()
            .uri(p.to_string())
            .body(Body::empty())
            .unwrap()
    };
    let (s, _, b) = send(&app, get("/")).await;
    assert_eq!((s, b), (StatusCode::OK, b"<html>museq</html>".to_vec()));
    assert_eq!(send(&app, get("/app.js")).await.0, StatusCode::OK);
    let (s, _, b) = send(&app, get("/some/client/route")).await;
    assert_eq!(
        (s, b),
        (StatusCode::OK, b"<html>museq</html>".to_vec()),
        "SPA fallback"
    );
    assert_eq!(
        send(&app, get("/v1/tracks")).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(send(&app, get("/v1/health")).await.0, StatusCode::OK);
    // traversal must not escape the web dir
    let (s, _, b) = send(&app, get("/..%2F..%2FCargo.toml")).await;
    assert!(!String::from_utf8_lossy(&b).contains("[package]"), "{s}");
}

#[tokio::test]
async fn logout_revokes_the_cookie_and_logins_are_rate_limited() {
    let app = api::router(state(None));
    let login = |token: &str| {
        Request::builder()
            .method(Method::POST)
            .uri("/v1/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"token": token}).to_string()))
            .unwrap()
    };
    let (_, h, _) = send(&app, login(TOKEN)).await;
    let cookie = h
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert!(
        !cookie.contains(TOKEN),
        "the cookie must not be the token itself"
    );
    let get = || {
        Request::builder()
            .uri("/v1/tracks")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(send(&app, get()).await.0, StatusCode::OK);

    // logout needs auth, then the cookie stops working
    let out_unauth = Request::builder()
        .method(Method::POST)
        .uri("/v1/logout")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, out_unauth).await.0, StatusCode::UNAUTHORIZED);
    let out = Request::builder()
        .method(Method::POST)
        .uri("/v1/logout")
        .header(header::COOKIE, &cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, out).await.0, StatusCode::NO_CONTENT);
    assert_eq!(
        send(&app, get()).await.0,
        StatusCode::UNAUTHORIZED,
        "a logged-out cookie must be dead"
    );

    // ten wrong tokens, then even the right one waits
    for _ in 0..10 {
        assert_eq!(
            send(&app, login("wrong-wrong-wrong-1")).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        send(&app, login(TOKEN)).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn commands_need_the_reporting_client_to_be_connected() {
    let app = api::router(state(None));
    let tid = first_track_id(&app).await;
    let addr = serve(app).await;
    let post = |body: Value| {
        let addr = addr.clone();
        async move {
            let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
            let body = body.to_string();
            let req = format!(
                "POST /v1/session HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            tokio::io::AsyncWriteExt::write_all(&mut s, req.as_bytes())
                .await
                .unwrap();
            let mut out = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut s, &mut out)
                .await
                .unwrap();
            String::from_utf8_lossy(&out)
                .split(' ')
                .nth(1)
                .unwrap()
                .to_string()
        }
    };
    let connect = |client: Option<&str>| {
        let url = match client {
            Some(c) => format!("ws://{addr}/v1/events?client_id={c}"),
            None => format!("ws://{addr}/v1/events"),
        };
        let mut r = url.into_client_request().unwrap();
        r.headers_mut().insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
        );
        async move { tokio_tungstenite::connect_async(r).await.unwrap().0 }
    };
    assert_eq!(
        post(json!({"action": "report", "client_id": "p1", "state": "playing", "track_id": tid}))
            .await,
        "200"
    );

    // only a widget is connected (no client id): the player is not there, so 409
    let _widget = connect(None).await;
    assert_eq!(post(json!({"action": "pause"})).await, "409");

    // the player connects: commands are accepted
    let mut player = connect(Some("p1")).await;
    assert_eq!(post(json!({"action": "pause"})).await, "202");

    // the player leaves: back to 409
    player.close(None).await.unwrap();
    drop(player);
    for _ in 0..40 {
        if post(json!({"action": "pause"})).await == "409" {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("commands were still accepted after the player disconnected");
}

#[tokio::test]
async fn unknown_api_paths_are_json_404_even_with_the_web_client_served() {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("index.html"), "<html></html>").unwrap();
    for app in [
        api::router(state(None)),
        api::router(state(Some(web.path().to_path_buf()))),
    ] {
        let (s, _, b) = send(
            &app,
            Request::builder()
                .uri("/v1/nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert!(
            String::from_utf8_lossy(&b).contains("\"error\""),
            "{}",
            String::from_utf8_lossy(&b)
        );
    }
}

#[tokio::test]
async fn a_station_can_be_reported_instead_of_a_track() {
    let app = api::router(state(None));
    let tid = first_track_id(&app).await;
    let (_, _, b) = send(
        &app,
        bearer(
            Method::POST,
            "/v1/stations",
            Some(json!({"name": "Test FM", "url": "http://a.example/s", "category": "News"})),
        ),
    )
    .await;
    let sid = serde_json::from_slice::<Value>(&b).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let report = |extra: Value| {
        let mut body = json!({"action": "report", "client_id": "c1", "state": "playing"});
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        bearer(Method::POST, "/v1/session", Some(body))
    };
    assert_eq!(
        send(&app, report(json!({"station_id": "99999"}))).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, report(json!({"station_id": "x"}))).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&app, report(json!({"station_id": sid, "track_id": tid})))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        send(&app, report(json!({"station_id": sid}))).await.0,
        StatusCode::OK
    );
    let (_, _, b) = send(&app, bearer(Method::GET, "/v1/session", None)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(
        (v["station"]["name"].as_str(), v["track"].clone()),
        (Some("Test FM"), Value::Null)
    );

    // reporting a track afterwards replaces the station
    assert_eq!(
        send(&app, report(json!({"track_id": tid}))).await.0,
        StatusCode::OK
    );
    let (_, _, b) = send(&app, bearer(Method::GET, "/v1/session", None)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(
        (v["station"].clone(), v["track"]["id"].as_str()),
        (Value::Null, Some(tid.as_str()))
    );
}

#[tokio::test]
async fn play_commands_can_name_a_station_or_a_track_and_are_validated() {
    let app = api::router(state(None));
    let tid = first_track_id(&app).await;
    let (_, _, b) = send(
        &app,
        bearer(
            Method::POST,
            "/v1/stations",
            Some(json!({"name": "Cmd FM", "url": "http://a.example/c"})),
        ),
    )
    .await;
    let sid = serde_json::from_slice::<Value>(&b).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, _, b) = send(
        &app,
        bearer(Method::GET, &format!("/v1/stations/{sid}"), None),
    )
    .await;
    assert_eq!(
        (
            s,
            serde_json::from_slice::<Value>(&b).unwrap()["name"].clone()
        ),
        (StatusCode::OK, json!("Cmd FM"))
    );
    assert_eq!(
        send(&app, bearer(Method::GET, "/v1/stations/99999", None))
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    let addr = serve(app).await;
    let post = |body: Value| {
        let addr = addr.clone();
        async move {
            let mut s = tokio::net::TcpStream::connect(&addr).await.unwrap();
            let body = body.to_string();
            let req = format!(
                "POST /v1/session HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            tokio::io::AsyncWriteExt::write_all(&mut s, req.as_bytes())
                .await
                .unwrap();
            let mut out = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut s, &mut out)
                .await
                .unwrap();
            String::from_utf8_lossy(&out)
                .split(' ')
                .nth(1)
                .unwrap()
                .to_string()
        }
    };
    assert_eq!(
        post(json!({"action": "report", "client_id": "p1", "state": "playing", "track_id": tid}))
            .await,
        "200"
    );
    let mut r = format!("ws://{addr}/v1/events?client_id=p1")
        .into_client_request()
        .unwrap();
    r.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(r).await.unwrap();
    let _snapshot = ws.next().await.unwrap().unwrap();

    assert_eq!(
        post(json!({"action": "play", "station_id": sid})).await,
        "202"
    );
    let cmd: Value =
        serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(
        (
            cmd["action"].as_str(),
            cmd["station_id"].as_str(),
            cmd["target"].as_str()
        ),
        (Some("play"), Some(sid.as_str()), Some("p1"))
    );

    assert_eq!(
        post(json!({"action": "play", "station_id": "99999"})).await,
        "404"
    );
    assert_eq!(
        post(json!({"action": "play", "station_id": sid, "track_id": tid})).await,
        "400"
    );
    assert_eq!(
        post(json!({"action": "pause", "station_id": sid})).await,
        "400"
    );
    assert_eq!(
        post(json!({"action": "play", "track_id": "x"})).await,
        "400"
    );
}
