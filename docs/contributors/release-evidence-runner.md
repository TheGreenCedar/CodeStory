# Release-evidence quality contract

The v0.17 release-evidence packet-quality contract targets the protected macOS
Metal and Windows quality lanes. The current workflow still selects the v0.16
Axios contract; EV-4c owns moving that producer to this contract. The Linux
ARM64 Colima guest host was retired from live lanes in H-1 (PR #1849), so it is
no longer a measurement authority. Its in-tree harness remains until EV-4c.

## v0.17 Ripgrep corpus boundary

`codestory-release-corpus-v0.17-ripgrep-rust-v2` defines one release-only
Ripgrep Rust task, `ripgrep-search-pipeline-v2`, with three deterministic cold
CLI packet repeats. Its task and CodeStory project manifests are bound by
SHA-256 in
`benchmarks/release-evidence/corpus-contracts/v0.17-ripgrep-rust-v2.json`.
The gate rejects a missing, changed, substituted, escaped, extra, or
task-inconsistent declaration.

The retained run `29624645979` measured every Ripgrep repeat at file recall
and citation coverage of `0.60`, symbol recall `0.80`, claim recall `1.0`,
anchor recall `0.80`, and zero forbidden claims. The v0.17 contract therefore
pins the D2-corrected `0.60` file-recall and citation-coverage floors; it keeps
the existing `0.65` symbol, `0.75` claim, `0.70` anchor, and zero-forbidden
claim floors. The protected macOS Metal and Windows quality lanes will supply
the current-host proof when EV-4c wires this contract. It makes a scoped
Ripgrep packet-quality claim only; it does not establish a general Rust,
parser-completeness, or answer-accuracy claim.

The v0.16 Axios contracts and their approved baselines remain frozen. The
holdout `ripgrep-search-pipeline.task.json` is separate and must remain
byte-identical; release-only work uses the v2 manifest.

## Retired Linux ARM64 host and retained harness

The `codestory-release-evidence-linux-arm64-v2` machine contract, guest
provisioning scripts, and Colima runner lifecycle remain in tree so the workflow
policy checker and its mutation tests continue to exercise their contract. The
host itself was retired from live lanes in H-1 (PR #1849); do not use it as
measurement authority or recreate it for new evidence. EV-4c retires the
harness and policy layer together, alongside the claims-graph and producer
re-pointing. Until then, the retained profile is historical contract coverage,
not a runnable release-evidence lane.

## Evidence boundary

Raw packet output must identify the selected corpus contract, exact task and
project-manifest hashes, cold CLI runtime mode, and exactly three repeats. The
release-evidence gate checks that binding before evaluating rows. A green local
contract test or a retained measurement is not a fresh-host qualification;
packaged, installed, and live behavior proof retain their own evidence tiers.
