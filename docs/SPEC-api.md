# API v1 (prose summary)

The contract is `docs/openapi.yaml`; this page only explains the conventions.

- JSON over HTTP, one WebSocket (`/v1/events`) for session and job events.
- Auth: one bearer token per device/user; `/v1/health` is open. Read-only tokens for widgets and other apps (not enforced yet).
- Audio is a plain byte stream (`/v1/audio/{id}`, Range supported); transcode only when `codec`/`kbps` is asked for.
- Playback state is one session document that every client reads and controls.
- Fetch jobs are refused with 403 unless enabled in config (off by default).
- Implemented so far: `GET /v1/health`. Everything else is draft.
