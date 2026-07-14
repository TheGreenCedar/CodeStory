# Changelog

## Unreleased

### Changed

- Delegated worktree setup now prepares and reports the local repository map
  without attempting full retrieval or printing backend repair instructions.
  Maintainers can request that separate proof explicitly with
  `--full-retrieval-proof` (or `-FullRetrievalProof` in PowerShell).
- Cold and stale repositories now present `ground`, `files`, and `affected` as
  direct activation paths. One grounding call builds or refreshes the local map;
  agents no longer get sent through a status loop before repository navigation.

## 0.15.0

### Highlights

- Agents now have one managed path from opening a project through indexing,
  packet construction, and search. Fresh plugin sessions can show diagnostics
  while the matching CLI is installed, then bring stale projects current
  through normal grounding instead of a separate repair command.
- Index and retrieval publication is atomic. Readers stay on one complete
  generation, failed or interrupted refreshes leave the previous generation
  usable, and source changes during a refresh cannot publish stale results.
- Packet, search, and context now fail closed when their publication, sidecars,
  embedding endpoint, process identity, or required accelerator proof is stale
  or ambiguous. Local graph navigation remains available when only deep search
  is warming or degraded.
- Every plugin request selects its project explicitly. Switching repositories
  cannot inherit another project's cache, endpoint, operation, or readiness
  evidence, including on case-sensitive Unix filesystems.
- Grounding is more precise for request flows, configuration files, framework
  routes, and compact executable paths. Diagnostic file hits no longer hide
  resolved graph evidence, while real source gaps still prevent an unsupported
  sufficiency claim.

### macOS

- macOS 15 and newer is a supported native target. Apple Silicon uses the
  checksum-pinned managed embedding server with Metal. CodeStory verifies the
  selected endpoint, process identity, GPU offload, and a timed embedding
  request before opening agent retrieval.
- Intel Macs support the native CLI, managed plugin, local graph, and grounding.
  Semantic retrieval requires explicit CPU/external policy; that policy may
  select a trusted external embedding endpoint. Intel status and evidence never
  claim Metal acceleration.
- Managed readiness follows the selected dynamic endpoint, reuses one matching
  live embedding process when projects switch, and blocks packet and search
  with repair guidance if the endpoint or verified accelerator identity
  disappears or changes.
- Direct-download Mac artifacts are release-ready only after the exact arm64 and
  x64 binaries pass Developer ID signing, notarization, quarantined extraction,
  and native execution checks. Release proof retains `spctl` diagnostics but no
  longer mistakes its bare-CLI "not an app" result for notarization failure.
  Source or unsigned package checks do not make that claim.

### Reliability and operations

- The legacy ONNX backend and its runtime, installer, setup, configuration, and
  environment selections have been removed. Use managed llama.cpp under the
  host's configured accelerator/CPU policy, or a trusted external endpoint
  under explicit CPU/external policy. Stale ONNX settings now fail with
  migration guidance instead of being silently ignored.
- Existing retrieval rows migrate in place. Ambiguous legacy sidecar or broker
  state is retained for bounded inventory and cleanup but is not reused; deep
  search may require a managed repair or rebuild after upgrade.
- Incremental indexing verifies file content rather than trusting modification
  times alone. Retrieval reads remain pinned to one graph and sidecar
  publication, and a concurrent publication change returns a bounded retry
  instead of mixing evidence from different generations.
- Lexical search avoids full-corpus validation on every query, and framework
  route discovery handles multiline imports, aliases, comments, and shadowing
  without repeatedly rescanning a module.
- Status, doctor, and malformed MCP resource reads remain observational: they
  do not refresh projects, start repair, or mutate sidecar state. Normal repair
  guidance stays in the managed MCP flow, with CLI commands retained for
  operator diagnostics.
- Native processes, ports, generations, downloads, and cleanup are tied to
  verified CodeStory ownership. Dead or reused processes, conflicting legacy
  state, unsafe paths, oversized downloads, and ambiguous cleanup all fail
  closed instead of being guessed through.
- Release-evidence re-evaluation stays behind its protected environment gate,
  accepts only the named secret explicitly passed by the release coordinator,
  and fails before evaluation when that approval is missing. PR packaging never
  receives the approval secret.
- The packaged plugin launches the native CodeStory CLI without a shell. On
  Windows, `CODESTORY_CLI` must name a native `.exe`; the supported `codex.cmd`
  host shim is unchanged. Cross-platform worktree setup uses one
  version-checked path on Windows, macOS, and Linux.
- Linux x64 requires glibc 2.31 or newer. Both Linux architectures receive
  packaged execution checks; accelerator and full-retrieval claims still
  require live sidecar evidence for the selected host.
- Repository-controlled network endpoints remain disabled by default. Setting
  `CODESTORY_ALLOW_PROJECT_NETWORK_CONFIG` is an explicit trust decision that
  permits the repository to choose summary and embedding egress endpoints.
- Release proof is staged by maturity: focused checks run during development,
  while exact-head platform, protected hardware, signing, installation, and
  live-runtime evidence run at their promotion or publication boundaries.

### Release boundary

- The repository version and release notes describe the candidate source.
  Publication is complete only when the matching archives and checksums are
  available and the required platform checks pass.
- Marketplace availability, quarantined native execution, installed runtime/version
  readback, and live full-retrieval behavior are separate post-publication
  claims and require their own evidence.

## 0.14.3

### Fixed

- Added native five-asset pre-publish and post-publish acceptance evidence,
  including Windows x64 managed plugin provisioning, local grounding, repair
  handoff, and installer ownership proof. Release notes and contributor
  guidance now preserve Apple Silicon, Windows arm64 acceleration,
  older-glibc, marketplace, and full-sidecar proof boundaries.
- Declared glibc 2.31 as the minimum supported Linux x64 userspace and added a
  pinned Debian Bullseye build plus Ubuntu 20.04 packaged-archive execution
  gate for version, help, and stdio initialization with retained loader and
  symbol diagnostics.
- Kept strict sidecar readiness fail-closed on interrupted index-run markers
  without globally rejecting completed generations that contain parser-partial
  files or repeatedly refreshing unchanged parser-partial inputs; parser
  coverage remains visible through file diagnostics.
- Standardized JSON-mode CLI failures on one versioned error envelope, including
  argument parsing, ambiguity, smoke checks, runtime failures, and background
  repair terminal state. Compacted MCP status by referencing canonical
  readiness verdicts instead of cloning them across every surface, and aligned
  operator guidance with local defaults, managed dynamic state, supported
  accelerator cells, and host-reload boundaries.
- Isolated CLI integration tests from user caches, install identity, plugin
  data, stdio state, sidecar port registries, and managed runtime roots. Test
  processes now share one explicit per-process state root, broker unit tests
  inject machine-state roots across worker threads, and a regression invariant
  rejects direct CLI process construction that bypasses the isolation helper.
- Compacted stale agent sidecar port allocations atomically under the existing
  registry lock while preserving live, state-backed, recently reserved, and
  unverified owners. Native embedding launches now attempt to preserve only a
  bounded previous log tail and truncate the current log without blocking
  startup when housekeeping fails, while accelerator proof reads only a bounded
  current tail.
