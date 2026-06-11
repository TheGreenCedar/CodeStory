# CodeStory Product Direction

This page is product direction, not proof that every idea below is fully done.
For measured behavior, use the benchmark docs. For architecture truth, use the
architecture docs.

CodeStory is meant to be the local codebase browser an agent uses before it
starts manual file inspection: index the repo, keep the evidence local, explain
retrieval, and hand back cited context.

## Now

These capabilities are represented in the current CLI/runtime surface:

- `doctor` reports project, cache, index, retrieval, managed embedding setup, and
  next-command health.
- `index` builds graph state, snapshots, lexical search state, graph-native
  symbol docs, component reports, and selected dense anchors in the local cache.
- `ground --why` gives broad repo orientation with retrieval and coverage notes.
- `report` emits a derived Markdown repo report or JSON graph export from the
  current SQLite store, including hotspots, entry points, bridge nodes,
  suggested follow-up queries, generation metadata, source locations, and
  confidence/certainty when available.
- `search --why` exposes candidate results and retrieval explanations.
- `symbol`, `trail`, `snippet`, and `explore` support focused navigation around
  concrete targets.
- `context` builds a DB-first evidence bundle around one concrete target.
- `drill` and `drill-suite` turn a realistic codebase question into an evidence
  packet, source-truth checklist, optional claim ledger, expected-file recall,
  and separate mechanical versus answer-quality verdicts.
- `serve --stdio` exposes the read surface for repeated agent queries.

## Next

The highest-value improvements are still about making the evidence loop easier
to trust and harder to misuse:

1. **Make reliability and readiness hard to misread**
   - Keep tightening `doctor` so it separates local cache/navigation readiness
     from agent-facing packet/search sidecar readiness.
   - Keep recovery commands focused on the failing layer: stale local index,
     managed embedding assets, sidecar manifest, sidecar process, or backend
     contract drift.
   - Preserve fail-closed behavior for sidecar packet/search and make stale
     index repair obvious before answer-quality work starts.

2. **Make answer-quality gates harder to bypass**
   - Treat source-truth correction counts as product evidence, not test noise.
   - Keep `drill-suite --ledger` as the repeatable loop for proving whether a
     CodeStory-only draft survived focused source reads.
   - A green index/build is not enough; final-answer status must stay pending or
     degraded until claim classifications prove no misleading or unsupported
     claims remain.

3. **Make target-context packets sharper**
   - Improve `context` so it gathers the right neighborhood around one target
     with fewer manual hops.
   - Keep it target-first; broad open-ended questions belong in `packet` or a
     `drill`/`drill-suite` run that records source-truth verification.

4. **Make retrieval explanations more useful**
   - Keep improving `--why` output for lexical, semantic, graph, fallback, and
     freshness signals.
   - The goal is to show why a result appeared and when not to trust it.

5. **Improve repository navigation**
   - Keep hardening `explore`, definition, references, symbol browsing, trails,
     and snippets before adding a separate web UI.
   - A new surface should be added only when it solves a workflow that the
     current surfaces do not.

6. **Polish setup without blurring readiness tracks**
   - Managed embeddings, profile selection, sidecar setup, and fail-closed
     diagnostics should make first use clear.
   - If the model path, backend, manifest, sidecar mode, or doc shape is stale,
     `doctor` should say so plainly.

## Later

- Saved query presets for repeated investigations.
- Sharper shareable result bundles that pair Markdown summaries with machine
  JSON across targeted workflows, beyond the current repo-level `report`
  artifact.
- Better typo and low-confidence query suggestions.
- A separate web UI only after the browser surface gate has current evidence.

## Research References

- Sourcegraph, *Cody Context* docs: multi-source context retrieval and context-window tradeoffs.
- Sourcegraph, *Code Graph* docs: graph structure as contextual signal.
- Sourcegraph, *Agentic Context Fetching* docs: proactive and iterative context gathering.
- GitHub docs, *Navigating code on GitHub*: symbol browsing, go-to-definition, and find-references patterns.
- Microsoft, *Language Server Protocol*: standard definition/reference workflows.
- Model Context Protocol specification: resources, prompts, tools, and safety/consent requirements.
- SQLite FTS5 docs: ranking and snippet/highlight primitives.
