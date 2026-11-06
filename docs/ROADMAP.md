# Roadmap

Status and exit checks only; behavior is in `SPEC.md`, choices in `DECISIONS.md`. Tick a box only when it is verified. Estimates are rough. Last updated: 2026-11-07.

| Phase | Name | Status |
|---|---|---|
| 0 | Decisions, specs, scaffold | done |
| 1a | Server core: config, token, SQLite, scan, tracks, range audio | done (pending CI run) |
| 1b | Web client (Webamp), session + WebSocket | todo |
| 2 | Stations (external list, passthrough) | todo |
| 3 | Metadata, duplicates, fetch, playlist extension | todo |
| 4 | Native clients + offline (Android, desktop) | todo |
| 5 | Library radio + transcoding | todo |
| 6 | Live phone radio (mic, ducking, mixer) | todo |
| 7 | Polish: phone-as-server, on-device dedupe, more skins | todo |

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
Known gaps carried to later: artist/album are plain text columns (own tables when dedupe needs them); `<audio>` elements can't send an Authorization header, so phase 1b needs a cookie or a short-lived signed URL for audio; the DB mutex is held for short queries only, fine for one owner.

## Phase 1b: web client + session (~2 weeks)
- [ ] `docs/SPEC-skin.md` (write first)
- [ ] Vite + TypeScript client with Webamp, default classic skin, `.wsz` upload
- [ ] `GET/POST /v1/session` and `/v1/events` (WebSocket); one global session
- [ ] Omarchy now-playing widget reads the session API (read-only token)
Exit: play a local album in the browser from another machine; the widget shows now-playing.

## Phase 2: stations (~1 week)
- [ ] Station table seeded from `data/stations/trayfm-stations.txt`; add `data/` to the image
- [ ] Liveness check job for the stream URLs; dead ones flagged, not deleted
- [ ] `GET /v1/stations` returns the upstream URL; the client plays it directly (no proxying)
- [ ] The Omarchy radio (Q3)
Exit: browser plays a station from the list; a dead URL shows an error and nothing crashes.

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
- 2026-11-07 Phase 1a built: scan, tracks, Range audio, playlists, token auth.
- 2026-11-07 Public repo + `v0.1.0`; docs refactored (SPEC-api folded into SPEC, DECISIONS added); phase 1 split into 1a/1b.
