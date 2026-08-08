# Release-evidence quality contract

The v0.17 release-evidence packet-quality contract runs as a non-gating adjunct
on the protected macOS Metal and Windows Vulkan quality lanes. The macOS lane
retains the frozen Axios v2 measurement and adds Ripgrep v2 as a separately
isolated row. The Windows x64 lane measures Ripgrep v2 from the exact candidate
archive with its own cache and stdio roots. CPU fallback is disabled in both
Ripgrep measurements.

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
claim floors. The protected macOS Metal and Windows Vulkan quality lanes supply
current-host measurements without becoming qualification or release gates. The
contract makes a scoped Ripgrep packet-quality claim only; it does not establish
a general Rust, parser-completeness, or answer-accuracy claim.

The v0.16 Axios contracts and their approved baselines remain frozen. The
holdout `ripgrep-search-pipeline.task.json` is separate and must remain
byte-identical; release-only work uses the v2 manifest.

## Quality-lane ownership

`.github/workflows/frozen-candidate-quality.yml` owns optional performance and
answer-quality evaluation. macOS Metal measures Axios v2 and Ripgrep v2;
Windows Vulkan measures Ripgrep v2; Linux Vulkan retains the non-gating Axios v2
smoke. The former Linux ARM64 guest runner and its provisioning harness are
retired and are not measurement authorities.

## Evidence boundary

Raw packet output must identify the selected corpus contract, exact task and
project-manifest hashes, cold CLI runtime mode, and exactly three repeats. The
release-evidence gate checks that binding before evaluating rows. A green local
contract test or a retained measurement is not a fresh-host qualification;
packaged, installed, and live behavior proof retain their own evidence tiers.
