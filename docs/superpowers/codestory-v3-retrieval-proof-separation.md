# CodeStory v3 retrieval/proof separation

## Global constraints

- Base all work on `origin/dev/codestory-next@b1c9198474f7d0a1d43159e4070c021505ae39f9`; recheck before implementation.
- Deliver five guarded PRs. PRs 1-4 are production-unreachable preparation. PR 5 is the only public CodeStory schema-v3 switch.
- Public authority after PR 5: `search` is discovery leads; `context` is evidence for one target; `packet` is broad evidence, gaps, and retrieval state; only `prove_call_path` emits `ContractProven`, `ContractRefuted`, `Unknown`, or `Unavailable`.
- #1968 owns packet projection and diagnostics. #1973 owns structured proof. The closing child issues are PR 1 #1975, PR 2 #1976, PR 3 #1974, PR 4 #1978, and PR 5 #1977.
- PRs 1-4 must not change current wire constants, current `ToolSpec`/TOOLS/RESOURCES/PROMPTS, current DTO serialization, production handlers, compact renderers, launch behavior, or generated v2 catalog bytes. V3-only types must be unreachable from production dispatchers, CLI command enums, HTTP routes, public runtime facades, current result serializers, and generated catalogs.
- Use strict TDD: add one real behavior test, observe its expected failure, add the minimum implementation, and keep the focused lane green. Do not add source-grep or prose tests.
- Keep product ownership: contracts define shared primitives; agent owns the pure checker and depends only on contracts; runtime owns pinned resolution/source reads/orchestration; CLI and plugin are adapters.
- Preserve unrelated work. Use focused checks per PR. Do not run broad workspace gates until the final accepted PR-5 head.
- No version bump, release, tag, marketplace publication, or production-release claim.

## Task 1: Compatibility harness

Issue: #1975. This task is public-v2 neutral.

### Outcome

Create the compatibility and exact-source qualification harness needed for the later v3 cut without changing current public behavior.

### Baseline and transcript contract

- Freeze the checked-in generated MCP catalog exactly at the post-#1972 base. Its known baseline is `b1c9198474f7d0a1d43159e4070c021505ae39f9`; do not use `419bf04a`.
- Capture native and launcher-fail-open v2 fixtures for initialize, tools/list, resources/list, resources/templates/list, prompts/list, representative successful tool results, the successful retryable `codestory_preparing` result, and unavailable/tool errors.
- Normalize only explicitly declared volatile fields: operation/request/packet/publication IDs, timestamps/timing, runtime binary hash, and source/build identity values. Do not normalize semantic fields, field order, schemas, constants, or payload vocabulary.
- Preserve current schema/publication constants and generated catalog. The exact catalog bytes must remain unchanged.

### Schema validation harness

- Generalize the existing closed JSON Schema subset validator to support the inputs and root-object tagged output variants required later, including `oneOf`, `anyOf`, `allOf`, required fields, bounds, tagged unions, and `additionalProperties:false` where the existing subset admits them.
- Add maximal fixtures covering every current success field/enum variant plus current native and launcher preparing/error shapes.
- Run output validation in tests/shadow audit only. Do not enforce it in production v2: current launcher fail-open and native error payloads are intentionally not all admitted by success-only schemas.
- The final validator boundary in PR 5 will be the post-projection/post-budget `structuredContent`, immediately before `CallToolResult`; task 1 should make that boundary callable without switching it on.

### Four MCP protocol profiles

Add official test fixtures and reusable profile data for:

| Revision | Tool fields | Result form |
| --- | --- | --- |
| `2024-11-05` | `name`, `description`, `inputSchema` | JSON `TextContent` only |
| `2025-03-26` | above plus `annotations` | JSON `TextContent` only |
| `2025-06-18` | top-level `title`, `outputSchema`, Tool `_meta` | conforming `structuredContent` plus identical JSON text |
| `2025-11-25` | same fields CodeStory uses; no task/icon claims | conforming `structuredContent` plus identical JSON text |

