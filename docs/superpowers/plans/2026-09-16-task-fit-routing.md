# Task-Fit Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Select the model with the best task-specific quality/cost value without deriving preferences from budget, then require a measurable quality improvement for cascade upgrades.

**Architecture:** Add a required, versioned `SelectionPolicy` to the submission and fixed execution plan. Capture it in routing snapshot schema 4 and evaluate candidates through a focused pure value function. Keep hard constraints, accounting, cancellation, and provider adapters unchanged; use external ranking only after the primary score ties exactly. Cascade reuses the same scorer while applying a strict `min_upgrade_gain` quality floor.

**Tech Stack:** Rust 2021, serde, Tokio, SQLite evidence store, PyO3/maturin, pytest, Ruff, archify.

---

## Scope and file responsibilities

- `src/contracts/selection.rs`: owns `SelectionPolicy` validation and the pure quality-value/cost-value calculations.
- `src/contracts.rs`: exports the policy and carries it in `TaskSpec`.
- `src/contracts/planning.rs`: carries the host policy through `SubmissionSpec` and `EffectivePlan`; planner proposals cannot alter it.
- `src/router/profiles.rs`: freezes the policy and algorithm version in snapshot schema 4.
- `src/router/selection.rs`: builds the primary score and deterministic comparison key.
- `src/router.rs`: retains hard filters and delegates scoring/comparison.
- `src/engine/execution.rs`: supplies the strict upgrade floor and preserves a failed artifact when no improving route exists.
- `tests/task_fit_routing.rs`: pure routing and contract behavior.
- `tests/engine.rs`: cascade, accounting, persistence, and bounded termination behavior.
- `tests/python/test_task_fit_routing.py`: public JSON/Python/native boundary.
- JSON examples and documentation: migrate every repository caller to submission schema 2 and explain the behavior boundary.

The optional critic-driven semantic evaluator from `task-fit-routing-contract.md` section 6 is intentionally excluded. It is independently testable and gets a separate plan after this deterministic-upgrade increment passes.

### Task 1: Add and validate the selection contract

**Files:**
- Create: `src/contracts/selection.rs`
- Modify: `src/contracts.rs`
- Modify: `src/contracts/planning.rs`
- Test: `tests/task_fit_routing.rs`

- [x] **Step 1: Write failing contract tests**

Create `tests/task_fit_routing.rs` with table-driven deserialization and validation tests:

```rust
use model_collaboration_engine::contracts::{Config, SelectionPolicy, SubmissionSpec};
use serde_json::{json, Value};

fn policy() -> Value {
    json!({
        "version":"host-task-fit-v1",
        "min_quality":0.75,
        "target_quality":0.90,
        "above_target_factor":0.10,
        "cost_reference":10000,
        "latency_reference_ms":5000,
        "min_upgrade_gain":0.05
    })
}

#[test]
fn selection_policy_rejects_invalid_bounds_and_unknown_fields() {
    let valid: SelectionPolicy = serde_json::from_value(policy()).unwrap();
    valid.validate().unwrap();
    for (field, value) in [
        ("min_quality", json!(0.91)),
        ("target_quality", json!(1.01)),
        ("above_target_factor", json!(-0.01)),
        ("cost_reference", json!(0)),
        ("latency_reference_ms", json!(0)),
        ("min_upgrade_gain", json!(0.0)),
    ] {
        let mut candidate = policy();
        candidate[field] = value;
        let parsed: SelectionPolicy = serde_json::from_value(candidate).unwrap();
        assert!(parsed.validate().is_err(), "{field}");
    }
    let mut unknown = policy();
    unknown["extra"] = json!(true);
    assert!(serde_json::from_value::<SelectionPolicy>(unknown).is_err());
}
```

Add a submission test that changes `schema_version` to 2, inserts `selection`, and asserts `SubmissionSpec::validate` accepts it; schema 1 and a missing `selection` must be rejected for new submissions.

- [x] **Step 2: Run the focused test and observe the missing type**

Run: `cargo test --test task_fit_routing selection_policy -- --nocapture`

Expected: compilation fails because `SelectionPolicy` is not exported.

- [x] **Step 3: Implement the contract and validation**

Create the following bounded type in `src/contracts/selection.rs`:

```rust
use super::{EngineError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionPolicy {
    pub version: String,
    pub min_quality: f64,
    pub target_quality: f64,
    pub above_target_factor: f64,
    pub cost_reference: u64,
    pub latency_reference_ms: u64,
    pub min_upgrade_gain: f64,
}

impl SelectionPolicy {
    pub fn validate(&self) -> Result<()> {
        let finite = [self.min_quality, self.target_quality,
            self.above_target_factor, self.min_upgrade_gain]
            .iter().all(|value| value.is_finite());
        if self.version.trim().is_empty()
            || self.version.len() > 256
            || !finite
            || !(0.0..=1.0).contains(&self.min_quality)
            || !(self.min_quality..=1.0).contains(&self.target_quality)
            || !(0.0..=1.0).contains(&self.above_target_factor)
            || !(0.0 < self.min_upgrade_gain && self.min_upgrade_gain <= 1.0)
            || self.cost_reference == 0
            || self.cost_reference > i64::MAX as u64
            || !(1..=86_400_000).contains(&self.latency_reference_ms)
        {
            return Err(EngineError::new("configuration", "invalid selection policy"));
        }
        Ok(())
    }

    pub fn quality_value(&self, quality: f64) -> f64 {
        quality.min(self.target_quality)
            + self.above_target_factor * (quality - self.target_quality).max(0.0)
    }
}
```

Count `version` in UTF-8 bytes (`as_bytes().len()` in the final implementation). Export it from `contracts.rs`; add `selection: SelectionPolicy` to `SubmissionSpec`, `TaskSpec`, and `EffectivePlan`. Set submission schema version 2, copy the policy through `From<TaskSpec>`, `SubmissionSpec::task`, and `EffectivePlan::task`, and call `selection.validate()` from task validation. Do not add `selection` to `PlannerProposal`.

- [x] **Step 4: Run the focused contract tests**

Run: `cargo test --test task_fit_routing selection_policy -- --nocapture`

Expected: all selection contract tests pass.

- [ ] **Step 5: Commit the contract increment**

```powershell
git add src/contracts.rs src/contracts/selection.rs src/contracts/planning.rs tests/task_fit_routing.rs
git commit -m "feat: add task-fit selection policy contract"
```

### Task 2: Migrate repository callers and preserve the host policy through planning

**Files:**
- Modify: `examples/task.json`
- Modify: `examples/submission.json`
- Modify: `tests/engine.rs`
- Modify: `tests/feedback.rs`
- Modify: `tests/planning/support.rs`
- Modify: `tests/planning/timeouts.rs`
- Modify: `tests/routing/support.rs`
- Modify: any additional `TaskSpec {` / `SubmissionSpec {` literals returned by `rg`
- Test: `tests/planning.rs`

- [x] **Step 1: Add a failing planning preservation assertion**

In the existing successful planning test, assert all three representations retain the same host value:

```rust
assert_eq!(submission.selection.version, "host-task-fit-v1");
assert_eq!(effective.selection.version, submission.selection.version);
assert_eq!(effective.task(&submission).selection.cost_reference, 10_000);
```

Also extend the invalid proposal JSON case with a `selection` key and assert the proposal is rejected as an unknown field.

- [x] **Step 2: Run planning tests and observe missing fixture fields**

Run: `cargo test --test planning -- --nocapture`

Expected: compilation or deserialization failures identify every unmigrated fixture.

- [x] **Step 3: Migrate JSON and Rust fixtures**

Use this explicit fixture value everywhere unless the test needs a different threshold:

```json
"selection": {
  "version": "host-task-fit-v1",
  "min_quality": 0.0,
  "target_quality": 0.9,
  "above_target_factor": 0.1,
  "cost_reference": 10000,
  "latency_reference_ms": 5000,
  "min_upgrade_gain": 0.05
}
```

Update repository submission examples to `schema_version: 2`. For direct Rust structs, import and construct the same `SelectionPolicy`. Do not use serde defaults, because absence must be visible to callers during this development-stage contract change.

- [x] **Step 4: Run all Rust tests to find every stale caller**

Run: `cargo test --all-features`

Expected: all current tests pass after every fixture is migrated.

- [ ] **Step 5: Commit the caller migration**

```powershell
git add examples tests src/contracts/planning.rs
git commit -m "refactor: migrate submissions to selection policy"
```

### Task 3: Freeze algorithm inputs in routing snapshot schema 4

**Files:**
- Modify: `src/router/profiles.rs`
- Modify: `src/router.rs`
- Test: `tests/task_fit_routing.rs`
- Test: `tests/routing_profiles.rs`

