# Autoresearch: Deep research: Reduce CodeStory real-repo agent drill friction across Sourcetrail, CodeStory, and rootandruntime. Track current-run quality gaps from source-verified drill evidence, implement improvements, rerun the drill, and repeat until gaps stop falling.

## Objective
Reduce CodeStory real-repo agent drill friction across Sourcetrail, CodeStory, and rootandruntime. Track current-run quality gaps from source-verified drill evidence, implement improvements, rerun the drill, and repeat until gaps stop falling.

## Metrics
- Primary: quality_closed (accepted gaps closed, higher is better)
- Secondary: quality_gap, quality_total, quality_newly_accepted, quality_newly_closed, quality_stagnating, quality_plateau

## How to Run
`powershell -NoProfile -ExecutionPolicy Bypass -File ./autoresearch.ps1` prints `METRIC name=value` lines.

## Files in Scope
- autoresearch.research/codestory-real-repo-friction-20260522

## Off Limits
- TBD: add off-limits files or behaviors if needed

## Constraints
- Decision contract: quality_closed is the cumulative accepted-gap closure signal. quality_gap remains the open-gap state, not proof that another flat-zero packet is useful.
- Keep research notes under autoresearch.research/codestory-real-repo-friction-20260522.
- Use source-backed evidence before implementing recommendations.

## Decision Rules
- Keep when quality_closed increases from a source-backed accepted gap, or when a baseline is needed and checks pass.
- Discard when the metric is equal or worse, unless the run only establishes the baseline or records a measurement-contract correction.
- Log crashes and failed checks with a concrete rollback reason.
- Put next-step guidance in ASI so another Codex session can continue.

## Stop Conditions
- Stop product iteration when `quality_gap=0`, `quality_newly_accepted=0`, checks pass, and no fresh candidate has been logged open before it is fixed.
- Start a fresh research round, promotion gate, or deeper design spike before continuing after a plateau.
- Stop when maxIterations is reached or the user interrupts.

## Research Notes
- Source-backed facts, contradictions, and open questions go here or in linked scratchpad files.
- For deep research loops, link the scratchpad folder and summarize the current synthesis.

## What's Been Tried
- Current state: Segment 10 run 45 is the measurement-only plateau after
  closing Fresh Round 21 (`quality_closed=51`, `quality_gap=0`,
  `quality_total=51`, `quality_newly_accepted=0`,
  `quality_newly_closed=0`, `quality_stagnating=1`,
  `quality_plateau=1`).
- Latest closed product state: run 44 closed Fresh Round 21 after first logging
  it open in run 43, proving the loop resumed only after a fresh candidate was
  accepted open.
- Guardrail: do not implement more product changes unless a fresh candidate is
  logged open before it is fixed.

## Resume This Session

Use these commands to pick the loop back up without rediscovering state:

```bash
node "C:\\Users\\alber\\.codex\\plugins\\cache\\thegreencedar-autoresearch\\codex-autoresearch\\1.3.7\\scripts\\autoresearch.mjs" state --cwd "C:\\Users\\alber\\source\\repos\\codestory"
node "C:\\Users\\alber\\.codex\\plugins\\cache\\thegreencedar-autoresearch\\codex-autoresearch\\1.3.7\\scripts\\autoresearch.mjs" doctor --cwd "C:\\Users\\alber\\source\\repos\\codestory" --check-benchmark
node "C:\\Users\\alber\\.codex\\plugins\\cache\\thegreencedar-autoresearch\\codex-autoresearch\\1.3.7\\scripts\\autoresearch.mjs" next --cwd "C:\\Users\\alber\\source\\repos\\codestory"
node "C:\\Users\\alber\\.codex\\plugins\\cache\\thegreencedar-autoresearch\\codex-autoresearch\\1.3.7\\scripts\\autoresearch.mjs" log --cwd "C:\\Users\\alber\\source\\repos\\codestory" --from-last --status keep --description "Describe the kept change"
node "C:\\Users\\alber\\.codex\\plugins\\cache\\thegreencedar-autoresearch\\codex-autoresearch\\1.3.7\\scripts\\autoresearch.mjs" export --cwd "C:\\Users\\alber\\source\\repos\\codestory"
```

## Run Ledger

