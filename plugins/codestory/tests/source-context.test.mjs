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

test('actual host observer optionally reports last response usage without changing capture', {skip:!['darwin','linux'].includes(process.platform)}, () => {
  const temp=fs.mkdtempSync(join(tmpdir(),'codestory-model-usage-'));
  try {
    const cli=join(temp,'cli'); const marker=join(temp,'must-not-execute');
    fs.writeFileSync(cli,`#!/bin/sh\nprintf bad > '${marker}'\n`,{mode:0o755});
    const codexHome=join(temp,'codex-home'); const pluginData=join(temp,'plugin-data');
    const timestamp=new Date(Date.now()-2000).toISOString();
    const sessions=join(codexHome,'sessions',...timestamp.slice(0,10).split('-'));
    fs.mkdirSync(sessions,{recursive:true});fs.mkdirSync(pluginData);
    const sessionId='01a18000-0000-7000-8000-000000000001';
    const turnId='01a18000-0000-7000-8000-000000000002';
    const transcript=join(sessions,`rollout-${timestamp.slice(0,19).replaceAll(':','-')}-${sessionId}.jsonl`);
    const records=[
      {timestamp,ordinal:0,type:'session_meta',payload:{id:sessionId,session_id:sessionId,timestamp,cwd:temp,cli_version:'0.164.0',source:'exec',thread_source:'user'}},
      {timestamp,ordinal:1,type:'token_usage_record',payload:{
        thread_id:sessionId,session_id:sessionId,turn_id:turnId,root_turn_id:turnId,response_id:'resp_usage_fixture',
        usage:{input_tokens:1000,cached_input_tokens:800,cache_write_input_tokens:0,output_tokens:25,reasoning_output_tokens:15,total_tokens:1025},
        turn_token_usage:{input_tokens:9000,cached_input_tokens:7000,cache_write_input_tokens:0,output_tokens:400,reasoning_output_tokens:100,total_tokens:9400},
        thread_token_usage:{input_tokens:20000,cached_input_tokens:16000,cache_write_input_tokens:0,output_tokens:900,reasoning_output_tokens:300,total_tokens:20900},
      }},
    ].map(record=>JSON.stringify(record)).join('\n')+'\n';
    fs.writeFileSync(transcript,records);
    const e=event();e.cwd=temp;e.tool_input.workdir=temp;
    e.session_id=sessionId;e.transcript_path=transcript;
    const env={...process.env,PLUGIN_DATA:pluginData,COPILOT_PLUGIN_DATA:pluginData,CODESTORY_PLUGIN_DATA:pluginData,CODEX_HOME:codexHome};
    delete env.CODESTORY_MODEL_USAGE_FEEDBACK;
    const invoke=extra=>{
      const output=spawnSync(process.execPath,[join(root,'hooks/codestory-source-context.cjs'),'--cli',cli],{
        input:JSON.stringify(e),encoding:'utf8',timeout:5000,env:{...env,...extra},
      });
      assert.equal(output.status,0,output.stderr);assert.ok(output.stdout.trim());
      return JSON.parse(output.stdout).hookSpecificOutput;
    };
    const baseline=invoke({});
    assert.equal(baseline.hookEventName,'PreToolUse');assert.equal(baseline.permissionDecision,'allow');
    assert.equal(baseline.additionalContext,undefined);
    const {command,...retained}=baseline.updatedInput;
    const {command:script,...expected}=e.tool_input;
    assert.deepEqual(retained,expected);
    assert.match(command,/internal-source-context capture --payload [0-9a-f]+$/u);
    const payload=JSON.parse(Buffer.from(command.match(/--payload ([0-9a-f]+)$/u)[1],'hex'));
    assert.equal(payload.command,script);assert.equal(payload.project,temp);assert.equal(payload.cwd,temp);

    const enabled=invoke({CODESTORY_MODEL_USAGE_FEEDBACK:'1'});
    assert.equal(enabled.hookEventName,baseline.hookEventName);
    assert.equal(enabled.permissionDecision,baseline.permissionDecision);
    assert.deepEqual(enabled.updatedInput,baseline.updatedInput);
    assert.equal(fs.existsSync(marker),false);assert.equal(fs.readFileSync(transcript,'utf8'),records);
    assert.equal(typeof enabled.additionalContext,'string','opt-in must deliver observed usage through the real hook entrypoint');
    const feedback=enabled.additionalContext;
    assert.ok(Buffer.byteLength(feedback)<=384,'usage feedback must remain a bounded notice');
    assert.match(feedback,/\binput(?:_tokens)?\b[^\d]{0,24}1[,_ ]?000\b/iu);
    assert.match(feedback,/\boutput(?:_tokens)?\b[^\d]{0,24}25\b/iu);
    assert.match(feedback,/\btotal(?:_tokens)?\b[^\d]{0,24}1[,_ ]?025\b/iu);
    assert.doesNotMatch(feedback,/\b(?:1[,_ ]?800|1[,_ ]?040|9[,_ ]?000|9[,_ ]?400|20[,_ ]?000|20[,_ ]?900)\b/u);
    assert.match(feedback,/cached[^.\n]*(?:includ|within|part)/iu);
    assert.match(feedback,/transcript/iu);
    assert.match(feedback,/lag|stale|older|previous|delayed|last[^.\n]*observed/iu);
    assert.match(feedback,/(?:helper|subagent)[^.\n]*(?:unknown|exclud|not[^.\n]*includ)|(?:unknown|exclud)[^.\n]*(?:helper|subagent)/iu);
  } finally { fs.rmSync(temp,{recursive:true,force:true}); }
});