- [x] **Step 1: Write failing snapshot compatibility tests**

Add assertions that a new capture stores schema 4, the exact selection policy, and an algorithm identifier:

```rust
let snapshot = support::snapshot(&config, TaskType::Reasoning);
assert_eq!(snapshot.schema_version, 4);
assert_eq!(snapshot.selection.version, "host-task-fit-v1");
assert_eq!(snapshot.algorithm_version, "task-fit-v1");
```

Extend historical roundtrip coverage: schema 1–3 snapshots omit the new fields and remain routable under their recorded legacy algorithm; schema 4 requires both fields. A schema 4 snapshot missing either must return a routing error rather than silently default.

- [x] **Step 2: Run the snapshot tests and verify schema 3 is still produced**

Run: `cargo test --test task_fit_routing snapshot -- --nocapture`

Expected: assertion fails because capture currently writes schema 3.

- [x] **Step 3: Add fixed selection evidence**

Add to `RoutingSnapshot`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub selection: Option<SelectionPolicy>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub algorithm_version: Option<String>,
```

Capture new tasks as schema 4 with `Some(plan.selection.clone())` and `Some("task-fit-v1".into())`. Keep fields optional solely to deserialize schemas 1–3. In `route_profiled`, require both for schema 4 and select the legacy scorer for schemas 1–3 so old hashes and recorded decisions keep their meaning.

- [x] **Step 4: Run snapshot and profile suites**

Run: `cargo test --test task_fit_routing --test routing_profiles`

Expected: all snapshot roundtrip, hash, and quality tests pass.

- [ ] **Step 5: Commit snapshot versioning**

```powershell
git add src/router.rs src/router/profiles.rs tests/task_fit_routing.rs tests/routing_profiles.rs
git commit -m "feat: freeze task-fit routing inputs"
```

### Task 4: Implement budget-independent primary scoring and ranking tie-breaks

**Files:**
- Create: `src/router/selection.rs`
- Modify: `src/router.rs`
- Modify: `src/router/rankings.rs`
- Test: `tests/task_fit_routing.rs`
- Modify: `tests/external_rankings.rs`

- [x] **Step 1: Write failing value-selection tests**

Build two otherwise identical candidates and assert the contract examples exactly:

```rust
#[test]
fn task_fit_value_selects_economic_or_strong_by_expected_gain() {
    let simple = choose_case(0.90, 1_000, 0.95, 4_000);
    assert_eq!(simple.model_id, "economic");
    assert!((simple.score - 0.890).abs() < 1e-12);

    let difficult = choose_case(0.76, 1_000, 0.95, 4_000);
    assert_eq!(difficult.model_id, "strong");
    assert!((difficult.score - 0.865).abs() < 1e-12);
}
```

Add two more cases: raising task budget and `max_call_cost` while both candidates remain eligible cannot change the winner; a ranking contribution cannot overturn unequal primary scores but breaks an exact primary-score tie.

- [x] **Step 2: Run the focused tests and verify legacy scoring fails them**

Run: `cargo test --test task_fit_routing task_fit_value -- --nocapture`

Expected: winner or score assertions fail because cost is normalized by `max_call_cost` and ranking is directly summed.

- [x] **Step 3: Implement the pure scorer**

Create a focused scorer:

```rust
pub(super) fn primary_parts(
    weights: &Weights,
    policy: &SelectionPolicy,
    quality: f64,
    capability: f64,
    reliability: f64,
    cost: u64,
    latency_ms: u64,
    uncertainty: f64,
) -> BTreeMap<String, f64> {
    BTreeMap::from([
        ("quality".into(), weights.quality * policy.quality_value(quality)),
        ("capability".into(), weights.capability * capability),
        ("reliability".into(), weights.reliability * reliability),
        ("cost".into(), -weights.cost * cost as f64 / policy.cost_reference as f64),
        ("latency".into(), -weights.latency * latency_ms as f64
            / policy.latency_reference_ms as f64),
        ("uncertainty".into(), -weights.uncertainty * uncertainty),
    ])
}
```

Use an internal candidate comparator: primary score descending, applied ranking contribution descending, estimated cost ascending, model ID ascending. Keep `breakdown["ranking"]` for audit, but exclude it from `score` and primary-part summation. Preserve the rank status and normalized value in `RoutingDecision.ranking`.

- [x] **Step 4: Update external-ranking expectations and run routing suites**

Change `external_rank_changes_selection_without_changing_quality` into two tests: exact-primary-score tie selects the ranked candidate; a lower primary score remains lower even with rank 1. Keep all hard-constraint and status tests.

Run: `cargo test --test task_fit_routing --test external_rankings --test routing_profiles`

Expected: all routing, rank, hard-constraint, and replay tests pass.

- [ ] **Step 5: Commit the scorer**

```powershell
git add src/router.rs src/router/selection.rs src/router/rankings.rs tests/task_fit_routing.rs tests/external_rankings.rs
git commit -m "feat: route by task-fit quality and cost value"
```

### Task 5: Require strict quality improvement for cascade upgrades

**Files:**
- Modify: `src/engine/execution.rs`
- Modify: `src/engine/invocation.rs`
- Test: `tests/engine.rs`
- Test: `tests/routing_profiles.rs`

- [x] **Step 1: Write failing cascade behavior tests**

Add cases using deterministic fake outputs and recorded model IDs:

```rust
#[tokio::test]
async fn cascade_requires_configured_quality_gain() {
    let (_dir, engine, _store, fake) = setup_with_qualities(
        0.70, 0.72, 0.05,
        vec![output("wrong", true)],
    ).await;
    let result = engine.run(task(Strategy::Cascade), CancellationToken::new())
        .await.unwrap();
    assert_eq!(result.status, "human_required");
    assert_eq!(*fake.models.lock().unwrap(), vec!["cheap"]);
    assert_eq!(result.artifact.text, "wrong");
}
```

Add a success case with Q 0.70 → 0.76, an upper-bound case with Q 0.98 and gain 0.05 yielding no candidate, and a budget-rejected candidate case. Assert settled/reserved cost and call count in every case. Inspect the persisted checkpoint or route evidence and assert `upgrade_reason`, previous Q, required Q, and exclusion reason.

- [x] **Step 2: Run cascade tests and verify the 0.72 candidate is currently attempted**

Run: `cargo test --test engine cascade -- --nocapture`

Expected: the strict-gain test fails because current code accepts any Q not lower than the previous Q.

- [x] **Step 3: Pass an explicit upgrade requirement into routing**

Replace the ambiguous `quality_floor: f64` call-site state with an evidence-bearing value:

```rust
#[derive(Debug, Clone, Copy, Serialize)]
pub struct UpgradeRequirement {
    pub previous_quality: f64,
    pub required_quality: f64,
}
```

On Revise in cascade, calculate `required_quality = previous_quality + task.selection.min_upgrade_gain` without clamping. If it exceeds 1, terminate as `human_required` with `no_quality_improvement`. Route later rounds with the required value; continue excluding all attempted generators. Preserve first-round `None` so normal `min_quality` applies. Record the requirement and no-candidate category in the checkpoint/route metadata.

When no route exists after at least one artifact, convert only the expected no-candidate routing result into bounded `human_required` while preserving the artifact. Propagate storage, accounting, cancellation, timeout, and configuration errors unchanged.

- [x] **Step 4: Run engine, accounting, cancellation, and profile suites**

Run: `cargo test --test engine --test routing_profiles --test feedback --test reconciliation`

Expected: all tests pass; strict improvement does not weaken accounting or cleanup paths.

- [ ] **Step 5: Commit dynamic upgrade behavior**

```powershell
git add src/engine/execution.rs src/engine/invocation.rs src/router.rs tests/engine.rs tests/routing_profiles.rs
git commit -m "feat: require measurable cascade quality upgrades"
```

### Task 6: Update the Python/native boundary and examples

**Files:**
- Modify: `tests/python/test_engine.py`
- Create: `tests/python/test_task_fit_routing.py`
- Modify: Python/example JSON construction sites found by `rg 'schema_version.*1|selection' examples tests/python`
- Modify: `examples/evaluate.py`
- Modify: `examples/collaboration_probe.py`

- [x] **Step 1: Write failing native integration tests**

Create a local-server test that uses both supported endpoint styles and submits schema 2 with selection. Assert a simple case chooses the economic model, a difficult-profile case chooses the strong model, and increasing budget without changing eligibility keeps the same selection. Add a cascade response sequence where the first artifact fails deterministic acceptance and the second model satisfies the strict gain.

Use the repository's existing local HTTP/SSE fixtures; do not call a real provider. Query SQLite and assert the saved submission, effective plan, routing snapshot algorithm version, score parts, upgrade evidence, attempts, and settled ledger agree.

- [x] **Step 2: Run before rebuilding and confirm the stale native module fails**

Run: `.venv/Scripts/python.exe -m pytest tests/python/test_task_fit_routing.py -q`

Expected: failure because the installed native module does not accept schema 2 or `selection` yet.

- [x] **Step 3: Migrate Python examples and rebuild the extension**

Add the explicit selection object from Task 2 to every generated task/submission. Do not create Python-only defaults. Rebuild:

Run: `.venv/Scripts/python.exe -m maturin develop`

Expected: native extension installs successfully.

- [x] **Step 4: Run focused and full Python tests**

Run: `.venv/Scripts/python.exe -m pytest tests/python/test_task_fit_routing.py -q`

Expected: new integration tests pass.

Run: `.venv/Scripts/python.exe -m pytest -q`

Expected: full Python suite passes.

- [ ] **Step 5: Commit Python/native coverage**

```powershell
git add examples tests/python
git commit -m "test: cover task-fit routing through Python"
```

### Task 7: Synchronize documentation, diagrams, and final verification

**Files:**
- Modify: `README.md`
- Modify: `CONTEXTS.md`
- Modify: `docs/usage.md`
- Modify: `docs/rankings.md`
- Modify: `docs/designs/current-status.md`
- Modify: `docs/designs/task-fit-routing.md`
- Modify: `docs/designs/task-fit-routing-contract.md`
- Modify: `.agent/code-map.md`
- Modify: `docs/diagrams/architecture.json` and generated evidence if module relationships change
- Modify: `docs/diagrams/feedback-routing.json` and generated evidence if upgrade evidence flow changes

- [x] **Step 1: Update user-facing contracts and implementation status**

Document submission schema 2 and every selection field with units and bounds. State that phase one optimizes a single call using acceptance probability evidence; it does not claim total collaboration-cost optimality or full semantic-quality detection. Explain that ranking is an exact-tie reference and old snapshot schemas retain legacy replay semantics.

Update `CONTEXTS.md` with concise domain definitions:

```markdown
**选择偏好（Selection Policy）**：宿主针对一次任务声明的质量目标、成本与延迟参考尺度，以及升级所需最小质量改善。它不扩大预算、权限或候选范围。

