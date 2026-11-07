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
- Auth: one owner token, as `Authorization: Bearer` (scripts, widgets) or as an HttpOnly, SameSite=Strict cookie set by `POST /v1/login` (browsers, so `<audio>`, skins and the WebSocket work). The cookie is a random session id kept in server memory, never the token: `POST /v1/logout` revokes it, sessions last 30 days and are forgotten on restart, and at most 64 are kept. After 10 failed logins in a minute, logins answer 429 for the rest of that minute. `/v1/health`, `/v1/login` and the static web client are open (logout needs auth, so a cross-site form can't end your session). Cookie requests that mutate must be JSON or a raw upload (CSRF guard); the WebSocket refuses a foreign `Origin`. The cookie has no `Secure` flag because the server is often used over plain HTTP on a LAN; use TLS in front of a public endpoint. Read-only and per-device tokens are later work.
- Session: one global session. The playing browser **reports** (`POST /v1/session` `action=report`, with its own `client_id`) on track and state changes and at most every 10 s; the server stores it and broadcasts it on `/v1/events`. Widgets read `GET /v1/session` and send commands (`play|pause|next|previous|volume`), which the server forwards over the WebSocket to the client that last reported; only that client acts, so other open tabs don't start playing. A command answers 409 unless that client still has its `/v1/events` socket open (players connect with `?client_id=`). Position is not reported by the web client yet.
- Web client: built with Vite, served by the server itself from `MUSEQ_WEB_DIR` (same origin, SPA fallback), so cookies need no CORS. In development Vite proxies `/v1` to the server.

## Stack
| Layer | Choice | Note |
|---|---|---|
| Server | Rust: axum + tokio, rusqlite `bundled`, symphonia (decode, probe) | Android client stays Kotlin |
| DB | SQLite (WAL), server is the only writer | one file, `cp` backup |
| Files | plain directory, sha256 content-addressed | duplicates fall out of the hash |
| Transcode | **ffmpeg subprocess in the image** (MP3/AAC need it); in-process Opus (`opus`+`ogg`+`rubato`+`symphonia`) is an optional path, decided at the start of phase 5 by a measured test (one station, one core) | see "Audio lessons" |
| Radio out | Icecast-compatible chunked HTTP stream | seconds of latency, fine for radio |
| Fetch | yt-dlp subprocess behind a config flag, **off by default** | legal gate, see Risks |
| Metadata | MusicBrainz + Discogs (tags), AcoustID (identify) with fingerprints computed in-process in Rust (`rusty-chromaprint`, no `fpcalc`) | AcoustID: 3 req/s, non-commercial, app key (checked); MusicBrainz ~1 rps and Discogs ~60/min [unverified] |
| Web | TypeScript/Vite + Webamp (MIT, checked in its `package.json`) for the classic skin | don't call the product "Winamp" |
| Desktop | Browser first. A native shell (Tauri/`wry` around Webamp, or Qt Quick) is open: Q13, decided at phase 4 | cliamp is only an optional existing client |
| Android | Kotlin, Media3/ExoPlayer, `mediaPlayback` foreground service; mic radio = second service (`microphone` type) | skin = Webamp in a WebView for v1 (Q5) |
| Deploy | Docker/Podman, Compose, Caddy optional | phone-as-server = same binary, later |

## Data model (SQLite)
`file(id, track_id, sha256, path, bytes, codec, bitrate_kbps, sample_rate, channels, lossless, duration_ms)` (one track can own several files; `track.preferred_file_id` is the one played) · `fingerprint(file_id, sha256, status ok|unsupported|error, duration_ms, data)` ·
`track(id, title, artist, album, genre, duration_ms, mbid, corrected, preferred_file_id, search_text)` (artist and album are text columns for now; a file belongs to a track through `file.track_id`) · `candidate(id, track_id, source, score, title, artist, album, year, mbid, status)` ·
`playlist(id, name, kind user|generated|extension, seed_genres, extend BOOL)` + `playlist_item(playlist_id, position, track_id)` ·
`station(id, name, kind, category, url, codec, bitrate, schedule_json, source manual|omarchy|radiobrowser|trayfm)` ·
`job(id, kind scan|fetch|tag|fingerprint, payload JSON, state, attempts, result JSON, error)` (one table for every background job) ·
`duplicate_group` + `duplicate_member` ·
`device(id, name, platform, last_seen, offline_manifest_json)` · `user(id, name, token_hash, role)`.

Rules: byte identity = sha256. Song identity for dedupe = fingerprint (later); until then hash + normalized artist/title, never pretend tags are content. Best file = lossless > bitrate > sample rate; losers stay until the owner deletes. Tag writes happen only on explicit accept (candidates are stored, not applied). Playlist extension never calls the network inside the "track ended" request. Jobs are retried with backoff and keep partial results.

## API conventions (the contract is `docs/openapi.yaml`)
- JSON over HTTP; one WebSocket (`/v1/events`) for session and job events.
- Audio ids are **file ids**. Playlists and the session refer to track ids; the server picks the track's preferred file.
- `POST /v1/jobs` with `kind` scan|check_stations|fetch|tag|fingerprint; `fetch` answers 403 unless enabled in config; one job of a kind at a time (409 otherwise); `scan` and `fingerprint` also exclude each other.
- Station addresses: a station URL must be http(s) with a host and no credentials. By default loopback, private, link-local (including the cloud metadata address), carrier-grade NAT and unique-local addresses are refused, both when adding a station and in every hop of the liveness check (redirects are followed by hand, at most 3, within one 10 s deadline). Radio on your own network needs `MUSEQ_ALLOW_PRIVATE_STATIONS=1`. Limit: a name is resolved again when connecting, so a hostile DNS server could answer differently the second time; stations are added by the owner only.
- Errors are `{ "error": "message" }` with a normal HTTP status, including unknown `/v1/...` paths. Malformed ids or bodies that axum rejects before our code runs (a non-numeric `{id}`, invalid JSON) still answer with its plain-text 400/422.
- Duplicates (`GET /v1/duplicates`, `POST /v1/duplicates/merge`): identical bytes are found by hash, the same recording in different files by an in-process Chromaprint fingerprint (job `fingerprint`, two files at a time, never together with a scan). A merge joins whole tracks (every file a named file's track owns comes along) in SQLite only; the owner's corrected tags and pending suggestions of an absorbed track are kept: no file is ever deleted or rewritten, and playlist entries follow. Opus cannot be fingerprinted yet (Symphonia has no Opus decoder); such files get status `unsupported`.
- Metadata (`tag` job, `GET /v1/tracks/{id}/candidates`, `POST /v1/tracks/{id}/identify`, `POST /v1/candidates/{id}/accept|reject`): MusicBrainz text search for tracks with a missing artist or album (1 request per second, descriptive User-Agent, one shared rate limiter for all jobs), and AcoustID by audio when the owner supplies an application key (`MUSEQ_ACOUSTID_KEY`; 501 without it). Answers become **candidates**; accepting one changes the track in the database only (never the audio file) and marks the track corrected so rescans keep it. CI and tests only ever talk to fake servers.
- Playlist extension (`POST /v1/playlists/{id}/extend`): tracks that share a genre with the playlist (heavier genres first, accent- and case-insensitive), by an artist who is not in it yet, one track per new artist, from the local library only; when nothing qualifies it says so and adds nothing. It never calls the network. The web client calls it when Webamp reaches the end of a playlist that has "extend" switched on, appends the tracks and plays on.
- Implemented so far: health, login/logout, tracks, audio (Range), playlists, scan jobs, session + events, skins, stations (list, add, delete, liveness check job, remote play), fingerprint job, duplicates (list, merge), metadata candidates (MusicBrainz, AcoustID), playlist extension. Search is case- and accent-insensitive (Greek tonos and final sigma fold) through a `search_text` column.

## Modules
1 api · 2 library (scan, tags, hash, quality) · 3 fetch (Spotify link → ISRC/title via Deezer → YouTube Music search → yt-dlp → tag+import) · 4 metadata · 5 dedupe (Chromaprint similarity, keep best, never auto-delete) · 6 playlistext (same genre/style, skip artists already present, introduce new; local pool first, online suggestions only once fetch exists) · 7 radio · 8 transcode · 9 offline (manifest diff, resume) · 10 auth · clients: web, android, desktop, widgets (Omarchy bar widget reading now-playing).

## What enters the image, by phase
| Phase | Added to the image |
|---|---|
| 1 | server binary only (symphonia reads tags and durations; library path and token via env/config) |
| 2 | `data/stations/` seed lists (shipped in the image, imported at startup) |
| 3 | nothing for fingerprints, duplicates, suggestions and extension (all in-process Rust); `yt-dlp` (pinned, flag off by default) only with 3d |
| 5 | `ffmpeg` (apt or a pinned LGPL build with checksum) |
| 6 | nothing new (libopus decode in-process) |

## Skins
Classic `.wsz` = zip of BMP/CUR/TXT. `docs/SPEC-skin.md` describes storage and the web renderer; a golden-image test is added when the second renderer exists. Bundle only skins whose licence allows it.

## Audio lessons (from an earlier recording project, applied when relevant)
- Opus: flush the encoder lookahead (`pre_skip` 312) or the tail is cut; read the last Ogg page's granule for duration; pin one resampler; the Rust Opus path is only proven at speech bitrates, music bitrates are unverified.
- ffmpeg as a tool: pinned build with SHA-256, keep the child handle and kill it on quit, cancel is not an error, hide the console window on Windows.
- Capture/playback (native clients only): cpal wants a fixed ~20 ms buffer, WASAPI loopback goes silent in silence, a device switch can lose audio.
- Packaging: AppImage/NSIS with checksums, smoke-test the packaged binary, short `CARGO_TARGET_DIR` on Windows (260-char path limit), test from a clean checkout (a `.gitignore` once hid a needed file).

## Non-goals for v1
Modern `.wal` skins, lyrics, scrobbling, multi-tenant hosting, iOS, DRM sources, video, realtime talkback, a custom skin language, a Spotify client of our own, a cliamp fork.

## Risks
- **Legal/ToS:** yt-dlp on YouTube, Spotify/Deezer APIs as a back door, re-streaming Greek stations. Mitigation: fetch flag off by default, personal use, passthrough for external stations, no credentials shipped. The owner must read the three sites' terms before phase 3 enables fetch; a summary is not that reading.
- **Fingerprinting:** Chromaprint-style fingerprints computed in-process in Rust, server-side, for dedupe (no API). Naming a song from audio = AcoustID. No official Android Chromaprint build found [unverified]; phone-side later. Chromaprint's own licence is unverified (use the FFmpeg-based build).
- **Android:** foreground services, audio focus, OEM battery killers; needs a real-handset test. Ducking our own music under our mic is a gain on our mixer bus.
- **Skin parity**, **yt-dlp breakage** (pin + self-update + health check), **CPU** (one encoder per station, 80% cap).
- **Licences:** cliamp MIT (Bjarne Øverli), Webamp MIT (checked; the licence of its bundled base skin is not), Tray-FM station list Unlicense.