test('actual usage observer rejects hostile metadata and file snapshots while preserving capture', {skip:!['darwin','linux'].includes(process.platform)}, () => {
  const temp=fs.mkdtempSync(join(tmpdir(),'codestory-usage-matrix-'));
  const sessionId='01a18000-0000-7000-8000-000000000001';
  const turnId='01a18000-0000-7000-8000-000000000002';
  const timestamp=new Date(Date.now()-5000).toISOString();
  const header=()=>({timestamp,type:'session_meta',payload:{id:sessionId,session_id:sessionId,cli_version:'0.164.0',source:'exec',thread_source:'user'}});
  const usage=()=>({timestamp,type:'token_usage_record',payload:{thread_id:sessionId,session_id:sessionId,turn_id:turnId,root_turn_id:turnId,response_id:'resp_matrix',usage:{input_tokens:1000,cached_input_tokens:800,cache_write_input_tokens:0,output_tokens:25,reasoning_output_tokens:15,total_tokens:1025},turn_token_usage:{input_tokens:9000,output_tokens:400,total_tokens:9400}}});
  const serialize=records=>records.map(record=>JSON.stringify(record)).join('\n')+'\n';
  const freshUsage=()=>{
    const record=usage();record.timestamp=new Date(Date.now()-3000).toISOString();record.payload.response_id='resp_duplicate_type';
    record.payload.usage={input_tokens:2222,cached_input_tokens:800,cache_write_input_tokens:0,output_tokens:33,reasoning_output_tokens:15,total_tokens:2255};
    return record;
  };
  const appendRaw=(f,line)=>fs.writeFileSync(f.transcript,serialize(f.records)+line+'\n');
  const tree=dir=>fs.readdirSync(dir,{withFileTypes:true}).flatMap(entry=>{
    const name=join(dir,entry.name);return entry.isDirectory()?[name,...tree(name)]:[name];
  }).sort();
  const cases=[
    {name:'R1 later usage with final event discriminator must omit feedback',write:f=>{
      const line=JSON.stringify(freshUsage());appendRaw(f,line.slice(0,-1)+',"type":"event_msg"}');
    }},
    {name:'identical duplicate is one snapshot',signal:true,change:f=>{f.records.push(structuredClone(f.records[1]));}},
    {name:'irrelevant numeric formatting need not roundtrip',signal:true,write:f=>{
      appendRaw(f,`{"timestamp":"${timestamp}","type":"event_msg","payload":{"duration":1.0}}`);
    }},
    ...[
      ['duplicate header owner',0,line=>line.replace(`"id":"${sessionId}"`,`"id":"${turnId}","id":"${sessionId}"`)],
      ['duplicate header session',0,line=>line.replace(`"session_id":"${sessionId}"`,`"session_id":"${turnId}","session_id":"${sessionId}"`)],
      ['duplicate header version',0,line=>line.replace('"cli_version":"0.164.0"','"cli_version":"0.165.0","cli_version":"0.164.0"')],
      ['duplicate nested usage object',1,line=>line.replace('"usage":','"usage":{"input_tokens":1},"usage":')],
      ['duplicate containing counter',1,line=>line.replace('"input_tokens":1000','"input_tokens":1,"input_tokens":1000')],
      ['same-value duplicate owner',0,line=>line.replace(`"id":"${sessionId}"`,`"id":"${sessionId}","id":"${sessionId}"`)],
      ['same-value duplicate counter',1,line=>line.replace('"input_tokens":1000','"input_tokens":1000,"input_tokens":1000')],
      ['same-value duplicate discriminator',1,line=>line.replace('"type":"token_usage_record"','"type":"token_usage_record","type":"token_usage_record"')],
    ].map(([name,index,mutate])=>({name,write:f=>{
      const lines=f.records.map(record=>JSON.stringify(record));lines[index]=mutate(lines[index]);
      fs.writeFileSync(f.transcript,lines.join('\n')+'\n');
    }})),
    {name:'reverse event discriminator cannot hide a later usage record',write:f=>{
      appendRaw(f,JSON.stringify(freshUsage()).replace('"type":"token_usage_record"','"type":"event_msg","type":"token_usage_record"'));
    }},
    {name:'cached input can equal input',signal:true,change:f=>{f.records[1].payload.usage.cached_input_tokens=1000;}},
    {name:'opaque private text is not emitted',signal:true,change:f=>{f.records.push({timestamp,type:'response_item',payload:{type:'message',content:'PRIVATE_USAGE_SENTINEL'}});}},
    {name:'later response replaces earlier cumulative totals',signal:true,change:f=>{
      const latest=usage();latest.timestamp=new Date(Date.now()-3000).toISOString();latest.payload.response_id='resp_latest';
      f.records[1].payload.usage={input_tokens:9000,cached_input_tokens:8000,cache_write_input_tokens:0,output_tokens:400,reasoning_output_tokens:100,total_tokens:9400};f.records.push(latest);
    }},
    {name:'unknown version',change:f=>{f.records[0].payload.cli_version='0.165.0';}},
    {name:'missing version',change:f=>{delete f.records[0].payload.cli_version;}},
    {name:'child first owner cannot be repaired by inherited parent',change:f=>{const parent=header();f.records[0].payload.id=turnId;f.records[0].payload.thread_source='subagent';f.records.splice(1,0,parent);}},
    {name:'later owner conflict',change:f=>{const other=header();other.payload.cli_version='0.165.0';f.records.push(other);}},
    {name:'hook session mismatch',change:f=>{f.event.session_id=turnId;}},
    {name:'usage thread mismatch',change:f=>{f.records[1].payload.thread_id=turnId;}},
    {name:'helper root-turn relation',change:f=>{f.records[1].payload.root_turn_id=sessionId;}},
    {name:'unknown usage layout',change:f=>{f.records[1].payload.usage={input:1000,output:25};}},
    ...[
      ['negative input','input_tokens',-1],['fractional output','output_tokens',25.5],
      ['string input','input_tokens','1000'],['unsafe input','input_tokens',Number.MAX_SAFE_INTEGER+1],
      ['cached exceeds input','cached_input_tokens',1001],['reasoning exceeds output','reasoning_output_tokens',26],
      ['wrong containing total','total_tokens',1040],['missing cached subset','cached_input_tokens',undefined],
    ].map(([name,key,value])=>({name,change:f=>{f.records[1].payload.usage[key]=value;}})),
    {name:'conflicting duplicate response',change:f=>{const duplicate=structuredClone(f.records[1]);duplicate.payload.usage.output_tokens=26;duplicate.payload.usage.total_tokens=1026;f.records.push(duplicate);}},
    {name:'out of order responses',change:f=>{const older=usage();older.timestamp=new Date(Date.parse(timestamp)-1000).toISOString();older.payload.response_id='resp_older';f.records.push(older);}},
    {name:'ambiguous equal timestamps',change:f=>{const duplicate=usage();duplicate.payload.response_id='resp_other';f.records.push(duplicate);}},
    {name:'partial newest record never falls back',write:f=>{fs.writeFileSync(f.transcript,serialize(f.records)+'{"timestamp":"');}},
    {name:'malformed newest usage never falls back',write:f=>{fs.writeFileSync(f.transcript,serialize(f.records)+`{"timestamp":"${timestamp}","type":"token_usage_record","payload":oops}\n`);}},
    {name:'invalid UTF8 never falls back',write:f=>{fs.writeFileSync(f.transcript,Buffer.concat([Buffer.from(serialize(f.records)),Buffer.from([0xff,10])]));}},
    {name:'oversize newest usage',change:f=>{f.records[1].payload.private='PRIVATE_USAGE_SENTINEL'.repeat(500);}},
    {name:'header outside bounded window',change:f=>{f.records[0].payload.padding='x'.repeat(65536);}},
    {name:'bounded row count',change:f=>{f.records.push(...Array.from({length:2048},()=>({timestamp,type:'event_msg',payload:{type:'ignored'}})));}},
    {name:'large sparse transcript with bounded latest tail',signal:true,write:f=>{
      fs.writeFileSync(f.transcript,serialize([f.records[0]]));
      const fd=fs.openSync(f.transcript,'r+');try {fs.writeSync(fd,Buffer.from('\n'+serialize([f.records[1]])),0,Buffer.byteLength('\n'+serialize([f.records[1]])),4*1024*1024);} finally {fs.closeSync(fd);}
      f.fault='readBound';
    }},
    {name:'usage outside tail is not discovered',write:f=>{fs.writeFileSync(f.transcript,serialize(f.records));fs.appendFileSync(f.transcript,serialize([{timestamp,type:'response_item',payload:{text:'x'.repeat(1024*1024)}}]));}},
    {name:'missing transcript',write:f=>{fs.unlinkSync(f.transcript);}},
    {name:'relative path',change:f=>{f.event.transcript_path='sessions/current.jsonl';}},
    {name:'dotdot path',change:f=>{f.event.transcript_path=join(f.home,'sessions')+'/../sessions/current.jsonl';}},
    {name:'outside sessions',change:f=>{f.event.transcript_path=join(f.home,'config.jsonl');}},
    {name:'sibling prefix is outside home',change:f=>{f.event.transcript_path=join(f.home+'-other','sessions','current.jsonl');}},
    {name:'directory is not a transcript',write:f=>{fs.unlinkSync(f.transcript);fs.mkdirSync(f.transcript);}},
    {name:'symlink leaf',write:f=>{const target=f.transcript+'.target';fs.renameSync(f.transcript,target);fs.symlinkSync(target,f.transcript);}},
    {name:'symlink descendant directory',write:f=>{const target=join(f.dir,'outside');fs.mkdirSync(target);fs.writeFileSync(join(target,'current.jsonl'),serialize(f.records));fs.symlinkSync(target,join(f.home,'sessions','linked'));f.event.transcript_path=join(f.home,'sessions','linked','current.jsonl');}},
    {name:'symlink home',write:f=>{const alias=f.home+'-alias';fs.symlinkSync(f.home,alias);f.env.CODEX_HOME=alias;f.event.transcript_path=join(alias,'sessions','current.jsonl');}},
    {name:'descriptor failure',fault:'open'},
    {name:'startup home failure preserves capture',fault:'home',change:f=>{delete f.env.CODEX_HOME;}},
    {name:'append during descriptor read',fault:'append'},
    {name:'leaf replaced before open',fault:'replace'},
    {name:'default off does not read metadata',disabled:true,signal:false,fault:'denyMetadata'},
    {name:'declined tool does not read metadata',change:f=>{f.event.tool_name='Read';},fault:'denyMetadata'},
  ];
  try {
    for (const [index,scenario] of cases.entries()) {
      const dir=join(temp,String(index));fs.mkdirSync(dir);
      const home=join(dir,'home');fs.mkdirSync(join(home,'sessions'),{recursive:true});
      const cli=join(dir,'cli');const marker=join(dir,'must-not-execute');
      fs.writeFileSync(cli,`#!/bin/sh\nprintf bad > '${marker}'\n`,{mode:0o755});
      const transcript=join(home,'sessions','current.jsonl');
      const e=event();e.cwd=dir;e.tool_input.workdir=dir;e.session_id=sessionId;e.transcript_path=transcript;
      const f={dir,home,cli,transcript,event:e,records:[header(),usage()],fault:scenario.fault,env:{...process.env,CODEX_HOME:home,PLUGIN_DATA:dir,COPILOT_PLUGIN_DATA:dir,CODESTORY_PLUGIN_DATA:dir,CODESTORY_MODEL_USAGE_FEEDBACK:'1'}};
      if (scenario.disabled) delete f.env.CODESTORY_MODEL_USAGE_FEEDBACK;
      scenario.change?.(f);fs.writeFileSync(transcript,serialize(f.records));scenario.write?.(f);
      const denied=join(dir,'unexpected-metadata-read');const preload=join(dir,'fault.cjs');
      const args=[join(root,'hooks/codestory-source-context.cjs'),'--cli',cli];
      if (f.fault) {
        fs.writeFileSync(preload,`
          const fs=require('node:fs');const target=${JSON.stringify(transcript)};const home=${JSON.stringify(home)};
          const fault=${JSON.stringify(f.fault)};const open=fs.openSync;const read=fs.readSync;let watched;let changed=false;let bytes=0;
          if(fault==='home')require('node:os').homedir=()=>{throw Error('fixture home failure');};
          if(fault==='denyMetadata')for(const key of ['lstatSync','realpathSync','openSync']){const original=fs[key];fs[key]=function(name,...rest){if(typeof name==='string'&&(name===home||name.startsWith(home+'/'))){fs.writeFileSync(${JSON.stringify(denied)},'unexpected');throw Error('denied metadata');}return original.call(this,name,...rest);};}
          else {
            fs.openSync=function(name,...rest){if(name===target){if(fault==='open')throw Error('fixture open failure');if(fault==='replace'){fs.renameSync(target,target+'.old');fs.writeFileSync(target,fs.readFileSync(target+'.old'));}}const fd=open.call(this,name,...rest);if(name===target)watched=fd;return fd;};
            fs.readSync=function(fd,...rest){const count=read.call(this,fd,...rest);if(fd===watched){bytes+=count;if(bytes>65536+1048576){fs.writeFileSync(${JSON.stringify(denied)},'read bound');throw Error('read bound');}if(fault==='append'&&!changed){changed=true;fs.appendFileSync(target,'\\n');}}return count;};
          }
        `);
        args.unshift('--require',preload);
      }
      const expected=rewrite(e,{cli,shimDirectory:join(root,'hooks/source-context-bin')});
      const before=tree(dir);
      const result=spawnSync(process.execPath,args,{input:JSON.stringify(e),encoding:'utf8',timeout:5000,env:f.env});
      assert.equal(result.status,0,`${scenario.name}: ${result.stderr}`);assert.equal(result.stderr,'',scenario.name);
      if (expected) {
        const actual=JSON.parse(result.stdout);const context=actual.hookSpecificOutput.additionalContext;
        delete actual.hookSpecificOutput.additionalContext;assert.deepEqual(actual,expected,scenario.name);
        if (scenario.signal) {
          assert.equal(typeof context,'string',scenario.name);assert.ok(Buffer.byteLength(context)<=384,scenario.name);
          assert.match(context,/\binput(?:_tokens)?\b[^\d]{0,24}1[,_ ]?000\b/iu,scenario.name);
          assert.match(context,/\boutput(?:_tokens)?\b[^\d]{0,24}25\b/iu,scenario.name);
          assert.match(context,/\btotal(?:_tokens)?\b[^\d]{0,24}1[,_ ]?025\b/iu,scenario.name);
          assert.doesNotMatch(context,/PRIVATE_USAGE_SENTINEL|resp_|01a18000|\/sessions\/|9400|20900/u,scenario.name);
        } else assert.equal(context,undefined,scenario.name);
      } else assert.equal(result.stdout,'',scenario.name);
      assert.equal(fs.existsSync(marker),false,scenario.name);assert.equal(fs.existsSync(denied),false,scenario.name);
      if (!['append','replace'].includes(f.fault)) assert.deepEqual(tree(dir),before,scenario.name);
    }
  } finally {fs.rmSync(temp,{recursive:true,force:true});}
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
