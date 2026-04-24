#!/usr/bin/env bash
# Reproduces an `ace diff` against real Stripe API drift.
#
# Fetches two versions of Stripe's public OpenAPI spec (a 2022 release and a
# recent one), runs each under `stripe-mock`, then runs the same ACE workflow
# against both. The diff output names the exact field Stripe renamed.
#
# Requires: `ace` on PATH, `stripe-mock` on PATH, `curl`, `python3`.
# Override with ACE=/path, STRIPE_MOCK=/path.

set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ACE="${ACE:-ace}"
STRIPE_MOCK="${STRIPE_MOCK:-stripe-mock}"
CACHE="${CACHE:-/tmp/ace-stripe-drift}"
PORT_OLD="${PORT_OLD:-12201}"
PORT_NEW="${PORT_NEW:-12202}"

# Dated Stripe OpenAPI snapshots — tags from stripe/openapi:
#   v353  → 2022-11-15  (Subscription has `discount`, `current_period_end`)
#   v2253 → 2026-04-22  (Subscription has `discounts`, drops `current_period_end`)
OLD_SHA="${OLD_SHA:-7e9d4151}"
NEW_SHA="${NEW_SHA:-25286160}"

mkdir -p "$CACHE"

fetch_spec() {
  local sha=$1
  local dst=$2
  if [[ ! -s "$dst" ]]; then
    echo "▶ fetching Stripe OpenAPI @ $sha"
    curl -sSfL -o "$dst" \
      "https://raw.githubusercontent.com/stripe/openapi/$sha/openapi/spec3.json"
  fi
}
fetch_spec "$OLD_SHA" "$CACHE/spec-old.json"
fetch_spec "$NEW_SHA" "$CACHE/spec-new.json"

echo "▶ starting stripe-mock instances"
"$STRIPE_MOCK" -spec "$CACHE/spec-old.json" -http-port "$PORT_OLD" > "$CACHE/sm-old.log" 2>&1 &
OLD_PID=$!
"$STRIPE_MOCK" -spec "$CACHE/spec-new.json" -http-port "$PORT_NEW" > "$CACHE/sm-new.log" 2>&1 &
NEW_PID=$!

cleanup() { kill "$OLD_PID" "$NEW_PID" 2>/dev/null || true; }
trap cleanup EXIT

# stripe-mock takes ~1s to index 450+ endpoints
sleep 1.5

echo
echo "════════════════════════════════════════════════════════════════"
echo " 1/3  staging run  (Stripe spec 2022-11-15 — pinned, not upgraded)"
echo "════════════════════════════════════════════════════════════════"
"$ACE" run "$DIR/scenario.yaml" \
  --var "base_url=http://localhost:$PORT_OLD" \
  -o "$CACHE/staging.json" || true

echo
echo "════════════════════════════════════════════════════════════════"
echo " 2/3  prod run     (Stripe spec 2026-04-22 — backend upgraded)"
echo "════════════════════════════════════════════════════════════════"
"$ACE" run "$DIR/scenario.yaml" \
  --var "base_url=http://localhost:$PORT_NEW" \
  -o "$CACHE/prod.json" || true

echo
echo "════════════════════════════════════════════════════════════════"
echo " 3/3  ace diff staging.json prod.json"
echo "════════════════════════════════════════════════════════════════"
"$ACE" diff "$CACHE/staging.json" "$CACHE/prod.json" || true

echo
echo "  specs:  $CACHE/spec-{old,new}.json"
echo "  logs:   $CACHE/{staging,prod}.json"