**动态升级（Dynamic Upgrade）**：候选产物需要修订时，在剩余资源内改选具有明确预期质量改善的未尝试候选。更贵或榜单更高本身不构成升级。
```

- [x] **Step 2: Update affected diagram JSON sources and regenerate artifacts**

Use the repository's archify workflow in `.agent/workflows/diagram.md`. Edit JSON sources only, regenerate HTML/evidence, refresh source hashes, and keep the pre-existing shutdown visual failure explicitly unresolved.

Expected evidence: architecture and feedback-routing structural validation pass without warnings; browser checks pass; updated views receive a separate visual review result.

- [x] **Step 3: Run final code verification sequentially**

Run these without overlapping Cargo and maturin:

```powershell
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
.venv/Scripts/python.exe -m maturin develop
.venv/Scripts/python.exe -m pytest -q
.venv/Scripts/python.exe -m ruff check .
git diff --check
```

Expected: every command exits 0. Report exact Rust/Python counts from this run rather than copying prior totals.

- [x] **Step 4: Review scope and secrets without printing values**

Use `git status --short`, `git diff --stat`, and targeted diffs. Compare changed text against environment-variable values through a script that reports only file/line and match status; never print `.env`, backup contents, or secret values. Confirm unrelated readiness/ranking work already in the dirty tree remains intact.

- [ ] **Step 5: Commit documentation and verification evidence**

```powershell
git add README.md CONTEXTS.md docs .agent/code-map.md
git commit -m "docs: explain task-fit routing and dynamic upgrades"
```

## Completion criteria

- Hard constraints remain prior to scoring, and money stays integer microcredits.
- Candidate preference does not change merely because the task budget increases.
- Similar expected quality favors lower cost; meaningful expected improvement can justify a strong model on the first call.
- Cascade only switches to an untried candidate meeting the strict configured gain and terminates safely otherwise.
- Ranking cannot overturn a primary task-fit score.
- Submission, effective plan, snapshot, each route, evaluation, and ledger form a recoverable evidence chain.
- Old snapshot schemas replay with their recorded legacy algorithm; new tasks use schema 4/task-fit-v1.
- No real provider request is required for implementation acceptance; any later real evaluation is reported separately.
