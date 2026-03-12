# Add A Shared Contract

1. Add the type in the legacy implementation crate where it belongs today.
2. Re-export it through `codestory-contracts`.
3. Update callers to import from `codestory-contracts`.
4. Add serialization or round-trip tests if it crosses adapter boundaries.
5. Update the contracts subsystem guide.
