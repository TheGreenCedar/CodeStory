import assert from "node:assert/strict";
import {execFileSync} from "node:child_process";
import {mkdir,readFile,writeFile} from "node:fs/promises";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {fileURLToPath} from "node:url";
import {executeRecorded,fileBinding,executionEnvironment} from "./lib/etr1-execution.mjs";
import {strIdentity,readExecutionBinding} from "./lib/str1-execution.mjs";
import {validateEtr1} from "./codestory-etr1-validate.mjs";

async function main() {
  const config=JSON.parse(await readFile(process.argv[2],"utf8")),{root,binary,etrBinary,diagnostic,method,sourceRoot}=config;
  await mkdir(root,{mode:0o700});const project=path.join(root,"repository");await mkdir(project,{mode:0o700});
  // Exercise the same command entry points used by the corpus owner, not
  // imported approximations that conceal asynchronous module-loading cycles.
  const command=async(name,script,input)=>{
    if(input.env)input={...input,env:executionEnvironment(input.env)};
    const file=path.join(root,`${name}-config.json`);await writeFile(file,JSON.stringify(input),{flag:"wx",mode:0o600});
    const stdout=execFileSync(process.execPath,[path.join(sourceRoot,script),file],{cwd:sourceRoot,encoding:"utf8",timeout:120_000});
    return stdout.trim().split("\n").map(line=>JSON.parse(line));
  };
  const supervise=async(name,input)=>{const output=await command(name,"scripts/lib/str1-execution.mjs",input);
    assert.equal(output.length,2);return {request:output[0].prelaunch_request,binding:output[1],receipt:await readExecutionBinding(output[1])};};
  const source=Array.from({length:32},(_,i)=>`fn commonneedle_${i}() { ${i===0?Array.from({length:12},(_,j)=>`commonneedle_${j+18}();`).join(" "):`commonneedle_${(i+1)%32}();`}${i===0?" /* raremarker */":""} }${i===1?" /* a\u2028b\u2029c */":""}${i%3===0?"\r\n":"\n"}`).join("");
  await writeFile(path.join(project,"canary.rs"),source,{flag:"wx",mode:0o600});
  const git=(...args)=>execFileSync("git",["-C",project,"-c","core.hooksPath=/dev/null",...args],{stdio:"pipe"});
  git("init","--quiet");git("add","canary.rs");git("-c","user.name=STR canary","-c","user.email=canary@invalid.local","-c","commit.gpgsign=false","commit","--quiet","-m","freeze synthetic source");
  const prepared=path.join(root,"prepared");execFileSync(etrBinary,["prepare-canary","--project-root",project,"--output-dir",prepared],{stdio:"pipe",timeout:60_000});
  const preparation=await fileBinding(path.join(prepared,"preparation.json")),p=JSON.parse(await readFile(preparation.path,"utf8"));
  const makeState=async name=>{const state=path.join(root,name),ipc=path.join(state,"ipc"),cache=path.join(state,"cache");
    await mkdir(state,{mode:0o700});await mkdir(ipc,{mode:0o700});await mkdir(cache,{mode:0o700});const nonce=`str1-${randomUUID()}`;
    return {state,events:path.join(ipc,`${nonce}.events.jsonl`),env:{...process.env,CODESTORY_CACHE_ROOT:cache,CODESTORY_EMBED_ALLOW_CPU:"false",CODESTORY_EMBED_QUALIFICATION_DIR:ipc,CODESTORY_EMBED_QUALIFICATION_NONCE:nonce}};};
  const docs=await makeState("document-state"),vectorPath=path.join(docs.state,"vectors.json");
  const documents=await executeRecorded({role:"documents",authority:"synthetic_canary_only",executable:diagnostic,
    args:["--input",p.embedding_input.path,"--input-sha256",p.embedding_input.sha256,"--state-root",docs.state,"--output",vectorPath],
    inputs:[p.embedding_input.path],outputPaths:[vectorPath],eventsPath:docs.events,directory:path.join(root,"document-execution"),sourceRoot,env:docs.env});
  assert.equal(documents.receipt.experiment_status,"completed");const vectors=await fileBinding(vectorPath);
  const ctl=await makeState("control-state"),controlDir=path.join(root,"control"),controlPath=path.join(controlDir,"run.json"),cancelFile=path.join(root,"cancel");
  const controlExecution=await executeRecorded({role:"paired_run",authority:"synthetic_canary_only",executable:etrBinary,
    args:["run","--prepared",preparation.path,"--prepared-sha256",preparation.sha256,"--fragment-vectors",vectors.path,"--fragment-vectors-sha256",vectors.sha256,
      "--document-execution",documents.binding.path,"--document-execution-sha256",documents.binding.sha256,"--state-root",ctl.state,"--output-dir",controlDir,"--cancel-file",cancelFile],
    inputs:[preparation.path,vectors.path,documents.binding.path],outputPaths:[controlPath],eventsPath:ctl.events,directory:path.join(root,"control-execution"),sourceRoot,env:ctl.env,cancelFile});
  assert.equal(controlExecution.receipt.experiment_status,"completed");const controlRun=await fileBinding(controlPath);
  const checkedControl=await validateEtr1({runBinding:controlRun,sourceRoot,executionBinding:controlExecution.binding,allowCanary:true});
  const cvPath=path.join(root,"control-validation.json");await writeFile(cvPath,JSON.stringify({contract:"codestory.etr1-validation/v1",authority:"synthetic_canary_only",experiment_status:"valid",decision:"not_evaluated",annotation_access:"not_accessed",run:controlRun,execution:controlExecution.binding,binary_sha256:checkedControl.run.build.binary_sha256}),{flag:"wx",mode:0o600});
  const controlValidation=await fileBinding(cvPath);
  const writeJob=async(name,job)=>{const file=path.join(root,`${name}-job.json`);await writeFile(file,JSON.stringify(job),{flag:"wx",mode:0o600});return file;};
  const graphDir=path.join(root,"graph-core"),indexJob=await writeJob("index",{operation:"index_canary",preparation,method,cancel_file:cancelFile,output:graphDir});
  execFileSync(binary,["--job",indexJob],{cwd:sourceRoot,stdio:"pipe",timeout:120_000});
  const exportJob=await writeJob("export",{operation:"export_graphs",preparation,method,cancel_file:cancelFile,output:path.join(root,"graphs"),graph_preparations:{canary:await fileBinding(path.join(graphDir,"prepared.json"))}});
  const graphExecution=await supervise("graph-supervisor",{binary,jobPath:exportJob,directory:path.join(root,"graph-execution"),sourceRoot,env:process.env});
  assert.equal(graphExecution.receipt.experiment_status,"completed");
  const graphData=JSON.parse(await readFile(graphExecution.receipt.output.path,"utf8"));
  assert.ok(graphData.graphs[0].relations.length>=12,"canary failed to exercise certain witnessed relationships");
  const query=await makeState("query-state"),runDir=path.join(root,"run");
  const runJob=await writeJob("run",{operation:"run",preparation,method,graphs:graphExecution.receipt.output,vectors,control_run:controlRun,state_root:query.state,cancel_file:cancelFile,output:runDir});
  const execution=await supervise("run-supervisor",{binary,jobPath:runJob,directory:path.join(root,"run-execution"),sourceRoot,env:query.env});
  assert.equal(execution.receipt.experiment_status,"completed");
  const validationPath=path.join(root,"validation.json");
  const [validation]=await command("validation","scripts/codestory-str1-validate.mjs",{execution:execution.binding,executionRequest:execution.request,graphExecution:graphExecution.binding,graphRequest:graphExecution.request,sourceRoot,controlValidation,controlSourceRoot:sourceRoot,reconstructionRoot:root,output:validationPath});
  assert.equal((await readExecutionBinding(validation)).experiment_status,"valid");
  const run=await readExecutionBinding(execution.receipt.output);
  assert.ok(run.rows.some(row=>row.candidate.steps.some(step=>step.eligible.length>8)),"canary did not exercise overflow");
  assert.deepEqual(run.rows.map(r=>r.seed_fragment_ids.length),[16,1,0]);
  const first=p.fragments[0],truth={authority:"synthetic_canary_only",cases:p.wordings.map(row=>({case_id:row.case_id,acceptable_sets:[{set_id:"first",required_relation_atoms:[],required_source_atoms:[{atom_id:"first",source_range:{path:first.path,content_digest:first.content_digest,byte_range:first.byte_range,line_range:first.line_range}}]}]}))};
  const truthPath=path.join(root,"annotations.json");await writeFile(truthPath,JSON.stringify(truth),{flag:"wx",mode:0o600});
  const annotations=await fileBinding(truthPath);
  const [evaluation]=await command("evaluation","scripts/codestory-str1-evaluate.mjs",{validation,annotations,sourceRoot,output:path.join(root,"evaluation.json")});
  const evaluated=await readExecutionBinding(evaluation);
  assert.deepEqual(evaluated.rows.map(row=>row.candidate.recall),[1,1,0]);
  const args=["--test","scripts/tests/str1-evidence.test.mjs"];
  const stdout=execFileSync(process.execPath,args,{cwd:sourceRoot,encoding:"utf8",timeout:60_000});
  const hostileOutput=path.join(root,"hostile-output.json");await writeFile(hostileOutput,JSON.stringify({stdout}),{flag:"wx",mode:0o600});
  const hostilePath=path.join(root,"hostile.json");await writeFile(hostilePath,JSON.stringify({exit_code:0,args,node:await fileBinding(process.execPath),test_source:await fileBinding(path.join(sourceRoot,"scripts/tests/str1-evidence.test.mjs")),output:await fileBinding(hostileOutput)}),{flag:"wx",mode:0o600});
  const receiptPath=path.join(root,"receipt.json");await writeFile(receiptPath,JSON.stringify({contract:"codestory.str1-canary/v1",authority:"synthetic_canary_only",experiment_status:"valid",binary:await fileBinding(binary),analysis:await strIdentity(sourceRoot),validation,annotations,evaluated,hostile:await fileBinding(hostilePath)}),{flag:"wx",mode:0o600});
  console.log(JSON.stringify(await fileBinding(receiptPath)));
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url))main().catch(e=>{console.error(e.stack);process.exitCode=1;});