- Add batch fixtures: 2024/March accept valid JSON-RPC arrays, process entries independently, omit notification responses, and preserve input order; June/November reject arrays with `-32600`.
- Keep all profiles unselectable by current production negotiation in this PR.
- Prepare revision-native allowed-field fixtures. Current top-level CodeStory `safety` is not standard in any revision; its removal happens only in the public cut.

### CodeStoryDev repair

- `.codex-plugin/plugin.json` currently names plugin `0.17.3`, while `plugins/codestory/cli-version.json` and the CLI crate pin `0.17.1`.
- Repair `scripts/install-codestory-dev-plugin.mjs` and its receipt validation so the plugin version and the pinned CLI version are read and checked separately.
- Add a regression fixture for divergent plugin/CLI versions. Preserve the current real 0.17.3/0.17.1 pairing; do not bump either version.
- A CodeStoryDev receipt proves source package identity and supplied binary SHA/version. It does not prove the binary was built from the named source head. Add or document a source-built chain that records clean exact head/tree, locked build command, built artifact SHA, installed receipt SHA, and live host SHA.

### Generated syntax

- `scripts/generate-codestory-skill-syntax.mjs` currently offers 2024 and records the negotiated reply as preferred. Refactor the generator so preferred revision comes from an explicit compiled preference/default selection rather than the caller's hard-coded offered revision.
- Separately exercise every supported fixture revision. With current singleton production support, the generated v2 file must remain byte-identical.

### Verification

- The new tests must fail on the existing pin conflation, hard-coded preferred derivation, or missing harness behavior before implementation.
- Run the focused CLI/unit tests, `cargo test --locked -p codestory-cli --test stdio_protocol_contracts`, `node --test scripts/tests/install-codestory-dev-plugin.test.mjs`, `node --test plugins/codestory/tests/plugin-static.test.mjs`, and the generator check with a source-built debug CLI.
- Commit with a short lowercase imperative message. Push and open a guarded PR targeting `dev/codestory-next` with `Closes #1975` and `Refs #1973`; add issue and PR to Project 4.

## Task 2: Dark exact call-path proof kernel

Issue: #1976. This task remains production-unreachable.

### Request and proof domain

Define non-wire types for a validated call-path contract. Public serialization waits for PR 5.

```rust
struct ProveCallPathRequestDto {
    source_text: String,
    clauses: Vec<ClauseAnchorDto>,
    spec: CallPathSpecDto,
}

struct CallPathSpecDto {
    start: ExactSymbolSelectorDto,
    steps: Vec<DirectCallStepDto>, // 1..=6
    prohibit_traversal_through: Vec<ExactScopeSelectorDto>,
    exclude_from_projection: Vec<ExactScopeSelectorDto>,
}
```

The domain is an ordered, direct, outgoing, indexed source-level `CALL` trail. Each target becomes the next source. It does not prove runtime execution, reachability, time, ownership, arbitrary data flow, or subsystem non-participation.

Selectors are exact: publication-bound pinned node `{project_id, core_generation_id, core_run_id, node_id}`, canonical ID, or qualified name plus optional normalized project file. There is no fuzzy matching, `DiscoverUnique`, substring selector, or signature. Missing and ambiguous resolutions are typed `Unknown` gaps.

### Translation anchors and digest

