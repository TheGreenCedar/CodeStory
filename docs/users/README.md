# CodeStory user guides

You want your coding agent to answer from cited repo evidence instead of
re-exploring the tree on every question. Pick your host below, install once, open
your repository, and start a fresh session there.

## Day-1 checklist

1. **Pick a host** — if unsure, start with [Codex](codex.md) (recommended first install).
2. **Install once** — follow your host guide; approve hooks when prompted.
3. **Open the repo** you want grounded and start a **fresh** agent session there.
4. **First prompt** — ask a concrete repo question; setup and preparation happen automatically when the host supports them (see [prompt patterns](prompt-patterns.md)).
5. **Success looks like** — the agent cites indexed files, names blocked surfaces if any, and does not guess from partial tree reads. Large repos may take several minutes to index on first open.
6. **Something wrong?** — [Trust and readiness](trust-and-readiness.md), then [Troubleshooting](troubleshooting.md).

You do not need the [CLI reference](cli-reference.md) or [glossary](../glossary.md) for first install.

## Pick your host

| Host | Guide | Best for |
| --- | --- | --- |
| Codex | [Codex](codex.md) | **Recommended first install** — full plugin path: MCP auto-start, hooks, skill, managed CLI bootstrap |
| Cursor | [Cursor](cursor.md) | Project rule plus manual MCP config |
| Claude Code | [Claude Code](claude-code.md) | Lifecycle hooks; MCP setup is manual |
| GitHub Copilot CLI | [Copilot](copilot.md#copilot-cli) | Session-start hooks; no MCP auto-start |
| GitHub Copilot editor | [Copilot](copilot.md#copilot-editor) | Repository instructions only |

Shared references:

- [Trust and readiness](trust-and-readiness.md) — when to trust agent output
- [What to expect](what-to-expect.md) — coverage and quality limits in your repo
- [Prompt patterns](prompt-patterns.md) — good shapes and anti-patterns
- [Troubleshooting](troubleshooting.md) when a session is blocked or output looks stale
- [CLI reference](cli-reference.md) for power-user repair and debug transcripts
- [Glossary](../glossary.md) for terms used across these pages

## Capability matrix

| Host | MCP auto-start | Hooks | Skill | Managed CLI bootstrap |
| --- | --- | --- | --- | --- |
| Codex | Yes | Yes | Yes | Yes |
| Cursor | Manual MCP config | Rule only | Via rule | Via MCP adapter when configured |
| Claude Code | Manual | Yes | Partial | Depends on MCP setup |
| Copilot CLI | No | Session start only | Partial | Manual |
| Copilot editor | No | Instructions only | No | Manual |

Codex is the reference experience. Other hosts reuse the same grounding rules
through thin adapters; see each guide for honest gaps.

## What you do vs what the agent handles

| You | Agent (with CodeStory) |
| --- | --- |
| Install the plugin or adapter for your host | Grounds the checkout, traces symbols, and cites sources |
| Open the repository you want grounded | Uses local graph tools for navigation and broad search when available |
| Ask concrete questions with repo terms | Retries managed preparation and reports plain-language gaps if it cannot proceed |
| Start a fresh session after install or repair | Selects the repository explicitly on every request |

Readiness boundaries in plain language: [Trust and readiness](trust-and-readiness.md).

## Portable prompt shapes

Use your project's symbols and paths, not CodeStory-internal names:

```text
Where is [Feature] defined and who calls it?
```

```text
I am changing [path/to/file]. What symbols are affected and what tests should I run first?
```

```text
How does [subsystem] work? Cite concrete files and flag gaps if coverage is incomplete.
```

More patterns: [Prompt patterns](prompt-patterns.md). Host-specific install steps and `@CodeStory` invocation: see your host guide.
