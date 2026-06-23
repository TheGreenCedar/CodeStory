const fs = require('fs');
const path = require('path');

const SKILL_PATH = path.join(__dirname, '..', 'skills', 'codestory-grounding', 'SKILL.md');

const FALLBACK = `CODESTORY BACKGROUND GROUNDING ACTIVE

Before making source claims, planning edits, choosing tests, or reviewing changes in a repository:

1. If the CodeStory MCP server is live, read codestory://status first.
2. Use server_version, server_executable, allowed_surfaces, and retrieval_mode from status as runtime truth.
3. Use local graph surfaces only when their own allowed_surfaces entry allows them.
4. Use packet, search, or context only when that surface is allowed and retrieval_mode=full.
5. If MCP is missing, use codestory-cli ready or doctor as a repair/debug fallback.

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

${body}`;
}

module.exports = {
  FALLBACK,
  getCodeStoryInstructions,
};
