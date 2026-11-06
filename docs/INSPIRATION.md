# What we rebuild ourselves, from behaviour

This page holds short behaviour briefs for features we saw in other projects whose code we cannot or do not want to copy (no licence, GPL, AGPL, or a project that is not open source). A few briefs come from MIT projects where the feature is worth rebuilding in our own way anyway. The licence facts are in `docs/COMPARISON.md`; the rule is in `docs/DECISIONS.md` (projects we cannot copy are re-implemented, not copied).

How to use a brief: it says what the user sees, the steps, what is stored, what can go wrong, how Mus-Eq builds it on its own stack (Rust, axum, SQLite, symphonia, the existing job table, the Webamp web client), and the roadmap phase. It is written so that nobody needs the other project's source. Phase numbers are those in `docs/ROADMAP.md`: 2 stations, 3 metadata/duplicates/fetch/extension, 4 clients, 5 radio and transcoding, 6 live radio. Anything marked **unverified** was not confirmed and must be checked before it is relied on.

Descriptions are in our own words from READMEs, documentation and observed behaviour. Do not add code, internal names or long quotes from those projects here.

---

## 1. Library scanner and rename detection

### 1.1 Stable track identity that survives moves
- **Sees:** after reorganising folders or fixing a tag, playlists, play counts and favourites still point at the same track.
- **Steps:** at scan time build an identity for each track. Use the MusicBrainz track and release ids from the tags when present; otherwise fall back to a tuple (album artist, album, disc, track number, title). The tuple fields are configurable. If a file's identity matches an existing track, update that track's path instead of creating a new one.
- **Keeps:** the identity string next to the file path and the sha256 already stored; a flag for how the identity was derived (id or tuple).
- **Failures and safety:** two different recordings with the same tuple would merge, so show a "possible clash" list instead of merging silently. Retagging may change a tuple identity; keep the old identity as an alias until the next full scan.
- **Mus-Eq:** extend the phase 1a scanner (sha256 already keeps a moved file's row). Add an `identity` column and an alias table in SQLite; compute from symphonia tags. No new job kind needed.
- **Phase:** 3 (needed before duplicates and tag fixes can refer to tracks).
- **Seen in:** Navidrome, Synthseek, stash (rename idea), Harmony (stable song ids).

### 1.2 Cheap fingerprint first, expensive one later
- **Sees:** a rescan after moving a whole folder is fast and does not look like "1000 tracks lost, 1000 added".
- **Steps:** at scan time compute only a cheap key: file size plus a checksum of the first and last 64 KiB. For a new file, look for an existing entry with the same key whose old path no longer exists. Exactly one match: it is a rename, update the path. None or several: it is a new file. For a whole folder, count how many files moved from the same old folder and treat the folder as renamed when most of them did. Strong hashes and audio fingerprints are separate background jobs.
- **Keeps:** the cheap key per file, the old path until the next clean-up, a "moved from" note for the log.
- **Failures and safety:** identical copies (same bytes) give several matches, so they are never treated as a rename; they go to the duplicate review (section 3). Never delete during a scan.
- **Mus-Eq:** a column on the file row; the rule runs inside the existing scan job. Strong hashing and fingerprinting run as `fingerprint` jobs in the job table.
- **Phase:** 3.
- **Seen in:** stash (video organiser, used only as a design reference), Synthseek (user-facing move tracking), Harmony.

### 1.3 "Missing" state and a deletion cap
- **Sees:** an unplugged disk or a bad mount does not wipe the catalogue.
- **Steps:** a scan that cannot find a known file marks the track "missing" with a date; it does not delete it. Missing tracks are hidden from lists but kept in playlists, and come back if the file reappears or matches by cheap key. A separate clean-up task, started by the owner, removes missing tracks older than N days. An incremental scan also refuses to mark more than a set share of the library missing in one run and reports why.
- **Keeps:** `missing_since` per file, the cap setting, the last scan summary.
- **Failures and safety:** an empty folder under a mount that failed looks like total loss; the cap catches it. Clean-up never touches audio files on disk, only catalogue rows.
- **Mus-Eq:** today a rescan "drops vanished" files (phase 1a); change that to the missing state and add the cap. Report via the existing job record and an event on the WebSocket.
- **Phase:** 3 (small change, can ship early).
- **Seen in:** Navidrome, RE-KORD, stash.

### 1.4 Multi-valued tags and a quality profile per track
- **Sees:** "Artist A; Artist B" becomes two linked artists when the owner wants it, and each track shows its real format (sample rate, bit depth, channels, lossless or not).
- **Steps:** a setting chooses how to read artist and genre fields: use real multi-value frames, split on a chosen separator, or keep the text as one value. The scanner stores the quality facts that symphonia already reports.
- **Keeps:** the setting, one row per credited artist, quality columns on the file row.
- **Failures and safety:** splitting on a separator breaks names that contain it (for example a band name with a slash). Default to "keep as one value" and let the owner opt in.
- **Mus-Eq:** needed when artist and album get their own tables (listed as a known gap after phase 1a). The same quality fields map to the OpenSubsonic fields (section 7).
- **Phase:** 3.
- **Seen in:** gonic, RiffPlayer, Navidrome.

---

## 2. Tag fixing and review

### 2.1 Import review with a similarity score and a tag diff
- **Sees:** a list of files or albums with a match percentage, the current tags beside the proposed ones, and Accept, Edit or Skip.
- **Steps:** read the tags (and optionally the fingerprint); search MusicBrainz or Discogs for candidate releases; score each candidate by how far it is from what the files say (title, artist, album, track count, track lengths); show the best few with the differences highlighted; apply only on accept.
- **Keeps:** the candidates and scores, the accepted choice, the before-values (so a change can be undone).
- **Failures and safety:** a wrong release gets accepted by a hurried click, so show the score, the count of changed fields and a one-click undo. Files with no match stay untouched and listed as unmatched. Never write without an accept.
- **Mus-Eq:** `tag` jobs fill a `tag_candidate` table; the web client shows the diff; accept writes SQLite first (Q12 default) and writes to the file only when the owner enables it. Call the public MusicBrainz and Discogs web APIs, respecting their rate limits.
- **Phase:** 3.
- **Seen in:** beets (MIT; the interaction, not the code), Picard (album-shaped confirm step), Harmony (lookup that writes nothing until save), octo (release chooser and tag preview).

### 2.2 Staging area with "needs attention" reasons
- **Sees:** new or imported albums wait in a "to review" list with a plain reason each: missing tags, no cover, duplicate track numbers, mixed albums, suspicious lengths.
- **Steps:** new files land in a staging state; a validator checks a fixed checklist; albums that pass move to the library automatically, the rest wait with their reasons; the owner fixes or overrides one by one.
- **Keeps:** the list of reasons per album, a content hash used to re-run validation only when something changed.
- **Failures and safety:** a strict checklist can strand the whole import, so every reason has an "accept anyway". Staging is a state in the catalogue, never a separate copy of the audio.
- **Mus-Eq:** a `staged` flag and a `needs_attention` table with a reason code; the same list shows the fix candidates from 2.1. Existing library files are not re-staged unless the owner asks.
- **Phase:** 3.
- **Seen in:** Melodee (MIT, a good model for the checklist), Synthseek (unidentified files list), Harmony (import staging).

### 2.3 Hand edits are protected; verify what a download really is
- **Sees:** a value the owner typed is never overwritten by a later rescan or lookup; a downloaded file whose audio is by someone else is rejected before it reaches the library.
- **Steps:** each field remembers its source (file, lookup, owner). Automatic sources may not overwrite an owner value. For a fetched file: fingerprint it, look up the recording, compare the artist and title with what was requested, and only then import; contradictions go to the review list.
- **Keeps:** a per-field source flag, the requested versus found identity per fetch job.
- **Failures and safety:** the fingerprint service needs a key and has terms (the free tier is non-commercial, Q10); if it is down, the file waits in review instead of passing unchecked.
- **Mus-Eq:** a `source` column on corrected fields; the verification step is part of the fetch job (section 4).
- **Phase:** 3.
- **Seen in:** RE-KORD, Synthseek, octo.

---

## 3. Fingerprint and duplicates

### 3.1 Two-stage duplicate finder with a keep-best proposal
- **Sees:** groups of "probably the same song" with every copy playable, a proposed best copy (lossless over lossy, higher bitrate, better tags, has cover), and a choice per group.
- **Steps:** stage one groups by cheap evidence: same identity, same recording id, same ISRC, normalised artist and title with similar duration. Stage two confirms with an audio fingerprint compared with a tolerance, then checks that durations agree. Grades run from exact to possible. The owner picks the keeper or "not a duplicate".
- **Keeps:** the fingerprint per file, the grade per pair, the decision ("keep this one", "not a duplicate") so it is not asked again.
- **Failures and safety:** live versions, remasters and radio edits can fingerprint as near-identical but are different on purpose, so only exact grades may be pre-selected, and nothing is ever auto-deleted. The fingerprint tool is meant for near-identical audio, not "sounds similar".
- **Mus-Eq:** a `fingerprint` job runs the `fpcalc` program on each new file and stores the result in SQLite; grouping is SQL plus a small comparison in Rust. Chromaprint should be run as a separate program or linked from the system, never built with the GPL FFT backend.
- **Phase:** 3.
- **Seen in:** Harmony (graded matches), Synthseek (policy), Chromaprint (the fingerprint), Lark (hash only), octo.

### 3.2 Safe resolution: recycle bin, never delete outright
- **Sees:** "Remove the worse copies" moves files to a bin that shows what, why and when, with Restore.
- **Steps:** resolving a group moves the losing files into a bin folder inside the library volume and marks them hidden; playlist entries that pointed at them are redirected to the keeper; the bin is emptied only by the owner or after a retention period the owner set.
- **Keeps:** original path, group id, reason, date, retention setting.
- **Failures and safety:** no auto-resolve except provable duplicates (same recording proof) and even then only with the setting on; a full disk must not trigger an automatic purge.
- **Mus-Eq:** a `trash` state plus a job that moves files; playlist redirect is one SQL update. Default retention is "never purge".
- **Phase:** 3.
- **Seen in:** Synthseek, Harmony ("never auto-replace").

### 3.3 Lossless-from-lossy check
- **Sees:** a badge "FLAC that looks like an upsampled MP3" on suspect files.
- **Steps:** analyse the frequency spectrum once; a hard cut-off around the typical lossy limit with nothing above suggests a re-encode. Show it as a hint, not a verdict.
- **Keeps:** a flag and the measured cut-off.
- **Failures and safety:** quiet or old recordings can look cut off; the hint never blocks an import by itself.
- **Mus-Eq:** optional job using decoded samples from symphonia and a small FFT crate. Low priority.
- **Phase:** 3 (optional; can slide).
- **Seen in:** octo.

---

## 4. Fetch pipeline and its legal gate

### 4.1 Request, resolve, fetch, verify, import
- **Sees:** the owner pastes a track or playlist link, watches a queue, and later finds organised files and a playlist. Tracks already in the library are marked and skipped.
- **Steps:** store the source link; list its tracks from public metadata; skip those already in the library; for each remaining track search for one exact match, download to a temporary file, verify it (section 2.3), write tags, move it to `Album Artist/Album/Track - Title`, add it to the library and the playlist.
- **Keeps:** the source snapshot, one job per track with its lifecycle (queued, started, done, failed with reason), and provenance (where the file came from).
- **Failures and safety:** no exact match means failed and out of the library, never "closest guess". Cookie or login requirements are reported, not worked around. Keep attempts bounded with backoff and keep partial results.
- **Mus-Eq:** `fetch` jobs in the existing table, an external downloader run as a pinned separate process, tags written by our own code, a rescan of the new files at the end.
- **Phase:** 3, behind the flag that is off by default.
- **Seen in:** Harmony, MusicSeeker, spotDL (MIT), RE-KORD, octo, Synthseek, selfhosted-music-server (guide).

### 4.2 Exact-match search that rejects wrong versions
- **Sees:** a requested studio track never silently becomes a live, remix or karaoke version.
- **Steps:** run one bounded search per track; reject results whose title carries version words the request does not have (live, remix, karaoke, cover, slowed and similar); check duration against the expected length; accept the first that passes, otherwise fail.
- **Keeps:** the rejection reasons per attempt, so the owner sees why nothing was found.
- **Failures and safety:** word lists are language-dependent and will be wrong sometimes; keep the list in a setting and the failure visible. A rescue method (a second tool) is allowed only with a short timeout.
- **Mus-Eq:** pure Rust matching on titles and durations; the search itself is the external tool's job.
- **Phase:** 3.
- **Seen in:** Harmony, spotDL (matching policy), octo.

### 4.3 The legal gate
- **Sees:** fetch is absent until the owner has switched it on, after reading a plain statement of the terms.
- **Steps:** the flag defaults to off; the server answers a clear error when fetch is requested while off; turning it on needs an explicit setting and an acknowledgement stored with a date; downloader versions are pinned and updated by the owner.
- **Keeps:** the flag, the acknowledgement, the downloader version in use.
- **Failures and safety:** most projects in this area depend on YouTube, Spotify, Soulseek, Usenet or torrents, which have terms and copyright exposure. Mus-Eq does not bundle Soulseek, torrent or Usenet grabbing; if the owner wants them, a separate service beside Mus-Eq may be called over its own HTTP API. Spotify use stays at reading public metadata, with the question of own-account sign-in left to Q6. Check each service's terms before the phase starts (a re-verification pass for every unverified licence or terms claim).
- **Mus-Eq:** config flag read at start and per request; `/v1` fetch routes return a fixed error when off; the Containerfile ships the pinned downloader only in the image variant that has the flag documented.
- **Phase:** 3 (Q2 and Q6 block it).
- **Seen in:** every fetch project above; the "wanted list" pattern in Lidarr is deliberately not copied.

### 4.4 Wanted versus have (optional, later)
- **Sees:** for a chosen artist, a list of albums that exist but are not in the library, with a button to request each.
- **Steps:** look up the artist's releases in MusicBrainz; compare with the library by identity; list the gaps; each request is a normal fetch job. A rejected or failed candidate tries the next one.
- **Keeps:** the monitored artist list, the last lookup date.
- **Failures and safety:** automatic grabbing of every missing album is where most legal trouble sits, so Mus-Eq only lists gaps and fetches on a click.
- **Mus-Eq:** a read-only view first; the click creates a `fetch` job.
- **Phase:** 3 (stretch goal; may slide to 7).
- **Seen in:** Lidarr, Ampache (a missing-albums feature), Synthseek.

---

## 5. Playlist extension and radio recommendations

### 5.1 Extend a playlist from the library
- **Sees:** a short playlist grows by N tracks that fit it, with no artist repeated and nothing already in it.
- **Steps:** take the playlist's genres and artists; build a candidate pool from the local library first (same genre, similar tags, optionally "similar" data from a listening service); drop artists already used; order by a mix of closeness and variety; append N; show what was added with Undo.
- **Keeps:** which tracks were added by the extender, so Undo removes only those.
- **Failures and safety:** a tiny library yields few candidates, so say so rather than loop. Network lookups are optional and cached.
- **Mus-Eq:** pure SQL and Rust over the catalogue; a `playlist_extend` endpoint; no learned ranking needed in v1.
- **Phase:** 3.
- **Seen in:** octo ("radio from this song"), Jellyfin (an instant mix for every kind of item), Navidrome, Synthseek (autoplay with the closest tracks the library has), RE-KORD.

### 5.2 Smart playlists as rules
- **Sees:** "recently added", "never played", "lossless only", "needs tag review" as live lists.
- **Steps:** a rule is groups of field, operator and value; rules are evaluated on demand.
- **Keeps:** the rule as JSON, no copied track list.
- **Failures and safety:** keep the rule set small; unknown fields are rejected.
- **Mus-Eq:** one `smart_playlist` table with a JSON rule, compiled to SQL with bound parameters (no string-built SQL).
- **Phase:** 3.
- **Seen in:** Navidrome, koel (MIT, mature rule builder), Lark, Ampache.

### 5.3 Library radio that blends sources
- **Sees:** a "start radio from this song, artist or genre" button that plays an endless queue.
- **Steps:** pick seeds; gather candidates from several sources with weights (library similarity, play history, optionally outside lists); drop recent repeats; lower a source's weight if its tracks keep getting skipped.
- **Keeps:** per-source weights, skip and play counts.
- **Failures and safety:** outside services need keys and have terms; the radio must still work with none of them.
- **Mus-Eq:** same engine as 5.1 feeding a queue; a station of kind `library` once phase 5 exists.
- **Phase:** 5 (the engine begins in 3).
- **Seen in:** octo, Synthseek, MusicSeeker (artist radio), RE-KORD (smart radio with block lists).

### 5.4 Submitting listens
- **Sees:** each finished play is reported to a listening-history service the owner has an account with.
- **Steps:** a play counts after a set share of the track or a time limit; send track, artist and optional MusicBrainz id with a timestamp; retry later if the service is down.
- **Keeps:** a small queue of unsent listens only.
- **Failures and safety:** duplicates on retry; only plays the owner actually listened to.
- **Mus-Eq:** web client or server posts JSON; queue in SQLite.
- **Phase:** 5 (low priority).
- **Seen in:** ListenBrainz server, Navidrome, RiffPlayer, gonic.

---

## 6. Stations and stream relay

### 6.1 Station list that is checked, not trusted
- **Sees:** a list of stations with dead ones flagged, a Play button, and an error that does not crash anything.
- **Steps:** seed the list from files; a liveness job tests each stream URL (connect, read a few bytes, note the content type); failures set a flag and a date, never a delete; the client plays the upstream URL directly.
- **Keeps:** URL, last check, last status, failure count.
- **Failures and safety:** plain-HTTP streams are blocked on HTTPS pages (already noted in `DECISIONS.md`); redirects and playlist-style URLs need resolving.
- **Mus-Eq:** the phase 2 station table and a `liveness` run on the job table.
- **Phase:** 2.
- **Seen in:** Navidrome and gonic (station lists), the Navidrome setup guide (stations as a plain list), Mopidy.

### 6.2 Station directory
- **Sees:** a search box that finds stations by name, country or tag and adds one to the list.
- **Steps:** query a public station directory; show results; add by click; the stored entry is the same as a seeded one.
- **Keeps:** the chosen entry only.
- **Failures and safety:** the directory's terms and rate limits are **unverified**; check before building. Never copy its whole database.
- **Mus-Eq:** a thin client in the server behind a setting; off if unreachable.
- **Phase:** 2 (optional, after the list works).
- **Seen in:** Lark.

### 6.3 Relaying a stream and reading "now playing"
- **Sees:** the station name and the current song title next to the player, and a stream that works for clients that cannot reach the station.
- **Steps:** connect to the station asking for in-band metadata; read the metadata blocks that arrive at a fixed byte interval; pass the audio through unchanged; publish the current title on the event socket.
- **Keeps:** the last title per station, in memory.
- **Failures and safety:** this conflicts with the decision that external stations are never proxied (licensing, load). Treat it as an open question: either keep passthrough and read only the title through a short-lived side connection, or relay for the owner's own use. Not decided.
- **Mus-Eq:** an axum streaming handler with a bounded buffer; one upstream connection per station shared by listeners.
- **Phase:** 5 (library and live stations use the same output code; external relay only if the decision changes).
- **Seen in:** koel (MIT; stream proxy and title reader), Ampache (broadcast), Navidrome.

### 6.4 Library station with rotation and an Icecast-style output
- **Sees:** a station URL any player can open, playing the owner's library with simple rules (no artist twice in an hour, a jingle every N songs).
- **Steps:** pick the next track by rules; decode, mix a short crossfade, encode to the chosen codec once, serve the same bytes to every listener.
- **Keeps:** the rotation state and a short history.
- **Failures and safety:** one encode per station, not per listener, or the 1-CPU target in the roadmap fails. Measure Opus in-process against ffmpeg first (already a phase 5 task).
- **Mus-Eq:** a station task owning a broadcast channel; listeners attach to it.
- **Phase:** 5.
- **Seen in:** Ampache (channels), Mixxx (broadcast output), Mopidy.

### 6.5 Listener-originated stream
- **Sees:** the owner talks into the phone; music ducks; listeners hear the mix.
- **Steps:** the phone mixes mic and music, sends an Ogg/Opus uplink; the server decodes and rebroadcasts through the station output from 6.4.
- **Keeps:** nothing beyond the live station's state.
- **Failures and safety:** latency and drop-outs; show a "live" indicator and fall back to the library rotation when the uplink stops.
- **Mus-Eq:** reuse 6.4's output; the mixer stays on the phone, as in the spec. A DJ surface (decks, beat matching) is out of scope.
- **Phase:** 6.
- **Seen in:** Ampache (broadcast and democratic play), Mixxx (broadcast concept), Jellyfin (group listening needs time sync, relevant only if listeners must be in step).

---

## 7. OpenSubsonic compatibility

### 7.1 A second API face
- **Sees:** existing phone and desktop apps for this protocol connect to Mus-Eq and browse, search and play.
- **Steps:** keep the own JSON API as the first-party contract; add an OpenSubsonic layer that maps its calls onto the same catalogue; answer the extension-discovery call with only the extensions that are truly supported.
- **Keeps:** nothing new; ids are mapped from the existing ones.
- **Failures and safety:** a half-implemented endpoint breaks apps worse than a missing one; publish a per-endpoint compatibility table and return the protocol's error for the rest. Prefer key-based login over the old salted-hash login; keep the old one only for legacy apps.
- **Mus-Eq:** an axum router under `/rest`, types generated from the Apache-2.0 OpenAPI files in the public spec, tests with a couple of real apps.
- **Phase:** 4.
- **Seen in:** the OpenSubsonic spec, Navidrome, Melodee (publishes a compatibility table), RiffPlayer, octo (a proxy in front of Navidrome), Feishin and Tempo (clients).

### 7.2 Extra data through extensions the apps ignore
- **Sees:** richer results in Mus-Eq's own client while stock apps keep working.
- **Steps:** add optional parameters or fields (quality filters, quality profile per track); proposals for new fields follow the spec's extension process.
- **Keeps:** nothing new.
- **Failures and safety:** do not change the meaning of any standard field.
- **Mus-Eq:** extra fields guarded by a header or parameter.
- **Phase:** 4.
- **Seen in:** RiffPlayer, Navidrome.

### 7.3 Client tells the server what it can play
- **Sees:** a phone on a weak connection gets a smaller stream without the owner doing anything.
- **Steps:** the client sends its supported codecs, maximum bitrate and container; the server answers "play as is" or "transcode to X"; a bounded cache keeps recent transcodes.
- **Keeps:** the client profile per device, a size-capped cache.
- **Failures and safety:** unsupported codec gets a clear error, never a silent re-encode (already a spec rule); cache never grows past its cap.
- **Mus-Eq:** `codec` and `kbps` on the audio endpoint (phase 5) and the matching OpenSubsonic transcoding calls.
- **Phase:** 4 (profile and API), 5 (the encoder).
- **Seen in:** Jellyfin (device profile), Melodee (per-device profiles), Navidrome, gonic (cache), Ariami (a small Rust transcoder; MIT, to be evaluated against ffmpeg).

---

## 8. Client packaging and single-session ideas

### 8.1 One web client in every shell
- **Sees:** the same player on the browser, Linux, Windows and Android.
- **Steps:** build the web client once; wrap it in a system web view shell for desktop and Android; the shell adds tray, media keys and file access.
- **Keeps:** client settings only; data stays on the server.
- **Failures and safety:** web view differences across platforms; media keys and lock-screen controls need native code on Android (Media3, per the roadmap).
- **Mus-Eq:** the Q13 decision (Tauri versus Qt Quick) is still open; Tauri is Apache-2.0 and Rust, so licensing is no obstacle.
- **Phase:** 4.
- **Seen in:** RE-KORD (one UI to desktop and Android), Lark (desktop shell), Feishin (Electron), Tempo and cliamp (separate Android apps).

### 8.2 Skins and themes
- **Sees:** classic Winamp skins in the player, plus plain colour themes for the rest of the interface.
- **Steps:** Webamp handles `.wsz` skins; a separate light theme file (colours, fonts) styles the library views.
- **Keeps:** the chosen skin and theme per device.
- **Failures and safety:** skin files are third-party works with their own copyright; do not ship them in the repo.
- **Mus-Eq:** keep the existing skin upload; add a small JSON theme format.
- **Phase:** 4.
- **Seen in:** Feishin (theme files in a folder), Lark (colour schemes separate from player styles), RiffPlayer (mobile Winamp style), koel, Webamp.

### 8.3 One playback session across devices
- **Sees:** start on the desktop, pick up on the phone; a phone acts as remote for the device that is playing.
- **Steps:** one device is the player and reports its state; others send commands through the server; "take over" hands the role to another device.
- **Keeps:** the current session in memory, last position in SQLite.
- **Failures and safety:** commands to a client that is gone must fail clearly (already true for the current session API); handover must stop the old player first.
- **Mus-Eq:** extend the phase 1b session with per-device roles and take-over; events over the existing socket (a one-way event stream is enough for job progress).
- **Phase:** 4.
- **Seen in:** Ariami (MIT), RiffPlayer, Jellyfin (group sessions), Octo.

### 8.4 Offline pins
- **Sees:** tracks or playlists marked "keep on phone" play with the screen off and no network.
- **Steps:** the phone downloads original or transcoded copies and records a sync position; the catalogue stays the server's; the phone cache is a replica.
- **Keeps:** pinned set, sync cursor, size budget.
- **Failures and safety:** low storage; changed files on the server (compare checksum); do not treat the phone as a second library.
- **Mus-Eq:** per the roadmap's Android item.
- **Phase:** 4.
- **Seen in:** Ariami, RiffPlayer, Lark (browser cache), Tempo (partial).

### 8.5 Control from other programs
- **Sees:** a status-bar widget, a script or another program can search, queue and control playback.
- **Steps:** a small documented set of calls over the same API, with a read-only token.
- **Keeps:** tokens per device.
- **Failures and safety:** a tool-calling endpoint can start downloads or delete things, so it gets a limited token that cannot touch fetch or delete.
- **Mus-Eq:** per-device read-only tokens (dropped in phase 1b, revisit here); the tool endpoint is an idea only, no commitment.
- **Phase:** 4.
- **Seen in:** Lark and Synthseek (tool endpoints), Feishin and cliamp (multi-server clients).

---

## Top 5 takeaways per phase

### Phase 2: stations
1. Stations are stream URLs the server stores; clients play them. No decoding, no proxy (Navidrome, Mopidy, the setup guide).
2. A liveness job that flags and never deletes, with the date of the last check.
3. Dead or plain-HTTP streams must fail gently in the client.
4. A station directory is a possible add-on; its terms are unverified, so it stays optional.
5. Generated "artist radio" is a playlist feature, not a station problem; leave it for phase 3 and 5.

### Phase 3: metadata, duplicates, fetch, extension
1. Stable identity (ids first, tag tuple fallback) plus a cheap fingerprint for renames and a "missing" state with a deletion cap, before anything else touches the catalogue.
2. Tag fixes are candidates with a score and a visible diff, applied only on accept, with undo (beets, Picard, Harmony).
3. A staging list with plain reasons, and hand edits that automatic sources cannot overwrite (Melodee, RE-KORD).
4. Duplicates: cheap evidence first, audio fingerprint to confirm, every copy playable, recycle bin, never auto-delete (Synthseek, Harmony, Chromaprint).
5. Fetch is behind a flag that is off by default, verifies what it downloaded, rejects wrong versions, and never grabs a whole wanted list on its own (Harmony, octo, Lidarr as the thing to avoid).

### Phase 4: native clients and offline
1. Add an OpenSubsonic face with only the extensions we really support; existing apps then work on day one (spec, Navidrome, Melodee).
2. One web client wrapped for desktop and Android; decide the shell at Q13 (RE-KORD, Lark).
3. Clients send a device profile and the server decides direct play or transcode (Jellyfin, Melodee).
4. One playback session across devices with take-over (Ariami, RiffPlayer).
5. Offline pins as a replica with a sync cursor and a size budget (Ariami, RiffPlayer); skins stay Webamp, themes become a small JSON file (Feishin, Lark).

### Phase 5: library radio and transcoding
1. One encode per station, shared by all listeners; measure first against the 1-CPU goal.
2. Rotation rules are simple and visible (no artist twice in an hour, a jingle every N songs).
3. Transcoding needs a size-capped cache and an explicit codec (gonic, Navidrome).
4. Radio recommendations blend several weighted sources and still work with none (octo, Synthseek).
5. Reading "now playing" from external streams needs a decision about proxying first (koel as the example, conflicting with the passthrough rule).

### Phase 6: live phone radio
1. Reuse the phase 5 station output; the live station is just another source (Ampache broadcast).
2. Mixing stays on the phone; the server decodes one uplink.
3. Fall back to library rotation when the uplink drops; show a clear live indicator.
4. Expect latency; the roadmap target is about 5 seconds for a web listener.
5. A DJ surface is out of scope (Mixxx); synchronised group listening only if a need appears (Jellyfin).
