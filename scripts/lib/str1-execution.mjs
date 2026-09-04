import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile, realpath, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { analysisIdentity, executionEnvironment, fileBinding, readExecutionBinding } from "./etr1-execution.mjs";
import {sha256} from "./etr1-evidence.mjs";
export { fileBinding, readExecutionBinding };
export const STR_FIXED=Object.freeze({method:"0902f8f8f6771fce5c6addf09bbdef061fd3706be0da050c32baead80fb171fc",
  preparation:"30b84d4d848f96bd4fe799f2e0f28b9114971da0e47bf98ebe54fe36242199fd",
  vectors:"7f604b30b823066bd5b0ed71106d10577c28495abd270444bc8ad5b7a63cb70a",
  control:"c14da697d03707c0096f5f2fd7a97bff2ab5b4a6f9326c4a9a03da2066d545f2",
  validation:"dd15c5842b68857c8c684b680cf3d1e307cd4b1a874736441c594490723e7145",
  graph_inputs:"668c990ee29b25a4bab0cb03e048d70eebd8ffe3dd62317d69b2e58b912a2c9f"});
export function assertStrInputs(job,preparation,controlValidation) {
  assert.equal(job.method.sha256,STR_FIXED.method,"unregistered structural method");
  assert.ok(["synthetic_canary_only","visible_development_frontier_only"].includes(preparation.authority));
  if(preparation.authority==="synthetic_canary_only")return;
  assert.equal(job.preparation.sha256,STR_FIXED.preparation,"unregistered ETR preparation");
  assert.equal(job.graph_inputs?.sha256,STR_FIXED.graph_inputs,"unregistered graph inputs");
  if(job.vectors)assert.equal(job.vectors.sha256,STR_FIXED.vectors,"unregistered fragment vectors");
  if(job.control_run)assert.equal(job.control_run.sha256,STR_FIXED.control,"unregistered control run");
  if(controlValidation)assert.equal(controlValidation.sha256,STR_FIXED.validation,"unregistered control validation");
}
export function assertStrRequest(request,observedBinding,expectedBinding) {
  assert.ok(expectedBinding,"independently frozen request required");
  assert.deepEqual(observedBinding,expectedBinding,"execution request differs from prelaunch freeze");
  assert.equal(request.contract,"codestory.str1-execution-request/v1");
  assert.deepEqual(request.args,["--job",request.job.path],"execution argv changed");
  assert.equal(request.context_sha256,sha256(JSON.stringify({cwd:request.cwd,environment:request.environment})),"execution context changed");
}
async function authenticateGraphInputs(job,preparation) {
  if(preparation.authority==="synthetic_canary_only")return;
  const inputs=await readExecutionBinding(job.graph_inputs);
  assert.equal(inputs.contract,"codestory.str1-graph-inputs/v1");
  assert.deepEqual(inputs.repositories.map(r=>r.repository_id).sort(),preparation.repositories.map(r=>r.repository_id).sort());
  for(const input of inputs.repositories) {
    for(const file of [input.preparation,input.core,input.pointer])assert.deepEqual(await fileBinding(file.path),file);
    const wal=await stat(`${input.core.path}-wal`).catch(e=>{if(e.code!=="ENOENT")throw e;return null;});
    assert.ok(!wal||wal.size===0,"unbound graph WAL");
    if(job.operation==="export_graphs")assert.deepEqual(job.graph_preparations[input.repository_id],input.preparation);
  }
}
export const STR_FILES = ["scripts/lib/str1-evidence.mjs", "scripts/lib/str1-execution.mjs",
  "scripts/codestory-str1-validate.mjs", "scripts/codestory-str1-evaluate.mjs", "scripts/codestory-str1-canary.mjs", "scripts/tests/str1-evidence.test.mjs"];
