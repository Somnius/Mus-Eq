# Open questions (answered ones move to `DECISIONS.md`)

| # | Question | Default if unanswered | Blocks |
|---|---|---|---|
| Q5 | Skin engine on native: Webamp in a WebView for v1, or a native renderer from day one? | WebView | phase 4 |
| Q16 | Fetch: are playlists and albums wanted (one job per track, or one job that expands)? | single tracks only | phase 3 review |
| Q17 | Should `yt-dlp` ship in the container image (pinned, with checksum), or stay the owner's own binary? | owner's binary | phase 4 |
| Q7 | Phone-as-server: embedded server, Termux, or only player + mixer on the phone? | player + mixer only | phase 7 |
| Q8 | Duplicates: server-side first, phone later (a friend thinks the phone can do it)? | server first | phase 3 |
| Q10 | Commercial use someday? (AcoustID free tier is non-commercial) | no | phase 3 |
| Q12 | Tag writes: in place, or corrections only in SQLite until accepted? | SQLite first | phase 3 |
| Q13 | Desktop shell: Tauri/`wry` around Webamp, or Qt Quick? macOS builds wanted. | browser first | phase 4 |
| Q15 | Now-playing titles for external stations (ICY metadata): keep pure passthrough and read titles through a short-lived side connection, or relay the stream for the owner? | passthrough + side connection | phase 5 |