- Anchor exact UTF-8 byte offsets and byte-exact `quote`; omit occurrence numbers. Allow overlap/nesting.
- Classifications are `ResolvedMaterial { fields }`, `UnresolvedMaterial { reason }`, and typed `NonMaterial`.
- Use a closed `ProofContractFieldDto`: Start, StepTarget, Directness, Ordering, Relation, TraversalProhibition, ProjectionExclusion.
- Every non-whitespace byte is covered. Precedence is unresolved, resolved, non-material. Every populated or implicit field has a resolved anchor.
- Valid incomplete translation returns `Unknown(unclassified_source_text|unresolved_material_clause|material_token_misclassified)`. Malformed spans, quote mismatch, contradictory classifications, missing required anchors, or invalid paths are semantic tool errors later.
- `clause_guard_v1` catches quoted/backticked identifiers, arrows/relation notation, directness/order/ordinals/only, negation/exclusion terms, path-like text, and qualified symbols.
- `source_text` may flow only to clause validation, hashing, diagnostics, and rendering. The executor accepts a validated contract plus hashes, never raw text. Add an architecture dependency test.
- Digest domain fields: proof contract schema version 1, proof domain `indexed_source_call_path_v1`, clause guard version `clause_guard_v1`, SHA-256 of original UTF-8 text, normalized typed clauses, and normalized ordered spec. Use RFC 8785 canonical JSON and a domain-separated SHA-256. Normalize clauses by `(start,end,clause_id,classification,typed field)` and deduplicate only exact duplicates.

### Raw edge admission and source receipt

Read raw `codestory_contracts::graph::Edge`. Never admit a presentation DTO, call `with_effective_endpoints()` first, or derive certainty from confidence. Require CALL, stored Certain, exact effective source/target, `resolved_target == Some(expected_target)`, empty candidate targets, file ID, line >= 1, and a canonical four-part pre-marker callsite identity whose file/line/raw-target match the edge. Empty/marker-only/opaque legacy identities fail closed.

Then require callable source and target nodes, edge file equal to caller file, unique-smallest callable containment for the line, and indexed-complete hash-verified source. Equal smallest extents are ambiguous.

Read working-tree bytes once inside the existing pinned public operation; hash the exact buffer and match the pinned stored file hash; strict UTF-8 decode; extract the complete recorded line including terminator; apply the post-operation freshness fence. Missing/mismatch is `Unavailable(source_not_bound_to_publication)`. A line over 8 KiB is `Unknown(source_window_too_large)`. `Occurrence` never authorizes an exact expression span. No indexer/store migration.

### Trail and refutation

- Edge-distinct static trail: vertices may repeat; one receipt discharges one step. Two recursive steps require two distinct edges.
- Current producers suppress self-call edges, so real recursion is `Unknown(recursive_call_not_representable)`. Synthetic pure-kernel tests accept an exact self-edge and reject reuse.
- `ContractRefuted` is either receipt-backed `PositiveContradiction` or `CertifiedAbsence` with extractor capability plus untruncated enumeration receipts.
- Production provides positive contradiction only. Certified absence is fixture-only. Missing edges remain Unknown.
- Projection exclusions never mean non-participation; hiding a required receipt yields `Unknown(projection_exclusion_conflicts_with_required_receipt)`.

### Budgets and proof fixtures

Build root-object `Complete` and `BudgetExceeded` projection variants with required `kind`. BudgetExceeded contains identity/digest/publication plus `Unknown(output_budget_exceeded)`, maximum bytes, and required complete bytes; no clauses/spec/steps/receipts.

Add all hostile edge cases, exact selector/path normalization cases, hash match/mismatch/stale cases, containment ambiguity, escape-heavy budget boundaries, source-text isolation, positive contradiction, and fixture-only absence. Add a source-built real index census and one exact positive source fixture proving the strict predicate is reachable.

Run agent/contracts/runtime focused tests as owned. Commit, review, push, and open a guarded PR with `Closes #1976` and `Refs #1973`.

## Task 3: Dark packet projections

Issue: #1974. Parents #1968 and #1973. This task remains production-unreachable.

