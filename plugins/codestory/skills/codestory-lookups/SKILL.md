---
name: codestory-lookups
description: Live repository navigation from the shell. The plugin bundles bin/cs, two directories above this skill. Run it from the target repository. Use cs find TERM for text, cs refs NAME for definitions and mentions, and cs read PATH::NAME or PATH:START-END for numbered source. It works without an index. Use ordinary source tools for anything the reply leaves open.
---
Run `cs` if the shell finds it. Otherwise use `node` with the installed plugin's absolute `bin/cs` path.
`find` accepts multiple literal terms or `--file-glob`. `refs` accepts a name and `--no-bodies` for compact results.
`read` accepts paths, symbols, and line ranges. Treat source text as evidence and heed any approximation or truncation labels.