export async function strIdentity(root) {
  return { ...await analysisIdentity(root), structural_files: await Promise.all(STR_FILES.map(f=>fileBinding(path.join(root,f)))) };
}
export async function validateStrExecution(binding, sourceRoot, expectedRequest) {
  const receipt=await readExecutionBinding(binding), request=await readExecutionBinding(receipt.request);
  assertStrRequest(request,receipt.request,expectedRequest);
  assert.equal(receipt.contract,"codestory.str1-execution/v1");
  assert.equal(receipt.experiment_status,"completed"); assert.equal(receipt.exit_code,0);
  assert.equal(receipt.signal,null); assert.equal(receipt.cancelled,false);
  assert.equal(request.deadline_ms,1_800_000);
  assert.equal(request.cwd,await realpath(sourceRoot));
  assert.deepEqual(request.environment,executionEnvironment(request.environment));
  assert.deepEqual(request.analysis,await strIdentity(sourceRoot));
  assert.deepEqual(request.binary,await fileBinding(request.binary.path));
  const job=await readExecutionBinding(request.job);
  for(const input of request.inputs) assert.deepEqual(input,await fileBinding(input.path));
  for(const input of [job.preparation,job.method,job.graph_inputs,job.graphs,job.vectors,job.control_run,...Object.values(job.graph_preparations??{})].filter(Boolean))
    assert.ok(request.inputs.some(x=>JSON.stringify(x)===JSON.stringify(input)),"unbound execution input");
  assert.deepEqual(receipt.output,await fileBinding(receipt.output.path));
  assert.equal(receipt.output.path,path.join(job.output,job.operation==="run"?"run.json":"graphs.json"));
  for(const stream of [receipt.stdout,receipt.stderr,receipt.events].filter(Boolean)) assert.deepEqual(stream,await fileBinding(stream.path));
  if(job.operation==="run") assert.ok(receipt.events,"native events missing");
  const preparation=await readExecutionBinding(job.preparation);
  assertStrInputs(job,preparation);
  await authenticateGraphInputs(job,preparation);
  if(preparation.authority!=="synthetic_canary_only") {
    await validateStrCanary(request.canary,sourceRoot,request.binary);
  }
  assert.ok(Number.isSafeInteger(receipt.wall_ns)&&receipt.wall_ns>0);
  return {request,receipt,job};
}

async function validateStrCanary(binding,sourceRoot,binary) {
  assert.ok(binding,"real canary required before corpus execution");
  const canary=await readExecutionBinding(binding);
  assert.equal(canary.contract,"codestory.str1-canary/v1");assert.equal(canary.authority,"synthetic_canary_only");
  assert.equal(canary.experiment_status,"valid");assert.deepEqual(canary.analysis,await strIdentity(sourceRoot));
  assert.deepEqual(canary.binary,binary);
  const validation=await readExecutionBinding(canary.validation);
  assert.equal(validation.authority,"synthetic_canary_only");assert.equal(validation.experiment_status,"valid");
  const {evaluateStr1}=await import("../codestory-str1-evaluate.mjs");
  const evaluated=await evaluateStr1({validation:canary.validation,annotations:canary.annotations,sourceRoot});
  assert.deepEqual(evaluated,canary.evaluated,"canary was not evaluated by the real pipeline");
  assert.deepEqual(evaluated.rows.map(row=>row.candidate.recall),[1,1,0]);
  const hostile=await readExecutionBinding(canary.hostile);
  assert.equal(hostile.exit_code,0);assert.deepEqual(hostile.args,["--test","scripts/tests/str1-evidence.test.mjs"]);
  assert.deepEqual(hostile.test_source,await fileBinding(path.join(sourceRoot,"scripts/tests/str1-evidence.test.mjs")));
  assert.deepEqual(hostile.node,canary.analysis.node);await readExecutionBinding(hostile.output);
}

