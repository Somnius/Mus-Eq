//! Fetching a track into the library: one YouTube watch link, or one Spotify track link that is
//! matched to a YouTube Music audio upload. OFF by default.
//!
//! Turning it on needs `MUSEQ_FETCH=on`, an absolute path to a pinned `yt-dlp` and a typed
//! acknowledgement (`MUSEQ_FETCH_ACK`) that the owner has read the terms of the services involved
//! and fetches only what they may store. The server never builds a command line from the pasted
//! text: yt-dlp only ever receives a `watch?v=<id>` address that this module assembled itself.
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

use crate::api::AppState;
use crate::{db, stations};

/// The sentence the owner has to put in `MUSEQ_FETCH_ACK` (documented in the README and SPEC).
pub const ACK_PHRASE: &str = "i-have-read-the-terms-and-fetch-only-what-i-may-store";

#[derive(Clone, Debug)]
pub struct FetchConfig {
    pub enabled: bool,
    /// Why it is off, when it was asked for but cannot start (never contains secrets).
    pub disabled_reason: Option<String>,
    pub ytdlp: PathBuf,
    pub inbox: PathBuf,
    pub max_bytes: u64,
    pub timeout: Duration,
    pub cookies: Option<PathBuf>,
    /// Where Spotify track pages are read from (replaced by a fake server in tests).
    pub spotify_base: String,
    /// Allow private addresses for the Spotify base (tests only).
    pub allow_private: bool,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            disabled_reason: Some(
                "fetch is off (set MUSEQ_FETCH=on and MUSEQ_FETCH_ACK to turn it on)".into(),
            ),
            ytdlp: PathBuf::new(),
            inbox: PathBuf::new(),
            max_bytes: 200 * 1024 * 1024,
            timeout: Duration::from_secs(900),
            cookies: None,
            spotify_base: "https://open.spotify.com".into(),
            allow_private: false,
        }
    }
}

impl FetchConfig {
    pub fn from_env(library: &Path) -> Self {
        let mut c = Self {
            inbox: library.join("_fetched"),
            ..Self::default()
        };
        if std::env::var("MUSEQ_FETCH").as_deref() != Ok("on") {
            return c;
        }
        let off = |c: &mut Self, why: &str| {
            c.enabled = false;
            c.disabled_reason = Some(why.to_string());
        };
        if std::env::var("MUSEQ_FETCH_ACK").as_deref() != Ok(ACK_PHRASE) {
            off(
                &mut c,
                &format!(
                    "MUSEQ_FETCH=on needs MUSEQ_FETCH_ACK={ACK_PHRASE} (read the terms first)"
                ),
            );
            return c;
        }
        let Some(path) = std::env::var_os("MUSEQ_FETCH_YTDLP")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_file())
        else {
            off(
                &mut c,
                "MUSEQ_FETCH_YTDLP must be the absolute path of an existing yt-dlp",
            );
            return c;
        };
        c.ytdlp = path;
        if let Some(i) = std::env::var_os("MUSEQ_FETCH_INBOX") {
            c.inbox = PathBuf::from(i);
        }
        if let Some(n) = std::env::var("MUSEQ_FETCH_MAX_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            c.max_bytes = n;
        }
        if let Some(n) = std::env::var("MUSEQ_FETCH_TIMEOUT_S")
            .ok()
            .and_then(|v| v.parse().ok())
        {
            c.timeout = Duration::from_secs(n);
        }
        if let Some(ck) = std::env::var_os("MUSEQ_FETCH_COOKIES").map(PathBuf::from) {
            use std::os::unix::fs::PermissionsExt;
            let ok = std::fs::metadata(&ck)
                .map(|m| m.is_file() && m.permissions().mode() & 0o077 == 0)
                .unwrap_or(false);
            if !ok {
                off(
                    &mut c,
                    "the cookies file must exist and be readable by its owner only (mode 600)",
                );
                return c;
            }
            c.cookies = Some(ck);
        }
        c.enabled = true;
        c.disabled_reason = None;
        c
    }
}

