# Research Synthesis: Eliminate user and AI friction in CodeStory skill-first repo explanation across Sourcetrail, rootandruntime, and CodeStory.

## Project Essence
- CodeStory is being tested as a skill-first repository browser/explainer for both humans and agents. The important product contract is not only correct indexing; it is a smooth loop from `doctor` through grounded evidence and repo explanation without hidden semantic failure, misleading health, or manual command archaeology.

## High-Impact Findings
- Semantic indexing must be robust by default. Sourcetrail exposed an oversized embedding-input failure that batching did not fix.
- Health reporting must be agent-legible. A partial semantic cache that looks like `semantic ok` causes broad `ask` to drift and wastes operator time.
- Query surfaces must agree. Divergence between `trail --query`/DSL trail and `symbol --query`/DSL symbol makes agents second-guess the CLI and adds manual verification hops.
- Output needs to explain its bounds. `ask` mode and `snippet --context` truncation were easy to misread in the manual tests.
- Skill guidance is part of the product. Agents need explicit semantic setup, semantic-health gates, and lexical/repo-text fallback recipes.

## Quality-Gap Translation
- Keep `quality-gaps.md` aligned with the current synthesis.

## Confidence And Gaps
- Confidence is high for the seeded issues because they recur across three manual sessions. The remaining uncertainty is whether the new full three-repo harness reaches zero after the code fixes and whether two fresh rounds reveal new high-impact friction.
