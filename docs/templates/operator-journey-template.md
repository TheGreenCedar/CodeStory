# Operator Journey Template

Use this template for host-specific guides and journey pages under `docs/users/`.

## Required sections (host guide skeleton)

### 1. What you get

- What the agent gains on this host
- What you still do manually on this host vs Codex
- Link to [capability matrix](../users/README.md#capability-matrix) when comparing hosts

### 2. Install

- Human steps only
- Approve hooks when your host prompts for them (see [capability matrix](../users/README.md#capability-matrix))
- No CLI commands unless this host requires manual MCP setup paths
- Paths to plugin files, hooks, rules, or instructions

### 3. Install verification

- Three checks before first real task (adapter present, hooks/MCP live, direct grounding call)
- Link to first-session prompt in the guide

### 4. First session

- What you do: open repo, start fresh session
- What the agent does: calls the intended tool and lets CodeStory prepare its prerequisites
- One concrete first prompt (portable template allowed)

### 5. Example prompts

- Three or four portable templates with `[Feature]`, `[path/to/file]`, `[subsystem]`
- Optional host invocation prefix (`@CodeStory`, rule behavior, etc.)

### 6. Troubleshooting (host-specific)

- Link to shared [troubleshooting](../users/troubleshooting.md) for common lanes
- Host-only failures (MCP config, plugin UI, missing hooks)

### 7. Limitations (honest vs Codex)

- Missing auto-start, hooks, skill, or managed CLI bootstrap
- What still works (local navigation vs packet/search)

## Content rules

- Open with the reader's job, not "agents rediscover the repo"
- Say what the user does vs what the agent handles
- One concept one owner: link [glossary](../glossary.md) and [troubleshooting](../users/troubleshooting.md) instead of duplicating
- CLI only in [CLI reference](../users/cli-reference.md) or troubleshooting step 2

## Example stage table (optional, for journey overview pages)

| Stage | You | Agent | Check |
| --- | --- | --- | --- |
| Install | Install plugin or configure MCP | Starts or connects MCP adapter | Fresh session sees CodeStory tools |
| First grounding | Open repo, ask a repository question | Calls `ground` directly | Current files and symbols return |
| Source work | Ask for plan or code path | Uses allowed local graph tools | Claims cite files and symbols |
| Broad discovery | Ask repo-wide question | Uses packet/search when allowed | `retrieval_mode=full` |
| Recovery | Retry the intended tool | Reuses the active preparation operation | The same tool returns evidence or one terminal fallback |

Degraded packet/search is navigation help only, not proof. See [glossary](../glossary.md#retrieval-mode).
