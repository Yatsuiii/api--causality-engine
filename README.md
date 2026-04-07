# ACE — API Causality Engine

[![CI](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/ci.yml)
[![Release](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/release.yml/badge.svg)](https://github.com/Yatsuiii/api--causality-engine/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A production-grade, stateful API workflow testing engine built in Rust. Define API test scenarios in YAML with state machine transitions, run them with concurrent users, assert on responses, and get detailed reports.

**Think Postman Collections meets state machines — but faster, deterministic, and CLI-native.**

## Install

### Pre-built binaries (recommended)

Download the latest binary for your platform from the [Releases page](https://github.com/Yatsuiii/api--causality-engine/releases/latest):

| Platform | File |
|----------|------|
| Linux x86_64 | `ace-linux-x86_64.tar.gz` |
| Linux aarch64 | `ace-linux-aarch64.tar.gz` |
| macOS x86_64 | `ace-macos-x86_64.tar.gz` |
| macOS Apple Silicon | `ace-macos-aarch64.tar.gz` |
| Windows x86_64 | `ace-windows-x86_64.zip` |

```bash
# Linux / macOS — one-liner
curl -L https://github.com/Yatsuiii/api--causality-engine/releases/latest/download/ace-linux-x86_64.tar.gz | tar xz
sudo mv ace /usr/local/bin/
```

### Build from source

Requires [Rust](https://rustup.rs):

```bash
cargo install --git https://github.com/Yatsuiii/api--causality-engine ace
```

## Features

- **Stateful workflows** — State machine-driven step transitions with pre-flight validation
- **Full HTTP support** — GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS with headers and JSON bodies
- **Assertions** — Status codes, body (JSONPath), headers, response time checks
- **Authentication** — Bearer, Basic, API Key — scenario-level with variable substitution
- **Dynamic extraction** — Extract values from responses using dot-notation paths (`data.user.id`)
- **Variable substitution** — `{{var}}`, `{{$env.KEY}}`, `{{$uuid}}`, `{{$timestamp}}`, `{{$randomInt}}`
- **Concurrent execution** — Multi-user simulation with independent state per task
- **Retry logic** — Configurable attempts and delay per step
- **Environment files** — `.env` file loading with CLI overrides (`--var key=value`)
- **Deterministic replay** — Replay past executions from JSON logs
- **Reporting** — Colored console output, JSON logs, JUnit XML for CI/CD
- **Pre-flight validation** — Catch state machine errors before execution
- **Performance metrics** — avg / p50 / p95 / p99 response times

## Quick Start

```bash
# Scaffold a new scenario
ace init

# Run it
ace run ace.yaml

# Verbose output (shows request/response bodies)
ace run ace.yaml -v

# Validate without running
ace validate ace.yaml

# Run with env file and variable overrides
ace run ace.yaml --env .env --var base_url=https://staging.api.com

# Generate JUnit XML report (for CI/CD)
ace run ace.yaml --junit report.xml

# Replay a previous execution
ace replay execution_log.json

# Generate report from log
ace report execution_log.json --format junit -o report.xml
```

## Scenario Format

```yaml
name: user CRUD workflow
initial_state: start
concurrency: 3
auth:
  bearer: "{{$env.API_TOKEN}}"
variables:
  base_url: https://api.example.com

steps:
  - name: create user
    method: POST
    url: "{{base_url}}/users"
    headers:
      Content-Type: application/json
      X-Request-Id: "{{$uuid}}"
    body:
      name: "Test User"
      email: "test@example.com"
    timeout_ms: 5000
    assert:
      - status: 201
      - body:
          id:
            exists: true
          name:
            eq: "Test User"
      - header:
          content-type:
            contains: "json"
      - response_time_ms:
          lt: 2000
    extract:
      user_id: "id"
    retry:
      attempts: 3
      delay_ms: 500
    transition:
      from: start
      to: created

  - name: get user
    method: GET
    url: "{{base_url}}/users/{{user_id}}"
    assert:
      - status: 200
    transition:
      from: created
      to: done
```

## Assertion Types

| Type | Example | Description |
|------|---------|-------------|
| Status code | `status: 200` | Exact match |
| Body exists | `body: { id: { exists: true } }` | Field exists |
| Body equals | `body: { name: { eq: "Alice" } }` | Exact value match |
| Body contains | `body: { bio: { contains: "engineer" } }` | Substring match |
| Header check | `header: { content-type: { contains: "json" } }` | Header value check |
| Response time | `response_time_ms: { lt: 2000 }` | Performance threshold |
| Numeric range | `body: { age: { gt: 18, lt: 100 } }` | Range check |
| Not equal | `body: { status: { ne: "error" } }` | Negative check |
| In list | `status: { in: [200, 201] }` | Value in set |

## Authentication

```yaml
# Bearer token
auth:
  bearer: "{{$env.API_TOKEN}}"

# Basic auth
auth:
  basic:
    username: admin
    password: "{{$env.PASSWORD}}"

# API key
auth:
  api_key:
    header: X-API-Key
    value: "{{$env.API_KEY}}"
```

## Variable Substitution

| Pattern | Description |
|---------|-------------|
| `{{var_name}}` | Context variable (from extract or variables) |
| `{{$env.KEY}}` | Environment variable |
| `{{$uuid}}` | Random UUID v4 |
| `{{$timestamp}}` | Unix timestamp (seconds) |
| `{{$randomInt}}` | Random integer |

## Architecture

```
crates/
├── model    — YAML scenario parsing & data structures
├── core     — Assertion engine, JSONPath, variable resolver, validator
├── http     — Production HTTP client (all methods, headers, body, timing)
├── runner   — Execution engine (state machine, retry, concurrency)
└── cli      — CLI interface (clap), reporters (console, JSON, JUnit)
```

Crates are decoupled by design: `core` is pure logic (no I/O), `runner` delegates HTTP to the `http` crate, and `cli` contains no business logic.

## Example Output

```
Scenario: login flow
Running: 3 user(s) × 2 step(s)

  [User 1] [start] --login--> [authenticated] ✓ (201) 342ms
    ✓ status == 201
  [User 1] [authenticated] --get profile--> [done] ✓ (200) 128ms
    ✓ status == 200
    ✓ body.username
    ✓ body.email
    ✓ response_time_ms

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  User 1: Final state: done (2 steps, 470ms)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  Steps: 2 total, 2 passed, 0 failed
  Timing: total 470ms | avg 235ms | p50 235ms | p95 342ms | p99 342ms

  PASS
```

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | All assertions passed |
| `1` | One or more assertions failed |
| `2` | Error (bad YAML, network failure, invalid scenario) |

## CI/CD Integration

ACE generates JUnit XML reports compatible with GitHub Actions, Jenkins, GitLab CI, and more:

```bash
ace run tests/api_smoke.yaml --junit results.xml --quiet
```

```yaml
# GitHub Actions example
- name: Run API tests
  run: cargo run -p ace -- run tests/api.yaml --junit results.xml -q

- name: Publish test results
  uses: dorny/test-reporter@v1
  if: always()
  with:
    name: API Tests
    path: results.xml
    reporter: java-junit
```

## Development

```bash
# Run all tests
cargo test --workspace

# Check formatting
cargo fmt --all -- --check

# Lint
cargo clippy --workspace

# Build release binary
cargo build --release -p ace
```

## License

MIT
