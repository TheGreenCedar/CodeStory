# Packet generalization

Horizon A is a prompt-blind, retrieval-first interim packet path. The original
question reaches generic lexical and semantic retrieval unchanged. Typed
exact probes constrain identity resolution, and typed free-query probes add
ordinary retrieval queries. No wording is translated into an answer shape or
structural traversal policy. Repository-derived compilation lands separately
under #2106.

## Required sequence

```text
question + typed probes
  -> unchanged-question generic query plan
  -> descriptor-only retrieval
  -> packet-wide admission
  -> admitted source and relation hydration
  -> 16-row / 16-KiB public projection
```

Exact typed selectors are resolved through identity indexes and admitted first.
Remaining candidates are admitted in versioned retrieval-score order. One
packet-scoped session admits at most sixteen stable identities and reserves at
most 16 KiB of conservative source bounds before any candidate source, graph
neighbourhood, node body, or file record is loaded. Initial retrieval, typed
probes, batches, and continuations share that session.

The interim finalizer retains only evidence that passed packet-wide admission
and exact hydration. It converts objective admission and ambiguity gaps into
typed stable continuations. It does not infer which evidence would answer the
question.

## Forbidden production shapes

- Prompt or task-class classifiers, including renamed or encoded variants
- Domain, lifecycle, relation-language, or expected-answer taxonomies
- Fixed answer stages, claim obligations, carrier classes, or magic evidence
  roles
- Prompt-word rescoring after generic retrieval
- Synthesized claims or benchmark-shaped result deletion passes
- Basename-only path identity
- Diagnostic prose reused as a continuation query
- Production dependencies on benchmark manifests or expected answers
- Sufficiency, absence, runtime-execution, or complete-coverage assertions
  inferred from retained or missing evidence

Typed `FreeQuery` probes are ordinary additional generic retrieval queries.
They receive no rank protection, materiality, or sufficiency authority. A
continuation carries stable selectors, publication pins, and the exact typed
structural reason it was offered. It does not claim that the current packet is
insufficient for the answer.

Historical 18-task, Q2, Dart, and 45/54 results are contaminated development
evidence only. They cannot authorize a product or release claim.

## Boundary checker

CI runs:

- `node scripts/lint-retrieval-generalization.mjs`
- `node scripts/check-packet-generalization-boundary.mjs`
- their hostile Node test suites

The packet boundary checker fails closed when no production files are scanned,
masks only real test-only regions, and rejects semantic equivalents of the
forbidden shapes rather than a list of retired identifiers. Vocabulary naming
those shapes is permitted only in tests, tooling, and checker fixtures.

## Public claim

The public packet reports bounded source and indexed structural evidence,
typed gaps, ambiguity, truncation, and its pinned publication identity. It
always reports `answer_sufficiency: not_asserted`. Horizon A establishes this
substrate contract only; it is not product-usefulness evidence.
