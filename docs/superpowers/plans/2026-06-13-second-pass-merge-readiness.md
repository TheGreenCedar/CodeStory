# Second Pass Merge Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the second-pass merge blocker by eliminating production benchmark-family branching, hardening the retrieval generalization lint against split benchmark strings, and making language evidence docs match current proof.

**Architecture:** Production packet/source-claim behavior must be source-structure driven and domain-neutral. Eval-only exact-family behavior can remain in `eval_probes.rs` and benchmark scripts, but production files must not branch on known holdout families or hide benchmark identifiers by splitting strings. Documentation should treat parser-backed graph support, real-repo corpus smoke evidence, and agent A/B packet evidence as separate claims.

**Tech Stack:** Rust 2024 workspace, Cargo targeted tests, Node.js lint script, Markdown docs.

---

## File Structure

- Modify `crates/codestory-runtime/src/agent/orchestrator.rs`: remove production `packet_terms_indicate_benchmark_*` helpers and the boolean gates that suppress generic source-derived claims for benchmark families.
- Modify `scripts/lint-retrieval-generalization.mjs`: add a compact/deobfuscated production scan that catches split benchmark-family strings such as `["s", "wr"].concat()` and `["auto", "mapper"].concat()`.
- Modify `crates/codestory-runtime/tests/retrieval_generalization_guard.rs`: add a regression proving the lint catches split benchmark-family strings in production fixtures while still allowing eval-only/test contexts.
- Modify `docs/architecture/language-support.md`: stop treating the language-expansion A/B suite as a blanket evidence floor for parser-backed graph support; call it separate agent-facing evidence with mixed current results.
- Modify `docs/testing/language-expansion-ab-report.md`: remove stale durable-surface paths and clarify that `CODESTORY_EVAL_PROBES` is test/eval-harness-only, not a release CLI knob.

---

### Task 1: Remove Production Benchmark-Family Branches And Harden Lint

**Files:**
- Modify: `crates/codestory-runtime/src/agent/orchestrator.rs`
- Modify: `scripts/lint-retrieval-generalization.mjs`
- Test: `crates/codestory-runtime/tests/retrieval_generalization_guard.rs`

- [ ] **Step 1: Add the failing lint regression**

Add this test near `linter_catches_current_holdout_literals_in_production` in `crates/codestory-runtime/tests/retrieval_generalization_guard.rs`:

```rust
#[test]
fn linter_catches_split_benchmark_family_literals_in_production() {
    let output = run_lint_with_fixture(
        r#"
pub fn leaked_split_family_markers() -> Vec<String> {
    vec![
        ["s", "wr"].concat(),
        ["use", "s", "wr"].concat(),
        ["string", "utils"].concat(),
        ["charsequence", "utils"].concat(),
        ["auto", "mapper"].concat(),
        ["source/animate", ".css"].concat(),
    ]
}
"#,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "split benchmark-family literals should fail lint; stderr={stderr}"
    );
    for expected in ["swr", "useswr", "stringutils", "automapper", "sourceanimatecss"] {
        assert!(
            stderr.to_ascii_lowercase().contains(expected),
            "lint failure should report compact benchmark marker {expected}; stderr={stderr}"
        );
    }
}
```

- [ ] **Step 2: Run the failing lint regression**

Run:

```powershell
cargo test -p codestory-runtime --test retrieval_generalization_guard linter_catches_split_benchmark_family_literals_in_production -- --nocapture
```

Expected before implementation: FAIL, because the current lint scans literal lines and string literals but does not reconstruct split benchmark-family strings.

- [ ] **Step 3: Harden the lint script**

In `scripts/lint-retrieval-generalization.mjs`, add compact patterns after `bannedLiteralPatterns`:

```javascript
const bannedCompactPatterns = [
  "swr",
  "useswr",
  "stringutils",
  "charsequenceutils",
  "automapper",
  "sourceanimatecss",
];
```

Add helpers near `scanProductionStringLiterals`:

```javascript
function compactProductionSource(text) {
  return text
    .replace(/["'`]/g, "")
    .replace(/[^a-zA-Z0-9]+/g, "")
    .toLowerCase();
}

