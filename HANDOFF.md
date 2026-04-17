# ACE — Next Steps Handoff

Fresh-session primer. Drop this into a new Claude Code session and ask it to work through the items.

## Repo context

- **Project**: ACE (API Causality Engine) — stateful API workflow testing via YAML scenarios
- **Workspace**: `crates/{cli, core, http, model, runner}`, `ui/{frontend, tauri}`
- **Recent direction**: moving from implicit to **explicit** configuration
  - Workflows use explicit top-level `edges:` (not per-step `transitions:`)
  - `concurrency:` in YAML is deprecated; use CLI `-c / --concurrency` instead
  - **Guiding principle**: scenario YAML describes *what the workflow is*, not *how/where it runs*. Runtime concerns belong on the CLI.

## Tasks (priority order)

### 1. Require explicit `terminal_states:` [HIGH]

**What**: Fail validation if an edge's `to` is neither a known step-state nor listed in `scenario.terminal_states`. Today, dangling edge targets (e.g. `error`, `done`) are silently inferred as terminals.

**Why**: Same ambiguity class as implicit edges — typos in edge targets pass validation today. Explicit > implicit.

**Files**:
- `crates/core/src/validate.rs` — the `inferred_terminals` logic (~line 132) should be removed or gated behind a deprecation. Tighten the existing "Transition target '…' is not a declared state or terminal state" check (currently only fires when `terminal_states` is `Some`).
- `examples/**/*.yaml` — add `terminal_states:` to any scenario currently relying on inference (`examples/workflows/login-create-retry.yaml` uses `error`; `examples/resilience/poll-until-ready.yaml` uses `done`; `examples/branching/conditional-transitions.yaml` uses `done` and `error`).
- `README.md` — document `terminal_states:` in the workflow section.

**Acceptance**: scenarios without `terminal_states:` fail validation with a clear message if any edge `to` isn't a step-state. All existing examples still validate. New test `detects_undeclared_terminal_target`.

### 2. Diagnostic output with error codes + origins [HIGH]

**What**: `ace validate` currently prints flat strings. Turn them into structured diagnostics: `error[E001]: step 'create_order' references '{{token}}' — never extracted` with file path and line/column.

**Why**: Blueprint v2 spec mandates this format. Enables IDE integration and makes errors actionable.

**Files**:
- `crates/core/src/validate.rs` — change `Vec<String>` return to `Vec<Diagnostic>` where `Diagnostic { code, severity, message, span }`.
- YAML loader in `crates/model/src/lib.rs` — need to preserve line/col spans from `serde_yaml` (use `serde_yaml::Value` with marker trait or switch to `saphyr` / `yaml-rust2` if spans are hard to get).
- `crates/cli/src/validate.rs` + `error.rs` — render diagnostics with colored `error[E001]:` prefix, file path, caret-pointing context line.
- Assign stable codes: `E001` undefined variable, `E002` missing outgoing edge, `E003` undeclared terminal, etc.

**Acceptance**: `ace validate` on a broken scenario prints `error[EXXX]: …\n  --> path:line:col\n   |\n 12 |   from: typo_state\n   |         ^^^^^^^^^^`. Existing issue strings still covered by codes.

### 3. Unreachable-step check [MEDIUM]

**What**: After building `outgoing`/`incoming` maps in `validate_scenario`, check whether every step's state is reachable from `initial_state` via BFS. Warn on unreachable steps.

**Why**: Currently a step nobody transitions to is silently dead code. Validator already does terminal reachability — this is the mirror check.

**Files**:
- `crates/core/src/validate.rs` — extend the BFS (already done ~line 150) to collect visited states, then emit a warning for any step whose state isn't visited.

**Acceptance**: new test `detects_unreachable_step` — scenario with 3 steps where step 3 has no inbound edge produces a warning.

### 4. Deprecate other runtime-ish scenario fields [MEDIUM, opinionated]

**What**: Same pattern as concurrency for `insecure`, `proxy`, `default_timeout_ms`. These describe how/where to run, not what the workflow is.

**Why**: Consistency with the principle established this cycle. Env-specific (dev/staging/prod proxy, self-signed certs in dev only) shouldn't be baked into committed YAML.

**Files**:
- `crates/model/src/lib.rs` — add `#[deprecated]` to each field, mirroring the concurrency pattern.
- `crates/cli/src/run.rs`, `crates/cli/src/main.rs` — deprecation warnings + existing CLI flags already cover `--insecure` and `--proxy`. Add `--default-timeout-ms` (or rename to just `--timeout-ms`).
- `crates/runner/src/lib.rs` — `RunConfig` already has `insecure`/`proxy`; add timeout override, prefer CLI over scenario.

**Acceptance**: `ace run` prints deprecation warning for any of those fields in YAML; CLI flags override. Compiler `#[allow(deprecated)]` bracketed at the legacy read sites only.

### 5. Tauri UI catch-up [LOW]

**What**: Frontend editor (`ui/frontend/src/components/Editor.tsx`) still exposes a `concurrency` input that writes into scenario YAML. Remove it — runtime concerns don't belong in the editor.

**Files**:
- `ui/frontend/src/components/Editor.tsx` lines ~116, ~207.
- `ui/frontend/src/types.ts` — keep the field for reading old YAML, surface a "deprecated" badge if present.
- `ui/tauri/src/commands/scenarios.rs` — keep deserializing the field for back-compat, don't write it on new scenarios.

**Acceptance**: new scenarios created via UI don't carry `concurrency:`. Opening an old YAML with it shows a deprecation hint.

## Design principles (cheat sheet)

- **Explicit > implicit** — edges, terminals, transitions. Don't infer.
- **Scenario vs. runtime** — YAML = workflow definition. CLI flags = how/where to run.
- **One pass per loop** — validator walks `scenario.steps` once, not three times.
- **Dedicated error variants** — `RunError::NoOutgoingEdges` over overloaded `InvalidTransition`.
- **Deprecate, don't remove** — `#[deprecated]` on the model field + runtime warning. Removal in a future breaking release.
- **Comments**: default to none. Only where *why* is non-obvious.

## Current test baseline

- 94 tests across the workspace, all passing.
- `cargo clippy --workspace --exclude ace-tauri -- -D warnings` clean.
- Use `cargo test --workspace --exclude ace-tauri` to skip the Tauri build during iteration.

## Files worth reading first

- `crates/core/src/validate.rs` — validator; most of this handoff touches it
- `crates/model/src/lib.rs` — scenario schema + `#[deprecated]` pattern
- `crates/runner/src/lib.rs` — `run_once`, `RunConfig`, `RunError`
- `crates/cli/src/run.rs` — CLI-flag-over-scenario precedence pattern
- `examples/workflows/login-create-retry.yaml` — most complete real example
