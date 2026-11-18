## `@COMMAND@` is `cs`

Run `cs` from the target repository.

- `cs refs NAME` finds definitions and uses. Pass multiple names or exact selectors such as `path::Symbol` and `path:LINE`.
- `cs find TERM...` finds literal matches. Use `--file-glob G` to narrow the search.
- `cs read SELECTOR...` accepts a path, `path::Symbol`, `path:LINE`, or `path:A-B`.
