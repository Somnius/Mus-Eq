# Verification plan

Things the roadmap marks "not verified", with the check that settles each one, what counts as a pass, and what only the owner can provide. Real-music checks are read-only on the owner's library: copies and variants live in the git-ignored `.work/` folder, and only aggregate numbers (counts, score ranges) go into these docs, never titles or paths.

Status: 2026-11-07. Items 1, 2, 4, 5, 6 and 9 are done (results below); 3, 7, 8 and 11 wait for something only the owner can provide.

The numbers in items 1 and 2 come from a one-off run on the owner's library through `tests/tune.rs` (an `#[ignore]` test that reads `MUSEQ_TUNE_DIR`). The library is not in the repository, so those numbers cannot be reproduced from a checkout; the test only shows how they were produced.

## 1. Duplicate thresholds on real music
**Result (done):** 188 labelled files built read-only from 20 radio tracks and 6 hour-long lofi mixes (twelve clips, two per mix): lossless, MP3, Ogg Vorbis and Opus re-encodes, a gain change, a trimmed head and a partial copy. All 492 same-recording pairs scored 0.00 to 1.27 (median 0.42). The 17,086 other pairs, 9,406 of them from the same mix, produced no stretch that scored below 7.4 and none that covered more than 11% of the shorter recording: zero false merges at every threshold up to 4.0. `MAX_SCORE` 2.0 and `MIN_COVERAGE` 0.8 stay; they sit in a gap of about six points. Partial copies are found too when the length window is lifted (the window stays at 4 s, so they are not offered). Not covered: live, remix and instrumental versions of one song, which the library does not contain. Tool: `tests/tune.rs`.

Original plan:
`MAX_SCORE` 2.0, `MIN_COVERAGE` 0.8 and the 4 s duration window were tuned on synthetic noise.
- **Check:** take about 20-30 real tracks (MP3 first), write variants with ffmpeg (MP3 at 64/128/320 kbps, Ogg Vorbis, gain of plus and minus 6 dB, a second of silence trimmed, a fade-in, the first minute only), scan and fingerprint them in a throwaway server under `.work/`, then dump every pairwise score and coverage. Label pairs: same recording, unrelated, same artist or album but a different song, and live, remix or instrumental versions of the same song.
- **Pass:** every re-encode or gain variant of one source is grouped; no false merge among roughly 400 unrelated pairs; same-album pairs score clearly above the true matches. Pick `MAX_SCORE` in the gap between the worst true match and the best false one; if the groups overlap, prefer missing a duplicate over a false merge.
- **Owner decision:** is a partial copy (the first minute only) a duplicate? The 4 s window currently hides such pairs.
- **Effort:** half a day.

## 2. Opus
**Result (done):** implemented with the `opus` crate (libopus, built from source, needs cmake) and the `ogg` crate, Ogg Opus mono and stereo. Over 288,000 samples the decoded audio differs from ffmpeg's by at most one least-significant bit (mean 0.000), and the end trim from the last page's granule position gives the exact duration. Real Opus re-encodes fingerprint and match (score median 0.55, maximum 1.07 against the lossless original). Speed: 3.4 hours of real Opus audio in 26 seconds on one core, about 470 times real time, so the whole library takes about ten minutes of one core. Opus with more than two channels is reported as unsupported.

Original plan:
Symphonia 0.6.1 has no Opus decoder. Most of the owner's library is Opus (172 of 205 local audio files), so without this the fingerprint job skips most of the real library.
- **Options:** the `opus` crate (libopus, BSD) with the `ogg` crate; a young pure-Rust port (to be checked for licence, maintenance and correctness before use); an ffmpeg subprocess (already planned for phase 5, but heavy for a fingerprint pass).
- **Recommendation:** libopus through the `opus` crate behind a cargo feature; it can later also decode the phone's live uplink (phase 6). Keep the pure-Rust port as a later swap candidate.
- **Check:** decode about 10 real Opus files with our path and with `ffmpeg -f f32le`, compare after aligning the pre-skip (maximum absolute difference below 1e-3, same sample count); fingerprint both and require a match score below 0.5; time all Opus files on one core; make sure the container image still builds.
- **Needs the owner:** libopus headers (a system package, installed with approval) if they are not present; a yes if pure Rust is a hard rule.
- **Effort:** one to two days.

## 3. AcoustID compatibility
**Status:** waiting for the owner's application key. Everything else is in place (`MUSEQ_ACOUSTID_KEY`, fingerprints in the AcoustID format, the key is never in results or errors).

Original plan:
No `fpcalc` here, so our compressed fingerprint has never been checked against the real service.
- **Needs the owner:** a free AcoustID application key (non-commercial use), and optionally a yes to install the Chromaprint package for a byte-for-byte comparison.
- **Check:** identify about 10 well-known tracks; stay far below the rate limit; with `fpcalc` available, compare its output with ours on the same files.
- **Pass:** at least 7 of 10 return the expected recording with a score above 0.9. A rejected fingerprint points to a compression or algorithm-id mismatch; an accepted one with no result points to the wrong sample rate or duration.
- **Effort:** one hour after the key exists.

## 4. Real MusicBrainz replies
**Result (done):** ten crafted public queries at one per second. The reply shape matches the parser. Two findings fixed: MusicBrainz answered 503 to 2 of 10 requests (the client now retries 429 and 503 with backoff), and the top hits for a famous song were mostly bootleg and live releases, so the search now asks for official studio albums first, falls back to the plain search, chooses the best release of each recording and ranks by score then earliest release. A real reply is kept as a test fixture.

