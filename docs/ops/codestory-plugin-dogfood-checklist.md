# CodeStory plugin dogfood checklist

Run this from the target repo after plugin/runtime changes. Keep the transcript
short: active binary, status, allowed surfaces, local-only behavior, final git
state.

## 1. Workspace state

```powershell
git status --short --branch
```

Keep: repo path, branch, and pre-smoke clean/dirty state.

## 2. Fallback CLI evidence

Use this only for repair/debug. Once MCP is live, `codestory://status` is the
runtime truth.

```powershell
where.exe codestory-cli
codestory-cli --version
```

Keep: first PATH hit and version. If these disagree with MCP status, trust
`server_executable` and `server_version` for the active session.

## 3. Status-first plugin smoke

Fresh Codex thread prompt:

```text
@CodeStory read codestory://status first. Report server_version, server_executable, retrieval_mode, allowed_surfaces for ground/files/symbol/definition/trail/references/snippet/affected/symbols/get_node/neighbors/shortest_path/query_subgraph, and allowed_surfaces.packet.allowed, allowed_surfaces.search.allowed, and allowed_surfaces.context.allowed. Do not run packet, search, or context unless that surface's allowed bit is true and retrieval_mode=full.
```

Pass:

- Local graph surfaces such as `ground`, `files`, `symbol`, `definition`,
  `trail`, `references`, `snippet`, `affected`, `symbols`, `get_node`,
  `neighbors`, `shortest_path`, and `query_subgraph` gate local browse.
- `allowed_surfaces.packet.allowed`, `allowed_surfaces.search.allowed`, and
  `allowed_surfaces.context.allowed` gate sidecar-backed agent surfaces.
- `packet`, `search`, and `context` stay blocked unless `retrieval_mode=full`.

## 4. Local-only audit smoke

Same thread prompt:

```text
@CodeStory do a read-only local audit of this checkout using only local graph surfaces allowed by codestory://status. Use ground/files/symbol/definition/trail/references/snippet/affected/symbols/get_node/neighbors/shortest_path/query_subgraph evidence only when that surface's allowed bit is true. Do not edit files and do not repair sidecars unless packet, search, or context is required.
```

Pass: local graph surfaces only, no `packet`, `search`, or `context` while
blocked, no sidecar repair for local-only work, and no file edits.

## 5. Final state

```powershell
git status --short --branch
```

Pass: no new changes from the local-only audit. Any changed file fails the smoke
until the write source is explained.
