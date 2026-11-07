// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
//! Audio fingerprints, computed in-process (no `fpcalc`). Used to find the same recording in
//! different files, whatever the format or the tags say.
use rusqlite::{Connection, params};
use rusty_chromaprint::{Configuration, Fingerprinter};
use std::path::Path;
use std::sync::{Arc, Mutex};
use symphonia::core::audio::sample::Sample;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::db;

#[derive(Debug)]
pub enum FpError {
    /// The codec cannot be decoded here (for example Opus with Symphonia 0.6).
    Unsupported(String),
    /// The file could not be read or decoded.
    Failed(String),
}

pub fn config() -> Configuration {
    Configuration::default()
}

/// Does this file start like an Ogg Opus stream? (Symphonia cannot decode Opus.)
fn is_ogg_opus(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 96];
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let n = f.read(&mut head).unwrap_or(0);
    head[..n].starts_with(b"OggS") && head[..n].windows(8).any(|w| w == b"OpusHead")
}

/// Decode Ogg Opus with libopus (stereo or mono only) and fingerprint it. Opus always decodes at
/// 48 kHz; the encoder's pre-skip samples are dropped, as the specification requires.
fn fingerprint_opus(path: &Path) -> Result<(Vec<u32>, i64), FpError> {
    let file = std::fs::File::open(path).map_err(|e| FpError::Failed(e.to_string()))?;
    let mut reader = ogg::PacketReader::new(std::io::BufReader::new(file));
    let head = reader
        .read_packet()
        .map_err(|e| FpError::Failed(format!("ogg: {e}")))?
        .ok_or_else(|| FpError::Failed("empty file".into()))?;
    let h = &head.data;
    if h.len() < 19 || &h[..8] != b"OpusHead" {
        return Err(FpError::Unsupported("not an Ogg Opus stream".into()));
    }
    let channels = h[9] as usize;
    let pre_skip = u16::from_le_bytes([h[10], h[11]]) as usize;
    if h[18] != 0 || !(1..=2).contains(&channels) {
        return Err(FpError::Unsupported(
            "Opus with more than two channels".into(),
        ));
    }
    let mut decoder = opus::Decoder::new(
        48_000,
        if channels == 1 {
            opus::Channels::Mono
        } else {
            opus::Channels::Stereo
        },
    )
    .map_err(|e| FpError::Failed(format!("opus: {e}")))?;
    let mut printer = Fingerprinter::new(&config());
    printer
        .start(48_000, channels as u32)
        .map_err(|e| FpError::Failed(format!("{e:?}")))?;
    let mut pcm = vec![0i16; 5760 * channels]; // the longest Opus packet is 120 ms
    let mut to_skip = pre_skip;
    let mut frames: u64 = 0;
    let mut seen_tags = false;
    loop {
        let packet = match reader.read_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => return Err(FpError::Failed(format!("ogg: {e}"))),
        };
        if !seen_tags {
            seen_tags = true;
            if packet.data.starts_with(b"OpusTags") {
                continue;
            }
        }
        let n = match decoder.decode(&packet.data, &mut pcm, false) {
            Ok(n) => n,
            Err(_) => continue, // a damaged packet: skip it
        };
        let skip = to_skip.min(n);
        to_skip -= skip;
        let mut end = n;
        if packet.last_in_stream() {
            // the last page's granule position says how many samples really belong to the stream
            let total = packet.absgp_page().saturating_sub(pre_skip as u64);
            let room = total.saturating_sub(frames) as usize;
            end = end.min(skip + room);
        }
        let out = &pcm[skip * channels..end.max(skip) * channels];
        frames += (out.len() / channels) as u64;
        printer.consume(out);
        if packet.last_in_stream() {
            break; // a chained second stream is a different recording: only the first counts
        }
    }
    if frames == 0 {
        return Err(FpError::Failed("no audio decoded".into()));
    }
    printer.finish();
    let fp = printer.fingerprint().to_vec();
    if fp.is_empty() {
        return Err(FpError::Failed("too short to fingerprint".into()));
    }
    Ok((fp, (frames * 1000 / 48_000) as i64))
}

