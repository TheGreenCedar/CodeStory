## `@COMMAND@` is `cs`

`cs` searches the current source of this repository. It is as fast as grep and returns code, not just locations.

- `cs refs NAME` lists every use of a name with the whole enclosing function of each use, the definition itself, and the functions the definition calls. Every definition of an ambiguous name is listed.
- `cs find TERM...` is a literal text search that returns enclosing functions the same way. `--file-glob G` filters, or lists files when used alone.
- `cs read path` prints a small file, or an outline of a large one. `cs read path::Symbol path:LINE path:A-B ...` prints whole declarations, several per call.
- `--no-bodies` gives a compact listing with one line per match.

One `cs refs` usually replaces a grep and the reads that follow it.
