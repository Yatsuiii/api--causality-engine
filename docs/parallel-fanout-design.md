# Parallel Fan-out — Design Doc

Status: **Draft for review**
Target: `executor` + `model` + `ace-core::validator`
Prereqs: edge model hardening (priority/tag/after_ms/max_takes), weighted routing, seeded RNG

---

## 1. Motivation

Today every scenario is a linear FSM traversal — one state at a time. Common real-world
flows need concurrent sub-workflows that rejoin:

- **Aggregate fetches**: load user → in parallel fetch posts, comments, photos → render
- **Chaos/redundancy**: hit primary + replica, compare responses
- **Setup/teardown**: parallel provision resources A/B/C, then run main test, then cleanup

`concurrency: N` (VUs) is the wrong tool: it runs N copies of the *whole* scenario. We
need fan-out *inside* a scenario.

---

## 2. YAML syntax (proposed)

Fan-out lives on a new edge variant. The edge from a source state declares its
branches and rendezvous point:

```yaml
edges:
  - from: fetch_user
    parallel:
      branches:
        - name: posts
          to: fetch_posts
        - name: comments
          to: fetch_comments
        - name: photos
          to: fetch_photos
      join: aggregate
      on_failure: fail_fast   # default; alt: all_complete
```

Rules:
- `parallel` is mutually exclusive with `to`, `when`, `default`, `weight` on the same edge.
- Each branch entry must declare `to:` and `name:`. Names are branch identifiers used
  for context namespacing and step-log tagging.
- `join:` names the rendezvous state. Every path inside every branch must converge on
  `join` (validator-enforced).
- `on_failure: fail_fast` — first branch error cancels siblings, propagates up.
- `on_failure: all_complete` — all branches run to completion; first error returned at join.

Branch subgraphs are **plain edges** rooted at each branch's `to:`. They can use
conditional/weighted/priority logic internally. They terminate when they reach `join`.

### What the FSM sees

The fan-out edge is *not* a transition to a single state. When the executor evaluates
edges from `fetch_user` and the winning edge has `parallel:`, it:
1. Snapshots context.
2. Spawns one task per branch, each running its own sub-FSM starting at `branch.to`.
3. Awaits them per `on_failure` policy.
4. Merges contexts, sets `current_state = join`, resumes main FSM.

### Terminal states inside branches

Prohibited in v1: branches cannot declare `terminal_states`. They exit only by
reaching `join`. Reasoning: a branch reaching a scenario-level terminal would collapse
the whole run mid-fan-out, which is ambiguous.

---

## 3. Context scoping & merge

Each branch gets its own `Context` (cloned from the fork point). After all branches
complete, extracts and hook-set variables collected by branch `X` are merged back
under the `X.` namespace.

**Merge rule**: every key the branch *added or overwrote* (diff against snapshot)
lands in parent context as `branch_name.key`.

Example — after fan-out:
```
{{posts.post_count}}
{{posts.latest_id}}
{{comments.unread_count}}
{{photos.album_id}}
```

Within a branch, refs are unprefixed — `{{post_count}}` inside `posts` works as usual.
Namespacing applies only at merge-back.

**Why namespaced, not flat last-writer-wins?**
- Flat is racey: extract order across concurrent branches is non-deterministic.
- Namespaced is self-documenting: readers see which branch produced a value.
- Git branches / Tauri channels / ETL sinks all use this pattern.

**Variable collision**: if a branch name matches an existing scenario variable key,
validator flags E017 (collision — merge would shadow). Branch names live in the
same namespace as extract keys.

---

## 4. Failure semantics

`fail_fast` (default):
- First branch to return `Err` triggers cancellation of the others (`tokio::task::JoinHandle::abort`).
- Main FSM returns that error; cancelled branches' logs are still captured (partial).
- `ExecutionLog.failed += 1` for the causing branch's step count, others not counted.

`all_complete`:
- Every branch runs to natural completion (success or failure).
- At join, if any branch errored, return the earliest-recorded error.
- All branch step logs captured fully.
- Useful for "collect all assertion failures in one run" style testing.

Skip (`RunError::Skipped`) inside a branch: treat like normal `Skipped` within the branch
subgraph (follows default edge) — does NOT propagate to fan-out.

---

## 5. Execution model