- Bounded plugin-managed CLI storage to the active checksummed runtime plus one
  verified upgrade or rollback candidate, with atomic staged publication,
  recoverable owner locks, Windows lock-safe cleanup, and retained, removed,
  and reclaimable byte diagnostics in plugin runtime status.
- Added post-publication sidecar generation retention across Qdrant, lexical,
  and SCIP artifacts. CodeStory now protects every manifest-referenced active
  generation sharing the sidecar scope plus at most one verified rollback,
  suppresses pruning on malformed or stale protection state, coordinates
  publication with namespace GC, and exposes the same byte-accounted plan to
  `retrieval inventory` and its explicit `--apply` path.
- Published lexical shard data, metadata, and runtime sidecar state through
  validated same-directory temporary files with atomic replacement. Shard
  readiness and search now reject malformed, truncated, count-mismatched, or
  hash-mismatched JSONL; bind published bytes and metadata to the manifest's
  sidecar input; and recheck live lexical input before manifest publication so
  failed or raced rebuilds preserve the last known good shard.
- Added a `dev/codestory-next` merge workflow that closes same-repository
  issues named by `Closes`, `Fixes`, or `Resolves` in merged pull requests.
- Made live incremental indexing crash-safe. Runs now persist an incomplete-run
  marker and a transient cross-version schema fence before mutating graph
  projections, exclude concurrent writers across processes, retry unchanged
  `complete=false` files, and clear the marker only after resolution and both
  grounding snapshot tiers succeed. Failed or cancelled runs report stale,
  cannot serve strict sidecar retrieval or cache rehydrate, and recover through
  the existing staged full-refresh publish path on the next refresh.
- Separated release-update advice from runtime readiness. A newer GitHub
  release or a newer checksum-valid managed CLI now appears under the
  non-blocking `runtime_update` status field without disabling compatible local
  graph or agent surfaces; an installed newer runtime recommends a host reload.
- Removed GitHub release discovery and CLI subprocess probes from the status
  request path. Release metadata is cached for six hours, refreshed in the
  background under a cross-process lock, and remains advisory when offline,
  rate-limited, stale, or malformed.
- Made MCP sidecar repair transfer one durable attempt reservation to the
  spawned CLI worker instead of rejecting its own handoff as a competing
  repair. The worker now inherits the parent cache scope, and MCP records its
  terminal exit code plus bounded stdout/stderr tails before clearing repair
  ownership.
- Removed the production semantic-document domain-alias catalog that encoded
  benchmark-shaped Root & Runtime and Sourcetrail answer text. Historical
  retrieval packets built with that catalog are no longer promotion evidence;
  the semantic-document schema now invalidates the affected persisted data, and
  semantic documents and sidecars must be regenerated.
- Consolidated semantic-document leakage enforcement into the retrieval
  generalization guard, which now derives prompts, expected and forbidden
  claims, paths, and symbols from every benchmark manifest plus the checked-in
  script prompt, cross-repo query, and eval-only probe corpora, and fails closed
  when a registered corpus is missing, malformed, or only partially parsed.

## 0.14.2

### Fixed

- Made the managed macOS arm64 Metal embedding launch emit the verbose llama.cpp
  initialization lines required for runtime-backed GPU proof.

## 0.14.1

CodeStory 0.14.1 is a hotfix for concurrent Codex tasks working in different
repositories through the same plugin installation.

### Fixed

- Replaced plugin-startup workspace binding with request-scoped MCP routing.
  Every MCP tool now requires an explicit `project`, and one stdio server can
  safely process interleaved requests for multiple repositories without reading
  another task's thread or global active-workspace state.
- Added a project-scoped `status` tool and carried the selected project through
  readiness, repair, retry, hook, skill, and agent-guide recommendations so
  follow-up calls cannot silently switch repositories.
- Added a two-repository stdio regression that queues requests for both projects
  before reading their responses, then switches back and verifies each result
  remains rooted in its requested repository.
- Updated the packaged release proof to pass its repository explicitly when it
  reads project-scoped MCP status.

## 0.14.0

CodeStory 0.14.0 turns the 0.13.x MCP recovery fixes into a durable readiness
contract. Compared with 0.13.12, agents now get lane-specific status, explicit
sidecar setup controls, GPU proof, and repair guidance that stays honest when
the installed runtime is stale, busy, or unavailable.

### Added

- Added a durable readiness broker for CLI/MCP repair state, with cross-process
  snapshots for active repairs, abandoned repairs, local refresh cleanup,
  machine-scoped native embedding locks, resource ownership, and GPU proof.
- Added explicit MCP `sidecar_setup` status/config/repair guidance for
  packet/search sidecars, including enabled/disabled/ask policy states and
  diagnostic fail-open status when the real stdio runtime cannot start.
- Added accelerator proof fields to status output so operators can see the
  requested device, observed device, provider, live embedding smoke result,
  elapsed smoke time, and degraded reason before trusting agent packet/search.

### Changed

- Made the CLI entrypoint and stdio transport async with Tokio, including
  bounded blocking work, ordered responses, frame limits, queued-request
  cancellation, and canceled-response suppression without changing command
  names or MCP wire contracts.
- Consolidated outbound sidecar HTTP status/body handling behind shared `ureq`
  helpers while keeping the loopback browser HTTP server unchanged.
- Added versioned project, workspace, and artifact-scope identity to readiness
  snapshots and sidecar ownership. Existing cache namespaces remain compatible,
  and dirty or unidentified worktrees fail closed to workspace-local reuse.
- Split readiness into local-navigation and agent-packet/search lanes so stale
  local indexes, sidecar policy, native embedding ownership, and packet/search
  readiness no longer collapse into one ambiguous ready/not-ready result.
- Made status reads observational: MCP status no longer starts background
  sidecar repair by itself, and recovery instructions point agents to the
  allowed `sidecar_setup` action instead of raw CLI repair commands.
- Made ready-repair and retrieval bootstrap share native embedding ownership
  rules. Existing CodeStory-owned sidecars can be reused only when recorded
  PID/launch metadata still match, and reuse-only repairs do not tear down
  sidecars they did not start.
- Updated plugin/operator guidance for the current status contract, including
  `gpu_proof` interpretation, diagnostic fail-open limits, stale workspace
  behavior, and Windows `codex.cmd` invocation.

### Fixed

- Hardened durable readiness repair ownership so live local-refresh and ready
  repair locks are not age-reclaimed, concurrent MCP repair is single-flight,
  and intentionally preserved live ownership reports an explicit orphan reason.
- Made readiness locks and broker snapshots publish atomically, including safe
  native lock handoff and Windows replacement, without changing PID, TTL,
  heartbeat, or ownership semantics.
- Required runtime/log-backed accelerator observation plus a successful smoke
  before GPU proof is verified; device inventory and operator assertions remain
  diagnostic evidence only.
- Made explicit `native_spawned` selection reject compose-only Linux backends,
  and exposed the durable broker snapshot consistently through `doctor`,
  `retrieval status`, and plugin bootstrap status.
- Prevented installed MCP from binding to another workspace's global active-state
  file when the host has a current Codex thread but no matching thread-scoped
  active-state file yet.
- Prevented stale repair records, malformed machine locks, PID reuse, stale
  local-refresh status, and broker-lock races from making CodeStory surfaces
  look busy or ready after the owning process has gone away.
