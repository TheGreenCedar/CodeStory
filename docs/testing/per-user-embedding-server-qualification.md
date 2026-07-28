# Per-user embedding server qualification

This is the release contract for CodeStory's automatically spawned per-user
embedding server. Source tests, package structure, protected accelerator
execution, installed plugin behavior, and release readiness are separate proof
tiers. A lower tier never inherits a higher claim.

## Bound contracts

The release binary and `codestory-native-manifest.json` bind three checked-in
documents under `crates/codestory-llama-sys/`:

- `per-user-embedding-server-protocol.json` fixes framing, operations, replay,
  privacy, and size limits;
- `per-user-embedding-server-constant-set.json` owns production timing values
  and pass thresholds; and
- `per-user-embedding-server-measurement-protocol.json` fixes clocks, event
  boundaries, scenarios, metrics, and explicit nonclaims.

The manifest also binds the exact source commit and tree, archive and executable
digests, package target, model/backend contract, and server proof marker. Any
source, binary, protocol, constant, measurement, package, or host identity
change invalidates earlier evidence.

## Identity boundary

The transport security boundary is OS-private same-user IPC. The listener
authenticates the account plus the client's native PID and process-start
identity. `Hello` must repeat that PID and process-start identity exactly; its
executable digest and version are same-user compatibility claims, not
adversarial attestation or package-provenance proof.

Before binding the listener, the server hashes its exact executable and reports
that digest and version in every snapshot. Clients bind the snapshot PID and
process-start identity to the authenticated transport and validate the reported
digest and version against the executable they captured before spawn or
connect. Installed-runtime qualification separately hashes the actual running
executable and binds it to the package manifest. A `Hello` claim cannot replace
that external proof.

`EmbedQuery` and `EmbedDocuments` may carry an opaque cancellation token.
`Cancel` must name the target request and repeat that token. The server accepts
the cancellation only when the token, authenticated client PID, and
process-start identity all match the original request; ambiguous matches fail
closed. Cancellation tokens never appear in snapshots or diagnostics.

## Calibration and qualification

Calibration measures a draft candidate and may inform a later constant-set
change. It cannot pass a package, hardware, installed-runtime, or release gate.
Qualification starts only after the constant set says `frozen`, every selected
value and threshold is non-null, and the freeze record names the source/tree,
host profile, sample artifact hash, selection rule, and selected values.

The qualification run cannot change its own thresholds. A failure returns to a
new source revision, which requires a new binary and new evidence.

All product durations use awake-time monotonic clocks. Each process measures
from its own origin. Correlated request IDs and the server event sequence order
cross-process events; subtracting timestamps from different process origins is
forbidden. Wall clock records provenance only. An unplanned sleep, hibernation,
VM pause, or power transition invalidates a performance block.
Calibration uses complete successful native-backed operation duration as a
conservative watchdog bound; it does not claim per-native-event progress
timing, while the watchdog still observes admission and native-batch progress
sequence.

## Required scenarios

| Scenario | Required result |
| --- | --- |
| Cold race | Two independent plugin hosts under one OS account and different repositories converge on one lifetime authority, listener, server, engine owner, native worker, load generation, and model load. |
| Mixed queue | Both 64-entry queues remain bounded; each class is FIFO; queries are preferred between bulk batches; bulk resumes when the query queue permits; retry conditions are useful; no project/scope round-robin or private text/path leakage appears. |
| Client death | Queued work and leases owned by the dead connection are reclaimed while another client continues against the same server. |
| Server crash | One replacement is elected. Only a pure embedding RPC replays, at most once; lost leases block candidate publication. |
| Worker stall | The independent watchdog fail-stops the server; unrelated processes survive; replay and publication rules match the server-crash case. |
| True idle and respawn | Queued, active, and leased work prevent exit. Idle connections and diagnostics do not. The server exits after 60 seconds of awake true idle and the next product operation spawns one replacement without consent or repair. |
| Incompatible owner | A fully idle owner drains before replacement. Active work or leases return typed retry state. Engine count never exceeds one. |
| Frozen owner | A whole-server freeze returns bounded `owner_unresponsive`. Clients do not unlink, kill a PID, take over authority, or start a second engine. |

The mixed-queue result does not establish bounded bulk starvation under
arbitrary sustained query traffic. The base same-account result does not
establish sharing across separate login, terminal, fast-user-switch, desktop,
or service sessions.

## Retained evidence

The producer runs behind a private random
`CODESTORY_EMBED_QUALIFICATION_DIR` plus
`CODESTORY_EMBED_QUALIFICATION_NONCE` gate. The gate changes the private
endpoint namespace and enables deterministic fault controls; it is absent from
public help and ordinary product APIs. Raw nonce values and project paths are
not retained.

Each passing record contains:

- exact source/tree, archive, executable, package target, protocol, constants,
  measurement protocol, model, backend, policy, cache, and residency identity;
- host fingerprint, OS-account relation, plugin package and managed runtime
  provenance, and two independent host process-start identities;
- shared endpoint, lifetime authority, listener, server, engine owner, native
  worker, load generation, and model-load identity;