// ---- what the owner pasted -----------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub enum Source {
    YouTube(String),
    SpotifyTrack(String),
}

fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Accept only the few kinds of link we know how to handle; everything else is refused.
pub fn parse_source(raw: &str) -> Result<Source, String> {
    let url = stations::parse_stream_url(raw.trim()).ok_or("that is not a usable web address")?;
    if url.scheme() != "https" {
        return Err("only https links are accepted".into());
    }
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let segments: Vec<&str> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
    match host.as_str() {
        "music.youtube.com" | "www.youtube.com" | "youtube.com" | "m.youtube.com" => {
            if segments.first() == Some(&"watch")
                && let Some((_, id)) = url.query_pairs().find(|(k, _)| k == "v")
                && valid_video_id(&id)
            {
                return Ok(Source::YouTube(id.into_owned()));
            }
            Err("give a link to one video: https://music.youtube.com/watch?v=… (playlists are not supported yet)".into())
        }
        "youtu.be" => match segments.first() {
            Some(id) if valid_video_id(id) => Ok(Source::YouTube((*id).to_string())),
            _ => Err("that youtu.be link has no video id".into()),
        },
        "open.spotify.com" => {
            // /track/<id> or /intl-xx/track/<id>
            let pos = segments.iter().position(|s| *s == "track");
            match pos.and_then(|p| segments.get(p + 1)) {
                Some(id) if id.len() == 22 && id.chars().all(|c| c.is_ascii_alphanumeric()) => Ok(Source::SpotifyTrack((*id).to_string())),
                _ => Err("give a link to one Spotify track (albums and playlists are not supported yet)".into()),
            }
        }
        _ => Err("this address is not on the list of supported sites (YouTube, YouTube Music, Spotify tracks)".into()),
    }
}

// ---- Spotify page ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_s: Option<i64>,
}

fn unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// The `content` of the first `<meta>` tag that carries `key` as its property or name.
fn meta_content(html: &str, key: &str) -> Option<String> {
    let mut rest = html;
    while let Some(start) = rest.find("<meta") {
        let tag_area = &rest[start..];
        let end = tag_area.find('>')?;
        let tag = &tag_area[..end];
        if tag.contains(&format!("\"{key}\"")) {
            let c = tag.find("content=\"")? + "content=\"".len();
            let close = tag[c..].find('"')?;
            return Some(unescape(&tag[c..c + close]));
        }
        rest = &tag_area[end..];
    }
    None
}

/// A Spotify track page carries the title, artists and duration in its meta tags. Anything we
/// cannot read is an error ("the page changed"), never a guess.
pub fn parse_spotify_page(html: &str) -> Result<TrackMeta, String> {
    let title = meta_content(html, "og:title")
        .filter(|t| !t.is_empty())
        .ok_or("the Spotify page has no title (it may have changed)")?;
    let artist = meta_content(html, "music:musician_description")
        .or_else(|| {
            meta_content(html, "og:description")
                .and_then(|d| d.split(" · ").next().map(String::from))
        })
        .filter(|a| !a.is_empty())
        .ok_or("the Spotify page has no artist (it may have changed)")?;
    Ok(TrackMeta {
        title,
        artist,
        album: meta_content(html, "music:album").filter(|a| !a.starts_with("http")),
        duration_s: meta_content(html, "music:duration").and_then(|d| d.parse().ok()),
    })
}

async fn spotify_meta(app: &AppState, id: &str) -> Result<TrackMeta, String> {
    let base = app.fetch.spotify_base.trim_end_matches('/').to_string();
    let url = reqwest::Url::parse(&format!("{base}/track/{id}")).map_err(|e| e.to_string())?;
    stations::host_allowed(&url, app.fetch.allow_private).await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("curl/8.0") // a browser agent gets an empty shell from this site
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Spotify answered HTTP {}", resp.status().as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| e.without_url().to_string())?
    {
        body.extend_from_slice(&chunk);
        if body.len() > 2 << 20 {
            return Err("the Spotify page is unexpectedly large".into());
        }
    }
    parse_spotify_page(&String::from_utf8_lossy(&body))
}

