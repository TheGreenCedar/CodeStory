import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile, writeFile, mkdtemp } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { validateStrExecution, readExecutionBinding, fileBinding, strIdentity, assertStrInputs } from "./lib/str1-execution.mjs";
import { structuralFrontier, validateStructuralGraph } from "./lib/str1-evidence.mjs";
import { authenticateFragment, f32Dot, sha256, validateVector } from "./lib/etr1-evidence.mjs";
import { parseEvents, validateEngine, validateEtr1 } from "./codestory-etr1-validate.mjs";

export async function validateStr1({execution,executionRequest,graphExecution,graphRequest,sourceRoot,controlValidation,controlSourceRoot,reconstructionRoot}) {
  const external=await validateStrExecution(execution,sourceRoot,executionRequest);
  const exported=await validateStrExecution(graphExecution,sourceRoot,graphRequest);
  const run=await readExecutionBinding(external.receipt.output),graphOutput=await readExecutionBinding(exported.receipt.output);
  assert.equal(run.contract,"codestory.str1-run/v1");assert.equal(graphOutput.contract,"codestory.str1-graphs/v1");
  assert.equal(run.annotation_access,"not_accessed");assert.equal(graphOutput.annotation_access,"not_accessed");
  assert.equal(run.experiment_status,"awaiting_validation");assert.equal(run.decision,"not_evaluated");
  assert.deepEqual(run.job,external.request.job);assert.deepEqual(graphOutput.job,exported.request.job);
  assert.deepEqual(run.graphs,exported.receipt.output);
  assert.deepEqual(run.preparation,external.job.preparation);assert.deepEqual(graphOutput.preparation,run.preparation);
  for(const item of [run,graphOutput]) {
    assert.equal(item.build.source_commit,external.request.analysis.source_commit);
    assert.equal(item.build.source_tree,external.request.analysis.source_tree);
    assert.equal(item.build.source_dirty,false);assert.equal(item.build.binary_sha256,external.request.binary.sha256);
    assert.deepEqual(item.method,external.job.method);
  }
  const old=await readExecutionBinding(controlValidation);
  assert.equal(old.experiment_status,"valid");assert.equal(old.decision,"not_evaluated");
  const controls=await validateEtr1({runBinding:old.run,sourceRoot:controlSourceRoot,executionBinding:old.execution,
    allowCanary:old.authority==="synthetic_canary_only"});
  assert.deepEqual(run.control_run,old.run);assert.deepEqual(run.preparation,controls.run.preparation);
  assert.deepEqual(run.vectors,controls.run.fragment_vectors);
  assertStrInputs(external.job,controls.preparation,controlValidation);
  // Re-export through the pinned read-session path; don't trust graph JSON
  // merely because someone also updated its digest.
  const reconstruction=await mkdtemp(path.join(reconstructionRoot,"str1-graph-reconstruction-"));
  const replayJob={...exported.job,output:path.join(reconstruction,"output"),cancel_file:path.join(reconstruction,"cancel")};
  const replayPath=path.join(reconstruction,"job.json");await writeFile(replayPath,JSON.stringify(replayJob),{flag:"wx",mode:0o600});
  execFileSync(external.request.binary.path,["--job",replayPath],{cwd:sourceRoot,env:external.request.environment,timeout:120_000,stdio:"pipe"});
  const rebuilt=JSON.parse(await readFile(path.join(replayJob.output,"graphs.json"),"utf8"));
  assert.deepEqual(rebuilt.graphs,graphOutput.graphs,"native graph reconstruction differs");
  graphOutput.graphs.forEach(validateStructuralGraph);
  const preparation=controls.preparation,fragments=new Map(preparation.fragments.map(f=>[f.fragment_id,f]));
  const vectors=await readExecutionBinding(run.vectors),vectorMap=new Map(vectors.records.map(r=>[r.id,r.vector]));
  const eventsBytes=await readFile(external.receipt.events.path);
  assert.equal(sha256(eventsBytes),run.events_sha256);
  const events=parseEvents(eventsBytes);let eventOrdinal=0,totalWall=0;
  validateEngine(run.initial_engine);validateEngine(run.final_engine);
  for(const key of ["server_instance_id","load_generation","model_digest","ggml_build_identity"])
    assert.deepEqual(run.initial_engine[key],run.final_engine[key]);
  for(const key of ["model_digest","ggml_build_identity"]) {
    assert.deepEqual(run.initial_engine[key],controls.run.initial_engine[key]);
    assert.deepEqual(run.initial_engine[key],vectors.initial_engine[key]);
  }
  assert.equal(run.rows.length,preparation.wordings.length);
  for(let index=0;index<run.rows.length;index++) {
    const row=run.rows[index],wording=preparation.wordings[index],controlRow=controls.rows[index];
    for(const key of ["case_id","phrasing_id","repository_id","group","question_sha256"])
      assert.deepEqual(row[key],wording[key]);
    assert.deepEqual(row.control_row,controls.run.rows[index],"frozen control binding changed");
    assert.equal(Object.hasOwn(row,"control"),false,"control must remain an immutable reference");
    // Join only after authenticating the original row. Never republish its floats.
    row.control=controlRow.control;
    assert.deepEqual(row.seed_fragment_ids,wording.seed_fragment_ids);
    const candidate=row.candidate,repository=preparation.repositories.find(r=>r.repository_id===row.repository_id);
    const graph=graphOutput.graphs.find(g=>g.repository_id===row.repository_id);
    assert.equal(candidate.query_input,wording.question,"raw query changed");
    const seeded=wording.seed_fragment_ids.length>0;
    assert.equal(candidate.batch_receipts.length,seeded?1:0);
    assert.equal(candidate.scores.length,seeded?repository.fragment_ids.length:0);
    if(seeded) {
      validateVector(candidate.query_vector);
      candidate.scores.forEach((score,i)=>assert.ok(Math.abs(score-f32Dot(candidate.query_vector,vectorMap.get(repository.fragment_ids[i])))<=2e-6,"structural similarity differs"));
      const batch=candidate.batch_receipts[0],event=events[eventOrdinal];
      assert.ok(event,"native completion omitted");
      assert.equal(batch.global_batch_ordinal,eventOrdinal++);assert.equal(batch.arm,"structural");
      assert.deepEqual(batch.query_ordinals,[0]);assert.deepEqual(batch.input_sha256,[sha256(wording.question)]);
      assert.equal(batch.completed_tokens,Number(event.details.completed_tokens));
      assert.equal(batch.qualification_native_completion_sequence,Number(event.details.native_completion_sequence));
      assert.equal(batch.qualification_server_event_sequence,event.server_event_sequence);
      assert.equal(batch.qualification_request_id_sha256,sha256(event.details.request_id));
      const baseline=row.control.batch_receipts[0];
      assert.equal(batch.completed_tokens*baseline.query_ordinals.length,baseline.completed_tokens,"raw query tokenization changed");
    } else assert.deepEqual(candidate.query_vector,[]);
    const scoreMap=new Map(repository.fragment_ids.map((id,i)=>[id,candidate.scores[i]]));
    const expected=structuralFrontier(graph,wording.seed_fragment_ids,scoreMap);
    assert.deepEqual(candidate.steps,expected.steps,"structural one-hop receipts differ");
    assert.deepEqual(candidate.successors,expected.successors,"structural successor selection differs");
    const pool=[...wording.seed_fragment_ids,...expected.successors];
    assert.ok(pool.length<=144&&new Set(pool).size===pool.length);
    assert.deepEqual(candidate.descriptor_pool,pool);assert.deepEqual(candidate.hydrated_pool,pool);
    assert.deepEqual(candidate.legally_selectable_pool,pool.filter(id=>repository.base_serialized_bytes+fragments.get(id).serialized_row_bytes<=16384));
    const sourceFiles=new Map();let sourceBytes=0;
    for(const id of pool) {
      const fragment=fragments.get(id);assert.equal(fragment.project_id,repository.project_id);
      if(!sourceFiles.has(fragment.path))sourceFiles.set(fragment.path,await readFile(path.join(repository.local_root,fragment.path)));
      authenticateFragment(fragment,sourceFiles.get(fragment.path));sourceBytes+=Buffer.byteLength(fragment.source);
    }
    assert.deepEqual(candidate.source_authentication.authenticated_fragment_ids,pool);
    assert.equal(candidate.source_authentication.fragment_source_bytes,sourceBytes);
    assert.equal(candidate.source_authentication.filesystem_bytes_read,[...sourceFiles.values()].reduce((s,b)=>s+b.length,0));
    assert.deepEqual(candidate.source_authentication.file_digests,Object.fromEntries([...sourceFiles].map(([p,b])=>[p,sha256(b)])));
    const {prepared_state_ns,unaccounted_ns,...phases}=candidate.timing;
    for(const value of Object.values(candidate.timing))assert.ok(Number.isSafeInteger(value)&&value>=0,"invalid timing interval");
    assert.equal(Object.values(phases).reduce((a,b)=>a+b,0)+unaccounted_ns,prepared_state_ns);
    totalWall+=prepared_state_ns;
  }
  assert.equal(eventOrdinal,events.length);assert.ok(totalWall<=external.receipt.wall_ns,"request wall exceeds process wall");
  return {run,preparation,graphs:graphOutput.graphs,validation:{contract:"codestory.str1-validation/v1",
    experiment_status:"valid",decision:"not_evaluated",annotation_access:"not_accessed",authority:preparation.authority,
    execution,execution_request:executionRequest,graph_execution:graphExecution,graph_request:graphRequest,control_validation:controlValidation,control_source_root:controlSourceRoot,
    reconstruction_root:reconstructionRoot,run:external.receipt.output,analysis:await strIdentity(sourceRoot),
    prepared_state_ns:totalWall,execution_wall_ns:external.receipt.wall_ns,outer_remainder_ns:external.receipt.wall_ns-totalWall}};
}
async function main() {
  const config=JSON.parse(await readFile(process.argv[2],"utf8"));let report;
  try{report=(await validateStr1(config)).validation;}catch(error){report={contract:"codestory.str1-validation/v1",experiment_status:"invalid",decision:"not_evaluated",error:error.message};process.exitCode=1;}
  await writeFile(config.output,JSON.stringify(report),{flag:"wx",mode:0o600});console.log(JSON.stringify(await fileBinding(config.output)));
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url))
  main().catch(error=>{console.error(error.stack);process.exitCode=1;});
