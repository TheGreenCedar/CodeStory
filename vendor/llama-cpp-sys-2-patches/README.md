# Pinned research native source patch

The research Cargo manifest overrides `llama-cpp-sys-2` 0.1.151 with a local
source tree. That tree was untracked in the originating checkout. Its authored
changes are preserved here as a patch against the checksum-pinned upstream
crate; the large upstream source copy is excluded from Git. Model assets, native
build outputs and executable packages are not included.

Before **any Cargo command**, reconstruct the excluded tree from an existing
cached archive:

```sh
python3 scripts/bootstrap-research-native-vendor.py
python3 scripts/bootstrap-research-native-vendor.py --verify
```

Alternatively pass `--crate /path/to/llama-cpp-sys-2-0.1.151.crate`. On a new
machine, after normal download/resource coordination, explicitly use
`--download`. The script checks the upstream archive and patch SHA-256, extracts
only regular files/directories, applies the source patch with Git (checkout
conversion disabled) in a temporary sibling,
and verifies the complete regular-file postimage content digest before copying
into an exclusively created destination. File creation is exclusive too, so
concurrent user files are never overwritten. Run one bootstrap writer per
checkout and do not build while it copies. A failed publication leaves its
partial destination for owner review; it does not silently delete or overwrite it.
It never overwrites an existing vendor tree or performs build/model/install
work. The `.cargo-ok` cache marker is intentionally excluded.

See `manifest.json` for the exact archive checksum, changed file hashes,
unchanged source count, source size and regular-file content digest. Verification
rejects symlinks and special entries; timestamps/modes/empty directories are not
part of the content digest. The upstream
license/attribution files are retained by source reconstruction. The patch
retains the existing native completion/device-placement changes; bootstrap does
not add a runtime feature. CI source-build jobs invoke the shared
`.github/actions/native-source` action before workspace Cargo commands. The
action passes `--ensure --download`: it verifies an existing tree or reconstructs
an absent tree from the checksum-pinned archive. Warm `--ensure` verifies the
pinned patch too; `--verify` checks the source tree only. A modified or partial existing
tree fails verification and is preserved. The native patch and manifest enter
the existing build-cache identities. Linux package and offline-proof containers
receive the reconstructed workspace tree; bootstrap performs no acquisition
inside the network-denied Cargo boundary. Runtime/model acquisition and product
qualification remain separate.

Run the small reconstruction/failure matrix with
`python3 scripts/tests/bootstrap-research-native-vendor.test.py`. It uses tiny
isolated archives and does not invoke Cargo or the network. The CI consumer
ordering check is `node --test scripts/tests/native-source-ci.test.mjs`.

The patch is immutable data. Unified-diff blank context lines contain a required
space; the first container whitespace check flagged those lines. The scoped
`.gitattributes` rule preserves them without weakening source-file checks or
changing the patch hash. LF checkout is required so Windows applies the same bytes.
