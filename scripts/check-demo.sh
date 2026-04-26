#!/usr/bin/env bash
# Validates a demo run against its expected.yaml manifest.
#
# Usage:   check-demo.sh <expected.yaml>
# Env:     ACE=/path/to/ace        (binary; defaults to "ace" on PATH)
#          CACHE=/path/to/dir      (root for `log`, `a`, `b` paths in the
#                                   manifest; defaults to /tmp/ace-stripe-drift,
#                                   matching examples/stripe-drift/run-demo.sh)
#
# Requires: python3 with PyYAML (preinstalled on ubuntu-latest GH runners),
#           ace on PATH (or ACE= env).
#
# Exit 0 on match, exit 1 with details on mismatch.

set -euo pipefail

EXPECTED_YAML="${1:?expected.yaml path required}"
ACE="${ACE:-ace}"
CACHE="${CACHE:-/tmp/ace-stripe-drift}"
FAIL=0

require() {
  command -v "$1" >/dev/null 2>&1 || { echo "error: $1 not found on PATH"; exit 1; }
}
require python3
python3 -c 'import yaml' 2>/dev/null || {
  echo "error: PyYAML not available — install with 'pip install pyyaml'"
  exit 1
}

# Convert expected.yaml to JSON via PyYAML once; all subsequent reads use the JSON.
MANIFEST_JSON=$(python3 -c '
import sys, json, yaml
with open(sys.argv[1]) as f:
    data = yaml.safe_load(f) or {}
print(json.dumps(data))
' "$EXPECTED_YAML")

# Resolve a manifest path against $CACHE. Absolute paths pass through unchanged
# so old absolute-path manifests still work (back-compat).
resolve_path() {
  local p=$1
  case "$p" in
    /*) printf '%s' "$p" ;;
    *)  printf '%s/%s' "$CACHE" "$p" ;;
  esac
}

# Get a scalar value by dotted path from the manifest JSON.
py_get() {
  python3 -c '
import sys, json
data = json.loads(sys.argv[1])
val = data
for k in sys.argv[2].split("."):
    if not isinstance(val, dict):
        val = None; break
    val = val.get(k)
print("" if val is None else val)
' "$MANIFEST_JSON" "$1"
}

check_run() {
  local name=$1
  local log log_raw expected_state expected_passed expected_failed

  log_raw=$(py_get "runs.${name}.log")
  log=$(resolve_path "$log_raw")
  expected_state=$(py_get "runs.${name}.terminal_state")
  expected_passed=$(py_get "runs.${name}.steps_passed")
  expected_failed=$(py_get "runs.${name}.steps_failed")

  if [[ -z "$log_raw" ]]; then echo "  SKIP run[$name]: no log path"; return; fi
  if [[ ! -f "$log" ]]; then echo "  FAIL run[$name]: log not found: $log"; FAIL=1; return; fi

  local ok=1
  python3 -c '
import sys, json
with open(sys.argv[1]) as f:
    data = json.load(f)
user = data[0]
name = sys.argv[5]
checks = [
    ("terminal_state", str(user.get("terminal_state", "")), sys.argv[2]),
    ("steps_passed",   str(user.get("passed", "")),         sys.argv[3]),
    ("steps_failed",   str(user.get("failed", "")),         sys.argv[4]),
]
fail = False
for field, actual, expected in checks:
    if expected and actual != expected:
        print(f"  FAIL run[{name}] {field}: expected={expected} actual={actual}")
        fail = True
sys.exit(1 if fail else 0)
' "$log" "$expected_state" "$expected_passed" "$expected_failed" "$name"
  # shellcheck disable=SC2181
  [[ $? -ne 0 ]] && ok=0 && FAIL=1
  [[ $ok -eq 1 ]] && echo "  OK   run[$name]"
}

check_diff() {
  local name=$1
  local a a_raw b b_raw expected_count tmp_json tmp_text

  a_raw=$(py_get "diff.${name}.a")
  b_raw=$(py_get "diff.${name}.b")
  a=$(resolve_path "$a_raw")
  b=$(resolve_path "$b_raw")
  expected_count=$(py_get "diff.${name}.divergence_count")

  if [[ -z "$a_raw" || -z "$b_raw" ]]; then echo "  SKIP diff[$name]: no a/b paths"; return; fi
  if [[ ! -f "$a" ]]; then echo "  FAIL diff[$name]: log not found: $a"; FAIL=1; return; fi
  if [[ ! -f "$b" ]]; then echo "  FAIL diff[$name]: log not found: $b"; FAIL=1; return; fi

  tmp_json=$(mktemp)
  tmp_text=$(mktemp)
  "$ACE" diff "$a" "$b" --format json --output "$tmp_json" --quiet 2>/dev/null || true
  "$ACE" diff "$a" "$b" > "$tmp_text" 2>/dev/null || true

  local ok=1
  python3 -c '
import sys, json

manifest   = json.loads(sys.argv[1])
name       = sys.argv[2]
exp_count  = sys.argv[3]
with open(sys.argv[4]) as f: diff = json.load(f)
with open(sys.argv[5]) as f: diff_text = f.read()
spec       = (manifest.get("diff") or {}).get(name, {})
fail       = False

actual_count = diff.get("summary", {}).get("divergences")
if exp_count and str(actual_count) != str(exp_count):
    print(f"  FAIL diff[{name}] divergence_count: expected={exp_count} actual={actual_count}")
    fail = True

actual_kinds = {d["kind"]["kind"] for d in diff.get("divergences", [])}
for k in (spec.get("kinds") or []):
    if k not in actual_kinds:
        print(f"  FAIL diff[{name}] kind missing: {k}")
        fail = True

for s in (spec.get("required_substrings") or []):
    if s not in diff_text:
        print(f"  FAIL diff[{name}] required_substring missing: {repr(s)}")
        fail = True

sys.exit(1 if fail else 0)
' "$MANIFEST_JSON" "$name" "$expected_count" "$tmp_json" "$tmp_text"
  local py_exit=$?
  rm -f "$tmp_json" "$tmp_text"
  [[ $py_exit -ne 0 ]] && ok=0 && FAIL=1
  [[ $ok -eq 1 ]] && echo "  OK   diff[$name]"
}

echo "check-demo: $EXPECTED_YAML  (CACHE=$CACHE)"
echo ""

run_names=$(python3 -c '
import sys, json
data = json.loads(sys.argv[1])
for k in (data.get("runs") or {}):
    print(k)
' "$MANIFEST_JSON")

diff_names=$(python3 -c '
import sys, json
data = json.loads(sys.argv[1])
for k in (data.get("diff") or {}):
    print(k)
' "$MANIFEST_JSON")

for name in $run_names; do check_run "$name"; done
for name in $diff_names; do check_diff "$name"; done

echo ""
if [[ $FAIL -eq 0 ]]; then echo "PASS"; exit 0
else echo "FAIL"; exit 1
fi
