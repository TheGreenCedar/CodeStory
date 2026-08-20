# Trust and readiness

CodeStory has two evidence lanes. They can be ready at different times, so a
useful local answer does not automatically prove that broad repository search
is current.

## Evidence lanes

| Lane | Covers | Trust it when |
| --- | --- | --- |
| **Repository map** | Files, symbols, definitions, callers, trails, snippets, and changed-file impact hints | Returned paths exist, symbols resolve, and the tool served a complete local publication |
| **Broad search** | Packet, semantic search, and broad context across lexical, vector, and graph artifacts | The requested tool succeeds against `retrieval_mode=full` and its citations resolve |

The repository map remains useful while broad search initializes. CodeStory
never treats a half-published generation as current: readers see one complete
old or new publication.

## Proof, planning evidence, and hints

| Result | How to use it |
| --- | --- |
| Resolved symbol, caller, trail, or snippet from a ready map | Source-navigation evidence |
| `affected` output | A bounded change-planning aid, never proof that a test ran or that every impact was found |
| Packet with `supported` disposition and resolvable citations | Evidence for the covered claims |
| Packet with `drill_once` disposition | A useful lead; run the one requested follow-up packet before claiming completeness |
| Packet with `not_established` disposition | Answer only what the compiled support establishes, then name what stayed unproven |
| Packet with `unavailable` disposition | Broad search could not run; fall back to focused source inspection and state the gap |
| Repo-text or semantic suggestion without a resolved symbol | Navigation hint to verify in source |
| `working_locally` state | Use local graph tools; broad search is still preparing |
| `unavailable` state | Fall back to focused source inspection and state the gap |

`retrieval_mode=full` proves that the retrieval infrastructure is coherent. It
does not guarantee that a particular answer found enough evidence. The packet's
disposition and citations still matter. Compact and MCP status also report
`degraded_reason` and `live_ready`, so `retrieval_mode=full` with
`live_ready=false` means the publication class is full but packet and search
must not run yet.

## When to stop trusting a CodeStory claim

- Paths or symbols do not match the checkout.
- The answer came from an old host session that does not expose CodeStory MCP.
- A broad tool is still preparing, unavailable, or returned a packet whose
  disposition is `drill_once` or `not_established`.
- Citations cannot be resolved to the files they name.
- The agent substituted a generic tree search without reporting the CodeStory
  gap.
- Status reports an actual runtime, protocol, publication, or schema failure for
  that surface.

A compatible update being available is advisory. It does not invalidate an
otherwise ready installed runtime.

## Normal retry behavior

Do not ask for status before every question. Call the tool that matches the task.
If it returns `preparing` or `updating`, wait for `retry_after_ms` and retry that
same tool with the same repository and arguments. Read status only when that
loop does not converge or the task is explicitly diagnostic.

See [Troubleshooting](troubleshooting.md) for recovery and the
[agent status contract](../../plugins/codestory/skills/codestory-grounding/references/status-contract.md)
for wire fields.
