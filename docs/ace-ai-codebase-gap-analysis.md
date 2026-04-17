# ACE AI Architecture vs Current Codebase

This maps the proposed AI architecture onto the repository as it exists today.

## Current State

ACE already implements the deterministic workflow engine. It does not yet implement the AI reasoning, retrieval, or LLM integration described in the PDF.

## What Already Exists

### Client surfaces

- CLI entrypoint and subcommands exist in [crates/cli/src/main.rs](/home/Yatsuiii/api--causality-engine/crates/cli/src/main.rs:1).
- Desktop UI commands invoke scenario run and validation in [ui/tauri/src/commands/runner.rs](/home/Yatsuiii/api--causality-engine/ui/tauri/src/commands/runner.rs:1).

Architecture mapping:

- PDF `Client CLI/API` is partially implemented as CLI plus Tauri UI.
- There is no dedicated HTTP API service layer in this repo yet.

### Core execution engine

- Scenario and step schemas live in [crates/model/src/lib.rs](/home/Yatsuiii/api--causality-engine/crates/model/src/lib.rs:1).
- Validation logic lives in [crates/core/src/validate.rs](/home/Yatsuiii/api--causality-engine/crates/core/src/validate.rs:1).
- Deterministic workflow execution lives in [crates/runner/src/lib.rs](/home/Yatsuiii/api--causality-engine/crates/runner/src/lib.rs:1).
- CLI orchestration of validation, run, and reporting lives in [crates/cli/src/run.rs](/home/Yatsuiii/api--causality-engine/crates/cli/src/run.rs:1).

Architecture mapping:

- PDF `ACE Core Engine` is implemented.
- The engine is already the authoritative runtime for validation, transitions, retries, auth, variables, and execution logs.

### Trace generation

- `ExecutionLog` and `StepLog` already capture structured run data in [crates/runner/src/lib.rs](/home/Yatsuiii/api--causality-engine/crates/runner/src/lib.rs:73).
- JSON and JUnit reporting exist in [crates/cli/src/report.rs](/home/Yatsuiii/api--causality-engine/crates/cli/src/report.rs:1).
- Tauri persists history entries after runs in [ui/tauri/src/commands/runner.rs](/home/Yatsuiii/api--causality-engine/ui/tauri/src/commands/runner.rs:1).

Architecture mapping:

- PDF `Trace Generator` is partially implemented as execution logging and reporting.
- The missing part is a distinct failure-centric trace model built for downstream AI consumption.

## What Is Missing

### AI context builder

Missing capabilities:

- A module that converts `ExecutionLog` into a compact, failure-focused summary.
- Secret redaction before prompts or retrieval.
- Causal ranking of preceding steps.

Suggested landing zone:

- New crate or module such as `crates/ai_context`.

### Retrieval layer and vector storage

Missing capabilities:

- Historical failure indexing.
- Embedding generation.
- Similar-case lookup.
- Storage abstraction for vector search.

Suggested landing zone:

- A new retrieval crate with a provider trait, plus persistent storage owned by the CLI/API/UI layer.

### LLM engine

Missing capabilities:

- Provider abstraction for chat/completions.
- Prompt templates grounded in structured trace data.
- Error handling, retries, rate limiting, and cost controls for AI calls.

Suggested landing zone:

- A new crate such as `crates/ai` with provider adapters behind feature flags.

### Response formatter for AI output

Missing capabilities:

- A combined engine-plus-AI failure report format.
- CLI and UI rendering that clearly separates facts from suggestions.
- JSON schema for AI-enriched outputs.

Suggested landing zone:

- Extend `crates/cli/src/report.rs` and the Tauri history types to carry `AIOutput`.

## Data Model Gaps

The current `ExecutionLog` shape is close to the PDF’s `Trace`, but not identical.

Current strengths:

- Step-level timing, state transitions, status codes, and assertion results already exist.
- Request and response bodies can already be captured.

Current gaps:

- No top-level `workflow_id` separate from report persistence concerns.
- No explicit `failure_point` field.
- No normalized error taxonomy for AI prompting.
- No retrieval metadata, citations, or confidence output types.

Practical conclusion:

- `ExecutionLog` should remain the raw runtime record.
- Add a derived `Trace` or `FailureTrace` model instead of forcing AI needs into the raw execution log.

## Suggested Target Mapping

```text
Current repo
  CLI / Tauri UI
  -> already present

Current repo
  model + core + runner
  -> already present

Needed next
  trace normalization layer
  -> derive FailureTrace from ExecutionLog

Needed next
  AI context builder
  -> summarize FailureTrace

Needed next
  retrieval layer
  -> search similar historical failures

Needed next
  LLM adapter
  -> explain cause and recommend fixes

Needed next
  response formatter
  -> merge facts and AI guidance for CLI/UI
```

## Recommended Next Build Steps

1. Introduce a `FailureTrace` struct derived from `ExecutionLog`.
2. Add redaction utilities for headers, bodies, and variable values.
3. Add a deterministic summarizer that works without any LLM.
4. Define `AIInput` and `AIOutput` Rust types in a dedicated crate.
5. Add an LLM provider trait and a no-op implementation for local builds.
6. Extend CLI and Tauri history views to display AI-enriched diagnostics.

## Bottom Line

The PDF architecture is directionally aligned with the repository. The execution engine, schema system, validation, and reporting layers already exist. The main gap is not “core workflow execution”; it is the entire post-failure intelligence path: trace normalization, retrieval, LLM integration, and AI-aware output formatting.
