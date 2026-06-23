const fs = require('fs');
const path = require('path');

const isCopilot = Boolean(process.env.COPILOT_PLUGIN_DATA);
const isCodex = !isCopilot && Boolean(process.env.PLUGIN_DATA);

const STATE_FILE = '.codestory-active';

function pluginDataDir() {
  if (isCodex) return process.env.PLUGIN_DATA;
  if (isCopilot) return process.env.COPILOT_PLUGIN_DATA;
  return null;
}

function rememberActiveState(state) {
  const stateDir = pluginDataDir();
  if (!stateDir) return;

  try {
    fs.mkdirSync(stateDir, { recursive: true });
    fs.writeFileSync(path.join(stateDir, STATE_FILE), JSON.stringify({
      ...state,
      updatedAt: new Date().toISOString(),
    }));
  } catch (e) {
    // Best effort only. Hook state must not block the host session.
  }
}

function writeHookOutput(event, context) {
  if (isCopilot) {
    process.stdout.write(JSON.stringify({ additionalContext: context }));
    return;
  }

  if (isCodex) {
    const output = {
      systemMessage: 'CODESTORY:BACKGROUND',
    };
    if (context) {
      output.hookSpecificOutput = {
        hookEventName: event,
        additionalContext: context,
      };
    }
    process.stdout.write(JSON.stringify(output));
    return;
  }

  process.stdout.write(context);
}

module.exports = {
  rememberActiveState,
  writeHookOutput,
};
