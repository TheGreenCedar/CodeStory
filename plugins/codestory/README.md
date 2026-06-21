# CodeStory Codex Plugin

Codex-specific package metadata for CodeStory. The plugin exposes the existing
`codestory-grounding` workflow and launches the local read-only server with:

```powershell
codestory-cli serve --project <workspace> --stdio --refresh none
```

If the binary is missing, the launcher prints the setup action from
`scripts/install-codestory.ps1`. If retrieval is not strict `full`, read
`codestory://status` and follow its repair commands before trusting packet or
search.

Core indexing, runtime, retrieval, packet, and sidecar behavior remains in
`codestory-cli`.