- Preserved live GPU smoke proof through final broker snapshots and refused to
  report accelerator-required packet/search as verified without a live timed
  embedding smoke.
- Kept native llama.cpp sidecars alive and accurately owned across Windows,
  macOS, Linux, endpoint reuse, `retrieval up`, `retrieval down`, failed repair,
  host job-object restrictions, and Unix shutdown.
- Prevented detached Windows llama.cpp sidecars from retaining the CLI's
  redirected standard handles, so callers capturing bootstrap output can
  complete while the owned sidecar remains alive.
- Fixed Apple Silicon Metal source contracts so managed llama.cpp installs
  accept the upstream macOS payload, existing cached models are discovered, and
  Metal logs can prove accelerator-required packet/search readiness without
  manual device assertions; live post-repair endpoint survival still needs
  macOS arm64 validation before release-level hardware proof.
- Made diagnostic fail-open mode reject fake repairs, keep only safe status and
  setup surfaces callable, and mark old repair history stale when it points at
  an old CLI version or path.
- Made deprecated `repair_all` obey its blocked status while returning canonical
  `sidecar_setup repair` guidance for compatibility callers.

## 0.13.12

CodeStory 0.13.12 fixes the installed plugin MCP recovery path so Codex can
reconnect to the runtime after startup drift without getting trapped in stale
diagnostic mode or foreground sidecar repair.

### Fixed

- Made diagnostic fail-open MCP re-read plugin active-state files and hand off
  to `codestory-cli serve --stdio` once a project root appears after startup,
  instead of freezing the stale startup diagnosis.
- Detached Windows native `llama-server.exe` embedding sidecars from the ready
  repair process so semantic endpoints can survive after repair exits, or fail
  spawn before claiming agent packet/search readiness.
- Reused an already-healthy native embedding endpoint during sidecar bootstrap
  so repeated CLI or MCP repairs do not spawn extra `llama-server.exe`
  processes for the same agent sidecar port.
- Made stdio `sidecar_setup repair` start the Rust repair in the background so
  long sidecar rebuilds do not block the MCP request past the host tool timeout.

## 0.13.11

CodeStory 0.13.11 fixes Windows native sidecar repair after the Linux
accelerated backend work, keeping Windows on native Vulkan `llama-server.exe`
without requiring Linux device mounts.

### Fixed

- Fixed Windows native sidecar repair so Vulkan acceleration does not require
  the Linux `/dev/dri` Compose override when Compose is only starting Qdrant and
  Zoekt, managed llama.cpp installs restore the full Windows DLL payload, and
  b9902 native sidecars can prove the requested Vulkan device through
  `llama-server --list-devices`.

## 0.13.10

CodeStory 0.13.10 defines the Linux accelerated sidecar backend matrix while
keeping Linux launch behavior fail-closed until host GPU devices and live proof
are available.

### Changed

- Added explicit Linux accelerated sidecar backend cells and Docker launch
  diagnostics so Vulkan requires a host `/dev/dri` render node, while CUDA,
  HIP/ROCm, SYCL, and OpenVINO remain contract-only until live GPU evidence is
  attached.

## 0.13.9

CodeStory 0.13.9 moves Windows native llama.cpp sidecar selection onto the same
manifest-backed backend contract used by Apple Silicon.

### Changed

- Moved Windows x64 Vulkan native llama.cpp sidecar selection onto the
  manifest-backed backend resolver while preserving the existing b9058 managed
  cache path as a legacy fallback.

## 0.13.8

CodeStory 0.13.8 fixes Apple Silicon sidecar acceleration by launching a native
Metal llama.cpp embedding sidecar on macOS arm64, hardens stale MCP workspace
detection, and updates release artifact actions to Node 24-backed versions.

### Changed

- Pinned release artifact upload/download actions to Node 24-backed versions so
  release runs stop emitting Node 20 deprecation annotations.

### Fixed

- Made installed CodeStory MCP detect when its live stdio child is serving a
  stale workspace, report `workspace_mismatch` diagnostics, and block stale
  repo repair commands until the host relaunches MCP for the active workspace.
- Added a macOS arm64 Metal llama.cpp backend resolver so Apple Silicon
  accelerator-required sidecars launch natively without inheriting the Windows
  Vulkan device default.
- Added managed macOS arm64 Metal `llama-server` install/checksum handling so
  native sidecar launch only uses manifest-verified managed binaries.
- Documented Apple Silicon sidecar repair/status interpretation so operators
  and agents do not treat Docker Vulkan or CPU fallback as the default macOS
  acceleration path.

## 0.13.7

CodeStory 0.13.7 fixes automatic first-start sidecar repair when a stale
ready-repair record was left behind by an earlier failed startup.

### Fixed

- Let status auto-repair retry past abandoned ready-repair records when plugin
  sidecar setup is enabled, so stale startup state cannot permanently block
  agent packet/search repair.

## 0.13.6

CodeStory 0.13.6 fixes fresh Codex plugin MCP startup without the removed hook
bridge, and hardens explicit sidecar repair so first-use threads can recover
through the live MCP server.

### Changed

- Added Cargo registry/source/build-output caching to the default Rust CI
  workspace checks so promotion PRs can reuse the cache already seeded by
  sidecar smoke jobs instead of recompiling the workspace from scratch.

### Fixed

- Removed the Codex hook bridge from the shipped plugin path. Codex hooks now
  only record active project state and route agents back to live CodeStory MCP
  resources; status reads can start Rust-owned sidecar repair without
  hook-injected substitute grounding.
- Kept the plugin MCP launcher alive in diagnostic fail-open mode when the
  delegated `codestory-cli serve --stdio` runtime exits nonzero, so Codex gets a
  `codestory://status` diagnostic instead of a closed transport.
- Let the MCP launcher accept a fresh hook-written global active project even
  when Codex gives the MCP process a stale thread id, fixing fresh app-server
  threads that ran CodeStory hooks but still reported `project_root_unavailable`.
- Let sidecar-backed search return resolved full-mode hits when a later
  expansion stage reaches its deadline, matching the packet path instead of
  rejecting otherwise usable MCP search results.
- Let explicit MCP repair retry after an abandoned sidecar setup record and
  return the foreground repair result instead of leaving the agent with only a
  detached setup worker to poll.
- Treat dead ready-repair owner processes as abandoned immediately so fresh MCP
  repairs do not wait behind stale sidecar setup locks.
- Let `codestory://status` start MCP-owned sidecar repair when plugin setup is
  enabled and agent packet/search readiness is blocked, then recommend status
  rereads instead of hidden tool calls.

## 0.13.5

CodeStory 0.13.5 makes first-turn Codex hook bridge proof explicit enough for
fresh threads to distinguish runtime readiness from packet sufficiency.

### Fixed

- Added Rust-owned retrieval-status evidence to the hidden-MCP hook bridge after
  a packet succeeds, so the visible bridge reports `agent_packet_search=ready`,
  sidecar mode, and the Vulkan accelerator request without duplicating Rust
  readiness logic in the hook.

## 0.13.4

CodeStory 0.13.4 fixes the remaining first-turn Codex hook bridge failure found
after the 0.13.3 release proof.

### Fixed

- Stopped hidden-MCP startup hooks from injecting `ground` output, which could
  report local symbolic retrieval and mask the repaired agent sidecar state.
