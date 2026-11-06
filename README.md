# Mus-Eq

A self-hosted music server and player. One headless server (Docker or Podman) that holds your library, fetches tracks, builds playlists, fixes tags, finds duplicates and streams radio. Thin clients talk to it over one API: a web player, and native players for Windows, Linux and Android. The first skin is the classic Winamp look.

**Status: early scaffold.** Right now the server only answers `GET /v1/health`. The API is drafted in [`docs/openapi.yaml`](docs/openapi.yaml), the plan is in [`docs/ROADMAP.md`](docs/ROADMAP.md) and the design is in [`docs/SPEC.md`](docs/SPEC.md). Don't expect it to play music yet.

## What it's meant to do

- Web player with classic skins (Webamp), native clients for Windows, Linux and Android
- Online storage on the server, offline storage on the device
- Radio: online stations (a Greek station list is included in `data/stations/`), a fake radio made from your own files, and a live channel from the phone's mic with auto-ducking and a small mixer
- Transcoding for the radio (Opus in Ogg and others)
- Fetch singles and playlists (Spotify link, then Deezer for the song id, then YouTube Music). Off by default, see the notes in the spec
- Tag fixes from MusicBrainz and Discogs, duplicate detection by audio content, playlists that extend themselves with new artists in the same style
- A control API so widgets and other apps can drive it
- Runs on a server, or on a PC or phone as a local server; reachable over LAN, Tailscale or VPN, or a public endpoint if you choose

## Try what exists

```bash
docker build -f Containerfile -t museq-server .
docker run --rm -p 127.0.0.1:8080:8080 museq-server
curl http://127.0.0.1:8080/v1/health
```

Or `docker compose up --build`. Podman should work with the same `Containerfile` but I haven't tested it.

From source you need a recent Rust toolchain: `cargo run -p museq-server` (listens on `127.0.0.1:8080`, change it with `MUSEQ_BIND`).

## Layout

```
crates/museq-server/   the server (Rust, axum)
docs/                  spec, roadmap, open questions, openapi.yaml
data/stations/         Greek radio station list (credit inside)
Containerfile, compose.yaml
```

## Contributing

Issues are open for bugs, feature ideas and station suggestions. See [CONTRIBUTING.md](CONTRIBUTING.md). Discussions are switched off on purpose, please use issues.

## Credits

The Greek station list comes from [Tray-FM](https://github.com/Constantinos/Tray-FM) by Constantinos (Unlicense).

## License

Not chosen yet.
