const MCP_RESOURCE_TEXT = 'If CodeStory MCP tools are hidden and tool_search is available, query "codestory mcp ground packet search symbol", then use the loaded tools.';

function normalizePrompt(prompt) {
  return String(prompt || '').replace(/\s+/g, ' ').trim();
}

function routeForPrompt(prompt) {
  const normalized = normalizePrompt(prompt);
  const text = normalized.toLowerCase();
  if (!text) return null;
  if (/^(?:thanks?|thank you|ok(?:ay)?|got it|sounds good|never ?mind)[.!?]*$/u.test(text)) {
    return null;
  }
  const explicitRepositoryIntent = /\b(?:codestory|repo(?:sitory)?|codebase|source code|source files?|pull request|worktree|mcp|grounding|git|commit|rebase|cherry-pick|merge conflict|current branch|this branch|feature branch|branch (?:name|head)|code review|changed files?|diff|build (?:the )?(?:code|repo|repository|project)|unit tests?|integration tests?|test suite|test files?|callers?|callees?|crates?)\b|\b(?:review|fix|update|merge)\b[^.!?]*\bpr\b|\bpr\b[^.!?]*\b(?:review|changes?|diff)\b|\b(?:cargo|npm|pnpm|yarn|go) test\b|\bpytest\b|\bdotnet test\b|\bwhere is [a-z_$][\w$]* (?:defined|implemented)\b|(?:^|[\s"'`])(?:src|crates|plugins|scripts|tests?|docs?)[/\\][\w./\\-]+|(?:^|[\s"'`])[\w./\\-]+\.(?:rs|js|cjs|mjs|ts|tsx|jsx|py|go|cs|cpp|c|h|toml|json|ya?ml|md)(?:\b|$)/u;
  const codeShapedIdentifier = /\b[a-z][a-z\d]*_[a-z\d_]+\b|\b[a-z]+[A-Z][A-Za-z\d]*\b/u.test(normalized);
  const identifierIntent = codeShapedIdentifier && /\b(?:fix|refactor|review|change|edit|debug|implement|test|define|defined|definition|trace|trail|caller|callee|invokes?|called by)\b|\b(?:where is|who calls)\b/u.test(text);
  const qualifiedIdentifier = /[A-Za-z_$][\w$]*::[A-Za-z_$][\w$]*/u.test(normalized);
  const codeChangeIntent = /\b(?:fix|refactor|review|change|edit|debug|implement|test)\b[^.!?]*\b(?:function|method|class|module|runtime|compiler|plugin|hook|symbol)s?\b/u.test(text);
  const repositoryIntent = explicitRepositoryIntent.test(text) || identifierIntent || qualifiedIdentifier || codeChangeIntent;
  if (
    !repositoryIntent
    && /\b(?:create|open|close|assign|organize|triage|label|comment on|update|status of)\b[^.!?]*\b(?:issue|initiative|epic|project (?:board|item))\b/u.test(text)
  ) return null;
  if (!repositoryIntent) return null;

  if (/\b(?:caller|callee|call path|call flow|trace|trail|invokes?|called by)\b|\bwho calls\b/u.test(text)) {
    return 'Call flow: use symbol, then callers/callees or trace/trail; use snippet only after the graph selects a concrete target.';
  }
  if (
    /\b(?:diff|changed files?|edit|refactor|bug|fix|impact|tests? should|affected|rebase|cherry-pick|merge)\b/u.test(text)
    || /\breview\b[^.!?]*\b(?:diff|changes?|pull request)\b/u.test(text)
    || /\b(?:review\b[^.!?]*\bpr|pr\b[^.!?]*\breview)\b/u.test(text)
  ) {
    return 'Review/change impact: use affected only with explicit git-changed paths, then inspect relevant symbol or trace evidence; affected is planning evidence, not proof.';
  }
  if (/\b(?:where is|defined|definition|owns?|ownership|symbol|class|function|method)\b/u.test(text)) {
    return 'Symbol ownership: use symbol, then definition; add callers/callees only when the question asks for flow.';
  }
  if (/\b(?:whole|entire|broad|architecture|subsystem|data flow|how does|how do|explain|codebase review)\b/u.test(text)) {
    return 'Broad question: call packet directly; if it is still preparing, use ground plus focused symbol/trace evidence and retry packet after its reported delay.';
  }
  return 'Repository orientation: use ground; use files only for language, role, path, or coverage questions.';
}

function selectedPlan(route, prompt) {
  const label = route.split(':', 1)[0];
  const blockedDeep = /\b(?:packet(?:\s*\/\s*|\s+and\s+)search|packet|search)\b[^.!?]*\bblocked\b|\bblocked\b[^.!?]*\b(?:packet|search)\b/u.test(
    normalizePrompt(prompt).toLowerCase(),
  );
  const plans = {
    'Repository orientation': ['orientation', ['ground', 'files']],
    'Symbol ownership': ['symbol_ownership', ['symbol', 'definition']],
    'Call flow': ['call_flow', ['symbol', 'callers', 'callees', 'trace']],
    'Review/change impact': ['change_impact', ['affected', 'symbol', 'trace']],
    'Broad question': blockedDeep
      ? ['broad_question', ['ground', 'symbol', 'trace']]
      : ['broad_question', ['packet']],
  };
  const [category, tools] = plans[label];
  return `Plan: category=${category}; tools=${tools.join(',')}`;
}

function eventHeader(event, input = {}) {
  if (event === 'UserPromptSubmit') {
    const route = routeForPrompt(input.prompt);
    if (!route) return '';
    return [
      'CODESTORY REQUEST ROUTING ACTIVE',
      '',
      `Route: ${route}`,
      selectedPlan(route, input.prompt),
      '',
    ].join('\n');
  }

  return [
    'CODESTORY SESSION ROUTING ACTIVE',
    '',
    'For repository work, use the codestory-grounding skill and call the intended tool with the explicit project.',
    'Task router: orientation -> ground/files; symbol -> symbol/definition; flow -> callers/callees/trace; review/change -> affected with explicit paths plus focused graph evidence; broad -> packet.',
    '',
  ].join('\n');
}

module.exports = {
  MCP_RESOURCE_TEXT,
  eventHeader,
  routeForPrompt,
};
