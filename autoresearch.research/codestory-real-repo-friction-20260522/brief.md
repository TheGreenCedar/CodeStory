# Research Brief: Reduce CodeStory real-repo agent drill friction across Sourcetrail, CodeStory, and rootandruntime. Track current-run quality gaps from source-verified drill evidence, implement improvements, rerun the drill, and repeat until gaps stop falling.

## Request
Reduce CodeStory real-repo agent drill friction across Sourcetrail, CodeStory, and rootandruntime. Track current-run quality gaps from source-verified drill evidence, implement improvements, rerun the drill, and repeat until gaps stop falling.

## Decision To Support
- Decide which CodeStory grounding improvements measurably reduce agent friction
  in the repeatable three-repo drill.

## Success Criteria
- `quality_closed` increases as accepted, source-backed gaps are closed.
- `quality_gap` remains the current open accepted-gap count; when it is zero,
  product iteration stops unless a fresh candidate is logged open first.
- Improvements are verified through targeted tests before being claimed.
- The real-repo drill is rerun after implementation to confirm the agent answer
  improves against source truth.
- Gaps are closed only with source-verified evidence, not because a command ran.

## Constraints
- Use this checkout as the owning repo: `C:\Users\alber\source\repos\codestory`.
- Use native PowerShell and serialize Cargo build/test/check commands.
- Every CodeStory CLI command in the drill must pass `--project` explicitly.
- Treat older autoresearch artifacts as stale for this goal; the active slug is
  `codestory-real-repo-friction-20260522`.
- Do not revert unrelated or user-made worktree changes.

## Known Unknowns
- Whether receiver-aware resolution can be fixed narrowly or needs a larger type
  and ownership model.
- Whether active-path ranking can be inferred from graph callers today or needs
  new persisted usage metadata.
- How much of the drill can become one canonical command without baking in these
  three repos too tightly.