// ---- matching ----------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub duration_s: Option<i64>,
}

fn fold(s: &str) -> String {
    crate::text::fold(s)
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Word-overlap similarity between two titles, 0 to 1.
fn similarity(a: &str, b: &str) -> f64 {
    let (a, b) = (fold(a), fold(b));
    let (wa, wb): (std::collections::HashSet<_>, std::collections::HashSet<_>) =
        (a.split(' ').collect(), b.split(' ').collect());
    if wa.is_empty() || wb.is_empty() {
        return 0.0;
    }
    wa.intersection(&wb).count() as f64 / wa.len().max(wb.len()) as f64
}

const VERSION_WORDS: &[&str] = &[
    "live",
    "remix",
    "karaoke",
    "cover",
    "slowed",
    "nightcore",
    "instrumental",
    "reverb",
    "sped",
    "acoustic",
    "demo",
    "edit",
    "mashup",
    "8d",
];

/// Pick the upload that is the studio recording: right title, right length, from the artist's
/// auto-generated "- Topic" channel if there is one, and not a version the request did not ask for.
pub fn pick(meta: &TrackMeta, cands: &[Candidate]) -> Result<Candidate, String> {
    let wanted = fold(&format!("{} {}", meta.title, meta.artist));
    let artist = fold(&meta.artist);
    let mut best: Option<(f64, &Candidate)> = None;
    let mut closest: Option<(f64, &Candidate)> = None;
    for c in cands {
        let sim = similarity(&meta.title, &c.title);
        if closest.is_none_or(|(s, _)| sim > s) {
            closest = Some((sim, c));
        }
        let title_words = fold(&c.title);
        let extra_version = VERSION_WORDS.iter().any(|w| {
            title_words.split(' ').any(|t| t == *w) && !wanted.split(' ').any(|t| t == *w)
        });
        let duration_ok = match (meta.duration_s, c.duration_s) {
            (Some(a), Some(b)) => (a - b).abs() <= 5,
            _ => true,
        };
        let channel = fold(&c.channel);
        let topic = channel.ends_with(" topic") && channel.contains(&artist);
        let artist_ok = topic || channel.contains(&artist) || fold(&c.title).contains(&artist);
        if sim < 0.6 || extra_version || !duration_ok || !artist_ok {
            continue;
        }
        let score = sim + if topic { 0.5 } else { 0.0 }
            - meta
                .duration_s
                .zip(c.duration_s)
                .map_or(0.0, |(a, b)| (a - b).abs() as f64 / 100.0);
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, c));
        }
    }
    best.map(|(_, c)| c.clone()).ok_or_else(|| match closest {
        Some((s, c)) => format!(
            "no upload matched {} by {} (closest: \"{}\" from \"{}\", similarity {:.2})",
            meta.title, meta.artist, c.title, c.channel, s
        ),
        None => format!("nothing was found for {} by {}", meta.title, meta.artist),
    })
}

// ---- yt-dlp -------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Failure {
    RateLimited,
    Gone,
    Geo,
    LoginRequired,
    Live,
    Other,
}

