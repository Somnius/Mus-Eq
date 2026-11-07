# Existing projects: what to learn, and what we may copy

Status: **the in-depth chart is now included** (section "In-depth comparison" at the end). This page records the verified facts and the licence rules we follow. Facts in the first tables come from GitHub itself (licence file, language, activity) on 2026-11-07; claims from surveys were treated as leads and checked. The in-depth section comes from reading each project's README and, for the integrated servers, a shallow look at the source tree; anything not checked is marked unverified.

## A remark worth keeping

> No mature project was found that perfectly checks every box: acquisition + automatic tagging + metadata correction + duplicate detection + library organisation + radio + streaming + playlists + web client + native Windows, Linux and Android clients, all maintained as one coherent project and API. The ecosystem is closer to **"excellent building blocks that can be combined"** than to a single definitive open-source Spotify replacement. (From a survey of existing projects, November 2026.)

That gap is the reason Mus-Eq exists. The usual answer today is a stack: a server such as Navidrome, a fetcher such as Lidarr, a tagger such as beets or Picard, and separate clients.

## Licence rule of thumb for this project

- **No licence file = all rights reserved.** We may read it and learn from it, never copy code.
- **MIT, Apache-2.0, Unlicense:** may be depended on or copied, keeping the notice.
- **GPL / AGPL:** ideas only, unless Mus-Eq itself adopts a compatible licence (Q14 is decided: MIT OR Apache-2.0, so GPL/AGPL code cannot be copied). Copying GPL/AGPL code into an MIT or unlicensed project is not allowed.
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

- [x] In-depth comparison chart (features, API compatibility, clients, fetch, tagging, dedupe, radio, skins, offline, maturity, licence), built from the per-project analysis.
- [x] Decide Mus-Eq's own licence (Q14): MIT OR Apache-2.0, decided 2026-11-07 (see DECISIONS.md).
- [ ] Pick the few ideas worth borrowing per phase (library, stations, metadata, fingerprinting, clients).

## In-depth comparison

Research date: 2026-11-07. Cells are short on purpose; anything marked "unverified" was not confirmed against a primary source. "Reuse policy" means: **copy** = code may be copied or depended on with its notice kept; **re-implement** = study behaviour only and write our own Rust (see `docs/INSPIRATION.md`). A separate program we only run (yt-dlp, `fpcalc`, ffmpeg) is a different case and is noted where it applies.

