//! Threshold tuning on real audio. Not part of the normal test run: it needs a folder of files
//! named `<set>-<n>__<variant>.<ext>` (`MUSEQ_TUNE_DIR`), where the part before `__` identifies
//! one recording (so the same id means "same recording") and `<set>` groups recordings that
//! sound alike (the same mix or album, the hard negatives).
//!
//!   MUSEQ_TUNE_DIR=.work/tune/files cargo test --test tune -- --ignored --nocapture
//!
//! Prints distributions only; with MUSEQ_TUNE_CSV=<path> it also writes one line per pair.
use museq_server::duplicates::compare;
use museq_server::fingerprint::fingerprint_file;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[test]
#[ignore]
fn score_distributions_on_real_audio() {
    let Ok(dir) = std::env::var("MUSEQ_TUNE_DIR") else {
        println!("set MUSEQ_TUNE_DIR to run this");
        return;
    };
    let mut files: Vec<(String, String, Vec<u32>, i64)> = Vec::new(); // id, variant, fp, ms
    let mut failed = 0;
    for e in std::fs::read_dir(PathBuf::from(dir)).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let stem = name
            .rsplit_once('.')
            .map_or(name.as_str(), |x| x.0)
            .to_string();
        let Some((id, variant)) = stem.split_once("__") else {
            continue;
        };
        match fingerprint_file(&e.path()) {
            Ok((fp, ms)) => files.push((id.to_string(), variant.to_string(), fp, ms)),
            Err(err) => {
                failed += 1;
                println!("could not fingerprint {name}: {err:?}");
            }
        }
    }
    files.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    println!("{} files fingerprinted, {failed} failed", files.len());

    // class -> (covered scores, best scores)
    let mut classes: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut by_variant: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut csv = String::from(
        "class,variant_a,variant_b,covered_score,best_score,best_coverage,ms_a,ms_b\n",
    );
    for i in 0..files.len() {
        for j in i + 1..files.len() {
            let (a, b) = (&files[i], &files[j]);
            let class = if a.0 == b.0 {
                "same_recording"
            } else if a.0.split('-').next() == b.0.split('-').next() {
                "same_set_different_recording"
            } else {
                "unrelated"
            };
            let Some(c) = compare(&a.2, &b.2) else {
                continue;
            };
            let score = if c.covered_score.is_finite() {
                c.covered_score
            } else {
                99.0
            };
            classes.entry(class.into()).or_default().push(score);
            if class == "same_recording" {
                by_variant
                    .entry(format!("{} vs {}", a.1, b.1))
                    .or_default()
                    .push(score);
            }
            csv.push_str(&format!(
                "{class},{},{},{:.3},{:.3},{:.3},{},{}\n",
                a.1, b.1, c.covered_score, c.best_score, c.best_coverage, a.3, b.3
            ));
        }
    }
    for (class, v) in classes.iter_mut() {
        let n = v.len();
        let (min, p50, p95, max) = (pct(v, 0.0), pct(v, 0.5), pct(v, 0.95), pct(v, 1.0));
        println!(
            "{class:30} pairs={n:6}  min={min:6.2}  median={p50:6.2}  p95={p95:6.2}  max={max:6.2}   (99 = no stretch covers enough)"
        );
    }
    println!("\nsame recording, by variant pair (median / max score):");
    for (k, v) in by_variant.iter_mut() {
        println!(
            "  {k:28} n={:3}  median={:5.2}  max={:5.2}",
            v.len(),
            pct(v, 0.5),
            pct(v, 1.0)
        );
    }
    for t in [1.0, 1.5, 2.0, 2.5, 3.0, 4.0] {
        let fp = classes
            .iter()
            .filter(|(k, _)| k.as_str() != "same_recording")
            .map(|(_, v)| v.iter().filter(|s| **s <= t).count())
            .sum::<usize>();
        let tp = classes
            .get("same_recording")
            .map_or(0, |v| v.iter().filter(|s| **s <= t).count());
        let total_pos = classes.get("same_recording").map_or(0, Vec::len);
        println!(
            "MAX_SCORE {t:3.1}: finds {tp}/{total_pos} same-recording pairs, {fp} false merges"
        );
    }
    if let Ok(path) = std::env::var("MUSEQ_TUNE_CSV") {
        std::fs::write(path, csv).unwrap();
    }
}