```rust
async fn execute_fan_out(
    fan_out: &FanOut,
    parent_context: &Context,
    graph: &Graph,
    client: &Client,
    config: &RunConfig,
    parent_task_id: usize,
    parent_log: &mut ExecutionLog,
) -> Result<String, RunError> {
    let snapshot = parent_context.clone();
    let mut handles = Vec::with_capacity(fan_out.branches.len());

    for (branch_idx, branch) in fan_out.branches.iter().enumerate() {
        let mut branch_ctx = snapshot.clone();
        let branch_name = branch.name.clone();
        let join_state = fan_out.join.clone();
        let branch_start = branch.to.clone();
        // ... clone graph/client/config ...
        handles.push(tokio::spawn(async move {
            run_branch_until_join(
                &branch_start,
                &join_state,
                &mut branch_ctx,
                // ...
                format!("{}#{}", parent_task_id, branch_idx),
            )
            .await
            .map(|log| (branch_name, branch_ctx, log))
        }));
    }

    let mut branch_results = Vec::new();
    let fail_fast = matches!(fan_out.on_failure, Some(FailurePolicy::FailFast) | None);

    if fail_fast {
        for handle in handles {
            match handle.await.expect("branch panicked") {
                Ok(r) => branch_results.push(r),
                Err(e) => {
                    // TODO: abort remaining handles
                    return Err(e);
                }
            }
        }
    } else {
        let mut first_err = None;
        for handle in handles {
            match handle.await.expect("branch panicked") {
                Ok(r) => branch_results.push(r),
                Err(e) if first_err.is_none() => first_err = Some(e),
                Err(_) => {}
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
    }

    // Merge branch logs into parent (interleaved by completion, tagged by branch).
    for (branch_name, branch_ctx, branch_log) in branch_results {
        for mut step in branch_log.steps {
            step.branch_path
                .get_or_insert_with(Vec::new)
                .insert(0, branch_name.clone());
            parent_log.steps.push(step);
        }
        parent_log.total_steps += branch_log.total_steps;
        parent_log.passed += branch_log.passed;
        parent_log.failed += branch_log.failed;

        let diff = context_diff(&snapshot, &branch_ctx);
        for (k, v) in diff {
            parent_context.insert(format!("{}.{}", branch_name, k), v);
        }
    }

    Ok(fan_out.join.clone())
}
```

Notes:
- `run_branch_until_join` is a thin wrapper around the existing `run_once` loop,
  with a different exit condition: stop when `current_state == join`.
- Branch task_id = `"{parent}#{branch_idx}"` — string format so nested fan-out (future)
  works. RNG seeds use `parent_seed.wrapping_add(hash(branch_idx))`.
- `parent_log.steps` becomes interleaved across branches; `StepLog.branch_path: Vec<String>`
  lets reporters group.

---

## 6. Model additions

```rust
pub struct Edge {
    pub from: String,
    // `to` becomes optional — only used for single-target edges.
    pub to: Option<String>,
    #[serde(default)]
    pub when: Option<TransitionCondition>,
    #[serde(default)]
    pub default: Option<bool>,
    // ... priority, tag, after_ms, max_takes, weight (existing)
    #[serde(default)]
    pub parallel: Option<FanOut>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FanOut {
    pub branches: Vec<Branch>,
    pub join: String,
    #[serde(default)]
    pub on_failure: Option<FailurePolicy>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    pub name: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    FailFast,
    AllComplete,
}
```

**Breaking change**: `Edge.to` becomes `Option<String>`. Every existing edge has `to`,
so parsing stays compatible, but all code reading `edge.to` must handle `None` (which
will be visible only when `parallel` is `Some`). This is acceptable — it forces
downstream code to pattern-match and handle the fan-out case rather than silently
defaulting.

