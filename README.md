# ACE — API Causality Engine

[![CI](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/ci.yml)
[![Release](https://github.com/Yatsuiii/api--causality-engine/actions/workflows/release.yml/badge.svg)](https://github.com/Yatsuiii/api--causality-engine/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

API testing tool for workflows that span multiple requests. You write a YAML file describing the steps, what to assert, what to extract from each response, and how to move between states. ACE runs it.

No GUI. No cloud account. No collections syncing to someone's server. Just a binary and a YAML file.

## Why

Postman is fine for poking at endpoints. It falls apart when you need to test something like:

1. Create a user → extract the ID
2. Log in → extract the token  
3. Use the token to fetch the user profile → assert the fields match
4. Delete the user → assert 204

That's a stateful workflow, not a collection of independent requests. ACE models it as a state machine so transitions are explicit, pre-validated, and replayable.

## Install

```bash
# Linux / macOS
curl -L https://github.com/Yatsuiii/api--causality-engine/releases/latest/download/ace-linux-x86_64.tar.gz | tar xz
sudo mv ace /usr/local/bin/
```

Pre-built binaries for Linux (x86_64, aarch64), macOS (x86_64, Apple Silicon), and Windows on the [releases page](https://github.com/Yatsuiii/api--causality-engine/releases/latest).

Or from source (requires Rust):

```bash
cargo install --git https://github.com/Yatsuiii/api--causality-engine ace
```

## Usage

```bash
ace init              # scaffold a new scenario
ace run scenario.yaml # run it
ace run scenario.yaml -v           # show request/response bodies
ace run scenario.yaml --env .env --var base_url=https://staging.api.com
ace run scenario.yaml --junit report.xml  # JUnit output for CI
ace validate scenario.yaml         # catch errors without running
ace replay execution_log.json      # replay a previous run exactly
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

## License

MIT
