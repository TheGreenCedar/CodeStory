# Autoresearch: CodeStory manual explanation friction

## Objective
Eliminate user and AI friction in CodeStory skill-first repo explanation across Sourcetrail, rootandruntime, and CodeStory.

## Metrics
- Primary: quality_gap (gaps, lower is better)
- Secondary: none yet

## How to Run
`powershell -NoProfile -ExecutionPolicy Bypass -File ./autoresearch.ps1` builds the release CLI, runs `node scripts/codestory-manual-friction-check.mjs --setup-embeddings`, and prints `METRIC name=value` lines.

## Files in Scope
- crates/codestory-runtime
- crates/codestory-cli
- crates/codestory-contracts
- .agents/skills/codestory-grounding
- scripts/codestory-manual-friction-check.mjs

## Off Limits
- TBD: add off-limits files or behaviors if needed

## Constraints
- Decision contract: quality_gap is treated as a quality-bearing score; faster runs should not be promoted when component evidence shows quality or correctness erosion.
- Manual friction loop covers `../Sourcetrail`, `../rootandruntime`, and `.`.
- `quality_gap` counts P0/P1/P2 user or AI friction from semantic health, broad explanation drift, search/symbol ambiguity, trail inconsistency, snippet truncation clarity, output labeling, and skill recipe gaps.
- Full benchmark packets use the skill-approved CLI path. Short targeted tests can guide implementation, but only the full three-repo benchmark can close the loop.

## Decision Rules
- Keep when the primary metric improves or a baseline is needed and checks pass.
- Discard when the metric is equal or worse, unless the run only establishes the baseline.
- Log crashes and failed checks with a concrete rollback reason.
- Put next-step guidance in ASI so another Codex session can continue.

## Stop Conditions
- Stop when `quality_gap=0`, checks pass, all three repos complete the explanation flow, and two fresh consecutive full rounds add no new P0/P1/P2 friction.
- For qualitative loops, `quality_gap=0` only closes the current accepted checklist; run a fresh discovery round before claiming no more friction remains.
- Stop when maxIterations is reached or the user interrupts.

## Research Notes
- Source-backed facts, contradictions, and open questions go here or in linked scratchpad files.
- For deep research loops, link the scratchpad folder and summarize the current synthesis.

## What's Been Tried
- 2026-05-07: Implemented semantic doc token budgeting, doctor semantic health labels, repo-overview ask fallback, query/trail resolver consistency, snippet/mode output clarity, and skill guidance updates.
- 2026-05-07 follow-up: fixed `trail` help discoverability for `dot`, corrected repo-explain local-agent mode labeling, replaced placeholder e2e commit labels, and made the harness rebuild the release CLI before measuring.
- Autoresearch packet `packet-1-089c8d5afed0` kept with `quality_gap=0`, `repos_checked=3`, and checks passing. A fresh packet should be logged after the follow-up patch so the ledger points at the final implementation commit.

## Resume This Session

Use these commands to pick the loop back up without rediscovering state:

```bash
node "C:\\Users\\alber\\source\\repos\\autoresearch\\plugins\\codex-autoresearch\\scripts\\autoresearch.mjs" state --cwd "C:\\Users\\alber\\source\\repos\\codestory"
node "C:\\Users\\alber\\source\\repos\\autoresearch\\plugins\\codex-autoresearch\\scripts\\autoresearch.mjs" doctor --cwd "C:\\Users\\alber\\source\\repos\\codestory" --check-benchmark
node "C:\\Users\\alber\\source\\repos\\autoresearch\\plugins\\codex-autoresearch\\scripts\\autoresearch.mjs" next --cwd "C:\\Users\\alber\\source\\repos\\codestory"
node "C:\\Users\\alber\\source\\repos\\autoresearch\\plugins\\codex-autoresearch\\scripts\\autoresearch.mjs" log --cwd "C:\\Users\\alber\\source\\repos\\codestory" --from-last --status keep --description "Describe the kept change"
node "C:\\Users\\alber\\source\\repos\\autoresearch\\plugins\\codex-autoresearch\\scripts\\autoresearch.mjs" export --cwd "C:\\Users\\alber\\source\\repos\\codestory"
```

## Run Ledger

<!-- AUTORESEARCH_RUN_LEDGER:START -->
- Run 1 keep: Close manual explanation friction checklist; metric=0; best=0; commit=acf962b; Git: committed acf962b..
- Run 2 keep: Verify forensic follow-up fixes; metric=0; best=0; commit=d205509; Git: committed d205509..
<!-- AUTORESEARCH_RUN_LEDGER:END -->
