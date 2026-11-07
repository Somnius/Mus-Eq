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
pub const MAX_SCORE: f64 = 2.0;
/// Edges up to this score are stored, so the threshold can move without recomputing everything.
pub const STORE_MAX_SCORE: f64 = 4.0;
/// The matching segment must cover at least this much of the shorter fingerprint.
const MIN_COVERAGE: f64 = 0.8;
/// Only files whose lengths differ by at most this many milliseconds are compared.
pub const DURATION_WINDOW_MS: i64 = 4000;

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
    id: i64,
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

fn load(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<FileInfo>> {
    let mut st = conn.prepare(
        "SELECT f.id, f.track_id, t.title, t.artist, f.path, f.codec, f.lossless, f.bitrate_kbps, f.sample_rate, f.bytes,
                f.duration_ms, f.sha256
         FROM file f LEFT JOIN track t ON t.id = f.track_id ORDER BY f.id",
    )?;
    let rows = st.query_map([], |r| {
        let id: i64 = r.get(0)?;
        Ok(FileInfo {
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
            id,
            sha256: r.get(11)?,
        })
    })?;
    rows.collect()
}

/// How alike two fingerprints are. Scores are Chromaprint's: lower means more alike.
#[derive(Debug, Clone, Copy)]
pub struct Comparison {
    /// The best score among matching stretches that cover at least `MIN_COVERAGE` of the
    /// shorter recording (infinity if none does): this is what the duplicate decision uses.
    pub covered_score: f64,
    /// The best score of any matching stretch, and what share of the shorter recording it covers.
    pub best_score: f64,
    pub best_coverage: f64,
}

pub fn compare(a: &[u32], b: &[u32]) -> Option<Comparison> {
    let cfg = fingerprint::config();
    let segs = match_fingerprints(a, b, &cfg).ok()?;
    let shorter = a.len().min(b.len()) as f64 * cfg.item_duration_in_seconds() as f64;
    if shorter <= 0.0 {
        return None;
    }
    let covered_score = segs
        .iter()
        .filter(|s| s.duration(&cfg) as f64 >= shorter * MIN_COVERAGE)
        .map(|s| s.score)
        .fold(f64::INFINITY, f64::min);
    let best = segs.iter().min_by(|x, y| x.score.total_cmp(&y.score));
    Some(Comparison {
        covered_score,
        best_score: best.map_or(f64::INFINITY, |s| s.score),
        best_coverage: best.map_or(0.0, |s| s.duration(&cfg) as f64 / shorter),
    })
}

fn group_files(files: Vec<FileInfo>, edges: Vec<(i64, i64, f64)>) -> Vec<Group> {
    let n = files.len();
    let index: HashMap<i64, usize> = files.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
    let mut dsu = Dsu((0..n).collect());
    let mut proofs: Vec<(usize, usize, &'static str, Option<f64>)> = Vec::new();

    // 1. identical bytes
    let mut by_hash: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, f) in files.iter().enumerate() {
        by_hash.entry(f.sha256.as_str()).or_default().push(i);
    }
    for idx in by_hash.values().filter(|v| v.len() > 1) {
        for w in idx.windows(2) {
            dsu.union(w[0], w[1]);
            proofs.push((w[0], w[1], "hash", None));
        }
    }

    // 2. the same audio: the pairs the fingerprint job found (stored, not recomputed here)
    for (a, b, score) in edges {
        if score > MAX_SCORE {
            continue;
        }
        if let (Some(&i), Some(&j)) = (index.get(&a), index.get(&b)) {
            dsu.union(i, j);
            proofs.push((i, j, "audio", Some((score * 100.0).round() / 100.0)));
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
            idx.iter().map(|&i| files[i].track_id.clone()).collect();
        if tracks.len() == 1 && tracks.iter().next().unwrap().is_some() {
            continue;
        }
        let mut members: Vec<FileInfo> = idx.iter().map(|&i| files[i].clone()).collect();
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
                a: files[*a].file_id.clone(),
                b: files[*b].file_id.clone(),
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

/// Files, stored audio matches (a, b, score) and the number of fingerprints not compared yet.
type Listing = (Vec<FileInfo>, Vec<(i64, i64, f64)>, i64);

pub async fn list_duplicates(State(s): State<AppState>) -> Response {
    let loaded: rusqlite::Result<Listing> = (|| {
        let conn = db::lock(&s.db);
        let files = load(&conn)?;
        let edges = conn
            .prepare("SELECT a, b, score FROM audio_match WHERE score <= ?1")?
            .query_map([MAX_SCORE], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let pending: i64 = conn.query_row(
            "SELECT COUNT(*) FROM fingerprint WHERE status='ok' AND compared=0",
            [],
            |r| r.get(0),
        )?;
        Ok((files, edges, pending))
    })();
    match loaded {
        Ok((files, edges, pending)) => {
            match tokio::task::spawn_blocking(move || group_files(files, edges)).await {
                Ok(groups) => Json(json!({
                    "groups": groups,
                    "pending_comparisons": pending,
                    "note": "nothing is deleted; merging only joins files under one track"
                }))
                .into_response(),
                Err(e) => internal(e),
            }
        }
        Err(e) => internal(e),
    }
}

/// Compare every newly fingerprinted file with the rest of the library (within the duration
/// window) and store the pairs that sound alike. Runs at the end of the fingerprint job, a few
/// threads at a time. Returns (files compared, matching pairs stored).
pub fn compare_new(db: &std::sync::Mutex<rusqlite::Connection>) -> (usize, usize) {
    struct Row {
        id: i64,
        dur: i64,
        done: bool,
        fp: Vec<u32>,
    }
    let mut rows: Vec<Row> = {
        let conn = db::lock(db);
        let q = conn.prepare(
            "SELECT f.id, COALESCE(f.duration_ms, 0), p.compared, p.data FROM fingerprint p
             JOIN file f ON f.id = p.file_id AND f.sha256 = p.sha256 WHERE p.status = 'ok'",
        );
        match q.and_then(|mut st| {
            st.query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    dur: r.get(1)?,
                    done: r.get::<_, i64>(2)? != 0,
                    fp: fingerprint::from_blob(&r.get::<_, Vec<u8>>(3)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        }) {
            Ok(v) => v,
            Err(_) => return (0, 0),
        }
    };
    rows.sort_by_key(|r| r.dur);
    let todo: Vec<usize> = (0..rows.len()).filter(|&i| !rows[i].done).collect();
    if todo.is_empty() {
        return (0, 0);
    }
    let threads = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, 4);
    let chunk = todo.len().div_ceil(threads);
    let rows_ref = &rows;
    let edges: Vec<(i64, i64, f64)> = std::thread::scope(|scope| {
        let handles: Vec<_> = todo
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    let mut out = Vec::new();
                    for &i in part {
                        let lo = rows_ref
                            .partition_point(|r| r.dur < rows_ref[i].dur - DURATION_WINDOW_MS);
                        for j in lo..rows_ref.len() {
                            if rows_ref[j].dur > rows_ref[i].dur + DURATION_WINDOW_MS {
                                break;
                            }
                            // two new files are compared once: from the one with the smaller id
                            if j == i || (!rows_ref[j].done && rows_ref[j].id < rows_ref[i].id) {
                                continue;
                            }
                            if let Some(c) = compare(&rows_ref[i].fp, &rows_ref[j].fp)
                                && c.covered_score <= STORE_MAX_SCORE
                            {
                                let (a, b) = (
                                    rows_ref[i].id.min(rows_ref[j].id),
                                    rows_ref[i].id.max(rows_ref[j].id),
                                );
                                out.push((a, b, c.covered_score));
                            }
                        }
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    let mut conn = db::lock(db);
    let stored = edges.len();
    let _ = (|| -> rusqlite::Result<()> {
        let tx = conn.transaction()?;
        for (a, b, score) in &edges {
            tx.execute(
                "INSERT OR REPLACE INTO audio_match(a, b, score) VALUES(?1,?2,?3)",
                params![a, b, score],
            )?;
        }
        for &i in &todo {
            tx.execute(
                "UPDATE fingerprint SET compared = 1 WHERE file_id = ?1",
                [rows[i].id],
            )?;
        }
        tx.commit()
    })();
    (todo.len(), stored)
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
            // The owner's work is not lost: if the kept track was never corrected but an absorbed
            // one was, its corrected tags win; pending suggestions move to the kept track.
            let target_corrected: i64 =
                tx.query_row("SELECT corrected FROM track WHERE id=?1", [target], |r| {
                    r.get(0)
                })?;
            if target_corrected == 0 {
                tx.execute(
                    "UPDATE track SET title=l.title, artist=l.artist, album=l.album, mbid=l.mbid,
                       search_text=l.search_text, corrected=1
                     FROM (SELECT title, artist, album, mbid, search_text FROM track WHERE id=?2 AND corrected=1) AS l
                     WHERE track.id=?1",
                    params![target, t],
                )?;
            }
            tx.execute(
                "UPDATE OR IGNORE candidate SET track_id=?1 WHERE track_id=?2",
                params![target, t],
            )?;
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
