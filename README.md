# ACE — API Causality Engine

[![CI](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/ci.yml)
[![Release](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/release.yml/badge.svg)](https://github.com/Yatsuiii/api--causality-engine/releases)
[![Docker](https://img.shields.io/badge/ghcr.io-yatsuiii%2Face-blue?logo=docker)](https://github.com/Yatsuiii/api--causality-engine/pkgs/container/ace)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**Your CI says "assertion failed." It doesn't say which request, which step, or what the system state was when it broke.**

ACE fixes that. You describe your API workflow as a state graph — login, create, verify, retry — and ACE validates the graph structure before running it, then executes it step by step and tells you exactly where and why it failed.

```
error[E003]: step 'create_order' — status == 201, got 503
  --> workflow.yaml:31
  [login] --login--> [create_order] ✗ (503) 89ms
    ✗ status == 201 — expected: 201, got: 503
```

Not a Postman replacement. A workflow validation engine for multi-step API flows and CI/CD pipelines.

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
ace replay execution_log.json      # replay a previous run exactly
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

## Retry

For flaky endpoints, add a retry block directly on the step:

```yaml
steps:
  - name: login
    method: POST
    url: "{{base_url}}/auth"
    retry:
      attempts: 3
      delay_ms: 500
    assert:
      - status: 200
```

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

Run `ace mock scenario.yaml` to spin up a local HTTP server that responds to each step's endpoint with a canned response. Useful when you want to test a client without hitting a real API.

```bash
ace mock scenario.yaml --port 9000
```

## Postman import

If you have an existing Postman collection, you can convert it to ACE YAML and go from there:

```bash
ace import my-collection.json --output ./scenarios/
```

It won't handle every Postman feature, but it gets you a starting point instead of rewriting everything by hand.

## Desktop UI

There's also a native desktop app if you'd rather not deal with YAML directly. It's a React + Tauri application that calls the ACE engine in-process — no server, no Python, no extra runtime to install.

**Install:** grab the platform bundle from the [releases page](https://github.com/Yatsuiii/api--causality-engine/releases/latest) (`.dmg` for macOS, `.msi` for Windows, `.deb`/`.AppImage` for Linux).

**Dev mode (requires Rust + Node):**

```bash
cd ui/frontend
npm install

# in the project root
cargo tauri dev
```

You can create and edit scenarios visually, run them, browse history, and manage environments — all stored locally.

## Examples

The `examples/` directory has runnable scenarios:

- `auth/` — bearer token flow, login and profile fetch
- `branching/` — conditional transitions based on response
- `resilience/` — retry on failure, poll until ready
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
