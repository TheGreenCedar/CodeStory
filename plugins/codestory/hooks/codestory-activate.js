#!/usr/bin/env node

const { getCodeStoryInstructions } = require('./codestory-instructions');
const { writeHookOutput } = require('./codestory-runtime');

try {
  writeHookOutput('SessionStart', getCodeStoryInstructions());
} catch (e) {
  // Best effort only. A hook failure must not block the agent session.
}
