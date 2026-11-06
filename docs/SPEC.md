# MusEq spec (v0, 2026-11-06)

Input: `docs/BRIEF.md`. Items marked **[unverified]** were not checked against a primary source and get re-verified before the phase that needs them.

## What it is
A self-hosted music server (one headless container) plus thin clients: web player, native Windows/Linux/Android players, one skin engine (classic Winamp look first). It stores a library, fetches tracks on request, builds and extends playlists, fixes metadata, finds duplicates, and streams radio (external stations, a "fake radio" from the library, a live phone-mic channel). One HTTP+WebSocket API drives everything, so widgets and other apps can control it.

## Architecture
```
clients: web / Linux+Win app / Android / widgets / other apps
            |  HTTPS JSON + WS events + audio URLs
            v
museq-server (one container, one process)
  api | library(sqlite) | fetch (flag, off by default) | metadata | dedupe
      | playlistext | radio (external/library/live) | transcode(ffmpeg) | offline | auth
phone "radio uplink" (Opus over HTTP chunks, WebRTC later) --> radio(live)
```
- The server owns the catalog and jobs. A phone cache is a replica with a sync cursor, never a second library.
- Playing a stored file is a byte stream (`/audio/{id}`, range requests). Transcoding only when a client asks for another codec.
- Radio is one interface with `kind = external | library | live`. External stations are passed through by default (no re-streaming, avoids licensing trouble).
- Mic, ducking and mixer live on the phone (the mic is there); the server receives an already-mixed stream.
- Bind to localhost by default. LAN/Tailscale/VPN/tunnel is the owner's network; a public endpoint is opt-in and needs token auth + TLS.

## Stack (language decided: Rust)
| Layer | Choice | Note |
|---|---|---|
| Server | **Rust** (axum + tokio, rusqlite `bundled`, symphonia); ffmpeg/yt-dlp/fpcalc as subprocesses | Rust fits the owner and the cross-platform/Android story. cliamp stays an unmodified Linux client; its station/yt-dlp logic is a reference (MIT notice if copied), not a base. |
| DB | SQLite (WAL), server is the only writer | one file, `cp` backup |
| Files | plain directory, sha256 content-addressed | duplicates fall out of the hash |
| Transcode | ffmpeg presets (Opus/Ogg, MP3, AAC); `-threads 2`, `nice` | CPU cap 80% |
| Radio out | Icecast-compatible HTTP chunked stream | seconds of latency, fine for radio |
| Fetch | yt-dlp subprocess behind a config flag, **off by default** | legal gate, see LEGAL risks |
| Metadata | MusicBrainz + Discogs (tags), AcoustID/Chromaprint (identify, dedupe) | 1 rps MB, 3 rps AcoustID, Discogs ~60/min [partly unverified] |
| Web | TypeScript/Vite + Webamp (MIT) for the classic skin | do not call the product "Winamp" |
| Linux / Windows / macOS | Browser first; cliamp (Linux) unchanged; later a Tauri/`wry` shell hosting Webamp (Qt Quick was the earlier idea, undecided) | macOS added 2026-11-06 |
| Android | Kotlin, Media3/ExoPlayer, `mediaPlayback` foreground service; mic radio = second service (`microphone` type) | skin = Webamp in a WebView for v1 (Q4) |
| Deploy | Podman/Docker, Compose, Caddy optional | phone-as-server = same binary, later |

## Data model (SQLite)
`file(id, sha256, path, bytes, codec, bitrate, sample_rate, duration_ms)` ·
`track(id, title, artist_id, album_id, mbid, acoustid, chromaprint, preferred_file_id, quality_score, source_kind, source_ref)` + `track_file(track_id, file_id, quality_rank)` ·
`artist`, `album` ·
`playlist(id, name, kind user|generated|extension, seed_genres, extend BOOL)` + `playlist_item` ·
`station(id, name, kind, url, codec, bitrate, schedule_json, source manual|omarchy|radiobrowser)` ·
`fetch_job`, `metadata_job`, `job` ·
`duplicate_group` + `duplicate_member` ·
`device(id, name, platform, last_seen, offline_manifest_json)` · `user(id, name, token_hash, role)`.

Rules: byte identity = sha256. Song identity for dedupe = fingerprint (later); until then hash + normalized artist/title, never pretend tags are content. Best file = lossless > bitrate > sample rate; losers stay until Lef deletes. Tag writes happen on explicit accept. Playlist extension never calls the network inside the "track ended" request.

## API (v1, OpenAPI is the contract)
`GET /v1/tracks?q=` · `POST /v1/playlists`, `/items` · `GET /v1/audio/{file_id}` (range) · `GET/POST /v1/session` (`play|pause|next|volume`) · `GET /v1/stations` · `GET /v1/radio/{id}.ogg|.mp3` · `POST /v1/jobs` (`fetch|tag|fingerprint`), `GET /v1/jobs/{id}` · `GET /v1/events` (WS) · skins upload. Auth: bearer token per device/user; read-only tokens for widgets.

## Modules
1 api · 2 library (scan, tags, hash, quality) · 3 fetch (Spotify link → ISRC/title via Deezer → YouTube Music search → yt-dlp → tag+import) · 4 metadata · 5 dedupe (Chromaprint similarity, keep best, never auto-delete) · 6 playlistext (same genre/style, skip artists already present, introduce new; local pool first, online suggestions only once fetch exists) · 7 radio · 8 transcode · 9 offline (manifest diff, resume) · 10 auth · clients: web, android, desktop, widgets (Omarchy bar widget reading now-playing).

## Skins
Classic `.wsz` = zip of BMP/CUR/TXT. One `SPEC-skin.md` (written in phase 0/1) and a golden-image test so renderers (Webamp, WebView, Qt) don't drift. Bundle only skins whose licence allows it.

## Non-goals for v1
Modern `.wal` skins, lyrics, scrobbling, multi-tenant hosting, iOS, DRM sources, video, realtime talkback, a custom skin language, a Spotify client of our own.

## Risks (details in the two plans)
- **Legal/ToS:** yt-dlp on YouTube, Spotify/Deezer APIs as a back door, re-streaming Greek stations. Mitigation: fetch flag off by default, personal use, passthrough for external stations, no credentials shipped. **Lef must read the three sites' terms before phase 3 enables fetch**; a summary is not that reading.
- **Fingerprinting:** Chromaprint/`fpcalc` server-side for dedupe (no API). Naming a song from audio = AcoustID (free, non-commercial, key + User-Agent rules). No official Android Chromaprint build found; phone-side later.
- **Android:** foreground services, audio focus, OEM battery killers; needs a real-handset test. Ducking our own music under our mic is a gain on our mixer bus.
- **Skin parity**, **yt-dlp breakage** (pin + self-update + health check), **CPU** (one encoder per station, 80% cap).
- **Licences:** cliamp MIT (Bjarne Øverli), Webamp MIT, Chromaprint LGPL-2.1 with bundled FFmpeg.