- Made hidden-MCP prompt hooks call the managed Rust `packet` path directly
  instead of rerunning a duplicate short-timeout `ready --goal agent` probe.
- Extended the hook packet timeout to cover the measured Rust packet path while
  preserving fast status-only startup when Codex still hides live MCP tools.

## 0.13.3

CodeStory 0.13.3 fixes first-turn Codex plugin startup truth after the 0.13.2
Windows dogfood run.

### Fixed

- Tightened Codex startup guidance so model-hidden CodeStory MCP sessions make
  host deferred discovery/tool_search the first repository-work action before
  manual source reads, keeping the hook bridge as a last-resort status label.
- Made hook bootstrap status ask the managed Rust runtime for the shared-agent
  readiness lane, so hook-bridged `runtime_truth` reports packet/search as
  `full` when `ready --goal agent` proves the sidecar instead of fabricating an
  unavailable agent lane from local/default status.
- Made retrieval bootstrap force-recreate CodeStory-owned Docker sidecars once
  when post-start health probes still fail, covering stale Windows Docker port
  proxies where containers are up but Zoekt is unreachable from the host.

## 0.13.2

CodeStory 0.13.2 fixes Codex plugin startup lifecycle failures found during
Windows dogfooding of the 0.13.1 release.

### Fixed

- Moved long-lived Codex plugin MCP wrapper processes out of the installed
  plugin cache directory before serving so Windows plugin refreshes are not
  blocked by the wrapper's current working directory.
- Blocked MCP `repair_all` when `codestory://status` reports a stale active
  CLI setup repair, preventing old runtimes from launching sidecar repair.
- Updated Codex hook and user guidance for model-hidden plugin MCP sessions to
  request host deferred discovery/tool_search when available instead of
  treating reload as the first repair step.
- Removed explicit `@CodeStory` tags from Codex marketplace prompt examples so
  the installed plugin advertises natural fresh-thread usage.

## 0.13.1

CodeStory 0.13.1 hardens the portable plugin/runtime path across Codex,
Cursor, Claude Code, and GPU hosts while keeping MCP visibility truth explicit.

### Fixed

- Added a hook MCP bridge for launchable-but-model-hidden Codex MCP sessions so
  hooks inject bounded `codestory://status` truth and optional hook-bridged
  context without claiming live MCP tools are model-visible.
- Removed ambient CodeStory CLI discovery diagnostics and repair guidance from
  the plugin adapter, stdio status, readiness output, installer, benchmark
  harness, tests, and agent-facing docs while preserving `CODESTORY_CLI` as an
  explicit local-development override.
- Added `CODESTORY_PLUGIN_DATA` as the portable managed-runtime data directory
  for non-Codex hosts, with Cursor and Claude Code examples that use managed
  runtime state directly.
- Made llama.cpp acceleration Vulkan-first by default with `Vulkan0` and GPU
  layer requests when CPU mode is not explicitly allowed, plus Linux compose
  `/dev/dri` access and operator docs for native/external endpoints on
  Windows/macOS.
- Expanded release and post-publish package proof to download every shipped
  binary archive, verify checksums, run version/help smoke, and validate stdio
  status shape while keeping full sidecar proof on runners that can support it.
- Refreshed packaged proof stdio status after sidecar repair and cleared cached
  proof output before each run so release artifacts reflect the current proof.

## 0.13.0

CodeStory 0.13.0 promotes the current `dev/codestory-next` MCP readiness and
agent-repair hardening work onto `main` as the next synchronized release.

### Fixed

- Rewrote agent-facing CodeStory guidance to make MCP status plus `repair_all`
  the single supported repair loop, with CLI commands labeled as
  maintainer/debug transcripts.
- Added `codestory-cli fix` and MCP `repair_all` as the single supported
  readiness repair entrypoint, with status recommendations collapsed to one
  repair action plus a `codestory://status` readback.
- Let the installed plugin MCP launcher use fresh active-project state even
  when the host hook cannot attach a Codex thread id or the state predates the
  MCP process start, while still rejecting state owned by another thread, and
  give local wait-fresh enough bounded time to pass on the CodeStory repo.
- Made packaged and post-publish agent proof fail when the installed plugin MCP
  launcher omits server-advertised `codestory://status` or
  `codestory://agent-guide` resources, while leaving true Codex host/model
  visibility proof open.
- Blocked CodeStory grounding when the plugin MCP is launchable but not
  model-visible, even when a managed CLI exists; diagnostic fail-open mode now
  exposes status/repair guidance instead of normal grounding tool names.
- Disabled ambient `PATH` CLI fallback for installed plugin runtime launches;
  missing managed CLI setup now stays in `managed_unavailable` diagnostics while
  preserving `CODESTORY_CLI` as an explicit local-dev override and keeping PATH
  checks documented as CLI diagnostics only.
- Bounded required-probe citation promotion by deduplicating probe queries and
  using set membership for promoted citation indexes, with a regression guard
  for large synthetic packet capping.
- Blocked agent packet/search readiness when the selected sidecar retrieval is
  not full, while keeping local/default graph readiness reported separately.

## 0.12.6

CodeStory 0.12.6 promotes the current `dev/codestory-next` release automation,
local-refresh coordination, retrieval hardening, packaging proof, and operator
guidance work onto `main` as the next synchronized patch release.

### Fixed

- Taught Auto Release to retry the current source version when a previous
  publish failed before creating a tag or release, while still refusing to
  overwrite existing release state.
- Taught Auto Release to reject version downgrades and guarded `main`
  promotions so release PRs must come from `dev/codestory-next`.
- Ignored Python bytecode caches emitted by local release-script checks.
- Removed shell execution from default file-open fallbacks so repo paths with
  shell metacharacters are passed as process arguments.
- Shared retrieval file-role state across strict batch workers so cache-miss
  fan-out does not clone the repo-wide role map per worker.
- Kept retrieval bootstrap Qdrant repair on the selected sidecar runtime so
  explicit agent profiles and run IDs do not fall back to ambient/default
  runtime layout during collection repair.
- Compacted local-refresh status across ready, agent preflight, and
  `codestory://status` so agent-facing output uses refreshed/refreshing/skipped
  states while maintainer JSON keeps stale freshness details.
- Added a project-scoped single-flight lock/status for local refresh so
  concurrent Codex/plugin processes report refreshing or skipped_locked instead
  of launching duplicate incremental indexing.
- Bounded stdio/MCP local-refresh waits so `ground` returns compact
  `local_refresh` repair guidance instead of consuming the full tool timeout on
  stale indexes.
- Let `agent preflight` run one bounded quiet local refresh so repairable stale
  local indexes report refreshed local graph readiness while packet/search stays
  fail-closed.
- Surfaced non-default active agent repairs in stdio status and `sidecar_setup`
  so MCP repair does not spawn a duplicate shared-agent repair for the same
  project.
- Moved Codex worktree setup from local/default retrieval bootstrap/index steps
  to the shared agent readiness lane used by MCP packet/search.
- Serialized first-boot agent sidecar port allocation through a cache-backed
  registry so concurrent namespaces do not choose duplicate dynamic ports before
  state files exist.
- Reported stale ready-repair status as abandoned sidecar work in MCP status
  with bounded inspect and cleanup commands instead of hiding aborted repairs.
