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
| Stable fingerprint | `colima-vz0.10.3/mac17.4/apple-m5/macos26.5.2/linux-arm64/4vcpu/17GiB/no-host-mount-v1` |
| Runner volume | `/srv/codestory-release-evidence` |
| Model directory | `/srv/codestory-release-evidence/models` |
| Drill manifest | `/srv/codestory-release-evidence/drills/real-repo-drill-cases.json` |

The workflow checks the guest-visible values rather than the Colima settings.
The current profile exposes more than 16 GiB to Python and more than 20 GiB of
free space to the runner workspace.

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
- verifies checksums before installing Node, Rust, GitHub CLI, and the Actions
  runner;
- disables automatic runner updates so baseline changes are deliberate;
- downloads and verifies the pinned BGE model;
- prepares a source-backed `serde_json` drill at an exact commit;
- pulls the digest-pinned ARM64 Qdrant and llama-server images;
- registers the runner only with `TheGreenCedar/CodeStory`; and
- keeps Cargo, Rust, temp, XDG, CodeStory, model, drill, work, and artifact state
  under the proof-owned volume.

The tracked CodeStory source used by provisioning checks is streamed into the
guest over SSH. No source checkout or tool is executed through a host mount.
`verify` prints the guest mount table and fails if it finds a host-backed
VirtioFS, 9p, SSHFS, Lima, osxfs, or gRPC FUSE mount; any `/Users` visibility is
also a hard failure.

The GitHub registration and removal tokens are short-lived and passed directly
to the guest. They are never written to the repository or provisioning
artifacts. Long-lived runner credentials remain inside the dedicated VM.

## Operate and recover

```sh
scripts/codestory-release-evidence-runner.sh stop
scripts/codestory-release-evidence-runner.sh start
scripts/codestory-release-evidence-runner.sh verify
```

After a host restart, start the profile and require `verify` to show the runner
online, both pinned images present as Linux ARM64, the exact model checksum, the
clean drill commit, the stable fingerprint, and sufficient guest capacity.

When intentionally changing the toolchain, runner, model, sidecar image, VM
shape, or drill commit, update the pinned constants in the provisioning script
and create a new approved baseline. Do not compare results across the identity
change as though they came from the same machine profile.

To unregister while preserving the VM and proof artifacts:

```sh
scripts/codestory-release-evidence-runner.sh unregister
```

To unregister and delete all proof-owned VM state:

```sh
scripts/codestory-release-evidence-runner.sh destroy
```

## Release workflow handoff

Dispatch `.github/workflows/release-candidate-evidence.yml` only from a trusted,
exact release head. The first measurement uses:

| Input | Value |
| --- | --- |
| `profile` | the release-eligible profile created from the approved baseline |
| `drill_manifest` | `/srv/codestory-release-evidence/drills/real-repo-drill-cases.json` |
| `embedding_model_dir` | `/srv/codestory-release-evidence/models` |
| `source_run_id` | empty for measurement; a rejected run ID only for exact-artifact re-evaluation |

The workflow uploads `release-evidence-<full SHA>` from
`target/release-evidence`, including provisioning, raw stats, packet summary,
candidate, approval when supplied, and report files that exist. Runner
provisioning alone does not establish a baseline, execute the real-repo drill,
or prove a candidate acceptable.
