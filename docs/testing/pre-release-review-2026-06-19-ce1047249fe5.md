# CodeStory Pre-Release Review - 2026-06-19

Candidate: `ce1047249fe5` (`ce1047249fe58e0216434b5fb2b2c5b3cf0deea8`)  
Baseline tag: `v0.9.0` (`2feb60990c6e`)  
Report package: `docs/testing/pre-release-review-2026-06-19-ce1047249fe5.*`

## Decision

#74 is the active release-review lane. #73 remains blocked until this report package is reviewed and the reviewer comparison branch from `v0.9.0` is created and cited.

Packet-runtime SLA optimization is skipped for this wave. #78 remains open and is carried as accepted/deferred release risk, not fixed and not cleared.

| Gate | Disposition | Evidence |
| --- | --- | --- |
| Release orchestration | blocker until report PR and reviewer branch exist | #74 issue body, #38, Project #1 |
| Packet-runtime SLA | accepted/deferred risk | #143 focused cold evidence: Apache `2/3` cold SLA misses, Redis `0/3`; warm SLA accepted residual risk |
| Packet quality and sufficiency | clear for focused #143 rows | Apache and Redis quality `3/3`, sufficiency `sufficient:3` |
| Draft PR stack | deferred future-wave | #133/#135/#138/#140/#143 are evidence surfaces, not delivered fixes |
| Version bump | blocker | #73 must wait for #74 completion and explicit unblock |
| Reviewer branch | blocker | `review/codestory-saga-from-v0.9.0-ce1047249fe5` still needs creation from `v0.9.0` |

## Evidence Manifest

| Evidence id | Source | Proof tier | Commit/artifact | Disposition | Notes |
| --- | --- | --- | --- | --- | --- |
| issue-74 | https://github.com/TheGreenCedar/CodeStory/issues/74 | coordinator gate | current issue body/comments | blocker until completed | Defines report, CSV, visuals, and reviewer branch gate. |
| issue-38 | https://github.com/TheGreenCedar/CodeStory/issues/38 | saga ledger | current issue body/comments | clear | Marks #74 active, #73 blocked, #78 accepted/deferred risk. |
| project-1 | https://github.com/users/TheGreenCedar/projects/1 | project state | Project README | clear | Same active-lane and deferred-SLA state as #38. |
| pr-133 | https://github.com/TheGreenCedar/CodeStory/pull/133 | draft PR evidence | `b45af82073a2` | deferred future-wave | Redis cold `0/3`; Apache cold `3/3`; draft/non-closing. |
| pr-135 | https://github.com/TheGreenCedar/CodeStory/pull/135 | draft PR evidence | `e0c648ba3a03` | deferred future-wave | #136 diagnostics review-clean; cold SLA still Apache `3/3`, Redis `1/3`. |
| pr-138 | https://github.com/TheGreenCedar/CodeStory/pull/138 | diagnostic evidence | `385733398b43` | clear as diagnostic only | Batch-overhead attribution; no product clearance. |
| pr-140 | https://github.com/TheGreenCedar/CodeStory/pull/140 | draft PR evidence | `db00a88ce9cb` | deferred future-wave | Redis cold `0/3`; Apache cold `3/3`; review-clean draft. |
| pr-143 | https://github.com/TheGreenCedar/CodeStory/pull/143 | focused evidence | `4ea187517f35` | accepted/deferred risk | Apache cold `2/3`, Redis cold `0/3`, quality `3/3`, sufficiency `sufficient:3`. |

## Packet/Search Quality

| Source | Repo/task | Mode | Runs | Quality | Sufficiency | SLA misses | Disposition |
| --- | --- | --- | ---: | ---: | --- | ---: | --- |
| #143 | Apache Commons Lang / `java-commons-lang-string-utils` | cold | 3 | 3/3 | `sufficient:3` | 2/3 | accepted/deferred residual risk |
| #143 | Redis / `c-redis-command-loop` | cold | 3 | 3/3 | `sufficient:3` | 0/3 | clear for focused cold row |
| #140 | Apache Commons Lang / `java-commons-lang-string-utils` | cold | 3 | 3/3 | `sufficient:3` | 3/3 | deferred future-wave |
| #140 | Redis / `c-redis-command-loop` | cold | 3 | 3/3 | `sufficient:3` | 0/3 | draft evidence only |
| #135 | Apache Commons Lang / `java-commons-lang-string-utils` | cold | 3 | 3/3 | `sufficient:3` | 3/3 | deferred future-wave |
| #135 | Redis / `c-redis-command-loop` | cold | 3 | 3/3 | `sufficient:3` | 1/3 | deferred future-wave |
| #133 | Apache Commons Lang / `java-commons-lang-string-utils` | cold | 3 | 3/3 | `sufficient:3` | 3/3 | deferred future-wave |
| #133 | Redis / `c-redis-command-loop` | cold | 3 | 3/3 | `sufficient:3` | 0/3 | draft evidence only |

