import test from 'node:test';
import assert from 'node:assert/strict';
import { validateManifest, navigationCommand, navigationEnvironment } from '../installed-navigation-profile.mjs';

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
  const { createSequencedStdioSession } = await import('../codestory-agent-ab-benchmark.mjs');
  const notifications = [];
  const script = `process.stdin.on('data', b => { const request=JSON.parse(String(b)); process.stdout.write(JSON.stringify({jsonrpc:'2.0',method:'notifications/tools/list_changed'})+'\\n'); process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:request.id,result:{ok:true}})+'\\n'); }); process.stdin.on('end',()=>process.exit(0));`;
  const channel = createSequencedStdioSession(process.execPath, ['-e', script], { timeoutMs: 5000, onNotification: value => notifications.push(value) });
  try {
    assert.deepEqual((await channel.request({jsonrpc:'2.0',id:1,method:'tools/list'})).result, {ok:true});
    assert.equal(notifications[0].method, 'notifications/tools/list_changed');
    await channel.close();
  } finally { await channel.stop(); }
});

test('typed canary rejects diagnostic substitutes, mixed source, and runtime drift', async () => {
  const { validateRuntime, validateSourceRead, validateInventory, validateTemplateConfig } = await import('../installed-navigation-profile.mjs');
  const arm = { marketplace_name: 'fixture', version: '0.17.5', schema_version: 2, runtime_source: 'managed', runtime_sha256: 'c'.repeat(64) };
  const result = { _meta: { codestory_publication: { schema_version: 2, served_from: 'complete_publication', core_publication: { generation_id: 'g' }, contract_runtime: { cli_sha256: arm.runtime_sha256, cli_source: 'managed', cli_version: arm.version, plugin_cli_version: arm.version, plugin_version: arm.version, pinned_pair_matches: true, known_override_skew_channel: false } } }, structuredContent: { ranges: [{ path: '/repo/index.js', start_line: 1, end_line: 1, snippet_truncated: false, snippet: 'export function navigationCanary() { return "BEFORE_REFRESH"; }' }] } };
  validateRuntime(result, arm); validateSourceRead(result, '/repo', 'BEFORE_REFRESH');
  for (const mutate of [
    r => { r.structuredContent.ranges = []; r.diagnostic = JSON.stringify(result); },
    r => { r.structuredContent.ranges[0].path = '/wrong/index.js'; },
    r => { r.structuredContent.ranges[0].snippet += 'AFTER_REFRESH'; },
    r => { r.structuredContent.ranges[0].snippet_truncated = true; },
  ]) { const bad = structuredClone(result); mutate(bad); assert.throws(() => validateSourceRead(bad, '/repo', 'BEFORE_REFRESH')); }
  for (const field of ['cli_sha256', 'cli_source', 'cli_version', 'plugin_version', 'pinned_pair_matches', 'known_override_skew_channel']) {
    const bad = structuredClone(result); bad._meta.codestory_publication.contract_runtime[field] = 'wrong'; bad.diagnostic = JSON.stringify(result);
    assert.throws(() => validateRuntime(bad, arm));
  }
  const plugin = { name: 'codestory', marketplaceName: 'fixture', pluginId: 'codestory@fixture', version: arm.version, installed: true, enabled: true };
  validateInventory([plugin], 'published', arm);
  for (const field of Object.keys(plugin)) assert.throws(() => validateInventory([{...plugin, [field]: 'wrong'}], 'published', arm));
  assert.throws(() => validateTemplateConfig('[mcp_servers.injected]\ncommand = "fake"', 'published', arm));
  assert.throws(() => validateTemplateConfig('[plugins."codestory@fixture"]\nenabled = false', 'published', arm));
});

test('arm names cannot escape preflight output', () => {
  const manifest = fixture(); manifest.arms[1] = 'x/../../escape'; manifest.sessions[1].arm = manifest.arms[1];
  assert.throws(() => validateManifest(manifest));
});

