import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile, realpath } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { analysisIdentity, executionEnvironment, fileBinding, readExecutionBinding } from "./etr1-execution.mjs";
export { fileBinding, readExecutionBinding };
export const STR_FILES = ["scripts/lib/str1-evidence.mjs", "scripts/lib/str1-execution.mjs",
  "scripts/codestory-str1-validate.mjs", "scripts/codestory-str1-evaluate.mjs", "scripts/codestory-str1-canary.mjs"];
export async function strIdentity(root) {
  return { ...await analysisIdentity(root), structural_files: await Promise.all(STR_FILES.map(f=>fileBinding(path.join(root,f)))) };
}
export async function validateStrExecution(binding, sourceRoot) {
  const receipt=await readExecutionBinding(binding), request=await readExecutionBinding(receipt.request);
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
  for(const input of [job.preparation,job.method,job.graphs,job.vectors,job.control_run,...Object.values(job.graph_preparations??{})].filter(Boolean))
    assert.ok(request.inputs.some(x=>JSON.stringify(x)===JSON.stringify(input)),"unbound execution input");
  assert.deepEqual(receipt.output,await fileBinding(receipt.output.path));
  assert.equal(receipt.output.path,path.join(job.output,job.operation==="run"?"run.json":"graphs.json"));
  for(const stream of [receipt.stdout,receipt.stderr,receipt.events].filter(Boolean)) assert.deepEqual(stream,await fileBinding(stream.path));
  if(job.operation==="run") assert.ok(receipt.events,"native events missing");
  const preparation=await readExecutionBinding(job.preparation);
  if(preparation.authority!=="synthetic_canary_only") {
    const canary=await readExecutionBinding(request.canary);
    assert.equal(canary.contract,"codestory.str1-canary/v1");
    assert.equal(canary.experiment_status,"valid");
    assert.deepEqual(canary.analysis,request.analysis);
    assert.deepEqual(canary.binary,request.binary);
    const validation=await readExecutionBinding(canary.validation);
    assert.equal(validation.authority,"synthetic_canary_only");
    assert.equal(validation.experiment_status,"valid");
  }
  assert.ok(Number.isSafeInteger(receipt.wall_ns)&&receipt.wall_ns>0);
  return {request,receipt,job};
}

export async function executeStr({binary,jobPath,directory,sourceRoot,env,canary=null}) {
  const jobBinding=await fileBinding(jobPath),job=await readExecutionBinding(jobBinding);
  const preparation=await readExecutionBinding(job.preparation);
  if(preparation.authority!=="synthetic_canary_only") {
    assert.ok(canary,"real canary required before corpus execution");
    const value=await readExecutionBinding(canary);
    assert.equal(value.experiment_status,"valid");
    assert.deepEqual(value.analysis,await strIdentity(sourceRoot));
    assert.deepEqual(value.binary,await fileBinding(binary));
  }
  await mkdir(directory,{mode:0o700});
  const request={contract:"codestory.str1-execution-request/v1",job:jobBinding,canary,
    binary:await fileBinding(binary),cwd:await realpath(sourceRoot),environment:executionEnvironment(env),
    analysis:await strIdentity(sourceRoot),deadline_ms:1_800_000,
    inputs:await Promise.all([job.preparation,job.method,job.graphs,job.vectors,job.control_run,...Object.values(job.graph_preparations??{})]
      .filter(Boolean).map(async binding=>{assert.deepEqual(await fileBinding(binding.path),binding);return binding;}))};
  const requestPath=path.join(directory,"request.json");await writeFile(requestPath,JSON.stringify(request),{flag:"wx",mode:0o600});
  const started=process.hrtime.bigint(),child=spawn(binary,["--job",jobPath],{cwd:request.cwd,env:request.environment,stdio:["ignore","pipe","pipe"]});
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
    request:await fileBinding(requestPath),...terminal,cancelled,wall_ns,output,events,
    stdout:await fileBinding(path.join(directory,"stdout.log")),stderr:await fileBinding(path.join(directory,"stderr.log"))};
  const receiptPath=path.join(directory,"receipt.json");await writeFile(receiptPath,JSON.stringify(receipt),{flag:"wx",mode:0o600});
  return {receipt,binding:await fileBinding(receiptPath)};
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const config=JSON.parse(await readFile(process.argv[2],"utf8"));
  const result=await executeStr(config);console.log(JSON.stringify(result.binding));
  if(result.receipt.experiment_status!=="completed")process.exitCode=1;
}
