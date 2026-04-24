# env-diff — the prod-only bug `ace diff` was built for

> Monitoring shows 200s. CI is green. Conversion is down 23%.
> Nobody knows why prod is silently routing checkouts to the retry queue.

This example reproduces that incident end-to-end, on your laptop, with no real backend.

## The bug

A backend team renamed the field `status` → `state` on the checkout response in a
minor release. Staging wasn't redeployed; prod was. The client workflow asserts
`body.status: { exists: true }` before confirming the order — so in prod that
assertion silently fails and every checkout falls through to the `retry_queued`
edge. The HTTP status is still 200. Grafana shows nothing. Logs show nothing.
The rename was in a changelog nobody read.

## One-command repro

```bash
./run-demo.sh
```

The script spawns two local mock backends with the two schemas, runs
[`checkout.yaml`](checkout.yaml) against each, and diffs the traces. Traces
land in `/tmp/ace-env-diff/{staging,prod}.json`.

Requires `ace` on `$PATH`. If you're running from source: `ACE=target/release/ace ./run-demo.sh`.

## What you see

```
════════════════════════════════════════════════════════════════
 3/3  ace diff staging.json prod.json
════════════════════════════════════════════════════════════════
User 1 / step "checkout"
  ↯ routing diverged
      trace-a: matched edge 093b1848 → poll_status
      trace-b: matched edge 084ba606 → retry_queued
               rejected edge 093b1848  [assertions failed: body.status (expected exists: true, got <missing>)]

User 1 / step "poll_status"
  ✗ step absent in trace-b

User 1 / step "retry_queued"
  ✗ step absent in trace-a

3 divergence(s) across 4 step(s).
```

Four things the diff is telling you, in one screen:

1. **The fork is on `checkout`, and the cause is right there.** Staging matched
   the edge to `poll_status`; prod rejected that same edge because `body.status`
   is missing from the response. The rejection reason carries the offending
   assertion — description, expected, actual — so you do not have to
   cross-reference the trace to know what changed.
2. **The edge id is stable.** `093b1848` hashes the condition, not the source
   line. Grep for it, track it in git, open an issue against it, assert on it
   in CI — it survives edits to the YAML as long as the semantics don't change.
3. **The downstream is consequence, not cause.** `poll_status` is absent in prod
   and `retry_queued` is absent in staging because of the upstream routing
   divergence. ACE reports them so you know the scope, but the root cause is
   the first entry.
4. **The exit code is `1`.** Non-zero on any divergence. Wire `ace diff` into
   CI against a last-known-good trace and a broken deploy fails the build.

## What `curl` and `diffy` would show you

```
$ curl -s staging/orders/checkout | jq
{ "id": "...", "status": "paid" }

$ curl -s prod/orders/checkout | jq
{ "id": "...", "state": "paid" }
```

Two 200 responses, both containing the word `paid`, differing by one field name.
A human has to notice the rename, understand that the client's workflow keys off
`status`, and trace the consequence through the state graph. That is the
30-minute debugging session `ace diff` collapses into one command.

## Files

- [`checkout.yaml`](checkout.yaml) — the client workflow under test
- [`servers/backend-old.yaml`](servers/backend-old.yaml) — staging backend, emits `status`
- [`servers/backend-new.yaml`](servers/backend-new.yaml) — prod backend, emits `state` (renamed)
- [`run-demo.sh`](run-demo.sh) — one-command orchestration

## Exit codes

| Code | Meaning |
|---|---|
| `0` | no divergences |
| `1` | divergences found |
| `2` | bad args, unreadable log, etc. |

## Going further

- `ace diff staging.json prod.json --format json -o divergences.json` for
  programmatic consumption. Each divergence carries a `kind` discriminator
  (`routing_diverged`, `rejection_reason_changed`, `edge_only_in_a`,
  `edge_only_in_b`, `outcome_diverged`, `step_missing_in_a`, `step_missing_in_b`)
  so downstream tooling can filter without string parsing.
- Commit a known-good trace to your repo. Re-run on every deploy and diff
  against it. Any unexplained routing change now fails CI.
- Change [`backend-new.yaml`](servers/backend-new.yaml) to return 503 instead of
  renaming the field — you'll see the status-code branch fire instead of the
  default fallthrough. Different divergence kind, same one-command repro.
