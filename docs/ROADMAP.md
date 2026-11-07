# Roadmap

Status and exit checks only; behavior is in `SPEC.md`, choices in `DECISIONS.md`. Tick a box only when it is verified. Estimates are rough. Last updated: 2026-11-07.

| Phase | Name | Status |
|---|---|---|
| 0 | Decisions, specs, scaffold | done |
| 1a | Server core: config, token, SQLite, scan, tracks, range audio | done |
| 1b | Web client (Webamp), session + WebSocket | done (pushed, CI green) |
| 2 | Stations (external list, passthrough) | done (pushed, CI green) |
| 3 | Metadata, duplicates, extension, fetch (3a-3d) | 3a-3c built and reviewed; 3d built on fakes (single tracks, off by default), review and real run pending |
| 4 | Native clients + offline (Android, desktop) | todo |
| 5 | Library radio + transcoding | todo |
| 6 | Live phone radio (mic, ducking, mixer) | todo |
| 7 | Polish: phone-as-server, on-device dedupe, more skins | todo |

## Phase gate (applies to every phase)
1. Tests are written and run (unit + integration; container check where relevant).
2. Independent reviews of the code and docs by two reviewers; findings are fixed or logged.
3. Docs updated: this roadmap, `SPEC.md`, `DECISIONS.md`, `openapi.yaml`.
4. The owner gives the green light before the next phase starts.

## Phase 0: decisions, specs, scaffold (done 2026-11-06/07)
Rust workspace, `/v1/health`, `docs/openapi.yaml` (lint valid), Containerfile + compose, CI workflow, public repo, `v0.1.0`.
Exit met: container answers `/v1/health` from the host (Docker; Podman untested); OpenAPI lint valid; CI green on GitHub.

## Phase 1a: server core (done 2026-11-07)
- [x] Config from env (library path, DB, token, bind); bearer auth on everything except `/v1/health`
- [x] SQLite schema + `user_version` migration; one `job` table (`kind=scan`)
- [x] Scanner: walk a folder, sha256, tags and duration via symphonia (mp3, flac and Opus fixtures; Opus duration comes out right); rescan skips unchanged files and drops vanished ones
- [x] `GET /v1/tracks?q=`, `GET /v1/tracks/{id}`, `GET /v1/audio/{file_id}` with Range
- [x] Playlist CRUD (create, list, get, delete, add item, replace items for remove/reorder)
- [x] 6 integration tests, clippy clean; verified in Docker: 401 without token, scan, search, 206 Range, catalog survives a restart, token not in logs
Image adds: nothing.
Exit met: with `curl` and a token a mixed folder scans, searches and streams with Range, and a restart keeps the catalog. Not yet checked: from another machine on the LAN (only localhost was tried), CI on GitHub for this commit.
Follow-ups done the same day: accent/case-insensitive Greek search (`search_text`, migration v2), a crashed scan now marks its job failed instead of blocking later scans, nullable fields fixed in the contract. Not tested: the scan-panic path (only reasoned), and a request from another machine on the LAN (owner to try: `curl http://<server-ip>:8080/v1/health` after publishing the port beyond localhost).
Known gaps carried to later: artist/album are plain text columns (own tables when dedupe needs them); `<audio>` elements can't send an Authorization header, so phase 1b needs a cookie or a short-lived signed URL for audio; the DB mutex is held for short queries only, fine for one owner.

