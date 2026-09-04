import assert from "node:assert/strict";
import {readFile,writeFile} from "node:fs/promises";
import path from "node:path";
import {fileURLToPath} from "node:url";
import {validateStr1} from "./codestory-str1-validate.mjs";
import {readExecutionBinding,fileBinding} from "./lib/str1-execution.mjs";
import {evaluateArm,mean,percentile} from "./lib/etr1-evidence.mjs";
import {gateOne,gateTwo,reproduceOracleFixtures} from "./codestory-etr1-evaluate.mjs";

export function aggregateStrRows(rows) {
  const cases=[...new Set(rows.map(r=>r.case_id))].sort().map(id=>{
    const phrasings=rows.filter(r=>r.case_id===id),aggregate=name=>({recall:mean(phrasings.map(r=>r[name].recall)),
      complete_set_rate:mean(phrasings.map(r=>Number(r[name].complete_source_set)))});
    return {case_id:id,group:phrasings[0].group,control:aggregate("control"),candidate:aggregate("candidate"),
      control_incomplete_for_gain:phrasings.filter(r=>!r.control.complete_source_set).length>=2,
      candidate_gained_atom:phrasings.filter(r=>r.candidate.reachable_atoms.some(a=>!r.control.reachable_atoms.includes(a))).length>=2};
  });
  const groups=[...new Set(cases.map(c=>c.group))].sort(),aggregate=name=>({mean_recall:mean(cases.map(c=>c[name].recall)),
    complete_set_rate:mean(cases.map(c=>c[name].complete_set_rate)),groups:Object.fromEntries(groups.map(group=>[group,
      {mean_recall:mean(cases.filter(c=>c.group===group).map(c=>c[name].recall)),complete_set_rate:mean(cases.filter(c=>c.group===group).map(c=>c[name].complete_set_rate))}]))});
  const control=aggregate("control"),candidate=aggregate("candidate"),sufficiency=gateOne(candidate),material=gateTwo(cases,control,candidate,sufficiency.pass);
  const quality=sufficiency.pass&&material.pass;
  const latency=quality?{status:"evaluated",p95_ns:percentile(rows.map(r=>r.candidate.prepared_state_ns),.95)}:{status:"not_evaluated",p95_ns:null};
  latency.pass=quality?latency.p95_ns<=1_250_000_000:null;
  return {cases,aggregates:{control,candidate},gates:{sufficiency,material,latency},
    decision:quality&&latency.pass?"structural_frontier_selected":"no_frontier_selected",
    next:quality&&latency.pass?"prepare_separate_selector_contract_only":"stop_automatic_packet_program"};
}

export async function evaluateStr1({validation,annotations,oracle,sourceRoot}) {
  const prior=await readExecutionBinding(validation);assert.equal(prior.experiment_status,"valid");assert.equal(prior.annotation_access,"not_accessed");
  const checked=await validateStr1({execution:prior.execution,graphExecution:prior.graph_execution,controlValidation:prior.control_validation,
    controlSourceRoot:prior.control_source_root,reconstructionRoot:prior.reconstruction_root,sourceRoot});
  assert.deepEqual(checked.validation,prior,"validation receipt changed");
  // First annotations read, after full native/control reconstruction.
  const synthetic=checked.preparation.authority==="synthetic_canary_only";
  if(!synthetic)assert.equal(annotations.sha256,checked.preparation.annotations.sha256);
  const truth=await readExecutionBinding(annotations);
  assert.equal(truth.authority,synthetic?"synthetic_canary_only":"visible_development_only");
  const reproduction=synthetic?null:await reproduceOracleFixtures({oracle:await readExecutionBinding(oracle),preparation:checked.preparation,annotations:truth});
  const fragments=new Map(checked.preparation.fragments.map(f=>[f.fragment_id,f]));
  const rows=checked.run.rows.map(row=>{
    const task=truth.cases.find(c=>c.case_id===row.case_id),repository=checked.preparation.repositories.find(r=>r.repository_id===row.repository_id);
    assert.ok(task&&repository);const score=name=>({...evaluateArm(task,repository.fragment_ids.map(id=>fragments.get(id)),row[name].legally_selectable_pool,repository.base_serialized_bytes),prepared_state_ns:row[name].timing.prepared_state_ns});
    return {case_id:row.case_id,phrasing_id:row.phrasing_id,group:row.group,control:score("control"),candidate:score("candidate")};
  });
  if(synthetic)return {contract:"codestory.str1-evaluation/v1",authority:"synthetic_canary_only",experiment_status:"valid",decision:"not_evaluated",rows};
  assert.equal(rows.length,72);assert.equal(truth.questions_sha256,checked.preparation.fixed_inputs.questions.sha256);
  for(const id of new Set(rows.map(r=>r.case_id)))assert.deepEqual(rows.filter(r=>r.case_id===id).map(r=>r.phrasing_id).sort(),["original","paraphrase_1","paraphrase_2"]);
  return {contract:"codestory.str1-evaluation/v1",authority:"visible_development_frontier_only",experiment_status:"valid",packet_decision:"not_evaluated",
    inputs:{validation,annotations,oracle},oracle_reproduction:reproduction,...aggregateStrRows(rows),rows};
}
if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const config=JSON.parse(await readFile(process.argv[2],"utf8"));let report;
  try{report=await evaluateStr1(config);}catch(error){report={contract:"codestory.str1-evaluation/v1",experiment_status:"invalid",decision:"not_evaluated",error:error.message};process.exitCode=1;}
  await writeFile(config.output,JSON.stringify(report),{flag:"wx",mode:0o600});console.log(JSON.stringify(await fileBinding(config.output)));
}
