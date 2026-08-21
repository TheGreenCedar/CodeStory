# Proof availability qualification methodology v1

This qualification decides only whether the indexed source call-path verifier
is useful enough for an automatic, stable explicit, experimental manual, or
dark product role. It does not prove runtime execution, unrestricted English
translation, installed-host latency, or production negative completeness.

## Frozen population and identities

The corpus contains 120 positive requests, 30 in each of four repository
cohorts, 312 audited positive call steps, and 240 negative mutations. Artifact
identity is one-way: this methodology's raw SHA-256 is stored in the canonical
threshold document; its domain-separated canonical SHA-256 is stored in the
corpus; the canonical corpus and threshold identities are stored in the result.
The evaluator validates all three inputs before calculating a decision.

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