- Skipped oversized parser-backed source files before reading their full body
  while persisting a nonfatal incomplete-file indexing error.
- Smoked every release matrix archive after packaging by unpacking it and
  verifying packaged `codestory-cli --version` before upload.
- Pinned the manual post-publish release smoke checkout to the requested release
  tag so older release proofs do not drift with the current branch.
- Narrowed owner-alias call resolution through exact/suffix candidate maps so
  repeated owner-qualified member calls avoid scanning every candidate node.
- Extended release readiness warnings to compare retrieval index/status/search
  timings against the latest stats-log baselines.
- Normalized release archive metadata and pinned the release Rust toolchain so
  package-twice checksum proof can catch reproducibility drift.
- Collapsed stale sidecar disable/profile production branches so retrieval
  follows the mandatory sidecar path while benchmark-contract setup keeps
  stale-environment rejection guidance.
- Removed deprecated benchmark, sidecar bootstrap, snapshot publish, and
  workspace alias compatibility surfaces after moving guards to maintained task
  manifests and concrete types.
- Extracted CLI doctor readiness fallback status selection into the shared
  readiness helper so rendering stays separate from local/default versus
  agent packet/search readiness truth.

### Documentation

- Documented Codex plugin refresh recovery for Windows cache-backup
  `Access is denied` failures and clarified marketplace snapshot vs package
  refresh vs runtime reload.
- Added contributor guidance requiring changelog updates before commits that
  change shipped behavior, operator guidance, release automation, packaging, or
  unreleased/latest version metadata, and requiring marketplace repo pushes when
  Codex needs to detect plugin updates.
- Restructured operator docs under `docs/users/` (host guides, CLI reference, troubleshooting, trust and readiness).
- Added CI markdown link checker: `node .github/scripts/check-doc-links.mjs` (`.github/workflows/docs-link-check.yml`).

## 0.12.0

CodeStory 0.12.0 promotes the managed plugin runtime path for Codex. The
plugin MCP can expose `codestory://status` in fresh contexts, provision and
report managed CLI state, repair sidecar onboarding when allowed, and avoid
falling back to stale ambient PATH binaries when Codex launches the installed
adapter without plugin data environment variables.

This release also removes the ONNX product embedding runtime, tightens agent
repair progress/status reporting, adds handoff proof-target status, and trims
Codex starter prompts so installed plugin manifests load without overlong
default-prompt warnings.

Supporting PRs: #671, #672, #673, #678.

## 0.11.22

CodeStory 0.11.22 fixes the plugin-bundled MCP launch directory. The plugin
now sets the MCP server `cwd` to the installed plugin root so Codex resolves
`./scripts/codestory-mcp.cjs` inside the plugin cache instead of the active
repo/session directory.

Supporting PRs: #677.

## 0.11.21

CodeStory 0.11.21 is a patch hotfix for the main-served plugin package. It
ships the MCP adapter startup wait cap from #675 through a synchronized release
version bump so an official plugin refresh installs new package metadata instead
of relying on same-version cache replacement.

The adapter now waits up to five seconds for local freshness repair during MCP
startup before failing open to diagnostic status. Operators can still override
the cap with `CODESTORY_PLUGIN_LOCAL_REPAIR_TIMEOUT_MS`. This release does not
claim fresh installed-plugin or model-visible MCP resource proof; those remain
runtime checks after publication and host/plugin refresh.

Supporting PRs: #675.

## 0.11.20

CodeStory 0.11.20 is the agent-readiness stabilization release. It makes
packet/search status harder to misread: local graph freshness, local/default
retrieval, and agent packet/search readiness are reported as separate lanes,
and repair only succeeds when the selected sidecar lane is actually full.

The release also tightens retrieval quality and runtime truth. Query-shape
fusion and broader lexical windows keep exact symbols, graph structure,
lexical matches, and dense retrieval evidence visible without flattening them
into one score. Agent sidecars now reuse stable run IDs, report observed
embedding-device state, distinguish requested AMD/Vulkan acceleration from
proof that acceleration was observed, and fail closed when stored sidecar
content no longer matches the active embedding backend.

Operationally, 0.11.20 adds sidecar inventory and garbage-collection surfaces,
dirty-hook freshness handling, bounded sidecar stage overruns, MCP visibility
reporting, safer GitHub status comment posting, and post-publish release smoke
coverage for packaged runtime assets. Release assets remain packaging evidence;
packet/search readiness still depends on the sidecar evidence tiers in
`docs/contributors/testing-matrix.md`.

Supporting PRs: #544, #593, #650, #653, #655, #656.

## 0.11.19

CodeStory 0.11.19 is the release-proof repair slice after the agent-readiness
work in 0.11.18. It keeps packaged agent proof tied to the active runtime and
records repo-scale full-sidecar packet/search evidence in the stats log instead
of relying on stale local assumptions.

The release also fixes the packaged proof setup path: release automation now
fetches and verifies the pinned embedding model before running packaged-agent
proof, so CI failure points at runtime readiness instead of a missing model
artifact.

Supporting PRs: #542, #546.

## 0.11.18

CodeStory 0.11.18 turns agent sidecar readiness into an explicit product
contract. Agent packet/search uses isolated sidecar runs, infers the agent
profile from the sidecar run ID, and exposes packet proof metadata so a caller
can see whether an answer came from full retrieval evidence or from a blocked
fallback path.

The release makes failure modes more useful for operators. The plugin MCP
launcher and local-index paths fail open with bounded guidance instead of
crashing the session, while readiness checks detect dead embedding runtimes,
zero-dense sidecar state, profile handoff mistakes, and blocked repair guidance
before packet/search is trusted.

0.11.18 also expands setup and packaging coverage with CI agent-smoke profiles,
Windows ARM64 installer asset resolution, managed embedding cache overrides,
task-brief packet support, stdio frame-size limits, and updated sidecar
readiness docs. It does not claim packet/search quality without full sidecar
readiness.

Supporting PRs: #467.

## 0.11.17

CodeStory 0.11.17 promotes the current `dev/codestory-next` release slice:
release workflow artifact actions are upgraded to v5, the release matrix now
restores and saves Cargo cache entries, and stale `target/release-dist` output
is cleared before packaging.

The CodeStory plugin now carries the sidecar setup policy surfaces from the
0.11.17 development slice: `ask`, `enabled`, and `disabled` policy modes,
`sidecar_setup` status and preflight exposure, background
`ready --goal agent --repair` scheduling when setup is enabled, disabled-policy
suppression, and enable/disable commands that persist through `--policy-file`
outside `PLUGIN_DATA`.

Operator note: Refs #460. Issue #460 required no source change. Public source
and release metadata were already correct; local operator recovery came from
refreshing the installed plugin cache with `codex.cmd plugin add
codestory@TheGreenCedar --json`. Fresh host/plugin `codestory://status` proof
remains outside this release PR.

## 0.11.16

CodeStory 0.11.16 promotes the current `dev/codestory-next` release slice:
agent preflight JSON output, Rust release-aware Cargo cache keys,
plugin-managed versioned CLI provisioning with `codestory://status` runtime
metadata, and the golden agent path docs for explicit hook behavior.

This release is a promotion and metadata sync only. It does not add new backlog
features or claim new packet/search readiness beyond the existing sidecar
evidence tiers.