## Phase 1b: web client + session (done 2026-11-07; reviewed, green light given, pushed, CI green)
- [x] `docs/SPEC-skin.md`
- [x] Server: cookie login/logout (random server-side session id, rate limited, CSRF guard), `GET/POST /v1/session` (report and commands, commands only while the reporting client's socket is open), `/v1/events` WebSocket with Origin check, skin upload/list/download, static web serving with SPA fallback, JSON 404 for unknown API paths
- [x] Web client (Vite + TypeScript + Webamp 2.3.1, MIT): login, library with search, Play/Add, skin upload and picker, session reporting (on change and every 10 s while playing), remote commands
- [x] Tests: 18 Rust tests (unit + integration, incl. WebSocket and migration) and a headless-browser smoke test (login, Greek search, Webamp renders, session reports the track, remote pause obeyed, no console errors); `scripts/check.sh` runs everything; CI now also runs `cargo test` and builds the web client
- [x] `contrib/now-playing.sh` for status bars and widgets (token passed to curl via stdin, not the command line)
- [x] Review findings fixed (see below)
- [ ] Omarchy bar-widget packaging: not done. The plugin format is unverified, so only the script exists
- [ ] Read-only token: dropped for now. The widget uses the owner token on the owner's machine
- [ ] Position reporting from the web client (not reported yet)
- [ ] Not tested: a scan that panics (only reasoned), access from another machine on the LAN, Podman
Review: two independent reviews found no auth bypass, traversal or SQL injection. Fixed: cookie was the token itself and logout couldn't revoke it; moved files lost their playlist entries; file+track insert not atomic; a command was accepted when only some other socket was open; no periodic report; no login rate limit; logout was unauthenticated; unknown `/v1` paths returned the web page; CI skipped tests; token visible in the widget script's process list; several contract gaps. Accepted and documented: Webamp's private state hook, plain-text framework rejections for malformed ids.
Image adds: the built web client (node build stage).
Exit: play a local album in the browser from another machine; the widget shows now-playing. Checked on localhost only; the other-machine check is still the owner's to try.

## Phase 2: stations (done 2026-11-07; reviewed, green light given, pushed, CI green)
- [x] Station table (migration v4), idempotent seed import from `data/stations/` (128 Tray-FM stations plus Mus-Eq's own list, Omarchy Radio first), several seed lists via `MUSEQ_STATIONS_SEED`
- [x] `GET/POST /v1/stations`, `GET/DELETE /v1/stations/{id}`: accent-insensitive Greek search, category and live filters, http(s)-only URL validation
- [x] Liveness check as a job (`check_stations`, 8 at a time, one 10 s deadline per stream, redirects followed by hand and every hop checked, handles old Shoutcast "ICY 200" replies, audio sniffing when the content type is missing, records why a stream failed; a dead URL never crashes anything).
- [x] Station addresses are restricted: http(s) with a host, no credentials, and no loopback/private/link-local targets unless `MUSEQ_ALLOW_PRIVATE_STATIONS=1` (the review found the check could be aimed at internal services) A real run on the 129 stations (Tray-FM list plus Omarchy Radio) with the final checks: 103 answered the first time and 107 the second, 22-26 did not (mostly HTTP 404, some DNS failures and connection errors). In the first run four stations were refused as "not public" and were fine in the second run: the cause (probably a transient DNS answer) is not verified
- [x] Web client: stations panel (search, category, live-only, status dots, Check streams, Play) with a plain audio element, because most streams send no CORS headers and Webamp needs them; plain-HTTP streams are disabled on HTTPS pages
- [x] Session can report a station; remote commands: `play` with `station_id` or `track_id` starts it in the player, pause/volume reach the station
- [x] The Omarchy radio (Q3): `https://radio.cliamp.stream/omarchy/stream`
- [x] Tests: 31 Rust tests (stations with fake radio servers incl. ICY, HTML, 404, refused, redirect, no-content-type sniffing; private-address refusal; session; migration) plus the headless-browser smoke test with a fake radio station (plays in the browser, session reports it, remote pause and remote play-station work)
- [x] CI now has an `e2e` job that runs `scripts/check.sh` (browser smoke test included). Verified on GitHub after the push: all four jobs (rust, web, openapi, e2e) passed
- [ ] Not tested: a real station played in a real browser from this code (only the fake station); liveness checks over IPv6; the upstream list's quality (26 of 128 were down today)
- [x] Phase gate: independent test run (31 Rust tests + 17 browser checks), two reviews, owner's green light
Image adds: `data/stations/` (the seed lists).
Exit: the browser plays a station from the list through the plain audio element; a dead URL shows an error and nothing crashes. Met on the fake station and on the live check.

## Side track: Omarchy plugin (after phase 2)
First-party bar widget in its own repository, built from the findings in `docs/PLUGINS.md`: service + bar-widget, session polling, token file, station picker. Needs the station command above, cover art and position in the session.

## Phase 3: metadata, duplicates, extension, fetch (split into four steps; plan 2026-11-07)
Checked before starting (2026-11-07): the pure-Rust fingerprint crate `rusty-chromaprint` (MIT, one `rubato`, no second Symphonia) fingerprints FLAC, MP3 and OGG through Symphonia and matches re-encodes (same noise source: score 0.33 as MP3, 1.07 as OGG; a different source scored 7.05; lower is better). **Symphonia 0.6.1 cannot decode Opus** ("unsupported audio codec"), so Opus files are marked "unsupported for fingerprinting" until ffmpeg joins the image (phase 5). Compatibility of its compressed fingerprint with AcoustID lookups is **unverified** (no `fpcalc` here to compare; it needs a real lookup with a registered key). The score thresholds come from synthetic noise only and must be checked on real music.

### 3a: fingerprints and duplicates (built 2026-11-07, no network)
- [x] Migration v5 (tested with a real version-1 database and its data): one track can own several files (`file.track_id`), plus sample rate, channels, average bitrate, lossless flag and genre; every file is re-probed on the next scan
- [x] `fingerprint` job: decode with Symphonia, fingerprint in Rust, 2 at a time, never together with a scan, tolerant of files that vanish; Opus and unreadable files get a status, never fail the job
- [x] `GET /v1/duplicates`: exact (same hash) and audio (fingerprint) groups, each with a proposed keeper and the proof; nothing is ever deleted
- [x] `POST /v1/duplicates/merge`: merge in SQLite only (files stay on disk and stream; playlist entries re-pointed; a merged track survives the loss of its preferred file)
- [x] Tests: seeded-noise fixtures (FLAC, MP3, OGG of one source, a different source), an Opus file, exact copies, merge validation, quality columns, scan/fingerprint exclusion
- [x] Verified later the same day (see `VERIFICATION.md`): thresholds on real music (492 true pairs 0.00-1.27, no false merge among 17,086 others), Opus decoding identical to ffmpeg within one bit and fingerprinted at about 470 times real time, stored comparisons (20,000 files: duplicate list in 0.03 s instead of 449 s)
- [ ] Still not verified: AcoustID acceptance of our fingerprints (needs the owner's key), live and remix versions of one song, a web page for duplicates (API only)
Exit met on fixtures: the same source as FLAC, MP3 and OGG becomes one track with the FLAC preferred, playlists keep working, the other source stays apart.

### 3b: metadata candidates (built 2026-11-07)
- [x] MusicBrainz text search for tracks with a missing artist or album (job `tag`, optional `track_ids`), 1 request per second behind one shared limiter, descriptive User-Agent (`MUSEQ_CONTACT`), configurable base URLs, candidates below a score of 70 dropped
- [x] Candidates stored; `accept` applies title, artist and album to the track in the database only (the audio file is never rewritten, checked byte for byte), marks the track corrected so rescans keep it, and supersedes the other pending candidates; `reject` dismisses one
- [x] `POST /v1/tracks/{id}/identify` through AcoustID: 501 without the owner's key (`MUSEQ_ACOUSTID_KEY`), 409 without a fingerprint
- [x] Tests against fake MusicBrainz and AcoustID servers only (request shape, User-Agent, 503 handling, no duplicate candidates on a second run, rescan keeps a correction, parsing); the rate limiter spacing is tested
- [x] Real MusicBrainz replies checked (ten queries): the parser matches; fixed 503 retries and release choice (official studio albums first). Still open: AcoustID with a real key; Discogs (needs the owner's token); a web page for suggestions (API only)
Exit met on fakes: a mistagged file shows a candidate and changes only on accept.

### 3c: playlist extension (local pool) and a minimal playlist UI (built 2026-11-07)
- [x] `POST /v1/playlists/{id}/extend`: same genre, artists not already in the playlist (accents and case ignored), one track per new artist, local library only; says so when the pool is empty or the playlist has no genre to match
- [x] Web client: playlist list, create (with an "extend" switch), "+ Playlist" buttons on library rows, play a playlist in Webamp; when Webamp reaches the end of a playlist with extend on, the new tracks are appended and played
- [x] Tests: 6 server tests (rules, accents, rotation, genre weight, API behaviour) and a browser test (a one-track playlist ends, grows by 3 tracks by new artists, playback carries on). Webamp needed an explicit "select the first new track and play" after the end: "next" alone did not restart it
- [ ] Not verified: how the choice feels on a real library (the pick is deterministic and rotates, with no similarity beyond the genre tag); libraries without genre tags get no extension (the reason is shown); artists are matched by tag text only (no MusicBrainz ids yet)
Exit met on fixtures: a short playlist grows by N tracks with no repeated artist.

### Review of 3a-3c (2026-11-07)
Two independent reviews and a separate full check run (48 tests then, 53 now, all 20 browser checks). No code path deletes or rewrites an audio file; every new route is behind auth; the AcoustID key stays out of errors and results (now pinned by a test). Fixed: migrations are atomic per step (a failed step changes nothing, tested); merge keeps the owner's corrections and moves pending suggestions; accepting a sparse candidate no longer blanks artist or album; suggestions without an id no longer pile up; tracks expose their genre; `tag` on named tracks includes corrected ones and is capped at 1000 ids; SPEC data model and job rules corrected. Accepted and noted: `/v1/duplicates` compares in memory (fine for a home library, not verified at scale); `tag` is bounded only by the MusicBrainz rate limit.

### 3d: fetch (built 2026-11-07, single tracks only, no real network)
- [x] Off by default: `MUSEQ_FETCH=on` plus the typed acknowledgement `MUSEQ_FETCH_ACK` plus an absolute path to an existing `yt-dlp`; `GET /v1/fetch/status` says what is missing; `fetch` jobs answer 403 while off (tested: default config, missing acknowledgement, relative or missing tool path, cookies file with loose permissions)
- [x] Link parsing: single YouTube/YouTube Music videos and single Spotify tracks only; everything else (http, playlists, albums, other hosts, odd ids) answers 400 before anything runs (tested)
- [x] YouTube link: yt-dlp run without a shell on a server-built `watch?v=<id>` address, file kept only if it stayed in the work folder with an audio extension and under the size cap, stored in the inbox under a sanitised name, scanned, fingerprinted, tagged from the source, recorded in `provenance` (migration v8); a second fetch of the same video is skipped (tested with a fake yt-dlp script)
- [x] Spotify link: track page read from its meta tags (fake page server in tests), yt-dlp search, matcher (title overlap, duration, artist, version words, Topic-channel preference); a changed page fails without downloading; private hosts refused (tested)
- [x] Failure handling: tool errors sorted into plain reasons and the job fails with nothing left behind; a run over the timeout is killed; cookies are used only after a sign-in wall, as a per-run copy, and the original is never written (tested); hostile titles cannot leave the inbox (tested)
- [x] One fetch at a time, never beside a scan or fingerprint job (tested)
- [x] 17 tests in `tests/fetch.rs`, all against a fake yt-dlp and a fake Spotify page; no test touches the network
- [ ] **Not done:** playlists and albums (refused with a message); Deezer (not used at all; the Spotify page carries what the matcher needs); `yt-dlp` in the container image (the owner supplies the path); a web page for fetching (API only)
- [ ] **Not verified:** no real YouTube Music or Spotify request has been made from this code. The Spotify meta-tag names, yt-dlp's search JSON shape and the `--print after_move:filepath` output were written from documentation and memory, not from a live run; the first real run will settle them. The matcher's thresholds (0.6 overlap, 5 s) come from hand-written candidates only
Exit met on fakes: fetch fails cleanly with the flag off (403 with the reason); with the fake tool a link becomes a catalogued, fingerprinted track with provenance.

Image adds: nothing for 3a to 3d (fingerprinting is in-process Rust; `yt-dlp` is not shipped yet).
Exit (whole phase): a known duplicate pair collapses to one preferred file; a mistagged file changes only on accept; a short playlist grows with no repeated artist; fetch fails cleanly with the flag off. Gate before 3d: independent check, both reviews, docs, green light.

## Phase 4: native clients + offline (~4-6 weeks)
- [ ] Android: Media3, foreground service, offline pins, lock-screen controls
- [ ] Desktop shell decision (Q13); optional cliamp provider
- [ ] Skin parity golden-image test
- [ ] Packaging checklist: AppImage/NSIS + SHA-256, smoke test on a clean archive, short Windows target dir, pinned tool downloads
Exit: Android plays offline with the screen off for 1 h; the same `.wsz` renders on web and the chosen desktop shell, or "classic everywhere" is dropped and written down.

## Phase 5: library radio + transcoding (~2-3 weeks)
- [ ] Measured test: in-process Opus vs ffmpeg for one station on one core; record the decision
- [x] Library radios, first slice (built 2026-11-07): create by name, optional genre, codec preset and artist gap; one shared ffmpeg encode at playing speed per radio, started by the first listener and stopped when the last leaves; rotation never repeats a file inside the gap or the same artist twice in a row; `icy-name` header; `tests/radio.rs` (rotation property test, validation, two listeners share one stream, encoder stops). **Not done / unverified:** Opus/Ogg listeners who join mid-track may get no stream headers until the next track (mp3 and aac tested only); players that cannot send a token (VLC) cannot reach the stream yet, so "VLC plays it" is not met; at most 4 radios encode at once and 50 can be defined; schedules, jingles, ICY metadata in the stream (track titles), Icecast source protocol, load test; the 24 h one-core exit is NOT measured
- [x] `codec`/`kbps` on `/v1/audio/{file_id}` (built 2026-11-07: ffmpeg subprocess, no shell, streamed, two at once, 501 without ffmpeg; `tests/transcode.rs`, 5 tests incl. fingerprint match of the transcode; `MUSEQ_FFMPEG` sets the path). Not done: caching, Range on transcodes, a failed ffmpeg mid-stream still ends as a short 200, a stalled client holds a slot until it disconnects, ffmpeg in the image is the distro package (not the pinned build SPEC asks for), bitrate sizes not measured
Image adds: `ffmpeg`.
Exit: a station runs 24 h under 1 CPU core; VLC, the browser and the widget play it.

## Phase 6: live phone radio (~3-4 weeks)
- [ ] Mic capture, ducking, 2-channel mixer on the phone; uplink Ogg/Opus over HTTP
- [ ] Server decodes the uplink in-process (libopus) and re-broadcasts
Exit: real handset: music drops while the owner speaks and returns after; a web listener hears the mix within ~5 s.

## Phase 7: polish (ongoing)
Phone-as-server (Q7), on-device dedupe (Q8), second skins.

## Phase log
- 2026-11-07 Phase 3d built: fetch module (YouTube and Spotify single tracks via yt-dlp, provenance table, typed acknowledgement), 17 tests on a fake tool and a fake page; no real network run, review pending.
- 2026-11-06 Phase 0 scaffold, Rust chosen, Greek station seed added.
- 2026-11-07 Phase 2 (stations) reviewed by two reviewers, fixed (private-address refusal, redirect checks, audio sniffing), pushed; CI green incl. the browser test.
- 2026-11-07 Phase 1 (1a+1b) reviewed by two reviewers, fixed, pushed; CI (rust incl. tests, web, openapi) green.
- 2026-11-07 Phase 1b built: cookie login, session + WebSocket, skins, Webamp client, now-playing script (review pending).
- 2026-11-07 Phase 1a built: scan, tracks, Range audio, playlists, token auth.
- 2026-11-07 Public repo + `v0.1.0`; docs refactored (SPEC-api folded into SPEC, DECISIONS added); phase 1 split into 1a/1b.
