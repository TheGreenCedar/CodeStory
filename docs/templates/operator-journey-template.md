# Operator Journey Template

## Overview

This template is for documenting the operator journey through CodeStory.

## Required Sections

### Journey Overview
- Brief explanation of the operator journey
- Key stages and their purposes
- Trust boundary explanation

### Stage-by-Stage Documentation
- Use a table to document each stage
- Include human action, agent/CLI action, and trust check
- Provide clear examples for each stage

### Context and Background
- Explanation of why this journey exists
- Background on the problem being solved
- Rationale for the journey structure

### Trust Boundary Documentation
- Clear explanation of trust boundaries
- When to trust what output
- How to verify readiness

### Examples and Templates
- Concrete examples for the specific repository
- Generalizable prompt templates
- Adaptation guidance for different scenarios

## Example Structure

```markdown
# Operator Journey

Brief explanation of the operator journey.

## Stage-by-Stage Documentation

| Stage | Human action | Agent/CLI action | Trust check |
| --- | --- | --- | --- |
| Install | Install the `codestory` plugin from `TheGreenCedar`. | Plugin starts `codestory-cli serve --stdio --refresh none`. | Fresh thread sees the active MCP runtime. |
| First grounding | Ask the agent to check readiness and ground the repo. | Read `codestory://status`, then `codestory://grounding` or `ground`. | `local_navigation` is ready before using local graph output. |
| Source work | Ask for a plan, review, or code path. | Use `files`, `symbol`, `trail`, `snippet`, `context`, and `affected`. | Claims cite concrete files, node ids, snippets, or trails. |
| Broad discovery | Ask a repo-wide question. | Use `packet` or `search`. | Trust only when `agent_packet_search` is ready and `retrieval_mode=full`. |
| Repair | Ask for a transcript or run CLI directly. | Use `doctor`, `index`, `retrieval status`, and sidecar repair commands. | Repeat readiness checks after repair. |

Packet/search output from degraded retrieval, missing sidecars, stale manifests,
or any non-`full` retrieval mode is navigation help only. It is not proof.
```
