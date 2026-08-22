# Exact Call Resolution Availability Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Increase exact call-path availability by repairing upstream callable identity and parser-backed call resolution while leaving the proof kernel's strict admission predicate unchanged.

**Architecture:** Keep exact selector lookup fail-closed in `codestory-runtime` and keep raw-edge admission in `codestory-agent` unchanged. Repair the owning `codestory-indexer` seams instead: give callable definitions stable declaration-first canonical identities, carry parser-proven receiver/import facts into an explicit exact-resolution decision, and persist `Certain` only when those structural facts identify one target. Name-only, fuzzy, semantic, ambiguous, and incomplete paths remain heuristic or unresolved.

**Tech Stack:** Rust 2024, tree-sitter graph rules, SQLite-backed CodeStory graph storage, Cargo integration tests, the production-dark `codestory-proof-availability` harness.

**Spec:** [CodeStory v3 retrieval/proof separation](../codestory-v3-retrieval-proof-separation.md)

## Global Constraints

- Start the remediation branch from the live `origin/dev/codestory-0.18`; do not use `dev/codestory-next`, `main`, or any 0.17 release branch as its base or proof source.
- Candidate 7 is qualification `20260822T143747Z-ff5f8b53f864`, source commit `ff5f8b53f864225244281a7d76382d50589b130e`, source tree `0926587beb10d475c59453059af57bd56adb2643`, and selected outcome `keep_proof_dark`. Its checked-in result directory is immutable.
- Keep `diagnose_raw_call_edge` and `admit_raw_call_edge` strict: stored `CALL`, stored `Certain`, exact effective source and target, `resolved_target == Some(expected_target)`, no candidate alternatives, valid file/line/canonical callsite identity, callable containment, and publication-bound source remain required. Change that predicate only in a separately reviewed soundness fix that reproduces an incorrect admission or rejection independently of this availability target.
- Do not lower a confidence or certainty threshold, map `Probable` or `Uncertain` to `Certain`, infer certainty from a score, add fuzzy selector fallback, or choose the first matching node.
- Do not add checks for repository names, paths from the four qualification repositories, Candidate 7 case IDs, expected symbol names, or benchmark answer shapes to production code. Test comments may cite case IDs; implementation must be language- and syntax-contract based.
- Do not edit `benchmarks/proof-availability/corpus-v1.json`, `benchmarks/proof-availability/paths/**`, `benchmarks/proof-availability/thresholds-v1.json`, or `benchmarks/proof-availability/results/20260822T143747Z-ff5f8b53f864/**`.
- Outcome C integration is independent. The evidence-only packet/context/search cut may proceed using Candidate 7 without waiting for, merging from, or consuming this remediation branch. This lane must not edit Outcome C integration files.
- Any implemented kernel, indexer, corpus, or threshold change invalidates Candidate 7 as evidence for the changed candidate. After this indexer change lands, create a fresh qualification ID and rerun Task 13; never compare changed-source output to Candidate 7 as one run.
- After two failed revisions of the same failure shape, stop. Redesign the identity or structural-evidence seam instead of adding another language exception or repository-specific heuristic.
- No version bump, changelog entry, release file, tag, marketplace mutation, publication, 0.17 change, or release qualification belongs in this plan.

---

## Trigger and exact routed rows

Candidate 7 contains `312 = 148 admitted + 80 first_zero_survivor + 84 unclassified` expected steps. The failed-step denominator is `164 = 80 first-zero + 84 unclassified`. The exact-call-resolution numerator is the disjoint union of failed steps with `certainty_absent`, `certainty_probable`, or `certainty_uncertain`, plus unclassified steps whose exact selector did not resolve.

| Cohort | Failed expected steps | Certainty failures | Missing exact selector target | Trigger | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| `codestory-rust` | 4 | 4 | 0 | `4 / 4 = 100.0%` | Pass |
| `flask-python` | 78 | 0 | 78 | `78 / 78 = 100.0%` | Pass |
| `gin-go` | 29 | 19 | 5 | `24 / 29 = 82.76%` | Pass |
| `vite-ts-js` | 53 | 52 | 1 | `53 / 53 = 100.0%` | Pass |
| **All cohorts** | **164** | **75** | **84** | **`159 / 164 = 96.95%`** | **Pass** |

