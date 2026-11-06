# Existing projects: what to learn, and what we may copy

Status: **in-depth comparison chart pending** (planned for after the stations phase; the per-project analysis is being collected). This page records the verified facts so far and the licence rules we follow. Facts below come from GitHub itself (licence file, language, activity) on 2026-11-07; claims from chat-assistant surveys were treated as leads and checked.

## A remark worth keeping

> No mature project was found that perfectly checks every box: acquisition + automatic tagging + metadata correction + duplicate detection + library organisation + radio + streaming + playlists + web client + native Windows, Linux and Android clients, all maintained as one coherent project and API. The ecosystem is closer to **"excellent building blocks that can be combined"** than to a single definitive open-source Spotify replacement. (From a survey of existing projects, November 2026.)

That gap is the reason Mus-Eq exists. The usual answer today is a stack: a server such as Navidrome, a fetcher such as Lidarr, a tagger such as beets or Picard, and separate clients.

## Licence rule of thumb for this project

- **No licence file = all rights reserved.** We may read it and learn from it, never copy code.
- **MIT, Apache-2.0, Unlicense:** may be depended on or copied, keeping the notice.
- **GPL / AGPL:** ideas only, unless Mus-Eq itself adopts a compatible licence (open question Q14). Copying GPL/AGPL code into an MIT or unlicensed project is not allowed.
- Running or calling a separate program (yt-dlp, ffmpeg, `fpcalc`) is different from copying its code, but check that program's own licence and terms.
- Station lists, skins and metadata services have their own terms (see the SPEC risks).

## How we use projects we can't copy: build our own in Rust

For a project with no licence, or with a licence we can't take code from (GPL, AGPL, or anything not compatible with ours), we do not copy it. We study **what it does and how it behaves** and build our own implementation in Rust, so Mus-Eq gets close to the same functionality with code that is ours.

To keep that clean:
- Work from behaviour: the project's documentation, its public API and file formats, and what it does when run, written down as our own short behaviour notes ("a scan walks the folder, skips unchanged files, detects moves by hash").
- Implement from those notes, not from the other project's source open beside us. Don't transcribe code, structure, names or comments.
- Public specifications are fair game to implement exactly (for example the OpenSubsonic API, which is Apache-2.0, or the MusicBrainz and AcoustID web APIs).
- Where a project is MIT, Apache-2.0 or Unlicense we may use or copy code with its notice, but we still prefer a small own implementation unless the code is a clear fit.
- Each borrowed idea gets a line in `docs/DECISIONS.md` saying where the idea came from, so credit stays honest.

## Verified facts (GitHub, 2026-11-07)

### Integrated servers and clients

