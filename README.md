# ACE — API Causality Engine

[![CI](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/ci.yml)
[![Release](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/release.yml/badge.svg)](https://github.com/Yatsuiii/api--causality-engine/releases)
[![Docker](https://img.shields.io/badge/ghcr.io-yatsuiii%2Face-blue?logo=docker)](https://github.com/Yatsuiii/api--causality-engine/pkgs/container/ace)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

API testing tool for workflows that span multiple requests. You write a YAML file describing the steps, what to assert, what to extract from each response, and how to move between states. ACE runs it.

No cloud account. No collections syncing to someone's server. Just a binary and a YAML file — or a desktop UI if you prefer clicking around.

## Why

I kept reaching for Postman to test multi-step flows and it kept falling apart. Login, extract the token, use it in the next request, assert something about the response — that's not a collection, that's a workflow, and Postman treats it like an afterthought.

So I built this. It's probably missing features you want. Open an issue.

The core idea: steps are states in a graph. You define where each step goes on success (or failure), and ACE validates the whole graph before running a single request. No more "oh it failed on step 4 because step 2 silently skipped the extract".

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
ace validate scenario.yaml         # catch errors without running
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

Steps don't have to go in a straight line. Use `transitions` (plural) with `when:` conditions to branch based on what the response actually looks like:

```yaml
steps:
  - name: login
    method: POST
    url: "{{base_url}}/auth"
    body: { username: admin, password: "{{$env.PASS}}" }
    assert:
      - status: 200
    extract:
      token: token
    transitions:
      - to: dashboard
        when:
          assertions: passed
      - to: login_failed
        default: true

  - name: login_failed
    # handle it however you want, then go to error or retry
    transitions:
      - to: error
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
    transitions:
      - to: done
        when:
          assertions: passed
      - to: wait_and_retry
        default: true

  - name: wait and retry
    state: wait_and_retry
    pre_request:
      - delay_ms: 500
    transitions:
      - to: check_status
        default: true
```

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

## License

MIT