| Project | Licence | Language | Server and clients | API compatibility | Fetch | Tag fixing | Duplicates | Radio | Skins/themes | Offline | Maturity and risk | Reuse policy for Mus-Eq |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| octo | GPL-3.0 | Rust (apps in Kotlin) | proxy in front of Navidrome; own Android/Windows/Linux apps (separate GPL repo); any Subsonic client | Subsonic (proxied) plus own fields | yes: Soulseek, Lidarr, YouTube, Spotify import | yes: MusicBrainz/AcoustID check on download | partial: download verification, fake-lossless check; no library dedupe | yes: Last.fm, YouTube Music, ListenBrainz, sounds-alike blend | no | partial | young, weekly releases, one main author; high ToS exposure | re-implement |
| RE-KORD | none (Cargo says MIT, no licence file) | Rust, Svelte, Tauri | hub plus Linux/Windows desktop, Android, PWA | own REST `/api/v1`; OpenSubsonic planned | yes: yt-dlp (YouTube Music, SoundCloud, Bandcamp) | yes: Discogs, MusicBrainz, iTunes, Deezer, TheAudioDB | no | partial: smart radio from library, RSS/live module | yes: 17 themes plus custom | no | young, active, one main author; ToS exposure | re-implement (all rights reserved) |
| Melodee | MIT | C# (.NET 10) | server, Blazor web, Electron desktop, Android/Auto app | OpenSubsonic, Jellyfin-compatible, own REST | no | yes: staging review, many metadata sources | no | yes: Subsonic stations | yes: theme packs | no (client side unverified) | 2 years, quiet since Jul 2026, one main author; needs PostgreSQL | copy with notice (C#, so mostly ideas) |
| Lark | AGPL-3.0 (plus adapted GPL/AGPL/MIT/Apache code) | Go, React | single binary, web, desktop, DLNA, Subsonic clients | own plus optional Subsonic, MCP endpoint | no | yes: path-assisted fixes, writeback, lyrics | partial: content hash only | yes: Radio Browser, custom lists | yes: 21 colour schemes plus player styles | yes: browser cache | 6 months, very active, one author; unofficial lyrics endpoints, unlicensed artwork | re-implement |
| RiffPlayer | AGPL-3.0 | TypeScript (Fastify), React, Flutter | server, PWA, Android | Subsonic/OpenSubsonic plus own | no | no (read-only, credits) | no | no | partial: web themes, mobile Winamp/iPod styles | yes: Android downloads | 4 months, pre-1.0, one author | re-implement |
| Ariami | MIT (paid desktop/TV apps are closed) | Dart/Flutter, Rust transcoder | server (GUI/CLI/Docker), iOS/Android, paid desktop/TV | own HTTP plus WebSocket, no Subsonic | no | no | no | no | no | yes: mobile downloads | 1 year, store-published, one author | copy with notice (mostly ideas; transcoder is Rust, evaluate) |
| Synthseek | MIT (front end only; backend not open source) | TypeScript (Next.js) plus closed Node backend | container, web, Subsonic clients | Subsonic/OpenSubsonic, tRPC, MCP | yes: 7 sources (Soulseek, Usenet, torrent, yt-dlp and others) | yes: beets plus AcoustID check | yes: title plus length proof, recycle bin | partial: autoplay, Last.fm/ListenBrainz discover | no | no | 9 months, active, backend cannot be forked; highest ToS exposure | re-implement (backend has no source; UI MIT but not useful) |
| Navidrome | GPL-3.0 | Go, React | single binary, web, any Subsonic client | Subsonic, OpenSubsonic, native REST (Jellyfin API in progress, extent unverified) | no | no (read-only) | no | yes: Subsonic stations | partial: ~30 colour themes | no | 10 years, very mature, large user base | re-implement |
| gonic | GPL-3.0 | Go | single binary, Subsonic clients, admin pages | Subsonic (some OpenSubsonic) | no | no | no | yes: Subsonic stations (unverified in code) | no | no | 7 years, steady, one main author | re-implement |
| Jellyfin (music only) | GPL-2.0 (not compatible with GPL-3) | C# | server, web, official apps, third-party music apps | own REST (OpenAPI) | no | no (sidecar files, MusicBrainz ids) | no | no | partial: web themes | client side | 8 years, many maintainers | re-implement (API shape only) |
| koel | MIT (paid "Plus" features gated by key) | PHP, Vue | server, web, Flutter mobile, Subsonic clients | own JSON plus Subsonic | no | partial: in-app edit, info lookups | no | yes: stations with stream proxy and now-playing text | partial: background themes, custom themes in Plus | PWA only | 10 years, active, one main author | copy with notice (PHP, so ideas) |
| Ampache | AGPL-3.0 | PHP, MySQL | server, web, many API clients, Subsonic/DAAP/UPnP | own API v3-6, Subsonic, Jellyfin (opt-in) | partial: "Wanted" module via a plugin | yes: tag write, MusicBrainz/Discogs plugins | partial: metadata duplicate finder (unverified) | yes: live streams, broadcast, democratic play | yes: themes | no | 25 years, active, one main author | re-implement |
| airsonic-advanced | GPL-3.0 | Java | server, JSP web, Subsonic clients, Sonos/UPnP | Subsonic | no | yes: tag editor | no | yes: stations | yes: classic themes | no | dormant since Apr 2024 | re-implement (historic reference only) |
| stash (scanner reference only) | AGPL-3.0 | Go, React | single binary, web | GraphQL | n/a | n/a | yes: cheap and strong hashes, rename detection | n/a | n/a | n/a | 8 years, active; video app, not music | re-implement (design only) |
| OpenSubsonic API | Apache-2.0 | spec (Hugo, OpenAPI) | n/a | the compatibility target: Subsonic 1.16.1 plus extensions | n/a | n/a | n/a | station endpoints | n/a | n/a | 3 years, working group of server and client authors | copy with notice (spec and OpenAPI files may be vendored) |
| Harmony | none (README badge says MIT, no licence file) | Python | web UI only, no player; files plus M3U for Navidrome | own (FastAPI) | yes: yt-dlp, SpotDL as rescue | partial: MusicBrainz assist, nothing written until save | partial: graded by ids and names, no fingerprint | no | light/dark | PWA shell only | 3 stars, created Jul 2026; legal risk | re-implement |
| MusicSeeker | none | JavaScript (FastAPI backend) | web, macOS, Android; runs beside Navidrome | own REST, uses Navidrome | yes: yt-dlp, Soulseek, Lidarr | not in README | skip if already present | partial: artist radio, Last.fm | dark theme only | not in README | early, 28 stars; ToS exposure is the product | re-implement |
| beets | MIT | Python | CLI plus plugins (web and MPD optional) | none | no | yes: MusicBrainz, Discogs, fingerprints | yes (plugin) | no | no | no | mature, since 2010 | copy with notice (we still prefer re-implementing the matching) |
| Picard | GPL-2.0-or-later | Python | desktop app only | none | no | yes: MusicBrainz, AcoustID, cover art | no | no | no | no | mature, since 2011 | re-implement |
| Lidarr | GPL-3.0 | C# | server plus web UI | own | yes: Usenet, BitTorrent, RSS | rename only | no | no | no | no | mature; its metadata server was unstable at research time | re-implement |
| slskd | AGPL-3.0 | C# | daemon plus web UI | own | yes: Soulseek | no | no | no | no | no | active; P2P copyright exposure | re-implement; if ever wanted, run it beside Mus-Eq and call its HTTP API |
| spotDL | MIT | Python | CLI | none | yes: Spotify metadata, YouTube audio | embeds tags | no | no | no | no | mature CLI; high ToS exposure | copy with notice; better to run as an optional separate program |
| yt-dlp | Unlicense | Python | CLI and library | none | yes (generic downloader) | post-process metadata | no | no | no | no | very mature; legal risk is the operator's | free to use; run as a pinned separate program |
| Chromaprint | mixed: MIT code plus bundled LGPL-2.1 parts, treated as LGPL-2.1; GPL if built with FFTW3 | C++ | library plus `fpcalc` | none | no | no | the fingerprint itself | stream monitoring only | no | no | mature library | run `fpcalc` or link the system library; do not copy the bundled sources |
| listenbrainz-server | GPL-2.0 ("or later" unverified) | Python, React | web app plus API (hosted by MetaBrainz) | own listen API | no | no | no | no | no | no | mature service | re-implement (we only call the public API) |
| Feishin | GPL-3.0 | TypeScript (Electron) | desktop plus web client | Navidrome, Jellyfin, OpenSubsonic | no | no | no | no | JSON theme files | not in README | active, popular | re-implement; use as a reference client against our API |
| Tempo | GPL-3.0 | Java | Android client only | Subsonic | no | no | no | if the server has it | no | partial, in progress | quiet since Jan 2025, one maintainer | re-implement; use as a reference client |
| Psst | MIT | Rust | native desktop Spotify client | private Spotify protocol | no | no | no | no | no | not in README | large but fragile; needs a Spotify account | copy with notice, but nothing from its Spotify session code |
| Mixxx | GPL-2.0-or-later (skins and SDK parts differ) | C++ | desktop DJ app | none | no | no | no | broadcast not in short README | yes: CC skins | no | mature | re-implement (behaviour only) |
| cliamp | MIT | Go | terminal player plus separate Android app | many sources incl. Navidrome | plays remote services | no | no | yes: streams, podcasts | terminal look only | not in README | young, very active; a dependency's licence unverified | copy with notice; future client only |
| Webamp | MIT | TypeScript | browser component | none (host page supplies media) | no | no | no | no | yes: Winamp skins | no | mature | copy / depend (already our client); skins are separate works |
| Mopidy | Apache-2.0 | Python | daemon plus MPD/web clients | MPD (core) | via extensions | not in README | no | yes: internet radio | no | no | mature; extension licences unverified | copy with notice (architecture lesson only) |
| Tauri | Apache-2.0 (README badge also claims MIT; unverified) | Rust | app shell, no server | none | no | no | no | no | no | no | mature framework | depend on it (phase 4 shell, Q13) |
| selfhosted-music-server | none | none (Compose guide) | Navidrome plus other tools | uses Navidrome | via Lidarr, Soulseek, a downloader | no | no | no | no | no | a guide from Jun 2026, not software | re-implement (nothing to copy) |
| WBs-Guide-to-Navidrome | none | none (guide) | Navidrome, Feishin and others | Subsonic via Navidrome | no | via Picard and other tools | no | external station manager | no | not in README | guide, last push Apr 2026 | re-implement (phase order idea only) |

