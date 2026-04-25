#!/usr/bin/env bash
# Red-test for scripts/check-demo.sh.
#
# Builds three throwaway scenarios — passing baseline, mismatched run counts,
# missing diff substring — and asserts the checker exits 0/1 as appropriate.
# Lets us prove the guard actually fails when the demo regresses, without
# needing stripe-mock or a real ace binary (we use a stub).
#
# Usage: bash scripts/check-demo.test.sh
# Exit 0 if all sub-tests pass.

set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECKER="$DIR/check-demo.sh"
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

# A fake `ace` that synthesizes a `diff` JSON/text from the two trace files.
# It compares terminal_state and synthesizes one routing_diverged divergence
# whenever they differ. Enough to drive check-demo's diff path deterministically.
mkdir -p "$ROOT/bin"
cat > "$ROOT/bin/ace" <<'STUB'
#!/usr/bin/env bash
# Minimal ace stub. Supports: ace diff <a> <b> [--format json]
set -euo pipefail
[[ "${1:-}" == "diff" ]] || { echo "stub only supports 'diff'" >&2; exit 2; }
A=$2; B=$3; FMT=text
shift 3
while [[ $# -gt 0 ]]; do
  case "$1" in
    --format) FMT=$2; shift 2 ;;
    *) shift ;;
  esac
done
python3 - "$A" "$B" "$FMT" <<'PY'
import sys, json
with open(sys.argv[1]) as f: a = json.load(f)[0]
with open(sys.argv[2]) as f: b = json.load(f)[0]
fmt = sys.argv[3]
divs = []
if a.get("terminal_state") != b.get("terminal_state"):
    divs.append({
        "user": 1, "step": "apply_discount", "occurrence": 0,
        "kind": {"kind": "routing_diverged"},
    })
    divs.append({
        "user": 1, "step": "fetch_subscription", "occurrence": 0,
        "kind": {"kind": "step_missing_in_b"},
    })
if fmt == "json":
    print(json.dumps({
        "divergences": divs,
        "summary": {"total_steps": 2, "divergences": len(divs)},
    }))
else:
    if not divs:
        print("no divergences across 2 step(s).")
    else:
        for d in divs:
            print(f"User 1 / step \"{d['step']}\"")
            print(f"  routing diverged — body.discounts now present")
        print(f"{len(divs)} divergence(s) across 2 step(s).")
PY
STUB
chmod +x "$ROOT/bin/ace"
export ACE="$ROOT/bin/ace"

# Helper: write a fake trace file (the array-of-ExecutionLog shape ace emits).
write_trace() {
  local out=$1 terminal=$2 passed=$3 failed=$4
  cat > "$out" <<EOF
[
  {
    "schema_version": 1,
    "steps": [],
    "total_duration_ms": 1,
    "total_steps": $((passed + failed)),
    "passed": $passed,
    "failed": $failed,
    "iterations": 1,
    "terminal_state": "$terminal",
    "seed": 0
  }
]
EOF
}

# ---- fixture 1: passing baseline ----
F1=$ROOT/pass; mkdir -p "$F1"
write_trace "$F1/staging.json" "done"            2 0
write_trace "$F1/prod.json"    "upgrade_required" 0 1
cat > "$F1/expected.yaml" <<EOF
runs:
  staging: { log: staging.json, terminal_state: done,             steps_passed: 2, steps_failed: 0 }
  prod:    { log: prod.json,    terminal_state: upgrade_required, steps_passed: 0, steps_failed: 1 }
diff:
  staging_vs_prod:
    a: staging.json
    b: prod.json
    divergence_count: 2
    kinds:
      - routing_diverged
      - step_missing_in_b
    required_substrings:
      - "body.discounts"
EOF

# ---- fixture 2: terminal_state mismatch on prod ----
F2=$ROOT/fail_state; mkdir -p "$F2"
write_trace "$F2/staging.json" "done" 2 0
write_trace "$F2/prod.json"    "done" 2 0   # bug: prod also passes — no drift
cat > "$F2/expected.yaml" <<EOF
runs:
  staging: { log: staging.json, terminal_state: done,             steps_passed: 2, steps_failed: 0 }
  prod:    { log: prod.json,    terminal_state: upgrade_required, steps_passed: 0, steps_failed: 1 }
EOF

# ---- fixture 3: diff substring missing ----
F3=$ROOT/fail_substr; mkdir -p "$F3"
write_trace "$F3/staging.json" "done"            2 0
write_trace "$F3/prod.json"    "upgrade_required" 0 1
cat > "$F3/expected.yaml" <<EOF
diff:
  staging_vs_prod:
    a: staging.json
    b: prod.json
    divergence_count: 2
    required_substrings:
      - "this string is not in the diff output"
EOF

# ---- run sub-tests ----
PASS=0; FAIL=0
expect() {
  local desc=$1 want=$2 got=$3
  if [[ "$got" == "$want" ]]; then
    echo "  OK   $desc (exit=$got)"; PASS=$((PASS+1))
  else
    echo "  FAIL $desc (want=$want got=$got)"; FAIL=$((FAIL+1))
  fi
}

set +e
CACHE=$F1 bash "$CHECKER" "$F1/expected.yaml" >/dev/null; expect "passing baseline exits 0" 0 $?
CACHE=$F2 bash "$CHECKER" "$F2/expected.yaml" >/dev/null; expect "terminal_state mismatch exits 1" 1 $?
CACHE=$F3 bash "$CHECKER" "$F3/expected.yaml" >/dev/null; expect "missing diff substring exits 1" 1 $?
set -e

echo
if [[ $FAIL -eq 0 ]]; then
  echo "PASS ($PASS sub-tests)"
  exit 0
else
  echo "FAIL ($FAIL of $((PASS+FAIL)) sub-tests failed)"
  exit 1
fi
