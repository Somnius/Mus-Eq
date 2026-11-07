# Contributing

It's very early, so the most useful things right now are bug reports, ideas and station suggestions. Use the issue templates.

- Code changes: open an issue first so we don't build the same thing twice. Check `docs/ROADMAP.md` to see what phase we're in.
- The API contract is `docs/openapi.yaml`. Change the contract in the same PR as the code.
- Before sending a PR: `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and lint the OpenAPI file if you touched it.
- Station list changes go in `data/stations/trayfm-stations.txt` style: `Name|Category|URL`. Please check the stream actually plays.
- Fetching from YouTube Music, Spotify or Deezer may break their terms. Anything touching that stays behind the config flag that is off by default.
- Licence: by sending a contribution you agree it is licensed under MIT OR Apache-2.0, the same as the project, with no additional terms.
- Sign your commits off (`git commit -s`), which adds a `Signed-off-by:` line. It means you wrote the change or have the right to send it, as set out in the Developer Certificate of Origin (https://developercertificate.org/).
- New source files (Rust, TypeScript, Kotlin, scripts) start with the two SPDX lines the others have; `python3 scripts/add-spdx.py` adds them and CI checks for them.
- Dependencies: a new crate must be under a licence listed in `deny.toml`. `python3 scripts/check-licences.py` checks this (no extra tool to install); CI also runs cargo-deny as a second opinion. A licence that is not on the list needs a decision first, recorded in `docs/DECISIONS.md`.

