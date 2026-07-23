# CodeStory Grounding

Use CodeStory proactively for repository questions. Do not wait for the user to mention it by name.

Before making source claims, planning edits, choosing tests, or reviewing changes in this repository:

1. Call the CodeStory tool that matches the task and pass the repository's absolute root as `project`.
2. If it reports `preparing` or `updating`, retry that same tool after its reported delay. Do not poll status.
3. Use `status` or `codestory://status` only to diagnose a failed or unexpectedly slow call.
4. If MCP is missing, inspect source normally and report that CodeStory was unavailable for the task.
