---
name: codestory-grounding
description: Use when an agent should ground a local repository with CodeStory before making source claims, planning edits, choosing tests, reviewing changes, or using broad retrieval evidence through the CodeStory plugin MCP.
---

# CodeStory Grounding

CodeStory keeps a local repository map and broad-search index so agents can
reach useful evidence without rediscovering the same code every turn.

The target is always the repository being grounded. Pass its exact absolute
root as `project` on every CodeStory call. Never rely on a global active
workspace.

## Direct Tool Loop

Call the tool that matches the task. Do not call `status` first.

Using this skill does not require an MCP call when the requested work is fully
local to an already named evidence surface—for example, inspecting or editing
the content of `assets/desk.svg`. Inspect that surface with the host's direct
file-read action; do not substitute CodeStory `snippet` or another MCP tool.
Naming a path inside a broad flow or subsystem question does not make the task
file-local. It also does not make the evidence surface complete when the task
asks about ownership, dependencies, runtime behavior, architecture, change
impact, or another claim whose evidence extends beyond the file. For those
tasks, select the narrowest CodeStory tool that can add evidence; do not call
broad `ground` as a pre-edit ceremony.

1. Resolve the target repository root.
2. Call the intended tool with `project=<absolute-root>`.
   Omit optional numeric bounds unless the task requires one; when supplied,
   keep them within the generated schema instead of guessing a generic page size.
3. If the result says `state=preparing` or `state=updating` and includes
   `retry_after_ms`, wait for that delay and retry the same tool with the same
   arguments. The delay tracks observed preparation progress, so honor the
   reported value instead of a fixed poll interval. Retry directly without a
   shell wait. Do not poll status or ask the user to set up CodeStory.
4. Preserve cited anchors in source claims. When the task still needs evidence,
   follow a returned stable identity or exact path with the narrow source or
   relation operation that answers the next question. Let observed repository
   evidence choose the next operation; do not translate prompt wording into a
   guessed answer flow.

CodeStory prepares its local repository map and shared per-user retrieval server
automatically. `status` and
the project-bound `codestory://status{?project}` resource are optional
diagnostics for a failed or unexpectedly slow request, not prerequisites for
normal grounding.

If CodeStory tools are hidden and deferred discovery is available,
search only for the intended tool, for example `codestory mcp packet`, then call
it directly. If the plugin MCP is unavailable, use ordinary source inspection
and report the visibility gap. Do not substitute CLI diagnostics for a live
plugin result unless the user explicitly asks.

Read linked installed guidance files directly. Once a linked path is known, do
not use `grep`, `rg`, or another search/probe command against the installed
plugin package to locate a documented field or excerpt.

## Task Router

| Situation | Route |
| --- | --- |
| Repository orientation | `ground`; use `files` for language mix or coverage gaps. |
| Exact named file, path, or static asset with file-local evidence | Use the host's direct file-read action, not CodeStory `snippet` or another MCP tool. When adding it to a packet, use an `exact_path` tagged probe; do not run broad grounding merely to rediscover the path. If the task asks about relationships, ownership, or impact, use the corresponding narrow tool. |
| Discover or disambiguate a symbol | Discovery leads come from `search`; they identify candidates and never prove a claim. Select an unambiguous returned identity, then use `context`, `snippet`, or an explicit relation operation when the task needs more evidence. Preserve ambiguity instead of guessing. |
| Get evidence for one selected target | Use `context` with that exact selected target. A user-supplied name is `query`; `id` is only an opaque `symbol_id` copied unchanged from a CodeStory result. Never guess an ID, broaden the target, or treat evidence availability as proof. |
| Follow a call path for navigation | `callers`, `callees`, `trace`, or `trail` can navigate the ordinary graph. Use `neighbors`, `shortest_path`, or `query_subgraph` only for a named node; none of these tools returns an exact proof disposition. |
| Verify an exact call path the user already wrote | When the request supplies a complete host-supplied `call-path/v1` document, call `verify_indexed_direct_calls` with that text unchanged in `call_path`. Do not compose one from English, and do not repair a partial one. |
| Review change impact | `affected` with explicit Git-changed `paths` (or `changed_paths` / `change_records`). Never omit the path source. |
| One ordinary graph node | `get_node`, `definition`, `references`, or `symbols` provide navigation details, not a proof disposition. Use `context` when the task needs the schema-v3 evidence projection for that selected target. |
| Broad structural question | Use `packet` first; answer from its evidence rows when they are enough, follow a returned bounded continuation at most once, and use exact navigation for a returned identity when the task still needs evidence. |

## Evidence Rules

- Treat CodeStory output as evidence, not omniscience.
- A failed direct file read produces no source evidence. Do not cite the path as
  read or make a source-backed claim from it; preserve the unresolved material.
- An irrelevant CodeStory call adds no evidence. Skipping one for a complete,
  file-local surface is a valid use of this router; do not report that as
  plugin unavailability.
- Local repository-map output is navigation evidence. Broad packet/search
  output is stronger only when the response reports full retrieval readiness.
- `packet`, `context`, and `search` report evidence availability, not whether a
  claim is true. Cite only the returned evidence rows and state every material
  returned gap. Never turn `available` into authority for a claim the rows do
  not establish. When a returned gap leaves requested material unresolved, the
  final outcome remains unknown even if the result also contains useful evidence.
