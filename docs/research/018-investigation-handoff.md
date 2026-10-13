# CodeStory 0.18 investigation WIP handoff

This branch preserves the complete known authored research candidate, including
previously uncommitted Rust/plugin source, the reproducible native dependency
patch, and the unfinished retrieval changes. It is a draft handoff,
not a release, acceptance result, or instruction to install. The product goal in
[issue 2173](https://github.com/TheGreenCedar/CodeStory/issues/2173) remains open.
The source/package version is still **0.17.7**; “0.18” names this research effort.
Read this document, [AGENTS.md](../../AGENTS.md), the
[testing matrix](../contributors/testing-matrix.md), and the owning architecture
pages before resuming. Public questions and small sanitized evidence are in
[the evidence directory](018-investigation-handoff-evidence/README.md).

## What to resume

The immediate unfinished user outcome is **reliable discovery of relevant source
and actual test assertions from unfamiliar ordinary-language software questions**.
Agents should be able to investigate arbitrary questions without the user
hand-tuning keywords, identifying entry points, or adding question-specific
production logic. A successful packet or manually corrected answer is insufficient
evidence of improved retrieval. Compare actual baseline and changed delivery,
including omissions, failures, follow-up cost, and adverse changes.

Two independently authored development questions currently expose the gap on
pinned robfig/cron: mixed five/six-field schedule configuration, and satisfiable
versus impossible calendar dates. The installed baseline initially omitted
decisive configuration or test-body evidence. A reader's later ordinary-language
search found relevant configuration and assertions. The new query/ranking source
has focused unit passes, but **has not been built into a normal CLI or evaluated
with changed public packets**. This handoff stops implementation and execution
until the work is explicitly resumed.

## Governing goal and acceptance

The [maintainer-approved amendment](https://github.com/TheGreenCedar/CodeStory/issues/2173#issuecomment-5743431622)
restored the full program. The historical issue body contains superseded
“complete-enough,” freeze and staffing statements; those must not close this work.
The current issue and linked canonical artifacts remain authoritative if amended
after this handoff. Preserve all original criteria:

- Demonstrate locate, inspect, follow, join, compare, bounded exploration,
  adaptive composition and explicit evidence limits. Cover teaching, actionable
  review, debugging, optimization, architecture, auth/permissions and UI; languages
  and frameworks; configuration/deployment; cross-service and multi-repository
  systems; and non-symbol artifacts. Source-only evidence cannot establish
  effective permission, deployed behavior, measured speedup or rendered UI.
- Existing primitives count when demonstrated. Implement the smallest evidenced
  gaps. A **new general abstraction** requires the same missing capability in
  **three unrelated repositories across two domains**; a local correctness fix
  does not require inventing an abstraction. No holdout names, answer selectors,
  benchmark families, claim taxonomy or hidden question policy in production.
- Product convergence precedes independent unseen acceptance: **12 targets,
  72 tasks, 3 arms, 216 sessions**. Use independent authors/graders, unseen
  exclusions, concrete inputs, grading/order controls, and a separate prospective
  resource/model freeze. Use the accepted unpublished maintenance comparator;
  retain original candidate source pins and separately identify rebuilt bytes.
  All comparative arms use the same frozen participant model, reasoning and tool
  permissions. Historical model identities must not be relabelled.
- Preserve domain/workflow composition, including at least four cross-service or
  infrastructure targets, AWS/IAM, Kubernetes/Terraform, UI, at least eight domain
  groups and substantial review coverage. Preserve no-loss and critical-claim
  gates. Material gain is eight additional successful tasks of 72 across at least
  four domain groups, or the prospectively selected 20% efficiency measure with
  equal-or-better correctness across at least four domain groups. Individual
  development successes do not qualify these thresholds.
- Keep development-panel work distinct from unseen acceptance. The source-backed
  bank of 1,360 questions, 5,334 anchors and 68 roots is development material,
  not a pass rate; its incomplete answers and external-evidence obligations remain.
  Neither an expired pilot nor a proposed panel establishes current admission.
  Issue 2116 remains excluded. Original failed calls, retries, fallback work,
  token-tradeoff amendment, costs and per-attempt ceilings remain preserved.
- After convergence and valid independent acceptance, perform canonical source,
  platform/package/native, installed and live qualification. Explicit maintainer
  approval is required before main promotion and release/publication closeout.
  This draft PR authorizes neither merge nor release.

The legacy four-session/two-wave exhaustion claims and Q17/no-broker bans were
discredited; do not reinstate them. Inventory counts are not admission bans.
Ordinary authorized investigation should not become repeated approval loops.
Resource forecasts may be re-estimated with evidence, preserving actual usage,
failed qualifications and high-waters. Genuine user limits and tool boundaries
remain distinct from internal forecasts. No formal study was admitted here.

## Source, remote and installed identities

| State | Identity and interpretation |
| --- | --- |
| Original research HEAD | `5e42cd99707025e17d577b7dcda8f32fb764cb8c`, branch `codex/018-header-comment-classification`. Six local commits follow common ancestor `76c94b7424a43ef8ddbb7790493e43a3a21abf9e`; HEAD alone did not contain the entire serving source. |
| Preserved serving snapshot | `cd3125e8e9634ebc2c2aaa89c243ea6e5406e50f`: commits all 102 changed files from the 109-file selected serving-source/manifests inventory. This is the source state used for v493/v494, before v495. |
| Newer development preserved | `2c85f93fa4d9d597959b375307dc820cc6a942e6`: Windows native launch from long paths, PR 2559. Merged at `022e7dcba5612cd0e16b7e86dd0015ab7965a452`; the only conflict was adjacent changelog entries, both retained. Its native launcher and debugging document were retained unchanged. |
| Source-only v495 WIP | `b95b37d7d41c226c7f060c1bd7562d1e117d0e25`: four generic query/refusal files. Subsequent handoff-only commits do not qualify this implementation. |
| Inspected main | `424d56b1b5452555460d528f40d08a231ac72a92`; not used as a replacement research baseline. |
| Last fully verified Dev installation (v494) | Both copies then had version 0.17.7, SHA-256 `66443951c3abfadcb660a66a851b112b86c523207b870392e9089e90005a8ddb`, normal CLI built from v493 selected serving source. **Does not contain v495 or the newer Windows merge.** |
| Fresh handoff observation | Dev staging binary still hashes to 66443951…; the former managed-cache CLI path is absent. Its cause and current host/registry state are unverified. No restoration or installation was attempted. |

The original research worktree/index and primary checkout were not reset or
rewritten. A fresh 109-file check found exactly the four expected v495 changes;
all other selected files matched the serving binding. This is a scoped inventory,
not an audit of every unrelated file on the machine. An additional 1,016 visible
tracked files outside that inventory matched their Git index blobs. The only
other visible untracked source was the native dependency described below.
Publishing only the four
latest files on old HEAD would have lost earlier work; this branch preserves the
full known composed candidate and its history.

The historically verified Dev roots were `$HOME/.codex/dev-plugins/codestory` and
`$HOME/.codex/plugins/cache/CodeStoryDev/codestory/0.17.7`; only the first CLI was
present in the fresh handoff check. Full old packages and
binaries remain locally archived. The install did not update source/remote by
itself. Installed CLI execution, plugin registration, running-host binary
binding, live retrieval and native isolation are separate proof tiers. The
public runtime SHA field was unexposed; requested Fast also has no exposed
effective-tier receipt. Never claim Fast or current-host candidate binding from
prose, a package hash, or a process name.

## Architecture and completed component work

The owning contracts are described in
[packet generalization](../architecture/packet-generalization.md),
[retrieval design](../architecture/retrieval-design.md),
[agent](../architecture/subsystems/agent.md),
[runtime](../architecture/subsystems/runtime.md),
[CLI](../architecture/subsystems/cli.md) and
[Store](../architecture/subsystems/store.md).

1. **Keep orchestration in runtime.** Syntactic retrieval planning may retain the
   unchanged question. Descriptor retrieval, admission, verified hydration,
   freshness retry, publication pinning and assembly are runtime-owned. The pure
   compiler consumes repository-derived typed evidence only, not prompt terms,
   tasks, roles, obligations or answer policy. Exact selector authority comes
   only from typed probes, never incidental question words.
2. **Preserve identity and bounds.** Project, source hash, Core generation/run,
   retrieval generation/input, request/parent and physical range are distinct.
   Admission remains 16 identities/16 KiB reserved source; public projection
   remains 16 rows/16 KiB. Source pages retain complete physical lines, unread
   bytes, authenticated next-line cursors and explicit unpageable-line refusal.
   Smaller verified extents are measured in the real caller envelope. Existing
   served prefixes and receipts cannot be silently shortened to fit new pages;
   missing progress is not answer sufficiency.
3. **Paging and continuations.** Generic source paging propagates actual delivered
   ranges through runtime, CLI and MCP. Continuation patches replace all fields,
   including explicit clears. Opaque option IDs are references, not access
   capabilities; parent presence currently does not prove stored offer membership
   or expiry. Partial-prefix restoration and verified-frontier corrections were
   causal repairs. v454 preserved all 12 original source rows and progressed an
   additional test body, but still omitted Cron.run. v449's earlier candidate
   preserved rows and added no source; that negative result remains.
4. **Source extraction/classification.** Header-comment classification and native
   callable/macro repairs are retained. The later bounded Go scanner handles
   division, raw strings and signature type braces separately from body braces,
   and requires a `func` anchor. Unsupported signatures/non-function anchors keep
   conservative fallback. `_test.go` is recognized generically, with vendor
   precedence and actual production-scope trail controls. Existing stored
   `Some(Source)` roles are still trusted; this does **not** migrate old indexes.
5. **Relations and projection.** Bounded graph traversal prioritizes certain edges
   to effective neighbors before hashed IDs and drains already bounded neighbors
   after truncation; caps/certainty stay unchanged. v460 delivered all four certain
   published Cron.run callees, still reporting 11 omitted edges. Derived component
   diagnostics retain graph identity/provenance but are excluded before Full
   source-result limits and defensively in public projection. This does not prove
   Core-only backfill or exhaustive graph coverage.
6. **Freshness/cache behavior.** Source drift retries revalidate within the original
   deadline; stale bytes still fail closed. Retrieval caches pin compatible
   publication identities rather than manually rewriting manifests. Compatible
   legacy-retention observation is limited to enumerated Core schemas 32–36,
   sharing manifest validation and WAL/recovery/lease/external-pointer fences;
   ordinary Store reads still require the current schema. Malformed/unsupported
   protection suppresses pruning. Inventory qualification was dry-run only.
7. **Root resource paging.** `codestory://symbols/root` returns a versioned object
   with `symbols`, `continuation_uri`, publication/inventory binding and remaining
   count. Each successful encoded MCP response is at most 8,192 bytes; batches
   also have an aggregate bound. Paging removes the resource's old 2,000-row
   upstream ceiling, while the ordinary symbols tool retains default 300/max
   2,000. Rendering/fingerprinting the full inventory remains eager per page;
   this is not bounded-memory or arbitrary-scale performance proof.
8. **Cache routing.** `serve --multi-project --cache-dir ...` used to accept and
   silently drop the exact override. The parser and typed startup validator now
   reject the incompatible pair before runtime creation. Bound single-project
   stdio preserves its exact cache; ordinary multi-project routing is unchanged.
   `serve --refresh none` is **not** a no-preparation fence for activating tools.
   Existing-only observational resources and ordinary activating tools differ.

## Native dependency preservation

The candidate's Cargo manifest uses a local path override for `llama-cpp-sys-2`
0.1.151. Its previously untracked upstream-plus-patch tree is a material dependency,
not a generated executable. The upstream crate SHA-256 is
`23c676b02f8f2bdd9d7df7bcca0bd176b27964440998e7aa31c6fe32552964d0`.
All 1,070 upstream files were compared. The final source has 1,071 regular files:
1,067 unchanged, three modified and one added header. The `.cargo-ok` cache marker
is excluded. All authored changes are preserved in the
[5,605-byte patch and manifest](../../vendor/llama-cpp-sys-2-patches/README.md),
patch SHA-256 `108dc72d03ef7e9f3df11da9234d450d1c5758ac4b5fff2ea881d184b36f067a`.
The complete regular-file postimage digest is
`2d24f6ebdd8eb5f979f6af989aef76d1a34a8d06077a0d3d0cfe2bfb65b1b5a1`.
The 22,969,666-byte reconstructed source copy is excluded from Git and recreated
from the pinned archive plus patch. The bootstrap and separate verification
actually reconstructed and matched that digest, without build, download, model
or install activity. Existing trees are preserved; files publish exclusively,
and verification rejects symlinks/special entries. Digest scope is regular-file
content, not timestamps/modes/empty directories.

The existing native repair rejects nontrivial operations on unknown/unselected
devices before compute, records successful compute/synchronization completion,
and carries immutable per-batch evidence to consumers. Positive completed
MUL_MAT evidence is required. The input-placement repair reuses the selected
repeating-layer device for complete offload; partial/no-device paths retain CPU
placement and the existing rejection. The original GET_ROWS refusal and local-
versus-agent/profile publication failures remain preserved. Counter-only mock
passes and a later real Full publication do not prove an independent Metal
timeline, vector-file physical checksum binding, or hard host isolation.

The local native source, caches, models and old binary packages remain untouched.
Do not remove the Cargo override, weaken CPU/device guards or silently substitute
upstream bytes merely to get a green build. The native source preservation is a
reconstruction requirement, separate from unfinished product qualification.
Current CI does not run the bootstrap, so its Cargo checks fail at the missing
path dependency; integrate a deliberate source-packaging policy during resumption.

## Unfinished v495 changes

`packet_plan.rs` retains the whole question as view zero and the existing literal
paragraph views. It fills remaining supplemental slots with deduplicated literal
sentence slices, uniformly sampling positions. At most seven supplemental views
plus the whole question are allowed. There are no English/domain importance
rules or inferred symbols. Quote/code delimiters, escaped quotes, contractions,
possessives, UTF-8, path/decimal dots, CRLF and repeated input have controls.
This is bounded punctuation handling, not complete natural-language segmentation.

`ranker.rs` makes `BroadQuestionV1` uniformly eligible across effective file roles.
The old pool-dependent filter could keep tests alone, then discard them when an
unrelated primary source appeared. Strict-query filtering and existing score
weights remain. Policy/cache receipt version is now
`weighted_rrf_v4_present_lanes_scaled_broad_eligible`. Baseline question 1 already
mentioned tests, so this filter is **not established as its original cause**.
The paired experiment changes two mechanisms together; it cannot isolate causal
contribution without separate evidence. Additional weak views may consume the
18-second budget or displace useful evidence; there is no delivered gain yet.

The separate compiler/browser change corrects the exhausted-page error path.
The baseline's exact advertised seven-option follow-up failed with
`internal: No requested page fits without losing authenticated context.` After
all extents fail, the adapter may preserve the actual measured BudgetExceeded
projection of the unchanged authenticated floor. It must have no new resolved
source or issuance seal, retain the baseline packet/support/disposition, and
require a measured byte requirement above the cap. Cancellation and current
owner/publication checks remain. A private exhaustion marker alone is not
capacity proof; missing measured frames retain the internal error. This change
does **not** improve retrieval selection or establish that advertised options fit.

The latest independent source review found no remaining source-visible blocker,
after correcting a plural-possessive splitter defect and the unmeasured-marker
classification. Final runtime controls, served-source hostile suite, normal CLI
build, real changed packets, independent changed answers and paired evaluation
remain unrun. Some architecture prose still describes v3/paragraph-only planning;
update it only to accurately describe the retained WIP, without promising gain.

## Evidence and failures that must keep their identities

| Development evidence | Verified result and practical limit |
| --- | --- |
| Original Cachelib | Baseline 0/4 versus lexical 2/4. Later exposed continued answer 4/4 is not fresh proof. |
| Original inih policy question | 0/4 fully supported, 3 partial, 1 omitted, 0 wrong. Later distinct C questions do not replace it. |
| v473 Go extraction | Causal regression failed before repair; final 13 focused + 76 protocol tests passed; plugin 157 passed/1 skip. Complete helper bodies delivered. Exposed answer 20/20 does not upgrade original fresh 8.5/20. |
| v474 fresh task | Failed 8/20, 1/8 fully supported, 0 wrong. Required paths remained unread; whole-chain latency and retention forecast also failed. Complete requested bodies did not establish complete investigation. |
| v476/v477 | Fresh adaptive case 20/20; literal citation check initially 9/10, separately corrected to 11/11. Summed CLI time below 18 s did not qualify contiguous whole-chain latency. |
| v482 projection/retention | Final corrected source 75 tests passed; 31 earlier executions are historical, not additional final proof. Dry-run inventory cleared five schema-32 observation errors; no cleanup performed. Same-query Full search improved 7 rows with a gap to 8 source rows without gaps, on the same publication. Exposed regression, not blind ranking gain. |
| v483/v484/v486 | Fresh 7/9 required, 1/2 optional, 0 wrong; feedback-exposed continuation 9/9, 2/2; later distinct fresh task 8/8, 1/3, 0 wrong. Guide delivery changed the procedure, not production ranking. No causal/cross-repository benefit claim. |
| v488/v489 paging | 97 Rust tests passed; plugin 157 passed/1 Windows-specific skip. Installed CLI delivered all 417 Cron roots over 13 complete JSON pages, max 8,184 bytes, across new processes. Second installed copy matched first/next pages. No new registered-host binding. |
| v493/v494 cache routing | Four focused controls passed; normal CLI SHA 66443951… built and both Dev copies installed. Each refused invalid flags and served retained inih generation d3860eff… distinct from default 5e5e1ca0…, with pointer/database hashes unchanged. |
| v490–v492 source answers | Pluggy 10/10, distinct inih Meson case 10/10, Cron/Pluggy join 16/17; no wrong facts. C had two implicit Core preparations; join omitted one direct test assertion. Feedback citation completion is separate. These Core workflows do not prove broad retrieval benefit. |
| v495 current tests | Final planner 15/15 and ranker 30/30 passed. Compiler exhausted-floor fixture 1/1 passed. Browser 3/3 is **historical before missing-frame correction**; final browser/history tests and changed-product comparison unrun. |

The [command/result record](018-investigation-handoff-evidence/commands-and-results.json)
contains exact sanitized saved argv and raw-output hashes for key lanes. A
prepared argv without a receipt is explicitly unexecuted. Historical focused
passes do not qualify the final composed head or every feature/platform. No new
implementation tests were run during this handoff.

Preserve these failed approaches: a non-progressing additive packet candidate;
partial-prefix transition failures; premature stopping after complete but
insufficient bodies; declaration-anchor crossover; plural possessive swallowing
later sentence views; treating an unmeasured marker as public capacity proof;
Core symbol-name fuzzy search as a substitute for broad behavior retrieval;
stale-manifest refusal despite equal document counts; and overinterpreting graph
truncation or Complete status as exhaustive evidence. Do not tune production to
the exposed missing names/ranges.

Operational failures also remain: target/source allocation forecast overruns;
wrong CLI context argument; wrong cache-acceptance JSON field; initial format and
compile failures; whole-workspace formatter timeout/interruption on an unchanged
file; and a rebuilt binary hash difference whose cause was not attributed. The
successful original cache response was decoded offline rather than rerun. One
older installation changed global configuration bytes, but authentic prior
contents were unavailable, so the cause remains unattributed. Later captured
byte/semantic comparisons were unchanged; they cannot repair that older gap.

The Oct9 registered-host root read delivered 227 intact visible rows but contained
host truncation in the middle of its wrapper. It established registration/data
delivery, not whole JSON, complete inventory, current binary binding or isolation.
It was terminal and must not trigger another host/read. Later direct installed
CLI paging is a distinct proof. The old unknown-server failure also remains.

## Reproduce and prioritize

Use an isolated checkout. Obtain the final published SHA from the draft PR and
pin it before work. These are resume commands, not commands executed in this
handoff beyond source preservation and documentation checks:

```sh
git clone https://github.com/TheGreenCedar/CodeStory.git CodeStory-wip
cd CodeStory-wip
git fetch origin codex/wip-018-investigation-handoff-20261013
git switch -c resume-018-investigation origin/codex/wip-018-investigation-handoff-20261013
# Replace FINAL_PR_SHA with the complete SHA verified in the draft PR.
git switch --detach FINAL_PR_SHA
git rev-parse HEAD
git submodule update --init --recursive
python3 scripts/bootstrap-research-native-vendor.py
python3 scripts/bootstrap-research-native-vendor.py --verify
```

1. Review the full diff and WIP semantics first. Run final browser controls and
   authenticated served-source hostile controls, then the two boundary scripts.
   The final planner/ranker passed on their exact four-file hashes; the Windows
   merge/final composed tree is not globally tested. Use serial locked Cargo lanes:

   ```sh
   cargo test --locked -p codestory-agent --lib packet_plan::tests
   cargo test --locked -p codestory-retrieval --lib ranker::tests
   cargo test --locked -p codestory-runtime --lib agent::packet_compiler::tests::exhausted_requested_page_preserves_the_measured_capacity_refusal
   cargo test --locked -p codestory-runtime --lib browser::packet_capacity_refusal_tests
   cargo test --locked -p codestory-runtime --lib agent::packet_served_sources::tests
   node scripts/lint-retrieval-generalization.mjs
   node scripts/check-packet-generalization-boundary.mjs
   ```

   Exact historical commands additionally use offline dependencies, two CPU jobs,
   pinned rustc 1.95.0, per-package test debug/codegen overrides and a retained
   target. Those argv are in the evidence record. Resolve placeholders locally;
   do not blindly reuse private cache paths or assume a new machine has assets.
   Read the model/build setup in the testing matrix before any build/download.
   Tests may perform isolated fixture preparation; they are not zero-activity
   proofs. Do not run the full workspace acceptance lane as routine draft proof.
2. Build a **normal** CLI and bind source tree, Cargo features/profile, executable
   and independent archive hash. The old build used `cargo build --locked
   --offline -p codestory-cli --bin codestory-cli --profile test` with saved
   overrides. `profile.test=false`, CLI features `[]` and runtime `[default]`
   distinguished its normal artifact from test support. Do not overwrite the
   installed 66443951… baseline. For a separate rebuilt baseline, make an isolated
   checkout at `cd3125e8e9634ebc2c2aaa89c243ea6e5406e50f`, bootstrap the same pinned
   native patch, and use the saved normal build flags. That old checkout predates
   the bootstrap bundle: copy `scripts/bootstrap-research-native-vendor.py` and
   `vendor/llama-cpp-sys-2-patches/` from the final handoff checkout into it first.
   This is an explicitly identified build-input reconstruction overlay; it does
   not change the baseline Rust logic. Name its
   new binary hash explicitly; rebuilding does not recreate or replace the
   original 66443951… receipt. Exact binary-byte reproduction is not established.

   ```sh
   git worktree add --detach ../CodeStory-baseline cd3125e8e9634ebc2c2aaa89c243ea6e5406e50f
   git archive FINAL_PR_SHA scripts/bootstrap-research-native-vendor.py vendor/llama-cpp-sys-2-patches | tar -x -C ../CodeStory-baseline
   (cd ../CodeStory-baseline && python3 scripts/bootstrap-research-native-vendor.py)
   ```

   Replace `FINAL_PR_SHA` with the verified handoff SHA. Cached source is used by
   default; `--download` is a separately explicit source download on a new machine.
3. Reproduce the deliberate two-question baseline/changed comparison before
   claiming retrieval improvement. Clone the exact public fixture:

   ```sh
   git clone https://github.com/robfig/cron.git cron-fixture
   git -C cron-fixture checkout --detach bc59245fe10efaed9d51b56900192527ed733435
   ```

   Use existing complete Core/Full publication if available and verified. On a
   new machine preparation is a **new** operation requiring a normal forecast;
   generation UUIDs cannot be recreated by editing manifests. The retained
   baseline used profile `agent`, run `shared-agent`, refresh `none`, standard
   16-row/16-KiB packet, 18,000-ms budget, no input probes. Core generation was
   `1731ee82-b30e-4c5f-8ced-9d87c6a809c5`, run
   `b5714e8a-1fa0-4acc-a69d-0d6d4baa390c`; retrieval was
   `24f5f6e74fca24e6-ae4ec05bdab9064a`, input SHA
   `ae4ec05bdab9064a3234655a64bfb06b08280c7efe7163d02caf6f0d2602d43f`.
   Preserve the questions byte-for-byte, binary identities and publication in
   both arms. Compare initial delivered source and actual assertions separately
   from adaptive answers. Never copy evaluator paths into initial probes.
   Preserve every failed call and inspect physical ranges rather than relying
   on `snippet_truncated=false`. Continuations must use newly returned exact
   actions/pins; the published sanitized old cursors are diagnostic fixtures,
   not current authority. Confirm all old issued source survives a new capacity
   refusal, no child issuance appears, and fitting continuations still work.
4. Freeze changed-reader answers/inventories before independent grading. The old
   baseline reader once saw incidental team-status metadata but no reference
   answers; disclose that limitation. Keep references withheld, forbid source,
   prior answers and team-summary access for a new blinded reader. The public
   handoff now exposes these development cases, so they cannot serve as new
   unseen acceptance questions. Author distinct ordinary questions after source
   convergence, with the prescribed independent protocol, instead of repeated
   equivalent queries or tuning to public failures.
5. If delivery remains insufficient or worsens, diagnose the existing retrieval,
   admission/hydration and measured selection path. Do not end at a better error
   message or manual fallback. Track losses and latency honestly; isolated CLI
   durations are not end-to-end answer latency. Resolve remaining stale stored
   roles, eager inventory cost and wire/context limits only with causal evidence.
6. After real convergence, assemble actual governing admission for the formal
   study. Platform/installed/live qualification and release approval come later.
   Effective Fast and native hard isolation remain unqualified. The expressly
   denied restrictive-profile initialize-only diagnostic must **not** be retried,
   rerouted through another task/executor, widened, or replaced with control-socket
   or private-state mutation. Owned process cleanup is not isolation proof.

## Excluded material and reproducibility limits

This preserves all **known authored candidate source**, including the native
patch and reproducible bootstrap. It is not a
public mirror of the private research workspace. The following remain local:

| Exclusion | Reason and how to resume without it |
| --- | --- |
| Large target trees, native libraries, GGUF assets, archived CLIs and full rollback plugin packages | Generated/platform-specific storage. Rebuild from the source checkpoints using documented setup; use current canonical installer only under separate install authority. Original binary hashes/normal-artifact identities are recorded, but old byte identity and rollback packages require the originating retained archive. |
| Unchanged upstream native source copy | About 23 MB; all authored differences are checked in as the pinned source patch. The bootstrap reconstructs 1,071 regular files and verifies the complete content digest from the original checksum-pinned crate. No native authored change is left only local. CI packaging/bootstrap integration is still unfinished. |
| SQLite/WAL, Core/retrieval caches, pointers, leases and CAS inventories | Machine/publication state, potentially private source; no raw DB or authority state published. Recreate an isolated publication from pinned public fixtures with normal supported preparation. Old UUIDs/timing/pointer-before-after results cannot be recreated exactly and remain historical. Never privately rewrite state to match receipts. |
| Raw supervisor/installation/host receipts, configuration snapshots and global environment | Contain machine paths or private state. Relevant assertions, commands, hashes and failures are consolidated here. Exact historical lifecycle, physical-allocation and native-proof audit still requires local originals; no stronger proof is claimed. |
| Withheld v495 references and other evaluator banks/keys | Preserve evaluator separation; do not ship expected answers into production. The original v495 reference seal is `e883015f14cff2bc399d6c6fc873de5fe005535129e8bf1c62bca412fac23427`. For an independent resumed development grade, use the retained sealed reference or have a new source-only evaluator author a rubric before seeing changed answers. Public cases are already exposed. |
| Private chat, internal coordination/checkpoint notes, credential/configuration data | Not necessary engineering inputs. No secrets or chat transcript published. Useful decisions, failures, criteria and next steps are consolidated in this document. |
| Older prototype lanes/failed binaries and unrelated worktrees/untracked files | Protected historical evidence or unrelated user work, not part of the intended candidate. Not deleted, reset, staged or published by this handoff. The main checkout's unrelated untracked work remains untouched. |

Small published packet/source excerpts originate from the pinned MIT-licensed
Cron repository; its [license](018-investigation-handoff-evidence/CRON-LICENSE.txt)
is included. Machine-root substitutions are explicit in the
[sanitization manifest](018-investigation-handoff-evidence/sanitization-manifest.json).
Published transformed bytes are not presented as original lossless receipts.

Continuous counters carry **270/272**; this handoff adds no project operation,
answer attempt, model/index, host diagnostic or install. Do not reset them in a
new thread. The private `continuation-labels-v410/plan.json` and checkpoint retain
their complete lineage. Last recorded target high-water was 26,509,037,568 bytes,
charged aggregate 54,041,280,512 bytes; source high-water 27,357,184 bytes; package
copy charge 22,922,160,640 bytes. CAS remains 128 MiB/scope, 512 MiB aggregate,
with lease/no-eviction rules; four allocations are inventory, not a four-scope ban.
Retain protected inputs, old failures and archives. The handoff checkout/evidence
is a separately measured source-only allocation, not a refund of old storage.
Take fresh disk/memory/shared GPU coordination before resuming execution; old
free-space samples and internal forecasts are not permanent capacity or approval.

The draft PR/linked handoff issue carry the final remote SHA, cross-links and
current CI state. A CI pass on this handoff, if obtained, cannot substitute for
the unfinished packet comparison, broad independent acceptance or release gates.

## Handoff publication checks and CI

The handoff's documentation-link and whitespace checks passed. Its native
bootstrap/verification round trip passed; this is source reconstruction, not
native execution. Independent review corrected omitted small supporting frames,
byte-count disclosure, baseline checkout reconstruction and no-clobber/symlink
handling. No additional Rust/product qualification was run.

The first published handoff head was
`e0eb4ab03c7ee04bed452deda93be4f4086d772f`. Its hosted checks had five failures,
two passes and one skip: draft source, durability, retrieval smoke and rustdoc
fail at the absent native path dependency; plugin static had 156 passes,
one skip and a timeout in `release asset downloader retries a transient failure`
under its 100 ms total fixture budget. Docs links and closing-issue guard passed;
the Windows missing-manifest lane skipped. Original logs remain retained. The
native packaging and plugin fixture failures were not fixed or rerun as an
implementation project. The final head's new CI state is reported separately in
the PR and issue; do not transfer earlier green checks to that head.