| Project | Licence | Language | Stars | Last push | What that means for us |
|---|---|---|---|---|---|
| [winters27/octo](https://github.com/winters27/octo) | GPL-3.0 | Rust | 172 | 2026-10-29 | ideas only unless Mus-Eq adopts a compatible licence |
| [Creiv/RE-KORD](https://github.com/Creiv/RE-KORD) | **none** | Rust | 117 | 2026-11-01 | ideas only (all rights reserved) |
| [melodee-project/melodee](https://github.com/melodee-project/melodee) | MIT | C# | 64 | 2026-07-14 | copy with notice |
| [ca-x/lark](https://github.com/ca-x/lark) | AGPL-3.0 | Go | 16 | 2026-11-06 | ideas only unless Mus-Eq adopts a compatible licence |
| [atabayrak460/riffplayer](https://github.com/atabayrak460/riffplayer) | AGPL-3.0 | TypeScript | 6 | 2026-10-14 | ideas only unless Mus-Eq adopts a compatible licence |
| [picccassso/Ariami](https://github.com/picccassso/Ariami) | MIT | Dart | 23 | 2026-11-06 | copy with notice |
| [arukaraz/synthseek](https://github.com/arukaraz/synthseek) | MIT | TypeScript | 52 | 2026-11-06 | copy with notice |
| [navidrome/navidrome](https://github.com/navidrome/navidrome) | GPL-3.0 | Go | 24553 | 2026-11-04 | ideas only unless Mus-Eq adopts a compatible licence |
| [sentriz/gonic](https://github.com/sentriz/gonic) | GPL-3.0 | Go | 2650 | 2026-11-06 | ideas only unless Mus-Eq adopts a compatible licence |
| [jellyfin/jellyfin](https://github.com/jellyfin/jellyfin) | GPL-2.0 | C# | 59069 | 2026-11-06 | ideas only unless Mus-Eq adopts a compatible licence |
| [koel/koel](https://github.com/koel/koel) | MIT | PHP | 17333 | 2026-11-06 | copy with notice |
| [ampache/ampache](https://github.com/ampache/ampache) | AGPL-3.0 | PHP | 3845 | 2026-11-06 | ideas only unless Mus-Eq adopts a compatible licence |
| [airsonic-advanced/airsonic-advanced](https://github.com/airsonic-advanced/airsonic-advanced) | GPL-3.0 | JavaScript | 1404 | 2024-04-24 | ideas only unless Mus-Eq adopts a compatible licence |

### Acquisition, tagging and fingerprinting

| Project | Licence | Language | Stars | Last push | What that means for us |
|---|---|---|---|---|---|
| [azimul-kabir/harmony](https://github.com/azimul-kabir/harmony) | **none** | Python | 3 | 2026-10-12 | ideas only (all rights reserved) |
| [lucashanak/music-seeker](https://github.com/lucashanak/music-seeker) | **none** | JavaScript | 28 | 2026-10-16 | ideas only (all rights reserved) |
| [beetbox/beets](https://github.com/beetbox/beets) | MIT | Python | 15900 | 2026-11-06 | copy with notice |
| [metabrainz/picard](https://github.com/metabrainz/picard) | GPL-2.0 | Python | 5350 | 2026-11-06 | ideas only unless Mus-Eq adopts a compatible licence |
| [Lidarr/Lidarr](https://github.com/Lidarr/Lidarr) | GPL-3.0 | C# | 5811 | 2026-11-04 | ideas only unless Mus-Eq adopts a compatible licence |
| [slskd/slskd](https://github.com/slskd/slskd) | AGPL-3.0 | C# | 4255 | 2026-11-05 | ideas only unless Mus-Eq adopts a compatible licence |
| [spotDL/spotify-downloader](https://github.com/spotDL/spotify-downloader) | MIT | Python | 26692 | 2026-11-03 | copy with notice |
| [yt-dlp/yt-dlp](https://github.com/yt-dlp/yt-dlp) | Unlicense | Python | 205775 | 2026-11-04 | free to use |
| [acoustid/chromaprint](https://github.com/acoustid/chromaprint) | Other | C++ | 1416 | 2026-10-15 | ideas only unless Mus-Eq adopts a compatible licence |
| [metabrainz/listenbrainz-server](https://github.com/metabrainz/listenbrainz-server) | GPL-2.0 | Python | 1053 | 2026-11-06 | ideas only unless Mus-Eq adopts a compatible licence |

### Clients, players and building blocks

| Project | Licence | Language | Stars | Last push | What that means for us |
|---|---|---|---|---|---|
| [jeffvli/feishin](https://github.com/jeffvli/feishin) | GPL-3.0 | TypeScript | 10344 | 2026-11-03 | ideas only unless Mus-Eq adopts a compatible licence |
| [CappielloAntonio/tempo](https://github.com/CappielloAntonio/tempo) | GPL-3.0 | Java | 2268 | 2025-01-31 | ideas only unless Mus-Eq adopts a compatible licence |
| [jpochyla/psst](https://github.com/jpochyla/psst) | MIT | Rust | 9510 | 2026-08-18 | copy with notice |
| [mixxxdj/mixxx](https://github.com/mixxxdj/mixxx) | Other | C++ | 7364 | 2026-11-06 | ideas only unless Mus-Eq adopts a compatible licence |
| [bjarneo/cliamp](https://github.com/bjarneo/cliamp) | MIT | Go | 4580 | 2026-11-05 | copy with notice |
| [captbaritone/webamp](https://github.com/captbaritone/webamp) | MIT | TypeScript | 11336 | 2026-11-05 | copy with notice |
| [mopidy/mopidy](https://github.com/mopidy/mopidy) | Apache-2.0 | Python | 8608 | 2026-11-02 | copy with notice |
| [tauri-apps/tauri](https://github.com/tauri-apps/tauri) | Apache-2.0 | Rust | 112562 | 2026-11-06 | copy with notice |
| [opensubsonic/open-subsonic-api](https://github.com/opensubsonic/open-subsonic-api) | Apache-2.0 | SCSS | 256 | 2026-11-02 | copy with notice |

## Next

- [ ] In-depth comparison chart (features, API compatibility, clients, fetch, tagging, dedupe, radio, skins, offline, maturity, licence), built from the per-project analysis.
- [ ] Decide Mus-Eq's own licence (Q14) with the table above in mind.
- [ ] Pick the few ideas worth borrowing per phase (library, stations, metadata, fingerprinting, clients).
