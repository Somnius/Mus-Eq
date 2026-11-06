# MusEq spec (v0.2, 2026-11-07)

Behavior, architecture and rules. The API contract is `docs/openapi.yaml`, status and exit checks are in `docs/ROADMAP.md`, answered questions are logged in `docs/DECISIONS.md`, open ones in `docs/OPEN_QUESTIONS.md`. `docs/BRIEF.md` is the frozen raw input. Items marked **[unverified]** were not checked against a primary source and get re-verified before the phase that needs them.

## What it is
A self-hosted music server (one headless container) plus thin clients: web player, native Windows/Linux/macOS/Android players, one skin engine (classic Winamp look first). It stores a library, fetches tracks on request, builds and extends playlists, fixes metadata, finds duplicates, and streams radio (external stations, a "fake radio" from the library, a live phone-mic channel). One HTTP+WebSocket API drives everything, so widgets and other apps can control it.

## Architecture
```
clients: web / desktop / Android / widgets / other apps
            |  HTTPS JSON + WS events + audio URLs
            v
museq-server (one container, one process)
  api | library(sqlite) | fetch (flag, off by default) | metadata | dedupe
      | playlistext | radio (external/library/live) | transcode | offline | auth
phone "radio uplink" (Ogg/Opus over HTTP chunks, WebRTC later) --> radio(live)
```
- The server owns the catalog and jobs. A phone cache is a replica with a sync cursor, never a second library.
- Playing a stored file is a byte stream (`/v1/audio/{file_id}`, Range supported). Transcoding (`codec`, `kbps`) only when a client asks; until phase 5 the server ignores or refuses it (501), it never silently re-encodes.
- Radio has `kind = external | library | live`. **External stations are never proxied**: `/v1/stations` returns the upstream URL and the client plays it (no re-streaming, avoids licensing trouble, no load on the server). `/v1/radio/{id}.{ext}` exists only for library and live stations.
- Mic, ducking and mixer live on the phone (the mic is there); the server receives an already-mixed stream.
- One global playback session: fine for one listener; per-device sessions come later if needed.
- Bind to localhost by default (`MUSEQ_BIND`). LAN/Tailscale/VPN/tunnel is the owner's network; a public endpoint is opt-in and needs token auth + TLS.
- Auth: one owner bearer token (token file or env) from phase 1; `/v1/health` is open. Read-only tokens for widgets and per-device tokens later.

## Stack
| Layer | Choice | Note |
|---|---|---|
| Server | Rust: axum + tokio, rusqlite `bundled`, symphonia (decode, probe) | Android client stays Kotlin |
| DB | SQLite (WAL), server is the only writer | one file, `cp` backup |
| Files | plain directory, sha256 content-addressed | duplicates fall out of the hash |
| Transcode | **ffmpeg subprocess in the image** (MP3/AAC need it); in-process Opus (`opus`+`ogg`+`rubato`+`symphonia`) is an optional path, decided at the start of phase 5 by a measured test (one station, one core) | see "Audio lessons" |
| Radio out | Icecast-compatible chunked HTTP stream | seconds of latency, fine for radio |
| Fetch | yt-dlp subprocess behind a config flag, **off by default** | legal gate, see Risks |
| Metadata | MusicBrainz + Discogs (tags), AcoustID/Chromaprint via `fpcalc` subprocess (identify, dedupe) | AcoustID: 3 req/s, non-commercial, app key (checked); MusicBrainz ~1 rps and Discogs ~60/min [unverified] |
| Web | TypeScript/Vite + Webamp (MIT [unverified]) for the classic skin | don't call the product "Winamp" |
| Desktop | Browser first. A native shell (Tauri/`wry` around Webamp, or Qt Quick) is open: Q13, decided at phase 4 | cliamp is only an optional existing client |
| Android | Kotlin, Media3/ExoPlayer, `mediaPlayback` foreground service; mic radio = second service (`microphone` type) | skin = Webamp in a WebView for v1 (Q5) |
| Deploy | Docker/Podman, Compose, Caddy optional | phone-as-server = same binary, later |

## Data model (SQLite)
`file(id, sha256, path, bytes, codec, bitrate, sample_rate, duration_ms)` ·
`track(id, title, artist_id, album_id, mbid, acoustid, chromaprint, preferred_file_id, quality_score, source_kind, source_ref)` + `track_file(track_id, file_id, quality_rank)` ·
`artist`, `album` ·
`playlist(id, name, kind user|generated|extension, seed_genres, extend BOOL)` + `playlist_item(playlist_id, position, track_id)` ·
`station(id, name, kind, category, url, codec, bitrate, schedule_json, source manual|omarchy|radiobrowser|trayfm)` ·
`job(id, kind scan|fetch|tag|fingerprint, payload JSON, state, attempts, result JSON, error)` (one table for every background job) ·
`duplicate_group` + `duplicate_member` ·
`device(id, name, platform, last_seen, offline_manifest_json)` · `user(id, name, token_hash, role)`.