**Alternative considered**: keep `to` required; use `to: <join_state>` on parallel
edges with separate `branches:`. Cleaner typing but confusing semantics (`to` on a
parallel edge doesn't mean "go here next"; it means "eventually go here").
Rejected — two meanings for one field.

---

## 7. Validator rules

New diagnostic codes:

- **E011** (error): `parallel.branches` is empty, or has only 1 branch (just use a
  regular edge).
- **E012** (error): branch `to:` names an unknown state.
- **E013** (error): `join:` state is not declared as a step or terminal state.
- **E014** (error): branch subgraph cannot reach `join:` (BFS from branch.to).
- **E015** (error): branch subgraph reaches a state with another `parallel:` edge
  (nested fan-out — rejected for v1).
- **E016** (error): duplicate branch names within a single fan-out.
- **E017** (error): branch name collides with a scenario-level variable or an
  extract key declared outside the branch — would shadow on merge.
- **E018** (error): `parallel` edge has other fields set (`to`, `when`, `default`,
  `weight`) — must be mutually exclusive.
- **E019** (warning): branch subgraph has a path to `join:` that also passes through
  another fan-out's `join:` — potential cross-fan-out contamination.

Also relax/update:
- E007 and friends continue to apply *within* branch subgraphs, scoped.

---

## 8. ExecutionLog changes

```rust
pub struct StepLog {
    // ... existing fields ...
    /// Ordered list of branch names this step executed under (outermost first).
    /// Empty/None = main line. Nested fan-out would stack names here (future).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_path: Option<Vec<String>>,
}
```

Branch step logs are interleaved into `parent_log.steps` in completion order.
Reporters can group by `branch_path` or flatten.

**Alternative**: nested `Vec<Vec<StepLog>>`. Rejected — flat+tagged is simpler to
serialize, more forward-compatible with nested fan-out, and easier to render as
linear console output (which is what CLI users see today).

---

## 9. Out of scope (v1)

- **Nested fan-out** — reject via E015. Opens up: nested branch paths, sub-seed
  derivation, abort cascading. Revisit after v1 lands.
- **Dynamic fan-out** ("spawn one branch per item in extracted JSON array"). That's
  "map" semantics — separate feature, needs extract-time branch construction.
- **Cross-branch communication** — branches are isolated. No `{{other_branch.*}}`
  mid-flight. Only after merge-back.
- **Branch-local timeouts** — branches share scenario's `default_timeout_ms`.
- **Partial cancellation** — `fail_fast` aborts via `JoinHandle::abort`, which
  cancels `await` points but doesn't clean up mid-request. Good enough for v1.
  Future: cooperative cancel via `CancellationToken`.

---

## 10. Test plan

Model:
- Parse `parallel:` edge with branches + join + on_failure.
- `deny_unknown_fields` rejects typos in FanOut/Branch.

Validator:
- E011 empty/1-branch.
- E012/E013/E014 unreachable join, unknown branch target.
- E015 nested fan-out rejection.
- E016 duplicate branch names.
- E017 name collision with variables.
- E018 mutually-exclusive field on parallel edge.

Executor (integration tests with mock HTTP):
- Happy path: 3 branches all succeed, merged context has namespaced keys.
- `fail_fast`: middle branch errors, others cancelled, error propagates.
- `all_complete`: two succeed, one fails, error returned after all finish.
- Branch with internal weighted routing — seeded determinism preserved.
- Timing: total duration ≈ max(branch_durations), not sum (concurrency assertion).
- Context isolation: branch A's extract does not leak into branch B mid-flight.

---

## 11. Open questions for review

1. **Merge rule**: namespace (`{{posts.count}}`) vs flat last-writer-wins vs explicit
   `merge:` mapping? Recommendation: **namespace** — least surprising, safest.
2. **Default failure policy**: `fail_fast` or `all_complete`? Recommendation:
   **fail_fast** — matches testing intent (halt on error).
3. **Minimum branch count**: 2 (feature isn't useful with 1) or 1 (allow degenerate)?
   Recommendation: **2+** (E011 rejects single-branch).
4. **Branch name**: required or optional? Recommendation: **required** — merge needs
   a namespace key.
5. **Edge `to` field**: make `Option<String>` (proposed) or use a union enum
   `EdgeKind::Single { to } | EdgeKind::Parallel { fan_out }`? Recommendation:
   `Option<String>` — smaller churn, serde untagged works cleanly.
6. **Scope limit on v1**: reject nested fan-out via E015 (proposed) or support it
   from day one? Recommendation: **reject v1**, add later. Nested semantics are
   non-trivial (how do branch-path merges compose?).

---

## 12. Rollout

Phase 1 (this doc): alignment. No code.
Phase 2: model types + parse tests.
Phase 3: validator E011–E018.
Phase 4: executor fan-out loop + context merge + integration tests.
Phase 5: CLI/Tauri — `StepLog.branch_path` rendering in reports; Tauri UI surface.
Phase 6: docs — update README with a fan-out example.

Estimated: 1–2 days end-to-end if design is stable.