pub fn classify(stderr: &str, exit_code: Option<i32>) -> Failure {
    let s = stderr.to_ascii_lowercase();
    if exit_code == Some(101) || s.contains("is live") || s.contains("live event") {
        Failure::Live
    } else if s.contains("429") || s.contains("too many requests") {
        Failure::RateLimited
    } else if s.contains("video unavailable")
        || s.contains("404")
        || s.contains("has been removed")
        || s.contains("private video")
    {
        Failure::Gone
    } else if s.contains("not available in your country") || s.contains("geo") {
        Failure::Geo
    } else if s.contains("sign in")
        || s.contains("login required")
        || s.contains("confirm your age")
        || s.contains("cookies")
    {
        Failure::LoginRequired
    } else {
        Failure::Other
    }
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run yt-dlp without a shell, in its own process group, with a deadline (the whole group is
/// killed on timeout). Output is capped.
async fn run_ytdlp(app: &AppState, args: &[String]) -> Result<Run, String> {
    let mut cmd = tokio::process::Command::new(&app.fetch.ytdlp);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start yt-dlp: {e}"))?;
    let pid = child.id();
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let work = async {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let read_out = async { (&mut out).take(8 << 20).read_to_end(&mut o).await };
        let read_err = async { (&mut err).take(1 << 20).read_to_end(&mut e).await };
        let _ = tokio::join!(read_out, read_err);
        let status = child.wait().await;
        (status, o, e)
    };
    match tokio::time::timeout(app.fetch.timeout, work).await {
        Ok((status, o, e)) => Ok(Run {
            code: status.ok().and_then(|s| s.code()),
            stdout: String::from_utf8_lossy(&o).into_owned(),
            stderr: String::from_utf8_lossy(&e).into_owned(),
        }),
        Err(_) => {
            if let Some(pid) = pid {
                // SAFETY: killpg on the group we created for this child
                unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
            }
            let _ = child.wait().await; // reap it, no zombie
            Err("yt-dlp took too long and was stopped".into())
        }
    }
}

async fn search(app: &AppState, meta: &TrackMeta) -> Result<Vec<Candidate>, String> {
    let q = format!("ytsearch10:{} {} audio", meta.artist, meta.title);
    let r = run_ytdlp(
        app,
        &[
            "--no-warnings".into(),
            "--flat-playlist".into(),
            "-J".into(),
            "--".into(),
            q,
        ],
    )
    .await?;
    if r.code != Some(0) {
        return Err(format!(
            "the search failed ({:?})",
            classify(&r.stderr, r.code)
        ));
    }
    let v: Value = serde_json::from_str(&r.stdout)
        .map_err(|_| "the search answered with something unreadable".to_string())?;
    Ok(v["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let id = e["id"].as_str().filter(|i| valid_video_id(i))?;
            Some(Candidate {
                id: id.to_string(),
                title: e["title"].as_str()?.to_string(),
                channel: e["channel"]
                    .as_str()
                    .or_else(|| e["uploader"].as_str())
                    .unwrap_or("")
                    .to_string(),
                duration_s: e["duration"].as_f64().map(|d| d as i64),
            })
        })
        .collect())
}

/// Keep a name safe to use as a single file name inside the inbox.
pub fn sanitize_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|\0".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned = cleaned
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .chars()
        .take(120)
        .collect::<String>();
    if cleaned.is_empty() {
        "track".into()
    } else {
        cleaned
    }
}

const FETCH_EXTENSIONS: &[&str] = &["m4a", "opus", "ogg", "webm", "mp3", "flac", "aac"];

struct Downloaded {
    path: PathBuf,
    info: Value,
}

async fn download(
    app: &AppState,
    video_id: &str,
    work: &Path,
    with_cookies: bool,
) -> Result<Downloaded, (Failure, String)> {
    let url = format!("https://music.youtube.com/watch?v={video_id}"); // built here, never the owner's text
    let mut args: Vec<String> = [
        "--no-playlist",
        "-q",
        "--no-warnings",
        "--socket-timeout",
        "30",
        "--retries",
        "3",
        "--no-write-playlist-metafiles",
        "-f",
        "ba[ext=m4a]/ba",
        "--write-info-json",
        "--break-match-filters",
        "!is_live",
        "--print",
        "after_move:filepath",
        "--no-simulate",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push("--max-filesize".into());
    args.push(app.fetch.max_bytes.to_string());
    args.push("-o".into());
    args.push(work.join("media.%(ext)s").to_string_lossy().into_owned());
    if with_cookies && let Some(src) = &app.fetch.cookies {
        // a private copy for this run; the original is never touched
        let copy = work.join("cookies.txt");
        std::fs::copy(src, &copy)
            .map_err(|e| (Failure::Other, format!("cannot copy the cookies file: {e}")))?;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o600));
        args.push("--cookies".into());
        args.push(copy.to_string_lossy().into_owned());
    }
    args.push("--".into());
    args.push(url);
    let r = run_ytdlp(app, &args)
        .await
        .map_err(|e| (Failure::Other, e))?;
    if r.code != Some(0) {
        let class = classify(&r.stderr, r.code);
        return Err((
            class,
            format!("yt-dlp could not fetch it ({} )", describe(&class)),
        ));
    }
    let printed = r
        .stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(PathBuf::from)
        .ok_or((Failure::Other, "yt-dlp printed no file name".to_string()))?;
    // the file must be inside the work dir and have a known audio extension
    let path = printed
        .canonicalize()
        .map_err(|_| (Failure::Other, "the downloaded file is missing".to_string()))?;
    let work_c = work
        .canonicalize()
        .map_err(|e| (Failure::Other, e.to_string()))?;
    let ext_ok = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| FETCH_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
    if !path.starts_with(&work_c) || !ext_ok {
        return Err((
            Failure::Other,
            "yt-dlp produced a file we do not accept".into(),
        ));
    }
    if std::fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(u64::MAX)
        > app.fetch.max_bytes
    {
        return Err((
            Failure::Other,
            "the download is larger than the allowed size".into(),
        ));
    }
    let info_text = std::fs::read_to_string(work.join("media.info.json")).unwrap_or_default();
    let info: Value = if info_text.len() < (8 << 20) {
        serde_json::from_str(&info_text).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    Ok(Downloaded { path, info })
}

