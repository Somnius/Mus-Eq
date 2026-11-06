# Decisions log (date, choice, reason). Open items live in `OPEN_QUESTIONS.md`.

- 2026-11-06 **Server in Rust**; Android client in Kotlin. Fits the owner, cross-compiles to Linux/Windows/macOS and Android, and the reusable part of cliamp (a Go terminal player) is only subprocess glue.
- 2026-11-06 **cliamp is an optional existing client, not a base.** Its station and yt-dlp logic is a reference (MIT notice if copied). A cliamp provider for MusEq is a phase 4 nice-to-have, not an exit criterion.
- 2026-11-06 **Greek radio list: Tray-FM** by Constantinos (Unlicense), copied to `data/stations/trayfm-stations.txt`. Stream liveness is checked in phase 2.
- 2026-11-06 **No WorkOS / hosted auth.** Token auth.
- 2026-11-07 **Auth: one owner bearer token from phase 1** (applies the Q9 default); `/v1/health` open.
- 2026-11-07 **Network: bind localhost by default, LAN/Tailscale is the owner's network** (applies the Q11 default).
- 2026-11-07 **External stations are passed through, never proxied.** `/v1/radio` is for library and live stations.
- 2026-11-07 **One `job` table** (kinds scan|fetch|tag|fingerprint) instead of separate job tables.
- 2026-11-07 **Transcoding: ffmpeg in the image; in-process Opus is an option measured at the start of phase 5.**
- 2026-11-07 **Phase 1 is split**: 1a server core (curl-checkable, CI green), 1b Webamp client and session, then phase 2.
- 2026-11-07 **Desktop shell stays open (Q13)**, browser first; decided at phase 4.
- 2026-11-07 **Phase 1a data shortcuts:** `artist`/`album` are text columns on `track`, one track per file, ids are SQLite integers exposed as strings, codec = file extension. Proper tables and fingerprints arrive with dedupe (phase 3).
- 2026-11-07 **Playlist edits:** items are removed/reordered by replacing the whole list (`PUT /v1/playlists/{id}/items`).
- 2026-11-07 **Server refuses to start without a token** (min 16 characters); it never prints it.
