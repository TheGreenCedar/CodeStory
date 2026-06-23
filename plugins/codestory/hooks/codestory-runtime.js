const isCopilot = Boolean(process.env.COPILOT_PLUGIN_DATA);
const isCodex = !isCopilot && Boolean(process.env.PLUGIN_DATA);

function writeHookOutput(event, context) {
  if (isCopilot) {
    process.stdout.write(JSON.stringify({ additionalContext: context }));
    return;
  }

  if (isCodex) {
    process.stdout.write(JSON.stringify({
      systemMessage: 'CODESTORY:BACKGROUND',
      hookSpecificOutput: {
        hookEventName: event,
        additionalContext: context,
      },
    }));
    return;
  }

  process.stdout.write(context);
}

module.exports = {
  writeHookOutput,
};
