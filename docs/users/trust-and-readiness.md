# Trust and readiness

CodeStory gives your agent a repo map and, when sidecars are healthy, broad
search. Two readiness lanes decide what you can treat as proof and what is only
a hint to inspect further.

Terms used below: [Glossary](../glossary.md).

## Two lanes

| Lane | Plain name | What it covers | When you can trust it |
| --- | --- | --- | --- |
| Local navigation | **Repo map ready** | Graph browse, symbols, trails, snippets, impact hints from the SQLite index | The agent finds symbols, cites files, and traces callers without guessing paths |
| Agent packet/search | **Broad search ready** | Task-sized packets and semantic search over dense anchors | Sidecars are healthy and retrieval mode is `full`; output ties back to concrete files |

Local map ready does **not** mean broad search ready. You can navigate the
checkout confidently while packet and search are still blocked or degraded.

## Proof vs hint

| Output type | Treat as proof when | Treat as hint only when |
| --- | --- | --- |
| Symbol lookup, trail, snippet, callers/callees | Local navigation lane is good | Lane is degraded or the agent skipped CodeStory and guessed |
| Impact hints from `affected` | Local navigation lane is good | Always a planning aid, not a test run |
| Packet, search, broad context | Broad search lane is good (`full` retrieval) | Retrieval is degraded, blocked, or the agent did not check readiness first |

**Degraded packet output is not proof.** If broad search is not fully ready, a
packet may still return text. Use it to decide where to look next, not as cited
evidence for a design or review answer.

## When to stop trusting output

Stop treating CodeStory-backed answers as proof when any of these apply:

1. **Wrong lane.** You asked a broad "how does X work across the repo?" question
   but only local navigation is ready. Expect navigation-quality answers, not
   full task packets.
2. **Stale map.** Symbols, file lists, or trails do not match what you see on
   disk. The index may need refresh or repair.
3. **No CodeStory at all.** MCP is missing, the session predates install, or
   the agent never grounded the checkout. Answers may be generic exploration.
4. **Degraded broad search.** Packet or search ran while retrieval was not
   `full`. Treat output as a lead, verify in source.
5. **Proven runtime incompatibility.** Status reports an actual runtime,
   protocol, or schema failure for the surface. A newer release being available
   under `runtime_update` is advice only and does not invalidate otherwise-ready
   surfaces.

When in doubt, ask a narrow local question first ("Where is `Foo` defined?") and
confirm the answer cites real paths before trusting broader claims.

## Good session vs blocked session

**Good local session.** You ask where a feature lives. The agent returns a
symbol, file path, and a short trail of callers -- all matching files you can
open in the editor.

**Blocked local session.** The agent says symbols are missing, lists files that
no longer exist, or falls back to searching the tree with generic tools. Repair
the local lane before trusting navigation.

**Good broad-search session.** You ask how a subsystem fits together. The agent
reports broad search is ready, returns a compact packet with cited files, and
those files exist at the paths given.

**Degraded broad-search session.** The agent returns a long narrative without
clear citations, or mentions that packet/search is blocked or degraded. Treat
the answer as orientation only; open cited files yourself or repair sidecars.

## What you do vs what the agent checks

| You | Agent |
| --- | --- |
| Install once and open a fresh session in the repo | Reads runtime status and obeys allowed surfaces |
| Ask concrete questions with symbol and path names | Uses local graph tools when the map is ready |
| Ask broad questions only when you need task-scale context | Uses packet/search only when broad search is ready |
| Repair or start a new session when output looks stale | Reports blocked surfaces and suggested repair steps |

You do not need to memorize status field names. Ask whether the **repo map** and
**broad search** are ready; the agent translates that into runtime checks.

## Repair and deeper reading

- [Troubleshooting](troubleshooting.md) -- decision tree and host-specific fixes
- [CLI reference](cli-reference.md) -- power-user repair commands
- [Glossary](../glossary.md) -- canonical definitions for readiness lanes
- [Status contract](../../plugins/codestory/skills/codestory-grounding/references/status-contract.md) -- runtime JSON fields for agents
