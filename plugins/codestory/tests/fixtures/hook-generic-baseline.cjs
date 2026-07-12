#!/usr/bin/env node

// Test-only executable reproduction of ca351218^ first-emission
// UserPromptSubmit output. Historical hook-state deduplication is out of scope.
const MAX_PROMPT_CHARS = 600;
const MCP_RESOURCE_TEXT = 'If mcp__codestory tools are not initially visible and tool_search is available, query "codestory mcp ground status packet search", then use the loaded CodeStory MCP tools before manual source reads.';

function compactPrompt(prompt) {
  const text = String(prompt || '').replace(/\s+/g, ' ').trim();
  return text.length <= MAX_PROMPT_CHARS ? text : `${text.slice(0, MAX_PROMPT_CHARS)}...`;
}

let input = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => { input += chunk; });
process.stdin.on('end', () => {
  let hookInput = {};
  try {
    hookInput = JSON.parse(input.replace(/^\uFEFF/, '') || '{}');
  } catch {
    // The former hook treated malformed input as an empty event.
  }
  const event = hookInput.hook_event_name || 'UserPromptSubmit';
  const prompt = compactPrompt(hookInput.prompt);
  const cwd = hookInput.cwd || process.cwd();
  const header = [
    'CODESTORY REQUEST GROUNDING ACTIVE',
    '',
    'Use CodeStory before source files for this user prompt.',
    MCP_RESOURCE_TEXT,
    prompt ? `Prompt: ${prompt}` : 'Prompt: unavailable from hook input.',
  ].join('\n');
  const mcpInstructions = [
    'CodeStory MCP startup path:',
    '1. Use live CodeStory MCP before manual source reads for repository work.',
    `2. If mcp__codestory tools are visible, call status with project=${JSON.stringify(cwd)}, then pass that same project to every CodeStory tool call.`,
    '3. If mcp__codestory is not visible and tool_search is available, query "codestory mcp ground status packet search", then use the loaded CodeStory MCP tools.',
    '4. The MCP is multi-project and request-scoped. Never infer workspace from another thread or a global active-state file.',
    '5. Do not treat hook text as grounding evidence; only live MCP results or verified source reads count.',
  ].join('\n');
  const context = `${header}\n\n${mcpInstructions}`;
  process.stdout.write(JSON.stringify({
    systemMessage: 'CODESTORY:BACKGROUND',
    hookSpecificOutput: {
      hookEventName: event,
      additionalContext: context.length <= 1200
        ? context
        : `${context.slice(0, 1200)}\n\n... CodeStory hook output truncated.`,
    },
  }));
});