/// Decode a file and return its fingerprint and decoded duration in milliseconds.
pub fn fingerprint_file(path: &Path) -> Result<(Vec<u32>, i64), FpError> {
    crate::scan::test_hook(&path.to_string_lossy(), "__museq_panic_fp__");
    if is_ogg_opus(path) {
        return fingerprint_opus(path);
    }
    let file = std::fs::File::open(path).map_err(|e| FpError::Failed(e.to_string()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| FpError::Unsupported(format!("unknown format: {e}")))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| FpError::Failed("no audio track".into()))?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| FpError::Failed("no audio parameters".into()))?
        .clone();
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| FpError::Unsupported(e.to_string()))?;

    let mut printer = Fingerprinter::new(&config());
    let mut started = false;
    let (mut rate, mut channels, mut frames) = (0u32, 1usize, 0u64);
    let mut buf: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(FpError::Failed(e.to_string())),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio) => {
                if !started {
                    rate = audio.spec().rate();
                    channels = audio.spec().channels().count().max(1);
                    printer
                        .start(rate, channels as u32)
                        .map_err(|e| FpError::Failed(format!("{e:?}")))?;
                    started = true;
                }
                buf.resize(audio.samples_interleaved(), f32::MID);
                audio.copy_to_slice_interleaved(&mut buf);
                let pcm: Vec<i16> = buf
                    .iter()
                    .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                    .collect();
                frames += (pcm.len() / channels) as u64;
                printer.consume(&pcm);
            }
            Err(SymError::DecodeError(_)) => {} // a damaged packet: skip it
            Err(e) => return Err(FpError::Failed(e.to_string())),
        }
    }
    if !started || frames == 0 {
        return Err(FpError::Failed("no audio decoded".into()));
    }
    printer.finish();
    let fp = printer.fingerprint().to_vec();
    if fp.is_empty() {
        return Err(FpError::Failed("too short to fingerprint".into()));
    }
    Ok((fp, (frames * 1000 / rate.max(1) as u64) as i64))
}

pub fn to_blob(fp: &[u32]) -> Vec<u8> {
    fp.iter().flat_map(|v| v.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Vec<u32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

/// Fingerprint every file that has none (or whose content changed). Two at a time, so the
/// machine stays responsive; Opus and unreadable files get a status instead of failing the job.
pub async fn run_job(
    db: Arc<Mutex<Connection>>,
    root: Arc<std::path::PathBuf>,
) -> serde_json::Value {
    let todo: Vec<(i64, String, String)> = {
        let conn = db::lock(&db);
        conn.prepare(
            "SELECT f.id, f.path, f.sha256 FROM file f
             LEFT JOIN fingerprint p ON p.file_id = f.id
             WHERE p.file_id IS NULL OR p.sha256 <> f.sha256 ORDER BY f.id",
        )
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect()
        })
        .unwrap_or_default()
    };
    let sem = Arc::new(tokio::sync::Semaphore::new(2));
    let mut tasks = tokio::task::JoinSet::new();
    for (id, rel, sha) in todo {
        let Ok(permit) = sem.clone().acquire_owned().await else {
            break;
        };
        let (db, root) = (db.clone(), root.clone());
        tasks.spawn(async move {
            let _permit = permit;
            let path = root.join(&rel);
            let outcome = tokio::task::spawn_blocking(move || {
                if !path.exists() {
                    return None; // vanished since the scan: nothing to record
                }
                std::panic::catch_unwind(|| fingerprint_file(&path))
                    .unwrap_or_else(|_| Err(FpError::Failed("decoder crashed".into())))
                    .into()
            })
            .await
            .ok()
            .flatten();
            let Some(result) = outcome else { return "skipped" };
            let (status, error, duration, data): (&str, Option<String>, Option<i64>, Option<Vec<u8>>) = match result {
                Ok((fp, ms)) => ("ok", None, Some(ms), Some(to_blob(&fp))),
                Err(FpError::Unsupported(e)) => ("unsupported", Some(e), None, None),
                Err(FpError::Failed(e)) => ("error", Some(e), None, None),
            };
            {
                let conn = db::lock(&db);
                // new content: the old comparisons no longer say anything about this file
                let _ = conn.execute("DELETE FROM audio_match WHERE a=?1 OR b=?1", [id]);
                let _ = conn.execute(
                    "INSERT INTO fingerprint(file_id, sha256, status, error, duration_ms, data, compared) VALUES(?1,?2,?3,?4,?5,?6,0)
                     ON CONFLICT(file_id) DO UPDATE SET sha256=excluded.sha256, status=excluded.status, error=excluded.error,
                       duration_ms=excluded.duration_ms, data=excluded.data, compared=0",
                    params![id, sha, status, error, duration, data],
                );
            }
            status
        });
    }
    let (mut ok, mut unsupported, mut failed, mut skipped) = (0u32, 0u32, 0u32, 0u32);
    while let Some(r) = tasks.join_next().await {
        match r {
            Ok("ok") => ok += 1,
            Ok("unsupported") => unsupported += 1,
            Ok("skipped") => skipped += 1,
            _ => failed += 1,
        }
    }
    // work out which files sound alike (CPU heavy, a few threads, off the async threads)
    let compare_db = db.clone();
    let (compared, matches) =
        tokio::task::spawn_blocking(move || crate::duplicates::compare_new(&compare_db))
            .await
            .unwrap_or((0, 0));
    serde_json::json!({"fingerprinted": ok, "unsupported": unsupported, "failed": failed, "skipped": skipped, "compared": compared, "matches_stored": matches})
}
