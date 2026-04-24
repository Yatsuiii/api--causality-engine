#!/usr/bin/env bash
# End-to-end reproduction of the env-diff demo.
#
# Spawns two local mock backends with deliberately divergent response schemas,
# runs the same checkout workflow against each, then diffs the traces.
#
# Requires: `ace` on PATH (or set ACE=/path/to/ace before running).

set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ACE="${ACE:-ace}"
PORT_OLD="${PORT_OLD:-9101}"
PORT_NEW="${PORT_NEW:-9102}"
OUT_DIR="${OUT_DIR:-/tmp/ace-env-diff}"

mkdir -p "$OUT_DIR"

echo "▶ starting mock backends"
"$ACE" mock "$DIR/servers/backend-old.yaml" --port "$PORT_OLD" > "$OUT_DIR/mock-old.log" 2>&1 &
OLD_PID=$!
"$ACE" mock "$DIR/servers/backend-new.yaml" --port "$PORT_NEW" > "$OUT_DIR/mock-new.log" 2>&1 &
NEW_PID=$!

cleanup() { kill "$OLD_PID" "$NEW_PID" 2>/dev/null || true; }
trap cleanup EXIT

sleep 0.5

echo
echo "════════════════════════════════════════════════════════════════"
echo " 1/3  staging run  (old backend — :$PORT_OLD, emits \`status\`)"
echo "════════════════════════════════════════════════════════════════"
"$ACE" run "$DIR/checkout.yaml" \
  --var "base_url=http://localhost:$PORT_OLD" \
  -o "$OUT_DIR/staging.json" || true

echo
echo "════════════════════════════════════════════════════════════════"
echo " 2/3  prod run     (new backend — :$PORT_NEW, emits \`state\`)"
echo "════════════════════════════════════════════════════════════════"
"$ACE" run "$DIR/checkout.yaml" \
  --var "base_url=http://localhost:$PORT_NEW" \
  -o "$OUT_DIR/prod.json" || true

echo
echo "════════════════════════════════════════════════════════════════"
echo " 3/3  ace diff staging.json prod.json"
echo "════════════════════════════════════════════════════════════════"
"$ACE" diff "$OUT_DIR/staging.json" "$OUT_DIR/prod.json" || true

echo
echo "  logs:  $OUT_DIR/{staging,prod}.json"
echo "  mocks: $OUT_DIR/mock-{old,new}.log"
