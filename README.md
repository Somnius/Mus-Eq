<p align="center">
  <img src="assets/logo.svg" alt="Mus-Eq logo" width="128" height="128">
</p>

<h1 align="center">Mus-Eq</h1>

<p align="center">
<!-- stack:start -->
<img src="https://img.shields.io/badge/Rust-edition_2024-dea584?style=flat-square&amp;logo=rust&amp;logoColor=white" alt="Rust edition 2024" height="20"> <img src="https://img.shields.io/badge/axum-0.8-2f2f2f?style=flat-square" alt="axum 0.8" height="20"> <img src="https://img.shields.io/badge/tokio-1.54-2f2f2f?style=flat-square" alt="tokio 1.54" height="20"> <img src="https://img.shields.io/badge/SQLite-rusqlite_0.41-003b57?style=flat-square&amp;logo=sqlite&amp;logoColor=white" alt="SQLite rusqlite 0.41" height="20"> <img src="https://img.shields.io/badge/Symphonia-0.6-2f2f2f?style=flat-square" alt="Symphonia 0.6" height="20"> <img src="https://img.shields.io/badge/TypeScript-7.0-3178c6?style=flat-square&amp;logo=typescript&amp;logoColor=white" alt="TypeScript 7.0" height="20"> <img src="https://img.shields.io/badge/Vite-8.3-646cff?style=flat-square&amp;logo=vite&amp;logoColor=white" alt="Vite 8.3" height="20"> <img src="https://img.shields.io/badge/Webamp-2.3-4caf50?style=flat-square" alt="Webamp 2.3" height="20"> <img src="https://img.shields.io/badge/OpenAPI-3.1.0-6ba539?style=flat-square&amp;logo=openapiinitiative&amp;logoColor=white" alt="OpenAPI 3.1.0" height="20"> <img src="https://img.shields.io/badge/Docker-Containerfile-2496ed?style=flat-square&amp;logo=docker&amp;logoColor=white" alt="Docker Containerfile" height="20">
<!-- stack:end -->
</p>

A self-hosted music server and player. One headless server (Docker or Podman) holds your library, serves it over one API, and thin clients talk to it. The web player uses Webamp, so the first skin is the classic Winamp look. Native clients are planned, not built yet.

**Status: early, still changing.** The server and the web player work. We have tried them on localhost and in a headless browser, not yet across a LAN, with Podman or on a large library. Phases 0 to 3d of the [roadmap](docs/ROADMAP.md) are built. Native Android and desktop clients are not built. Fetching single tracks exists but is off by default and has only been tested against fakes, never against the real services. No licence has been chosen yet.

## What works today

- Library: scan a folder, read tags, search tracks (accent and case insensitive, Greek included), stream files with Range support
- Web player: Webamp, login, library search, playlists, skin upload and picker, and a session that other tools can read and control
- Playlists that extend themselves: when a playlist ends, new tracks by artists not yet in it, with the same genre, are added from your own library
- Duplicates: detection by audio fingerprint (and by exact hash), with a proposed keeper. Nothing is ever deleted; a merge only changes the database
- Metadata suggestions: MusicBrainz search for tracks with missing tags, and AcoustID lookup if you bring your own key. A suggestion changes the database only when you accept it; audio files are never rewritten
- Radio stations: a station list (a Greek list is included in `data/stations/`) with search and a liveness check, played in the browser
- A control API, described in `docs/openapi.yaml`, so widgets and other apps can drive the player

## What is planned

- Native clients for Android, Windows and Linux, with offline storage on the device
- Fetching playlists and albums (single tracks exist, see below)
- A radio made from your own files, transcoding, and a live channel from a phone's mic
- Discogs tag suggestions, a web page for duplicates and suggestions (API only for now), and more skins

## Try what exists

The server needs an owner token (at least 16 characters) and a folder of music.

```bash
cp .env.example .env            # then edit the token (.env is git-ignored)
mkdir -p music                  # put some audio files in here
docker compose up --build
```

Open http://127.0.0.1:8080/ for the web player and log in with the token. Or use the API:

```bash
export T=<your token>
curl -XPOST -H "Authorization: Bearer $T" -H 'content-type: application/json' -d '{"kind":"scan"}' http://127.0.0.1:8080/v1/jobs
curl -H "Authorization: Bearer $T" 'http://127.0.0.1:8080/v1/tracks?q=something'
```

