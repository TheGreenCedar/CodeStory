# Verifiable Research and Technology Proposal

## 1. Core Problem Analysis

The committed remediation made packet evidence typed and non-benchmark-shaped, but the promotion proof still fails because sufficiency, compact packet budgeting, dynamic-symbol naming, and benchmark provenance are not aligned. The next slice should not add more fixture strings; it should make resolved evidence roles, language-tier policy, and promotion telemetry carry the remaining proof.

## 2. Verifiable Technology Recommendations

| Technology/Pattern | Rationale & Evidence |
|---|---|
| **Resolved code-intelligence indexes as the proof tier** | SCIP is a language-agnostic protocol for code indexes that power go-to-definition, references, and implementations [cite:1]. GitLab's LSIF lifecycle treats code intelligence as an artifact generated for a project, processed, uploaded, and then used to display definitions and references [cite:2]. CodeStory should keep resolved graph/source evidence as the only sufficiency proof tier for parser-backed languages, with retrieval and dense search only discovering candidates [cite:1]. |
| **Concise repository maps as routing context, not proof** | Aider's repo map is deliberately concise and sends important classes, functions, signatures, and definition lines to the model with each change request [cite:3]. CodeStory should use compact packet maps to orient the agent, but sufficiency must come from typed evidence roles attached to citations, not from map text alone [cite:3]. |
| **Symbol-level exploration with explicit language capability boundaries** | Serena exposes symbol-level retrieval such as find-symbol, file outline, references, declarations, and implementations so agents can inspect code structure without reading whole files [cite:4]. Serena also documents that some capabilities depend on the language server or JetBrains backend, so CodeStory should publish parser-backed, structural-source, and lexical-only tiers instead of pretending every language has identical graph proof [cite:4]. |
| **Agent-computer-interface feedback loops** | SWE-agent attributes its results partly to LM-centric commands and feedback formats for browsing, editing, and executing code [cite:5]. It also found a bounded file viewer more useful than raw full-file output [cite:5]. CodeStory should keep follow-up commands short and role-specific, and compact packets should prioritize missing proof roles before verbose context [cite:5]. |
| **Persistent local code memory with graph-backed queries** | Codebase-Memory MCP advertises persistent knowledge-graph indexing, broad language coverage, low-token queries, SQLite, tree-sitter, and MCP integration [cite:6]. CodeStory should borrow the local-first, graph-query posture without adding another server in this slice; the current sidecar stack is enough if coverage semantics are corrected [cite:6]. |
| **Ground-truth retrieval evaluation plus functional gates** | CodeRAG-Bench emphasizes rigorous retrieval evaluation with ground-truth documents and execution-style final evaluation, and it reports that retrievers still struggle on harder repository-level tasks [cite:7]. SWE-bench evaluates agents by checking fail-to-pass tests after repository edits, so promotion gates should distinguish product quality failures from harness bookkeeping failures and keep exact benchmark expectations inside manifests [cite:8]. |

## 3. Browsed Sources

- [1] https://github.com/scip-code/scip - SCIP README for language-agnostic code-intelligence indexes.
- [2] https://docs.gitlab.com/development/code_intelligence/ - GitLab LSIF artifact lifecycle.
- [3] https://aider.chat/docs/repomap.html - Aider repository map behavior.
- [4] https://github.com/oraios/serena - Serena retrieval and language-support behavior.
- [5] https://swe-agent.com/0.7/background/aci/ - SWE-agent Agent-Computer Interface notes.
- [6] https://github.com/DeusData/codebase-memory-mcp - Codebase-Memory MCP README and project claims.
- [7] https://code-rag-bench.github.io/ - CodeRAG-Bench benchmark goals and retrieval findings.
- [8] https://www.swebench.com/original.html - SWE-bench evaluation contract.

## 4. Current Local Evidence From 2026-06-18

Current HEAD/worktree branch is `codex/packet-answer-quality-hardening-review`. The stale cache-provenance issue from the June 17 subset is fixed or superseded; it is not the active blocker.

| Artifact | Generated | Result | Publishable status |
|---|---:|---|---|
| `target/agent-benchmark/language-expansion-proof-full-form-command-shapes/packet-runtime-summary.md` | `2026-06-18T12:03:23.059Z` | 108 runs, 108 success, 108 quality, 108 sufficient, 9 cold SLA misses. | Not publishable because cold SLA misses remain. |
| `target/agent-benchmark/language-expansion-publishable-full-form-command-shapes/packet-runtime-summary.md` | `2026-06-18T12:23:54.418Z` | 108 runs, 108 success, 106 quality, 107 sufficient, 1 partial, 8 cold SLA misses. | Failed publishable gate. |

Latest publishable blockers: `apache-commons-lang` cold SLA 3/3; `redis` cold SLA 3/3; `AutoMapper` cold SLA 1/3; `dart-http` cold SLA 1/3; `square-okio` cold quality 2/3; `Alamofire` cold quality 2/3 and 1 partial sufficiency.

`cargo test -p codestory-runtime --lib` is still required verification, but it is not confirmed passed and must not be claimed as passed.

## 5. Superseded Local Evidence From 2026-06-17 Targeted Subset

The targeted rerun used the committed branch with seven failure-cluster tasks, cold and warm packet modes, `--jobs 4`, serial sidecar prep, and artifacts under `C:\Users\alber\.codex\tmp\deep-research\20260617-110319-codestory-packet-promotion-gaps\deliverables\failure-subset-packet-runtime`.

| Cluster | Evidence | Implication |
|---|---|---|
| Harness provenance | All 14 rows reported retrieval `full`, freshness `fresh`, indexed `true`, and retrieval shadow `full`, but `cache_policy` was `unprepared-cache-blocked`. | Superseded by the June 18 artifacts; cache provenance is no longer the active publishable blocker. |
| Express quality | Express found all expected files but only `app.route` among six expected symbols and no expected claims. | JavaScript prototype/assignment symbols and route response claims need generic source-backed alias handling, not benchmark strings. |
| HTML false sufficient | HTML sufficiency was `sufficient` while quality found only 50% of expected files, 50% of symbols, and 25% of claims. | Generic `source evidence`, `event loop`, and unrelated role claims are too eligible for structural HTML sufficiency. |
| SQL false partial | SQL quality was 100% across files, symbols, claims, and citations, but sufficiency was `partial` because synthetic SQL source-scan citations could not satisfy role-backed claims. | Structural source scans must be eligible for declared structural roles such as table definitions and foreign keys. |
| CSS over-required | CSS quality was 100%, but sufficiency required `html app shell`, `module script entry`, and `interactive element styles`. | Stylesheet animation flow must be split from HTML template flow. |
| Python probe overhang | Python quality was 100%, but sufficiency required `handler dispatch` and `transport send`. | Required probes must discover evidence; they should not block sufficiency when equivalent roles are already covered. |
| fmt/Swift compact pressure | fmt and Swift quality passed, but compact packets were partial due truncation and omitted packet payload. | Compact budget should reserve proof roles first and should not require standard budget when required coverage is already present. |
