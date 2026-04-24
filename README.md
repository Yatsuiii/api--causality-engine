# ACE — API Causality Engine

[![CI](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/ci.yml)
[![Release](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/release.yml/badge.svg)](https://github.com/Yatsuiii/api--causality-engine/releases)
[![Docker](https://img.shields.io/badge/ghcr.io-yatsuiii%2Face-blue?logo=docker)](https://github.com/Yatsuiii/api--causality-engine/pkgs/container/ace)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**ACE tells you why the same API workflow took a different path across environments.**

Model your flow as a state machine, run it against staging and prod, diff the traces. One command shows which edge matched in staging, which was rejected in prod, and what the response said when it rejected:

```
$ ace run scenario.yaml --var base_url=https://staging.api.com -o staging.json
$ ace run scenario.yaml --var base_url=https://prod.api.com    -o prod.json
$ ace diff staging.json prod.json

User 1 / step "checkout"
  ↯ routing diverged
      trace-a: matched edge a3f2b1c4 → paid
      trace-b: matched edge 7d81e920 → retry_queued
               rejected edge a3f2b1c4  [assertions failed: body.status (expected exists: true, got <missing>)]

User 1 / step "poll_status"
  ⚠ different rejection reason on edge b2c8f019
      trace-a: body .state: expected "ok", got "pending"
      trace-b: body .state: expected "ok", got "failed"

2 divergence(s) across 5 step(s).
```

That is the gap between staging and prod in one screen — not "something is broken" but "the checkout edge that routes to `paid` is being rejected in prod because `body.status` is missing, and the poll_status edge is seeing a different body value." Runnable example in [`examples/env-diff/`](examples/env-diff/).

Not a Postman replacement. A workflow-testing CLI for multi-step API flows and CI/CD pipelines.

## Why not diffy / OpenTelemetry / Pact?

Prior art that solves adjacent problems:

- **[diffy](https://github.com/twitter-archive/diffy)** shadow-traffics a request to two services and diffs the *responses*. Byte-level diff, no workflow model. ACE diffs *routing decisions* in a state graph — it tells you which edge matched and why, not just that the JSON differs.
- **OpenTelemetry + Jaeger/Tempo** diff production spans across deploys. Requires traces to exist and agents to be deployed. ACE runs locally or in CI against any HTTP API — zero instrumentation on the target.
- **Pact / contract tests** catch divergence at build time by pinning request/response shapes. They don't cover multi-step workflows where the interesting bug is which *path* the flow took.
- **`diff <(curl a) <(curl b)`** is free and fine for one request. Falls apart the moment login tokens, extracted IDs, or conditional branching enter the picture.

ACE's narrow claim: *diff the decisions a workflow made, not the bytes it returned.*

## Why

Standard API tools test one request at a time. Production failures happen across request chains — the token extracted in step 1 is invalid by step 3, or a 202 in step 2 means you need to poll before step 4 can succeed. You can't catch that with isolated tests.

ACE models the whole workflow as a state machine:

- Every step is a state with explicit outgoing transitions
- ACE validates the graph (dead ends, missing states, undefined variables) before it runs anything
- Execution follows the graph; the trace shows every transition, assertion result, and extracted value
- When something fails, you see the failure cause and where in the workflow it happened — not just a generic error code

## Before / after

**Typical CI output:**
```
FAIL
AssertionError: expected 201 but got 503
    at Object.<anonymous> (tests/api.test.js:47:5)
```

You know something failed. You don't know which user session, which prior request set up the broken state, or what was extracted along the way.

**ACE output for the same failure:**
```
  [User 1] [login] --login--> [create_order] ✗ (503) 89ms
    ✗ status == 201 — expected: 201, got: 503

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  User 1: FAILED — State 'create_order': no matching transition for status 503

  FAIL
```

The transition, the HTTP status, and the assertion that failed are all in one line. The prior step (login → create_order) tells you the system state when it broke.

## Install

**Homebrew (macOS / Linux):**
```bash
brew tap yatsuiii/tap
brew install ace
```

**One-liner (Linux / macOS):**
```bash
curl -fsSL https://raw.githubusercontent.com/Yatsuiii/api--causality-engine/main/install.sh | sh
```

**Docker:**
```bash
docker run --rm -v $(pwd):/scenarios ghcr.io/yatsuiii/ace run scenario.yaml
```

**Manual download:**
Pre-built binaries for Linux (x86_64, aarch64), macOS (x86_64, Apple Silicon), and Windows on the [releases page](https://github.com/Yatsuiii/api--causality-engine/releases/latest).

**From source (requires Rust):**
```bash
cargo install --git https://github.com/Yatsuiii/api--causality-engine ace
```

## Usage

```bash
ace init                           # scaffold a new scenario
ace run scenario.yaml              # run it
ace run scenario.yaml -v           # show request/response bodies
ace run scenario.yaml --env .env --var base_url=https://staging.api.com
ace run scenario.yaml --junit report.xml   # JUnit output for CI
ace validate scenario.yaml         # catch graph/variable errors without running
ace validate scenario.yaml --graph # print resolved state graph
ace show execution_log.json        # re-render a recorded run (no re-execution)
ace diff staging.json prod.json    # diff two execution logs — show routing divergences
ace report execution_log.json      # convert a run log to JSON or JUnit
ace import collection.json         # convert a Postman collection to ACE YAML
ace mock scenario.yaml             # spin up a mock server from a scenario
ace docs scenario.yaml             # generate API docs from a scenario
```

## Scenario

```yaml
name: user lifecycle
initial_state: register

variables:
  base_url: https://api.example.com

steps:
  - name: register
    method: POST
    url: "{{base_url}}/users"
    body:
      email: "test@example.com"
      password: "hunter2"
    assert:
      - status: 201
      - body:
          id: { exists: true }
    extract:
      user_id: id
    transition:
      from: register
      to: login

  - name: login
    method: POST
    url: "{{base_url}}/auth/login"
    body:
      email: "test@example.com"
      password: "hunter2"
    assert:
      - status: 200
      - body:
          token: { exists: true }
    extract:
      token: token
    transition:
      from: login
      to: fetch_profile

  - name: fetch_profile
    method: GET
    url: "{{base_url}}/users/{{user_id}}"
    headers:
      Authorization: "Bearer {{token}}"
    assert:
      - status: 200
      - body:
          email: { eq: "test@example.com" }
      - response_time_ms: { lt: 500 }
    transition:
      from: fetch_profile
      to: done
```

Run it against 5 concurrent users: add `concurrency: 5` at the top.

## Assertions

```yaml
assert:
  - status: 201
  - body:
      id: { exists: true }
      role: { eq: "admin" }
      score: { gt: 0, lt: 100 }
      status: { ne: "banned" }
      plan: { in: ["free", "pro"] }
      bio: { contains: "engineer" }
  - header:
      content-type: { contains: "application/json" }
  - response_time_ms: { lt: 1000 }
```

`response_time_ms` is measured end-to-end — from the moment the request is sent to the moment the full response body has been read. Slow-drip servers that flush headers fast but dribble the body cannot hide behind TTFB-only timing.

### JSONSchema validation

For responses where the full shape matters — not just a few fields — point an assertion at a JSONSchema instead of re-writing the shape in the ACE DSL. The schema can be inline or a file path (resolved relative to the scenario file):

```yaml
assert:
  - status: 200
  - schema: ./schemas/user.json        # file reference (JSON or YAML)

# ...or inline when the schema is small:
  - schema:
      type: object
      required: [id, email]
      properties:
        id: { type: integer }
        email: { type: string, format: email }
```

Validation errors include the offending JSON path (e.g. `/email`) so you can tell *which* field broke the contract, not just that something did. Schema assertions compose with the existing `body:` checks — use schema for structure, `body:` for specific values you care about.

## Variables

| Pattern | What it resolves to |
|---------|---------------------|
| `{{name}}` | value from `variables:` or extracted from a previous response |
| `{{$env.KEY}}` | environment variable (or from `--env .env`) |
| `{{$uuid}}` | random UUID v4 |
| `{{$timestamp}}` | unix timestamp |
| `{{$randomInt}}` | random integer |

## Auth

Declared once at the scenario level, applied to every step:

```yaml
auth:
  bearer: "{{$env.API_TOKEN}}"
  # or: basic: { username: admin, password: "{{$env.PASS}}" }
  # or: api_key: { header: X-API-Key, value: "{{$env.KEY}}" }
```

## Branching

Steps don't have to go in a straight line. Define explicit top-level `edges` with `when:` conditions to branch based on what the response actually looks like:

```yaml
name: login flow
initial_state: login

steps:
  - name: login
    state: login
    method: POST
    url: "{{base_url}}/auth"
    body: { username: admin, password: "{{$env.PASS}}" }
    assert:
      - status: 200
    extract:
      token: token

  - name: login_failed
    state: login_failed
    # handle it however you want, then go to error or retry

  - name: dashboard
    state: dashboard
    method: GET
    url: "{{base_url}}/me"

edges:
  - from: login
    to: dashboard
    when:
      assertions: passed
  - from: login
    to: login_failed
    default: true
  - from: login_failed
    to: error
    default: true
  - from: dashboard
    to: done
    default: true
```

Polling loops work the same way — just transition back to an earlier step:

```yaml
name: wait for job
initial_state: check_status
max_iterations: 10

steps:
  - name: check status
    state: check_status
    method: GET
    url: "{{base_url}}/jobs/{{job_id}}"

  - name: wait and retry
    state: wait_and_retry
    pre_request:
      - delay_ms: 500

edges:
  - from: check_status
    to: done
    when:
      assertions: passed
  - from: check_status
    to: wait_and_retry
    default: true
  - from: wait_and_retry
    to: check_status
    default: true
```

## Execution model

**Sequential within a branch.** Each concurrency slot runs one step at a time, advancing through the graph by following edges. There is no parallelism within a single branch — `step B` only executes after `step A` completes and its transition is resolved.

**Parallel across branches.** `--concurrency N` (or `-c N`) spawns N independent state machines, each with its own variable context. Variables extracted in one branch are invisible to others. This models N simultaneous users running the same workflow.

```
ace run scenario.yaml -c 10   # 10 users in parallel
```

**What happens on failure:**

| Failure type | Behaviour |
|---|---|
| Network / timeout | Step is marked as an engine error. That branch stops immediately. Other branches continue. Exit code 2. |
| Assertion failed | Recorded in the log. The transition still fires using edge conditions — an `assertions: failed` edge can route to a retry or error state. Exit code 1 if any branch ends with failures. |
| No matching transition | Branch stops with `NoMatchingTransition`. Prevent this by always including a `default: true` edge from every state. |
| `max_iterations` exceeded | Branch stops. Default limit is 100; set `max_iterations:` in the scenario to change it. |

**Skipped steps.** A step is skipped when a `pre_request` hook's `skip_if:` resolves to `"true"`. ACE follows the `default` edge from that state and continues rather than stopping.

**Variable scope.** Each branch starts with a fresh copy of the initial context (scenario `variables:` + CLI `--var` overrides). Extraction results and hook `set:` values are branch-local — mutations in one branch never bleed into another.

## Parallel fan-out

Some steps are genuinely independent — loading a dashboard means fetching profile + posts + todos simultaneously, not one after another. Declare a `parallel:` edge to run branches concurrently and rejoin at a named state:

```yaml
edges:
  - from: login
    parallel:
      branches:
        - { name: profile, to: fetch_profile }
        - { name: posts,   to: fetch_posts }
        - { name: todos,   to: fetch_todos }
      join: render
      on_failure: fail_fast   # or all_complete
```

Each branch runs in its own context. On success, extracted values are merged under the branch name — `{{profile.username}}`, `{{posts.0.title}}`. Sibling branches can't see each other's variables.

**`fail_fast`** surfaces the first branch error immediately and discards partial work. **`all_complete`** waits for every branch to finish, merges what succeeded, then reports errors. Pick based on whether a partial dashboard is useful or misleading.

Nested fan-out is rejected by the validator (error E015). Branch targets, join targets, unknown names, and scope collisions are all caught before anything runs. See `examples/fanout/dashboard-load.yaml` for a runnable version.

## Weighted routing

For load-distribution scenarios — canaries, A/B traffic splits, chaos injection — attach `weight:` to multiple edges from the same state:

```yaml
edges:
  - { from: pick_backend, to: stable, weight: 90, tag: stable-v1 }
  - { from: pick_backend, to: canary, weight: 10, tag: canary-v2 }
```

Within the highest-priority tier of matching edges, ACE samples by cumulative distribution. All edges in a weighted group must declare a weight — mixing weighted and unweighted is rejected (E010).

Runs are deterministic per `--seed`:

```bash
ace run scenario.yaml --seed 42    # same seed → same routing every time
```

The seed is echoed in `execution_log.json` so you can re-run with `--seed <value>` and hit the same routing decisions. See `examples/weighted/canary-rollout.yaml`.

## Retry

For flaky endpoints, add a retry block directly on the step:

```yaml
steps:
  - name: fetch_order
    method: GET
    url: "{{base_url}}/orders/{{order_id}}"
    retry:
      attempts: 5
      delay_ms: 200           # initial delay
      backoff: exponential    # or: fixed (default)
      multiplier: 2.0         # each attempt: delay_ms * multiplier ^ (n-1)
      max_delay_ms: 5000      # cap for any single wait
      jitter: full            # none (default) | full | equal
      retry_on: [502, 503]    # optional override; see default below
    assert:
      - status: 200
```

| Field | Default | Meaning |
|---|---|---|
| `attempts` | 3 | Max total tries including the first. |
| `delay_ms` | 1000 | Wait before the first retry. Also the per-retry wait for `fixed`. |
| `backoff` | `fixed` | `fixed` holds `delay_ms` constant. `exponential` multiplies each retry. |
| `multiplier` | 2.0 | Growth factor for `exponential`. Ignored for `fixed`. |
| `max_delay_ms` | 30000 | Upper bound on any single wait, even if exponential would exceed it. |
| `jitter` | `none` | `full` picks uniformly in `[0, delay]`. `equal` picks in `[delay/2, delay]`. |
| `retry_on` | `[408, 429, 500, 501, 502, 503, 504]` | Status codes that trigger a retry. Empty list means use this default. |

**Behavior change:** earlier ACE versions retried on any 4xx/5xx. The current default only retries timeout-adjacent (408, 429) and server errors (5xx) — retrying a 401 or 404 won't make it succeed. If you need the old behavior for a specific step, set `retry_on` explicitly. Transport errors (connection refused, timeouts) always retry regardless.

Jitter uses a thread-local RNG, so retry timing is not reproducible across runs, even with the same `--seed`.

## CI

```bash
ace run tests/smoke.yaml --junit results.xml -q
```

Exit codes: `0` = all passed, `1` = assertions failed, `2` = error (bad YAML, network, etc.)

JUnit output works with GitHub Actions, Jenkins, GitLab CI, and anything else that reads JUnit XML.

```yaml
- name: API tests
  run: ace run tests/smoke.yaml --junit results.xml -q
- uses: dorny/test-reporter@v1
  if: always()
  with:
    name: API Tests
    path: results.xml
    reporter: java-junit
```

## Mock server

Run `ace mock scenario.yaml` to spin up a local HTTP server that stubs each step's endpoint. The response body is a JSON stub shaped from the step's `extract:` keys (e.g. `extract: { user_id: id }` yields `{"id": "mock_user_id"}`); steps without `extract` return `{"ok": true}`. The status code is taken from the step's `assert: - status: N` if present, otherwise `200`.

```bash
ace mock scenario.yaml --port 9000
```

**Caveats:** this is a smoke-test stub, not a full mock framework — there's no request matching on body or headers, no stateful responses, and if two steps share a `METHOD path` only the first is served (later ones are logged as unreachable and skipped).

## Postman import

If you have an existing Postman collection, you can convert it to ACE YAML and go from there:

```bash
ace import my-collection.json --output ./scenarios/
```

It won't handle every Postman feature, but it gets you a starting point instead of rewriting everything by hand.

## Examples

The `examples/` directory has runnable scenarios:

- `env-diff/` — **staging vs prod divergence** — the headline use case for `ace diff`
- `auth/` — bearer token flow, login and profile fetch
- `branching/` — conditional transitions based on response
- `fanout/` — parallel branches that rejoin at a named state
- `resilience/` — retry on failure, poll until ready
- `weighted/` — canary rollout with seeded load split
- `workflows/` — CRUD lifecycle, first run scaffold

### login-create-retry — a real workflow

`examples/workflows/login-create-retry.yaml` models a common production pattern: authenticate, create a resource, retry if the server isn't ready, then verify.

```yaml
name: login create resource with retry
initial_state: login
max_iterations: 8
terminal_states: [done, error]

variables:
  base_url: https://api.example.com
  username: demo-user
  password: "{{$env.DEMO_PASSWORD}}"

steps:
  - name: login
    state: login
    method: POST
    url: "{{base_url}}/auth/login"
    body: { username: "{{username}}", password: "{{password}}" }
    assert:
      - status: 200
      - body: { token: { exists: true } }
    extract:
      token: token

  - name: create_resource
    state: create_resource
    method: POST
    url: "{{base_url}}/resources"
    headers: { Authorization: "Bearer {{token}}" }
    body: { name: "order-{{$timestamp}}", type: "demo" }
    assert:
      - status: { in: [201, 202] }
      - body: { id: { exists: true } }
    extract:
      resource_id: id

  # ... create_retry_wait, verify_resource, verify_retry_wait, auth_failed

edges:
  - { from: login,        to: create_resource,    when: { assertions: passed } }
  - { from: login,        to: auth_failed,         default: true }
  - { from: create_resource, to: verify_resource,  when: { assertions: passed } }
  - { from: create_resource, to: create_retry_wait, default: true }
  - { from: create_retry_wait, to: create_resource, default: true }
  - { from: verify_resource, to: done,             when: { assertions: passed } }
  - { from: verify_resource, to: verify_retry_wait, default: true }
  - { from: verify_retry_wait, to: verify_resource, default: true }
  - { from: auth_failed,  to: error,               default: true }
```

Validate before running:

```
$ ace validate examples/workflows/login-create-retry.yaml --graph

Validation Report
Scenario: login create resource with retry | Steps: 5 | Concurrency: 1

State Graph
  initial_state: login
  mode: graph
  [login] --(assertions)--> [create_resource]
  [login] --(default)--> [auth_failed]
  [create_resource] --(assertions)--> [verify_resource]
  [create_resource] --(default)--> [create_retry_wait]
  [create_retry_wait] --(default)--> [create_resource]
  [verify_resource] --(assertions)--> [done]
  [verify_resource] --(default)--> [verify_retry_wait]
  [verify_retry_wait] --(default)--> [verify_resource]
  [auth_failed] --(default)--> [error]

Static Checks
  ✓ no validation issues found
```

Run it (happy path — server returns 201 and resource is immediately ready):

```
$ DEMO_PASSWORD=secret ace run examples/workflows/login-create-retry.yaml

Scenario: login create resource with retry
Running: 1 user(s) × 5 step(s)

  [User 1] [login] --login--> [create_resource] ✓ (200) 138ms
    ✓ status == 200
    ✓ body.token exists
  [User 1] [create_resource] --create_resource--> [verify_resource] ✓ (201) 92ms
    ✓ status in [201, 202]
    ✓ body.id exists
  [User 1] [verify_resource] --verify_resource--> [done] ✓ (200) 61ms
    ✓ status == 200
    ✓ body.id exists
    ✓ body.status in ["ready", "active"]

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  User 1: Final state: done (3 steps, 291ms)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Steps: 3 total, 3 passed, 0 failed
  Timing: total 291ms | avg 97ms | p50 92ms | p95 138ms | p99 138ms

  PASS

  Log: execution_log.json
```

If the resource needs time to become ready, ACE automatically loops `verify_resource → verify_retry_wait → verify_resource` until assertions pass or `max_iterations` is hit — no polling logic to write.

## License

MIT