The five excluded `wrong_effective_target`-only failures are `gin-go-15` step 1, `gin-go-19` step 1, `gin-go-20` step 1, `gin-go-24` step 0, and `gin-go-29` step 0. Do not fold them into this owner lane.

Exact contributor case IDs:

- `codestory-rust`, certainty: `codestory-rust-06-l4`, `codestory-rust-07-l4`, `codestory-rust-30-l1`.
- `flask-python`, missing selector target: `flask-python-l1-01`, `flask-python-l1-02`, `flask-python-l1-03`, `flask-python-l1-04`, `flask-python-l1-05`, `flask-python-l1-06`, `flask-python-l1-07`, `flask-python-l1-08`, `flask-python-l1-09`, `flask-python-l1-10`, `flask-python-l2-01`, `flask-python-l2-02`, `flask-python-l2-03`, `flask-python-l2-04`, `flask-python-l2-05`, `flask-python-l2-06`, `flask-python-l2-07`, `flask-python-l3-01`, `flask-python-l3-02`, `flask-python-l3-03`, `flask-python-l3-04`, `flask-python-l3-05`, `flask-python-l4-01`, `flask-python-l4-02`, `flask-python-l4-03`, `flask-python-l5-01`, `flask-python-l5-02`, `flask-python-l5-03`, `flask-python-l6-01`, `flask-python-l6-02`.
- `gin-go`, certainty: `gin-go-01`, `gin-go-02`, `gin-go-03`, `gin-go-04`, `gin-go-05`, `gin-go-07`, `gin-go-08`, `gin-go-09`, `gin-go-10`, `gin-go-11`, `gin-go-14`, `gin-go-21`, `gin-go-22`, `gin-go-23`, `gin-go-25`, `gin-go-26`, `gin-go-27`, `gin-go-30`; missing selector target: `gin-go-06`, `gin-go-28`.
- `vite-ts-js`, certainty: `vite-ts-js-01-l6`, `vite-ts-js-02-l6`, `vite-ts-js-03-l5`, `vite-ts-js-04-l5`, `vite-ts-js-05-l5`, `vite-ts-js-06-l4`, `vite-ts-js-07-l4`, `vite-ts-js-08-l4`, `vite-ts-js-09-l3`, `vite-ts-js-10-l3`, `vite-ts-js-11-l3`, `vite-ts-js-12-l3`, `vite-ts-js-13-l3`, `vite-ts-js-15-l2`, `vite-ts-js-16-l2`, `vite-ts-js-17-l2`, `vite-ts-js-18-l2`, `vite-ts-js-19-l2`, `vite-ts-js-20-l2`, `vite-ts-js-21-l1`, `vite-ts-js-22-l1`, `vite-ts-js-23-l1`, `vite-ts-js-24-l1`, `vite-ts-js-25-l1`, `vite-ts-js-27-l1`, `vite-ts-js-28-l1`, `vite-ts-js-29-l1`, `vite-ts-js-30-l1`; missing selector target: `vite-ts-js-26-l1`.

## Source ownership and implementation boundary

| Responsibility | Current owner | Plan decision |
| --- | --- | --- |
| Exact canonical, pinned, and qualified-name selector lookup | `crates/codestory-runtime/src/indexed_source_call_path_v1.rs:843-1010` | Keep exact and fail-closed. No fallback from canonical ID to qualified name and no ambiguity-breaking selection. |
| Strict raw-edge admission | `crates/codestory-agent/src/indexed_source_call_path_v1.rs:106-188` | Keep unchanged. It is the consumer contract, not the availability repair point. |
| Callable qualification and canonical ID generation | `crates/codestory-indexer/src/lib.rs:10226-10573` | Repair declaration/reference ordering so exact callable identities are stable and unique. |
| Parser artifact identity | `crates/codestory-indexer/src/cache.rs:1-160` | Bump the parser artifact version because canonical nodes and remapped edges change. |
| Language-backed receiver/import facts | `crates/codestory-indexer/src/languages/python.rs`, `go.rs`, and `javascript.rs`; graph rules under `crates/codestory-indexer/rules/` | Emit only syntax-backed owner/module facts; ambiguous or shadowed bindings remain unresolved. |
| Call candidate selection and persisted certainty | `crates/codestory-indexer/src/resolution/candidate_selection.rs:3-501` and `crates/codestory-indexer/src/resolution/mod.rs:487-717,1154-1199,1740-2665` | Separate exact structural evidence from heuristic strategy and stamp `Certain` only for the former. |
| Qualification measurement | `crates/codestory-bench/src/bin/codestory_proof_availability/` and `benchmarks/proof-availability/**` | Read-only during implementation; rerun the frozen Task 13 interface after merge under a new ID. |