- Add immutable `PacketExecutionRecord` with packet ID, project identity, request digest, exact core/retrieval publication, canonical evidence, and diagnostics.
- Define exhaustive explicit packet/context/search/diagnostic projection DTOs. Do not serialize shared internal DTOs.
- Separate generated heuristic `DiscoveryLead` from user-explicit/source-spanned blocking `MaterialClaim`.
- Keep redundant query skipping but let `SkippedBecauseDischarged { claim_id, receipt_ids }` exist only after receipt-backed discharge. Query completion or bare `not_dispatched` never proves a claim.
- V3 packet is broad evidence, gaps, continuation/retrieval state; no arbitrary-NL proof disposition, public `supported`, `eligible_for_sufficiency`, or default-admit debug keys.
- Build the compact agent view and bounded immutable diagnostic bytes from one finalized record after receipt reconciliation. No rerun or live read for diagnostics.
- Packet whole-result cap is 16 KiB. Measure escaped revision-specific ToolResult. Trim only explicitly optional context evidence deterministically. Preserve packet/publication/evidence/gap identities and diagnostics descriptor. If the mandatory envelope does not fit, return explicit BudgetExceeded/Unavailable with question hash/publication/diagnostics capability and no partial evidence. If even that cannot fit later, protocol internal error.
- Keep current public v2 handlers/DTOs/renderers/catalog/include_evidence untouched. Add frozen-v2 and production-unreachability tests.
- Focused agent/runtime/CLI tests, review, push, guarded PR with `Closes #1974` and `Refs #1968 #1973`.

## Task 4: Dark revision-native MCP machinery

Issue: #1978. This task remains production-unreachable.

- Add session-scoped profiles for 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25, preferred newest.
- Revision-native tools: 2024 name/description/inputSchema; March adds annotations; June adds title/outputSchema/Tool `_meta`; November uses the same CodeStory fields and no tasks/icons. Remove top-level `safety` in v3 views; vendor activation metadata lives at `_meta["com.thegreencedar.codestory/safety"]` June/November and descriptions earlier.
- `readOnlyHint:true` only for truly observational tools. Activation-capable tools omit it/default false.
- 2024/March accept valid JSON-RPC batches in order and omit notification replies. June/November reject arrays with -32600.
- Modern results use conforming structuredContent plus identical compact JSON text. Output schemas are root objects. Preparing is a successful tagged variant. Tool errors are text-only with isError true.
- Error map: bad request shape -32602; semantically invalid proof interpretation tool error; typed Unknown/Unavailable successful; server-invalid output -32603 with invalid payload suppressed; syntactically valid unavailable diagnostics URI -32002; registry/internal -32603.
- Validate post-projection/post-budget structuredContent immediately before CallToolResult. Keep enforcement unreachable/shadow until task 5.
- Diagnostics capability: `codestory://packet-diagnostics/{packet_id}/{token}`, per-session 32-byte OS-CSPRNG secret, random packet ID, HMAC-SHA256 over domain/packet/project/publication/request/wall-expiry, constant-time check, same-session 8-entry/8-MiB/1-MiB registry, ten-minute expiry enforced only with monotonic Instant. Resource read serves immutable bytes without activation/status/publication/source/rerender and is not listed.
- Token appears only in the result URI: text for old profiles, same URI in structured and mirrored JSON for modern. Never in CodeStory-owned logs, cached diagnostics, catalogs, skills, errors, or unrelated prose.
- Compute a per-revision SHA-256 discovery contract over canonical initialize capabilities, tools, resources, templates, prompts, advertised schemas, and CodeStory publication schema. Store negotiation in native/launcher session state and reject old/new or wrong-v3 handoff skew.
- Keep production v2 paths and generated files unchanged. Focused protocol/plugin tests, review, push, guarded PR with `Closes #1978` and `Refs #1973`.

## Task 5: Atomic public v3 switch

Issue: #1977. Parents #1968 and #1973. One integration commit owns the public cut.

