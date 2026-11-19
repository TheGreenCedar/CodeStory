# CodeStory user guide

CodeStory gives a coding agent live source lookups, a local repository map, and
cited search. Set it up for your host, open a fresh session in your repository,
and ask a code question.

For shell lookups, install the 0.18 CLI and use the command for your host. The
optional plugin adds the repository map and broad search through MCP. On the
plugin path, the first call downloads the matching runtime when needed. See
[First use downloads the runtime](troubleshooting.md#first-use-downloads-the-runtime).

## Pick your host

| Host | Setup | What it provides |
| --- | --- | --- |
| [Codex](codex.md) | `codestory-cli setup codex` | Lookup note in standing instructions |
| [Cursor](cursor.md) | `codestory-cli setup cursor` from the repository | Project rule under `.cursor/rules` |
| [Claude Code](claude-code.md) | `codestory-cli setup claude-code` | Lookup note in standing instructions |
| OpenCode | `codestory-cli setup opencode` | Lookup note in its configuration directory or the repository with `--repo` |
| Antigravity | `codestory-cli setup antigravity` | Lookup note in `~/.gemini/GEMINI.md` or the repository with `--repo` |
| [Copilot CLI](copilot.md#copilot-cli) | Install hooks and configure MCP | Session instructions; preparation after MCP connects |
| [Copilot editor](copilot.md#copilot-editor) | Add repository instructions and optionally MCP | CodeStory evidence only when MCP is connected |

## Capability matrix

The table describes the plugin and MCP setup for each host.

| Host | MCP auto-start | Hooks | Grounding skill or rule | Managed CLI through adapter |
| --- | --- | --- | --- | --- |
| Codex | Yes | Yes | Full skill | Yes |
| Cursor | User-enabled | Yes | Full skill and rule | Yes, after MCP connects |
| Claude Code | Usually manual | Session hook | Host-dependent | Yes, after MCP connects |
| Copilot CLI | No | Session hook | Partial | Yes, after MCP connects |
| Copilot editor | No | No | Repository instructions | Only when MCP is configured |

The Codex, Cursor and Claude Code guides also cover installing the optional plugin.

## Other MCP hosts

Any host that can launch a stdio MCP server can run the same project-scoped
runtime. Point the host's MCP configuration at the packaged launcher script:

```json
{
  "mcpServers": {
    "codestory": {
      "type": "stdio",
      "command": "node",
      "args": ["<plugin-root>/scripts/codestory-mcp.cjs"],
      "cwd": "<plugin-root>",
      "env": { "CODESTORY_CLI": "/absolute/path/to/codestory-cli" }
    }
  }
}
```

- `CODESTORY_CLI` takes an absolute path to a `codestory-cli` executable and
  selects it instead of the provisioned runtime. Status labels this path
  `local_dev_override`.
- Keep the executable in a stable location you own. If you relocate it with a
  link, use a hard link or a real copy: the launcher's containment check
  rejects a managed path whose real location escapes its version directory
  (`manifest_path_escape`).
- Never point a host directly into the revision-hashed plugin cache
  (`<plugin-data>/codestory-cli/<version>/`). Those paths are the plugin's
  private, verified storage and move on upgrade.
- Without `CODESTORY_CLI`, the launcher fetches and verifies the matching CLI
  itself; the override exists for local development and custom installs.

Every tool call still carries an absolute `project` root. There is no ambient
"current repository" for the host to configure.

For read-only MCP clients, set `CODESTORY_MCP_TOOL_SURFACE=lean` in the
launcher environment. Tool discovery returns `find`, `refs`, and `read`
while provisioning and after the runtime starts. Each still takes its own
absolute project path. The default catalog includes the broader graph tools.

For existing installations and custom integrations, follow the
[upgrade guide](upgrading.md) for your current version.

## First use

1. Follow the setup guide for your host.
2. Start a fresh agent session in the repository.
3. Ask a concrete repository question, such as:

   ```text
   Where is request authentication implemented, who calls it, and which tests cover it?
   ```

Shell lookups can read current source before an index exists. With the plugin,
the first relevant call builds or refreshes the repository map. A broad query
may also initialize semantic search. If a tool returns a delay while it
prepares the repository, the agent retries that tool.

Success looks like an answer that names real files and symbols, cites its
evidence, and says when coverage is incomplete. It should not ask you to install
a model, start a service, choose a port, or run a repair command.

## What runs locally

The released CodeStory executable includes its search model and embedding
engine. When semantic work is needed, that exact executable automatically runs
one hidden server for the current OS user over private local IPC. It does not
download a model or backend, expose a TCP port, or use Docker. Apple Silicon
uses Metal; supported Windows and Linux systems use Vulkan. Production never
silently changes from GPU to CPU.

Each repository has its own cache and publication identity. Compatible host
processes share one warm embedding server, but every request still names its
repository explicitly and the server never receives that path. Repositories do
not share indexes or readiness state.

Source and index data stay local by default. Remote summaries are a separate,
explicit trusted-operator feature; repository-controlled network configuration
is disabled unless the process opts into it.

## Platform summary

<!-- codestory-public-support:start -->
| Platform | Release support |
| --- | --- |
| macOS 15+ on Apple Silicon | Supported with Metal |
| Windows x64 | Supported with Vulkan |
| Linux x64 | Supported with Vulkan |
| CPU-only Windows and Linux | Unsupported |
| Intel Mac | Unsupported |
| Windows ARM | Unsupported |
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
- [Glossary](../glossary.md) for readiness terms;
- [CLI reference](cli-reference.md) for maintainer diagnostics.

Product configuration keys are in
[configuration-reference](configuration-reference.md). `CODESTORY_TEST_*`
variables on that page are harness switches, not product settings.
