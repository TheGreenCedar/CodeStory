# Exact Call Resolution Availability Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Increase exact call-path availability by repairing upstream callable identity and parser-backed call resolution while leaving the proof kernel's strict admission predicate unchanged.

**Architecture:** Keep exact selector lookup fail-closed in `codestory-runtime` and keep raw-edge admission in `codestory-agent` unchanged. Repair the owning `codestory-indexer` seams instead: give callable definitions stable declaration-first canonical identities, carry parser-proven receiver/import facts into an explicit exact-resolution decision, and persist `Certain` only when those structural facts identify one target. Name-only, fuzzy, semantic, ambiguous, and incomplete paths remain heuristic or unresolved.

**Tech Stack:** Rust 2024, tree-sitter graph rules, SQLite-backed CodeStory graph storage, Cargo integration tests, the production-dark `codestory-proof-availability` harness.

**Spec:** [CodeStory v3 retrieval/proof separation](../codestory-v3-retrieval-proof-separation.md)

## Global Constraints

- Start the remediation branch from the live `origin/dev/codestory-0.18`; do not use `dev/codestory-next`, `main`, or any 0.17 release branch as its base or proof source.
- Use branch `codex/exact-call-resolution-availability`. In its delegated worktree, run `node scripts/codex-worktree-setup.mjs --project "$(pwd -P)" --intended-base-ref origin/dev/codestory-0.18 --pr-head-ref codex/exact-call-resolution-availability --branch-head-proof` before source inspection or edits, and retain its printed base/head/proof target in the owner issue.
- Candidate 7 is qualification `20260822T143747Z-ff5f8b53f864`, source commit `ff5f8b53f864225244281a7d76382d50589b130e`, source tree `0926587beb10d475c59453059af57bd56adb2643`, and selected outcome `keep_proof_dark`. Its checked-in result directory is immutable.
- Keep `diagnose_raw_call_edge` and `admit_raw_call_edge` strict: stored `CALL`, stored `Certain`, exact effective source and target, `resolved_target == Some(expected_target)`, no candidate alternatives, valid file/line/canonical callsite identity, callable containment, and publication-bound source remain required. Change that predicate only in a separately reviewed soundness fix that reproduces an incorrect admission or rejection independently of this availability target.
- Do not lower a confidence or certainty threshold, map `Probable` or `Uncertain` to `Certain`, infer certainty from a score, add fuzzy selector fallback, or choose the first matching node.
- Do not add checks for repository names, paths from the four qualification repositories, Candidate 7 case IDs, expected symbol names, or benchmark answer shapes to production code. Test comments may cite case IDs; implementation must be language- and syntax-contract based.
- Do not edit `benchmarks/proof-availability/corpus-v1.json`, `benchmarks/proof-availability/paths/**`, `benchmarks/proof-availability/thresholds-v1.json`, or `benchmarks/proof-availability/results/20260822T143747Z-ff5f8b53f864/**`.
- Outcome C integration is independent. The evidence-only packet/context/search cut may proceed using Candidate 7 without waiting for, merging from, or consuming this remediation branch only while its kernel, indexer, corpus, thresholds, and qualification calculation remain unchanged. This lane must not edit Outcome C integration files.
- Any implemented kernel, indexer, corpus, threshold, or qualification-calculation change invalidates Candidate 7. The changed candidate has no A/B/C decision until a fresh qualification ID completes the entire Task 13 materialize/run/verify/review/result-commit sequence. Never compare changed output to Candidate 7 as one run or let Outcome C cite Candidate 7 after a qualification-calculation change.
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
| Language-backed receiver/import facts | `crates/codestory-indexer/src/lib.rs:4442-4460,4982-4991,8204-8538,9949-9982`, `crates/codestory-indexer/src/languages/python.rs`, `go.rs`, and `javascript.rs`; graph rules under `crates/codestory-indexer/rules/` | Emit closed syntax-backed call-fact markers through the existing callsite identity; ambiguous, dynamic, wildcard, or shadowed bindings remain unmarked. |
| Call candidate selection and persisted certainty | `crates/codestory-indexer/src/resolution/candidate_selection.rs:3-501` and `crates/codestory-indexer/src/resolution/mod.rs:487-717,1154-1199,1740-2665` | Separate exact structural evidence from heuristic strategy and stamp `Certain` only for the former. |
| Qualification measurement | `crates/codestory-bench/src/bin/codestory_proof_availability/` and `benchmarks/proof-availability/**` | Read-only during implementation; rerun the frozen Task 13 interface after merge under a new ID. |

The smallest safe owner boundary is therefore `codestory-indexer`, with end-to-end non-regression coverage in `codestory-runtime`. A runtime selector fallback would make a malformed or stale selector appear exact. A kernel relaxation would admit evidence that the index did not establish. Neither is part of this remediation.

### Task 1: Stabilize callable definition identity ahead of placeholders

**Files:**
- Modify: `crates/codestory-indexer/src/lib.rs:10226-10573`
- Modify: `crates/codestory-indexer/src/cache.rs:1-160`
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

Create `exact_call_resolution_availability.rs` with these imports and complete helpers; do not invent a second fixture API:

```rust
use std::{collections::HashMap, fs};

use codestory_contracts::{
    events::EventBus,
    graph::{Edge, EdgeKind, Node, NodeId, NodeKind, ResolutionCertainty},
};
use codestory_indexer::WorkspaceIndexer;
use codestory_store::Store as Storage;
use codestory_workspace::{BuildMode, RefreshInfo};
use tempfile::tempdir;

fn index_project(files: &[(&str, &str)]) -> anyhow::Result<Storage> {
    let dir = tempdir()?;
    let root = dir.path();
    let mut files_to_index = Vec::with_capacity(files.len());
    for (relative_path, source) in files {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, source)?;
        files_to_index.push(path);
    }
    let mut storage = Storage::new_in_memory()?;
    WorkspaceIndexer::new(root.to_path_buf()).run_incremental(
        &mut storage,
        &RefreshInfo {
            mode: BuildMode::Incremental,
            files_to_index,
            files_to_remove: Vec::new(),
            existing_file_ids: HashMap::new(),
        },
        &EventBus::new(),
        None,
    )?;
    Ok(storage)
}

fn callable_matches(
    storage: &Storage,
    relative_path: &str,
    qualified_name: &str,
) -> anyhow::Result<Vec<Node>> {
    let file = storage
        .get_files()?
        .into_iter()
        .find(|file| file.path.ends_with(relative_path))
        .unwrap_or_else(|| panic!("missing indexed file {relative_path}"));
    Ok(storage
        .get_nodes()?
        .into_iter()
        .filter(|node| node.file_node_id == Some(NodeId(file.id)))
        .filter(|node| matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD))
        .filter(|node| node.qualified_name.as_deref() == Some(qualified_name))
        .collect())
}

fn assert_unique_callable_canonical_id(
    storage: &Storage,
    relative_path: &str,
    qualified_name: &str,
    canonical_id: &str,
) -> anyhow::Result<()> {
    let matches = callable_matches(storage, relative_path, qualified_name)?;
    assert_eq!(matches.len(), 1, "expected one exact callable: {matches:?}");
    assert_eq!(matches[0].canonical_id.as_deref(), Some(canonical_id));
    let all_with_id = storage
        .get_nodes()?
        .into_iter()
        .filter(|node| node.canonical_id.as_deref() == Some(canonical_id))
        .collect::<Vec<_>>();
    assert_eq!(all_with_id.len(), 1, "canonical ID must name one node");
    assert!(matches!(all_with_id[0].kind, NodeKind::FUNCTION | NodeKind::METHOD));
    Ok(())
}
```

