# Omarchy plugins: what helps Mus-Eq

Research date: 2026-11-07. Source: the Omarchy plugin marketplace (https://plugins.omarchy.org/, registry in https://github.com/omacom/omarchy-plugin-marketplace), which lists about 6,300 plugin repositories.

## How the list was narrowed

The registry gives each plugin's repository, category and tags. We took the repositories tagged `media` or matching music words (955), read each manifest, ran an automated classifier with seven yes/no questions (music player, media-server client, radio, now-playing, identification or metadata, audio equalizer or visualizer, downloader), and kept the 276 where any music-relevant answer was clearly yes. Those were then read by hand in clusters: server clients, now-playing widgets, identification, downloaders, radio, and audio/equalizer plugins. Licences were read from each repository's LICENSE file, not only from GitHub's detector (eleven listings the detector could not read are MIT).

Result: **no existing plugin talks to Mus-Eq**, which is expected, so we need a first-party Mus-Eq plugin. Several existing ones are good templates, and several will work unchanged once Mus-Eq speaks the OpenSubsonic API (planned, see `docs/SPEC.md`).

## The plugin format (verified against the marketplace docs)

- A plugin is a public Git repository with `manifest.json` at its root (`schemaVersion: 1`, `id`, `name`, `version`, `author`, `description`, `kinds`, `entryPoints`; optional `license`, `keepLoaded`, and a `barWidget` block with display name, category, default section and a settings schema). IDs are globally unique and namespaced, like `io.github.<owner>.<name>`; `omarchy.*` is reserved.
- Kinds: `bar-widget`, `panel`, `overlay`, `menu`, `service` (headless singleton) and `bar`. Each kind points at one QML file.
- Install: `omarchy plugin add <git-url> --enable`, which places it in `~/.config/omarchy/plugins/<id>/`. Plugins run inside the shell's single Quickshell process with the user's permissions: **they are not sandboxed**.
- Publishing: a README with install and removal steps, a LICENSE at the root, optional `preview.png`, then a `[Plugin]: <name>` issue on the marketplace repository. A bot checks the structure and runs a static security scan; it is not a security review.
- Reading data from QML: MPRIS (for players), `XMLHttpRequest` (HTTP JSON), `Process` running `curl` or a helper script, `Socket` for **unix** sockets only, `FileView`, timers. **QML here has no WebSocket**, so live updates need polling or a small helper process.

## What a Mus-Eq bar widget needs

1. Kinds `service` + `bar-widget`; settings for the server URL and the path of a token file (mode 0600). The token is never put in the shell config or on a command line.
2. A request helper that runs `curl` with the token passed on stdin, one request in flight at a time. This is what `contrib/now-playing.sh` already does.
3. Poll `GET /v1/session` every few seconds, staying quiet about brief failures (show a fault only after about 45 s of continuous failure). Commands go to `POST /v1/session`. A 409 means "no player is connected" and is shown as such.
4. A bar item (play/pause glyph and `artist - title`; click toggles, middle skips) and a panel with transport, volume and a station list.
5. **Gaps in our API to close:** cover art for the current track, and position in the session. (Starting a station remotely is done: `play` with a `station_id` or `track_id`.)
6. **MPRIS:** the web client cannot publish MPRIS. A small helper (or the later desktop client) that publishes an MPRIS player mirroring `/v1/session` would make Omarchy's own media widget, media keys and every MPRIS-based plugin work with Mus-Eq unchanged.

## Best candidates (all MIT unless noted; copy with notice, keep copyright lines)

| Plugin | Use for Mus-Eq |
|---|---|
| Music Assistant (rwilson131/omarchy.music-assistant) | Template for the request layer: HTTP JSON + bearer token, curl with the token on stdin, one in-flight request. Closest shape to our widget. |
| Media Controls + Album Art (crmne/omarchy-mpris) | Best-built small bar widget: adaptive width, text limiting, a capped artwork downloader, QML tests. |
| Navidrome (backmeupplz/omarchy-navidrome) | A complete minimal OpenSubsonic client and mpv controller; first thing to point at Mus-Eq once the OpenSubsonic layer exists. |
| Navidrome Remote / Play (Kyrunner, nbare) | "Fault only after 45 s" readiness logic for polling; verb-per-script backends. |
| Better Media (bibek.media) | Shows how to add a non-MPRIS source to the stock media widget. |
| Verse (n501.karaoke) | A complete lyrics stack (LRCLIB, local `.lrc`); lyrics are a v1 non-goal but this is the reference for later. |
| Spotify (stappmus/Omarchy-Spotify) | Shape of "small Rust daemon + Quickshell UI over a unix socket", MPRIS published from Rust. |
| OmaAmp (iainlennox/OmaAmp) | Live 8-band equalizer through mpv's audio-filter property, for a future desktop client. |
| omajam (matjam/omajam) | Push-based multi-connection bridge: the design for a helper that consumes our `/v1/events` WebSocket. |
| WiiM Companion, Singapore Radio, SomaFM Bar, Hertz Radio, DR Lyd, rpbar | Small-card and station-list ideas for the station panel (favourites, "a failed station stays selected", grouped lists, history, a status file the bar reads). |
| Equalizer (ParvvOK/equalizer), Parametric EQ | Desktop output equalizers on a PipeWire filter chain. They act on the sound card, not on files. |
| Shazam-style widget, SongRec plugin | Policy for identifying a song from an external stream (ask the player first, then fingerprint); relevant to later identification work. |

Not reusable as code: Omafin, Omamich and Celestune Bar (GPL-3: ideas only), and BearWave (GPL-3). Many radio and music plugins depend on YouTube, Spotify, Pandora or Sirius access; those carry terms-of-service risk and are ideas at most.

## Cautions

- Plugins are unsandboxed code. Some install packages or pip dependencies on first run (one even uses `pkexec pacman`). We will not install any of them on the owner's machine without asking.
- Several widgets assume MPRIS. They only work with Mus-Eq after something publishes an MPRIS player for it.
- Station and metadata services have their own terms; see `docs/SPEC.md` risks.

## Follow-up work

- A separate repository for the first-party Mus-Eq Omarchy plugin (own roadmap item, after the API gaps above).
- An MPRIS helper (phase 4, with the desktop client).
