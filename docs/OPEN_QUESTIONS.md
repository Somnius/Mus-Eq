# Open questions for Lef (answer here; SPEC and ROADMAP get updated when one is settled)

| # | Question | Default if unanswered | Blocks |
|---|---|---|---|
| Q1 | ~~Server language~~ **Decided 2026-11-06: Rust** (Android client stays Kotlin; cliamp kept as a separate client). Reversible before phase 1. | - | - |
| Q2 | May fetch store audio from YouTube Music? (flag, off by default either way) | no | phase 3 |
| Q3 | Which "Omarchy radio"? URL/path/plugin name. Nothing found by search or in `~/.config/omarchy`. | one hand-entered URL | phase 2 |
| Q4 | ~~Greek radio list~~ **Answered 2026-11-06:** Tray-FM by Constantinos (Linux-user.gr), Unlicense, seed copied to `data/stations/trayfm-stations.txt`. Still to do: check which URLs are alive (phase 2). | - | - |
| Q5 | Skin engine on native: Webamp in a WebView for v1, or native renderer from day one? | WebView | phase 4 |
| Q6 | Spotify: link resolution only, or your own account (OAuth)? | link only | phase 3 |
| Q7 | Phone-as-server: embedded server, Termux, or only player + mixer on the phone? | player + mixer only | phase 7 |
| Q8 | Duplicates: server-side first, phone later (a friend thinks the phone can do it)? | server first | phase 3 |
| Q9 | Users: one owner (+ read tokens) or accounts? No hosted auth service. | one owner, token auth | phase 1 |
| Q10 | Commercial use someday? (AcoustID free tier is non-commercial) | no | phase 3 |
| Q11 | Deploy priority: LAN only, Tailscale, public + TLS? | LAN/Tailscale | phase 1 |
| Q12 | Tag writes: in place, or corrections only in SQLite until accepted? | SQLite first | phase 3 |
| Q13 | Desktop shell: Tauri/`wry` around Webamp, or Qt Quick later? macOS builds wanted. | browser first | phase 4 |