## 0.11.15

CodeStory 0.11.15 fixes installed hook execution in Codex plugin caches that
inherit an ES module package scope from the Codex home. Hook runtime scripts now
ship as CommonJS `.cjs` entrypoints so `SessionStart` and `UserPromptSubmit`
can load before emitting ambient grounding context.

This release does not change the hook grounding behavior introduced in 0.11.14;
it makes that behavior executable from the installed plugin surface.

## 0.11.14

CodeStory 0.11.14 makes plugin lifecycle hooks ambient instead of merely
instructional. Hook-enabled hosts now keep CodeStory-first grounding in hidden
context, attempt a strict startup ground snapshot, and attempt request-aware
packet grounding from the actual user prompt.

Hooks still fail open: missing runtime pieces, degraded retrieval, non-repo
folders, empty output, or hook-budget timeouts leave next-step guidance instead
of blocking the agent session.

## 0.11.13

CodeStory 0.11.13 reports the host restart/reload boundary after a stale
active stdio process detects that repair installed a newer codestory-cli
outside the running MCP process. The readiness status now avoids repeating the
installer loop in that state and labels the next action as a host restart.

## 0.11.12

CodeStory 0.11.12 restores Codex lifecycle hook registration in the plugin
manifest so installed builds appear in the Codex hooks manager and can inject
the existing status-first grounding guidance at session start.

No runtime, indexing, packet/search, or sidecar behavior changed in this
release.

## 0.11.11

CodeStory 0.11.11 carries the post-adapter guardrail cleanup and release
metadata sync. Ambient agent instructions now avoid wasting context in huge or
non-code folders, trust `packet`, `search`, and `context` when status reports
full sidecar readiness, and keep ordinary setup on incremental/default refresh
paths before explicit rebuilds.

The plugin package versions are aligned with the CLI release, and the Codex
plugin category is now Developer Tools.

## 0.11.10

CodeStory 0.11.10 shipped the portable agent adapter source and marketplace
publication split. The CodeStory repo owns the plugin source, manifests, hooks,
skill, and runtime docs, while the external marketplace catalog repo owns the
host marketplace entries.

The adapter hooks inject status-first grounding guidance at session start
without running indexing, packet, search, or repair work themselves. Claude Code
and GitHub Copilot host manifests now ship beside the existing Codex plugin
source.

## 0.11.9

CodeStory 0.11.9 promotes the plugin grounding consolidation so Codex sessions
can trust the active MCP status before choosing a tool.

The stdio status resource now reports `server_version`, best-effort
`server_executable`, warnings, and per-surface `allowed_surfaces`. Local graph
surfaces stay usable when local navigation is ready, while `packet`, `search`,
and `context` stay blocked unless full agent packet/search readiness is present.
The plugin README, grounding skill, sidecar docs, usage docs, and static tests
now point agents at that same status-first contract.

This release does not claim new packet/search answer-quality proof, sidecar
performance improvement, benchmark promotion, or live installed plugin proof
beyond the source and release checks in the promotion PR.

## 0.11.8

CodeStory 0.11.8 promotes the latest reviewed readiness and plugin guidance
fixes from `dev/codestory-next` onto `main` without carrying stale 0.11.6
release metadata forward.

The release documents the MCP registration failure path in the plugin guidance,
repairs readiness setup around bundled compose artifacts and explicit ready
environment handling, and keeps llama.cpp environment propagation visible when
`ready` prepares semantic sidecars. The version is aligned across every
`codestory-*` workspace crate, `Cargo.lock`, and the CodeStory plugin manifest.

Supporting PRs: #396, #398, #401. This release does not claim new packet/search
answer-quality proof, sidecar performance improvement, benchmark promotion, or
live installed plugin proof beyond the source and release checks in the
promotion PR.

## 0.11.7

CodeStory 0.11.7 closes the product-grade intelligence saga with the final
post-release polish from `dev/codestory-next` promoted to `main`.

The docs now describe the plugin path as an agent plugin backed by the local
`codestory-cli serve --stdio --refresh none` surface, while keeping
Codex-specific installation wording in the Codex plugin flow. The stdio/MCP
initialize response now reports the crate package version instead of the old
hard-coded `0.1.0`, and the protocol contract covers both version fields.

Supporting PRs: #389, #390. This release does not add a wrapper layer, move the
marketplace catalog into CodeStory, claim new sidecar performance, or broaden
packet/search readiness beyond the existing sidecar evidence gates.

## 0.11.6

CodeStory 0.11.6 promotes the reviewed `dev/codestory-next` release delta onto
`main` as a synchronized patch release. The version is aligned across every
`codestory-*` workspace crate, `Cargo.lock`, and the CodeStory plugin manifest
so future release checks catch plugin/package drift before a PR reaches review.

The plugin and release path now make stale runtime repair more explicit. The
plugin package version tracks the CLI release, while the Windows installer can
recover when an old stdio server keeps the default `codestory-cli` binary
locked: it installs the current release into a versioned directory, moves that
directory ahead of stale PATH entries for new launches, and fails loudly if
`codestory-cli --version` still resolves to the wrong binary.

The documentation cleanup keeps readers on the durable operating surfaces:
usage for operator flow, architecture pages for subsystem ownership, sidecar
runbooks for packet/search readiness, and benchmark/testing docs for promotion
evidence. Packet/search remains proof-bearing only when sidecar retrieval is
full.

Supporting PRs: #376, #377, #379. This release does not claim new answer-quality
proof, sidecar performance improvement, benchmark promotion, marketplace
publication, or live installed plugin proof beyond the source and release
checks in the promotion PR.

## 0.11.5

CodeStory 0.11.5 carries the setup and documentation repair cleanup that
landed after 0.11.4 onto `main` as a synchronized patch release. The version is
aligned across every `codestory-*` workspace crate and `Cargo.lock`, with
`crates/codestory-cli/Cargo.toml` still acting as the release version source.

The release tightens the human operator path around sidecar repair and readiness
checks. The docs now explain when local navigation is usable, when packet/search
needs full sidecar evidence, and how to recover from stale or missing retrieval
state without turning the changelog into a release ledger.

Setup now handles locked installed CLI binaries more predictably. When an
existing installed `codestory-cli` cannot be replaced directly, the installer
falls back to a locked-safe path instead of leaving the operator with a stale
binary and a quiet success signal.

Supporting PRs: #368, #369. This release does not create manual tags, add new
answer-quality claims, or change runtime behavior beyond the promoted setup,
documentation, and version metadata.

## 0.11.4

CodeStory 0.11.4 promotes the docs/plugin/setup wave from
`dev/codestory-next` to `main` as a synchronized patch release. The release
version is aligned across every `codestory-*` workspace crate and `Cargo.lock`;
`crates/codestory-cli/Cargo.toml` remains the version source.

This release makes the operator path clearer without changing the product
runtime contract. The README, docs entry points, glossary, usage guide, and
architecture docs now start from how an agent or maintainer actually uses
CodeStory: choose the local navigation lane first, keep source citations and
uncertainty visible, and treat packet/search proof as valid only when full
retrieval sidecars are ready. The README evidence was also tightened around a
small with-vs-without task and then scoped back so it does not read like a broad
benchmark claim.

