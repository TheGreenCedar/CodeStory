# Documentation Template

Consistent structure for new CodeStory documentation.

## Header

- File title as H1
- One sentence stating the reader's job for this page
- No trust-boundary tables in the opening paragraph

## Body structure

- H2 for major sections, H3 for subsections
- ASCII only in new edits unless the file already uses Unicode for a clear reason
- Two-space list indentation

## Content rules

1. Open with the reader's job, not internal architecture unless the page is architecture-owned.
2. Example prompts use portable templates: `[Feature]`, `[path/to/file]`, `[subsystem]`.
3. CLI commands belong in [CLI reference](../users/cli-reference.md), troubleshooting step 2, or contributor docs -- not user quick starts.
4. State what the user does vs what the agent handles.
5. One concept one owner: link [glossary](../glossary.md) and canonical pages instead of redefining terms.

## Tables

- Clear headers; align columns for readability
- Capability or routing tables link to canonical owner pages

## Code blocks

- `text` for agent prompts
- `sh` for POSIX shell
- `powershell` for Windows PowerShell
- `json` for config snippets

## Cross-links

- User topics: `docs/users/`
- Terms: `docs/glossary.md`
- Contributor verification: `docs/contributors/testing-matrix.md`
- Use relative paths; validate links before merge

## Quality checks

Before committing:

```sh
git diff --check
node .github/scripts/check-doc-links.mjs
```

- No duplicated readiness definitions (link glossary)
- No `codestory-cli` in user quick-start sections
- Examples use reader repo placeholders, not CodeStory-internal names, unless the page is contributor-specific

## Example skeleton

    # Document Title

    One sentence: what job this page helps the reader finish.

    ## Section one

    Content with links to [glossary](../glossary.md) for shared terms.

    ## Section two

    ### Subsection

    Detailed content.

    ## See also

    - Related canonical page (`path/to/page.md`)
