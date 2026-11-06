# Brief: music player on phone + web-server companion (raw requirements from Lef, 2026-11-06)

## Clients
- Web-based player.
- Native clients for Windows, Linux, Android.
- Skin engine; first skin is classic Winamp, on all platforms ("easy").

## Bot / fetching
- Fetch singles and playlists. Spotify -> Deezer to resolve a song id, then fetch from YouTube Music.
- Playlist extension feature: when the last song of a user-created playlist finishes, extend it with the same theme/music style, skip artists already in the playlist, introduce new ones.

## Storage
- Online storage (on the server) and offline storage (on the device).

## Streaming / radio
1. Online stations (first: the Omarchy radio). Greek radio stations, from the list used by the Linux user app (name not remembered).
2. Self-hosted fake radio built from music tracks in the online storage.
3. Radio server on the phone: mic input with auto-lowered music volume (ducking) and a virtual mixer.
- Transcoding for the radio with codec options (e.g. Opus in Ogg, others).

## Hosting / deployment
- Self-hosted: on a server, or on the phone or PC itself with a local server.
- Headless server in a Docker or Podman container.
- Reachable via LAN/intranet, Tailscale/VPN/tunnel, or a public endpoint.
- API for control: embedded radio in other apps, control from other apps, widgets.

## Existing code / sources to reuse
- cliamp (code to reuse).
- Omarchy plugin marketplace, for the radio apps / widgets.
- Greek radio list from the Linux user app.

## Metadata (Features #3)
- Update ID3 tags from CD databases (MusicBrainz/freedb-style) and other sources; Discogs (https://www.discogs.com/) is a named source.
- From a friend's feedback:
  - Identify a song by audio (fixes a wrong filename/metadata). Lef: awkward, unsure a free API exists, look at it in phase 2-3.
  - Identify duplicates by content, not only filename; the best-quality file stays. Lef: easy, but not at Android level, needs audio pattern matching. Friend: believes the phone can do it locally.
