# stripe-drift — real public API drift, real forked workflow path

> A public API shipped a breaking change between two versioned releases.
> A client written against the old shape kept working on staging.
> On prod — upgraded — the workflow took a different path. HTTP was still 200.
>
> This demo reproduces that class of incident end-to-end, on your laptop,
> using real published OpenAPI specs and a local mock. No real credentials.
> One command. `ace diff` names the field and the fork.

## The drift

Between Stripe OpenAPI commits [`7e9d4151`](https://github.com/stripe/openapi/tree/7e9d4151) (2022-11-15, tag `v353`) and [`25286160`](https://github.com/stripe/openapi/tree/25286160) (2026-04-22, tag `v2253`), the `Subscription` object changed:

| Field | 2022-11-15 | 2026-04-22 |
|---|---|---|
| `current_period_end` | required, on root | **removed from root** (moved onto items) |
| `discount` | nullable single | replaced by `discounts[]` |

A client written against the 2022 shape and pinned to staging keeps working.
Prod, upgraded to the newer Stripe spec, silently drops `current_period_end`.
HTTP is still 200. The server is still Stripe. The workflow routes to a
different terminal.

## One-command repro

```bash
ACE=$(pwd)/target/release/ace \
STRIPE_MOCK=/path/to/stripe-mock \
./run-demo.sh
```

The script:

1. Fetches both Stripe spec snapshots into `/tmp/ace-stripe-drift/`.
2. Spawns two `stripe-mock` instances — one per spec — on ports 12201/12202.
3. Runs the same [`scenario.yaml`](scenario.yaml) against each.
4. Runs `ace diff` on the two traces.

Requires `stripe-mock` (`brew install stripe/stripe-mock/stripe-mock` or
[pre-built binary](https://github.com/stripe/stripe-mock/releases)).

## What ACE shows

```
User 1 / step "fetch_subscription"
  ↯ routing diverged
      trace-a: matched edge d6932bc2 → apply_discount
      trace-b: matched edge 7e692f83 → upgrade_required
               rejected edge d6932bc2  [assertions failed: body.current_period_end (expected exists: true, got <missing>)]

User 1 / step "apply_discount"
  ✗ step absent in trace-b

2 divergence(s) across 2 step(s).
```

Read the output top-down:

- **The fork is on `fetch_subscription`.** Staging matched edge `d6932bc2`
  to `apply_discount`. Prod rejected the same edge, with the reason on the
  same screen: `body.current_period_end` missing from the response. The
  field Stripe removed in the upgrade. No cross-referencing traces, no
  schema diffing — the offending field is named.
- **The edge id is stable.** `d6932bc2` hashes the condition, not the
  source line. You can grep for it, track it in git, assert on it in CI.
- **`apply_discount` is a consequence, not a root cause.** It's absent in
  trace-b because the routing decision upstream chose a different edge.
  ACE reports it so you know the blast radius, but the first entry is the
  actual change.
- **Exit code is 1.** Wire this into CI against a last-known-good trace
  and a Stripe upgrade that breaks your workflow fails the build before
  it ships.

## Why this is harder than `env-diff`

`env-diff` controls both sides of the wire — we wrote the mock, we picked
the rename. This demo does not. The backend is `stripe-mock` driven by
Stripe's published OpenAPI schemas, pinned to two commits that are
~3.5 years apart. The drift (`current_period_end` moving off the root of
`Subscription`) is a **real shipped breaking change** from Stripe's own
API evolution, documented in their repo history. We didn't design it to
look catchable — we pointed ACE at it and it caught it.

If you want to see a different class of signal, change the assertion in
[`scenario.yaml`](scenario.yaml) to `discount: { exists: true }` instead.
Both traces fail that one (the 2022 spec returns `null` for `discount`,
which `exists: true` rejects), and the diff emits `rejection_reason_changed`
on the same edge — quieter output, but still correct: "same decision,
new cause." Either divergence kind is useful; which one you get depends
on how your workflow's default branches absorb failure.

## Files

- [`scenario.yaml`](scenario.yaml) — client workflow asserting the old `current_period_end` shape
- [`run-demo.sh`](run-demo.sh) — fetch specs, spawn two `stripe-mock`s, run, diff
- `/tmp/ace-stripe-drift/` — specs, logs, stripe-mock output (generated)

## Pinned spec SHAs

```
OLD_SHA=7e9d4151   # tag v353   — 2022-11-15
NEW_SHA=25286160   # tag v2253  — 2026-04-22
```

Override with `OLD_SHA=... NEW_SHA=... ./run-demo.sh` to diff other version
pairs from `stripe/openapi`.
