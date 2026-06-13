# Verifiable Research and Remediation Proposal

## 1. Core Problem Analysis

The reviewed branch improved parser-backed language coverage, but it also left benchmark-family knowledge in the production packet path and split language-support truth across discovery, indexing, semantic document text, CLI output, docs, and tests. The fix must remove benchmark steering from product behavior, make language support claims derive from one shared contract, and add diagnostics where retrieval evidence is unresolved instead of pretending silence is success.

## 2. Evidence Sources

| ID | Source | Claim Supported | Confidence |
| --- | --- | --- | --- |
| E1 | `C:/Users/alber/Downloads/review_gemini_3_1.md` | Reviewer found hardcoded Chinook, MDN, Okio, Monolog, and Alamofire benchmark-family branches in `orchestrator.rs` and recommended deleting production static citation steering. | High |
| E2 | `C:/Users/alber/Downloads/review_codex.md` | Reviewer found production exact-family steering enabled by default, incomplete semantic language labels, split support registries, packet sidecar unresolved-candidate opacity, filtered `files` count ambiguity, and hardcoded holdout assumptions. | High |
| E3 | `C:/Users/alber/Downloads/review_gemini_3_5.md` | Reviewer found monolithic modules, string-based parameter parsing, cross-file receiver-call resolution risk, and proposed dynamic parser loading as a longer architecture direction. | Medium |
| E4 | `crates/codestory-runtime/src/agent/orchestrator.rs:75` | `CODESTORY_PACKET_EXACT_FAMILY_STEERING` is defined in production runtime code. | High |
| E5 | `crates/codestory-runtime/src/agent/orchestrator.rs:83` | `packet_exact_family_steering_enabled()` defaults to `true` when the env var is unset. | High |
| E6 | `crates/codestory-runtime/src/agent/orchestrator.rs:409` | The product packet path appends Chinook, MDN, Okio, Monolog, and Alamofire static family citations when steering is enabled. | High |
| E7 | `crates/codestory-runtime/src/agent/orchestrator.rs:6075` | Static citation functions inject negative synthetic node IDs, hardcoded file paths, scores, and provenance rather than graph-resolved evidence. | High |
| E8 | `scripts/lint-retrieval-generalization.mjs` | The generalization lint bans several repo-specific literals, but its `bannedPatterns` list does not yet include the new review-named benchmark families. | High |
| E9 | `crates/codestory-runtime/src/semantic_doc_text.rs:6` | Semantic document language labeling uses a smaller hardcoded extension map that omits currently claimed parser-backed languages such as Go, Ruby, PHP, C#, Kotlin, Swift, Dart, and Bash. | High |
| E10 | `crates/codestory-indexer/src/lib.rs:10931` | Runtime support profiles are defined in the indexer through `language_support_profile_for_ext` and `language_support_profile_for_language_name`. | High |
| E11 | `crates/codestory-workspace/src/lib.rs:607` | Workspace discovery has a broader extension universe than semantic document labeling, including Vue, Astro, cshtml, Lua, PowerShell, Sass, Less, and others. | High |
| E12 | `docs/architecture/language-support.md:7` | Current docs call the indexer profile functions the support-claim source of truth, which does not feed all runtime surfaces. | High |
| E13 | `crates/codestory-runtime/src/agent/retrieval_primary.rs:282` | Single sidecar search rejects unresolved-only sidecar candidates. | High |
| E14 | `crates/codestory-runtime/src/agent/retrieval_primary.rs:483` | Packet batch rejection ignores `_resolved_hits`, making unresolved-only subqueries indistinguishable from empty subqueries. | High |
| E15 | `crates/codestory-runtime/src/agent/retrieval_primary.rs:1710` | A test currently locks in packet batch tolerance for unresolved full-mode candidates. | High |
| E16 | `crates/codestory-runtime/src/lib.rs:8691` | `indexed_files()` computes summary language/file/error counts before applying path/language/role filters. | High |
| E17 | `crates/codestory-cli/src/main.rs:7669` | The CLI renders those summary counts as plain `files:` and `languages:` values, which can read as filtered counts even when the file list is filtered. | High |
| E18 | `crates/codestory-indexer/src/lib.rs:4571` | Manual receiver call edge appending silently skips specs when the owner/method target cannot be found in the local node/edge set. | High |
| E19 | `crates/codestory-indexer/src/lib.rs:5212` | Kotlin, Swift, Dart, and related receiver parameter handling use raw signature text, top-level comma splitting, and keyword filtering instead of fully declarative AST/query attributes. | High |
| E20 | `docs/review-action-plan.md` | The existing action plan marks earlier language-support cleanup as done, but current code still contradicts that claim for production steering and semantic language labels. | High |

## 3. Recommendation Summary

| Recommendation | Rationale and Evidence |
| --- | --- |
| Remove exact-family packet steering from the production runtime path. | Production packet execution currently defaults benchmark steering on and appends static citations for named benchmark families, which contradicts first-class language support because retrieved evidence no longer has to come from the graph or sidecar path. Evidence: E4, E5, E6, E7. |
| Move benchmark-family knowledge to eval-only manifests or an explicitly opt-in eval module. | Benchmark probes can exist, but product code must not know named repositories or benchmark families. The existing lint already encodes this boundary for older families and should be extended to the new families. Evidence: E1, E2, E8. |
| Create a shared language-support registry in `codestory-contracts`. | `codestory-workspace` cannot depend on `codestory-indexer`, while `codestory-runtime` already depends on both; putting claim and extension metadata in contracts lets workspace discovery, indexer routing, runtime semantic docs, CLI output, and docs share one source without dependency inversion. Evidence: E9, E10, E11, E12. |
| Keep parser construction and tree-sitter rules indexer-owned. | The shared registry should describe support claims, extensions, structural/parser-backed mode, and safe user-facing labels; parser handles, rule assets, and language-specific AST work remain in `codestory-indexer`. Evidence: E10, E11. |
| Track unresolved sidecar candidates as packet diagnostics and sufficiency gaps. | Single search already rejects unresolved-only results, but packet batch currently tolerates them without preserving the distinction between no candidates and unresolved candidates. Evidence: E13, E14, E15. |
| Separate whole-index counts from filtered visible counts in `files`. | Runtime computes whole-index summaries before filters, and the CLI labels them in a way that can read as filtered. The API should expose both or label the current summary clearly. Evidence: E16, E17. |
| Treat cross-file receiver resolution and declarative parameter extraction as a staged follow-up after the product overfit cleanup. | The current typed receiver path can silently skip missing local targets and relies on string-sliced signatures for several languages. That is real debt, but it should not block removing benchmark steering and support-claim drift. Evidence: E3, E18, E19. |

## 4. Scope Decision

This remediation spec covers the product correctness fixes needed before the branch can be trusted: production overfit removal, support registry consolidation, sidecar diagnostics, `files` truthfulness, and verification gates. Dynamic parser loading with `libloading` and fully externalized language profiles is out of scope for this fix because it changes packaging, parser distribution, and trust boundaries; it should become a separate architecture spec only after the current production contract is clean.