Rules: byte identity = sha256. Song identity for dedupe = fingerprint (later); until then hash + normalized artist/title, never pretend tags are content. Best file = lossless > bitrate > sample rate; losers stay until the owner deletes. Tag writes happen only on explicit accept (candidates are stored, not applied). Playlist extension never calls the network inside the "track ended" request. Jobs are retried with backoff and keep partial results.

## API conventions (the contract is `docs/openapi.yaml`)
- JSON over HTTP; one WebSocket (`/v1/events`) for session and job events.
- Audio ids are **file ids**. Playlists and the session refer to track ids; the server picks the track's preferred file.
- `POST /v1/jobs` with `kind` scan|fetch|tag|fingerprint; `fetch` answers 403 unless enabled in config.
- Errors are `{ "error": "message" }` with a normal HTTP status.
- Implemented so far: health, tracks, audio (Range), playlists, scan jobs. Search is case- and accent-insensitive (Greek tonos and final sigma fold) through a `search_text` column.

## Modules
1 api · 2 library (scan, tags, hash, quality) · 3 fetch (Spotify link → ISRC/title via Deezer → YouTube Music search → yt-dlp → tag+import) · 4 metadata · 5 dedupe (Chromaprint similarity, keep best, never auto-delete) · 6 playlistext (same genre/style, skip artists already present, introduce new; local pool first, online suggestions only once fetch exists) · 7 radio · 8 transcode · 9 offline (manifest diff, resume) · 10 auth · clients: web, android, desktop, widgets (Omarchy bar widget reading now-playing).

## What enters the image, by phase
| Phase | Added to the image |
|---|---|
| 1 | server binary only (symphonia reads tags and durations; library path and token via env/config) |
| 2 | `data/stations/` seed (currently excluded by `.dockerignore`; fix then) |
| 3 | `fpcalc`, `yt-dlp` (pinned, flag off by default) |
| 5 | `ffmpeg` (apt or a pinned LGPL build with checksum) |
| 6 | nothing new (libopus decode in-process) |

## Skins
Classic `.wsz` = zip of BMP/CUR/TXT. `docs/SPEC-skin.md` is written at the start of phase 1b, with a golden-image test so renderers (Webamp, WebView, native) don't drift. Bundle only skins whose licence allows it.

## Audio lessons (from an earlier recording project, applied when relevant)
- Opus: flush the encoder lookahead (`pre_skip` 312) or the tail is cut; read the last Ogg page's granule for duration; pin one resampler; the Rust Opus path is only proven at speech bitrates, music bitrates are unverified.
- ffmpeg as a tool: pinned build with SHA-256, keep the child handle and kill it on quit, cancel is not an error, hide the console window on Windows.
- Capture/playback (native clients only): cpal wants a fixed ~20 ms buffer, WASAPI loopback goes silent in silence, a device switch can lose audio.
- Packaging: AppImage/NSIS with checksums, smoke-test the packaged binary, short `CARGO_TARGET_DIR` on Windows (260-char path limit), test from a clean checkout (a `.gitignore` once hid a needed file).

## Non-goals for v1
Modern `.wal` skins, lyrics, scrobbling, multi-tenant hosting, iOS, DRM sources, video, realtime talkback, a custom skin language, a Spotify client of our own, a cliamp fork.

## Risks
- **Legal/ToS:** yt-dlp on YouTube, Spotify/Deezer APIs as a back door, re-streaming Greek stations. Mitigation: fetch flag off by default, personal use, passthrough for external stations, no credentials shipped. The owner must read the three sites' terms before phase 3 enables fetch; a summary is not that reading.
- **Fingerprinting:** Chromaprint/`fpcalc` server-side for dedupe (no API). Naming a song from audio = AcoustID. No official Android Chromaprint build found [unverified]; phone-side later. Chromaprint's own licence is unverified (use the FFmpeg-based build).
- **Android:** foreground services, audio focus, OEM battery killers; needs a real-handset test. Ducking our own music under our mic is a gain on our mixer bus.
- **Skin parity**, **yt-dlp breakage** (pin + self-update + health check), **CPU** (one encoder per station, 80% cap).
- **Licences:** cliamp MIT (Bjarne Øverli), Webamp MIT [unverified], Tray-FM station list Unlicense.
