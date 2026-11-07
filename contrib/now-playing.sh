#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Lef
# Print what Mus-Eq is playing, for a status bar or widget. Needs curl and jq.
#   now-playing.sh            -> "Artist - Title" (or nothing when stopped), exit 0
#   now-playing.sh --json     -> a {text, tooltip, class} object (Waybar-style)
#   now-playing.sh pause|play|next|previous   -> send a command to the playing client
# Settings: MUSEQ_URL (default http://127.0.0.1:8080), MUSEQ_TOKEN or MUSEQ_TOKEN_FILE.
set -euo pipefail
url=${MUSEQ_URL:-http://127.0.0.1:8080}
token=${MUSEQ_TOKEN:-}
[ -z "$token" ] && [ -n "${MUSEQ_TOKEN_FILE:-}" ] && token=$(tr -d '[:space:]' < "$MUSEQ_TOKEN_FILE")
[ -n "$token" ] || { echo "set MUSEQ_TOKEN or MUSEQ_TOKEN_FILE" >&2; exit 2; }
# The token goes to curl through stdin (--config -), so it never shows up in the process list.
api() { # api METHOD PATH [JSON_BODY]
  local method=$1 path=$2 body=${3:-}
  { printf 'header = "Authorization: Bearer %s"\n' "$token"; } |
    curl -fsS --max-time 3 --config - -X "$method" -H 'content-type: application/json' ${body:+-d "$body"} "$url$path"
}

case "${1:-}" in
  play|pause|next|previous)
    api POST /v1/session "{\"action\":\"$1\"}" >/dev/null ;;
  ""|--json)
    s=$(api GET /v1/session 2>/dev/null) || { [ "${1:-}" = --json ] && echo '{"text":"","class":"offline"}'; exit 0; }
    state=$(jq -r .state <<<"$s")
    text=$(jq -r 'if .track then ((.track.artist // "") + (if .track.artist then " - " else "" end) + .track.title) else "" end' <<<"$s")
    if [ "${1:-}" = --json ]; then
      jq -n --arg t "$text" --arg c "$state" '{text:$t, tooltip:$t, class:$c}'
    elif [ "$state" != stopped ]; then
      echo "$text"
    fi ;;
  *) echo "usage: $0 [--json|play|pause|next|previous]" >&2; exit 2 ;;
esac
