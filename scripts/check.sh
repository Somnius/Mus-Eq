#!/usr/bin/env bash
# Full local check: fmt, clippy, Rust tests, web build, OpenAPI lint, headless-browser smoke test.
# Keeps the machine under its CPU cap (24 of 32 threads, nice 10).
set -euo pipefail
cd "$(dirname "$0")/.."
run() { taskset -c 0-23 nice -n 10 "$@"; }
run cargo fmt --all --check
run cargo clippy --all-targets -- -D warnings
run cargo test
(cd web && run npm ci --no-audit --no-fund >/dev/null && run npm run build >/dev/null)
npx --yes @redocly/cli@2.76.0 lint docs/openapi.yaml | tail -2

work=$(mktemp -d); trap 'kill ${pid:-0} 2>/dev/null || true; rm -rf "$work"' EXIT
token=$(head -c 24 /dev/urandom | base64 | tr -d '/+=')
port=${MUSEQ_TEST_PORT:-8096}
MUSEQ_TOKEN=$token MUSEQ_BIND=127.0.0.1:$port MUSEQ_LIBRARY=$PWD/crates/museq-server/tests/fixtures/lib \
  MUSEQ_DB=$work/db MUSEQ_SKINS_DIR=$work/skins MUSEQ_WEB_DIR=$PWD/web/dist ./target/debug/museq-server >"$work/log" 2>&1 &
pid=$!
sleep 1
curl -fsS -X POST -H "Authorization: Bearer $token" -H 'content-type: application/json' -d '{"kind":"scan"}' "http://127.0.0.1:$port/v1/jobs" >/dev/null
sleep 1
(cd web && MUSEQ_URL=http://127.0.0.1:$port MUSEQ_TOKEN=$token run node e2e/smoke.mjs)
grep -q "$token" "$work/log" && { echo "FAIL: token found in server log"; exit 1; } || echo "ok   token not in server log"
echo "ALL CHECKS PASSED"