The plugin package and grounding skill now match that story. They keep the
marketplace/catalog boundary outside this repository, document the direct
`codestory-cli serve --stdio` launch path, guard the read-only stdio tool
catalog with static tests, and clarify the difference between local navigation,
exact-target context, and full sidecar-backed packet/search proof.

Contributor and setup docs now bias toward the smallest useful verification
lane before expensive checks. The worktree setup script also rejects stale
`codestory-cli` binaries instead of accepting any executable that can print
`--help`, which makes failed setup noisier but more honest.

Supporting PRs: #340, #342, #344, #347, #350, #354, #355, #357. This release
does not claim new answer-quality proof, new token-savings generalization,
benchmark promotion, sidecar performance improvement, marketplace catalog
publication, or live installed plugin proof beyond the source and release
checks in the promotion PR.

## 0.11.3

CodeStory 0.11.3 promotes the post-0.11.2 plugin/runtime polish from
`dev/codestory-next` into a synchronized patch release. The release keeps the
plugin model simple: the CodeStory repository owns the plugin source,
grounding skill, runtime docs, and direct stdio launch path, while
`TheGreenCedar/AgentPluginMarketplace` remains the external marketplace catalog
owner.

The agent-facing stdio surface now includes read-only `files` and `affected`
tools. `files` exposes indexed file inventory and coverage from the existing
local cache; `affected` maps explicit changed paths or change records against
that cache. Both tools are documented and contract-tested as read-only local
navigation surfaces: they do not discover git changes, refresh the index, or
bootstrap sidecars.

The plugin and root README were rewritten around the real operating flow:
install or refresh the plugin, check readiness, use local grounding tools first,
and trust packet/search only when sidecars report full retrieval readiness. The
docs also make the marketplace split explicit and keep install/update/remove
guidance cross-platform instead of hiding platform-specific assumptions in the
happy path.

The Windows installer now resolves the latest GitHub release when no version is
passed, requires an exact matching `codestory-cli` version instead of accepting
older minimum-compatible binaries, updates the user/process `PATH` when it
installs into the managed bin directory, and fails loudly for stale explicit
CLI overrides.

Supporting PRs: #288, #298, #299, #301, #303, #305, #307, #309, #311, #315,
#316. This release does not claim new packet/search quality, sidecar readiness,
benchmark improvement, marketplace catalog publication, or live installed
plugin proof beyond the source and release checks in the promotion PR.

## 0.11.2

CodeStory 0.11.2 carries the post-0.11.1 documentation and MCP stdio work from
`dev/codestory-next` into a synchronized patch release. The release version is
now aligned across all `codestory-*` workspace crates and `Cargo.lock`.

The user-facing docs were tightened around the way people actually install,
operate, and review CodeStory. The README and usage docs now separate source
state from runtime proof, keep readiness checks visible, and avoid implying that
docs alone prove packet/search health. Plugin install guidance now points at the
latest-release flow where this repository owns the plugin package, while the
external marketplace catalog remains owned by
`TheGreenCedar/AgentPluginMarketplace`.

The plugin MCP path is intentionally direct: `.mcp.json` runs
`codestory-cli serve --stdio --refresh none` instead of carrying a duplicate
adapter runtime. The stdio catalog also exposes a read-only `ground` tool for
grounding snapshots, alongside the existing resource and packet/search safety
boundaries.

This release does not promote packet/search readiness, sidecar readiness,
benchmark results, or query quality. It also does not claim live installed
plugin runtime proof unless that surface is dogfooded separately from this
source release lane.

## 0.11.1

CodeStory 0.11.1 was published from `main` at
`9dc3a20e7de84b7955579e6ad8dd44945a47d47a`. It ships the Codex plugin
packaging work that landed after `v0.11.0`: install/readiness stays in the CLI
wrapper, the plugin package owns only Codex metadata and skill text, and
`.mcp.json` launches `codestory-cli serve --stdio` directly instead of carrying
a Node adapter.

Release evidence:

- GitHub release: https://github.com/TheGreenCedar/CodeStory/releases/tag/v0.11.1
- Full comparison: https://github.com/TheGreenCedar/CodeStory/compare/v0.11.0...v0.11.1
- Version and packaging lane: #267

The marketplace catalog is still outside this repository. Issue #264 closed the
separate `TheGreenCedar/AgentPluginMarketplace` catalog lane, while PR #262 left
CodeStory owning only the plugin package source under `plugins/codestory`. This
release does not claim packet/search readiness, sidecar promotion, or benchmark
improvement.

### Shipped Since 0.11.0

| Area | Delivered in 0.11.1 | Evidence |
| --- | --- | --- |
| Release version | All `codestory-*` workspace crates and `Cargo.lock` are synchronized at `0.11.1`. | Issue #267; tag `v0.11.1` |
| Plugin packaging | `plugins/codestory` now contains the Codex plugin manifest, MCP metadata, package README, grounding skill, and static package tests. | PR #262 |
| Direct CLI MCP launch | The plugin `.mcp.json` launches `codestory-cli serve --stdio --refresh none` directly, with no in-package Node adapter or duplicated retrieval/runtime logic. | PR #262 |
| Install and readiness wrapper | `scripts/install-codestory.ps1` added the Windows x64 happy path for finding or installing `codestory-cli`, then reporting binary, local-navigation, and packet/search readiness from `doctor`. | PR #261 |
| Cross-platform plugin readiness | Plugin README, skill guidance, and static tests now cover Windows, macOS, and Linux install/readiness paths without adding an adapter runtime or changing Rust product behavior. | PR #269 |
| Release-note hygiene | Stale generated 0.11 pre-release docs and ledger-style artifacts were removed from committed docs before this release. | PR #260 |

Binary release assets are packaging evidence only. In this release, the plugin
docs and installer defaults kept archive names release-bound to `v0.11.1`; the
marketplace catalog remains outside this repository.

## 0.11.0

CodeStory 0.11.0 was published from `main` at
`d793965b11e526449f66b1eb1166b137a0d3839f`. It carries the post-0.10.1
development branch into a synchronized release without changing the rule that
packet/search readiness needs fresh sidecar evidence.

Release evidence:

- GitHub release: https://github.com/TheGreenCedar/CodeStory/releases/tag/v0.11.0
- Full comparison: https://github.com/TheGreenCedar/CodeStory/compare/v0.10.1...v0.11.0
- Version bump PR: #256

### Shipped Since 0.10.1

| Area | Delivered in 0.11.0 | Evidence |
| --- | --- | --- |
| Release version | All `codestory-*` workspace crates and `Cargo.lock` are synchronized at `0.11.0`. | PR #256; tag `v0.11.0` |
| Rustdoc and API docs | Rustdoc baseline guidance, public API documentation passes across contracts, workspace/store/indexer, retrieval/runtime, and CLI integration surfaces, plus a rustdoc warning gate. | PR #221, #225, #230, #234, #237, #239 |
| Sidecar and packet diagnostics | Sidecar status repair hints, vector timing diagnostics, Turbovec diagnostic gates, lexical/rerank probes, and embedding identity probes. | PR #224, #227, #236, #241, #242, #247, #251, #252 |
| Workflow and reliability | Dev PR flow documentation, worktree setup bootstrap, stale rehydrate-env source hardening, manifest schema repair, workspace dependency cleanup, compact proof anchors, and dependency audit repair. | PR #195, #201, #204, #206, #207, #217, #219 |

