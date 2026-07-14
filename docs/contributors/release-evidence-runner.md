# Release-evidence runner

CodeStory release-candidate measurements run on one repository-scoped Linux
runner so baseline identity, cache state, model bytes, and sidecar images remain
stable between candidates. Ordinary pull requests must not target this runner.

## Machine contract

The approved host shape for the first v0.15 baseline is:

| Field | Value |
| --- | --- |
| GitHub runner | `codestory-release-evidence-m5-colima-arm64` |
| Required labels | `self-hosted`, `Linux`, `ARM64`, `codestory-release-evidence` |
| Physical host | `Mac17,4`, Apple M5, 24 GiB, macOS 26.5.2 |
| VM | Colima VZ profile `codestory-release-evidence`, Ubuntu 24.04 ARM64 |
| Colima | 0.10.3 |
| Capacity | 4 vCPU, 17 GiB configured memory, 80 GiB data disk |
| Host mounts | none; the runner cannot see or write `/Users` or the macOS home directory |
| Host Docker context | never activated by the evidence profile |
| Stable profile ID | `codestory-release-evidence-linux-arm64-v1` |
| Machine contract | `scripts/release-evidence/machine-contract.json` |
| Runner volume | `/srv/codestory-release-evidence` |
| Model directory | `/srv/codestory-release-evidence/models` |
| Drill manifest | `/srv/codestory-release-evidence/drills/real-repo-drill-cases.json` |

The profile ID is a stable comparison key, not evidence about the current
machine. Provisioning records the observed host, generated Colima VM shape,
guest boot, native package manifest, and toolchain in one attestation. The
workflow reruns the guest verifier and derives its fingerprint from the profile
ID plus the checked-in contract hash. A stale host attestation from another VM
boot is rejected.

## Provision and verify

The host needs macOS, Colima, and an authenticated `gh` with repository
administration access. A 24 GiB Mac cannot safely run this 17 GiB profile beside
the normal 8 GiB Colima profile; stop the normal profile first after confirming
it has no active work.

From a clean trusted CodeStory checkout:

```sh
scripts/codestory-release-evidence-runner.sh provision
scripts/codestory-release-evidence-runner.sh verify
```

Provisioning is idempotent. It:

- creates the dedicated VM and service account;
- disables Colima's default writable host-home mount;
- leaves the caller's active Docker context unchanged;
- verifies the checksum-pinned Colima base image before VM creation;
- installs native packages from a fixed Ubuntu archive snapshot at exact
  versions, then records the complete native package manifest;
- verifies checksums before installing Node, Rust, GitHub CLI, and the Actions runner;
- disables automatic runner updates so baseline changes are deliberate;
- streams a checksum-verified BGE model from the standard CodeStory cache when
  present, otherwise downloads it, then verifies the contract checksum again
  inside the guest;
- prepares a source-backed `serde_json` drill at an exact commit;
- pulls the digest-pinned ARM64 Qdrant and llama-server images;
- registers the runner only with `TheGreenCedar/CodeStory`; and
- keeps Cargo, Rust, temp, XDG, CodeStory, model, drill, work, and artifact state
  under the proof-owned volume.

The tracked CodeStory source used by provisioning checks is streamed into the
guest over SSH. It replaces the previous validation tree atomically enough for
the stopped runner, so untracked or modified validation files cannot survive a
provisioning pass. No source or tool is executed through a host mount. `verify`
prints the guest mount table and fails if it finds a host-backed VirtioFS, 9p,
SSHFS, Lima, osxfs, or gRPC FUSE mount; any `/Users` visibility is also a hard
failure. Provision, verify, start, and the workflow preflight also require the
dedicated VM to have no running containers. CodeStory sidecars may start only
after that preflight as part of the active evidence job.

The model seed also crosses SSH rather than a host mount. Its default source is
`~/Library/Caches/dev.codestory.codestory/retrieval/models/`; set
`CODESTORY_RELEASE_EVIDENCE_MODEL_SEED` to an alternate exact file. The host
and guest both reject bytes that do not match the machine contract.

Provisioning first proves that an existing owned runner is idle. It requests a
GitHub registration token only when the runner is unconfigured, checks the
exact runner binary version and `.runner` repository/name identity, and leaves
the systemd service disabled across VM boots. The host `start` command verifies
and attests the current host and VM before starting the service. Starting the
Colima profile directly therefore leaves the runner offline. Provision, verify,
start, and stop all quiesce the exact runner first and confirm that GitHub sees
it offline and idle before changing validation or proof state.

The GitHub registration and removal tokens are short-lived and passed directly
to the guest. They are never written to the repository or provisioning
artifacts. Long-lived runner credentials remain inside the dedicated VM.

## Operate and recover

```sh
scripts/codestory-release-evidence-runner.sh stop
scripts/codestory-release-evidence-runner.sh start
scripts/codestory-release-evidence-runner.sh verify
```

After a host restart, use the script's `start` command rather than `colima
start`. Require `verify` to show the runner online, both pinned images present
as Linux ARM64, the exact model checksum, the clean drill commit, a current-boot
host attestation, the contract-derived fingerprint, and sufficient guest
capacity.

When intentionally changing the toolchain, runner, model, sidecar image, VM
shape, or drill commit, update the pinned constants in the provisioning script
and create a new approved baseline. Do not compare results across the identity
change as though they came from the same machine profile.

`stop`, `unregister`, and `destroy` deliberately do not require the pinned host
OS, Colima version, or a clean checkout. They validate the durable ownership
marker and exact local and remote runner identity first. `stop` requires GitHub
access so it cannot stop a runner whose busy state is unknown. If GitHub access
is unavailable during unregister, the script leaves the runner, credentials,
ownership marker, and proof-owned VM unchanged. `destroy` removes the VM only
after GitHub confirms that the runner was deleted or already absent. API
failures never count as absence, and a busy runner is never reprovisioned,
stopped, unregistered, or destroyed.

To unregister while preserving the VM and proof artifacts:

```sh
scripts/codestory-release-evidence-runner.sh unregister
```

To unregister and delete all proof-owned VM state:

```sh
scripts/codestory-release-evidence-runner.sh destroy
```

## Release workflow handoff

`release.yml` calls `release-candidate-evidence.yml` on the exact release head
after preflight and before packaged proof. The automatic path measures fresh
evidence with:

| Input | Value |
| --- | --- |
| `profile` | `codestory-release-evidence-linux-arm64-v1` |
| `drill_manifest` | `/srv/codestory-release-evidence/drills/real-repo-drill-cases.json` |
| `embedding_model_dir` | `/srv/codestory-release-evidence/models` |
| `source_run_id` | empty for measurement; a rejected run ID only for exact-artifact re-evaluation |

If a measured candidate is rejected and receives an exact, expiring approval,
manually dispatch `release.yml` on the same SHA and version with
`source_run_id=<rejected-run-id>`. The reusable workflow downloads that run's
artifact and evaluates it again without producing new measurements. Before the
download, it requires a failed trusted evidence workflow from this repository
whose head is the exact evidence SHA.

The approval secret belongs only to the protected `release-evidence`
environment used by the reusable workflow's measurement job. Release callers
must not pass or inherit it. A re-evaluation with no nonempty protected approval
fails before the evaluator runs.

The workflow uploads `release-evidence-<full SHA>` from
`target/release-evidence`, including provisioning, raw stats, packet summary,
candidate, approval when supplied, and report files that exist. Runner
provisioning alone does not establish a baseline, execute the real-repo drill,
or prove a candidate acceptable. The release remains blocked until the selected
profile exists as an approved, release-eligible baseline.
