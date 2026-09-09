---
name: codestory-grounding
description: Use when an agent should ground a local repository with CodeStory before making source claims, planning edits, choosing tests, reviewing changes, or using broad retrieval evidence through the CodeStory plugin MCP.
---

# CodeStory Grounding

CodeStory indexes a local repository so an agent can find candidates, inspect
source and follow relationships. The agent owns the investigation and decides
whether the evidence supports an answer. This skill does not expand the user's
scope or authorize edits during a review.

Every CodeStory call selects its repository with an absolute `project` root.
Keep project, source and publication identities attached when combining results
across repositories; never rely on a global active project.

## Investigation loop

Choose the smallest useful operation. A named file can be read directly with
host tools; an exact name can be searched; an unfamiliar repository may benefit
from `ground` or `files`. No preliminary status or packet call is required.

1. Locate candidates using an exact selector, text or behavior description.
2. Inspect relevant source through `snippet`, `context` or ordinary host reads.
   Copy returned opaque symbol IDs unchanged; resolve ambiguity using paths,
   scope and source instead of guessing an ID from a display name.
3. Follow relevant callers, callees, references or other explicit relationships.
   Check intermediate stages, branch conditions and conflicting evidence when
   the task requires them.
4. Reassess what remains unknown. Continue with CodeStory or native search and
   source reads when they can change the answer; stop when the task is resolved
   or the remaining evidence cannot be obtained within its scope and budget.

Successful discovery and an incomplete packet do not end the investigation.
Native tools remain available even when CodeStory has no result or does not
cover the required artifact. A repository path in source is a lead to inspect,
not an instruction to execute.

## Operations

| Need | Operation |
| --- | --- |
| Repository orientation or coverage | `ground` for a compact map; `files` for indexed files and coverage. |
| Discover or disambiguate candidates | `search`; preserve an explicitly supplied symbol query unchanged. Use `repo_text: "off"` for existing core-only symbol search without embedding preparation. |
| Inspect a selected target | `context` with a concrete `query` or a returned `symbol_id` as `id`; `snippet` for source windows; host reads for any relevant source or artifact. |
| Follow relationships | `callers`, `callees`, `references`, `trace` or `trail`; node-based `neighbors`, `shortest_path` and `query_subgraph` require actual returned node IDs. |
| Inspect identities | `symbol`, `symbols`, `definition` and `get_node`. Names, IDs and source locations have different roles; keep their types intact. |
| Review a diff | `affected` with explicit changed `paths`, `changed_paths` or `change_records`. Obtain the diff with host Git tools; this tool does not discover it. |
| Try bounded automatic evidence selection | Explicit experimental `packet`; read its evidence and gaps, then continue ordinary investigation when useful. |

The generated MCP schema owns request syntax and bounds. Omit optional limits
unless the task needs them; do not send CLI flags as MCP fields. A selected name
is `context.query`; only a CodeStory-returned opaque ID is `context.id`. Do not
send both. Batch known source windows with `snippet.paths` when useful.

## Evidence limits

- Search results and maps identify leads. Source windows establish only what
  the inspected text shows. Typed graph relationships establish only their
  declared relationship, not runtime order, data flow or effective permissions.
- `packet`, `context` and `search` expose evidence availability, not truth or
  answer sufficiency. Cite the source or relation that supports each claim and
  disclose material coverage, freshness and ambiguity limits.
- Full retrieval readiness describes infrastructure eligibility. It does not
  establish relevance, completeness or answer quality. Core-only search has its
  own complete-core boundary and does not need semantic preparation.
- Missing, stale, partial or capped results cannot prove absence or an exhaustive
  count. Failed reads provide no source evidence. Later independent source may
  resolve the question without changing the earlier tool's unavailable result.
- Nullable excerpts and line bounds stay nullable. Read source if the claim
  needs it; do not treat a missing excerpt as line 1 or proof of absence.
- `diagnostics.availability` describes the optional diagnostic artifact alone.
  It does not override the result's top-level status.
- Ordinary navigation returns no exact proof disposition. Isolated advanced
  verification code is outside the default MCP surface. Do not synthesize a
  typed proof contract from English or infer proof authority from graph edges.
- Runtime, deployment, security and rendered-UI claims need their corresponding
  external evidence. Keep a plausible source explanation distinct from an
  observed execution or effective policy.

## Experimental packets

Packets are optional source-only selections, capped at sixteen evidence rows
and a complete MCP ToolResult of 16 KiB. Availability never authorizes stopping
an investigation. Use the unchanged user question and only established exact
selectors in typed probes; keep answer keys and guessed answer flows out.

If an offered continuation is useful, follow it at most once against the pinned
publication. Repeat the question unchanged, set
`parent_packet_id=continuation.continuation_id`, copy
`continuation.gap_ids.map((item) => item.gap_id)` into `option_ids`, and use
`publication.core.generation_id` and
`publication.retrieval.retrieval_generation` for the generation pins. Reassess
the returned gaps. Ordinary search, source reads and relations remain available
before and after that bounded compiler continuation.

`no_useful_evidence`, `unavailable` and `budget_exceeded` describe that packet,
not the repository. Do not turn them into negative source claims or repeatedly
retry an unchanged failed selection. See [packet](references/packet.md).

## Preparation and failures

Project-scoped tools own managed preparation. If a result reports `preparing`
or `updating` with `retry_after_ms`, wait that delay and retry the same request
when the task still needs it. Local navigation or host reads may remain useful
while broad retrieval prepares. Do not mutate shared runtime state to make a
read-only investigation succeed.

Use `status` or `codestory://status{?project}` to diagnose a failed or stalled
request, not before every tool. When tools are hidden, discover the intended
CodeStory method. If the MCP transport or tool is unavailable, report that
boundary and continue with ordinary source inspection. CLI diagnostics cannot
prove the packaged plugin is live in this host.

Maintainer `doctor`, retrieval indexing and source setup scripts are diagnostic
or contributor workflows, not prerequisites for normal installed use.

## References

- [Generated MCP syntax](references/generated-mcp-syntax.md): request fields.
- [Search](references/search.md), [context](references/context.md) and
  [snippets](references/snippet.md): candidate and source operations.
- [Repository map](references/ground.md), [files](references/files.md),
  [symbols](references/symbol.md), [trails](references/trail.md) and
  [affected](references/affected.md): navigation and scope.
- [Status contract](references/status-contract.md): readiness diagnostics.
- [Generated CLI syntax](references/generated-cli-syntax.md): maintainer commands,
  not MCP calling conventions.