function scanProductionCompactPatterns(filePath, marker) {
  const production = productionSource(filePath);
  const compact = compactProductionSource(production);
  if (!compact.includes(marker.toLowerCase())) {
    return [];
  }
  return [`${filePath}: compact production source contains split benchmark marker ${marker}`];
}
```

Then, inside the main scan loop and only for non-eval production files, scan `bannedCompactPatterns`:

```javascript
for (const pattern of bannedCompactPatterns) {
  const hits = scanProductionCompactPatterns(filePath, pattern);
  if (hits.length > 0) {
    console.error(
      `Banned compact benchmark marker /${pattern}/ in ${path.relative(repoRoot, filePath)} (production slice):\n${hits.join("\n")}\n`,
    );
    failed = true;
  }
}
```

Do not add `gin` as a compact marker because it is too short and causes false positives in ordinary words.

- [ ] **Step 4: Remove production benchmark-family branching**

In `crates/codestory-runtime/src/agent/orchestrator.rs`, delete these helpers entirely:

```rust
fn packet_terms_indicate_benchmark_server_route_family(terms: &[String]) -> bool { ... }
fn packet_terms_indicate_benchmark_hook_family(terms: &[String]) -> bool { ... }
fn packet_terms_indicate_benchmark_java_string_family(terms: &[String]) -> bool { ... }
fn packet_terms_indicate_benchmark_stylesheet_family(terms: &[String]) -> bool { ... }
fn packet_terms_indicate_benchmark_mapping_family(terms: &[String]) -> bool { ... }
```

In `packet_source_derived_claims_for_citation`, remove the five local `benchmark_*_family` variables and remove their negated gates. The generic source-derived claim checks should become:

```rust
if packet_terms_indicate_server_route_dispatch_flow(&prompt_terms) {
    claims.extend(packet_generic_server_route_flow_claims(symbol, source));
}

if packet_terms_indicate_hook_cache_flow(&prompt_terms) {
    claims.extend(packet_generic_hook_cache_flow_claims(symbol, source));
}

if packet_terms_indicate_string_predicate_flow(&prompt_terms) {
    claims.extend(packet_generic_string_predicate_flow_claims(symbol, source));
}

if packet_terms_indicate_stylesheet_animation_flow(&prompt_terms) {
    claims.extend(packet_generic_css_animation_flow_claims(source));
}

