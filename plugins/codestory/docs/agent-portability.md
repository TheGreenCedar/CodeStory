# Agent Portability

Maintainer pointer for host adapters. Operator documentation lives in the
[user guides hub](../../../docs/users/README.md).

## Operators

| Need | Doc |
| --- | --- |
| Pick a host and install | [User guides](../../../docs/users/README.md) |
| Blocked session | [Troubleshooting](../../../docs/users/troubleshooting.md) |
| CLI repair | [CLI reference](../../../docs/users/cli-reference.md) |

## Maintainers

Adapter layout, MCP script, hooks, and plugin checks:

| Topic | Doc |
| --- | --- |
| Plugin package and host surfaces | [Plugin README](../README.md) |
| Optional dirty-marker Git hooks | [Contributor debugging](../../../docs/contributors/debugging.md#optional-dirty-marker-git-hooks) |
| Status field contract (agent-only) | [status-contract.md](../skills/codestory-grounding/references/status-contract.md) |

Keep adapters thin: point hook-capable hosts at `hooks/` and `skills/`; align
rule-only hosts with the grounding skill contract.

## Hook qualification evidence

The deterministic [route matrix](../tests/fixtures/hook-route-qualification.json)
runs both the current hook and a test-only reproduction of the former policy's
first `UserPromptSubmit` emission as child processes. It scores route categories
and exact tool-name tokens, not rendered prose fragments; historical hook-state
deduplication is outside this comparison.

The separate [observed MCP drills](../tests/fixtures/hook-observed-mcp-drills.json)
record sourced orientation, symbol, call-flow, change-impact, and blocked
fallback observations. Static tests validate those artifacts and the matching
hook category; they do not execute MCP or promote recorded observations into
live proof.
