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

On a host whose direct file read is surfaced as a bounded command action, one
`cat` or `sed` read of the exact authorized file is the direct-read action. It
does not authorize shell search, directory probing, a second read of the same
path, or a recovery loop. Attempt that exact read before reporting the named
file unavailable.

1. Resolve the target repository root.
2. Call the intended tool with `project=<absolute-root>`.
   Omit optional numeric bounds unless the task requires one; when supplied,
   keep them within the generated schema instead of guessing a generic page size.
3. If the result says `state=preparing` or `state=updating` and includes
   `retry_after_ms`, wait for that delay and retry the same tool with the same
   arguments. The delay tracks observed preparation progress, so honor the
   reported value instead of a fixed poll interval. Retry directly without a
   shell wait. Do not poll status or ask the user to set up CodeStory.
4. Preserve cited anchors in source claims. Read focused source only when the
   task is file-local and the user named the exact file, or a material result
   gap identified one exact focused path. Read each authorized path at most
   once. A path named only as a conditional fallback is not a
   user-named file for this purpose: the returned gap itself must name that
   exact path. A generic gap or unresolved obligation authorizes no source
   read. A path appearing only in an evidence row is also not source-read
   authorization. A search lead is never source-read authorization.

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
| Discover or disambiguate a symbol | Discovery leads come from `search`; they identify candidates and never prove a claim. After a successful search, stop for that turn unless the current request already supplied an exact selection criterion such as one project-relative path and asked for focused evidence after disambiguation. In that case only, choose the unique typed evidence row with a non-null `symbol_id` that matches the exact path, then call `context` with that `symbol_id` as `id`. Do not inspect source merely to upgrade a discovery result. Missing excerpts, unavailable diagnostics, and multiple leads do not authorize source inspection. |
| Get evidence for one selected target | Use `context` with that exact selected target. A user-supplied name is `query`; `id` is only an opaque `symbol_id` copied unchanged from a CodeStory result. Never guess an ID, broaden the target, or treat evidence availability as proof. |
| Follow a call path for navigation | `callers`, `callees`, `trace`, or `trail` can navigate the ordinary graph. Use `neighbors`, `shortest_path`, or `query_subgraph` only for a named node; none of these tools returns an exact proof disposition. |
| Verify an already translated exact call-path contract | For a host-supplied or user-supplied complete typed contract, call `prove_call_path` with the unchanged `source_text`, clauses, and exact spec. Do not infer or assemble a typed contract from English. |
| Review change impact | `affected` with explicit Git-changed `paths` (or `changed_paths` / `change_records`). Never omit the path source. |
| One ordinary graph node | `get_node`, `definition`, `references`, or `symbols` provide navigation details, not a proof disposition. Use `context` when the task needs the schema-v3 evidence projection for that selected target. |
| Broad structural question | Use `packet`; answer only from its evidence rows, name its gaps, and follow a returned bounded continuation at most once. Use `search` or `context` only for a user-named exact target, not as packet recovery. |

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
- For every requested material stage that a cited evidence row establishes,
  state a direct subject-verb claim naming the subject and its established
  action before discussing any gap. Do not substitute a heading, symbol
  inventory, or adjacent partial observation for that supported claim, and do
  not make the claim broader than the cited row. Then scope gaps only to the
  requested stages or links the rows do not establish. A gap does not erase a
  supported stage and does not authorize a source read unless it identifies the
  exact focused path required by the direct-tool rule above.
- A `context` evidence row matching the returned target `symbol_id` is focused
  identity and location evidence even when its optional `excerpt` is null.
  That null does not itself create an omission or an `unknown` outcome unless
  the request asked for source text or a claim that the remaining row fields
  cannot support.
- `diagnostics.availability` describes only the optional diagnostics artifact.
  It never overrides the result's top-level `status`, creates a gap, or supplies
  an `unavailable` outcome or reason code.
- A successful discovery-only `search` is terminal for that turn unless the
  request already supplied one exact selection criterion. That exception may
  map the unique matching non-null `evidence[].symbol_id` into `context.id`;
  it does not authorize source inspection or another discovery query. Successful
  `context`, completed `packet`, and `prove_call_path` results are also terminal
  except for an explicitly returned packet continuation or an exact authorized
  source fallback. Do not raise authority by adding an unrequested source read.
- Pass an explicitly supplied symbol name to `search.query` unchanged. Do not
  add descriptive words such as "declarations named" or rewrite the selector.
- `prove_call_path` is the only surface that returns `contract_proven` or
  `contract_refuted`. It verifies a host-supplied interpretation; it does not
  translate prose. Never call it automatically from a packet, search result,
  context result, or guessed natural-language contract. Cite only the
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
- When the user asks for exact proof from English but supplies no complete typed
  contract, stop and report that the typed contract is required. Do not call a
  repository tool or substitute packet, search, context, or source evidence for
  the requested proof.
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
  only when the continuation still reports them. Do not start a free-form
  `search` / `context` / `trail` / `snippet` recovery loop from packet. Do not
  substitute globbing, directory listing, repository search, or shell commands
  for that forbidden recovery loop.
- `no_useful_evidence` and `unavailable` are terminal CodeStory states. A typed
  `Unavailable` result is terminal and remains unavailable. Inspect ordinary
  source only when the user named the file or the returned gap identifies the
  exact focused surface; that fallback does not erase the unavailable outcome.
  Never turn a gap into an unconstrained repository search.
- `affected` is planning evidence, not a guarantee that every runtime effect was
  found.
- Tagged probes select exact or additional evidence work. They do not choose
  route order or replace the packet availability and gap fields.
- A path named only for a conditional continuation or source fallback is
  fallback-only. Do not send it as an initial packet probe or synthesize
  continuation pins before the first result explicitly returns them. It may be
  read only when a returned material gap itself names that exact path; a
  generic gap does not combine with the conditional path to authorize a read.
- Do not paste empty grounding output as context. If a repository truly has no
  supported files, fall back to ordinary inspection or resolve the intended
  root when it is ambiguous.

## Failure Handling

- `preparing`: retry the same tool after its delay.
- `updating`: the last complete repository map remains usable; retry the same
  tool when current publication evidence is required.
- `working_locally`: use local navigation while broad search prepares.
- MCP transport or tool absence authorizes ordinary source inspection when it
  is needed to continue, with the CodeStory availability gap reported. A
  successful tool result tagged `unavailable`, including a typed `Unavailable`,
  follows the terminal result and exact source-authorization rules above.

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
