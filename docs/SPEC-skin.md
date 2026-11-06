# Skins (v0)

- A skin is a classic `.wsz`: a zip of BMP/CUR/TXT files. The server stores the raw upload and never unpacks it; the browser (Webamp) parses it. Limits: zip magic bytes checked, at most 8 MiB, name sanitised, id = first 16 hex of the SHA-256 (so re-uploading the same file is a no-op).
- API: `GET /v1/skins` (list), `POST /v1/skins?name=` (raw body), `GET /v1/skins/{id}` (the file). Auth like the rest.
- Web client: the default skin is Webamp's bundled base skin. Uploaded skins appear in the page's skin list and are applied with `setSkinFromUrl`. Webamp's own skin menu is filled once at load, so a skin uploaded later shows there after a reload.
- Licences: bundle only skins whose licence allows it. Webamp itself is MIT (checked in its `package.json`); what it says about the base skin's licence is not verified yet.
- Parity rule for later clients (Android WebView, desktop shell): render the same `.wsz`; a golden-image test of the base skin is added when the second renderer exists (phase 4).
- Not in scope: modern `.wal` skins, a custom skin format, extracting or editing skins on the server.
