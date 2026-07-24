# CodeStory user guide

CodeStory gives a coding agent a local map of your checkout and a cited search
index. Install the adapter for your host, open a repository, and ask an ordinary
code question. Repository preparation happens on the first relevant tool call;
there is no background service for you to configure or approve.

## Pick your host

| Host | Setup | What is automatic |
| --- | --- | --- |
| [Codex](codex.md) | Install from `/plugins` | MCP, hooks, skill, matching CLI |
| [Cursor](cursor.md) | Add the rule and MCP config | Repository preparation after MCP connects |
| [Claude Code](claude-code.md) | Install the plugin and configure MCP | Hooks; preparation after MCP connects |
| [Copilot CLI](copilot.md#copilot-cli) | Install hooks and configure MCP | Session instructions; preparation after MCP connects |
| [Copilot editor](copilot.md#copilot-editor) | Add repository instructions and optionally MCP | CodeStory evidence only when MCP is connected |

Codex is the reference experience. Other hosts use the same project-scoped MCP
runtime but require more adapter setup.

## Capability matrix

| Host | MCP auto-start | Hooks | Grounding skill or rule | Managed CLI through adapter |
| --- | --- | --- | --- | --- |
| Codex | Yes | Yes | Full skill | Yes |
| Cursor | No | Rule only | Project rule | Yes, after MCP connects |
| Claude Code | Usually manual | Session hook | Host-dependent | Yes, after MCP connects |
| Copilot CLI | No | Session hook | Partial | Yes, after MCP connects |
| Copilot editor | No | No | Repository instructions | Only when MCP is configured |

## First use

1. Install the plugin or adapter for your host.
2. Start a fresh agent session in the repository.
3. Ask a concrete repository question, such as:

   ```text
   Where is request authentication implemented, who calls it, and which tests cover it?
   ```

The first call builds or refreshes the local repository map. A broad question
may also initialize semantic search and publish its first complete retrieval
generation. On a large checkout this can take several minutes. The agent should
retry the same CodeStory tool after its returned delay while local navigation
remains available.

Success looks like an answer that names real files and symbols, cites its
evidence, and says when coverage is incomplete. It should not ask you to install
a model, start a service, choose a port, or run a repair command.

## What runs locally

The released CodeStory executable includes its search model and embedding
engine. When semantic work is needed, that exact executable automatically runs
one hidden server for the current OS user over private local IPC. It does not
download a model or backend, expose a TCP port, or use Docker. Apple Silicon
uses Metal; supported Windows hardware uses Vulkan. Production never silently
changes from GPU to CPU.

Each repository has its own cache and publication identity. Compatible host
processes share one warm embedding server, but every request still names its
repository explicitly and the server never receives that path. Repositories do
not share indexes or readiness state.

Source and index data stay local by default. Remote summaries are a separate,
explicit trusted-operator feature; repository-controlled network configuration
is disabled unless the process opts into it.

## Platform summary

<!-- codestory-public-support:start -->
| Released package | Local map | Broad retrieval |
| --- | --- | --- |
| macOS 15+ on Apple Silicon | Yes | Metal |
| Windows x64 | Yes | Vulkan |

CodeStory 0.16 publishes only these managed package targets.
Unshipped targets: linux-arm64, linux-x64, macos-x64, windows-arm64. Answer quality and performance are separate release non-claims.
<!-- codestory-public-support:end -->

## Everyday use

Ask about the code, not CodeStory's internal commands. Useful prompt shapes are
collected in [Prompt patterns](prompt-patterns.md).

## Portable prompt shapes

Use the same ordinary questions on every host. The canonical examples and
anti-patterns live in [Prompt patterns](prompt-patterns.md); host guides contain
only installation details and host-specific limitations.

You do not need to check status before a normal question. If a call stays
blocked, use:

- [Trust and readiness](trust-and-readiness.md) to decide what counts as proof;
- [Troubleshooting](troubleshooting.md) for the shortest recovery path;
- [What to expect](what-to-expect.md) for language and repository-size limits;
- [CLI reference](cli-reference.md) for maintainer diagnostics.
