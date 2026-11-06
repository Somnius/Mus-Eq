# Contributing

It's very early, so the most useful things right now are bug reports, ideas and station suggestions. Use the issue templates.

- Code changes: open an issue first so we don't build the same thing twice. Check `docs/ROADMAP.md` to see what phase we're in.
- The API contract is `docs/openapi.yaml`. Change the contract in the same PR as the code.
- Before sending a PR: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and lint the OpenAPI file if you touched it.
- Station list changes go in `data/stations/trayfm-stations.txt` style: `Name|Category|URL`. Please check the stream actually plays.
- Fetching from YouTube Music, Spotify or Deezer may break their terms. Anything touching that stays behind the config flag that is off by default.