Warm packet-runtime SLA is accepted residual release risk. It was not optimized in this wave and must not be hidden in release notes or version-bump handoff.

## Performance and Sidecar Summary

| Evidence | Apache retrieval median | Redis retrieval median | Batch total / attributed / overhead median | Disposition |
| --- | ---: | ---: | --- | --- |
| #143 focused cold run | 15528 ms | 7722 ms | Apache `7602 / 5120 / 2338 ms`; Redis `2450 / 3753 / 0 ms` | accepted/deferred risk |
| #140 lexical narrowing | 21432 ms | 10142 ms | Apache lexical overhead `4014 ms`; Redis lexical batch not material | deferred future-wave |
| #138 attribution | 21049 ms | 13688 ms | Apache batch overhead `7164 ms`; Redis batch overhead `1568 ms` | diagnostic only |

Sidecar proof must stay layer-specific. A live checkout with degraded `doctor` or missing retrieval manifest is not proof of sidecar readiness, and `retrieval_mode=full` alone is not packet quality or SLA proof.

## Lens Disposition

| Lens | Disposition | Finding count | Blocker count | Evidence |
| --- | --- | ---: | ---: | --- |
| Release orchestration | blocker | 3 | 2 | #74, #38, Project #1 |
| Benchmark/promotion evidence | accepted risk | 4 | 0 | #133/#135/#138/#140/#143 |
| Performance/sidecar readiness | accepted risk | 3 | 0 | #138/#140/#143 |
| Security/local trust | accepted risk | 2 | 0 | #74 lens-intake checkpoint |
| Documentation/operator usability | blocker | 2 | 1 | #74 lens-intake checkpoint |
| Engineering/architecture | clear for report path | 1 | 0 | #74 lens-intake checkpoint |

Current blockers before #73:

- Final report PR must be reviewed.
- Reviewer comparison branch from `v0.9.0` must be created and cited.
- Report must continue to state that #78 is open and deferred, not fixed.

## Reviewer Branch Workflow

Safer default for this report PR: defer branch creation until this report branch is open and reviewed.

Next coordinator action:

```powershell
git fetch origin
git switch --detach v0.9.0
git switch -c review/codestory-saga-from-v0.9.0-ce1047249fe5
# apply the accepted saga candidate diff from ce1047249fe5 without version-bump-only files
git diff --stat v0.9.0..HEAD
git diff --name-status v0.9.0..HEAD
git push -u origin review/codestory-saga-from-v0.9.0-ce1047249fe5
```

Expected compare URL:

`https://github.com/TheGreenCedar/CodeStory/compare/v0.9.0...review/codestory-saga-from-v0.9.0-ce1047249fe5`

## Remaining Gaps

| Gap | Classification | Owner action |
| --- | --- | --- |
| Reviewer branch missing | blocker | Create/push branch after this report PR opens. |
| Version bump not started | blocker by design | Keep #73 blocked until #74 explicitly unblocks it. |
| Packet-runtime SLA not cleared | accepted/deferred risk | Keep #78 open for later wave. |
| Draft PR stack not delivered | deferred future-wave | Do not count #133/#135/#138/#140/#143 as shipped fixes. |
| Optional XLSX not generated | clear | CSV is source of truth; workbook can be generated later from the CSV if desired. |

## Release Handoff Statement

This report does not unblock #73 by itself. It prepares the release-review evidence package and records the explicit release-owner decision: packet-runtime SLA optimization is skipped/deferred for this wave, #78 remains open, and the remaining Apache cold and warm SLA risk is accepted for the release-review path.
