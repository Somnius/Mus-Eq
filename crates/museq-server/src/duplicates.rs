//! Finding the same recording more than once. Nothing here deletes a file: a merge only
//! changes which track the files belong to.
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::params;
use rusty_chromaprint::match_fingerprints;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};

use crate::api::{AppState, err, internal};
use crate::{db, fingerprint};

/// Chromaprint segment score: lower means more alike. Re-encodes of one source scored 0.3 to 1.1
/// on synthetic noise, a different source 7.0. Tuned on synthetic audio only: check on real music.
const MAX_SCORE: f64 = 2.0;
/// The matching segment must cover at least this much of the shorter fingerprint.
const MIN_COVERAGE: f64 = 0.8;
/// Only files whose lengths differ by at most this many milliseconds are compared.
const DURATION_WINDOW_MS: i64 = 4000;

#[derive(Clone, Serialize)]
pub struct FileInfo {
    file_id: String,
    track_id: Option<String>,
    title: Option<String>,
    artist: Option<String>,
    path: String,
    codec: Option<String>,
    lossless: bool,
    bitrate_kbps: Option<i64>,
    sample_rate: Option<i64>,
    bytes: i64,
    duration_ms: Option<i64>,
    #[serde(skip)]
    sha256: String,
}

#[derive(Serialize)]
struct Proof {
    a: String,
    b: String,
    kind: &'static str,
    /// chromaprint score for `audio` proofs (lower is more alike)
    score: Option<f64>,
}

#[derive(Serialize)]
struct Group {
    id: String,
    keep_file_id: String,
    files: Vec<FileInfo>,
    proof: Vec<Proof>,
}

struct Dsu(Vec<usize>);
impl Dsu {
    fn find(&mut self, x: usize) -> usize {
        if self.0[x] != x {
            let r = self.find(self.0[x]);
            self.0[x] = r;
        }
        self.0[x]
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[rb] = ra;
        }
    }
}

/// Higher is better: lossless first, then bitrate, then sample rate, then size.
fn quality(f: &FileInfo) -> (bool, i64, i64, i64) {
    (
        f.lossless,
        f.bitrate_kbps.unwrap_or(0),
        f.sample_rate.unwrap_or(0),
        f.bytes,
    )
}

fn load(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<(FileInfo, Option<Vec<u32>>)>> {
    let mut st = conn.prepare(
        "SELECT f.id, f.track_id, t.title, t.artist, f.path, f.codec, f.lossless, f.bitrate_kbps, f.sample_rate, f.bytes,
                f.duration_ms, f.sha256, p.status, p.data
         FROM file f LEFT JOIN track t ON t.id = f.track_id
         LEFT JOIN fingerprint p ON p.file_id = f.id AND p.sha256 = f.sha256
         ORDER BY f.id",
    )?;
    let rows = st.query_map([], |r| {
        let id: i64 = r.get(0)?;
        let status: Option<String> = r.get(12)?;
        let data: Option<Vec<u8>> = r.get(13)?;
        let fp = if status.as_deref() == Some("ok") {
            data.map(|d| fingerprint::from_blob(&d))
        } else {
            None
        };
        Ok((
            FileInfo {
                file_id: id.to_string(),
                track_id: r.get::<_, Option<i64>>(1)?.map(|v| v.to_string()),
                title: r.get(2)?,
                artist: r.get(3)?,
                path: r.get(4)?,
                codec: r.get(5)?,
                lossless: r.get::<_, i64>(6)? != 0,
                bitrate_kbps: r.get(7)?,
                sample_rate: r.get(8)?,
                bytes: r.get(9)?,
                duration_ms: r.get(10)?,
                sha256: r.get(11)?,
            },
            fp,
        ))
    })?;
    rows.collect()
}

fn group_files(files: Vec<(FileInfo, Option<Vec<u32>>)>) -> Vec<Group> {
    let cfg = fingerprint::config();
    let n = files.len();
    let mut dsu = Dsu((0..n).collect());
    let mut proofs: Vec<(usize, usize, &'static str, Option<f64>)> = Vec::new();

    // 1. identical bytes
    let mut by_hash: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, (f, _)) in files.iter().enumerate() {
        by_hash.entry(f.sha256.as_str()).or_default().push(i);
    }
    for idx in by_hash.values().filter(|v| v.len() > 1) {
        for w in idx.windows(2) {
            dsu.union(w[0], w[1]);
            proofs.push((w[0], w[1], "hash", None));
        }
    }

    // 2. the same audio: compare fingerprints of files of similar length only
    let mut order: Vec<usize> = (0..n).filter(|&i| files[i].1.is_some()).collect();
    order.sort_by_key(|&i| files[i].0.duration_ms.unwrap_or(0));
    for (pos, &i) in order.iter().enumerate() {
        for &j in &order[pos + 1..] {
            let (di, dj) = (
                files[i].0.duration_ms.unwrap_or(0),
                files[j].0.duration_ms.unwrap_or(0),
            );
            if dj - di > DURATION_WINDOW_MS {
                break;
            }
            let (a, b) = (
                files[i].1.as_deref().unwrap_or(&[]),
                files[j].1.as_deref().unwrap_or(&[]),
            );
            if let Ok(segs) = match_fingerprints(a, b, &cfg) {
                let shorter = a.len().min(b.len()) as f64 * cfg.item_duration_in_seconds() as f64;
                let best = segs
                    .iter()
                    .filter(|s| s.duration(&cfg) as f64 >= shorter * MIN_COVERAGE)
                    .map(|s| s.score)
                    .fold(f64::INFINITY, f64::min);
                if best <= MAX_SCORE {
                    dsu.union(i, j);
                    proofs.push((i, j, "audio", Some((best * 100.0).round() / 100.0)));
                }
            }
        }
    }

    let mut comps: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let r = dsu.find(i);
        comps.entry(r).or_default().push(i);
    }
    let mut groups = Vec::new();
    for (_, idx) in comps {
        if idx.len() < 2 {
            continue;
        }
        // already merged into one track: nothing left to decide
        let tracks: std::collections::HashSet<_> =
            idx.iter().map(|&i| files[i].0.track_id.clone()).collect();
        if tracks.len() == 1 && tracks.iter().next().unwrap().is_some() {
            continue;
        }
        let mut members: Vec<FileInfo> = idx.iter().map(|&i| files[i].0.clone()).collect();
        let keep = members
            .iter()
            .max_by_key(|f| quality(f))
            .map(|f| f.file_id.clone())
            .unwrap_or_default();
        members.sort_by_key(|f| std::cmp::Reverse(quality(f)));
        let ids: Vec<&str> = {
            let mut v: Vec<&str> = members.iter().map(|f| f.file_id.as_str()).collect();
            v.sort_unstable();
            v
        };
        let proof = proofs
            .iter()
            .filter(|(a, b, _, _)| idx.contains(a) && idx.contains(b))
            .map(|(a, b, kind, score)| Proof {
                a: files[*a].0.file_id.clone(),
                b: files[*b].0.file_id.clone(),
                kind,
                score: *score,
            })
            .collect();
        groups.push(Group {
            id: ids.join("-"),
            keep_file_id: keep,
            files: members,
            proof,
        });
    }
    groups
}