- Register `prove_call_path` and expose `codestory-cli prove-call-path --project <ROOT> --spec <PATH|->`. `-` reads stdin; cap input at 64 KiB before deserialize; same request/response DTO as MCP; JSON default; no profile/run/retrieval/latency/evidence controls; observational and no semantic retrieval activation.
- Switch packet/context/search to explicit v3 projections. Only prove_call_path owns proof disposition vocabulary.
- Remove packet `include_evidence` from MCP request, AgentPacketRequestDto, CLI packet `--no-evidence`, and task brief `--no-evidence`; keep ask/context evidence controls. Old packet args fail -32602.
- Enable packet diagnostics and standalone packet `--diagnostics-out <PATH>` atomic owner-only write.
- Enable four MCP profiles, batch rules, final-result validation, and discovery digests in native and launcher sessions.
- For proof, build complete response and whole revision ToolResult, measure including escaping/text mirror, fallback above 64 KiB, validate/remeasure, and return -32603 only if fallback itself cannot fit. Packet uses analogous 16 KiB rule.
- Update CLI JSON, stdio, launcher, HTTP, task brief, drill, renderers, analyzers, skill/help/docs, generated catalogs, and CHANGELOG Unreleased.
- Set CodeStory `schema_version = minimum_compatible_schema_version = 3` across Rust constants, launcher mirrors, generated catalog, CLI/HTTP/stdio stamps, docs, and tests exactly once.
- Replace negative dark reachability with positive route-registration-once tests; legacy Supported cannot reach a public response.
- Required protocol/error/budget/diagnostics/skew/CLI-MCP equivalence matrices all pass.
- Commit, review, push, open guarded PR with `Closes #1977` and `Refs #1968 #1973`. Add issue and PR to Project 4.

## Task 6: Final exact-head qualification

- Freeze clean PR-5 head/tree before expensive gates. Any later source/wire change invalidates this evidence.
- Run serially: `cargo fmt --all -- --check`; `cargo check --locked --workspace`; `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`; `cargo nextest run --locked --workspace --no-fail-fast`; `cargo test --locked --workspace --doc`; `cargo test --locked -p codestory-cli --test stdio_protocol_contracts`; generator check with debug CLI; installer tests; plugin-static; doc links; `git diff --check`.
- Build the locked release CLI from the exact clean head and record head/tree, build command, artifact SHA. Install via repaired CodeStoryDev lane. Restart a fresh host and match installed package receipt, CLI SHA, `cli_source=local_dev_override`, v3 publication stamp, negotiated revision, and discovery identity to that source-built chain.
- Run the 16-prompt translation conformance set. Require zero false ContractProven results and zero silent material omissions. Treat it as scoped host-translation qualification, not unrestricted English correctness or release proof.
- Close #1968 and #1973 only after final acceptance. No release/version/tag/marketplace work.

## 2026-08-21 Proof availability amendment

The [proof availability qualification plan](plans/2026-08-21-codestory-v3-proof-availability-qualification.md) supersedes only this document's fixed delivery sequence and unconditional proof activation. The proof contract, receipt requirements, negative-proof limits, and authority boundaries above remain unchanged. In particular, packet, context, and search remain evidence products; only the exact call-path proof domain may emit proof dispositions.

The qualification work is now split into Q1 followed by Q2. Q1 builds and freezes a production-dark benchmark harness, oracle corpus, thresholds, and verification machinery. Q1 does not produce an availability result and does not make an activation decision. Q2 runs that frozen machinery once on an exact clean head and selects one of four outcomes:

- **Outcome A:** register the stable CLI and MCP proof surfaces, with automatic or explicit-only workflow guidance determined by the frozen thresholds.
- **Outcome B:** expose only an explicitly experimental CLI verifier; do not register MCP, catalog, or skill proof surfaces.
- **Outcome C:** keep proof dark while shipping the evidence-only packet, context, and search separation.
- **Outcome D:** delay the public v3 cut only if source-level evidence shows that the evidence-only surfaces cannot ship independently of proof activation.

All remaining integration work targets `dev/codestory-0.18`. The concurrent 0.17.4 release lane is outside this program and must not be changed, rebased, or used as qualification evidence. Publication schema 3 is still cut exactly once, after Q2, only in the selected final integration outcome.
