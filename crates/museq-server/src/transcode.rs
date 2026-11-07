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

/// The ffmpeg command that re-encodes `path` to stdout. `realtime` (`-re`) paces the output like
/// a live source, which is what a radio station needs.
pub fn command(
    ffmpeg: &str,
    path: &Path,
    codec: Codec,
    kbps: i64,
    realtime: bool,
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
        .args(["-vn", "-map_metadata", "-1", "-c:a", enc, "-b:a"])
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

/// Stream `path` re-encoded. The file path is passed as one argument after `-i`; a leading `-`
/// is neutralised by making the path absolute first.
pub fn stream<G: Send + 'static>(
    ffmpeg: &str,
    path: &Path,
    codec: Codec,
    kbps: i64,
    guard: G,
) -> Response {
    let (_, _, mime) = codec.args();
    let mut cmd = command(ffmpeg, path, codec, kbps, false);
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
    let out = child.stdout.take().expect("piped");
    // the child (killed on drop) and the guard (a concurrency permit) live as long as the body
    let body = Body::from_stream(futures_util::stream::unfold(
        (out, child, guard),
        |(mut out, child, guard)| async move {
            let mut buf = vec![0u8; 32 * 1024];
            match out.read(&mut buf).await {
                Ok(0) | Err(_) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((Ok::<_, std::io::Error>(buf), (out, child, guard)))
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
