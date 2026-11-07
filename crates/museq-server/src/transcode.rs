// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
//! On-the-fly transcoding for `/v1/audio/{id}?codec=&kbps=`: one ffmpeg process per request,
//! no shell, output streamed as it is produced. Transcoded answers have no Range support.
use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncReadExt;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    Opus,
    Mp3,
    Aac,
}

impl Codec {
    pub fn parse(s: &str) -> Option<Codec> {
        match s {
            "opus" => Some(Codec::Opus),
            "mp3" => Some(Codec::Mp3),
            "aac" => Some(Codec::Aac),
            _ => None,
        }
    }
    fn args(self) -> (&'static str, &'static str, &'static str) {
        // (encoder, container, mime)
        match self {
            Codec::Opus => ("libopus", "ogg", "audio/ogg"),
            Codec::Mp3 => ("libmp3lame", "mp3", "audio/mpeg"),
            Codec::Aac => ("aac", "adts", "audio/aac"),
        }
    }
    pub fn default_kbps(self) -> i64 {
        match self {
            Codec::Opus => 96,
            Codec::Mp3 => 192,
            Codec::Aac => 128,
        }
    }
}

/// Title and artist of the track being encoded, for the stream's own tags (Opus) or its ICY title (mp3, aac).
#[derive(Clone, Debug, Default)]
pub struct Tags {
    pub title: String,
    pub artist: Option<String>,
}

/// A tag text that is safe to put in an ffmpeg argument or an ICY block: no control characters, no quote or
/// semicolon (they end the `StreamTitle='...';` field), trimmed, at most `max_chars` characters.
pub fn clean_tag(s: &str, max_chars: usize) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_control() && *c != '\'' && *c != ';')
        .collect();
    cleaned.trim().chars().take(max_chars).collect()
}

/// The ffmpeg command that re-encodes `path` to stdout. `realtime` (`-re`) paces the output like
/// a live source, which is what a radio station needs.
pub fn command(
    ffmpeg: &str,
    path: &Path,
    codec: Codec,
    kbps: i64,
    realtime: bool,
    tags: Option<&Tags>,
) -> tokio::process::Command {
    let (enc, container, _) = codec.args();
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args(["-nostdin", "-v", "error", "-protocol_whitelist", "file"]);
    if realtime {
        cmd.arg("-re");
    }
    cmd.arg("-i")
        .arg(format!("file:{}", path.display()))
        .args(["-vn", "-map_metadata", "-1"]);
    // after -map_metadata -1, which clears the file's own tags: these are the only tags the stream carries
    if let Some(t) = tags {
        let title = clean_tag(&t.title, 200);
        if !title.is_empty() {
            cmd.arg("-metadata").arg(format!("title={title}"));
        }
        if let Some(a) = t
            .artist
            .as_deref()
            .map(|a| clean_tag(a, 200))
            .filter(|a| !a.is_empty())
        {
            cmd.arg("-metadata").arg(format!("artist={a}"));
        }
    }
    cmd.args(["-c:a", enc, "-b:a"])
        .arg(format!("{kbps}k"))
        .args(["-f", container, "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd
}

pub fn mime(codec: Codec) -> &'static str {
    codec.args().2
}

/// The last few KiB of ffmpeg's error output, kept for the log. The reader keeps draining the pipe so ffmpeg never
/// blocks on a full one.
fn drain_stderr(
    stderr: Option<tokio::process::ChildStderr>,
) -> std::sync::Arc<std::sync::Mutex<Vec<u8>>> {
    let tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    if let Some(mut err) = stderr {
        let keep = tail.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok(n) = err.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut t = keep.lock().unwrap_or_else(|e| e.into_inner());
                t.extend_from_slice(&buf[..n]);
                let over = t.len().saturating_sub(4096);
                t.drain(..over); // only the end is kept
            }
        });
    }
    tail
}

/// Waits for ffmpeg to exit, but not for ever: after five seconds it is killed. None if it could not be waited for.
async fn wait_bounded(child: &mut tokio::process::Child) -> Option<std::process::ExitStatus> {
    match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
        Ok(r) => r.ok(),
        Err(_) => {
            let _ = child.kill().await;
            child.wait().await.ok()
        }
    }
}