<!-- AUTORESEARCH_RUN_LEDGER:START -->
- Run 1 keep: baseline current-run checklist before CodeStory real-repo friction fixes; metric=12; best=12; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 2 keep: after two implementation packets for CodeStory real-repo drill friction; metric=6; best=6; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 3 keep: packet 3 drill evidence adds caller summaries execution boundaries and verdicts; metric=4; best=4; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 4 keep: fresh dashboard sync measurement confirms current four-gap state; metric=4; best=4; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 5 keep: packet 4 adds related Payload collection consumers without closing remaining storage gap; metric=4; best=4; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 6 keep: packet 5 exposes StorageAccess text consumer hints and closes caller consumer gap; metric=3; best=3; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 7 keep: packet 6 trail and Search Plan low-confidence bridge suppression; metric=2; best=2; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 8 keep: packet 7 canonical cross-repo drill-suite command; metric=1; best=1; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 9 keep: packet 8 real-repo drill-suite golden regression gate; metric=0; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 10 keep: fresh round baseline for drill-suite cache isolation; metric=1; best=1; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 11 keep: packet 9 drill-suite explicit cache isolation; metric=0; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 12 keep: fresh round 3 baseline for bridge-to-verification gaps; metric=4; best=4; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 13 keep: packet 10 bridge verification follow-ups; metric=1; best=1; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 14 keep: packet 11 actionable partial bridge evidence; metric=0; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 15 keep: fresh round 4 baseline for freshness and bridge ranking; metric=2; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 16 keep: packet 12 freshness and evidence ranking; metric=0; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 17 keep: packet 17 freshness and evidence ranking UX; metric=0; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 18 keep: packet 18 handoff surface parity; metric=0; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 19 keep: packet 19 related payload target truth; metric=0; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 20 keep: packet 20 hide-speculative layout parity; metric=0; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 21 keep: packet 21 broad question named anchors; metric=0; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 22 keep: packet 22 drill-suite progress visibility; metric=0; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 23 keep: packet 23 source-truth checklist compression; metric=0; best=0; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 24 keep: segment 5 accepted-gap closure dashboard baseline; metric=29; best=29; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 25 keep: fresh round 11 bridge degradation visibility gap accepted; metric=29; best=29; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 26 keep: suite bridge degradation visibility gap closed; metric=30; best=30; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 27 keep: segment 6 accepted gaps baseline after newly-closed metric repair; metric=30; best=30; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 28 keep: segment 6 report and metric clarity gaps closed; metric=34; best=34; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 29 keep: fresh round 15 id-stable follow-up command gap accepted; metric=34; best=34; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 30 keep: id-stable drill follow-up handoff closed; metric=35; best=35; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 31 keep: fresh round 16 pending claim scoring gap accepted; metric=35; best=35; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 32 keep: claim-ledger pending scoring and pending-claim scope closed; metric=37; best=37; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 33 keep: fresh round 17 source verification handoff gaps accepted; metric=37; best=37; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 34 keep: fresh round 17 source verification handoff gaps closed; metric=41; best=41; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 35 keep: fresh round 18 semantic setup and coverage gaps accepted; metric=41; best=41; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 36 keep: closed round 18 semantic fallback and dashboard progress metric gaps; metric=43; best=43; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 37 keep: fresh round 19 bridge payload and cache-dir determinism gaps accepted; metric=43; best=43; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 38 keep: closed round 19 bridge payload and cache-dir determinism gaps; metric=45; best=45; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 39 keep: plateau measurement after round 19 closure; metric=45; best=45; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 40 keep: fresh round 20 suite handoff and coverage gaps accepted; metric=45; best=45; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 41 keep: closed round 20 suite handoff and coverage gaps; metric=50; best=50; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 42 keep: plateau measurement after round 20 closure; metric=50; best=50; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 43 keep: fresh round 21 stale blocker handoff gap accepted; metric=50; best=50; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 44 keep: closed round 21 plateau handoff blocker gap; metric=51; best=51; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
- Run 45 keep: plateau measurement after round 21 closure; metric=51; best=51; commit=cd7845c61d65; Git: recorded existing commit cd7845c61d65..
<!-- AUTORESEARCH_RUN_LEDGER:END -->
