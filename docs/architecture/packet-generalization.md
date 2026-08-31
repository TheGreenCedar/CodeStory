# Packet generalization

Packet planning must stay repository-evidence driven. Prompt text may supply
generic seeds (paths, qualified symbols, identifiers, original wording, and
explicit probes). It must not select domain taxonomies, fixed flow stage lists,
carrier predicates, post-rank cleanup passes, or flow-template claims.

## Required sequence

```text
generic seed plan → uncapped seed retrieval → bounded pinned graph
  → repository evidence plan → hydrate → identity/range dedup → 16-row/16 KiB projection
```

One pinned publication. One bounded retry when that publication changes. The
public packet budget remains 16 evidence rows / 16 KiB.

## Forbidden production shapes

- Prompt → domain-flow classifiers (including ASCII byte-array encodings of
  brand or holdout terms such as `[115, 119, 114]` → `swr`)
- Fixed stage lists and flow-requirement dispatchers keyed by those classifiers
- Domain evidence-carrier predicates and post-rank sibling cleanup passes
- Flow-template claims and holdout `expected_files` / `expected_symbols` anchors
- Production dependencies on `benchmarks/`, `codestory-bench`, or eval manifests

Historical 18-task / language-expansion holdout scores are
`evidence_eligibility: contaminated_development` only. They are never a release
or generalization gate.

## Boundary checker

CI job `retrieval-generalization` runs:

- `node scripts/lint-retrieval-generalization.mjs`
- `node scripts/check-packet-generalization-boundary.mjs`
- their hostile Node test suites

The packet boundary checker fails closed on contaminated heads. Renaming a
classifier while keeping encoded brands, holdout anchors, or deleted cleanup
APIs must still fail. Vocabulary for those banned shapes is permitted only in
tests, tooling, and the checker fixtures.

## Claims

Packet claims may state only observed structural facts with node or edge
identity, plus dynamic gaps / continuation / unknown. They must not infer
absence or runtime-execution truth from a missing row.