Original plan:
Only fake servers were used so far.
- **Check:** at most 30 requests, one per second, with the project's User-Agent, using crafted well-known queries (exact title and artist, title only, a Greek title, nonsense). Save the raw replies under `.work/`.
- **Compare with the fake:** field names (`artist-credit`, `releases`, the type of `score`), sparse records, error behaviour. Record the differences first, then fix the parser and refresh the test fixtures with sanitised samples.
- **Effort:** one hour. No key.

## 5. Speed and memory at scale
**Result (done), and it found a real problem:** listing duplicates recomputed every comparison on each request (4.5 s at 2,000 files, 28 s at 5,000, 112 s at 10,000, 449 s at 20,000). Comparisons are now computed once, by the fingerprint job, and stored (`audio_match`): 20,000 synthetic files take 161 s the first time (four threads), a run with nothing new 0.1 s, and the duplicate list 0.03 s; peak memory 394 MB. Scan and tag timings at scale are still unmeasured.

Original plan:
- **Check:** generate 10,000 to 50,000 synthetic fingerprints (no audio) and a few thousand tiny files; record scan time per 1000 files, `GET /v1/duplicates` wall time and memory, and database size, under the 80% CPU rule.
- **Decision rule:** if `/v1/duplicates` takes more than about 5 s or about 1 GB at 20,000 files, build an incremental index (store results per pair, compare only new files) and paging.
- **Effort:** half a day. No owner input.

## 6. A real station in a real browser
**Result (done):** Omarchy Radio and seven Greek stations (HTTP and HTTPS) all start playing in headless Chrome with no media errors (`web/e2e/net-stations.mjs`, opt-in, needs network). The plain-HTTP-on-an-HTTPS-page case is untested (no TLS in the test setup). Live titles are not read.

Original plan:
- **Check:** extend the headless-browser test, behind an opt-in switch that CI never uses, to play Omarchy Radio and two or three Greek stations for about 10 s each (one HTTPS, one plain HTTP, one old Shoutcast): the audio element reaches `playing`, the console shows no CORS or mixed-content error, a plain-HTTP station stays disabled on an HTTPS page. Live titles (ICY metadata) are only logged, since playback does not depend on them.
- **Effort:** a few hours. No owner input.

## 7. Another machine on the LAN or Tailscale
- **Setup:** start the server with `MUSEQ_BIND=0.0.0.0:8080` (or the Tailscale address), publish the port, use the owner token.
- **One-minute check from a phone or another PC:** open the page, log in, play a track and seek (a `206` for a Range request), see now-playing update on the other device (WebSocket), confirm the health endpoint answers without a token and everything else refuses without one.
- **Needs the owner:** two devices.

## 8. Podman
- **Check:** `podman build -f Containerfile`, then the compose file rootless; same smoke test as item 7. Watch for `restart: unless-stopped` (needs the Podman systemd service) and read-only bind mounts.
- **Needs the owner:** approval to install Podman. **Effort:** one hour.

## 9. A crash inside a job
**Result (done):** a decoder panic on one file leaves that file catalogued without tags and the job finishes; a panic outside every guard ends the job as `failed` with a clear error and does not block the next scan; truncated, empty and garbage files are survived. A test-only hook (`MUSEQ_TEST_HOOKS=1`) triggers the panics.

Original plan:
`catch_unwind` already turns a decoder panic into an error for one file, so a corrupt file is the wrong test. Two tests are needed: a test-only hook that panics inside the wrapped decoder (the file must be marked as errored and the job finish), and one that panics outside it (the job must end as `failed` and must not block the next scan). Also feed a truncated FLAC and a zero-byte MP3 to the real decoder to see whether it errors or panics. **Effort:** under half a day.

## 10. Smaller items
- Stations refused as "not public" once and fine the next time: log the resolved addresses on repeated runs.
- Playlist extension on a real library: count how many real tracks have a genre tag at all (counts only).

## 11. Fetch against the real services
**Status:** not done. Every fetch test uses a fake `yt-dlp` script and a fake Spotify page; no request to YouTube Music or Spotify has been made from this code.
- **Needs the owner:** having read the terms of the services, the acknowledgement in `MUSEQ_FETCH_ACK`, and their own `yt-dlp` binary.
- **Check:** one YouTube Music link and one Spotify track link the owner may store, on a throwaway library under `.work/`. Record whether the Spotify page still carries the expected meta tags, whether yt-dlp's search JSON and `--print after_move:filepath` output match what the code reads, which upload the matcher picked and why, and the stored provenance row. Then a second fetch of the same link (must be skipped) and a link to a removed video (must fail with "gone").
- **Pass:** the track plays from the library with the right title and artist; nothing outside the inbox changes; the cookies file (if any) is byte-identical afterwards.
- **Effort:** half an hour once enabled.

## Order
**Now, without the owner:** items 9, 5, 4, 6, item 1 on MP3, and an Opus prototype.
**Needs a key, a package or a decision:** 3 (key), 2 (headers and the pure-Rust question), 1 (the partial-copy question), 7 (two devices), 8 (Podman), 11 (terms read, acknowledgement, yt-dlp).
**Not worth doing yet:** Discogs, web pages for duplicates and suggestions, full-library Opus timing before item 2 lands.
