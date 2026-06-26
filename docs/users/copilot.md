# GitHub Copilot

CodeStory adapters for GitHub Copilot split by surface: CLI hooks vs editor
instructions. Neither path auto-starts MCP.

## Copilot CLI

Use session-start hooks to inject CodeStory-first grounding rules. You manage
the CLI and any MCP connection yourself.

### What you get

| You | Agent |
| --- | --- |
| Install Copilot CLI plugin metadata | `sessionStart` runs `codestory-activate.cjs` |
| Ensure runtime is reachable | Uses MCP only if you configure it separately |
| Ask repo questions | Follows injected instructions when hooks run |

Plugin manifest: `plugins/codestory/.github/plugin/plugin.json`  
Hooks: `plugins/codestory/hooks/copilot-hooks.json`

### Install

From the CodeStory repository root (or any checkout that contains
`plugins/codestory/`):

**Install from GitHub (no local clone required):**

```bash
copilot plugin install TheGreenCedar/CodeStory:plugins/codestory
```

**Install from a local checkout:**

```bash
copilot plugin install plugins/codestory
```

Re-run the same `copilot plugin install ...` command after you change plugin
files; Copilot CLI caches installed plugins.

Verify the install:

```bash
copilot plugin list
```

You should see **codestory** in the list.

Configure MCP manually if you need `codestory://status` in Copilot CLI
sessions — point MCP at `plugins/codestory/scripts/codestory-mcp.cjs` (same
shape as [Cursor MCP config](cursor.md#2-mcp-server-copy-shipped-config)).

Open the repository you want to ground in Copilot CLI.

Skills ship under `plugins/codestory/skills/` but coverage is partial compared
to Codex.

### Install verification

Run these three checks before your first real task:

1. **Adapter present** — `copilot plugin list` shows **codestory**. Confirm the
   manifest at `plugins/codestory/.github/plugin/plugin.json` and hooks at
   `plugins/codestory/hooks/copilot-hooks.json`.
2. **Hooks live** — Start a new Copilot CLI session in the repo; session-start
   hook should inject CodeStory grounding rules (`node` must be on PATH).
3. **First status read succeeds** — Use the readiness prompt in [First
   session](#first-session). With MCP configured, the agent should answer in
   plain English whether your repo map is ready; without MCP, follow [CLI
   reference](cli-reference.md) repair steps.

### First session

Start a new Copilot CLI session in the repo. Session-start hook injects
grounding rules. Ask:

```text
Read codestory://status if MCP is available, ground this checkout if allowed, and tell me which surfaces are ready.
```

**Expected wait:** On a large repository, the first index build can take several
minutes. Let the agent finish grounding before you ask it to edit files.

**Success looks like:** The agent confirms your repo map is ready (or gives clear
repair steps), and hooks ran without blocking the session.

Without MCP, see [CLI reference](cli-reference.md) for repair transcripts.

### Example prompts

```text
Where is [Feature] defined and who calls it?
```

```text
I am changing [path/to/file]. What symbols are affected and what tests should I run first?
```

```text
How does [subsystem] work? Cite concrete files.
```

More pairs and anti-patterns: [Prompt patterns](prompt-patterns.md).

### Troubleshooting (CLI)

| Symptom | What to try |
| --- | --- |
| No grounding injection | Confirm plugin hooks registered; `node` on PATH |
| No status resource | Configure MCP or use [CLI reference](cli-reference.md) |
| Stale graph | [Troubleshooting - local navigation](troubleshooting.md#local-navigation-stale-or-blocked) |

### Limitations vs Codex

No MCP auto-start, no prompt hooks (session start only), partial skill, manual
CLI bootstrap. Compare [capability matrix](README.md#capability-matrix).

---

## Copilot editor

Repository instructions tell the editor agent to prefer CodeStory when MCP is
available. No hooks, no skill package, no managed bootstrap.

### What you get

| You | Agent |
| --- | --- |
| Keep `.github/copilot-instructions.md` in the repo | Reads instructions before source claims |
| Optionally connect MCP yourself | Uses `codestory://status` when MCP is live |
| Ask repo questions | Grounding only when MCP + instructions align |

Canonical instruction file in this repo: `.github/copilot-instructions.md`
(same rules as `plugins/codestory/.cursor/rules/codestory.mdc`).

### Install

1. Add or copy `.github/copilot-instructions.md` to your project root.
2. Optionally configure an MCP server to `plugins/codestory/scripts/codestory-mcp.cjs`
   if your Copilot editor build supports MCP (see [Cursor MCP config](cursor.md#2-mcp-server-copy-shipped-config)).
3. Open the repository in the editor.

### Install verification

Run these three checks before your first real task:

1. **Adapter present** — `.github/copilot-instructions.md` exists at the repo
   root (copy from this repo or from
   `plugins/codestory/.cursor/rules/codestory.mdc` content).
2. **MCP live (optional)** — If your editor build supports MCP, the CodeStory
   server is connected.
3. **First session succeeds** — Use the readiness prompt in [First
   session](#first-session-1). The agent should acknowledge CodeStory guidance
   and, when MCP is connected, confirm whether your repo map is ready.

### First session

Open Copilot chat in the repo and ask:

```text
Check CodeStory status if available, then tell me what is indexed in this checkout before I edit [path/to/file].
```

**Expected wait:** On a large repository, the first index build can take several
minutes when MCP triggers a fresh index.

**Success looks like:** The agent follows the repo instructions and tells you
what is indexed before suggesting edits.

### Example prompts

Same portable shapes as [Copilot CLI](#example-prompts) above.

### Troubleshooting (editor)

| Symptom | What to try |
| --- | --- |
| Agent ignores CodeStory | Confirm `copilot-instructions.md` at repo root; start fresh chat |
| No MCP | Instructions point to CLI repair -- [CLI reference](cli-reference.md) |
| Wrong repo scope | Open workspace at repository root |

### Limitations vs Codex

Instructions only: no hooks, no skill, no MCP auto-start, manual CLI. Weakest
automation of the supported hosts.

Shared repair: [Troubleshooting](troubleshooting.md).
