// Test-only executable model of the pre-router UserPromptSubmit policy: every
// prompt received the same grounding instruction and none were suppressed.
function emitGenericUserPromptPolicy() {
  return {
    hookSpecificOutput: {
      additionalContext: [
        'CODESTORY REQUEST GROUNDING ACTIVE',
        '',
        'Use CodeStory before source files for this user prompt.',
        'Call status with the target repository before manual source reads.',
      ].join('\n'),
    },
  };
}

module.exports = { emitGenericUserPromptPolicy };
