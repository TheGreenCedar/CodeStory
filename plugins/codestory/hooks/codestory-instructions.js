const fs = require('fs');
const path = require('path');

const SKILL_PATH = path.join(__dirname, '..', 'skills', 'codestory-grounding', 'SKILL.md');

const FALLBACK = `CODESTORY BACKGROUND GROUNDING ACTIVE

Before making source claims, planning edits, choosing tests, or reviewing changes in a repository:

1. Confirm the target is a repository workspace before grounding it. In huge or mixed folders, stop CodeStory grounding if status, ready, or ground reports no repo, no supported files, or zero indexed files; do not inject or summarize empty ground output.
2. If the CodeStory MCP server is live, read codestory://status first.
3. Use server_version, server_executable, allowed_surfaces, and retrieval_mode from status as runtime truth.
4. Use local graph surfaces only when their own allowed_surfaces entry allows them.
5. Use packet, search, or context confidently when that surface is allowed and retrieval_mode=full.
6. If MCP is missing, use codestory-cli ready --goal local --repair as an incremental-by-default setup/repair fallback; reserve full rebuilds for explicit stale, corrupt, schema, or root-change cases.

Do this without waiting for the user to mention CodeStory.`;

function skillBody() {
  try {
    return fs.readFileSync(SKILL_PATH, 'utf8').replace(/^---[\s\S]*?---\s*/, '').trim();
  } catch (e) {
    return null;
  }
}

function getCodeStoryInstructions() {
  const body = skillBody();
  if (!body) return FALLBACK;

  return `CODESTORY BACKGROUND GROUNDING ACTIVE

Use CodeStory proactively for repository grounding. Do not wait for the user to call it by name.
First confirm the target is a repository with supported files; avoid no-op grounding context in huge or non-code folders.
When retrieval sidecars are full and allowed, use packet, search, and context confidently.
Use incremental ready repair as the default setup path once a repository target is known.

${body}`;
}

module.exports = {
  FALLBACK,
  getCodeStoryInstructions,
};