pub async fn list_duplicates(State(s): State<AppState>) -> Response {
    let loaded = load(&db::lock(&s.db));
    match loaded {
        // the comparison is CPU work: keep it off the async threads
        Ok(files) => match tokio::task::spawn_blocking(move || group_files(files)).await {
            Ok(groups) => Json(json!({"groups": groups, "note": "nothing is deleted; merging only joins files under one track"})).into_response(),
            Err(e) => internal(e),
        },
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct MergeBody {
    file_ids: Vec<String>,
    keep_file_id: String,
}

/// Join the files under the track of `keep_file_id`. Files stay on disk and keep working;
/// playlist entries of the absorbed tracks now point at the kept track.
pub async fn merge(State(s): State<AppState>, Json(b): Json<MergeBody>) -> Response {
    let parse = |v: &str| v.parse::<i64>().ok();
    let Some(keep) = parse(&b.keep_file_id) else {
        return err(StatusCode::BAD_REQUEST, "keep_file_id is not a valid id");
    };
    let mut ids: Vec<i64> = Vec::new();
    for v in &b.file_ids {
        match parse(v) {
            Some(i) => ids.push(i),
            None => return err(StatusCode::BAD_REQUEST, "file_ids contains an invalid id"),
        }
    }
    ids.sort_unstable();
    ids.dedup();
    if ids.len() < 2 || !ids.contains(&keep) {
        return err(
            StatusCode::BAD_REQUEST,
            "give at least two file_ids, including keep_file_id",
        );
    }
    let mut conn = db::lock(&s.db);
    let res: rusqlite::Result<Result<i64, &'static str>> = (|| {
        let tx = conn.transaction()?;
        let mut tracks: Vec<Option<i64>> = Vec::new();
        for id in &ids {
            let t: Option<Option<i64>> = tx
                .query_row("SELECT track_id FROM file WHERE id=?1", [id], |r| r.get(0))
                .map(Some)
                .or_else(|e| {
                    if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                        Ok(None)
                    } else {
                        Err(e)
                    }
                })?;
            match t {
                None => return Ok(Err("no such file")),
                Some(tr) => tracks.push(tr),
            }
        }
        let Some(target) = tx.query_row("SELECT track_id FROM file WHERE id=?1", [keep], |r| {
            r.get::<_, Option<i64>>(0)
        })?
        else {
            return Ok(Err("the kept file has no track"));
        };
        let losers: std::collections::BTreeSet<i64> = tracks
            .into_iter()
            .flatten()
            .filter(|t| *t != target)
            .collect();
        for t in &losers {
            tx.execute(
                "UPDATE file SET track_id=?1 WHERE track_id=?2",
                params![target, t],
            )?;
            tx.execute(
                "UPDATE playlist_item SET track_id=?1 WHERE track_id=?2",
                params![target, t],
            )?;
            tx.execute("DELETE FROM track WHERE id=?1", [t])?;
        }
        // any chosen file that had no track yet joins the kept one
        for id in &ids {
            tx.execute(
                "UPDATE file SET track_id=?1 WHERE id=?2 AND track_id IS NULL",
                params![target, id],
            )?;
        }
        tx.execute(
            "UPDATE track SET preferred_file_id=?1 WHERE id=?2",
            params![keep, target],
        )?;
        tx.commit()?;
        Ok(Ok(target))
    })();
    match res {
        Ok(Ok(track)) => Json(json!({"track_id": track.to_string(), "preferred_file_id": keep.to_string(), "merged_files": ids.len()})).into_response(),
        Ok(Err(msg)) => err(StatusCode::NOT_FOUND, msg),
        Err(e) => internal(e),
    }
}
