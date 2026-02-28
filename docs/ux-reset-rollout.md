# UX Reset V2 Rollout Gates

## Scope
This runbook defines staged rollout, guardrails, and rollback criteria for `uxResetV2` and related flags:
- `uxResetV2`
- `onboardingStarter`
- `singlePaneInvestigate`
- `spacesLibrary`

## Rollout Stages
1. Internal only (100% of internal users).
2. Beta cohort (10% external users).
3. Expanded beta (50% external users).
4. Default on (100% users).

## KPI Guardrails
Track weekly and compare to baseline:
- First-session core flow completion (`open -> index -> ask -> inspect`) should not drop more than 5%.
- Median time-to-first-answer should improve by at least 15% before stage 3.
- `starter_card_cta_clicked` should occur in at least 30% of first sessions.
- `investigate_mode_switched` should show use across at least two modes in 40% of active sessions.
- `library_space_reopened` should trend up vs baseline by stage 3.

## Hard Rollback Triggers
Rollback immediately to previous stage if any occur for 24h:
- Core flow completion drops by 10% or more.
- Error rate in ask/index workflows increases by 20% or more.
- Critical accessibility regression in keyboard-only path.
- Severe layout breakage on laptop/mobile form factors.

## Rollback Procedure
1. Toggle `uxResetV2=false`.
2. Keep `spacesLibrary` unchanged unless storage regressions are observed.
3. Verify app boot, open project, index, ask, graph, and code flows.
4. Announce rollback in internal channel and attach metrics snapshot.

## Promote Criteria Per Stage
- Stage 1 -> 2: No P0/P1 UX issues, no guardrail breaches.
- Stage 2 -> 3: Time-to-first-answer improved and no sustained error increase.
- Stage 3 -> 4: All guardrails stable for 7 days.