async function accountingFixture(t, option) {
  const fs = await import('node:fs/promises'); const os = await import('node:os'); const path = await import('node:path');
  const { createHash } = await import('node:crypto');
  const { runInstalledNavigation } = await import('../installed-navigation-profile.mjs');
  const { extractUsage } = await import('../codestory-agent-ab-benchmark.mjs');
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'navigation-test-'));
  t.after(() => fs.rm(root, {recursive:true,force:true}));
  const template = path.join(root, 'template'); await fs.mkdir(template); await fs.writeFile(path.join(template, 'config.toml'), '');
  if (option === 'skills') { await fs.mkdir(path.join(template, 'skills')); }
  const manifest = fixture(); manifest.arms = ['native']; manifest.repeats = 2;
  manifest.sessions = [1,2].map((repeat, index) => ({sequence:index+1,session_id:`native-${repeat}`,task_id:'t',arm:'native',repeat}));
  const bytes = JSON.stringify(manifest); await fs.writeFile(path.join(root, 'manifest.json'), bytes);
  const originalNow = Date.now; let now = originalNow();
  const installation = { manifest_sha256:createHash('sha256').update(bytes).digest('hex'), budget:{max_sessions:2,max_wall_ms:1200000}, arms:{native:{codex_home_template:template}} };
  if (option.startsWith('deadline')) { installation.budget.deadline_utc = new Date(now + 10000).toISOString(); Date.now = () => now; }
  await fs.writeFile(path.join(root, 'installations.json'), JSON.stringify(installation));
  let attempts = 0;
  const pass = stdout => ({status:'pass',stdout,stderr:'',exitCode:0,timedOut:false});
  const helpers = { extractUsage, analyzeTranscript:()=>({}), runProcess:async(command,args,options) => {
    if (command === 'git') {
      if (args.includes('clone')) await fs.mkdir(args.at(-1), {recursive:true});
      return pass(args.includes('rev-parse') ? args.at(-1)==='HEAD^{tree}' ? 'b'.repeat(40) : 'a'.repeat(40) : '');
    }
    if (args[0] === 'plugin') {
      const preflight = options.env.CODEX_HOME.includes('preflight-');
      if (option === 'deadline-preflight' && preflight || option === 'deadline-preparation' && !preflight) now += 20000;
      if (option === 'preparation' && options.env.CODEX_HOME.includes('native-1')) return {...pass(''),status:'fail',stderr:'inventory unavailable'};
      return pass('{"installed":[]}');
    }
    attempts++; assert.ok(options.timeoutMs > 0 && options.timeoutMs <= 600000);
    if (option === 'spawn') throw new Error('spawn unavailable');
    const usage = option === 'usage' ? {input_tokens:11} : {input_tokens:11,output_tokens:7,total_tokens:18};
    return {...pass(`${JSON.stringify({type:'turn.completed',usage})}\n`), ...(option === 'model-failure' ? {status:'fail',exitCode:1} : {})};
  } };
  let error;
  try { await runInstalledNavigation(['--manifest',path.join(root,'manifest.json'),'--installations',path.join(root,'installations.json'),'--out',path.join(root,'out')],helpers); }
  catch (caught) { error = caught; } finally { Date.now = originalNow; }
  return { attempts, error, summary:JSON.parse(await fs.readFile(path.join(root,'out/summary.json'),'utf8')) };
}

test('failure and budget matrix retains every row without launching after expiry', async t => {
  for (const option of ['skills','deadline-preflight','deadline-preparation','preparation','spawn','usage','model-failure']) {
    const result = await accountingFixture(t, option);
    assert.equal(result.summary.results.length, 2);
    assert.ok(result.summary.results.every(row => row.status !== 'not_run'));
    if (['skills','deadline-preflight','deadline-preparation'].includes(option)) assert.equal(result.attempts, 0);
    else if (option === 'preparation') { assert.equal(result.attempts, 1); assert.equal(result.summary.results[0].status,'preparation_failed'); assert.equal(result.summary.results[1].status,'pass'); }
    else assert.equal(result.attempts, 2);
    if (option === 'usage') assert.ok(result.summary.results.every(row => row.telemetry_complete === false));
    if (option === 'spawn') assert.ok(result.summary.results.every(row => row.status === 'model_failed'));
    if (option === 'model-failure') assert.ok(result.summary.results.every(row => row.status === 'fail'));
  }
});
