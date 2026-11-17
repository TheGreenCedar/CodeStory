## `@COMMAND@` code lookups & symbol edits

Prefer `@COMMAND@` for codebase exploration and syntax-checked edits:

- `cs slice QUESTION` packs key multi-file declarations (`<=7500B`).
- `cs refs NAME [--file-glob G]` lists uses, definitions, and called functions.
- `cs find TERM... [--file-glob G]` searches text with enclosing declarations.
- `cs read SELECTOR...` prints small files, outlines long ones, or `path:LINE` / `path::Symbol` / `path:A-B` (`<=16000B`).
- `cs edit path::Symbol --stdin` validates syntax and replaces a symbol atomically. Keep edits minimal and verify requested checks.
