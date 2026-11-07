// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Lef
//! Scale checks on synthetic data. Not part of the normal run:
//!   MUSEQ_SCALE_N=20000 cargo test --release --test scale -- --ignored --nocapture
use axum::{
    body::Body,
    http::{Method, Request, header},
};
use http_body_util::BodyExt;
use museq_server::{api, db, fingerprint};
use std::time::Instant;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789";

fn rss_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |key: &str| {
        s.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    kb("VmHWM:") / 1024.0
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[tokio::test]
#[ignore]
async fn duplicate_listing_at_scale() {
    let n: usize = std::env::var("MUSEQ_SCALE_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let st = api::AppState::new(
        db::open_in_memory().unwrap(),
        std::env::temp_dir(),
        TOKEN.into(),
        std::env::temp_dir(),
        None,
    );
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let t0 = Instant::now();
    {
        let mut c = db::lock(&st.db);
        let tx = c.transaction().unwrap();
        for i in 0..n {
            let secs = 120 + (rng.next() % 240) as i64; // 2 to 6 minutes
            let items = (secs as f64 / 0.1238) as usize;
            let fp: Vec<u32> = (0..items).map(|_| rng.next() as u32).collect();
            tx.execute("INSERT INTO file(sha256, path, bytes, mtime, duration_ms, codec) VALUES(?1,?2,1,1,?3,'mp3')", rusqlite::params![format!("h{i}"), format!("{i}.mp3"), secs * 1000]).unwrap();
            let fid = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO track(title, preferred_file_id, search_text) VALUES(?1,?2,?1)",
                rusqlite::params![format!("t{i}"), fid],
            )
            .unwrap();
            let tid = tx.last_insert_rowid();
            tx.execute(
                "UPDATE file SET track_id=?1 WHERE id=?2",
                rusqlite::params![tid, fid],
            )
            .unwrap();
            tx.execute("INSERT INTO fingerprint(file_id, sha256, status, duration_ms, data) VALUES(?1,?2,'ok',?3,?4)", rusqlite::params![fid, format!("h{i}"), secs * 1000, fingerprint::to_blob(&fp)]).unwrap();
        }
        tx.commit().unwrap();
    }
    println!(
        "built {n} synthetic files with fingerprints in {:.1}s",
        t0.elapsed().as_secs_f64()
    );
    let t = Instant::now();
    let (compared, matches) = museq_server::duplicates::compare_new(&st.db);
    println!(
        "compare_new over {n} files: compared {compared}, stored {matches} pairs, in {:.1}s, peak memory {:.0} MB",
        t.elapsed().as_secs_f64(),
        rss_mb()
    );
    let t = Instant::now();
    let (again, _) = museq_server::duplicates::compare_new(&st.db);
    println!(
        "a second run with nothing new: compared {again} files in {:.2}s",
        t.elapsed().as_secs_f64()
    );
    let app = api::router(st);
    let before = rss_mb();
    let t = Instant::now();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/duplicates")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    println!(
        "GET /v1/duplicates over {n} files: {status} in {:.2}s, peak memory {:.0} MB (was {:.0} MB), reply {} bytes",
        t.elapsed().as_secs_f64(),
        rss_mb(),
        before,
        bytes.len()
    );
}