The smallest safe owner boundary is therefore `codestory-indexer`, with end-to-end non-regression coverage in `codestory-runtime`. A runtime selector fallback would make a malformed or stale selector appear exact. A kernel relaxation would admit evidence that the index did not establish. Neither is part of this remediation.

### Task 1: Stabilize callable definition identity ahead of placeholders

**Files:**
- Modify: `crates/codestory-indexer/src/lib.rs:10226-10573`
- Modify: `crates/codestory-indexer/src/cache.rs:1-160`
- Modify: `crates/codestory-indexer/tests/integration.rs:560-620`
- Modify: `crates/codestory-indexer/rules/python.scm`
- Modify: `crates/codestory-indexer/rules/go.scm`
- Modify: `crates/codestory-indexer/rules/javascript.scm`
- Modify: `crates/codestory-indexer/rules/typescript.graph.scm`
- Modify: `crates/codestory-indexer/rules/tsx.graph.scm`
- Create: `crates/codestory-indexer/tests/exact_call_resolution_availability.rs`

**Interfaces:**
- Consumes: tree-sitter graph nodes, their `canonical_role`, `apply_qualified_names`, and the existing `{file}:{qualified_name}#{ordinal}` canonical-ID format.
- Produces: `CanonicalNodeRole::Definition`; declaration-first ordinals for identity-bearing callables; stable `#0` identities for one-definition names; parser artifact cache version `4`.
- Preserves: exact canonical-ID string comparison in `codestory-runtime`; type-like IDs; special preserved IDs; overload order among real declarations; current native path rules.

- [ ] **Step 1: Write identity RED tests**

Add an in-memory `WorkspaceIndexer` helper to `exact_call_resolution_availability.rs`, then add these tests with small syntax fixtures rather than copied repository files:

```rust
#[test]
fn python_callable_definition_keeps_zero_ordinal_when_same_named_calls_surround_it() -> anyhow::Result<()> {
    let storage = index_project(&[(
        "workflow.py",
        "def before():\n    target()\n\ndef target():\n    return 1\n\ndef after():\n    target()\n",
    )])?;
    assert_unique_callable_canonical_id(&storage, "workflow.py:target#0", "target")
}

#[test]
fn go_method_keeps_owner_qualified_identity_without_a_local_type_declaration() -> anyhow::Result<()> {
    let storage = index_project(&[(
        "deprecated.go",
        "package sample\nfunc (c *Context) BindWith() { c.MustBindWith() }\n",
    )])?;
    assert_unique_callable_qualified_name(&storage, "deprecated.go", "Context.BindWith")
}

#[test]
fn typescript_exported_function_is_one_exact_file_qualified_match() -> anyhow::Result<()> {
    let storage = index_project(&[(
        "src/fs.ts",
        "export function isDirectory(path: string) { return tryStatSync(path) }\nfunction tryStatSync(path: string) { return path }\n",
    )])?;
    assert_unique_callable_qualified_name(&storage, "src/fs.ts", "isDirectory")
}
```

`assert_unique_callable_canonical_id` must require one callable node with the exact canonical ID and no non-callable node sharing it. `assert_unique_callable_qualified_name` must require one callable match in the named file; it must not accept a prefix or choose among duplicates. Cite the Flask selector cases, `gin-go-06`, `gin-go-28`, and `vite-ts-js-26-l1` in test comments only.

- [ ] **Step 2: Run RED**

```sh
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability python_callable_definition_keeps_zero_ordinal_when_same_named_calls_surround_it -- --exact
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability go_method_keeps_owner_qualified_identity_without_a_local_type_declaration -- --exact
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability typescript_exported_function_is_one_exact_file_qualified_match -- --exact
```

