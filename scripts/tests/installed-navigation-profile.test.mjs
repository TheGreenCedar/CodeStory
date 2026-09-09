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
  assert.deepEqual(args, ['exec', '--disable', 'remote_plugin', '--model', 'gpt-5.6-terra', '--config', 'model_reasoning_effort="low"', '--sandbox', 'workspace-write', '--cd', '/checkout', '--json', '--output-last-message', '/output/answer.md', '-']);
});
test('runtime and host overrides cannot escape the isolated session', () => {
  const env = navigationEnvironment({ PATH: '/bin', HOME: '/real', CODEX_HOME: '/real/.codex', CODESTORY_CLI: '/wrong/binary', CODESTORY_PLUGIN_DATA: '/shared', CODESTORY_EMBED_QUALIFICATION_DIR: '/shared', OPENAI_API_KEY: 'never-retain', PLUGIN_DATA: '/shared' }, '/isolated', 'nonce');
  assert.equal(env.HOME, '/isolated/home');
  assert.equal(env.CODEX_HOME, '/isolated/codex');
  assert.equal(env.CODESTORY_EMBED_QUALIFICATION_DIR, '/isolated/qualification');
  assert.equal(env.PATH, '/bin');
  for (const key of ['CODESTORY_CLI', 'CODESTORY_PLUGIN_DATA', 'OPENAI_API_KEY', 'PLUGIN_DATA']) assert.equal(env[key], undefined);
});

test('explicit host registration preserves installed transport and forwards only isolated controls', async () => {
  const { installedHostConfig } = await import('../installed-navigation-profile.mjs');
  const env = navigationEnvironment({ PATH: '/bin' }, '/isolated', 'nonce');
  env.CODESTORY_PLUGIN_DATA = '/isolated/data';
  env.CODESTORY_PLUGIN_RELEASE_DIR = '/archives';
  env.CODESTORY_CLI = '/forbidden';
  const config = installedHostConfig({ command: 'node', args: ['./scripts/codestory-mcp.cjs'], cwd: '.', env: {}, tool_timeout_sec: 300 }, '/installed', { marketplace_name: 'fixture' }, env);
  assert.match(config, /\[plugins\."codestory@fixture"\.mcp_servers\.codestory\]\nenabled = false/);
  assert.match(config, /\[mcp_servers\.codestory\]/);
  assert.match(config, /tool_timeout_sec = 300/);
  assert.match(config, /CODESTORY_PLUGIN_RELEASE_DIR/);
  assert.match(config, /CODESTORY_EMBED_QUALIFICATION_NONCE/);
  assert.doesNotMatch(config, /CODESTORY_CLI|approval|enabled_tools|disabled_tools|shell_environment/);
  for (const bad of [{command:'node',args:['./scripts/codestory-mcp.cjs'],cwd:'.',env:{CODESTORY_CLI:'/wrong'}}, {command:'node',args:['../escape.cjs'],cwd:'.',env:{}}]) {
    assert.throws(() => installedHostConfig(bad, '/installed', {marketplace_name:'fixture'}, env));
  }
});