fn describe(f: &Failure) -> &'static str {
    match f {
        Failure::RateLimited => "the service is rate limiting this machine, try again later",
        Failure::Gone => "the video is gone or private",
        Failure::Geo => "not available in this country",
        Failure::LoginRequired => "the service wants a sign-in",
        Failure::Live => "a live stream, not a track",
        Failure::Other => "an unspecified error",
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The whole job. Returns the job result (a JSON object); an `Err` fails the job with that message.
pub async fn run(app: AppState, source: Source) -> Result<Value, String> {
    // 1. what are we after?
    let (meta, video_id, matched) = match &source {
        Source::YouTube(id) => (None, id.clone(), None),
        Source::SpotifyTrack(id) => {
            let meta = spotify_meta(&app, id).await?;
            let cands = search(&app, &meta).await?;
            let chosen = pick(&meta, &cands)?;
            let id = chosen.id.clone();
            (Some(meta), id, Some(chosen))
        }
    };
    // 2. already here?
    {
        let conn = db::lock(&app.db);
        let have: Option<i64> = conn
            .query_row(
                "SELECT file_id FROM provenance WHERE video_id=?1",
                [&video_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(fid) = have {
            return Ok(
                json!({"outcome": "skipped", "reason": "this video is already in the library", "file_id": fid.to_string(), "video_id": video_id}),
            );
        }
    }
    // 3. download into a private work folder under the inbox
    std::fs::create_dir_all(&app.fetch.inbox)
        .map_err(|e| format!("cannot create the inbox: {e}"))?;
    // leftovers of a crashed run (fetch jobs never overlap, so none of these is live)
    if let Ok(rd) = std::fs::read_dir(&app.fetch.inbox) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(".work-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let work = app
        .fetch
        .inbox
        .join(format!(".work-{}-{}", std::process::id(), now()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = download_and_import(&app, &source, meta, &video_id, matched, &work).await;
    let _ = std::fs::remove_dir_all(&work); // on every exit path
    result
}

async fn download_and_import(
    app: &AppState,
    source: &Source,
    meta: Option<TrackMeta>,
    video_id: &str,
    matched: Option<Candidate>,
    work: &Path,
) -> Result<Value, String> {
    let dl = match download(app, video_id, work, false).await {
        Ok(d) => d,
        Err((Failure::LoginRequired, _)) if app.fetch.cookies.is_some() => {
            download(app, video_id, work, true)
                .await
                .map_err(|(_, m)| m)?
        }
        Err((_, m)) => return Err(m),
    };
    // 4. names and tags: the Spotify page when we have it, else what yt-dlp learned
    let info = &dl.info;
    let title = meta
        .as_ref()
        .map(|m| m.title.clone())
        .or_else(|| {
            info["track"]
                .as_str()
                .or_else(|| info["title"].as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| "track".into());
    let artist = meta
        .as_ref()
        .map(|m| m.artist.clone())
        .or_else(|| {
            info["artists"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|x| x.as_str())
                .or_else(|| info["artist"].as_str())
                .or_else(|| info["uploader"].as_str())
                .map(|s| s.trim_end_matches(" - Topic").to_string())
        })
        .unwrap_or_default();
    let album = meta
        .as_ref()
        .and_then(|m| m.album.clone())
        .or_else(|| info["album"].as_str().map(String::from));
    // 5. into the inbox under a safe name that cannot collide or escape
    let ext = dl
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("m4a")
        .to_ascii_lowercase();
    let base = sanitize_name(&if artist.is_empty() {
        title.clone()
    } else {
        format!("{artist} - {title}")
    });
    let mut final_path = app.fetch.inbox.join(format!("{base}.{ext}"));
    let mut n = 1;
    while final_path.exists() {
        n += 1;
        final_path = app.fetch.inbox.join(format!("{base} ({n}).{ext}"));
    }
    let inbox_c = app.fetch.inbox.canonicalize().map_err(|e| e.to_string())?;
    if final_path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .as_deref()
        != Some(inbox_c.as_path())
    {
        return Err("refused: the target name would leave the inbox".into());
    }
    if std::fs::rename(&dl.path, &final_path).is_err() {
        std::fs::copy(&dl.path, &final_path).map_err(|e| format!("cannot store the file: {e}"))?;
    }
    // 6. catalogue it (a normal scan skips what it already knows), then fingerprint it
    let (db_a, root) = (app.db.clone(), app.root.clone());
    let stats = tokio::task::spawn_blocking(move || crate::scan::scan(&db_a, &root))
        .await
        .map_err(|_| "the scan crashed".to_string())?;
    let rel = final_path
        .strip_prefix(&*app.root)
        .map_err(|_| "the inbox must be inside the library folder".to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let (file_id, track_id): (i64, Option<i64>) = db::lock(&app.db)
        .query_row("SELECT id, track_id FROM file WHERE path=?1", [&rel], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map_err(|_| "the new file was not picked up by the scan".to_string())?;
    {
        let conn = db::lock(&app.db);
        let version = info["_version"]["version"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let _ = conn.execute(
            "INSERT OR REPLACE INTO provenance(file_id, source, source_url, video_id, fetched_at, tool_version, matched_title, matched_channel) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                file_id,
                match source { Source::YouTube(_) => "youtube", Source::SpotifyTrack(_) => "spotify" },
                match source { Source::YouTube(id) => format!("https://music.youtube.com/watch?v={id}"), Source::SpotifyTrack(id) => format!("https://open.spotify.com/track/{id}") },
                video_id,
                now(),
                version,
                matched.as_ref().map(|c| c.title.clone()).or_else(|| info["title"].as_str().map(String::from)),
                matched.as_ref().map(|c| c.channel.clone()).or_else(|| info["channel"].as_str().map(String::from)),
            ],
        );
        // the file itself has no useful tags: keep what the source told us, and keep it across rescans
        if let Some(t) = track_id {
            let search = crate::db::search_text(&title, Some(&artist), album.as_deref());
            let _ = conn.execute(
                "UPDATE track SET title=?1, artist=?2, album=COALESCE(?3, album), search_text=?4, corrected=1 WHERE id=?5",
                params![title, if artist.is_empty() { None } else { Some(&artist) }, album, search, t],
            );
        }
    }
    let fp = crate::fingerprint::run_job(app.db.clone(), app.root.clone()).await;
    Ok(json!({
        "outcome": "imported",
        "file_id": file_id.to_string(),
        "track_id": track_id.map(|t| t.to_string()),
        "title": title, "artist": artist, "album": album,
        "video_id": video_id,
        "matched": matched.map(|c| json!({"title": c.title, "channel": c.channel, "duration_s": c.duration_s})),
        "scan": stats,
        "fingerprint": fp,
    }))
}

pub fn status(app: &AppState) -> Value {
    json!({"enabled": app.fetch.enabled, "reason": app.fetch.disabled_reason, "needs_ack": ACK_PHRASE})
}