Expected: at least one test fails because a placeholder/reference participates in callable identity or the exact callable qualification is missing/duplicated. If all three pass, stop this task and use the failing Candidate 7 selector row to add the smallest syntax-equivalent fixture before changing source; do not add runtime fallback.

- [ ] **Step 3: Mark actual definitions without changing public graph types**

Add a private definition role and parse it from graph attributes:

```rust
enum CanonicalNodeRole {
    Definition,
    Declaration,
    ForwardDeclaration,
    ImplAnchor,
    Unspecified,
}

fn canonical_role_from_graph_attr(value: &str) -> CanonicalNodeRole {
    match value {
        "definition" => CanonicalNodeRole::Definition,
        "declaration" => CanonicalNodeRole::Declaration,
        "forward_declaration" => CanonicalNodeRole::ForwardDeclaration,
        "impl_anchor" => CanonicalNodeRole::ImplAnchor,
        _ => CanonicalNodeRole::Unspecified,
    }
}

fn canonical_role_priority(role: CanonicalNodeRole) -> u8 {
    match role {
        CanonicalNodeRole::Definition => 4,
        CanonicalNodeRole::Declaration => 3,
        CanonicalNodeRole::Unspecified => 2,
        CanonicalNodeRole::ForwardDeclaration => 1,
        CanonicalNodeRole::ImplAnchor => 0,
    }
}
```

Add `canonical_role = "definition"` to executable function/method definition nodes in the five affected graph-rule files. Do not tag call placeholders, import bindings, usage nodes, or forward declarations as definitions.

- [ ] **Step 4: Rank definition/declaration lines before reference lines**

Change `declaration_ordinals` to accept `canonical_roles`. For each qualified name, sort unique identity-bearing lines first and all remaining lines second; preserve source order within each group. When a language supplies no roles for a name, retain the current all-lines ordering. Pass the roles from `canonicalize_nodes_with_file_identity`.

```rust
let identity_bearing = matches!(
    canonical_roles.get(&node.id),
    Some(CanonicalNodeRole::Definition | CanonicalNodeRole::Declaration | CanonicalNodeRole::ForwardDeclaration)
);
```

This keeps `file:qualified#0` attached to the sole real callable definition while retaining distinct IDs for unresolved placeholders. Do not discard or merge placeholder nodes at this stage.

- [ ] **Step 5: Invalidate stale parser artifacts**

Set `INDEX_ARTIFACT_CACHE_VERSION` to `4` and update the adjacent comment to name declaration-first callable identity. Extend `test_index_artifact_cache_copies_across_compatible_roots` in `crates/codestory-indexer/tests/integration.rs` so a version-3 artifact cannot satisfy the version-4 lookup.

- [ ] **Step 6: Run GREEN and identity regression lanes**

```sh
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability
cargo test --locked -p codestory-indexer canonical
cargo test --locked -p codestory-indexer --test integration test_index_artifact_cache_copies_across_compatible_roots -- --exact
```

Expected: the new exact-identity tests pass; duplicate definitions remain ambiguous; same-name calls no longer displace the real declaration's ordinal; copied version-3 artifacts miss.

- [ ] **Step 7: Commit**

```sh
git add crates/codestory-indexer/src/lib.rs crates/codestory-indexer/src/cache.rs crates/codestory-indexer/tests/integration.rs crates/codestory-indexer/rules/python.scm crates/codestory-indexer/rules/go.scm crates/codestory-indexer/rules/javascript.scm crates/codestory-indexer/rules/typescript.graph.scm crates/codestory-indexer/rules/tsx.graph.scm crates/codestory-indexer/tests/exact_call_resolution_availability.rs
git commit -m "stabilize exact callable identities"
```

### Task 2: Persist certainty only from exact structural call evidence

**Files:**
- Modify: `crates/codestory-indexer/src/languages/python.rs`
- Modify: `crates/codestory-indexer/src/languages/go.rs`
- Modify: `crates/codestory-indexer/src/languages/javascript.rs`
- Modify: `crates/codestory-indexer/src/resolution/mod.rs:487-717,1154-1199,1740-2665`
- Modify: `crates/codestory-indexer/src/resolution/candidate_selection.rs:3-501`
- Modify: `crates/codestory-indexer/tests/exact_call_resolution_availability.rs`
- Modify: `crates/codestory-indexer/tests/call_resolution_common_methods.rs`
- Modify: `crates/codestory-runtime/src/indexed_source_call_path_v1.rs:1280-3225` tests only

