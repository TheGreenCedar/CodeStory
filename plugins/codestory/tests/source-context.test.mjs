import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { tmpdir } from 'node:os';
import fs from 'node:fs';
import { spawnSync } from 'node:child_process';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const require = createRequire(import.meta.url);
const { rewrite } = require(join(root, 'hooks/codestory-source-context.cjs'));
const resolved = { cli: '/owned/cli', shimDirectory: '/owned/plugin/hooks/source-context-bin' };
const event = () => ({
  hook_event_name: 'PreToolUse', tool_name: 'Bash', cwd: '/owned/project',
  tool_input: { command: "rg -n 'value' source.py; printf 'unchanged'", workdir: '/owned/project', shell: '/bin/zsh', login: true, tty: false, sandbox_permissions: 'use_default', prefix_rule: ['rg'], yield_time_ms: 1000, max_output_chars: 10000 },
});

test('observer preserves security arguments and carries bounded unchanged script without tickets', () => {
  const original = event(); const result = rewrite(original, resolved);
  assert.ok(result, 'supported ordinary input must reach capture');
  const output = result.hookSpecificOutput;
  assert.equal(output.hookEventName, 'PreToolUse');
  assert.equal(output.permissionDecision, 'allow');
  const { command, ...retained } = output.updatedInput;
  const { command: script, ...expected } = original.tool_input;
  assert.deepEqual(retained, expected);
  assert.match(command, /internal-source-context capture --payload [0-9a-f]+$/u);
  assert.ok(Buffer.byteLength(command) <= 65536);
  const hex = command.match(/--payload ([0-9a-f]+)$/u)[1];
  const payload = JSON.parse(Buffer.from(hex, 'hex').toString('utf8'));
  assert.equal(payload.version, 1); assert.equal(payload.project, original.cwd);
  assert.equal(payload.cwd, original.tool_input.workdir);
  assert.equal(payload.command, script); assert.equal(payload.shim_directory, resolved.shimDirectory);
  assert.deepEqual(Object.keys(payload).sort(), ['command', 'cwd', 'project', 'shim_directory', 'version']);
});

test('observer does not parse or steer the original compound script', () => {
  const original = event();
  original.tool_input.command = "value=$(printf 'a;b'); printf '%s\\n' \"$value\"\ncat <<'END'\ntext ; | $(unexecuted)\nEND";
  const result = rewrite(original, resolved); assert.ok(result);
  const hex = result.hookSpecificOutput.updatedInput.command.match(/--payload ([0-9a-f]+)$/u)[1];
  assert.equal(JSON.parse(Buffer.from(hex, 'hex')).command, original.tool_input.command);
});

test('unsupported host shape, recursion, ambiguous scope and missing verified CLI stay unchanged', () => {
  const cases = [
    e => { e.tool_name = 'Read'; }, e => { e.hook_event_name = 'SessionStart'; },
    e => { e.tool_input.tty = true; }, e => { e.tool_input.interactive = true; },
    e => { e.tool_input.shell = '/bin/bash'; }, e => { e.tool_input.login = false; },
    e => { e.cwd = 'relative'; }, e => { e.tool_input.workdir = '/outside'; },
    e => { e.tool_input.cwd = '/owned/project/other'; },
    e => { e.tool_input.command = "'/owned/cli' internal-source-context capture --payload 00"; },
    e => { e.tool_input.command = 'bad\0script'; },
  ];
  for (const change of cases) { const e=event(); change(e); assert.equal(rewrite(e,resolved),null); }
  assert.equal(rewrite(event(), {...resolved, cli:null}),null);
});

test('observer declines source script or encoded command over fixed byte bounds', () => {
  const e=event(); e.tool_input.command='雪'.repeat(6000); assert.equal(rewrite(e,resolved),null);
  const huge={...resolved,cli:'/'+ 'x'.repeat(65536)};assert.equal(rewrite(event(),huge),null);
});

