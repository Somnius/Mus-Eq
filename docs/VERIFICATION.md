# Verification plan

Things the roadmap marks "not verified", with the check that settles each one, what counts as a pass, and what only the owner can provide. Real-music checks are read-only on the owner's library: copies and variants live in the git-ignored `.work/` folder, and only aggregate numbers (counts, score ranges) go into these docs, never titles or paths.

Status: 2026-11-07, planned. Results are recorded in `ROADMAP.md` as they come in.

## 1. Duplicate thresholds on real music
`MAX_SCORE` 2.0, `MIN_COVERAGE` 0.8 and the 4 s duration window were tuned on synthetic noise.
- **Check:** take about 20-30 real tracks (MP3 first), write variants with ffmpeg (MP3 at 64/128/320 kbps, Ogg Vorbis, gain of plus and minus 6 dB, a second of silence trimmed, a fade-in, the first minute only), scan and fingerprint them in a throwaway server under `.work/`, then dump every pairwise score and coverage. Label pairs: same recording, unrelated, same artist or album but a different song, and live, remix or instrumental versions of the same song.
- **Pass:** every re-encode or gain variant of one source is grouped; no false merge among roughly 400 unrelated pairs; same-album pairs score clearly above the true matches. Pick `MAX_SCORE` in the gap between the worst true match and the best false one; if the groups overlap, prefer missing a duplicate over a false merge.
- **Owner decision:** is a partial copy (the first minute only) a duplicate? The 4 s window currently hides such pairs.
- **Effort:** half a day.

## 2. Opus
Symphonia 0.6.1 has no Opus decoder. Most of the owner's library is Opus (172 of 205 local audio files), so without this the fingerprint job skips most of the real library.
- **Options:** the `opus` crate (libopus, BSD) with the `ogg` crate; a young pure-Rust port (to be checked for licence, maintenance and correctness before use); an ffmpeg subprocess (already planned for phase 5, but heavy for a fingerprint pass).
- **Recommendation:** libopus through the `opus` crate behind a cargo feature; it can later also decode the phone's live uplink (phase 6). Keep the pure-Rust port as a later swap candidate.
- **Check:** decode about 10 real Opus files with our path and with `ffmpeg -f f32le`, compare after aligning the pre-skip (maximum absolute difference below 1e-3, same sample count); fingerprint both and require a match score below 0.5; time all Opus files on one core; make sure the container image still builds.
- **Needs the owner:** libopus headers (a system package, installed with approval) if they are not present; a yes if pure Rust is a hard rule.
- **Effort:** one to two days.

## 3. AcoustID compatibility
No `fpcalc` here, so our compressed fingerprint has never been checked against the real service.
- **Needs the owner:** a free AcoustID application key (non-commercial use), and optionally a yes to install the Chromaprint package for a byte-for-byte comparison.
- **Check:** identify about 10 well-known tracks; stay far below the rate limit; with `fpcalc` available, compare its output with ours on the same files.
- **Pass:** at least 7 of 10 return the expected recording with a score above 0.9. A rejected fingerprint points to a compression or algorithm-id mismatch; an accepted one with no result points to the wrong sample rate or duration.
- **Effort:** one hour after the key exists.

## 4. Real MusicBrainz replies
Only fake servers were used so far.
- **Check:** at most 30 requests, one per second, with the project's User-Agent, using crafted well-known queries (exact title and artist, title only, a Greek title, nonsense). Save the raw replies under `.work/`.
- **Compare with the fake:** field names (`artist-credit`, `releases`, the type of `score`), sparse records, error behaviour. Record the differences first, then fix the parser and refresh the test fixtures with sanitised samples.
- **Effort:** one hour. No key.

## 5. Speed and memory at scale
- **Check:** generate 10,000 to 50,000 synthetic fingerprints (no audio) and a few thousand tiny files; record scan time per 1000 files, `GET /v1/duplicates` wall time and memory, and database size, under the 80% CPU rule.
- **Decision rule:** if `/v1/duplicates` takes more than about 5 s or about 1 GB at 20,000 files, build an incremental index (store results per pair, compare only new files) and paging.
- **Effort:** half a day. No owner input.

## 6. A real station in a real browser
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
`catch_unwind` already turns a decoder panic into an error for one file, so a corrupt file is the wrong test. Two tests are needed: a test-only hook that panics inside the wrapped decoder (the file must be marked as errored and the job finish), and one that panics outside it (the job must end as `failed` and must not block the next scan). Also feed a truncated FLAC and a zero-byte MP3 to the real decoder to see whether it errors or panics. **Effort:** under half a day.

## 10. Smaller items
- Stations refused as "not public" once and fine the next time: log the resolved addresses on repeated runs.
- Playlist extension on a real library: count how many real tracks have a genre tag at all (counts only).

## Order
**Now, without the owner:** items 9, 5, 4, 6, item 1 on MP3, and an Opus prototype.
**Needs a key, a package or a decision:** 3 (key), 2 (headers and the pure-Rust question), 1 (the partial-copy question), 7 (two devices), 8 (Podman).
**Not worth doing yet:** Discogs, web pages for duplicates and suggestions, full-library Opus timing before item 2 lands.
