const fs = require('fs');
const path = require('path');

const SKILL_PATH = path.join(__dirname, '..', 'skills', 'codestory-grounding', 'SKILL.md');
const MAX_PROMPT_CHARS = 600;

const FALLBACK = `CODESTORY BACKGROUND GROUNDING ACTIVE

Before reading source files, making source claims, planning edits, choosing tests, or reviewing changes in a repository:

1. Confirm the target is a repository workspace before grounding it. In huge or mixed folders, stop CodeStory grounding if status, ready, or ground reports no repo, no supported files, or zero indexed files; do not inject or summarize empty ground output.
2. If the CodeStory MCP server is live, read codestory://status first. If MCP is configured but resources are not model-visible, use the hook bridge when present and reload the host/plugin only to expose live MCP tools.
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
      prompt ? `Prompt: ${prompt}` : 'Prompt: unavailable from hook input.',
      '',
    ].join('\n');
  }

  const source = input.source ? ` (${input.source})` : '';
  return [
    `CODESTORY SESSION GROUNDING ACTIVE${source}`,
    '',
    'Keep CodeStory ambient in this session. Before source reads, claims, edits, reviews, or test choices, check CodeStory status and use allowed grounding surfaces first.',
    '',
  ].join('\n');
}

function getCodeStoryInstructions(event = 'SessionStart', input = {}) {
  const body = skillBody();
  if (!body) return `${eventHeader(event, input)}${FALLBACK}`;

  return `${eventHeader(event, input)}CODESTORY BACKGROUND GROUNDING RULES

Use CodeStory proactively for repository grounding. Do not wait for the user to call it by name.
Before manually opening source files, first read codestory://status when MCP is live. When MCP is configured but resources are not model-visible, use hook-bridged status when present and reload the host/plugin only to expose live MCP tools. When MCP is unavailable, CodeStory grounding is unavailable in this host; use ordinary source inspection and report the MCP blocker.
For broad user requests, prefer a packet tied to the user's actual question. For concrete symbols, files, or routes, use search/context/trail/snippet. For no request context, use a compact ground snapshot only after confirming the target repo is indexable.
Avoid no-op grounding context in huge or non-code folders.
When retrieval sidecars are full and allowed, use packet, search, and context confidently.
Use status recommended_next_calls as the setup path once a repository target is known: call MCP repair_all when recommended, then reread codestory://status.

${body}`;
}

module.exports = {
  FALLBACK,
  compactPrompt,
  eventHeader,
  getCodeStoryInstructions,
};