fn log_failure(
    path: &Path,
    status: Option<std::process::ExitStatus>,
    tail: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
) {
    let t = tail.lock().unwrap_or_else(|e| e.into_inner());
    let text = String::from_utf8_lossy(&t);
    eprintln!(
        "ffmpeg failed for {} (exit: {}): {}",
        path.display(),
        status.map_or("unknown".to_string(), |s| s.to_string()),
        text.trim().lines().last().unwrap_or("no error output")
    );
}

/// Stream `path` re-encoded. The file path is passed as one argument after `-i`; a leading `-`
/// is neutralised by making the path absolute first.
///
/// The first bytes are read before the answer is built, so a file ffmpeg cannot convert is a 502, not an empty
/// 200. Once the answer has started, a non-zero exit of ffmpeg ends the body with an error: the client sees a
/// broken transfer instead of a short file that looks complete.
pub async fn stream<G: Send + 'static>(
    ffmpeg: &str,
    path: &Path,
    codec: Codec,
    kbps: i64,
    guard: G,
) -> Response {
    let (_, _, mime) = codec.args();
    let mut cmd = command(ffmpeg, path, codec, kbps, false, None);
    // keep the end of ffmpeg's error output for the server's log (it is never sent to the client: it names paths)
    cmd.stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                axum::Json(serde_json::json!({"error": "transcoding needs ffmpeg, which is not installed or could not be started; see GET /v1/tools for how to install it (it is downloaded only after you confirm)"})),
            )
                .into_response();
        }
    };
    let mut out = child.stdout.take().expect("piped");
    let stderr_tail = drain_stderr(child.stderr.take());
    let mut first = vec![0u8; 32 * 1024];
    let n = match tokio::time::timeout(std::time::Duration::from_secs(30), out.read(&mut first))
        .await
    {
        Ok(Ok(n)) if n > 0 => n,
        Ok(_) => {
            // ended before producing anything: the file could not be converted
            let status = wait_bounded(&mut child).await;
            log_failure(path, status, &stderr_tail);
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({"error": "ffmpeg could not convert this file"})),
            )
                .into_response();
        }
        Err(_) => {
            let _ = child.kill().await;
            return (
                StatusCode::GATEWAY_TIMEOUT,
                axum::Json(
                    serde_json::json!({"error": "ffmpeg produced nothing within 30 seconds"}),
                ),
            )
                .into_response();
        }
    };
    first.truncate(n);
    // the child (killed on drop) and the guard (a concurrency permit) live as long as the body
    let path_for_log = path.to_path_buf();
    let body = Body::from_stream(futures_util::stream::unfold(
        (out, child, guard, Some(first), false),
        move |(mut out, mut child, guard, first, done)| {
            let stderr_tail = stderr_tail.clone();
            let path = path_for_log.clone();
            async move {
                if done {
                    return None;
                }
                if let Some(chunk) = first {
                    return Some((
                        Ok::<_, std::io::Error>(chunk),
                        (out, child, guard, None, false),
                    ));
                }
                let mut buf = vec![0u8; 32 * 1024];
                match out.read(&mut buf).await {
                    Ok(0) => {
                        // a clean end only if ffmpeg itself says it finished, and it must do so promptly
                        let status = wait_bounded(&mut child).await;
                        if status.is_some_and(|st| st.success()) {
                            None
                        } else {
                            log_failure(&path, status, &stderr_tail);
                            Some((
                                Err(std::io::Error::other("ffmpeg failed part way through")),
                                (out, child, guard, None, true),
                            ))
                        }
                    }
                    Err(e) => {
                        let _ = child.kill().await;
                        Some((Err(e), (out, child, guard, None, true)))
                    }
                    Ok(n) => {
                        buf.truncate(n);
                        Some((Ok(buf), (out, child, guard, None, false)))
                    }
                }
            }
        },
    ));
    let mut resp = Response::new(body);
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    resp.headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("none"));
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}