Add these exact fixtures; they are syntax-only reductions of the selector failure classes and contain no qualification repository path or symbol:

```rust
#[test]
fn python_callable_definition_keeps_zero_ordinal_when_same_named_calls_surround_it() -> anyhow::Result<()> {
    let storage = index_project(&[(
        "workflow.py",
        "def before():\n    target()\n\ndef target():\n    return 1\n\ndef after():\n    target()\n",
    )])?;
    assert_unique_callable_canonical_id(&storage, "workflow.py", "target", "workflow.py:target#0")
}

#[test]
fn go_method_keeps_owner_qualified_identity_without_a_local_type_declaration() -> anyhow::Result<()> {
    let storage = index_project(&[(
        "deprecated.go",
        "package sample\nfunc (c *Context) BindWith() { c.MustBindWith() }\n",
    )])?;
    assert_unique_callable_canonical_id(
        &storage,
        "deprecated.go",
        "Context.BindWith",
        "deprecated.go:Context.BindWith#0",
    )
}

#[test]
fn typescript_exported_function_is_one_exact_file_qualified_match() -> anyhow::Result<()> {
    let storage = index_project(&[(
        "src/fs.ts",
        "export function isDirectory(path: string) { return tryStatSync(path) }\nfunction tryStatSync(path: string) { return path }\n",
    )])?;
    assert_unique_callable_canonical_id(
        &storage,
        "src/fs.ts",
        "isDirectory",
        "src/fs.ts:isDirectory#0",
    )
}

#[test]
fn duplicate_definitions_keep_distinct_ordinals_and_remain_ambiguous() -> anyhow::Result<()> {
    let storage = index_project(&[(
        "duplicate.py",
        "def target():\n    return 1\n\ndef target():\n    return 2\n",
    )])?;
    let mut ids = callable_matches(&storage, "duplicate.py", "target")?
        .into_iter()
        .filter_map(|node| node.canonical_id)
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, ["duplicate.py:target#0", "duplicate.py:target#1"]);
    Ok(())
}
```

Cite the Flask selector cases, `gin-go-06`, `gin-go-28`, and `vite-ts-js-26-l1` in test comments only.

- [ ] **Step 2: Run RED**

```sh
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability python_callable_definition_keeps_zero_ordinal_when_same_named_calls_surround_it -- --exact
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability go_method_keeps_owner_qualified_identity_without_a_local_type_declaration -- --exact
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability typescript_exported_function_is_one_exact_file_qualified_match -- --exact
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability duplicate_definitions_keep_distinct_ordinals_and_remain_ambiguous -- --exact
```

Expected: one or more positive tests fail because a reference line receives the declaration ordinal or the exact callable qualification is absent; the duplicate test either fails or proves the current ambiguity boundary. If every positive test already passes, record Task 1 as a disproved source hypothesis, run the exact no-change assertion below, skip Steps 3-7, and continue to Task 2. Do not invent another fixture or add runtime fallback.

```sh
git diff --exit-code -- crates/codestory-indexer/src/lib.rs crates/codestory-indexer/src/cache.rs crates/codestory-indexer/rules/python.scm crates/codestory-indexer/rules/go.scm crates/codestory-indexer/rules/javascript.scm crates/codestory-indexer/rules/typescript.graph.scm crates/codestory-indexer/rules/tsx.graph.scm crates/codestory-indexer/tests/exact_call_resolution_availability.rs
```

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

Replace `declaration_ordinals` with this declaration-first implementation and pass `canonical_roles` from `canonicalize_nodes_with_file_identity`:

```rust
fn declaration_ordinals(
    nodes: &[Node],
    canonical_roles: &HashMap<NodeId, CanonicalNodeRole>,
) -> HashMap<String, BTreeMap<u32, usize>> {
    let mut lines_by_name = HashMap::<String, (BTreeSet<u32>, BTreeSet<u32>)>::new();
    for node in nodes {
        if preserved_canonical_id(node).is_some() || !node_needs_declaration_ordinal(node) {
            continue;
        }
        let qualified_name = node
            .qualified_name
            .clone()
            .unwrap_or_else(|| node.serialized_name.clone());
        let line = node.start_line.unwrap_or(1);
        let identity_bearing = matches!(
            canonical_roles.get(&node.id),
            Some(
                CanonicalNodeRole::Definition
                    | CanonicalNodeRole::Declaration
                    | CanonicalNodeRole::ForwardDeclaration
            )
        );
        let entry = lines_by_name.entry(qualified_name).or_default();
        if identity_bearing {
            entry.0.insert(line);
            entry.1.remove(&line);
        } else if !entry.0.contains(&line) {
            entry.1.insert(line);
        }
    }
    lines_by_name
        .into_iter()
        .map(|(name, (identity_lines, other_lines))| {
            let ordinals = identity_lines
                .into_iter()
                .chain(other_lines)
                .enumerate()
                .map(|(ordinal, line)| (line, ordinal))
                .collect();
            (name, ordinals)
        })
        .collect()
}
```

This keeps `file:qualified#0` attached to the sole real callable definition while retaining distinct IDs for unresolved placeholders. Do not discard or merge placeholder nodes at this stage.

- [ ] **Step 5: Invalidate stale parser artifacts**

Set `INDEX_ARTIFACT_CACHE_VERSION` to `4` and update the adjacent comment to name declaration-first callable identity. Add this exact unit beside the existing key tests in `cache.rs`:

```rust
#[test]
fn test_artifact_cache_key_uses_definition_role_version() {
    let config = crate::get_language_for_ext("python").expect("python config");
    let key = build_index_artifact_cache_key(
        Path::new("/workspace"),
        Path::new("workflow.py"),
        b"def target():\n    return 1\n",
        &config,
        None,
        false,
        true,
    )
    .expect("portable cache key");
    assert!(key.starts_with("v4:"));
    assert_ne!(key, key.replacen("v4:", "v3:", 1));
}
```

- [ ] **Step 6: Run GREEN and identity regression lanes**

```sh
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability
cargo test --locked -p codestory-indexer canonical
cargo test --locked -p codestory-indexer cache::tests::test_artifact_cache_key_uses_definition_role_version -- --exact
```

Expected: the new exact-identity tests pass; duplicate definitions remain ambiguous; same-name calls no longer displace the real declaration's ordinal; copied version-3 artifacts miss.

- [ ] **Step 7: Commit**

```sh
git add crates/codestory-indexer/src/lib.rs crates/codestory-indexer/src/cache.rs crates/codestory-indexer/rules/python.scm crates/codestory-indexer/rules/go.scm crates/codestory-indexer/rules/javascript.scm crates/codestory-indexer/rules/typescript.graph.scm crates/codestory-indexer/rules/tsx.graph.scm crates/codestory-indexer/tests/exact_call_resolution_availability.rs
git commit -m "stabilize exact callable identities"
```

### Task 2: Persist certainty only from exact structural call evidence