test('missing executor metadata needs a startup declaration and conflicting fields still decline', () => {
  for (const [startup, later, expected] of [[null, '1', false], ['1', '0', true], ['0', '1', false]]) {
    const env = { ...process.env };
    delete env.CODESTORY_SOURCE_CONTEXT_ZSH_LOGIN;
    if (startup !== null) env.CODESTORY_SOURCE_CONTEXT_ZSH_LOGIN = startup;
    const script = `
      const {rewrite}=require(${JSON.stringify(join(root, 'hooks/codestory-source-context.cjs'))});
      process.env.CODESTORY_SOURCE_CONTEXT_ZSH_LOGIN=${JSON.stringify(later)};
      const event=${JSON.stringify(event())};
      delete event.tool_input.shell;delete event.tool_input.login;
      const resolved=${JSON.stringify(resolved)};
      const missing=Boolean(rewrite(event,resolved));
      const wrongShell=Boolean(rewrite({...event,tool_input:{...event.tool_input,shell:'/bin/bash'}},resolved));
      const wrongLogin=Boolean(rewrite({...event,tool_input:{...event.tool_input,login:false}},resolved));
      process.stdout.write(JSON.stringify([missing,wrongShell,wrongLogin]));
    `;
    const output = spawnSync(process.execPath, ['-e', script], { env, encoding: 'utf8', timeout: 5000 });
    assert.equal(output.status, 0, output.stderr);
    assert.deepEqual(JSON.parse(output.stdout), [expected, false, false]);
  }
});

test('actual host observer resolves existing CLI without executing it or writing event files', () => {
  const temp=fs.mkdtempSync(join(tmpdir(),'codestory-source-observer-'));
  try {
    const cli=join(temp,'cli'); const marker=join(temp,'must-not-execute');
    fs.writeFileSync(cli,`#!/bin/sh\nprintf bad > '${marker}'\n`,{mode:0o755});
    const e=event();e.cwd=temp;e.tool_input.workdir=temp;
    const before=fs.readdirSync(temp).sort();
    const output=spawnSync(process.execPath,[join(root,'hooks/codestory-source-context.cjs'),'--cli',cli],{
      input:JSON.stringify(e),encoding:'utf8',timeout:5000,
      env:{...process.env,PLUGIN_DATA:temp,COPILOT_PLUGIN_DATA:temp,CODESTORY_PLUGIN_DATA:temp,CODEX_HOME:temp},
    });
    assert.equal(output.status,0,output.stderr);assert.ok(output.stdout.trim());
    assert.equal(JSON.parse(output.stdout).hookSpecificOutput.hookEventName,'PreToolUse');
    assert.equal(fs.existsSync(marker),false);assert.deepEqual(fs.readdirSync(temp).sort(),before);
  } finally { fs.rmSync(temp,{recursive:true,force:true}); }
});

test('native and legacy Codex registration preserve interface and isolate other hosts', () => {
  const read = p => JSON.parse(fs.readFileSync(join(root,p),'utf8'));
  const manifest=read('plugin.json');const legacy=read('.codex-plugin/plugin.json');
  assert.equal(manifest.extensions['com.openai'].hooks,'./hooks/codex-source-context-hooks.json');
  assert.deepEqual(manifest.extensions['com.openai'].interface,legacy.interface);
  assert.equal(legacy.hooks,'./hooks/codex-source-context-hooks.json');
  assert.equal(legacy.skills,'./skills/');assert.equal(legacy.mcpServers,'./mcp.codex.json');
  const config=read('hooks/codex-source-context-hooks.json');
  assert.equal(config.hooks.PreToolUse[0].matcher,'^Bash$');
  assert.equal(config.hooks.PreToolUse[0].hooks[0].command,'node "${PLUGIN_ROOT}/hooks/codestory-source-context.cjs"');
  assert.deepEqual(read('hooks/claude-codex-hooks.json').hooks,{});
  assert.equal(read('.claude-plugin/plugin.json').hooks,'./hooks/claude-codex-hooks.json');
  assert.equal(read('.cursor-plugin/plugin.json').hooks,'./hooks/cursor-hooks.json');
});