`GET /v1/health` is open. Everything else needs a login or `Authorization: Bearer <token>`. The port is published on localhost only; change that in `compose.yaml` on purpose if you want LAN or Tailscale access. Compose reads `MUSEQ_TOKEN` and `MUSEQ_MUSIC_DIR` (default `./music`) from `.env`. Podman should work with the same `Containerfile`, but it is untested.

From source (recent Rust toolchain, and Node for the web client): `MUSEQ_TOKEN=... MUSEQ_LIBRARY=./music cargo run -p museq-server`. Settings:

- `MUSEQ_TOKEN` or `MUSEQ_TOKEN_FILE`: the owner token (one is required)
- `MUSEQ_BIND`: listen address, default `127.0.0.1:8080`
- `MUSEQ_LIBRARY`: the music folder, `MUSEQ_DB`: the SQLite file, `MUSEQ_SKINS_DIR`: where uploaded skins go
- `MUSEQ_WEB_DIR`: a built web client to serve at `/` (the container image includes one)
- `MUSEQ_STATIONS_SEED`: comma-separated station list files to import at startup
- `MUSEQ_ALLOW_PRIVATE_STATIONS=1`: allow radio stations on your own network (refused by default)
- `MUSEQ_ACOUSTID_KEY`: your own AcoustID application key, to identify tracks by audio (off without it)
- `MUSEQ_CONTACT`: contact shown to MusicBrainz in the User-Agent

## Fetching tracks (off by default)

The server can fetch one track at a time from a YouTube Music link or a Spotify track link, through your own copy of `yt-dlp`, and file it in your library with a note of where it came from. It is off by default and stays off until you switch it on yourself.

**Read the terms of service of YouTube, YouTube Music and Spotify first, and only fetch what you are allowed to store.** Whether that is the case depends on where you live and what you fetch; this project does not decide it for you, and nothing here has been checked against those terms. Enabling it means you did that reading:

```bash
MUSEQ_FETCH=on
MUSEQ_FETCH_ACK=i-have-read-the-terms-and-fetch-only-what-i-may-store
MUSEQ_FETCH_YTDLP=/absolute/path/to/yt-dlp     # your own binary; the image does not include one
```

Then `GET /v1/fetch/status` tells you whether it is on (and if not, why), and `POST /v1/jobs` with `{"kind":"fetch","input":{"url":"https://music.youtube.com/watch?v=..."}}` starts a job. Playlists and albums are refused. Optional: `MUSEQ_FETCH_INBOX` (default `<library>/_fetched`), `MUSEQ_FETCH_MAX_BYTES`, `MUSEQ_FETCH_TIMEOUT_S`, and `MUSEQ_FETCH_COOKIES` (a Netscape cookies file with permissions 600, used only when the service asks for a sign-in; the server works on a copy and never writes to your file).

This has been tested only against a fake `yt-dlp` and a fake Spotify page. No real request to either service has been made from this code yet, so the first real run may find something the fakes did not.

## Layout

```
crates/museq-server/   the server (Rust, axum)
web/                   the web client (TypeScript, Vite, Webamp)
docs/                  spec, roadmap, decisions, openapi.yaml
data/stations/         radio station lists (credit inside)
scripts/               checks and helper scripts
contrib/               small extras, such as a now-playing script for status bars
Containerfile, compose.yaml
```

## Docs

- [docs/SPEC.md](docs/SPEC.md): what the system does and how
- [docs/ROADMAP.md](docs/ROADMAP.md): phases, status and what is checked
- [docs/COMPARISON.md](docs/COMPARISON.md): similar projects and what we may borrow
- [docs/INSPIRATION.md](docs/INSPIRATION.md): ideas and sources
- [docs/PLUGINS.md](docs/PLUGINS.md): widgets and plugins
- [docs/openapi.yaml](docs/openapi.yaml): the API contract
- [docs/DECISIONS.md](docs/DECISIONS.md): choices made and why

## Contributing

Issues are open for bugs, feature ideas and station suggestions. See [CONTRIBUTING.md](CONTRIBUTING.md). Discussions are switched off on purpose, please use issues.

## Credits

The Greek station list comes from [Tray-FM](https://github.com/Constantinos/Tray-FM) by Constantinos (Unlicense).

## License

Not chosen yet.
