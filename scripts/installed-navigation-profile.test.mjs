import test from 'node:test';
import assert from 'node:assert/strict';
import { validateManifest, navigationCommand, navigationEnvironment } from './installed-navigation-profile.mjs';

const fixture = () => ({ schema_version: 1, profile: 'pilot', model: { name: 'gpt-5.6-terra', reasoning_effort: 'low' },
  repositories: [{ id: 'r', commit: 'a'.repeat(40), tree: 'b'.repeat(40), seed_clone: '/tmp/seed' }],
  tasks: [{ id: 't', repository_id: 'r', effect_mode: 'read_only', prompt: 'Inspect source.' }],
  arms: ['native', 'published'], repeats: 1,
  sessions: [{ sequence: 1, session_id: 't-native', task_id: 't', arm: 'native', repeat: 1 }, { sequence: 2, session_id: 't-published', task_id: 't', arm: 'published', repeat: 1 }] });

test('complete manifest accepts equal native and installed arms', () => assert.equal(validateManifest(fixture()).tasks.size, 1));
test('schedule and source mutations fail closed before execution', () => {
  for (const mutate of [
    m => m.sessions.pop(),
    m => { m.sessions[1].arm = 'native'; },
    m => { m.sessions[1].session_id = m.sessions[0].session_id; },
    m => { m.sessions[0].session_id = '../escape'; },
    m => { m.sessions[0].repeat = 2; },
    m => { m.sessions[0].sequence = 2; },
    m => { m.repositories[0].commit = 'main'; },
    m => { m.model.reasoning_effort = 'xhigh'; },
    m => { m.tasks[0].repository_id = 'missing'; },
    m => { m.profile = 'codestory-0176-navigation-maintenance'; },
  ]) { const manifest = fixture(); mutate(manifest); assert.throws(() => validateManifest(manifest)); }
});
test('same normal sandbox and low reasoning without approval override or tool restrictions', () => {
  const { args } = navigationCommand('codex', '/checkout', '/output/answer.md');
  assert.deepEqual(args, ['exec', '--model', 'gpt-5.6-terra', '--config', 'model_reasoning_effort="low"', '--sandbox', 'workspace-write', '--cd', '/checkout', '--json', '--output-last-message', '/output/answer.md', '-']);
});
test('runtime and host overrides cannot escape the isolated session', () => {
  const env = navigationEnvironment({ PATH: '/bin', HOME: '/real', CODEX_HOME: '/real/.codex', CODESTORY_CLI: '/wrong/binary', CODESTORY_PLUGIN_DATA: '/shared', CODESTORY_EMBED_QUALIFICATION_DIR: '/shared', OPENAI_API_KEY: 'never-retain', PLUGIN_DATA: '/shared' }, '/isolated', 'nonce');
  assert.equal(env.HOME, '/isolated/home');
  assert.equal(env.CODEX_HOME, '/isolated/codex');
  assert.equal(env.CODESTORY_EMBED_QUALIFICATION_DIR, '/isolated/qualification');
  assert.equal(env.PATH, '/bin');
  for (const key of ['CODESTORY_CLI', 'CODESTORY_PLUGIN_DATA', 'OPENAI_API_KEY', 'PLUGIN_DATA']) assert.equal(env[key], undefined);
});

test('ordinary launcher notifications are retained without consuming a response', async () => {
  const { createSequencedStdioSession } = await import('./codestory-agent-ab-benchmark.mjs');
  const notifications = [];
  const script = `process.stdin.on('data', b => { const request=JSON.parse(String(b)); process.stdout.write(JSON.stringify({jsonrpc:'2.0',method:'notifications/tools/list_changed'})+'\\n'); process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result:{ok:true}})+'\\n'); }); process.stdin.on('end',()=>process.exit(0));`;
  const channel = createSequencedStdioSession(process.execPath, ['-e', script], { timeoutMs: 5000, onNotification: value => notifications.push(value) });
  try {
    assert.deepEqual((await channel.request({jsonrpc:'2.0',id:1,method:'tools/list'})).result, {ok:true});
    assert.equal(notifications[0].method, 'notifications/tools/list_changed');
    await channel.close();
  } finally { await channel.stop(); }
});