**Interfaces:**
- Consumes: `ManualReceiverCallSpec` owner/module facts, canonical callsite markers, unique exact candidate-index lookups, and current `ResolutionStrategy` telemetry.
- Produces: private `ExactCallResolutionEvidence` and `SelectedCallTarget`; one persisted resolved target with `ResolutionCertainty::Certain` and an empty alternative set only when syntax-backed evidence names that unique target.
- Preserves: the existing confidence values and `ResolutionCertainty::from_confidence` behavior for same-name, global-unique, fuzzy, semantic, incomplete, and ambiguous fallbacks.

- [ ] **Step 1: Add exact-evidence RED tests and hostile negatives**

Extend `exact_call_resolution_availability.rs` with one exact and one ambiguous/shadowed fixture per affected language family. Each exact case must assert all strict persisted fields, not only `resolved_target`:

```rust
fn assert_strict_exact_call(edge: &Edge, source: NodeId, target: NodeId) {
    assert_eq!(edge.kind, EdgeKind::CALL);
    assert_eq!(edge.effective_source(), source);
    assert_eq!(edge.effective_target(), target);
    assert_eq!(edge.resolved_target, Some(target));
    assert_eq!(edge.certainty, Some(ResolutionCertainty::Certain));
    assert!(edge.candidate_targets.is_empty());
    assert!(edge.file_node_id.is_some());
    assert!(edge.line.is_some_and(|line| line >= 1));
    assert!(edge.callsite_identity.as_deref().is_some_and(|id| {
        !id.is_empty() && id.split('|').all(|part| !part.is_empty())
    }));
}
```

Name the positive tests `python_exact_structural_call_is_certain`, `go_exact_structural_call_is_certain`, `typescript_exact_structural_call_is_certain`, and `rust_exact_structural_call_is_certain`. Their fixtures are:

- Python: a receiver whose owner is established by a local annotation and a unique owner member.
- Go: a method receiver call whose declared receiver type uniquely identifies `Owner.Method` in the same package/directory.
- TypeScript: a constructor- or property-bound receiver whose import module, owner, and member uniquely identify one project callable.
- Rust: a fully qualified direct call with one exact declaration target.

Name the negative tests with the `ambiguous_structural_call_` prefix. Their fixtures must cover duplicate matching members, a shadowed receiver binding, a wildcard/dynamic import, an unresolved receiver owner, and two equal semantic candidates. Every negative remains unresolved, `Probable`, or `Uncertain`; none becomes `Certain`.

In the runtime test module, add `fresh_index_exact_structural_call_builds_one_authoritative_receipt` and `fresh_index_ambiguous_call_remains_unknown`. Both must index their fixtures through the production indexer, build exact qualified-name contracts through `build_from_store_observed`, and assert the stored edge shape through the unchanged agent admission path. The exact test requires one admitted receipt and no gap; the ambiguous test requires `SelectorAmbiguous`, `SelectorMissing`, or `DirectCallMissing` and no receipt.

- [ ] **Step 2: Run RED**

```sh
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability exact_structural_call -- --nocapture
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability ambiguous_structural_call -- --nocapture
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_exact_structural_call_builds_one_authoritative_receipt -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_ambiguous_call_remains_unknown -- --exact
```

Expected: exact indexer and runtime cases fail on absent/probable certainty or a missing resolved target; hostile negatives already fail closed and stay that way.

- [ ] **Step 3: Introduce a closed exact-evidence decision**

Keep the new types private to `resolution`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExactCallResolutionEvidence {
    SameFileDeclaredCallable,
    DeclaredReceiverOwnerMember,
    ImportedReceiverOwnerMember,
    ParserQualifiedProjectCallable,
}