**Files:**
- Modify: `crates/codestory-indexer/src/lib.rs:67-73,4442-4465,8204-8538,8974-9220,9949-9982,15320-15375`
- Modify: `crates/codestory-indexer/src/languages/python.rs`
- Modify: `crates/codestory-indexer/src/languages/go.rs`
- Modify: `crates/codestory-indexer/src/languages/javascript.rs`
- Modify: `crates/codestory-indexer/rules/python.scm`
- Modify: `crates/codestory-indexer/rules/go.scm`
- Modify: `crates/codestory-indexer/rules/javascript.scm`
- Modify: `crates/codestory-indexer/rules/typescript.graph.scm`
- Modify: `crates/codestory-indexer/rules/tsx.graph.scm`
- Modify: `crates/codestory-indexer/rules/rust.graph.scm`
- Modify: `crates/codestory-indexer/src/resolution/mod.rs:487-717,1154-1199,1740-2665`
- Modify: `crates/codestory-indexer/src/resolution/candidate_selection.rs:3-501`
- Modify: `crates/codestory-indexer/tests/exact_call_resolution_availability.rs`
- Modify: `crates/codestory-indexer/tests/call_resolution_common_methods.rs`
- Modify: `crates/codestory-runtime/src/indexed_source_call_path_v1.rs:1280-3225` tests only

**Interfaces:**
- Consumes: a closed `ParserExactCallFact` marker, existing owner/module markers, exact imported-callable markers, unique-only candidate-index lookups, and current `ResolutionStrategy` telemetry.
- Produces: private `ExactCallResolutionEvidence` and `SelectedCallTarget`; one persisted resolved target with `ResolutionCertainty::Certain` and an empty alternative set only when one allowed parser fact and one allowed unique lookup meet the decision table below.
- Preserves: the existing confidence values and `ResolutionCertainty::from_confidence` behavior for same-name, global-unique, fuzzy, semantic, incomplete, and ambiguous fallbacks.

- [ ] **Step 1: Add exact fixture constants and strict-edge RED tests**

Extend `exact_call_resolution_availability.rs` with these exact fixtures:

```rust
const PYTHON_EXACT: &[(&str, &str)] = &[
    ("worker.py", "class Worker:\n    def run(self):\n        return 1\n"),
    ("caller.py", "from worker import Worker\n\ndef caller(worker: Worker):\n    return worker.run()\n"),
];
const GO_EXACT: &[(&str, &str)] = &[
    ("worker.go", "package sample\ntype Worker struct{}\nfunc (w *Worker) Run() {}\n"),
    ("caller.go", "package sample\nfunc Caller(worker *Worker) { worker.Run() }\n"),
];
const TYPESCRIPT_EXACT: &[(&str, &str)] = &[
    ("worker.ts", "export class Worker { run(): number { return 1 } }\n"),
    ("caller.ts", "import { Worker } from './worker'\nexport function caller(worker: Worker) { return worker.run() }\n"),
];
const RUST_EXACT: &[(&str, &str)] = &[(
    "src/lib.rs",
    "pub mod worker { pub fn run() {} }\npub fn caller() { worker::run(); }\n",
)];

fn one_callable(storage: &Storage, file: &str, terminal_name: &str) -> anyhow::Result<Node> {
    let file_row = storage
        .get_files()?
        .into_iter()
        .find(|row| row.path.ends_with(file))
        .unwrap_or_else(|| panic!("missing indexed file {file}"));
    let mut matches = storage
        .get_nodes()?
        .into_iter()
        .filter(|node| node.file_node_id == Some(NodeId(file_row.id)))
        .filter(|node| matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD))
        .filter(|node| {
            node.serialized_name == terminal_name
                || node.qualified_name.as_deref().is_some_and(|name| {
                    name.rsplit(['.', ':']).find(|part| !part.is_empty()) == Some(terminal_name)
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "expected one callable: {matches:?}");
    Ok(matches.remove(0))
}

fn one_call_edge(storage: &Storage, source: NodeId, target: NodeId) -> anyhow::Result<Edge> {
    let mut matches = storage
        .get_edges()?
        .into_iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .filter(|edge| edge.effective_source() == source)
        .filter(|edge| edge.effective_target() == target)
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "expected one exact CALL edge: {matches:?}");
    Ok(matches.remove(0))
}

fn assert_strict_exact_call(edge: &Edge, source: &Node, target: &Node) {
    assert_eq!(edge.kind, EdgeKind::CALL);
    assert_eq!(edge.effective_source(), source.id);
    assert_eq!(edge.effective_target(), target.id);
    assert_eq!(edge.resolved_target, Some(target.id));
    assert_eq!(edge.certainty, Some(ResolutionCertainty::Certain));
    assert!(edge.candidate_targets.is_empty());
    assert_eq!(edge.file_node_id, source.file_node_id);
    let line = edge.line.filter(|line| *line >= 1).expect("positive line");
    let identity = edge.callsite_identity.as_deref().expect("callsite identity");
    let pre_marker = identity.split_once('|').map_or(identity, |(head, _)| head);
    let fields = pre_marker.split(':').collect::<Vec<_>>();
    assert_eq!(fields.len(), 4);
    assert_eq!(fields[0].parse::<i64>().unwrap(), edge.file_node_id.unwrap().0);
    assert_eq!(fields[1].parse::<u32>().unwrap(), line);
    fields[2].parse::<u32>().expect("column or ordinal");
    assert_eq!(fields[3].parse::<i64>().unwrap(), edge.target.0);
}
```

Implement `python_exact_structural_call_is_certain`, `go_exact_structural_call_is_certain`, `typescript_exact_structural_call_is_certain`, and `rust_exact_structural_call_is_certain` by passing the corresponding constant to `index_project`, resolving the named `caller`/`Caller` and `run`/`Run` nodes with `one_callable`, selecting the edge with `one_call_edge`, and calling `assert_strict_exact_call`.

Add these complete negative fixtures and name every test with the `ambiguous_structural_call_` prefix:

```rust
const PYTHON_WILDCARD: &[(&str, &str)] = &[
    ("worker.py", "class Worker:\n    def run(self):\n        return 1\n"),
    ("caller.py", "from worker import *\n\ndef caller(worker):\n    return worker.run()\n"),
];
const GO_UNRESOLVED_OWNER: &[(&str, &str)] = &[(
    "caller.go",
    "package sample\nfunc Caller(worker interface{}) { worker.Run() }\n",
)];
const TYPESCRIPT_SHADOWED: &[(&str, &str)] = &[
    ("worker.ts", "export class Worker { run(): number { return 1 } }\n"),
    ("caller.ts", "import { Worker } from './worker'\nexport function caller(Worker: any) { const worker = new Worker(); return worker.run() }\n"),
];
const RUST_AMBIGUOUS: &[(&str, &str)] = &[(
    "src/lib.rs",
    "mod left { pub fn run() {} }\nmod right { pub fn run() {} }\npub fn caller() { run(); }\n",
)];
```

For each negative, index once, find the caller and every `CALL` row on its line, and assert that no row satisfies all of `resolved_target.is_some()`, `certainty == Certain`, and `candidate_targets.is_empty()`. The Python wildcard, Go unresolved owner, TypeScript shadow, and Rust duplicate name remain unresolved, `Probable`, or `Uncertain`; none may be promoted by global uniqueness, suffix matching, or semantic score.

In the runtime test module, preserve the existing one-file helper and add this complete multi-file constructor:

```rust
struct SourceBuiltFixture {
    _root: TempDir,
    root: PathBuf,
    source_path: PathBuf,
    source_paths: HashMap<String, PathBuf>,
    store: Store,
    publication: IndexPublicationRecord,
    project_id: String,
}

fn source_built_fixture(source: &str) -> SourceBuiltFixture {
    source_built_fixture_files(&[("src/lib.rs", source)])
}

fn source_built_fixture_files(files: &[(&str, &str)]) -> SourceBuiltFixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let mut files_to_index = Vec::with_capacity(files.len());
    let mut source_paths = HashMap::new();
    for (relative_path, source) in files {
        let path = root.join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        files_to_index.push(path.clone());
        source_paths.insert((*relative_path).to_owned(), path);
    }
    let mut store = Store::new_in_memory().unwrap();
    WorkspaceIndexer::new(root.clone())
        .run_incremental(
            &mut store,
            &RefreshInfo {
                mode: BuildMode::Incremental,
                files_to_index,
                files_to_remove: Vec::new(),
                existing_file_ids: HashMap::new(),
            },
            &EventBus::new(),
            None,
        )
        .unwrap();
    SourceBuiltFixture {
        _root: temp,
        root: root.clone(),
        source_path: source_paths.values().next().unwrap().clone(),
        source_paths,
        store,
        publication: IndexPublicationRecord {
            generation: 1,
            generation_id: "source-built-generation-1".to_owned(),
            run_id: "source-built-run-1".to_owned(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        },
        project_id: project_identity_v3(&root).project_id,
    }
}
```

Define the same four positive fixture arrays locally in the runtime test module. Add the four tests and exact assertions specified in Task 3: `fresh_index_strict_receipt_binds_callsite_containment_and_hash`, `fresh_index_repeated_vertex_path_requires_two_distinct_edges`, `fresh_index_hash_and_containment_fail_closed`, and `fresh_index_missing_or_ambiguous_relation_is_unknown_without_certified_absence`. These tests are written now, before Step 3 source changes, so their positive assertions join the RED run below.

- [ ] **Step 2: Run RED**

```sh
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability exact_structural_call -- --nocapture
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability ambiguous_structural_call -- --nocapture
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_strict_receipt_binds_callsite_containment_and_hash -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_repeated_vertex_path_requires_two_distinct_edges -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_hash_and_containment_fail_closed -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_missing_or_ambiguous_relation_is_unknown_without_certified_absence -- --exact
```

Expected: one or more exact rows fail on absent/probable certainty or a missing resolved target; every hostile negative already fails closed and stays that way. Save the failing assertion text for each positive language before source changes.

- [ ] **Step 3: Add the closed parser-fact grammar and construction seams**

Add these crate-private types and exact marker grammar in `lib.rs`:

```rust
pub(crate) const EXACT_CALL_FACT_PREFIX: &str = "exact-call-fact:v1:";
pub(crate) const EXACT_CALL_MODULE_PREFIX: &str = "exact-call-module:";
pub(crate) const EXACT_CALL_IMPORTED_NAME_PREFIX: &str = "exact-call-imported-name:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParserExactCallFact {
    BareIdentifier,
    QualifiedCallable,
    DeclaredReceiver,
    ImportedReceiver,
    ImportedCallable,
}

impl ParserExactCallFact {
    pub(crate) fn graph_attr(value: &str) -> Option<Self> {
        match value {
            "bare_identifier" => Some(Self::BareIdentifier),
            "qualified_callable" => Some(Self::QualifiedCallable),
            _ => None,
        }
    }

    pub(crate) const fn marker(self) -> &'static str {
        match self {
            Self::BareIdentifier => "exact-call-fact:v1:bare-identifier",
            Self::QualifiedCallable => "exact-call-fact:v1:qualified-callable",
            Self::DeclaredReceiver => "exact-call-fact:v1:declared-receiver",
            Self::ImportedReceiver => "exact-call-fact:v1:imported-receiver",
            Self::ImportedCallable => "exact-call-fact:v1:imported-callable",
        }
    }
}

impl ManualReceiverCallSpec {
    fn mark_exact_receiver_fact(&mut self) {
        if self.binding_marker.is_some() || self.owner_name.trim().is_empty() {
            return;
        }
        let fact = if self.owner_module.as_deref().is_some_and(|module| {
            !module.trim().is_empty() && !module.contains('|') && !module.contains('*')
        }) {
            ParserExactCallFact::ImportedReceiver
        } else {
            ParserExactCallFact::DeclaredReceiver
        };
        self.binding_marker = Some(fact.marker().to_owned());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportedCallableCallSpec {
    pub(crate) local_name: String,
    pub(crate) imported_name: String,
    pub(crate) module_name: String,
    pub(crate) line: u32,
    pub(crate) col: u32,
}
```

In each affected graph rule, add `call_fact = "bare_identifier"` only to the grammar arm whose callee is one bare identifier. Add `call_fact = "qualified_callable"` only to Rust's `scoped_identifier` call arm. In the graph-edge attribute loop in `lib.rs`, parse only `ParserExactCallFact::graph_attr`, then append its marker after `ensure_callsite_identity`. Unknown `call_fact` values add no marker.

At the end of `python::receiver_call_specs`, `go::receiver_call_specs`, and `javascript::receiver_call_specs`, call `mark_exact_receiver_fact` only on specs whose existing collector proved a lexical owner binding. Do not call it for implicit/dynamic receivers, wildcard imports, multiple visible bindings, shadowed names, inferred global fallback, or a spec whose existing `binding_marker` is populated. The existing `annotate_receiver_call_placeholder_owner` and `append_manual_receiver_call_placeholder_edge` functions carry the marker. Extend `remove_generic_call_placeholders` to retain parts beginning with either `RECEIVER_BINDING_CALLSITE_PREFIX` or `EXACT_CALL_FACT_PREFIX`, so a parser fact survives replacement by a resolved manual edge.

Add `javascript::imported_callable_call_specs(tree: &Tree, source: &str) -> Vec<ImportedCallableCallSpec>`. It accepts only a static named `import { imported as local } from "./module"` or `import { imported } from "./module"`, requires one lexical binding, rejects namespace/default/wildcard/dynamic imports and any later same-scope declaration of `local`, and returns every bare `local(...)` call after the import with its one-based line and column. `annotate_exact_imported_callable_calls` in `lib.rs` must match one unresolved `CALL` edge by line, column, and exact local target name; only then append, in order, `ImportedCallable.marker()`, `exact-call-module:{module_name}`, and `exact-call-imported-name:{imported_name}`. Reject empty values or values containing `|`; zero or multiple matching edges remain unmarked.

- [ ] **Step 4: Add unique-only candidate APIs and the closed decision table**

In `resolution/mod.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UniqueExactCandidate {
    Missing,
    Unique(i64),
    Ambiguous,
}

fn unique_exact_candidate(ids: impl IntoIterator<Item = i64>) -> UniqueExactCandidate {
    let mut ids = ids.into_iter().collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    match ids.as_slice() {
        [] => UniqueExactCandidate::Missing,
        [id] => UniqueExactCandidate::Unique(*id),
        _ => UniqueExactCandidate::Ambiguous,
    }
}
```

Implement `CandidateIndex::find_unique_exact_same_file_declared_callable`, `find_unique_exact_same_module_declared_callable`, `find_unique_exact_imported_callable`, and `find_unique_exact_qualified_callable`. Each filters callable candidates by exact serialized or exact qualified name plus the required file/module binding, collects all matching IDs, and calls `unique_exact_candidate`. These methods must not call `first_in_file`, `first_in_module`, a suffix map, an ASCII-folded fallback, global-name fallback, semantic lookup, or a confidence threshold.