- every preregistered scenario assertion plus hashes of its raw artifacts;
- every required metric, its unit, frozen threshold, comparison, and result.
  Retrieval quality is the pass rate derived from the exact-head
  `publishable-three-repeat-packet/v1` raw packet artifact; the verifier binds
  its source commit and tree, requires every declared repeat and row, and
  recomputes the 1.0 pass rate instead of trusting a declared quality result;
- explicit lower-tier nonclaims; and
- the highest tier actually exercised.

The verifier rejects a missing scenario or metric, an unknown assertion,
unfrozen constants, stale identity, moved threshold, direct-runtime override at
installed tier, or a record below the requested tier.

## Running the harness

Synthetic self-tests validate the verifier only:

```sh
python .github/scripts/check-packaged-agent-proof.py --self-test
```

A draft exact-package run is explicitly calibration:

```sh
cargo build --release --locked -p codestory-bench \
  --bin codestory_embedding_qualification
python .github/scripts/check-packaged-agent-proof.py \
  --archive <archive> \
  --checksum-file <checksums> \
  --expected-version <version> \
  --project <repo-a> \
  --plugin-root plugins/codestory \
  --plugin-handoff \
  --engine-policy cpu_explicit \
  --expected-backend CPU \
  --proof-tier calibration \
  --qualification-matrix-cell hosted_linux_x64_cpu \
  --produce-qualification-evidence \
  --qualification-driver target/release/codestory_embedding_qualification \
  --qualification-evidence <calibration.json>
```

The proof harness invokes this separate driver with `--cli` pointing at the
exact unpacked packaged executable. Only the nonce-gated worker remains in the
shipped CLI; scenario orchestration and evidence writing stay in the proof
tool.

Accuracy or performance qualification may replace `calibration` with its exact
requested tier and pass `--retrieval-quality-evidence` the exact-head
`packet-runtime-summary.json` emitted by the release-evidence workflow. v0.16
release closeout does not consume that artifact. Hosted and protected package
tiers may bind the unpacked archive through the source plugin launcher.
`installed_runtime` instead requires the managed installed plugin and managed
executable it claims; it rejects
`CODESTORY_CLI`, a repository-source plugin root, and a direct unpacked binary
override.

v0.16 release proof has candidate-installed lanes for Apple Silicon, Windows
x64, and Linux x64. Each copies the exact source plugin into an isolated private
installation root outside the source checkout, stages the exact packaged
archive as the managed runtime, and binds both to the trusted coordinator run.
These lanes cannot stand in for the three post-publish marketplace proofs.

The standard release path is narrower than qualification. On each protected
host, `--server-behavior-only` initializes one plugin host, grounds one real
project, waits for search readiness in that same project, and verifies the
installed engine, server, package, and expected Metal or Vulkan backend
identity. It rejects calibration and retrieval-quality inputs and makes no
two-host sharing, answer-quality, performance, or broader lifecycle claim.

The explicit `linux` coordinator scope runs that protected Linux x64 Vulkan
package and candidate-installed proof without scheduling Mac or Windows
protected jobs. It has the same single-project claim boundary as the standard
release path and is valid for either an accepted platform-proof PR or an exact
live `dev/codestory-next` integration dispatch.

Frozen calibration bundles are accepted only from a successful
`workflow_dispatch` run of `packaged-platform-pr.yml` in this repository. Every
consumer binds the run ID, exact `embedding-calibration-bundle-<source-sha>`
artifact name, unexpired artifact record, source commit, and bundle producer
identity before applying the frozen thresholds.

The frozen `hosted_package` qualification additionally passes
`--enforce-calibration-freeze-lineage`, so the exact calibration-to-package
source lineage is proved rather than assumed: the calibration commit must be an
ancestor of the packaged commit, the verification checkout must be that packaged
commit, and
`crates/codestory-llama-sys/per-user-embedding-server-constant-set.json` must be
the only path that differs between them. The packaged proof therefore checks out
full history.

That rule fixes the release ordering to **bump-then-calibrate**: bump the
version first with `node scripts/bump-version.mjs --version <version>`,
calibrate on the bumped tree, then land the constant-set freeze commit as the
only commit between calibration and the packaged release. Calibrating first and
bumping afterwards puts a second commit in that range, and the guard rejects it
by name -- the failure lists the offending paths and repeats this ordering. The
fix is always to move the bump ahead of calibration and recalibrate on the
bumped tree, never to widen the allowed path set.

Platform proof boundaries:

- Apple Silicon requires the exact package, CPU disallowed, physical Metal,
  backend-observed execution, and full layer offload.
- Windows x64 requires the exact package, CPU disallowed, physical Vulkan,
  software-adapter rejection, and backend-observed execution.
- Linux x64 requires the exact package, CPU disallowed, physical Vulkan,
  software-adapter rejection, and backend-observed execution.

Answer quality, performance, cross-user sharing, stronger cross-session
sharing, whole-server automatic takeover, and bounded bulk starvation remain
nonclaims unless their own named evidence tier proves them.