#[derive(Debug, Clone, Copy)]
struct SelectedCallTarget {
    target_id: i64,
    confidence: f32,
    strategy: ResolutionStrategy,
    exact_evidence: Option<ExactCallResolutionEvidence>,
}
```

An arm may set `exact_evidence` only when parser facts and the candidate index jointly identify one callable target. `find_global_unique_readonly`, fuzzy lookup, semantic fallback, and a bare confidence score always set `None`.

- [ ] **Step 4: Preserve language facts through callsite identity**

Use the existing owner/module fields and marker families in `python.rs`, `go.rs`, and `javascript.rs`. Add a marker only when the parser proves the binding in lexical scope. Consume that closed marker in `compute_call_resolution`; do not parse arbitrary source text or module names in `candidate_selection.rs`.

For Go, a method receiver declaration is sufficient to qualify the source as `Owner.Method` even when the type declaration is in another file in the same package. For Python and TypeScript, missing, wildcard, duplicate, or shadowed import/receiver bindings remain unannotated. These rules are about language syntax, never Flask, Gin, Vite, or CodeStory paths.

- [ ] **Step 5: Separate exact certainty from heuristic confidence**

Replace the call-resolution use of the generic selected tuple with `SelectedCallTarget`. Add a call-specific update builder:

```rust
let certainty = match selected.exact_evidence {
    Some(_) => Some(ResolutionCertainty::Certain.as_str()),
    None => ResolutionCertainty::from_confidence(Some(selected.confidence))
        .map(ResolutionCertainty::as_str),
};
let candidate_payload = if selected.exact_evidence.is_some() {
    candidate_json(&[])?
} else {
    candidate_json(candidates)?
};
```

Leave import resolution on its existing builder. Do not increase `ResolutionPolicy` values and do not change `ResolutionCertainty::{CERTAIN_MIN,PROBABLE_MIN}`.

- [ ] **Step 6: Prove ambiguity and heuristic behavior stayed fail-closed**

Add unit cases beside `compute_call_resolution` for every `ExactCallResolutionEvidence` variant and for each forbidden promotion: global unique by name only, semantic fallback, tied best candidate, common unqualified call, owner without module/binding, and retained candidate alternatives.

```sh
cargo test --locked -p codestory-indexer resolution::candidate_selection::tests
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability
cargo test --locked -p codestory-indexer --test call_resolution_common_methods
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_exact_structural_call_builds_one_authoritative_receipt -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_ambiguous_call_remains_unknown -- --exact
```

Expected: exact structural rows are `Certain` with one exact target and no alternatives; heuristic and ambiguous rows never satisfy that shape.

- [ ] **Step 7: Commit**

```sh
git add crates/codestory-indexer/src/languages/python.rs crates/codestory-indexer/src/languages/go.rs crates/codestory-indexer/src/languages/javascript.rs crates/codestory-indexer/src/resolution/mod.rs crates/codestory-indexer/src/resolution/candidate_selection.rs crates/codestory-indexer/tests/exact_call_resolution_availability.rs crates/codestory-indexer/tests/call_resolution_common_methods.rs crates/codestory-runtime/src/indexed_source_call_path_v1.rs
git commit -m "resolve structurally exact calls"
```

### Task 3: Prove the unchanged strict consumer boundary end to end

**Files:**
- Verify: `crates/codestory-runtime/src/indexed_source_call_path_v1.rs:1280-3225` tests added in Task 2
- Test: `crates/codestory-agent/src/indexed_source_call_path_v1.rs` unchanged production predicate
- Test: `crates/codestory-indexer/tests/exact_call_resolution_availability.rs`

**Interfaces:**
- Consumes: freshly indexed strict edge rows from Tasks 1-2 and the existing `build_observed_indexed_source_call_path_facts` path.
- Produces: an end-to-end test proving exact selectors resolve, `diagnose_raw_call_edge` admits the stored row unchanged, source/containment checks build one authoritative receipt, and heuristic/ambiguous rows remain gaps.

- [ ] **Step 1: Audit the runtime integration assertions added in Task 2**

Confirm the feature-gated fixture indexes one exact source-to-target call and one ambiguous decoy. Build a validated contract with exact qualified-name selectors and call `build_from_store_observed`. The exact case must assert:

```rust
assert!(!observed.trace.selector_early_return);
assert!(matches!(observed.trace.steps[0].outcome, StepQualificationOutcome::Admitted { .. }));
assert_eq!(observed.built.receipts.len(), 1);
assert!(observed.built.gaps.is_empty());
```

The decoy contract must produce `SelectorAmbiguous`, `SelectorMissing`, or `DirectCallMissing`; it must never be proven by a first/prefix/fuzzy match. If either assertion is absent, return to Task 2 before implementation and add it to the RED run.

- [ ] **Step 2: Rerun the end-to-end GREEN on the accepted Task 2 commit**

```sh
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_exact_structural_call_builds_one_authoritative_receipt -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_ambiguous_call_remains_unknown -- --exact
```

Expected: the exact case emits one receipt; the ambiguous case remains unknown. The corresponding pre-implementation failures are recorded by Task 2 Step 2.

- [ ] **Step 3: Prove strict admission source is untouched**

```sh
git diff --exit-code "$(git merge-base HEAD origin/dev/codestory-0.18)" -- crates/codestory-agent/src/indexed_source_call_path_v1.rs
cargo test --locked -p codestory-agent indexed_source_call_path_v1
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1
```

Expected: no diff in the agent kernel file and both suites pass. If an independently reproduced soundness bug requires a kernel edit, stop this plan and open a separate soundness lane before proceeding. Task 3 is a verification checkpoint and creates no commit.

### Task 4: Run the indexer generalization and source gates once

**Files:**
- Verify: all remediation files from Tasks 1-3
- Do not modify: benchmark corpus, thresholds, Candidate 7 results, public v3 adapters, release files

**Interfaces:**
- Consumes: the accepted remediation head after independent adversarial review.
- Produces: focused indexer/runtime proof plus the exact-head source receipt required before merging an indexer change.

- [ ] **Step 1: Run focused formatting, check, and lint serially**

```sh
cargo fmt --all -- --check
cargo check --locked -p codestory-indexer
cargo clippy --locked -p codestory-indexer --all-targets --all-features -- -D warnings
cargo check --locked -p codestory-runtime --features proof-qualification-support
```

- [ ] **Step 2: Run the full indexer acceptance binaries**

```sh
cargo test --locked -p codestory-indexer --test fidelity_regression
cargo test --locked -p codestory-indexer --test tictactoe_language_coverage
```

These full binaries are mandatory because parser rules, qualified identities, and resolution changed. Do not replace them with name filters.

- [ ] **Step 3: Recheck the forbidden-path boundary**

```sh
forbidden_paths="$(git diff --name-only "$(git merge-base HEAD origin/dev/codestory-0.18)"...HEAD | rg '^(benchmarks/proof-availability/(corpus-v1.json|paths/|thresholds-v1.json|results/20260822T143747Z-ff5f8b53f864/)|crates/codestory-agent/src/indexed_source_call_path_v1.rs|CHANGELOG.md|\.github/workflows/|plugins/codestory/cli-version.json)' || true)"
test -z "$forbidden_paths"
```

Expected: no output. Any output blocks merge.

- [ ] **Step 4: Apply the two-revision stop rule**

For each repeated failure class—callable selector identity or raw certainty—count a failed revision only after the exact focused fixture and full affected acceptance binary ran on that revision. If two revisions of that class fail with the same reason, stop the lane, preserve the logs, and redesign the identity/evidence model. Do not add a third marker, symbol-name exception, path exception, confidence bump, or repository check.

- [ ] **Step 5: Run source stabilization once on the accepted merge-ready head**

```sh
cargo fmt --all -- --check
cargo check --workspace --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo nextest run --workspace --locked --no-fail-fast
cargo test --workspace --doc --locked
cargo test --locked -p codestory-indexer --test fidelity_regression
cargo test --locked -p codestory-indexer --test tictactoe_language_coverage
```

Run these commands once, serially, after all remediation commits are accepted. Record `git rev-parse HEAD^{commit} HEAD^{tree}` before the gate. A later source change invalidates the receipt.

### Task 5: Invalidate Candidate 7 and rerun Task 13 under a fresh ID

**Files:**
- Add under the newly generated `benchmarks/proof-availability/results/$qualification_id/`: `cases.json`, `decision.json`, `environment.json`, `failure-funnel.json`, `findings.md`, `inventory.json`, `summary.json`, and `trails.json`
- Preserve unchanged: `benchmarks/proof-availability/results/20260822T143747Z-ff5f8b53f864/**`

**Interfaces:**
- Consumes: one clean merged remediation head on `dev/codestory-0.18`, the unchanged frozen corpus and thresholds, and one locked release build of `codestory-proof-availability`.
- Produces: a new immutable Task 13 result with its own qualification ID and machine-selected A/B/C outcome. It does not revise Candidate 7.

- [ ] **Step 1: Freeze and build the changed candidate once**

```sh
git fetch origin
qualification_source="$(git rev-parse origin/dev/codestory-0.18)"
qualification_short="$(git rev-parse --short=12 "$qualification_source")"
qualification_branch="codex/proof-availability-$qualification_short"
qualification_worktree="/Users/albert/Developer/CodeStory/.worktrees/proof-availability-$qualification_short"
test -z "$(git branch --list "$qualification_branch")"
test -z "$(git ls-remote --heads origin "$qualification_branch")"
test ! -e "$qualification_worktree"
git worktree add -b "$qualification_branch" "$qualification_worktree" "$qualification_source"
cd "$qualification_worktree"
test -z "$(git status --short)"
git rev-parse HEAD^{commit} HEAD^{tree}
cargo build --release --locked -p codestory-bench --bin codestory-proof-availability
qualification_bin=target/release/codestory-proof-availability
shasum -a 256 "$qualification_bin"
```

- [ ] **Step 2: Create a fresh qualification root**

```sh
qualification_id="$(date -u +%Y%m%dT%H%M%SZ)-$(git rev-parse --short=12 HEAD)"
test "$qualification_id" != "20260822T143747Z-ff5f8b53f864"
run_root="target/proof-availability/$qualification_id"
results_root="$run_root/results"
test ! -e "$run_root"
mkdir -p "$run_root" "$results_root"
```

- [ ] **Step 3: Run the unchanged Task 13 interface once**

```sh
"$qualification_bin" materialize \
  --qualification-id "$qualification_id" \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --workspace "$run_root/workspaces" \
  --cache-root "$run_root/cache" \
  --out "$run_root/environment.json"

"$qualification_bin" run \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --thresholds benchmarks/proof-availability/thresholds-v1.json \
  --environment "$run_root/environment.json" \
  --out "$results_root/$qualification_id"

"$qualification_bin" verify \
  --corpus benchmarks/proof-availability/corpus-v1.json \
  --thresholds benchmarks/proof-availability/thresholds-v1.json \
  --results "$results_root/$qualification_id"
```

Do not rerun an unchanged product result. A harness/oracle/logic failure returns to its owning Q1 lane; a valid A/B/C result is evidence even if exact-call availability remains below the desired role threshold.

- [ ] **Step 4: Recompute the Task 14 routing and inspect the exact cases**

Require `120` positives, `312` positive steps, `240` negatives, a complete first-failure partition, zero false proofs, exact receipt reconciliation, and all four profile sizes. Recompute the Task 14 numerator from the new `cases.json`; report the new case IDs and step arithmetic without deleting or rewriting Candidate 7.

- [ ] **Step 5: Copy and commit only the new result directory**

```sh
mkdir -p "benchmarks/proof-availability/results/$qualification_id"
cp "$results_root/$qualification_id"/{cases.json,decision.json,environment.json,failure-funnel.json,findings.md,inventory.json,summary.json,trails.json} "benchmarks/proof-availability/results/$qualification_id/"
git add "benchmarks/proof-availability/results/$qualification_id"
git diff --cached --name-only | rg -v "^benchmarks/proof-availability/results/$qualification_id/" && exit 1 || true
git diff --exit-code HEAD -- benchmarks/proof-availability/results/20260822T143747Z-ff5f8b53f864
git commit -m "record refreshed proof availability decision"
```

- [ ] **Step 6: Keep Outcome C integration independent**

The new result governs only the changed candidate. Candidate 7 remains the decision consumed by the already independent Outcome C integration lane. Do not merge remediation commits into that integration branch to alter its frozen decision, and do not delay the evidence-only packet/context/search cut on this epic. Any later public proof activation follows the new result through a separate authorized integration decision.
