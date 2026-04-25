#!/usr/bin/env bash
# Validates a demo run against its expected.yaml manifest.
# Usage: check-demo.sh <expected.yaml>
# Override ace binary: ACE=/path/to/ace
#
# Requires: python3, ace on PATH (or ACE= env).
# Exit 0 on match, exit 1 with details on mismatch.

set -euo pipefail

EXPECTED_YAML="${1:?expected.yaml path required}"
ACE="${ACE:-ace}"
FAIL=0

require() {
  command -v "$1" >/dev/null 2>&1 || { echo "error: $1 not found on PATH"; exit 1; }
}
require python3

# Convert expected.yaml to JSON once; all subsequent reads use the JSON.
MANIFEST_JSON=$(python3 - "$EXPECTED_YAML" <<'PYEOF'
"""
Minimal YAML->JSON converter for expected.yaml.
Handles: nested dicts (indent-based), string scalars, int scalars, lists (- item).
No anchors, no multiline blocks, no flow sequences beyond simple dash-lists.
"""
import sys, json, re

def parse(text):
    lines = text.splitlines()
    # strip comments and trailing whitespace
    lines = [re.sub(r'\s*#.*$', '', l).rstrip() for l in lines]

    def indent(line):
        return len(line) - len(line.lstrip())

    def scalar(v):
        v = v.strip()
        if v in ('true', 'True'):   return True
        if v in ('false', 'False'): return False
        if v == 'null':             return None
        try: return int(v)
        except ValueError: pass
        try: return float(v)
        except ValueError: pass
        return v.strip('"').strip("'")

    def parse_block(lines, base_indent):
        result = {}
        i = 0
        while i < len(lines):
            line = lines[i]
            if not line.strip():
                i += 1; continue
            ind = indent(line)
            if ind < base_indent:
                break
            if ind > base_indent:
                i += 1; continue
            # list item at this level?
            if line.lstrip().startswith('- '):
                # switch result to list mode
                lst = []
                while i < len(lines):
                    l = lines[i]
                    if not l.strip(): i += 1; continue
                    if indent(l) < base_indent: break
                    if l.lstrip().startswith('- '):
                        lst.append(scalar(l.lstrip()[2:]))
                        i += 1
                    else:
                        i += 1
                return lst
            m = re.match(r'^(\s*)([^:]+):\s*(.*)', line)
            if not m:
                i += 1; continue
            key = m.group(2).strip()
            val_inline = m.group(3).strip()
            # gather child lines
            child_lines = []
            j = i + 1
            while j < len(lines):
                cl = lines[j]
                if not cl.strip(): j += 1; child_lines.append(cl); continue
                if indent(cl) > base_indent:
                    child_lines.append(cl); j += 1
                else:
                    break
            if val_inline:
                result[key] = scalar(val_inline)
                i += 1
            else:
                result[key] = parse_block(child_lines, base_indent + 2)
                i = j
        return result

    return parse_block(lines, 0)

with open(sys.argv[1]) as f:
    text = f.read()
print(json.dumps(parse(text)))
PYEOF
)

# Get a scalar value by dotted path from the manifest JSON.
py_get() {
  python3 - "$MANIFEST_JSON" "$1" <<'PYEOF'
import sys, json
data = json.loads(sys.argv[1])
val = data
for k in sys.argv[2].split("."):
    if not isinstance(val, dict):
        val = None; break
    val = val.get(k)
print("" if val is None else val)
PYEOF
}

check_run() {
  local name=$1
  local log expected_state expected_passed expected_failed

  log=$(py_get "runs.${name}.log")
  expected_state=$(py_get "runs.${name}.terminal_state")
  expected_passed=$(py_get "runs.${name}.steps_passed")
  expected_failed=$(py_get "runs.${name}.steps_failed")

  if [[ -z "$log" ]]; then echo "  SKIP run[$name]: no log path"; return; fi
  if [[ ! -f "$log" ]]; then echo "  FAIL run[$name]: log not found: $log"; FAIL=1; return; fi

  local ok=1
  python3 - "$log" "$expected_state" "$expected_passed" "$expected_failed" "$name" <<'PYEOF'
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
PYEOF
  # shellcheck disable=SC2181
  [[ $? -ne 0 ]] && ok=0 && FAIL=1
  [[ $ok -eq 1 ]] && echo "  OK   run[$name]"
}

check_diff() {
  local name=$1
  local a b expected_count diff_json diff_text

  a=$(py_get "diff.${name}.a")
  b=$(py_get "diff.${name}.b")
  expected_count=$(py_get "diff.${name}.divergence_count")

  if [[ -z "$a" || -z "$b" ]]; then echo "  SKIP diff[$name]: no a/b paths"; return; fi
  if [[ ! -f "$a" ]]; then echo "  FAIL diff[$name]: log not found: $a"; FAIL=1; return; fi
  if [[ ! -f "$b" ]]; then echo "  FAIL diff[$name]: log not found: $b"; FAIL=1; return; fi

  local tmp_json tmp_text
  tmp_json=$(mktemp)
  tmp_text=$(mktemp)
  "$ACE" diff "$a" "$b" --format json > "$tmp_json" 2>/dev/null || true
  "$ACE" diff "$a" "$b" > "$tmp_text" 2>/dev/null || true

  local ok=1
  python3 - "$MANIFEST_JSON" "$name" "$expected_count" "$tmp_json" "$tmp_text" <<'PYEOF'
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
PYEOF
  local py_exit=$?
  rm -f "$tmp_json" "$tmp_text"
  [[ $py_exit -ne 0 ]] && ok=0 && FAIL=1
  [[ $ok -eq 1 ]] && echo "  OK   diff[$name]"
}

echo "check-demo: $EXPECTED_YAML"
echo ""

run_names=$(python3 - "$MANIFEST_JSON" <<'PYEOF'
import sys, json
data = json.loads(sys.argv[1])
for k in (data.get("runs") or {}):
    print(k)
PYEOF
)

diff_names=$(python3 - "$MANIFEST_JSON" <<'PYEOF'
import sys, json
data = json.loads(sys.argv[1])
for k in (data.get("diff") or {}):
    print(k)
PYEOF
)

for name in $run_names; do check_run "$name"; done
for name in $diff_names; do check_diff "$name"; done

echo ""
if [[ $FAIL -eq 0 ]]; then echo "PASS"; exit 0
else echo "FAIL"; exit 1
fi