export async function executeStr({binary,jobPath,directory,sourceRoot,env,canary=null}) {
  const jobBinding=await fileBinding(jobPath),job=await readExecutionBinding(jobBinding);
  const preparation=await readExecutionBinding(job.preparation);
  assertStrInputs(job,preparation);
  await authenticateGraphInputs(job,preparation);
  if(preparation.authority!=="synthetic_canary_only") {
    await validateStrCanary(canary,sourceRoot,await fileBinding(binary));
  }
  await mkdir(directory,{mode:0o700});
  const request={contract:"codestory.str1-execution-request/v1",job:jobBinding,canary,
    binary:await fileBinding(binary),cwd:await realpath(sourceRoot),environment:executionEnvironment(env),
    analysis:await strIdentity(sourceRoot),deadline_ms:1_800_000,
    inputs:await Promise.all([job.preparation,job.method,job.graph_inputs,job.graphs,job.vectors,job.control_run,...Object.values(job.graph_preparations??{})]
      .filter(Boolean).map(async binding=>{assert.deepEqual(await fileBinding(binding.path),binding);return binding;}))};
  request.args=["--job",jobPath];
  request.context_sha256=sha256(JSON.stringify({cwd:request.cwd,environment:request.environment}));
  const requestPath=path.join(directory,"request.json");await writeFile(requestPath,JSON.stringify(request),{flag:"wx",mode:0o600});
  const requestBinding=await fileBinding(requestPath);
  // Emitted before spawn so the owner can retain it independently of results.
  console.log(JSON.stringify({prelaunch_request:requestBinding}));
  const started=process.hrtime.bigint(),child=spawn(binary,request.args,{cwd:request.cwd,env:request.environment,stdio:["ignore","pipe","pipe"]});
  const stdout=[],stderr=[];child.stdout.on("data",b=>stdout.push(b));child.stderr.on("data",b=>stderr.push(b));
  let cancelled=false,killTimer;
  const cancel=()=>{cancelled=true;void writeFile(job.cancel_file,"cancel\n",{flag:"wx",mode:0o600}).catch(()=>{});
    killTimer??=setTimeout(()=>child.kill("SIGKILL"),5000);};
  process.once("SIGINT",cancel);process.once("SIGTERM",cancel);const timer=setTimeout(cancel,request.deadline_ms);
  const terminal=await new Promise(resolve=>{child.once("error",e=>resolve({exit_code:null,signal:null,error:e.message}));
    child.once("close",(code,signal)=>resolve({exit_code:code,signal}));});
  clearTimeout(timer);clearTimeout(killTimer);process.removeListener("SIGINT",cancel);process.removeListener("SIGTERM",cancel);
  const wall_ns=Number(process.hrtime.bigint()-started);
  for(const [name,bytes]of [["stdout",stdout],["stderr",stderr]])await writeFile(path.join(directory,`${name}.log`),Buffer.concat(bytes),{flag:"wx",mode:0o600});
  const output=await fileBinding(path.join(job.output,job.operation==="run"?"run.json":"graphs.json")).catch(()=>null);
  const events=job.operation==="run"?await fileBinding(path.join(job.output,"events.jsonl")).catch(()=>null):null;
  const completed=terminal.exit_code===0&&!terminal.signal&&!cancelled&&output&&(job.operation!=="run"||events);
  const receipt={contract:"codestory.str1-execution/v1",experiment_status:completed?"completed":"invalid",decision:"not_evaluated",
    request:requestBinding,...terminal,cancelled,wall_ns,output,events,
    stdout:await fileBinding(path.join(directory,"stdout.log")),stderr:await fileBinding(path.join(directory,"stderr.log"))};
  const receiptPath=path.join(directory,"receipt.json");await writeFile(receiptPath,JSON.stringify(receipt),{flag:"wx",mode:0o600});
  return {receipt,request:requestBinding,binding:await fileBinding(receiptPath)};
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  // Finish module evaluation before loading the validator, which imports this
  // module too. Awaiting that cycle at top level deadlocks the corpus gate.
  readFile(process.argv[2],"utf8").then(JSON.parse).then(executeStr).then(result=>{
    console.log(JSON.stringify(result.binding));
    if(result.receipt.experiment_status!=="completed")process.exitCode=1;
  }).catch(error=>{console.error(error.stack);process.exitCode=1;});
}
