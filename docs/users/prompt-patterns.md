# Prompt patterns

CodeStory works best when you ask about **your repository** — symbols, files,
call paths, and tests — not about how the agent should explore the tree.

Use portable placeholders (`[Feature]`, `[path/to/file]`, `[SYMBOL]`) and your
project's real names. Ask the repository question directly; the host adapter or
grounding skill chooses the CodeStory tool.

## Good vs bad

| Bad | Good |
| --- | --- |
| Grep the whole repo for `UserService` and read every hit. | Where is `UserService` defined, who constructs it, and which files should I read first? |
| Run `find . -name "*.rs"` and open files until you find the handler. | How does the HTTP handler for `[route]` work? Cite concrete files and flag gaps. |
| Read `codestory://status` and paste the JSON here. | I am changing `[path/to/file]`. What symbols are affected and what tests should I run first? |
| Use packet/search no matter what. | How does `[subsystem]` interact with `[other area]`? Use CodeStory and cite sources; say if broad search is not ready. |

**Anti-pattern:** asking the agent to grep, glob, or walk the tree **before**
CodeStory has a chance to answer from the indexed graph. That repeats the work
CodeStory already did and burns tokens on duplicate reads.

**Better:** name the symbol, file, or behavior you care about and ask for
definition, callers, impact, or a cited overview.

## Language-flavored examples

These shapes work across hosts; swap in your symbols and paths.

### Python

**Bad**

```text
Search every `.py` file for `process_order` and summarize what you find.
```

**Good**

```text
Where is `process_order` defined, what module imports it, and which pytest files exercise that path?
```

### TypeScript / JavaScript

**Bad**

```text
List all files under `src/` and grep for `useAuth`.
```

**Good**

```text
I am editing `src/hooks/useAuth.ts`. What components import it and what tests should I run first?
```

### Rust

**Bad**

```text
Open every file in `crates/` until you find who calls `dispatch`.
```

**Good**

```text
Where is `dispatch` defined in this workspace, who calls it across crates, and cite the test modules that cover it.
```

## Portable placeholders

| Placeholder | Use for |
| --- | --- |
| `[Feature]` | User-visible capability or subsystem name |
| `[SYMBOL]` | Function, type, class, or module symbol |
| `[path/to/file]` | File you are editing or reviewing |
| `[subsystem]` | Area you want an overview of |
| `[OWNING_MODULE]` | Package, crate, or directory that owns the behavior |
| `[route]` | HTTP route, CLI command, or entry surface |

## Normal use

Ask the repository question directly. CodeStory prepares what it needs on the
first relevant call. If preparation is still running, the agent retries that
same call; you do not need a readiness prompt or setup command.

Host installation and adapter differences: [Codex](codex.md),
[Cursor](cursor.md), [Claude Code](claude-code.md), and [Copilot](copilot.md).

Quality limits and degraded output: [What to expect](what-to-expect.md).