test('actual host boundary rejects missing controls and successful envelopes carrying unavailable tools', async () => {
  const { validateChildEnvironment, toolPayload } = await import('../installed-navigation-profile.mjs');
  const expected = { HOME:'/isolated/home', CODESTORY_PLUGIN_RELEASE_DIR:'/archives', CODESTORY_EMBED_QUALIFICATION_DIR:'/isolated/qualification', CODESTORY_EMBED_QUALIFICATION_NONCE:'nonce', CODESTORY_EMBED_ALLOW_CPU:'0' };
  validateChildEnvironment(expected, expected);
  for (const key of Object.keys(expected)) {
    const actual={...expected};delete actual[key];assert.throws(()=>validateChildEnvironment(actual,expected));
    actual[key]='/shared';assert.throws(()=>validateChildEnvironment(actual,expected));
  }
  for (const result of [{isError:true,content:[]}, {content:[{type:'text',text:JSON.stringify({code:'codestory_unavailable',failure:'managed_cli_asset_fetch_failed'})}]}])assert.throws(()=>toolPayload(result));
  assert.deepEqual(toolPayload({structured_content:{kind:'preparing',state:'preparing'}}),{kind:'preparing',state:'preparing'});
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

test('app-server transport handles versionless notifications without changing MCP defaults', async () => {
  const { createSequencedStdioSession } = await import('../codestory-agent-ab-benchmark.mjs');
  const notifications=[];
  const script=`process.stdin.on('data', b=>{const r=JSON.parse(String(b));process.stdout.write(JSON.stringify({method:'thread/started',params:{}})+'\\n'+JSON.stringify({id:r.id,result:{ok:true}})+'\\n');});process.stdin.on('end',()=>process.exit(0));`;
  const channel=createSequencedStdioSession(process.execPath,['-e',script],{protocol:'app-server',timeoutMs:5000,onNotification:n=>notifications.push(n)});
  try { assert.deepEqual((await channel.request({id:1,method:'initialize'})).result,{ok:true});assert.equal(notifications[0].method,'thread/started');await channel.close(); }
  finally { await channel.stop(); }
});

test('host catalog and participant runtime guards reject duplicates, changed tools, and nested provisioning failures', async () => {
  const { validateHostCatalog, auditParticipantRuntime }=await import('../installed-navigation-profile.mjs');
  const tool={name:'search',inputSchema:{type:'object'}};
  const arm={version:'0.17.6',schema_version:3,runtime_source:'managed',runtime_sha256:'a'.repeat(64)};
  const server={name:'codestory',runtimeStatus:'connected',pluginId:null,serverInfo:{name:'codestory',version:arm.version},tools:{search:tool}};
  validateHostCatalog({data:[server]},[tool],arm);
  validateHostCatalog({data:[server]},[{...tool,safety:{effect:'read_only'}}],{...arm,schema_version:2});
  assert.throws(()=>validateHostCatalog({data:[server]},[{...tool,annotations:{readOnlyHint:false}}],{...arm,schema_version:2}));
  for(const data of [[server,{...server,name:'duplicate'}],[{...server,tools:{}}],[{...server,pluginId:'unexpected'}]])assert.throws(()=>validateHostCatalog({data},[tool],arm));
  const event=result=>({type:'item.completed',item:{type:'mcp_tool_call',server:'codestory',tool:'search',result,status:'completed'}});
  const failed=event({content:[{type:'text',text:JSON.stringify({code:'codestory_unavailable',failure:'managed_cli_provision_failed:managed_cli_asset_fetch_failed'})}]});
  const identity={schema_version:3,contract_runtime:{cli_sha256:arm.runtime_sha256,cli_source:'managed',cli_version:arm.version,plugin_cli_version:arm.version,plugin_version:arm.version,pinned_pair_matches:true,known_override_skew_channel:false}};
  const good=event({_meta:{codestory_publication:identity}});
  assert.deepEqual(auditParticipantRuntime([good],arm),{calls:1,identity_stamps:1,failures:[]});
  assert.equal(auditParticipantRuntime([failed],arm).failures[0].code,'managed_cli_provision_failed:managed_cli_asset_fetch_failed');
  const bad=structuredClone(good);bad.item.result._meta.codestory_publication.contract_runtime.cli_sha256='wrong';
  assert.equal(auditParticipantRuntime([bad],arm).failures[0].code,'runtime_identity_mismatch');
  assert.deepEqual(auditParticipantRuntime([],arm),{calls:0,identity_stamps:0,failures:[]});
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
      assert.ok(args.includes('--disable') && args.includes('remote_plugin'));
      const preflight = options.env.CODEX_HOME.includes('preflight-');
      if (option === 'deadline-preflight' && preflight || option === 'deadline-preparation' && !preflight) now += 20000;
      if (option === 'preparation' && options.env.CODEX_HOME.includes('native-1')) return {...pass(''),status:'fail',stderr:'inventory unavailable'};
      return pass('{"installed":[]}');
    }
    attempts++; assert.ok(args.includes('--disable') && args.includes('remote_plugin')); assert.ok(options.timeoutMs > 0 && options.timeoutMs <= 600000);
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
