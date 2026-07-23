# README Template

Use this template for repository root README files that introduce CodeStory to
operators.

## Required sections

### Title and badges

- Project title as H1
- One-line description focused on the reader's job (cited evidence for agents)
- License and technology badges

### Value proposition

- One paragraph: what the reader gets, not how indexing works internally
- No trust-boundary tables in the opening

### Pick your host

- Table linking to `../users/<host>.md` guides
- Link to [user guides hub](../users/README.md)

### Quick start

- Host-specific install in three steps max
- Approve hooks when your host prompts for them (see [capability matrix](../users/README.md#capability-matrix))
- No `codestory-cli` commands in quick start
- Link to host guide for first-session prompt

### Example prompts

- Three portable templates using `[Feature]`, `[path/to/file]`, `[subsystem]`
- No CodeStory-internal symbol names in the root README examples

### What your agent gets

- Short table: need vs CodeStory surface
- Link to [glossary](../glossary.md) for readiness lanes

### Documentation

- Link to `docs/README.md` for routing
- Link to [troubleshooting](../users/troubleshooting.md)

### Evaluation (below the fold)

- Benchmark or holdout summary table
- Link to stats page with scope and boundary notes
- Demote evaluation below install and prompts

## Do not include in root README

- CLI command cheat sheets (use [CLI reference](../users/cli-reference.md))
- Trust-boundary tables duplicating glossary
- Marketplace maintainer details (use plugin README or host guide)

## Example skeleton

Use four-space indentation for nested code blocks. Do not nest triple-backtick fences.

    # Project Name

    **Brief description** -- graph-backed context, source citations, and explicit uncertainty.

    One paragraph value proposition for the reader's job.

    ## Pick your host

    | Host | Guide |
    | --- | --- |
    | Codex | [Codex guide](../users/codex.md) |

    ## Quick start

    1. Install the plugin for your host (see guide above).
    2. Approve hooks when your host prompts for them.
    3. Open the repository you want to ground and start a fresh session.
    4. Ask a repository question from your host guide.

    ## Example prompts

    ```text
    Where is [Feature] defined and who calls it?
    ```

    ## What your agent gets

    | Need | CodeStory surface |
    | --- | --- |
    | Repo orientation | Grounding snapshot, file inventory |

    Readiness terms: [Glossary](../glossary.md).

    ## Documentation

    Full routing: [docs/README.md](../README.md).

    ## Evaluation

    Benchmark table and link to stats page.