## Where the claims from the chat surveys were wrong or partly wrong

Only facts the research found:

- **A badge is not a licence.** Harmony shows an MIT badge but has no licence file. RE-KORD's Cargo manifest says MIT but the repo has no licence file. Both are all rights reserved until the authors add one.
- **A repo can be only part of the product.** Synthseek's public repo is only the web front end (MIT); the server it needs is not published and its download link returns 404. Octo's own apps live in a separate GPL-3.0 repo, and whether its iOS app ships is unverified.
- **Paid, closed apps exist beside open ones.** Ariami's desktop player and TV apps are sold and not in the public repo; only the server core and mobile apps are MIT. Koel is MIT but gates some features behind a paid key. Symfonium (named in a guide) is closed source.
- **Not everything called a project is software.** The "selfhosted-music-server" repo is a Compose recipe, and the Navidrome setup guide is a document. Neither has a licence.
- **"Mixed" licences are easy to miss.** Chromaprint's GitHub field says "Other" because MIT code sits next to bundled LGPL-2.1 parts, and a build with FFTW3 becomes GPL. Mixxx is the same story. Tauri's README claims MIT or Apache but only the Apache text was seen (dual licence unverified). Jellyfin is GPL-2.0 only, so it cannot feed a GPL-3.0 project either.
- **Adapted code carries other licences.** Lark is AGPL-3.0 but its notice file lists code taken from GPL, AGPL, MIT and Apache projects, and some artwork with no stated licence.
- **Some surveys overstated maturity.** Melodee has been quiet since July 2026, Tempo since January 2025, airsonic-advanced since April 2024. Nearly every project in the integrated-server group has one main author.
- **Some features are narrower than described.** Harmony's duplicate detection uses ids, names and duration, not audio fingerprints. RE-KORD has no fingerprinting. Octo verifies downloads but does not de-duplicate a whole library. Ariami has no Subsonic API, so third-party clients cannot connect to it. Melodee needs PostgreSQL. Spotify-based tools such as Psst need a paid account and now a user-supplied developer client id.
- **Unverified items remain**: dual licence of Tauri, "or later" for the ListenBrainz server, the licence of a Spotify library under cliamp, extra bundled licences inside yt-dlp, licence names of Ampache and stash files, and the endpoint list of the OpenSubsonic README (the spec itself was read in the tree).

