'use strict';

const path = require('node:path');
const runtime = require('./codestory-runtime.cjs');
// The operator declares a fixed executor default, not a per-request override.
const declaredZshLogin = process.env.CODESTORY_SOURCE_CONTEXT_ZSH_LOGIN === '1';
const modelUsageFeedback = process.env.CODESTORY_MODEL_USAGE_FEEDBACK === '1';
let modelUsageHome = null;
if (modelUsageFeedback) {
  try {
    modelUsageHome = process.env.CODEX_HOME ?? path.join(require('node:os').homedir(), '.codex');
  } catch {
    // An unavailable host home also leaves the existing rewrite available.
  }
}

function rewrite(event, resolved) {
  if (event?.hook_event_name !== 'PreToolUse' || event?.tool_name !== 'Bash' || !resolved?.cli) return null;
  const input = event.tool_input;
  if (!input || typeof input !== 'object' || Array.isArray(input)) return null;
  const script = input.command;
  if (typeof script !== 'string' || !script || script.includes('\0') || Buffer.byteLength(script) > 16384) return null;
  if (script.includes('internal-source-context capture')) return null;
  if (['tty', 'interactive'].some(key => key in input && input[key] !== false)) return null;
  if ('shell' in input ? input.shell !== '/bin/zsh' : !declaredZshLogin) return null;
  if ('login' in input ? input.login !== true : !declaredZshLogin) return null;
  const absolute = value => typeof value === 'string' && path.isAbsolute(value)
    && !value.includes('\0') && !value.split('/').includes('..');
  if (!absolute(event.cwd) || !absolute(resolved.cli) || !absolute(resolved.shimDirectory)) return null;
  const project = path.normalize(event.cwd);
  const choices = ['cwd', 'workdir'].filter(key => key in input).map(key => input[key]);
  if (choices.some(value => !absolute(value))) return null;
  if (choices.length === 2 && path.normalize(choices[0]) !== path.normalize(choices[1])) return null;
  const cwd = path.normalize(input.workdir ?? input.cwd ?? project);
  const relative = path.relative(project, cwd);
  if (relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) return null;
  const payload = Buffer.from(JSON.stringify({ version: 1, project, cwd, command: script, shim_directory: resolved.shimDirectory }), 'utf8').toString('hex');
  const quote = value => "'" + value.replaceAll("'", "'\"'\"'") + "'";
  const command = `${quote(resolved.cli)} internal-source-context capture --payload ${payload}`;
  if (Buffer.byteLength(command) > 65536) return null;
  return { hookSpecificOutput: { hookEventName: 'PreToolUse', permissionDecision: 'allow', updatedInput: { ...input, command } } };
}

async function main() {
  let size = 0;
  const chunks = [];
  for await (const chunk of process.stdin) {
    size += chunk.length;
    if (size > 131072) return;
    chunks.push(chunk);
  }
  try {
    const event = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    const cli = runtime.resolveCli(process.argv.slice(2), runtime.pluginDataDir());
    const result = rewrite(event, { cli, shimDirectory: path.join(__dirname, 'source-context-bin') });
    if (result && modelUsageFeedback) {
      try {
        const context = require('./codestory-model-usage.cjs').observe(event, modelUsageHome);
        if (context) result.hookSpecificOutput.additionalContext = context;
      } catch {
        // Usage feedback must never suppress an otherwise valid capture rewrite.
      }
    }
    if (result) process.stdout.write(`${JSON.stringify(result)}\n`);
  } catch {
    // Optional observer fails open without changing the original tool input.
  }
}

if (require.main === module) main().catch(() => {});
module.exports = { rewrite };
