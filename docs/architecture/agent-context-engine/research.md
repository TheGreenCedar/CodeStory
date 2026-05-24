# Agent Context Engine Research

## Research Summary

The research phase compared CodeStory's current negative agent A/B baseline with public context-engine patterns and CodeStory's own benchmark diagnostics. Public docs keep competitor-specific notes out of the architecture record unless they are linked from a verifiable source.

## Findings

| Source | Relevant Finding | Implication For CodeStory |
| --- | --- | --- |
| Current CodeStory A/B baseline | The with-CodeStory arm used more median tokens, wall time, and tool starts than the no-CodeStory arm. | CodeStory must replace ordinary exploration, not merely precede it. |
| Current CodeStory packet-first diagnostics | Corrected with-CodeStory rows now verify answer-packet-first behavior, zero ordinary source reads, and quality pass/fail separately from token and wall-time medians. | CodeStory needs public multi-repo medians before claiming agent savings. |
| Current CodeStory packet-first diagnostics | Rows only become useful when the packet is the first repository-context command and the answer stops broad ordinary source exploration. | CodeStory needs a packet sufficiency contract that tells agents when not to keep reading files. |
| Current CodeStory packet-first diagnostics | Strict reanalysis invalidated older packet-first claims when the first successful repository-context command was not an answer packet. | The CodeStory skill must route packet-first and include a stop rule. |
| Sourcegraph context docs | Coding context can combine search, code graph, and repository signals rather than relying on one source. | CodeStory should preserve hybrid lexical, semantic, and graph retrieval in packet planning. |
| MCP specification | Tools, resources, and prompts are first-class integration surfaces. | CodeStory should expose packet over warm stdio/MCP-compatible read-only tools. |
| LSP specification | Definition, references, and symbol operations are foundational code-intelligence primitives. | CodeStory should keep primitive navigation strong while making packet the broad-task entrypoint. |

## Benchmark Bar

Before CodeStory promotes agent-savings claims, the public benchmark should reach this bar:

- at least five public repositories;
- at least four language families;
- at least six task classes;
- at least three repeats per arm, with four repeats preferred for headline rows;
- medians for cost, tokens, wall time, and tool starts;
- quality gates for expected anchors, citations, and false claims;
- behavior telemetry for ordinary source reads after packet.

## Product Bar

The strongest product lesson is behavioral: the context engine wins when an agent can answer from one compact context call plus one focused source call, then stop. CodeStory should optimize for that behavior.

That means the first packet milestone should measure:

- how many broad `rg` or direct file reads happen after packet;
- whether packet output tells the agent what is already covered;
- whether packet output names the only files worth opening next;
- whether answer quality passes without broad manual exploration.

## Sources

- Internal baseline: [benchmark-results.md](../../testing/benchmark-results.md)
- Internal harness: [codestory-agent-ab-benchmark.mjs](../../../scripts/codestory-agent-ab-benchmark.mjs)
- Sourcegraph context docs: https://sourcegraph.com/docs/cody/core-concepts/context
- Sourcegraph agentic context docs: https://sourcegraph.com/docs/cody/core-concepts/agentic-context
- Model Context Protocol specification: https://modelcontextprotocol.io/specification
- Language Server Protocol 3.17 specification: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/
