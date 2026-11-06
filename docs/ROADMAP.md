# Roadmap (updated at the end of every phase)

Status: `todo` / `doing` / `done`. Exit criteria are the definition of done. Estimates are rough.
Last updated: 2026-11-06 (planning only, no code).

| Phase | Name | Status |
|---|---|---|
| 0 | Decisions, specs, scaffold | doing (exit met; skin spec, Q3 and repo reminder left) |
| 1 | Library on LAN + web player (+ cliamp, Omarchy widget) | todo |
| 2 | Stations (external radio, Greek list) | todo |
| 3 | Metadata, duplicates, fetch bot, playlist extension | todo |
| 4 | Native clients + offline (Android, desktop) + skins | todo |
| 5 | Library radio + transcoding | todo |
| 6 | Live phone radio (mic, ducking, mixer) | todo |
| 7 | Polish: phone-as-server, on-device dedupe, more skins | todo |

## Phase 0: decisions, specs, scaffold (~1 week)
- [x] Answer Q1-Q4 (`docs/OPEN_QUESTIONS.md`): Q1 Rust, Q4 Tray-FM list. Q3 (Omarchy radio) still open, only blocks phase 2
- [x] `docs/openapi.yaml` (v1 draft, lints clean with 19 style warnings) + `docs/SPEC-api.md`
- [ ] `docs/SPEC-skin.md`
- [x] Repo layout (cargo workspace `crates/museq-server`), `Containerfile`, `compose.yaml`
- [x] CI workflow written (`.github/workflows/ci.yml`), untested: no remote yet
- [x] Initial commit
- [ ] Remind Lef to create the GitHub repo `Mus-Eq` + README and push
Exit: `docker/podman run museq-server` answers `/v1/health`; OpenAPI validates. **Met 2026-11-06** with Docker (image 119 MB; curl from the host returned 200 `{"status":"ok","version":"0.1.0"}`; redocly 2.76.0 lint valid). Podman not tested (not installed).

## Phase 1: library on LAN + web player (~3-4 weeks)
Scan folder, SQLite, search, playlist CRUD, range streaming, Webamp web client with default classic skin + `.wsz` upload, cliamp pointed at the server, Omarchy now-playing widget, Compose file.
Exit: Lef plays a local album in the browser and from another machine over LAN/Tailscale; container restart keeps the catalog.

## Phase 2: stations (~1-2 weeks)
Station table, the Omarchy radio (Q3), Greek list (Q4), error shown for dead URLs without killing the process.
Exit: browser and cliamp play the station through MusEq.

## Phase 3: metadata, duplicates, fetch, extension (~4-6 weeks)
Tags from MusicBrainz/Discogs (accept before write), `fpcalc` duplicate groups with a keep-best proposal (never auto-delete), AcoustID identify-by-audio (look at free-API reality here), fetch jobs for singles and playlists behind the flag, playlist extension (local pool, same genre, new artists only).
Exit: a mistagged file shows a candidate and changes only on accept; two copies of one rip collapse to one preferred file (a known pair is the fixture); a short playlist grows by N tracks with no repeated artist; fetch fails cleanly with the flag off.

## Phase 4: native clients + offline (~4-6 weeks)
Android (Media3, foreground service, offline pins, lock-screen controls), Linux/Windows native (cliamp plugin first, Qt Quick later), skin parity test.
Exit: Android plays offline with the screen off for 1 h; Windows runs the same `.wsz` as the web, or "classic everywhere" is dropped and written down.

## Phase 5: library radio + transcoding (~2-3 weeks)
Library stations with rotation rules, codec presets (Opus/Ogg, MP3, AAC), Icecast-compatible output, embed snippet, tokens for other apps.
Exit: a station runs 24 h under 1 CPU core; VLC, browser and the widget play it.

## Phase 6: live phone radio (~3-4 weeks)
Mic capture, ducking, 2-channel mixer, uplink (HTTP Ogg/Opus first), server re-broadcast. Real handset test.
Exit: music drops while Lef speaks and returns after; a web listener hears the mix within ~5 s.

## Phase 7: polish (ongoing)
Phone-as-server (Q7), on-device dedupe (Q8), second skins.

## Phase log
- 2026-11-06 phase 0 exit criteria met; Rust chosen over Go; Greek station seed added.
(one line per finished phase)
