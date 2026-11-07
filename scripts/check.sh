#!/usr/bin/env bash
# Full local check: fmt, clippy, Rust tests, web build, OpenAPI lint, headless-browser smoke test.
# Keeps the machine under its CPU cap (24 of 32 threads, nice 10).
set -euo pipefail
cd "$(dirname "$0")/.."
# keep the machine under its CPU cap: pin to 24 cores when there are that many, always nice
if [ "$(nproc)" -ge 24 ]; then run() { taskset -c 0-23 nice -n 10 "$@"; }; else run() { nice -n 10 "$@"; }; fi
run cargo fmt --all --check
run cargo clippy --all-targets -- -D warnings
run cargo test
(cd web && run npm ci --no-audit --no-fund >/dev/null && run npm run build >/dev/null)
npx --yes @redocly/cli@2.76.0 lint docs/openapi.yaml | tail -2
python3 scripts/readme-badges.py --check

mkdir -p .work/tmp; work=$(mktemp -d "$PWD/.work/tmp/check.XXXXXX"); trap 'kill ${pid:-0} 2>/dev/null || true; rm -rf "$work"' EXIT
# the browser test's library: the standard fixtures plus four short Rock tracks by different artists
mkdir -p "$work/library"; cp -r crates/museq-server/tests/fixtures/lib/. "$work/library/"; cp crates/museq-server/tests/fixtures/ext/*.mp3 "$work/library/"
token=$(head -c 24 /dev/urandom | base64 | tr -d '/+=')
port=${MUSEQ_TEST_PORT:-8096}
MUSEQ_ALLOW_PRIVATE_STATIONS=1 MUSEQ_TOKEN=$token MUSEQ_BIND=127.0.0.1:$port MUSEQ_LIBRARY=$work/library \
  MUSEQ_DB=$work/db MUSEQ_SKINS_DIR=$work/skins MUSEQ_WEB_DIR=$PWD/web/dist MUSEQ_STATIONS_SEED=$PWD/crates/museq-server/tests/fixtures/stations-seed.txt ./target/debug/museq-server >"$work/log" 2>&1 &
pid=$!
# wait for the server to answer (a slow runner needs more than a second), up to 30 s
for _ in $(seq 1 60); do curl -s -o /dev/null "http://127.0.0.1:$port/" && break; kill -0 "$pid" 2>/dev/null || { echo "FAIL: server exited"; cat "$work/log"; exit 1; }; sleep 0.5; done
curl -fsS -X POST -H "Authorization: Bearer $token" -H 'content-type: application/json' -d '{"kind":"scan"}' "http://127.0.0.1:$port/v1/jobs" >/dev/null
sleep 1
(cd web && MUSEQ_URL=http://127.0.0.1:$port MUSEQ_TOKEN=$token run node e2e/smoke.mjs)
grep -q "$token" "$work/log" && { echo "FAIL: token found in server log"; exit 1; } || echo "ok   token not in server log"
echo "ALL CHECKS PASSED"