## Licence decision summary for Mus-Eq (Q14 decided: MIT OR Apache-2.0)

The decision was taken on 2026-11-07 and is recorded in `DECISIONS.md`; this table is the research it drew on. The research gives these trade-offs for a self-hosted project:

| Licence for Mus-Eq | Pros | Cons |
|---|---|---|
| MIT | Short; maximum adoption; client authors and companies can embed it; may copy MIT, Apache and Unlicense code; can later move to GPL/AGPL (the reverse is hard) | Cannot copy Navidrome, Picard, Lidarr, Feishin and similar; anyone can take it private |
| Apache-2.0 | Same as MIT plus an explicit patent grant and a notice mechanism | Same copy limits as MIT; slightly longer to comply with |
| GPL-3.0 | Could copy GPL-3.0 code (octo, Navidrome, gonic); derivatives stay open when distributed | Cannot take GPL-2.0-only code (Jellyfin) or AGPL code; a hosted fork can still stay closed; cannot go back to MIT |
| AGPL-3.0 | Everything GPL-3.0 allows plus AGPL code (Lark, RiffPlayer, Ampache, slskd, stash); closes the hosted-service gap, which suits a server | Strongest copyleft; some companies and app stores avoid it; fewer closed products will embed it; does not make downloading from YouTube or Soulseek lawful |

Findings that matter for the choice:

- The roadmap works under MIT or Apache-2.0 if fingerprinting runs `fpcalc` as a separate program, tagging uses the public MusicBrainz and AcoustID web APIs, clients are Webamp plus an OpenSubsonic surface, and fetch is an optional pinned yt-dlp process. Building Chromaprint with FFTW3 would make that binary GPL, so avoid it.
- Choose GPL or AGPL only if a specific source tree must be merged rather than rewritten. If only ideas are reused, MIT keeps every option open.
- If the server were AGPL, one option from the research is GPL-3.0 or MIT for the clients, because some stores and distributors refuse AGPL apps.
- Whatever is chosen, decide it before any code is ported, and log each borrowed idea in `docs/DECISIONS.md`.
- The licence of Mus-Eq does not fix the legal exposure of fetching: that comes from the sites and networks it is pointed at (see the fetch section of `docs/INSPIRATION.md`).
