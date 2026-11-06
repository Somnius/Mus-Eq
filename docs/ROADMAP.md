# Roadmap

Status and exit checks only; behavior is in `SPEC.md`, choices in `DECISIONS.md`. Tick a box only when it is verified. Estimates are rough. Last updated: 2026-11-07.

| Phase | Name | Status |
|---|---|---|
| 0 | Decisions, specs, scaffold | done |
| 1a | Server core: config, token, SQLite, scan, tracks, range audio | done |
| 1b | Web client (Webamp), session + WebSocket | done (pushed, CI green) |
| 2 | Stations (external list, passthrough) | in progress |
| 3 | Metadata, duplicates, fetch, playlist extension | todo |
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

## Phase 2: stations (~1 week)
- [ ] Station table seeded from `data/stations/trayfm-stations.txt`; add `data/` to the image
- [ ] Liveness check job for the stream URLs; dead ones flagged, not deleted
- [ ] `GET /v1/stations` returns the upstream URL; the client plays it directly (no proxying)
- [x] The Omarchy radio (Q3): `https://radio.cliamp.stream/omarchy/stream` is in `data/stations/museq-stations.txt`
- [ ] Remote "play this station" command through `POST /v1/session` (needed by a bar widget; see `docs/PLUGINS.md`)
Exit: browser plays a station from the list; a dead URL shows an error and nothing crashes.

## Side track: Omarchy plugin (after phase 2)
First-party bar widget in its own repository, built from the findings in `docs/PLUGINS.md`: service + bar-widget, session polling, token file, station picker. Needs the station command above, cover art and position in the session.

## Phase 3: metadata, duplicates, fetch, extension (~4-6 weeks)
Jobs retry with backoff and keep partial results (`attempts`).
- [ ] Tag candidates from MusicBrainz/Discogs, stored and applied only on accept
- [ ] `fpcalc` duplicate groups with a keep-best proposal, never auto-delete
- [ ] AcoustID identify-by-audio (check the free-API reality first)
- [ ] Fetch jobs for singles and playlists behind the flag (owner has read the terms)
- [ ] Playlist extension: same genre, new artists only, local pool first
Image adds: `fpcalc`, pinned `yt-dlp`.
Exit: a mistagged file shows a candidate and changes only on accept; a known duplicate pair collapses to one preferred file; a short playlist grows by N tracks with no repeated artist; fetch fails cleanly with the flag off.

## Phase 4: native clients + offline (~4-6 weeks)
- [ ] Android: Media3, foreground service, offline pins, lock-screen controls
- [ ] Desktop shell decision (Q13); optional cliamp provider
- [ ] Skin parity golden-image test
- [ ] Packaging checklist: AppImage/NSIS + SHA-256, smoke test on a clean archive, short Windows target dir, pinned tool downloads
Exit: Android plays offline with the screen off for 1 h; the same `.wsz` renders on web and the chosen desktop shell, or "classic everywhere" is dropped and written down.

## Phase 5: library radio + transcoding (~2-3 weeks)
- [ ] Measured test: in-process Opus vs ffmpeg for one station on one core; record the decision
- [ ] Library stations with rotation rules, codec presets (Opus/Ogg, MP3, AAC), Icecast-compatible output
- [ ] `codec`/`kbps` on `/v1/audio/{file_id}`
Image adds: `ffmpeg`.
Exit: a station runs 24 h under 1 CPU core; VLC, the browser and the widget play it.

## Phase 6: live phone radio (~3-4 weeks)
- [ ] Mic capture, ducking, 2-channel mixer on the phone; uplink Ogg/Opus over HTTP
- [ ] Server decodes the uplink in-process (libopus) and re-broadcasts
Exit: real handset: music drops while the owner speaks and returns after; a web listener hears the mix within ~5 s.

## Phase 7: polish (ongoing)
Phone-as-server (Q7), on-device dedupe (Q8), second skins.

## Phase log
- 2026-11-06 Phase 0 scaffold, Rust chosen, Greek station seed added.
- 2026-11-07 Phase 1 (1a+1b) reviewed by two reviewers, fixed, pushed; CI (rust incl. tests, web, openapi) green.
- 2026-11-07 Phase 1b built: cookie login, session + WebSocket, skins, Webamp client, now-playing script (review pending).
- 2026-11-07 Phase 1a built: scan, tracks, Range audio, playlists, token auth.
- 2026-11-07 Public repo + `v0.1.0`; docs refactored (SPEC-api folded into SPEC, DECISIONS added); phase 1 split into 1a/1b.
