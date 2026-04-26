# ACE Improvement Plan

Design sketches for closing the gap between "demo works" and "prod works".

Context: the Stripe-drift demo shipped with a broken assertion (`current_period_end`, a field stripe-mock doesn't return in either spec). Nobody ran it end-to-end before merge. Fixing that one bug exposed deeper structural problems: assertions are *predictive* (you only catch what you thought to look for), dynamic response data looks like drift, and we have no CI guard on our own examples.

## Thesis

ACE detects drift *you predicted*. To be prod-useful it has to flip: detect drift *you didn't predict* while suppressing noise you don't care about. That requires three shifts — schema-level assertions (vs field-by-field), dynamic-field masking (vs raw diff), and self-validation of our own demos (vs "trust the author").

## Tiers

### P0 — This week. Non-negotiable before anyone points this at real prod.

| # | Sketch | Effort | Status |
|---|---|---|---|
| 0.1 | [CI for every demo](p0-1-ci-for-examples.md) | 1 day | Done — PyYAML-based `check-demo.sh`, relative paths via `CACHE`, red-test in `check-demo.test.sh`, expanded triggers |
| 0.2 | [Schema-based assertions](p0-2-schema-assertions.md) | 3-4 days | Done — OpenAPI component refs, strict mode, default feature; cache key includes `strict`; inline cache canonicalized |
| 0.3 | [Dynamic-field masking at diff time](p0-3-dynamic-field-masking.md) | 2-3 days | Done — body + header masking wired end-to-end; `ace diff` emits `BodyDiverged`/`HeadersDiverged` from normalized data so masks actually suppress noise (not just label it); raw body retained when masking is on; mask paths validated at scenario-load (E021) |
| 0.4 | [Output clarity](p0-4-output-clarity.md) | 3-5 days | Done — five-glyph legend (`✓`/`✗`/`↯`/`⊘`/`·`), grouped step renderer, `--format markdown/json`, `ACE_SUMMARY` stdout line, `--quiet`, `--show-masked`, scenario name/path on logs, `ace show` header; glyph lint test enforces no vocabulary drift |

### P1 — Next 2 months. Adoption path.

| # | Sketch | Effort | Status |
|---|---|---|---|
| 1.4 | [`ace import har` / `ace import openapi`](p1-4-import-har-openapi.md) | 5-7 days | Partial (Postman only today) |
| 1.5 | [Semantic drift kinds](p1-5-semantic-drift-kinds.md) | 4-5 days | Not started; depends on P0.3 |
| 1.6 | [`ace lint`](p1-6-ace-lint.md) | 4-6 days | Not started |
| 1.7 | [Run aggregation + distribution diff](p1-7-run-aggregation.md) | 4-5 days | Not started |
| 1.8 | [GitHub Action + PR comment](p1-8-github-action.md) | 2-3 days | Not started |
| 1.9 | [Output sinks (Slack/webhook/Datadog)](p1-9-output-sinks.md) | 5-7 days | Partial (stdout/file/junit exist) |

### P2 — 3-6 months. Expansion.

| # | Sketch | Effort | Status |
|---|---|---|---|
| 2.10 | [Replay from production traces](p2-10-replay-from-prod.md) | 2-3 weeks | Not started |
| 2.11 | [Drift severity model](p2-11-drift-severity.md) | 3-4 days | Not started |
| 2.12 | [Canary analysis mode](p2-12-canary-analysis.md) | 1-2 weeks | Not started; depends on 1.7 + 2.11 |
| 2.13 | [Schema generation from runs](p2-13-schema-generation.md) | 1 week | Not started |

### Architectural direction (not a deliverable)

| Doc |
|---|
| [Contract + baseline model](architecture-contract-baseline.md) |

## What we explicitly say NO to

- GUI / web dashboard — useless until the CLI is solid
- Spec-only drift (oasdiff already does static OpenAPI diffing)
- Distributed / multi-node execution
- LLM-generated remediation suggestions — brittle on real APIs; revisit once detection is reliable

## Week-one execution order

1. P0.1 (CI for examples) — prevents the next `current_period_end`-style regression ✓
2. P0.2 (schema assertions, OpenAPI ref support) — the feature that makes assertions tractable on real APIs ✓
3. P0.3 (dynamic-field masking) — unblocks P1.5 and any real-prod pilot ✓
4. P0.4 (output clarity) — turns the working machine into something a teammate will read

Ship those four, then the stripe-drift demo is defensibly real: "point ACE at your OpenAPI + two environments, get a one-line verdict and a clean diff." Until then the rest is premature.

## Carryover debt from P0.1-P0.3

All closed:

- ~~**P0.2 schema cache key omits `strict`**~~ — fixed in `assertions.rs`; cache key is now `openapi:{path}:{component}:{strict|lax}`. Regression test `openapi_strict_and_lax_share_no_cache_entry` proves it.
- ~~**P0.2 cyclic `$ref` schemas fail to compile**~~ — `compile_schema` was discarding the OpenAPI root doc returned by `resolve()`, so cycle-preserved refs (e.g. `Subscription.latest_invoice.subscription`) had no document to resolve against. Fixed by rewriting preserved cycle refs to a synthetic URI (`urn:ace:openapi-root#/...`) and registering the root doc via `JSONSchema::options().with_document(...)`. End-to-end regression: `cyclic_openapi_component_compiles_and_validates_deep_ref` validates a `Node`-shaped schema two levels deep and rejects a deep type violation.
- ~~**P0.3 header masking is dead config**~~ — `StepLogBuilder::build` now calls `mask::normalize_headers_tracked` when any `header:` rule exists; `StepLog` has `response_headers`/`response_headers_normalized`/`masked_headers`.
- ~~**P0.3 raw body dropped at non-verbose runs even when masking is on**~~ — runner now retains raw body whenever `mask:` is non-empty so a future `--show-masked` works without re-running.
- ~~**P0.3 masking computed but `ace diff` never consumed it**~~ — `diff_step` previously only labeled divergences with masked-field names; it didn't compare bodies/headers at all, so masking was decorative. Added `BodyDiverged` and `HeadersDiverged` divergence kinds that compare `response_body_normalized` / `response_headers_normalized` (with `--mask-extra` applied on top), with a routing-divergence guard so we don't double-report. Suppression is proven by `body_diff_suppressed_when_only_masked_field_differs` — a body that differs only in a masked field produces zero divergences.
- ~~**P0.1 hand-rolled YAML parser in `scripts/check-demo.sh`**~~ — replaced with `python3 -c 'import yaml'`; checker also accepts a `CACHE` env var so `expected.yaml` paths can be relative.

Other improvements landed alongside:

- **Validator E021** — rejects unsupported JSONPath in `mask:` rules at scenario-load time (the engine parser silently ignored `$.foo[0]`, filters, etc.). Supported subset documented in `crates/engine/src/mask.rs` module doc and `examples/masks/common.yaml`.
- **`scripts/check-demo.test.sh`** — self-test for the demo checker. Builds throwaway fixtures and a stub `ace` binary, asserts the checker exits 0/1 as appropriate. Wired into `demos.yml` so a regression in the checker fails CI before the real Stripe demo even runs.
- **`docs/assertions.md`** — documents the three `schema:` forms, cache-key semantics, and known limitations (OpenAPI 3.0/3.1 draft selection, cyclic `$ref`).
- **demos.yml fix** — `cargo build --release -p cli` was wrong (package is named `ace`); now builds. Path triggers extended to include `Cargo.toml`/`Cargo.lock` so dependency-only changes still validate the demo.