- Make claims no broader than the cited source or typed relation. A gap does
  not erase supported evidence, and a missing edge does not prove absence.
- A `context` evidence row matching the returned target `symbol_id` is focused
  identity and location evidence even when its optional `excerpt` is null.
  That null does not itself create an omission or an `unknown` outcome unless
  the request asked for source text or a claim that the remaining row fields
  cannot support.
- `diagnostics.availability` describes only the optional diagnostics artifact.
  It never overrides the result's top-level `status`, creates a gap, or supplies
  an `unavailable` outcome or reason code.
- Search and packet results may lead to a focused source or relation operation.
  Keep each follow-up tied to a returned stable identity or exact path, and stop
  when additional evidence cannot change the task outcome.
- Copy a returned `symbol_id` unchanged into `context.id`; never derive that ID
  from a display name or other prose.
- Pass an explicitly supplied symbol name to `search.query` unchanged. Do not
  add descriptive words such as "declarations named" or rewrite the selector.
- `verify_indexed_direct_calls` is the only surface that returns `contract_proven` or
  `contract_refuted`. It verifies the `call-path/v1` document it is given; it
  does not translate prose. Never call it automatically from a packet, search
  result, context result, or guessed natural-language contract. Cite only the
  `receipt_id` values selected by its disposition; a proof `fact_id` or
  `edge_id` is not an authoritative receipt identity. When summarizing a
  refutation basis in a scalar field, copy its `refutation.kind`; do not replace
  that scalar with the entire refutation object. Typed proof gaps have a `kind`
  and selector or step index, not a packet-style `gap_id`; preserve the kind as
  a reason code instead of inventing a gap identity.
- A semantic proof tool error (`isError:true`) is an invalid contract, not
  typed-proof evidence. Preserve no proof authority or disposition, copy a
  reason code only when the payload supplies one explicitly, and never derive a
  code from human-readable validation text.
- When the user asks for exact proof from English but supplies no complete
  `call-path/v1` document, stop and report that the document is required. Do not
  call a repository tool or substitute packet, search, context, or source
  evidence for the requested proof.
- Preserve `contract_proven`, `contract_refuted`, `unknown`, and `unavailable`
  exactly. `unknown` is not absence, and `unavailable` is not negative proof.
  Exact structural proof does not establish runtime execution, reachability,
  temporal order, ownership, data flow, or subsystem non-participation.
- When `packet.status=continuation_available`, execute only the returned bounded
  continuation, once, against its pinned publication. When the user supplies
  an exact packet question, copy it verbatim into `question`, including its
  punctuation; do not paraphrase or trim it. Repeat that same question with
  `parent_packet_id=continuation.continuation_id`, use
  `continuation.gap_ids.map((item) => item.gap_id)` as the string `option_ids`,
  and copy the core/retrieval generation
  IDs from `publication`. Then answer from the combined evidence and the
  continuation result's remaining gaps. A first-pass continuation-required gap
  is resolved when it is absent from that result; retain other first-pass gaps
  only when the continuation still reports them. The bounded packet
  continuation is distinct from ordinary exact navigation after the packet.
- `no_useful_evidence` and `unavailable` describe the packet result, not the
  repository. Preserve that outcome while using exact navigation when it can
  still ground the task. Never turn the missing packet evidence into an absence
  claim.
- `affected` is planning evidence, not a guarantee that every runtime effect was
  found.
- Tagged probes select exact or additional evidence work. They do not choose
  route order or replace the packet availability and gap fields.
- Exact probes carry user-supplied or already established paths and identities.
  Do not synthesize a selector or continuation pin from diagnostic prose.
- Do not paste empty grounding output as context. If a repository truly has no
  supported files, fall back to ordinary inspection or resolve the intended
  root when it is ambiguous.

## Failure Handling

- `preparing`: retry the same tool after its delay.
- `updating`: the last complete repository map remains usable; retry the same
  tool when current publication evidence is required.
- `working_locally`: use local navigation while broad search prepares.
- MCP transport or tool absence permits ordinary source inspection when it is
  needed to continue, with the CodeStory availability gap reported. A
  successful result tagged `unavailable` remains unavailable even if another
  repository operation later supplies evidence.

Maintainer commands such as `doctor`, `ready`, and retrieval status are debug
transcript tools. They do not prove that the installed plugin is live in the
agent host.

`setup.ps1` and `setup.sh` under this skill are build-from-source paths for
contributors, not normal installation steps.

## References

- [Generated MCP syntax](references/generated-mcp-syntax.md) is the agent
  argument source of truth. Do not send CLI flags as MCP fields.
- [status contract](references/status-contract.md)
- [repository map](references/ground.md)
- [files](references/files.md)
- [affected](references/affected.md)
- [packet](references/packet.md)
- [search](references/search.md)
- [context](references/context.md)
- [symbols](references/symbol.md)
- [trails](references/trail.md)
- [snippets](references/snippet.md)

Maintainer CLI `--help` lives in [generated CLI syntax](references/generated-cli-syntax.md)
and is not an MCP calling convention.
