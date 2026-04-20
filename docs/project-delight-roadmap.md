# CodeStory Essence & Delight Roadmap

CodeStory is building a local-first, language-aware code understanding substrate that turns repositories into durable, queryable knowledge (symbols, edges, trails, snippets, and search projections), then exposes that intelligence through a practical CLI and serving interface so both humans and agents can reason about real codebases with less guesswork.

## High-impact changes (delight-focused, not bug fixes)

1. **Ship an “Agentic Ask” command that auto-plans retrieval before answering**
   - Add a first-class command (for example `codestory-cli ask`) that performs iterative context gathering across `search`, `symbol`, `trail`, `snippet`, and optional web/MCP tools before producing a final answer packet.
   - Why this is high impact: this removes manual query choreography for users and makes CodeStory feel like a collaborative investigator rather than a toolbox.
   - Research signal: Sourcegraph documents that proactive, iterative context gathering improves response quality and reduces user burden in coding assistants.

2. **Make retrieval explainable by default (`--why` mode + score breakdown)**
   - Add explainability output to `search` and `ground`: lexical/semantic/graph contributions, top matched evidence, and fallback reasons in a compact “why these results” section.
   - Why this is high impact: trust and debuggability become product features. Users will tune prompts/queries faster and keep confidence when retrieval quality dips.
   - Research signal: modern context systems emphasize multi-source retrieval/ranking; users need visibility into which source won and why.

3. **Add a polished “code navigation UX lane” (definition/references/symbol pane parity) in `explore` and `serve`**
   - Build a fast path for `go to definition`, `find references`, and repository symbol browsing as explicit operations in the explorer and HTTP endpoints.
   - Why this is high impact: navigation primitives are daily workflows; making them one-keystroke/one-call lowers friction and positions CodeStory as an always-on code map.
   - Research signal: GitHub’s code navigation and LSP both treat definition/reference workflows as foundational developer experience features.

4. **Productize MCP as a first-class integration surface (resources/prompts/tools, not only tools)**
   - Expand `serve --stdio` beyond basic tool exposure into richer MCP compatibility: explicit resources, templates, prompts, and safety metadata.
   - Why this is high impact: CodeStory can become the local “context backbone” for multiple AI clients (editors, agents, terminals) without custom glue.
   - Research signal: MCP spec centers standard access to resources/prompts/tools; broader MCP feature coverage increases interoperability and adoption.

5. **Introduce “delightful zero-setup semantic mode” with model bootstrap + profile wizard**
   - Today symbolic fallback is graceful; push further by offering a guided setup that can fetch/validate local embedding assets, benchmark profiles quickly, and choose a sane default for the machine.
   - Why this is high impact: it closes the gap between “it runs” and “it feels magical” for first-time users, especially on fresh environments.
   - Research signal: hybrid retrieval quality rises when multiple context channels (lexical + structure + semantic) are available and tuned.

## Small QoL changes

1. **`codestory-cli doctor`**: one command to report environment health, cache status, retrieval mode, model asset presence, and common remediation steps.
2. **Saved query presets**: lightweight named recipes (e.g., `impact:<symbol>`, `oncall:<error>`) to make repeated investigations instant.
3. **Result sharing artifacts**: one-flag output bundles (`--bundle`) that include markdown summary + machine JSON for team handoffs.
4. **Progressive onboarding output**: after first `index`, print a short “next 3 useful commands” tutorial tailored to repo size and retrieval availability.
5. **Search ergonomics**: add typo suggestions and optional query rewriting hints when confidence is low.

## Implementation snapshot

This roadmap is now represented in the CLI/runtime surface:

- `ask` builds a DB-first answer packet with retrieval trace, citations, graphs, optional local-agent synthesis via `--with-local-agent`, and shareable `--bundle` artifacts.
- `search --why` and `ground --why` add human-readable retrieval explanations; search JSON carries score breakdowns when runtime produces hybrid scored hits.
- `explore` JSON/Markdown includes definition plus incoming/outgoing reference navigation metadata.
- `serve` now exposes navigation HTTP routes (`/definition`, `/references`, `/symbols`) and `serve --stdio` publishes tools, resources, resource templates, and prompts.
- `doctor` reports project/cache/index/retrieval health, embedding-related environment settings, and next commands.

## External research references

- Sourcegraph, *Cody Context* docs: multi-source context retrieval (keyword search, search API, code graph) and context-window quality tradeoffs.
- Sourcegraph, *Code Graph* docs: graph structure (definitions/references/symbols) as core contextual signal.
- Sourcegraph, *Agentic Context Fetching* docs: proactive and iterative context gathering with tool use including MCP.
- GitHub docs, *Navigating code on GitHub*: first-class symbol browsing, go-to-definition, and find-references UX patterns.
- Microsoft, *Language Server Protocol*: standardized features for go-to-definition/find-references and broad editor interoperability.
- Model Context Protocol specification: standardized resources/prompts/tools and explicit safety/consent requirements.
- SQLite FTS5 docs: native ranking (`bm25`/`rank`) and snippet/highlight primitives useful for explainability and readable search hits.