if packet_terms_indicate_mapper_runtime_flow(&prompt_terms) {
    claims.extend(packet_generic_mapper_runtime_claims(source));
}
```

Keep this eval-only hook unchanged:

```rust
if eval_probes_enabled() {
    claims.extend(
        crate::agent::eval_probes::source_derived_claims_for_citation(prompt, citation, source),
    );
}
```

- [ ] **Step 5: Verify task**

Run:

```powershell
cargo test -p codestory-runtime --test retrieval_generalization_guard linter_catches_split_benchmark_family_literals_in_production -- --nocapture
cargo test -p codestory-runtime exact_family_source_claims_require_eval_probes packet_supported_claims_generic_source_claims_are_domain_neutral_without_eval_probes -- --nocapture
node scripts\lint-retrieval-generalization.mjs
rg -n "packet_terms_indicate_benchmark|benchmark_.*_family|\\[\"s\", \"wr\"\\]|\\[\"auto\", \"mapper\"\\]|\\[\"string\", \"utils\"\\]" crates\codestory-runtime\src\agent\orchestrator.rs scripts\lint-retrieval-generalization.mjs
git diff --check
```

Expected: all tests/lints pass; `rg` has no matches in `orchestrator.rs` and only intentional lint-script pattern definitions if any.

- [ ] **Step 6: Commit**

Run:

```powershell
git add crates\codestory-runtime\src\agent\orchestrator.rs scripts\lint-retrieval-generalization.mjs crates\codestory-runtime\tests\retrieval_generalization_guard.rs
git commit -m "remove production benchmark family gates"
```

---

### Task 2: Make Language Evidence Docs Match Current Proof

**Files:**
- Modify: `docs/architecture/language-support.md`
- Modify: `docs/testing/language-expansion-ab-report.md`

- [ ] **Step 1: Fix the language support matrix wording**

In `docs/architecture/language-support.md`, replace the parser-backed graph row's evidence-floor cell so it no longer treats the A/B suite as blanket proof:

```markdown
fidelity lab, tictactoe coverage, raw graph contracts, targeted rule/resolution suites, and the opt-in OSS language corpus; agent-facing A/B evidence is separate and currently mixed
```

Immediately after the matrix, add:

```markdown
Agent-facing packet/search quality is a separate claim from parser-backed graph
support. The current language-expansion A/B report records a mixed full
18-language result and a stronger packet-eligible slice; do not use that report
as blanket promotion proof for every parser-backed language.
```

- [ ] **Step 2: Fix stale durable surface paths**

In `docs/testing/language-expansion-ab-report.md`, remove durable-surface entries
for files that are not present in the current checkout. The maintained list
should be exactly:

```markdown
- `scripts/codestory-agent-ab-benchmark.mjs`
- `scripts/codestory-agent-ab-score.mjs`
- `scripts/codestory-language-holdout-integrity.mjs`
- `scripts/tests/codestory-agent-ab-analyzer.test.mjs`
- `benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json`
- `docs/testing/oss-language-corpus.md`
```

- [ ] **Step 3: Clarify eval-probe diagnostics**

In the eval-only diagnostic snippet, replace the placeholder diagnostic command
comment with a concrete test/eval-harness example:

```powershell
# Only Rust tests and explicit benchmark/eval harnesses can enable this switch;
# release CLI/runtime builds ignore it.
$env:CODESTORY_EVAL_PROBES = "1"
cargo test -p codestory-runtime --test retrieval_generalization_guard -- --nocapture
Remove-Item Env:CODESTORY_EVAL_PROBES
```

- [ ] **Step 4: Verify docs**

Run:

```powershell
$task2StalePattern = @(
  ("codestory-agent-ab-analyzer" + ".mjs"),
  ("language-expansion-holdout/" + "repos.json"),
  "language-expansion agent A/B suite",
  "placeholder diagnostic command"
) -join "|"
rg -n $task2StalePattern docs\architecture\language-support.md docs\testing\language-expansion-ab-report.md
node scripts\codestory-language-holdout-integrity.mjs
git diff --check
```

Expected: `rg` has no matches for stale paths/wording; integrity script passes.

- [ ] **Step 5: Commit**

Run:

```powershell
git add docs\architecture\language-support.md docs\testing\language-expansion-ab-report.md
git commit -m "clarify language evidence limits"
```

---

### Task 3: Final Readiness Repair And Evidence

**Files:**
- Modify: `docs/testing/codestory-e2e-stats-log.md` only if the ignored repo-scale e2e gate is rerun successfully.

- [ ] **Step 1: Run targeted serialized verification**

Run:

```powershell
cargo check --workspace
cargo test -p codestory-runtime --test retrieval_generalization_guard -- --nocapture
cargo test -p codestory-runtime exact_family_source_claims_require_eval_probes packet_supported_claims_generic_source_claims_are_domain_neutral_without_eval_probes -- --nocapture
node scripts\lint-retrieval-generalization.mjs
node scripts\codestory-language-holdout-integrity.mjs
git diff --check origin/main...HEAD
```

Expected: all pass.

- [ ] **Step 2: Repair active sidecar readiness**

Run:

```powershell
target\release\codestory-cli.exe retrieval bootstrap --project . --format json
target\release\codestory-cli.exe retrieval index --project . --refresh full --format json
target\release\codestory-cli.exe ready --project . --format json
target\release\codestory-cli.exe doctor --project . --format json
```

Expected: `ready` reports both `local_navigation` and `agent_packet_search` as `ready`; `doctor` reports `retrieval_mode: "full"` and semantic contract `ok`.

- [ ] **Step 3: Run repo-scale e2e only if preparing another commit**

If any files changed after Task 2, run:

```powershell
cargo build --release -p codestory-cli
$env:CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES = "1"
cargo test -p codestory-cli --test codestory_repo_e2e_stats -- --ignored --nocapture
Remove-Item Env:CODESTORY_ALLOW_SKIP_REAL_REPO_DRILL_CASES
```

Expected: pass. If this emits a fresh stats row for the new HEAD, append it to `docs/testing/codestory-e2e-stats-log.md` before committing.

- [ ] **Step 4: Final branch review**

Run:

```powershell
git status --short --branch
git diff --stat origin/main...HEAD
git diff --check origin/main...HEAD
```

Expected: branch clean; only intentional changes over `origin/main`; no whitespace errors.
