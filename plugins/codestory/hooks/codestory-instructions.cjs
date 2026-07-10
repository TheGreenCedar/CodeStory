const fs = require('fs');
const path = require('path');

const SKILL_PATH = path.join(__dirname, '..', 'skills', 'codestory-grounding', 'SKILL.md');
const MAX_PROMPT_CHARS = 600;
const MCP_RESOURCE_TEXT = 'If mcp__codestory tools are not initially visible and tool_search is available, query "codestory mcp ground status packet search", then use the loaded CodeStory MCP tools before manual source reads.';

const FALLBACK = `CODESTORY BACKGROUND GROUNDING ACTIVE

Before reading source files, making source claims, planning edits, choosing tests, or reviewing changes in a repository:

1. Confirm the target is a repository workspace before grounding it. In huge or mixed folders, stop CodeStory grounding if status, ready, or ground reports no repo, no supported files, or zero indexed files; do not inject or summarize empty ground output.
2. If the CodeStory MCP server is live, call status with the target repository's absolute path first and pass that same project to every CodeStory tool. ${MCP_RESOURCE_TEXT} Report hidden tool actions truthfully; do not synthesize a hook substitute for live MCP tools.
3. Use server_version, server_executable, allowed_surfaces, and retrieval_mode from status as runtime truth.
4. Use local graph surfaces only when their own allowed_surfaces entry allows them.
5. Use packet, search, or context confidently when that surface is allowed and retrieval_mode=full.
6. If MCP is unavailable, CodeStory grounding is unavailable in this host. Use ordinary source inspection, report the MCP blocker, and reserve CLI commands for maintainer/debug transcripts only.

Do this without waiting for the user to mention CodeStory.`;

function skillBody() {
  try {
    return fs.readFileSync(SKILL_PATH, 'utf8').replace(/^---[\s\S]*?---\s*/, '').trim();
  } catch (e) {
    return null;
  }
}

function compactPrompt(prompt) {
  const text = String(prompt || '').replace(/\s+/g, ' ').trim();
  if (text.length <= MAX_PROMPT_CHARS) return text;
  return `${text.slice(0, MAX_PROMPT_CHARS)}...`;
}

function eventHeader(event, input = {}) {
  if (event === 'UserPromptSubmit') {
    const prompt = compactPrompt(input.prompt);
    return [
      'CODESTORY REQUEST GROUNDING ACTIVE',
      '',
      'Use CodeStory before source files for this user prompt.',
      MCP_RESOURCE_TEXT,
      prompt ? `Prompt: ${prompt}` : 'Prompt: unavailable from hook input.',
      '',
    ].join('\n');
  }

  const source = input.source ? ` (${input.source})` : '';
  return [
    `CODESTORY SESSION GROUNDING ACTIVE${source}`,
    '',
    'Keep CodeStory ambient in this session. Before source reads, claims, edits, reviews, or test choices, check CodeStory status and use allowed grounding surfaces first.',
    MCP_RESOURCE_TEXT,
    '',
  ].join('\n');
}

function getCodeStoryInstructions(event = 'SessionStart', input = {}) {
  const body = skillBody();
  if (!body) return `${eventHeader(event, input)}${FALLBACK}`;

  return `${eventHeader(event, input)}CODESTORY BACKGROUND GROUNDING RULES

Use CodeStory proactively for repository grounding. Do not wait for the user to call it by name.
Before manually opening source files, first call status with the target repository's absolute path when MCP is live. Pass that same project to every CodeStory tool call; the server is multi-project and has no global workspace binding. When CodeStory is not initially model-visible, use host deferred discovery/tool_search to load the registered CodeStory MCP. When MCP is unavailable, CodeStory grounding is unavailable in this host; use ordinary source inspection and report the MCP blocker.
For broad user requests, prefer a packet tied to the user's actual question. For concrete symbols, files, or routes, use search/context/trail/snippet. For no request context, use a compact ground snapshot only after confirming the target repo is indexable.
Avoid no-op grounding context in huge or non-code folders.
When retrieval sidecars are full and allowed, use packet, search, and context confidently.
Use status recommended_next_calls as the setup path once a repository target is known: call MCP sidecar_setup with the same project and action=repair when recommended, then call project-scoped status again.

${body}`;
}

module.exports = {
  FALLBACK,
  compactPrompt,
  eventHeader,
  getCodeStoryInstructions,
};
