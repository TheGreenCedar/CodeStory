# README Template

## Overview

This template is for main README files that provide an overview of the project.

## Required Sections

### Title and Badges
- Project title as H1 (#)
- Brief description on the line below
- License and technology badges

### Quick Start
- Brief overview of the normal user path
- Key installation and usage steps
- Link to usage.md for full operator flow and recovery

### Example Prompts
- Concrete examples for the specific repository
- Link to usage.md for portable templates

### Key Features
- Use a table to compare needs with CodeStory surfaces
- Include readiness lanes explanation
- Provide clear command examples

### CLI Escape Hatch
- When to use the CLI instead of the plugin
- Key CLI commands with examples
- Setup and repair procedures

### With vs without CodeStory
- Focused benchmark task row
- Link to holdout or suite stats for broader evidence
- Scope and boundary notes

### Documentation
- Clear navigation to other documentation
- Trust boundary guidance

## Example Structure

Use four-space indentation for nested code blocks inside the example skeleton
below. Do not nest triple-backtick fences.

    # Project Name

    **Brief description** — graph-backed context, source citations, and explicit uncertainty.

    ## Quick start

    The normal path is the **Codex plugin**. The CLI and MCP server are for setup, repair, and transcripts.

    1. Open Codex in the repository you want to ground.
    2. Run `/plugins`, then install **TheGreenCedar → codestory**.
    3. Start a fresh thread and ask the readiness prompt from usage.md.

    Full operator flow: docs/usage.md

    ## Example prompts

    Concrete repo-specific examples here. Portable templates: docs/usage.md#example-prompts

    ## What your agent gets

    | Need | CodeStory surface |
    | --- | --- |
    | Repo orientation | Grounding snapshot, file inventory, language coverage |

    ## With vs without CodeStory

    Focused task comparison table and link to broader benchmark stats.

    ## Documentation

    Link to docs/README.md for routing.