Binary release assets are packaging evidence only. Use the sidecar and
promotion tiers in `docs/contributors/testing-matrix.md` before claiming
packet/search readiness, answer quality, or performance promotion.

## 0.10.1

CodeStory 0.10.1 was published from `main` at
`02ae23d23519e6ee63a0824ecc96fcfc0a3bb45a`.

Release evidence:

- GitHub release: https://github.com/TheGreenCedar/CodeStory/releases/tag/v0.10.1
- Full comparison: https://github.com/TheGreenCedar/CodeStory/compare/v0.10.0...v0.10.1
- Version bump PR: #192

### Shipped Since 0.10.0

| Area | Delivered in 0.10.1 | Evidence |
| --- | --- | --- |
| Release version | All `codestory-*` workspace crates and `Cargo.lock` are synchronized at `0.10.1`. | PR #192; tag `v0.10.1` |
| Structural source proof | GitHub Actions workflow routing, Docker Compose structural collectors, Cargo manifest structural anchors, and OpenAPI endpoint evidence demotion. | PR #162, #177, #180, #182 |
| Retrieval and packet correctness | Cache rehydrate freshness guard, stdio packet budget timing, retrieval shadow fixture repairs, release evidence docs repair, retrieval mode override removal, and precise semantic SCIP diagnostics. | PR #163, #164, #165, #166, #167, #169, #183 |
| Durable docs hygiene | Stale generated pre-release review docs were removed from committed documentation. | PR #185 |

## 0.10.0

CodeStory 0.10.0 turns the post-0.9.0 research wave into releaseable
contracts, proof/provenance plumbing, cache-reuse primitives, release evidence,
and smaller maintenance surfaces. It is not a packet-runtime SLA clearance
release: #78 was carried as accepted/deferred release risk and later closed as
stale before `v0.11.0`.

Reviewer comparison branch:
`https://github.com/TheGreenCedar/CodeStory/compare/v0.9.0...review/codestory-saga-from-v0.9.0-f4f6d3d6`

### Shipped Since 0.9.0

| Area | Delivered in 0.10.0 | Evidence |
| --- | --- | --- |
| Language/support claims | Tiered language claim definitions, sidecar manifest contract, anti-overfit claim profile gates, product agent workflow contract, and explicit performance/ops gates. | PR #43, #44, #45, #46, #56, #57, #58 |
| Retrieval proof/provenance | Compact packet provenance counts, SCIP proof adapter contract slice, structural workflow source-proof pilot, unresolved-candidate diagnostics, publishable blocker buckets, and packet artifact UX improvements. | PR #66, #68, #70, #71, #80, #81, #130, #131 |
| Cache reuse across worktrees | `cache rehydrate` command, SQLite graph/search/doc rebasing, portable v2 artifact-cache keys, canonical repository identity, canonical sidecar generation identity, and fail-closed sidecar revalidation semantics. | PR #84, #92, #114, #118, #123 |
| Cross-platform operator docs | Cache recovery and release-review support documented for Windows, macOS, and Linux operator flows. | PR #146 |
| Packet-runtime diagnostics | Batch setup reuse, search timing, batch overhead attribution, final-output/residual-wall timing, strict batch bounds, compact probe tapering, and artifact/reporting cleanup. | PR #86, #88, #93, #97, #101, #110, #116, #125, #127, #130 |
| Code reduction and abstraction cleanup | `enum_dispatch` resolver slice, shared language registry routing, mirrored enum conversion cleanup, retrieval manifest fixture helper, CLI DTO fixture cleanup, and retrieval stage metadata centralization. | PR #94, #102, #103, #108, #109, #113 |
| Release evidence and review surface | Promotion audit evidence, cross-platform release-review support, and a reviewer branch rooted at `v0.9.0` before version-bump noise. Generated report packages belong in PRs, issues, or external artifacts, not durable repo docs. | PR #77, #145, #146, #151 |

### Evidence and Comparison

| Gate | 0.9.0 baseline / previous state | 0.10.0 result | Evidence |
| --- | --- | --- | --- |
| Reviewer diff | Baseline tag `v0.9.0` at `2feb60990c6e`. | Review branch `review/codestory-saga-from-v0.9.0-f4f6d3d6` preserves the saga diff before the version bump. | Compare URL above; #74 |
| Workspace release version | Workspace crates were synchronized at `0.9.0`. | All eight `codestory-*` workspace crates and `Cargo.lock` are synchronized at `0.10.0`. | PR #151; `check-codestory-release.py --version 0.10.0` |
| Repo-scale e2e after sidecar repair | No release claim based only on `retrieval_mode=full`. | E2E passed after repair with 14,041 symbol docs, 760 dense docs, 0 index errors, 83.31s full index, 28.42s repeat refresh, and 8.70s retrieval index. | #72 and associated target artifacts |
| Focused packet quality | Publishable packet-runtime evidence was blocked. | Focused Apache and Redis rows had quality `3/3` and sufficiency `sufficient:3`. | #143 |
| Packet-runtime SLA | Not cleared. | Redis focused cold row cleared `0/3` SLA misses; Apache focused cold still missed `2/3`. Warm SLA remains accepted residual risk. | #78; #143 |
| Cache reuse | Cache identity was path/root-bound and expensive for parallel agent worktrees. | SQLite graph/search/doc rows and portable v2 artifact-cache rows can be reused across compatible clean worktrees; retrieval sidecars revalidate/rebuild fail-closed instead of being blindly trusted. | #82; PR #84, #114, #118, #123 |
| Release notes / review package | No final package for the saga diff. | Report package was produced for review outside committed docs. | #143 |

### Packet-Runtime Release Risk

| Evidence row | Quality | Sufficiency | SLA result | Retrieval median | Decision |
| --- | ---: | --- | ---: | ---: | --- |
| Apache Commons Lang cold focused row | 3/3 | `sufficient:3` | 2/3 misses | 15,528 ms | Accepted/deferred risk; #78 was later closed as stale. |
| Redis cold focused row | 3/3 | `sufficient:3` | 0/3 misses | 7,722 ms | Clear for the focused cold row. |

The full publishable packet-runtime gate is not claimed as cleared. Earlier draft
and diagnostic PRs remain evidence surfaces, not shipped SLA fixes, unless their
specific code changes landed in the PR list above.

### Still Not Shipped

- Packet-runtime SLA clearance and publishable promotion evidence.
- Full precise semantic import implementation beyond the contract/proof slices.
- Broad structural collector rollout beyond the workflow-source pilot.
- True offline retrieval sidecar preservation during `cache rehydrate`; current
  behavior is fail-closed revalidation/rebuild under canonical sidecar identity.
- Any manually created release tag. Tags and binary assets remain owned by the
  repository release workflow.

## 0.7.0

- Current synchronized workspace release baseline.
- Future synchronized CodeStory workspace version bumps on `main` create GitHub
  releases with cross-platform `codestory-cli` binary assets and `SHA256SUMS.txt`.

## Release Notes

- Add concise human-facing notes under the bumped version before merging a
  release version change to `main`.
- Keep release notes focused on user-visible CLI, grounding, retrieval,
  packaging, and documentation changes.
