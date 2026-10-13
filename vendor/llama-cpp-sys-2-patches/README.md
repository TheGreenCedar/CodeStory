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
only regular files/directories, applies the source patch in a temporary sibling,
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
not add a runtime feature. Repository CI does not currently invoke this
handoff bootstrap, so Cargo jobs may fail until the source packaging strategy
is integrated. That integration and all product qualification remain WIP.
