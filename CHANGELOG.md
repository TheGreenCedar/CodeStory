# Changelog

## 0.10.0

- Ship the synchronized CodeStory workspace release after the #74 pre-release
  review artifacts landed in PR #145 and PR #146.
- Preserve the `v0.9.0` reviewer comparison surface:
  `https://github.com/TheGreenCedar/CodeStory/compare/v0.9.0...review/codestory-saga-from-v0.9.0-f4f6d3d6`.
- Carry #78 packet-runtime SLA misses as accepted/deferred release risk; this
  release does not claim packet-runtime SLA clearance.

## 0.7.0

- Current synchronized workspace release baseline.
- Future synchronized CodeStory workspace version bumps on `main` create GitHub
  releases with cross-platform `codestory-cli` binary assets and `SHA256SUMS.txt`.

## Release Notes

- Add concise human-facing notes under the bumped version before merging a
  release version change to `main`.
- Keep release notes focused on user-visible CLI, grounding, retrieval,
  packaging, and documentation changes.
