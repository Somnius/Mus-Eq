//! The tool installer: nothing is downloaded without the typed confirmation, the pinned SHA-256 decides
//! what is accepted, and a failed install leaves nothing behind. A local server stands in for GitHub.

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use http_body_util::BodyExt;
use museq_server::tools::{Archive, Asset, ExtraCheck, ToolSpec, Tools};
use museq_server::{api, db};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789";
const NAME: &str = "faketool";

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A tar.xz holding `<top>/bin/<NAME>` with the given script text.
fn tar_xz(script: &str, member: &str) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_size(script.len() as u64);
    h.set_mode(0o755);
    h.set_cksum();
    tar.append_data(&mut h, member, script.as_bytes()).unwrap();
    let raw = tar.into_inner().unwrap();
    use std::io::Write;
    let mut enc = xz2::write::XzEncoder::new(Vec::new(), 1);
    enc.write_all(&raw).unwrap();
    enc.finish().unwrap()
}

const GOOD_SCRIPT: &str = "#!/bin/sh\necho \"faketool version t1\"\n";

async fn serve(router: Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(l, router).await.unwrap();
    });
    format!("http://127.0.0.1:{}", addr.port())
}

fn spec(base: &str, path: &str, archive_bytes: &[u8], archive: Archive) -> ToolSpec {
    ToolSpec {
        name: NAME.into(),
        purpose: "to test the installer".into(),
        version: "t1".into(),
        licence: "MIT".into(),
        homepage: "https://example.invalid".into(),
        binary: NAME.into(),
        check_args: vec!["--version".into()],
        check_prefix: "faketool version".into(),
        extra_checks: vec![],
        hosts: vec!["127.0.0.1".into()],
        notes: String::new(),
        assets: vec![Asset {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            url: format!("{base}{path}"),
            sha256: sha(archive_bytes),
            bytes: archive_bytes.len() as u64,
            archive,
        }],
    }
}

fn app(tools: Tools) -> Router {
    api::router(
        api::AppState::new(
            db::open_in_memory().unwrap(),
            std::env::temp_dir(),
            TOKEN.into(),
            std::env::temp_dir().join("museq-test-skins-tools"),
            None,
        )
        .tools(tools),
    )
}

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    token: bool,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
    }
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (
        parts.status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn tool(v: &Value) -> &Value {
    v.as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == NAME)
        .unwrap()
}