Keep these types private to `resolution`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExactCallResolutionEvidence {
    SameFileDeclaredCallable,
    SameModuleDeclaredCallable,
    DeclaredReceiverOwnerMember,
    ImportedReceiverOwnerMember,
    ImportedCallableBinding,
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

Apply this table literally; it is the complete set of promotions:

| Parser fact | Required marker data | Required unique lookup | Exact evidence | Persisted result |
| --- | --- | --- | --- | --- |
| `BareIdentifier` | no owner/module/imported-name marker | exact declaration in caller file | `SameFileDeclaredCallable` | `Certain`, exact target, empty alternatives |
| `BareIdentifier` | no owner/module/imported-name marker | exact declaration under caller module/package | `SameModuleDeclaredCallable` | `Certain`, exact target, empty alternatives |
| `DeclaredReceiver` | exactly one non-empty `receiver-owner`, no `receiver-module` | exact `Owner.Member` in caller file, or for Go exactly one in caller directory/package | `DeclaredReceiverOwnerMember` | `Certain`, exact target, empty alternatives |
| `ImportedReceiver` | exactly one non-empty owner and one static non-wildcard module | exact imported `Owner.Member` under that module path | `ImportedReceiverOwnerMember` | `Certain`, exact target, empty alternatives |
| `ImportedCallable` | exactly one static module and one imported name | exact imported callable under that module path | `ImportedCallableBinding` | `Certain`, exact target, empty alternatives |
| `QualifiedCallable` | syntactically qualified raw target | exact full qualified callable | `ParserQualifiedProjectCallable` | `Certain`, exact target, empty alternatives |
| missing/duplicate/conflicting fact markers, missing data, or `UniqueExactCandidate::{Missing,Ambiguous}` | any | any | none | unresolved or existing heuristic result; never promoted |
| current first/suffix/global/fuzzy/semantic route or confidence alone | any | any | none | existing certainty mapping and candidate payload unchanged |

If more than one exact-fact marker, owner, module, or imported-name value appears in the callsite identity, parse the fact as invalid and take the non-promoting row. `candidate_selection.rs` consumes parsed typed facts; it must not parse source text, repository paths, or benchmark names.

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

Add unit cases beside `compute_call_resolution` for every row in the table. Construct `UnresolvedEdgeRow` callsite identities from the literal markers above and `CandidateIndex::from_nodes` candidates with exact duplicate IDs where ambiguity is required. Cover all six evidence variants and these forbidden promotions: first same-file match with a duplicate, first same-module match with a duplicate, global unique by name only, semantic fallback, tied best candidate, common unqualified call without `BareIdentifier`, owner without `DeclaredReceiver`, module without `ImportedReceiver`, shadowed/wildcard imported call without `ImportedCallable`, conflicting exact-fact markers, and retained candidate alternatives.

```sh
cargo test --locked -p codestory-indexer resolution::candidate_selection::tests
cargo test --locked -p codestory-indexer --test exact_call_resolution_availability
cargo test --locked -p codestory-indexer --test call_resolution_common_methods
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_strict_receipt_binds_callsite_containment_and_hash -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_repeated_vertex_path_requires_two_distinct_edges -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_hash_and_containment_fail_closed -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_missing_or_ambiguous_relation_is_unknown_without_certified_absence -- --exact
```

Expected: exact structural rows are `Certain` with one exact target and no alternatives; heuristic and ambiguous rows never satisfy that shape.

- [ ] **Step 7: Commit**

```sh
git add crates/codestory-indexer/src/lib.rs crates/codestory-indexer/src/languages/python.rs crates/codestory-indexer/src/languages/go.rs crates/codestory-indexer/src/languages/javascript.rs crates/codestory-indexer/rules/python.scm crates/codestory-indexer/rules/go.scm crates/codestory-indexer/rules/javascript.scm crates/codestory-indexer/rules/typescript.graph.scm crates/codestory-indexer/rules/tsx.graph.scm crates/codestory-indexer/rules/rust.graph.scm crates/codestory-indexer/src/resolution/mod.rs crates/codestory-indexer/src/resolution/candidate_selection.rs crates/codestory-indexer/tests/exact_call_resolution_availability.rs crates/codestory-indexer/tests/call_resolution_common_methods.rs crates/codestory-runtime/src/indexed_source_call_path_v1.rs
git commit -m "resolve structurally exact calls"
```

### Task 3: Prove the unchanged strict consumer boundary end to end

**Files:**
- Verify: `crates/codestory-runtime/src/indexed_source_call_path_v1.rs:1280-3225` tests written RED and committed in Task 2
- Test: `crates/codestory-agent/src/indexed_source_call_path_v1.rs` unchanged production predicate
- Test: `crates/codestory-indexer/tests/exact_call_resolution_availability.rs`

**Interfaces:**
- Consumes: freshly indexed strict edge rows from Tasks 1-2 and the existing `build_observed_indexed_source_call_path_facts` path.
- Produces: end-to-end evidence for the complete strict contract: canonical four-part callsite identity bound to edge file/line/raw target; callable source/target and unique-smallest containment; exact working-tree hash; distinct receipts and edges when vertices repeat; and `Unknown` without production `CertifiedAbsence` for missing, ambiguous, hash-drifted, or containment-ambiguous relations.

- [ ] **Step 1: Bind every positive receipt field to the fresh source fixture**

Add this complete assertion helper and call it for each of the four positive fixture arrays from Task 2:

```rust
fn assert_strict_source_receipt(
    fixture: &SourceBuiltFixture,
    relative_path: &str,
    source: &Node,
    target: &Node,
    expected_line: &str,
    receipt: &IndexedCallEdgeReceipt,
) {
    assert!(matches!(source.kind, NodeKind::FUNCTION | NodeKind::METHOD));
    assert!(matches!(target.kind, NodeKind::FUNCTION | NodeKind::METHOD));
    assert_eq!(receipt.source.pinned.node_id, source.id.0.to_string());
    assert_eq!(receipt.target.pinned.node_id, target.id.0.to_string());

    let edge_id = receipt.receipt.edge_id.parse::<i64>().unwrap();
    let edge = fixture
        .store
        .get_edges()
        .unwrap()
        .into_iter()
        .find(|edge| edge.id == EdgeId(edge_id))
        .expect("receipt edge exists in fresh index");
    let admitted = diagnose_raw_call_edge(&edge, source.id, target.id)
        .expect("unchanged strict raw admission accepts edge");
    assert_eq!(admitted.file_node_id, source.file_node_id.unwrap());
    assert_eq!(admitted.line, edge.line.unwrap());
    assert_eq!(admitted.raw_target, edge.target);
    assert_eq!(receipt.callsite_identity, admitted.callsite_identity);

    let pre_marker = receipt
        .callsite_identity
        .split_once('|')
        .map_or(receipt.callsite_identity.as_str(), |(head, _)| head);
    let fields = pre_marker.split(':').collect::<Vec<_>>();
    assert_eq!(fields.len(), 4);
    assert_eq!(fields[0].parse::<i64>().unwrap(), edge.file_node_id.unwrap().0);
    assert_eq!(fields[1].parse::<u32>().unwrap(), edge.line.unwrap());
    assert_eq!(fields[2].parse::<u32>().unwrap(), admitted.column_or_ordinal);
    assert_eq!(fields[3].parse::<i64>().unwrap(), edge.target.0);

    assert_eq!(receipt.containment.file_node_id, source.file_node_id.unwrap());
    assert_eq!(receipt.containment.owner_node_id, source.id);
    assert_eq!(receipt.containment.start_line, source.start_line.unwrap());
    assert_eq!(receipt.containment.end_line, source.end_line.unwrap());

    let source_path = fixture.source_paths.get(relative_path).unwrap();
    let bytes = fs::read(source_path).unwrap();
    let expected_hash = sha256_hex(&bytes);
    assert_eq!(receipt.line_window.indexed_sha256, expected_hash);
    assert_eq!(receipt.line_window.observed_sha256, expected_hash);
    assert_eq!(receipt.line_window.anchor_line, edge.line.unwrap());
    assert_eq!(receipt.line_window.text, expected_line);
    let (start, end, line) = complete_line(&bytes, edge.line.unwrap()).unwrap();
    assert_eq!(receipt.line_window.byte_start, start);
    assert_eq!(receipt.line_window.byte_end, end);
    assert_eq!(receipt.line_window.text, line);
}
```

`fresh_index_strict_receipt_binds_callsite_containment_and_hash` iterates this exact table:

| Files | Source selector | Target selector | Expected complete line |
| --- | --- | --- | --- |
| `PYTHON_EXACT` | `caller.py` / `caller` | `worker.py` / `run` | `    return worker.run()\n` |
| `GO_EXACT` | `caller.go` / `Caller` | `worker.go` / `Run` | `func Caller(worker *Worker) { worker.Run() }\n` |
| `TYPESCRIPT_EXACT` | `caller.ts` / `caller` | `worker.ts` / `run` | `export function caller(worker: Worker) { return worker.run() }\n` |
| `RUST_EXACT` | `src/lib.rs` / `caller` | `src/lib.rs` / `run` | `pub fn caller() { worker::run(); }\n` |

For each row, create `source_built_fixture_files`, resolve exactly one source and target callable in their named files, build `validated_contract(canonical_id(source), &[canonical_id(target)])`, call `build_from_store_observed`, and require: selector early-return false; one `Admitted` step naming exactly one edge; one fact; one receipt; no gaps/unavailable; `assert_strict_source_receipt`; and `check_built_call_path_integration(...).disposition() == ContractProven` with that receipt.

- [ ] **Step 2: Prove repeated vertices require distinct indexed edges**

Use this exact source:

```rust
let fixture = source_built_fixture(
    "pub fn alpha(reenter: bool) { if reenter { beta(); } }\n\
     pub fn beta() { alpha(false); }\n",
);
```

Build the exact path `alpha -> beta -> alpha`. Require two admitted trace steps, two receipts, two unique `receipt_id` values, two unique `edge_id` values, and pinned vertices `[alpha, beta, alpha]` in order. `ContractProven` must contain both receipt refs. Replace the second receipt ID with the first and separately replace the second edge ID with the first; both mutated fact sets must return `Unknown(ReceiptOrEdgeAlreadyUsed { step_index: 1 })`, never `ContractProven`.

- [ ] **Step 3: Prove hash and unique-smallest containment failures fail closed**

For the hash case, index `RUST_EXACT`, resolve its exact contract, then replace the working-tree caller line with `pub fn caller() { worker::missing(); }\n` without reindexing. `build_from_store_observed` must produce zero facts/receipts and `UnavailableReason::SourceNotBoundToPublication`; checked integration must be `Unavailable`, not `ContractProven` or `ContractRefuted`.

For containment, create a fresh `RUST_EXACT` fixture, then insert `node(i64::MAX - 1, NodeKind::METHOD, "intruder", "intruder-id", source_file_id, source.start_line.unwrap(), source.end_line.unwrap())` and `projection(source_file_id, i64::MAX - 1, source.start_line.unwrap(), source.end_line.unwrap())`. The equal-smallest owner makes containment ambiguous. Require zero facts/receipts, `FactBuildGap::EdgeContainmentUnproven { step_index: 0 }`, and checked `Unknown`.

- [ ] **Step 4: Prove missing and ambiguous relations stay Unknown without CertifiedAbsence**

Add this helper:

```rust
fn assert_unknown_without_certified_absence(result: &CheckedBuiltCallPathIntegration) {
    assert!(result
        .built_facts()
        .facts
        .iter()
        .all(|fact| !matches!(fact, VerifiedProofFact::CertifiedAbsence(_))));
    assert!(matches!(result.disposition(), ProofDisposition::Unknown { .. }));
    assert!(!matches!(
        result.disposition(),
        ProofDisposition::ContractRefuted {
            refutation: Refutation::CertifiedAbsence { .. },
            ..
        }
    ));
}
```

The missing fixture is `pub fn caller() {}\npub fn target() {}\n` with contract `caller -> target`; require `DirectCallMissing`. The ambiguous fixture is `RUST_AMBIGUOUS` with an exact caller selector and exact `left::run` target selector; require no strict edge to the chosen target and `DirectCallMissing`. Run both through checked integration and `assert_unknown_without_certified_absence`. Production code must not construct `VerifiedProofFact::CertifiedAbsence`; its enum/refutation arms remain under the existing test-only cfg.

- [ ] **Step 5: Rerun the exact strict-contract GREEN tests on the accepted Task 2 commit**

```sh
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_strict_receipt_binds_callsite_containment_and_hash -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_repeated_vertex_path_requires_two_distinct_edges -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_hash_and_containment_fail_closed -- --exact
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1::tests::fresh_index_missing_or_ambiguous_relation_is_unknown_without_certified_absence -- --exact
cargo check --locked -p codestory-runtime --no-default-features --features proof-qualification-support
```

Expected: every positive fixture satisfies every strict field; repeated vertices use distinct receipts/edges; hash and containment hostility fail closed; missing/ambiguous relations are `Unknown`; and the production-like feature build compiles without a production absence provider. The corresponding pre-implementation failures are recorded by Task 2 Step 2.

- [ ] **Step 6: Prove strict admission source is untouched**

```sh
git diff --exit-code "$(git merge-base HEAD origin/dev/codestory-0.18)" -- crates/codestory-agent/src/indexed_source_call_path_v1.rs
cargo test --locked -p codestory-agent indexed_source_call_path_v1
cargo test --locked -p codestory-runtime --features proof-qualification-support indexed_source_call_path_v1
```

Expected: no diff in the agent kernel file and both suites pass. If an independently reproduced soundness bug requires a kernel edit, stop this plan and open a separate soundness lane before proceeding. Task 3 audits tests committed in Task 2 and creates no commit.

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

### Task 5: Push, review, and land the exact accepted remediation head

**Files:**
- Modify: no source file; this task changes only the existing owner issue, focused PR, Project 4 membership, and `dev/codestory-0.18` through the reviewed merge

**Interfaces:**
- Consumes: the clean exact head/tree accepted by Task 4 and the existing open owner epic titled `Improve upstream exact call resolution from the Candidate 7 failure census`.
- Produces: one guarded, checked, independently approved PR whose exact reviewed head is an ancestor of the fetched `origin/dev/codestory-0.18` landing head.

- [ ] **Step 1: Freeze the reviewed branch and resolve the owner issue**

```sh
test "$(git branch --show-current)" = "codex/exact-call-resolution-availability"
test -z "$(git status --short)"
reviewed_head="$(git rev-parse HEAD^{commit})"
reviewed_tree="$(git rev-parse HEAD^{tree})"
owner_issue_json="$(gh issue list --repo TheGreenCedar/CodeStory --state open --search '"Improve upstream exact call resolution from the Candidate 7 failure census" in:title' --json number,title,url)"
owner_issue_json="$(printf '%s' "$owner_issue_json" | jq '[.[] | select(.title == "Improve upstream exact call resolution from the Candidate 7 failure census")]')"
test "$(printf '%s' "$owner_issue_json" | jq 'length')" -eq 1
owner_issue="$(printf '%s' "$owner_issue_json" | jq -r '.[0].number')"

child_title="Implement parser-backed exact call resolution"
child_issue_json="$(gh issue list --repo TheGreenCedar/CodeStory --state open --search '"Implement parser-backed exact call resolution" in:title' --json number,title,url)"
child_issue_json="$(printf '%s' "$child_issue_json" | jq --arg title "$child_title" '[.[] | select(.title == $title)]')"
case "$(printf '%s' "$child_issue_json" | jq 'length')" in
  0)
    child_issue_url="$(gh issue create --repo TheGreenCedar/CodeStory --title "$child_title" --body "Refs #$owner_issue

Implement Tasks 1-5 of the exact-call-resolution availability remediation plan. Preserve strict proof admission, prohibit repository-specific heuristics, run the exact source gates, and land the independently reviewed head on dev/codestory-0.18 before a fresh Task 13 qualification.")"
    child_issue="$(gh issue view "$child_issue_url" --repo TheGreenCedar/CodeStory --json number --jq .number)"
    ;;
  1) child_issue="$(printf '%s' "$child_issue_json" | jq -r '.[0].number')" ;;
  *) exit 1 ;;
esac
```

Post the exact head/tree and completed Task 4 commands to the owner issue with the repository helper and a temporary body outside the repository. Do not claim a gate that did not run:

```sh
status_body="$(mktemp)"
printf 'Exact remediation head: `%s`\nTree: `%s`\n\nTask 4 gates completed: `cargo fmt --all -- --check`, `cargo check --workspace --locked`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo nextest run --workspace --locked --no-fail-fast`, `cargo test --workspace --doc --locked`, `cargo test --locked -p codestory-indexer --test fidelity_regression`, and `cargo test --locked -p codestory-indexer --test tictactoe_language_coverage`.\n' "$reviewed_head" "$reviewed_tree" > "$status_body"
node scripts/github-status-comment.mjs --issue "$owner_issue" --body-file "$status_body"
rm "$status_body"
```

- [ ] **Step 2: Push that exact head and open or update one focused PR**

```sh
git push --set-upstream origin "$reviewed_head:refs/heads/codex/exact-call-resolution-availability"
test "$(git ls-remote origin refs/heads/codex/exact-call-resolution-availability | cut -f1)" = "$reviewed_head"
pr_number="$(gh pr list --repo TheGreenCedar/CodeStory --state open --head codex/exact-call-resolution-availability --base dev/codestory-0.18 --json number --jq '.[0].number // empty')"
pr_body="$(mktemp)"
printf 'Closes #%s\n\nRefs #%s\n\nImplements the reviewed exact-call-resolution plan while preserving strict admission and Candidate 7.\n\nReviewed head: %s\nReviewed tree: %s\n' "$child_issue" "$owner_issue" "$reviewed_head" "$reviewed_tree" > "$pr_body"
if test -z "$pr_number"; then
  pr_url="$(gh pr create --repo TheGreenCedar/CodeStory --base dev/codestory-0.18 --head codex/exact-call-resolution-availability --title 'Improve exact call resolution availability' --body-file "$pr_body")"
  pr_number="$(gh pr view "$pr_url" --repo TheGreenCedar/CodeStory --json number --jq .number)"
else
  gh pr edit "$pr_number" --repo TheGreenCedar/CodeStory --base dev/codestory-0.18 --body-file "$pr_body" --add-label saga
fi
rm "$pr_body"
test "$(gh pr view "$pr_number" --repo TheGreenCedar/CodeStory --json headRefOid --jq .headRefOid)" = "$reviewed_head"
gh project item-add 4 --owner TheGreenCedar --url "$(gh issue view "$owner_issue" --repo TheGreenCedar/CodeStory --json url --jq .url)"
gh project item-add 4 --owner TheGreenCedar --url "$(gh issue view "$child_issue" --repo TheGreenCedar/CodeStory --json url --jq .url)"
gh project item-add 4 --owner TheGreenCedar --url "$(gh pr view "$pr_number" --repo TheGreenCedar/CodeStory --json url --jq .url)"
```

- [ ] **Step 3: Require exact-head independent review and required checks**

Give the adversarial reviewer the exact Task 2 decision table plus the four Task 3 hostile mutations. Its output is limited to counterexamples or acceptance evidence. After review, re-read the remote SHA, target, mergeability, approval, and required checks:

```sh
test "$(gh pr view "$pr_number" --repo TheGreenCedar/CodeStory --json headRefOid --jq .headRefOid)" = "$reviewed_head"
test "$(gh pr view "$pr_number" --repo TheGreenCedar/CodeStory --json baseRefName --jq .baseRefName)" = "dev/codestory-0.18"
test "$(gh pr view "$pr_number" --repo TheGreenCedar/CodeStory --json reviewDecision --jq .reviewDecision)" = "APPROVED"
gh pr checks "$pr_number" --repo TheGreenCedar/CodeStory --required --watch --fail-fast
test "$(gh pr view "$pr_number" --repo TheGreenCedar/CodeStory --json headRefOid --jq .headRefOid)" = "$reviewed_head"
```

Any pushed commit revokes the review and Task 4 source receipt; return to Task 4 rather than merging.

- [ ] **Step 4: Merge and prove the reviewed head landed before qualification**

```sh
gh pr merge "$pr_number" --repo TheGreenCedar/CodeStory --merge --delete-branch
git fetch origin dev/codestory-0.18
landed_head="$(git rev-parse origin/dev/codestory-0.18^{commit})"
landed_tree="$(git rev-parse origin/dev/codestory-0.18^{tree})"
git merge-base --is-ancestor "$reviewed_head" "$landed_head"
test "$(gh pr view "$pr_number" --repo TheGreenCedar/CodeStory --json state --jq .state)" = "MERGED"
test -n "$(gh pr view "$pr_number" --repo TheGreenCedar/CodeStory --json mergedAt --jq '.mergedAt // empty')"
```

Record `reviewed_head`, `reviewed_tree`, `landed_head`, and `landed_tree`. Task 6 must use exactly `landed_head`; a later source or qualification-calculation commit invalidates this transition and blocks the rerun.

### Task 6: Invalidate Candidate 7 and rerun Task 13 under a fresh ID

**Files:**
- Add under the newly generated `benchmarks/proof-availability/results/$qualification_id/`: `cases.json`, `decision.json`, `environment.json`, `failure-funnel.json`, `findings.md`, `inventory.json`, `summary.json`, and `trails.json`
- Preserve unchanged: `benchmarks/proof-availability/results/20260822T143747Z-ff5f8b53f864/**`

**Interfaces:**
- Consumes: one clean merged remediation head on `dev/codestory-0.18`, the unchanged frozen corpus and thresholds, and one locked release build of `codestory-proof-availability`.
- Produces: a new immutable Task 13 result with its own qualification ID and machine-selected A/B/C outcome. It does not revise Candidate 7.

- [ ] **Step 1: Create and prepare the fresh qualification worktree**

```sh
git fetch origin
qualification_source="$(git rev-parse origin/dev/codestory-0.18)"
test "$qualification_source" = "$landed_head"
qualification_short="$(git rev-parse --short=12 "$qualification_source")"
qualification_branch="codex/proof-availability-$qualification_short"
qualification_worktree="/Users/albert/Developer/CodeStory/.worktrees/proof-availability-$qualification_short"
test -z "$(git branch --list "$qualification_branch")"
test -z "$(git ls-remote --heads origin "$qualification_branch")"
test ! -e "$qualification_worktree"
git worktree add --detach "$qualification_worktree" "$qualification_source"
cd "$qualification_worktree"
test -z "$(git status --short)"
setup_output="$(node scripts/codex-worktree-setup.mjs --project "$qualification_worktree" --intended-base-ref origin/dev/codestory-0.18 --pr-head-ref "$qualification_source" --branch-head-proof)"
printf '%s\n' "$setup_output"
test "$(git rev-parse HEAD^{commit})" = "$qualification_source"
test "$(git rev-parse origin/dev/codestory-0.18^{commit})" = "$qualification_source"
test -z "$(git status --short)"
git rev-parse HEAD^{commit} HEAD^{tree}
```

The setup output must name `qualification_source` as intended base, child head, PR head/proof target, report a ready repository map, and contain no base/head mismatch. Stop before building if any printed identity differs.

- [ ] **Step 2: Build the changed candidate once**

```sh
cargo build --release --locked -p codestory-bench --bin codestory-proof-availability
qualification_bin=target/release/codestory-proof-availability
shasum -a 256 "$qualification_bin"
```

- [ ] **Step 3: Create a fresh qualification root**

```sh
qualification_id="$(date -u +%Y%m%dT%H%M%SZ)-$(git rev-parse --short=12 HEAD)"
test "$qualification_id" != "20260822T143747Z-ff5f8b53f864"
run_root="target/proof-availability/$qualification_id"
results_root="$run_root/results"
test ! -e "$run_root"
mkdir -p "$run_root" "$results_root"
```

- [ ] **Step 4: Run the unchanged Task 13 interface once**

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

Do not rerun an unchanged product result. A harness/oracle/logic failure or any qualification-calculation change invalidates Candidate 7 and this attempted result, returns to its owning Q1 lane, and requires a fresh qualification ID plus the complete Task 13 sequence after the correction. Until that finishes, no A/B/C outcome—including Outcome C—may cite Candidate 7. A valid unchanged-calculation A/B/C result is evidence even if exact-call availability remains below the desired role threshold.

- [ ] **Step 5: Recompute the Task 14 routing and inspect the exact cases**

Require `120` positives, `312` positive steps, `240` negatives, a complete first-failure partition, zero false proofs, exact receipt reconciliation, and all four profile sizes. Recompute the Task 14 numerator from the new `cases.json`; report the new case IDs and step arithmetic without deleting or rewriting Candidate 7.

- [ ] **Step 6: Copy and commit only the new result directory**

```sh
git switch -c "$qualification_branch"
mkdir -p "benchmarks/proof-availability/results/$qualification_id"
cp "$results_root/$qualification_id"/{cases.json,decision.json,environment.json,failure-funnel.json,findings.md,inventory.json,summary.json,trails.json} "benchmarks/proof-availability/results/$qualification_id/"
git add "benchmarks/proof-availability/results/$qualification_id"
git diff --cached --name-only | rg -v "^benchmarks/proof-availability/results/$qualification_id/" && exit 1 || true
git diff --exit-code HEAD -- benchmarks/proof-availability/results/20260822T143747Z-ff5f8b53f864
git commit -m "record refreshed proof availability decision"
```

- [ ] **Step 7: Review and land the complete fresh Task 13 result**

Push `qualification_branch`, open one guarded PR into `dev/codestory-0.18`, reference the owner epic, add both to Project 4, and require independent raw-artifact review plus required checks on the exact result commit:

```sh
result_head="$(git rev-parse HEAD^{commit})"
result_tree="$(git rev-parse HEAD^{tree})"
test -z "$(git status --short)"
git push --set-upstream origin "$qualification_branch"
test "$(git ls-remote origin "refs/heads/$qualification_branch" | cut -f1)" = "$result_head"
owner_issue_json="$(gh issue list --repo TheGreenCedar/CodeStory --state open --search '"Improve upstream exact call resolution from the Candidate 7 failure census" in:title' --json number,title,url)"
owner_issue_json="$(printf '%s' "$owner_issue_json" | jq '[.[] | select(.title == "Improve upstream exact call resolution from the Candidate 7 failure census")]')"
test "$(printf '%s' "$owner_issue_json" | jq 'length')" -eq 1
owner_issue="$(printf '%s' "$owner_issue_json" | jq -r '.[0].number')"
result_pr_url="$(gh pr create --repo TheGreenCedar/CodeStory --base dev/codestory-0.18 --head "$qualification_branch" --title "Record proof availability $qualification_id" --body "Closes #$owner_issue

Fresh Task 13 result for the landed remediation candidate.

Qualification: $qualification_id
Result head: $result_head
Result tree: $result_tree")"
result_pr="$(gh pr view "$result_pr_url" --repo TheGreenCedar/CodeStory --json number --jq .number)"
gh project item-add 4 --owner TheGreenCedar --url "$(gh pr view "$result_pr" --repo TheGreenCedar/CodeStory --json url --jq .url)"
test "$(gh pr view "$result_pr" --repo TheGreenCedar/CodeStory --json headRefOid,baseRefName --jq '.headRefOid + ":" + .baseRefName')" = "$result_head:dev/codestory-0.18"
"$qualification_bin" verify --corpus benchmarks/proof-availability/corpus-v1.json --thresholds benchmarks/proof-availability/thresholds-v1.json --results "benchmarks/proof-availability/results/$qualification_id"
gh pr checks "$result_pr" --repo TheGreenCedar/CodeStory --required --watch --fail-fast
test "$(gh pr view "$result_pr" --repo TheGreenCedar/CodeStory --json reviewDecision --jq .reviewDecision)" = "APPROVED"
test "$(gh pr view "$result_pr" --repo TheGreenCedar/CodeStory --json headRefOid --jq .headRefOid)" = "$result_head"
gh pr merge "$result_pr" --repo TheGreenCedar/CodeStory --merge --delete-branch
git fetch origin dev/codestory-0.18
test "$(git ls-tree -r --name-only origin/dev/codestory-0.18 -- "benchmarks/proof-availability/results/$qualification_id" | wc -l | tr -d ' ')" -eq 8
```

The independent reviewer must inspect `cases.json`, `failure-funnel.json`, receipt reconciliation, negative mutations, four profile sizes, and the A/B/C arithmetic rather than reviewing only `decision.json`. This review/result-commit/merge is part of Task 13; the rerun is incomplete without it.

- [ ] **Step 8: Keep Outcome C integration independent without using invalid evidence**

The new result governs the changed candidate. Candidate 7 remains consumable only by an Outcome C integration lineage whose kernel, indexer, corpus, thresholds, and qualification calculation are byte-for-byte unchanged from Candidate 7. Do not merge remediation commits into that frozen integration branch or delay its evidence-only packet/context/search cut. If the qualification calculation changes anywhere, Candidate 7 is invalid for every lineage and Outcome C must wait for the fresh Task 13 result. Any later public proof activation follows the new result through a separate authorized integration decision.
