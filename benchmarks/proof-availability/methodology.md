# Proof availability qualification methodology v1

This qualification decides only whether the indexed source call-path verifier
is useful enough for an automatic, stable explicit, experimental manual, or
dark product role. It does not prove runtime execution, unrestricted English
translation, installed-host latency, or production negative completeness.

## Frozen population and identities

The corpus contains 120 positive requests, 30 in each of four repository
cohorts, 312 audited positive call steps, and 240 negative mutations. Artifact
identity is one-way: this methodology's raw-byte SHA-256 is stored in both the
threshold document and corpus; the RFC 8785 canonical threshold SHA-256 is
stored in the corpus; the RFC 8785 canonical hash of each 30-path cohort file
is also stored in the corpus; and the canonical corpus and threshold identities
are stored in the result. The freeze order is methodology bytes, thresholds,
four cohort path files, then corpus. The evaluator validates the available
bindings before calculating a decision.

## Source-only curation and materialization

The four repository declarations are closed: CodeStory, Vite, Flask, and Gin
use the exact HTTPS URLs, 40-hex commits, and workspaces recorded by this
methodology's contract registry. Each cohort file owns exactly 30 paths with
the length distribution 10/7/5/3/3/2 and 78 positive steps. Every path retains
its own curator, independent reviewer, review date, and source-area attribution;
cohort audit identities are summaries and never replace the per-path audit.

Selection and review use pinned source only. They do not run CodeStory indexing,
proof, search, context, packet, Store, or runtime APIs. A positive step records
the exact caller and target declaration ranges, the exact call-expression
range, and the complete receipt line range. All ranges name a normalized
project-relative file, exact file byte length, UTF-8 byte offsets, and the
SHA-256 of that exact slice. Expression and full-line hashes are intentionally
independent. The expression must lie within the declared line; the line includes
LF or CRLF when present and extends to EOF only for an unterminated final line.

Each positive path carries two complete executable negative specifications:
one replaces a step target and one replaces that step's source. Each mutation
changes only its declared typed coordinate and carries a source-audited caller,
target, complete caller body, and `no_direct_call` finding. These negatives may
produce only `Unknown` in production because this corpus supplies audit truth,
not extractor-completeness receipts.

Positive specifications and both concrete negative mutations are converted to
the product's unvalidated proof-contract types and must be accepted as
`Validated` by the product `validate_contract` implementation and
`clause_guard_v1`. The benchmark owns no separate material-token vocabulary.
This keeps whole-input coverage, typed-field coverage, selector and path rules,
UTF-8 spans, exact quotes, overlap, and every guard family on the same executable
contract the product uses.

`materialize --verify-only` may fetch the four fixed repositories into a new
staging directory. It uses noninteractive exact-SHA fetches, detached clean
checkouts, rejects `.gitmodules`, gitlinks, symlinks and untracked/non-regular
oracle files, and hashes the raw bytes from
`git ls-tree -r -z --full-tree <commit>` as source-tree identity. It reads each
tracked source file once, strictly decodes UTF-8, revalidates every declaration,
expression, receipt-line, and negative-audit range, and repeats the HEAD/clean
fence after reads. It atomically installs the workspace and a bounded source
environment descriptor, refuses overwrite, and never creates a cache, index,
database, result, or proof artifact.

Before any write, materialization resolves each destination through its real
existing ancestor and native filesystem identity. It rejects non-root symlink
components and overlap between workspace, cache, and output even when different
spellings traverse a platform root alias. The workspace and output parent
identities are revalidated before output staging, workspace installation, and
descriptor persistence; a failed no-clobber persist removes only the workspace
installed by that attempt.

Corpus validation bijects four references to four loaded cohort roots and
reconciles repository declarations, source trees, canonical hashes, counts,
length distributions, audit rows, and global totals. Within a cohort, positive
caller-target pairs are unique, one primary caller file supplies at most six
cases, and at least five source areas are required when the root declares that
coverage available. No CodeStory result may influence the frozen paths,
methodology, or thresholds.

## Statistical definitions

Full-proof rates use a two-sided 95% Wilson score interval with
`z = 1.959963984540054`. Every interval retains its raw numerator and
denominator and full floating-point bounds. Gate comparison uses the unrounded
lower bound against the exact threshold ratio (`720` means `0.720`). A scaled
floor-to-thousandths value may be rendered in failed-gate diagnostics, but it
never decides the gate. Consequently 96/120 passes the 96-count gate but its
71.9633% lower bound misses 72%; 97/120 is the nearest automatic overall pass.
Likewise 24/120 misses the exact 14% Wilson gate and 25/120 is the nearest
experimental overall pass. The stable 60/120, automatic cohort 21/30, and
stable cohort 12/30 lower bounds clear their exact thresholds.

Ratios use integer half-up rounding to thousandths. P95 uses nearest rank:
sort ascending and select rank `ceil(0.95 * n)`. An empty latency or size set is
zero and therefore non-blocking.

## Derived observations

- A full proof is a product `ContractProven` whose authoritative exact receipts
  cover every ordered oracle step and whose disposition matches that evidence.
- Positive-step recall is the number of distinct oracle steps covered by exact
  authoritative receipts divided by 312.
- A useful partial is an incomplete result with a nonzero exact proven prefix
  and a closed actionable exact gap. Full-or-useful partial divides full proofs
  plus useful partials by 120.
- Actionable incomplete gap divides incomplete results carrying one closed
  actionable gap by all incomplete results. With no incomplete results, the
  value is 100%.
- Warm Unknown p95 uses `warm_end_to_end_ms` for Unknown cases only.
- Revision-native transport p95 is computed separately for each of the four
  negotiated revisions from monotonic construction-and-serialization
  `elapsed_ns`. Revisions are never averaged. This is not installed-host proof;
  installed CLI and MCP end-to-end p95 remains a Task 16 acceptance gate.
- Complete response p95 uses the largest revision-native serialized result for
  each supported `ContractProven` case. Unknown response p95 uses the same
  per-case maximum for Unknown cases. Absolute maximum observes every complete
  projection, serialized result, and reported over-budget actual size.

## Hard gates and decisions

Qualification fails hard on any false `ContractProven` (positive or negative
mutation), non-exact authoritative receipt, production `CertifiedAbsence`,
unclassified positive step, stale or missing project materialization,
invalid result, over-budget result, transport error, response above 64 KiB, or
product/evidence disposition mismatch. Input validation separately rejects
missing cohorts, malformed evidence, or broken provenance and artifact binding.

A validated source dependency with a named passing inseparability test selects
D independently of metrics. Without it, any hard failure selects C. Otherwise
the evaluator checks automatic A, stable-explicit A, experimental B, then C.
Stable A requires every cohort. Experimental B requires at least one cohort at
12/30; failures for every other cohort remain visible. Failed-gate identifiers
are emitted in fixed hard, automatic, stable, experimental order, with cohorts
sorted by repository identity.