/// Polls until the install is done or failed.
async fn wait_finished(app: &Router) -> Value {
    for _ in 0..200 {
        let (_, list) = call(app, Method::GET, "/v1/tools", true, None).await;
        let t = tool(&list).clone();
        if matches!(t["progress"]["phase"].as_str(), Some("done" | "failed")) {
            return t;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the install did not finish");
}

fn no_leftovers(dir: &Path) {
    assert!(
        !dir.join(NAME).join("t1").join(NAME).exists(),
        "a program was installed"
    );
    assert!(
        !dir.join(format!(".download-{NAME}.part")).exists(),
        "a partial download is left"
    );
    assert!(
        !dir.join(NAME)
            .join("t1")
            .join(format!("{NAME}.new"))
            .exists(),
        "an unchecked program is left"
    );
}

fn dir() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("tools");
    (d, p)
}

#[tokio::test]
async fn the_list_describes_what_would_be_downloaded_and_needs_the_token() {
    let archive = tar_xz(GOOD_SCRIPT, "x/bin/faketool");
    let base = serve(Router::new()).await;
    let (_keep, d) = dir();
    let app = app(Tools::new(
        d,
        vec![spec(
            &base,
            "/t.tar.xz",
            &archive,
            Archive::TarXz {
                member: "/bin/faketool".into(),
            },
        )],
    )
    .insecure_for_tests());

    let (s, _) = call(&app, Method::GET, "/v1/tools", false, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/v1/tools/{NAME}/install"),
        false,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let (s, list) = call(&app, Method::GET, "/v1/tools", true, None).await;
    assert_eq!(s, StatusCode::OK);
    let t = tool(&list);
    let msg = t["message"].as_str().unwrap();
    assert!(
        msg.contains("to test the installer")
            && msg.contains("127.0.0.1")
            && msg.contains(&sha(&archive))
    );
    assert!(msg.contains("Nothing is downloaded until you confirm"));
    assert_eq!(t["confirm_phrase"], "install faketool t1");
    assert_eq!(t["found"], "missing");
    assert_eq!(t["progress"]["phase"], "idle");
}

#[tokio::test]
async fn nothing_is_downloaded_without_the_exact_confirmation() {
    let archive = tar_xz(GOOD_SCRIPT, "x/bin/faketool");
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let h = hits.clone();
    let a = archive.clone();
    let base = serve(Router::new().route(
        "/t.tar.xz",
        get(move || {
            h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let a = a.clone();
            async move { a }
        }),
    ))
    .await;
    let (_keep, d) = dir();
    let app = app(Tools::new(
        d.clone(),
        vec![spec(
            &base,
            "/t.tar.xz",
            &archive,
            Archive::TarXz {
                member: "/bin/faketool".into(),
            },
        )],
    )
    .insecure_for_tests());
    let uri = format!("/v1/tools/{NAME}/install");

    // no body, an empty phrase, a near miss, another tool's phrase: all refused with the explanation
    for body in [
        None,
        Some(json!({})),
        Some(json!({"confirm": ""})),
        Some(json!({"confirm": "install faketool"})),
        Some(json!({"confirm": "yes"})),
        Some(json!({"confirm": "install faketool t2"})),
    ] {
        let (s, v) = call(&app, Method::POST, &uri, true, body).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let e = v["error"].as_str().unwrap();
        assert!(
            e.contains("not confirmed")
                && e.contains("install faketool t1")
                && e.contains("127.0.0.1"),
            "{e}"
        );
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "something was fetched without confirmation"
    );
    assert!(!d.exists() || !d.join(NAME).exists());

    let (s, _) = call(
        &app,
        Method::POST,
        "/v1/tools/nope/install",
        true,
        Some(json!({"confirm": "x"})),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_confirmed_install_downloads_checks_unpacks_and_runs_the_program() {
    let archive = tar_xz(GOOD_SCRIPT, "faketool-top/bin/faketool");
    let a = archive.clone();
    let base = serve(Router::new().route(
        "/t.tar.xz",
        get(move || {
            let a = a.clone();
            async move { a }
        }),
    ))
    .await;
    let (_keep, d) = dir();
    let tools = Tools::new(
        d.clone(),
        vec![spec(
            &base,
            "/t.tar.xz",
            &archive,
            Archive::TarXz {
                member: "/bin/faketool".into(),
            },
        )],
    )
    .insecure_for_tests();
    let app = app(tools);

    let (s, v) = call(
        &app,
        Method::POST,
        &format!("/v1/tools/{NAME}/install"),
        true,
        Some(json!({"confirm": "install faketool t1"})),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    let t = wait_finished(&app).await;
    assert_eq!(t["progress"]["phase"], "done", "{t}");
    assert_eq!(t["found"], "installed");
    let path = PathBuf::from(t["path"].as_str().unwrap());
    assert!(path.starts_with(&d) && path.is_file());

    // it is the program from the archive, and it runs
    let out = std::process::Command::new(&path)
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("faketool version t1"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o755);
    }
    // the record names what was fetched and its hashes
    let rec: Value = serde_json::from_str(
        &std::fs::read_to_string(path.with_file_name("installed.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(rec["archive_sha256"], sha(&archive));
    assert_eq!(rec["binary_sha256"].as_str().unwrap().len(), 64);
    no_leftovers_except_program(&d);

    // a second install while installed is fine (it replaces), and removal takes it away
    let (s, _) = call(
        &app,
        Method::DELETE,
        &format!("/v1/tools/{NAME}"),
        true,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(!path.exists());
    let (_, list) = call(&app, Method::GET, "/v1/tools", true, None).await;
    assert_eq!(tool(&list)["found"], "missing");
}

fn no_leftovers_except_program(dir: &Path) {
    assert!(
        !dir.join(format!(".download-{NAME}.part")).exists(),
        "a partial download is left"
    );
    assert!(
        !dir.join(NAME)
            .join("t1")
            .join(format!("{NAME}.new"))
            .exists()
    );
}

#[tokio::test]
async fn a_download_that_does_not_match_the_pinned_hash_installs_nothing() {
    let pinned = tar_xz(GOOD_SCRIPT, "x/bin/faketool");
    let served = tar_xz(
        "#!/bin/sh\necho \"faketool version evil\"\n",
        "x/bin/faketool",
    );
    assert_ne!(sha(&pinned), sha(&served));
    let s2 = served.clone();
    let base = serve(Router::new().route(
        "/t.tar.xz",
        get(move || {
            let a = s2.clone();
            async move { a }
        }),
    ))
    .await;
    let (_keep, d) = dir();
    // the pin is for `pinned`; the server hands out `served` of any size
    let mut sp = spec(
        &base,
        "/t.tar.xz",
        &pinned,
        Archive::TarXz {
            member: "/bin/faketool".into(),
        },
    );
    sp.assets[0].bytes = served.len() as u64;
    let app = app(Tools::new(d.clone(), vec![sp]).insecure_for_tests());

    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/v1/tools/{NAME}/install"),
        true,
        Some(json!({"confirm": "install faketool t1"})),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let t = wait_finished(&app).await;
    assert_eq!(t["progress"]["phase"], "failed");
    assert!(
        t["progress"]["error"]
            .as_str()
            .unwrap()
            .contains("pinned SHA-256"),
        "{t}"
    );
    assert_eq!(t["found"], "missing");
    no_leftovers(&d);
}

#[tokio::test]
async fn a_download_larger_than_the_pinned_size_is_refused() {
    let small = tar_xz(GOOD_SCRIPT, "x/bin/faketool");
    let huge = vec![7u8; 3 * 1024 * 1024];
    let base = serve(Router::new().route(
        "/t.tar.xz",
        get(move || {
            let a = huge.clone();
            async move { a }
        }),
    ))
    .await;
    let (_keep, d) = dir();
    let app = app(Tools::new(
        d.clone(),
        vec![spec(
            &base,
            "/t.tar.xz",
            &small,
            Archive::TarXz {
                member: "/bin/faketool".into(),
            },
        )],
    )
    .insecure_for_tests());
    call(
        &app,
        Method::POST,
        &format!("/v1/tools/{NAME}/install"),
        true,
        Some(json!({"confirm": "install faketool t1"})),
    )
    .await;
    let t = wait_finished(&app).await;
    assert_eq!(t["progress"]["phase"], "failed");
    assert!(
        t["progress"]["error"]
            .as_str()
            .unwrap()
            .contains("larger than the pinned size"),
        "{t}"
    );
    no_leftovers(&d);
}

#[tokio::test]
async fn an_archive_without_the_program_or_a_program_that_does_not_identify_itself_is_refused() {
    // right hash, wrong content: the archive has no /bin/faketool
    let no_member = tar_xz(GOOD_SCRIPT, "x/bin/other");
    // right hash, the program runs but says something else
    let liar = tar_xz("#!/bin/sh\necho \"something else\"\n", "x/bin/faketool");
    // right hash, the program fails
    let broken = tar_xz("#!/bin/sh\nexit 3\n", "x/bin/faketool");
    for (archive, expect) in [
        (no_member, "has no /bin/faketool"),
        (liar, "did not identify itself"),
        (broken, "exited with"),
    ] {
        let a = archive.clone();
        let base = serve(Router::new().route(
            "/t.tar.xz",
            get(move || {
                let a = a.clone();
                async move { a }
            }),
        ))
        .await;
        let (_keep, d) = dir();
        let app = app(Tools::new(
            d.clone(),
            vec![spec(
                &base,
                "/t.tar.xz",
                &archive,
                Archive::TarXz {
                    member: "/bin/faketool".into(),
                },
            )],
        )
        .insecure_for_tests());
        call(
            &app,
            Method::POST,
            &format!("/v1/tools/{NAME}/install"),
            true,
            Some(json!({"confirm": "install faketool t1"})),
        )
        .await;
        let t = wait_finished(&app).await;
        assert_eq!(t["progress"]["phase"], "failed", "{t}");
        assert!(
            t["progress"]["error"].as_str().unwrap().contains(expect),
            "{expect}: {t}"
        );
        assert_eq!(t["found"], "missing");
        no_leftovers(&d);
    }
}

#[tokio::test]
async fn a_redirect_to_a_host_that_is_not_allowed_is_refused() {
    let archive = tar_xz(GOOD_SCRIPT, "x/bin/faketool");
    let a = archive.clone();
    let real = serve(Router::new().route(
        "/t.tar.xz",
        get(move || {
            let a = a.clone();
            async move { a }
        }),
    ))
    .await;
    // the pinned address redirects to the same server under another host name ("localhost" is not on the list)
    let target = real.replace("127.0.0.1", "localhost");
    let base = serve(Router::new().route(
        "/t.tar.xz",
        get(move || {
            let loc = format!("{target}/t.tar.xz");
            async move { (StatusCode::FOUND, [(header::LOCATION, loc)]).into_response() }
        }),
    ))
    .await;
    let (_keep, d) = dir();
    let app = app(Tools::new(
        d.clone(),
        vec![spec(
            &base,
            "/t.tar.xz",
            &archive,
            Archive::TarXz {
                member: "/bin/faketool".into(),
            },
        )],
    )
    .insecure_for_tests());
    call(
        &app,
        Method::POST,
        &format!("/v1/tools/{NAME}/install"),
        true,
        Some(json!({"confirm": "install faketool t1"})),
    )
    .await;
    let t = wait_finished(&app).await;
    assert_eq!(t["progress"]["phase"], "failed", "{t}");
    assert!(
        t["progress"]["error"]
            .as_str()
            .unwrap()
            .contains("download failed"),
        "{t}"
    );
    no_leftovers(&d);
}

#[tokio::test]
async fn a_second_install_while_one_runs_is_refused() {
    let archive = tar_xz(GOOD_SCRIPT, "x/bin/faketool");
    let a = archive.clone();
    let base = serve(Router::new().route(
        "/t.tar.xz",
        get(move || {
            let a = a.clone();
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                a
            }
        }),
    ))
    .await;
    let (_keep, d) = dir();
    let app = app(Tools::new(
        d,
        vec![spec(
            &base,
            "/t.tar.xz",
            &archive,
            Archive::TarXz {
                member: "/bin/faketool".into(),
            },
        )],
    )
    .insecure_for_tests());
    let uri = format!("/v1/tools/{NAME}/install");
    let body = Some(json!({"confirm": "install faketool t1"}));
    let (s, _) = call(&app, Method::POST, &uri, true, body.clone()).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let (s, v) = call(&app, Method::POST, &uri, true, body).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let (s, _) = call(
        &app,
        Method::DELETE,
        &format!("/v1/tools/{NAME}"),
        true,
        None,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "removal during an install must wait"
    );
    assert_eq!(wait_finished(&app).await["progress"]["phase"], "done");
}

#[tokio::test]
async fn downloads_can_be_switched_off_and_a_missing_platform_build_is_said_plainly() {
    let archive = tar_xz(GOOD_SCRIPT, "x/bin/faketool");
    let base = serve(Router::new()).await;
    let (_keep, d) = dir();
    let sp = spec(
        &base,
        "/t.tar.xz",
        &archive,
        Archive::TarXz {
            member: "/bin/faketool".into(),
        },
    );
    let off = app(Tools::new(d.clone(), vec![sp.clone()])
        .insecure_for_tests()
        .downloads_allowed(false));
    let (s, v) = call(
        &off,
        Method::POST,
        &format!("/v1/tools/{NAME}/install"),
        true,
        Some(json!({"confirm": "install faketool t1"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("MUSEQ_TOOLS_DOWNLOADS=off")
    );

    let mut other = sp;
    other.assets[0].arch = "no-such-arch".into();
    let app2 = app(Tools::new(d, vec![other]).insecure_for_tests());
    let (_, list) = call(&app2, Method::GET, "/v1/tools", true, None).await;
    assert_eq!(tool(&list)["platform_supported"], false);
    assert!(tool(&list)["message"].is_null());
    let (s, v) = call(
        &app2,
        Method::POST,
        &format!("/v1/tools/{NAME}/install"),
        true,
        Some(json!({"confirm": "install faketool t1"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("pins no faketool build")
    );
}

#[tokio::test]
async fn a_program_the_owner_points_to_wins_over_an_installed_one() {
    let archive = tar_xz(GOOD_SCRIPT, "x/bin/faketool");
    let a = archive.clone();
    let base = serve(Router::new().route(
        "/t.tar.xz",
        get(move || {
            let a = a.clone();
            async move { a }
        }),
    ))
    .await;
    let (_keep, d) = dir();
    let tools = Tools::new(
        d.clone(),
        vec![spec(
            &base,
            "/t.tar.xz",
            &archive,
            Archive::TarXz {
                member: "/bin/faketool".into(),
            },
        )],
    )
    .insecure_for_tests();
    assert_eq!(
        tools.resolve(NAME),
        NAME,
        "nothing found: the bare name is looked up on PATH"
    );
    tools.set_explicit(NAME, "/opt/mine/faketool".into());
    assert_eq!(tools.resolve(NAME), "/opt/mine/faketool");
    let (_, list) = call(&app(tools), Method::GET, "/v1/tools", true, None).await;
    assert_eq!(tool(&list)["found"], "configured");
    assert_eq!(tool(&list)["path"], "/opt/mine/faketool");
}

#[tokio::test]
async fn a_build_without_the_encoders_mus_eq_needs_is_refused() {
    // runs and identifies itself, but its encoder list lacks libopus
    let script = "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"faketool version t1\"; else echo \"libmp3lame only\"; fi\n";
    let archive = tar_xz(script, "x/bin/faketool");
    let a = archive.clone();
    let base = serve(Router::new().route(
        "/t.tar.xz",
        get(move || {
            let a = a.clone();
            async move { a }
        }),
    ))
    .await;
    let (_keep, d) = dir();
    let mut sp = spec(
        &base,
        "/t.tar.xz",
        &archive,
        Archive::TarXz {
            member: "/bin/faketool".into(),
        },
    );
    sp.extra_checks = vec![ExtraCheck {
        args: vec!["-encoders".into()],
        contains: vec!["libopus".into(), "libmp3lame".into()],
    }];
    let app = app(Tools::new(d.clone(), vec![sp]).insecure_for_tests());
    call(
        &app,
        Method::POST,
        &format!("/v1/tools/{NAME}/install"),
        true,
        Some(json!({"confirm": "install faketool t1"})),
    )
    .await;
    let t = wait_finished(&app).await;
    assert_eq!(t["progress"]["phase"], "failed", "{t}");
    assert!(
        t["progress"]["error"]
            .as_str()
            .unwrap()
            .contains("lacks what Mus-Eq needs (libopus)"),
        "{t}"
    );
    no_leftovers(&d);
}

#[tokio::test]
async fn an_installed_file_that_no_longer_matches_its_record_is_not_used() {
    let archive = tar_xz(GOOD_SCRIPT, "x/bin/faketool");
    let a = archive.clone();
    let base = serve(Router::new().route(
        "/t.tar.xz",
        get(move || {
            let a = a.clone();
            async move { a }
        }),
    ))
    .await;
    let (_keep, d) = dir();
    let tools = Arc::new(
        Tools::new(
            d.clone(),
            vec![spec(
                &base,
                "/t.tar.xz",
                &archive,
                Archive::TarXz {
                    member: "/bin/faketool".into(),
                },
            )],
        )
        .insecure_for_tests(),
    );
    let app = api::router(
        api::AppState::new(
            db::open_in_memory().unwrap(),
            std::env::temp_dir(),
            TOKEN.into(),
            std::env::temp_dir().join("museq-test-skins-tools"),
            None,
        )
        .tools_shared(tools.clone()),
    );
    call(
        &app,
        Method::POST,
        &format!("/v1/tools/{NAME}/install"),
        true,
        Some(json!({"confirm": "install faketool t1"})),
    )
    .await;
    let t = wait_finished(&app).await;
    assert_eq!(t["found"], "installed");
    let path = PathBuf::from(t["path"].as_str().unwrap());
    assert_eq!(tools.resolve(NAME), path.to_string_lossy());

    // somebody replaces the program: it is reported, and no longer run
    std::fs::write(&path, "#!/bin/sh\necho replaced\n").unwrap();
    let (_, list) = call(&app, Method::GET, "/v1/tools", true, None).await;
    assert_eq!(tool(&list)["found"], "modified");
    assert_eq!(
        tools.resolve(NAME),
        NAME,
        "a modified program must not be the one that runs"
    );
    assert!(tools.installed_binary(NAME).is_none());
}

#[tokio::test]
async fn fetch_uses_the_installed_yt_dlp_and_says_plainly_when_there_is_none() {
    let script = "#!/bin/sh\necho \"2026.1.1\"\n";
    let a = script.as_bytes().to_vec();
    let a2 = a.clone();
    let base = serve(Router::new().route(
        "/yt-dlp",
        get(move || {
            let a = a2.clone();
            async move { a }
        }),
    ))
    .await;
    let (_keep, d) = dir();
    let mut sp = spec(&base, "/yt-dlp", &a, Archive::Raw);
    sp.name = "yt-dlp".into();
    sp.binary = "yt-dlp".into();
    sp.check_args = vec!["--version".into()];
    sp.check_prefix = "2026.".into();
    let tools = Arc::new(Tools::new(d.clone(), vec![sp]).insecure_for_tests());
    let state = api::AppState::new(
        db::open_in_memory().unwrap(),
        std::env::temp_dir(),
        TOKEN.into(),
        std::env::temp_dir().join("museq-test-skins-tools"),
        None,
    )
    .fetch(museq_server::fetch::FetchConfig {
        enabled: true,
        ..Default::default()
    })
    .tools_shared(tools.clone());
    assert!(state.ytdlp_path().is_none(), "nothing installed yet");
    let app = api::router(state.clone());

    // fetch is on but yt-dlp is not there: the job is refused with the way out, not queued to fail later
    let (s, v) = call(&app, Method::POST, "/v1/jobs", true, Some(json!({"kind": "fetch", "input": {"url": "https://music.youtube.com/watch?v=abcdefghijk"}}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("not installed"),
        "{v}"
    );

    // after the owner installs it, fetch finds it (and still never looks on PATH)
    let (s, _) = call(
        &app,
        Method::POST,
        "/v1/tools/yt-dlp/install",
        true,
        Some(json!({"confirm": "install yt-dlp t1"})),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    for _ in 0..200 {
        if state.ytdlp_path().is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let p = state.ytdlp_path().expect("the installed yt-dlp is found");
    assert!(p.starts_with(&d) && p.ends_with("yt-dlp"));
}
