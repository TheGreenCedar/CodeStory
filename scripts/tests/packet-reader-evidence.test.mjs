import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fragmentId, sha256 } from "../lib/etr1-evidence.mjs";
import { sourcePacket, referencePacket, readerPrompt, validateReaderAnswer,
  validateReaderEvents } from "../lib/packet-reader-evidence.mjs";
import { readerArgs, readerProcess, readerEnvironment, prepareReader, validateCanary } from "../codestory-packet-reader.mjs";

const publication = { project_id: "project", core_generation_id: "core", retrieval_generation: "retrieval" };
function fixture(source = "first();\nsecond();\n", file = "src/a.rs") {
  const bytes = Buffer.from(source);
  const fragment = { project_id: "project", path: file, content_digest: sha256(bytes),
    byte_range: { start: 0, end: bytes.length }, line_range: { start: 1, end: 2 }, source };
  fragment.fragment_id = fragmentId(fragment);
  return { fragment, sources: new Map([[file, bytes]]) };
}
const answer = { claims: [{ text: "The source calls first, then second.",
  citations: [{ path: "src/a.rs", start_line: 1, end_line: 2 }] }], limitations: [] };

test("source packet authenticates exact bytes and carries no oracle authority", () => {
  const { fragment, sources } = fixture();
  const packet = sourcePacket([fragment], publication, sources);
  assert.equal(packet.answer_sufficiency, "not_asserted");
  assert.equal(packet.support[0].kind, "source_range");
  assert.equal(packet.support[0].start_line, 1);
  assert.ok(packet.support[0].snippet.includes("first();"));
  assert.deepEqual(Object.keys(packet).sort(),
    ["answer_sufficiency", "continuation", "publication", "support"]);
  validateReaderAnswer(answer, packet);
});

test("source authentication refuses the complete identity/range/publication mutation class", () => {
  const { fragment, sources } = fixture();
  for (const mutate of [
    f => { f.source = "fabricated"; },
    f => { f.path = "../a.rs"; },
    f => { f.project_id = "another"; f.fragment_id = fragmentId(f); },
    f => { f.byte_range.end--; f.fragment_id = fragmentId(f); },
    f => { f.line_range.end++; },
    f => { f.content_digest = "0".repeat(64); },
  ]) {
    const changed = structuredClone(fragment); mutate(changed);
    assert.throws(() => sourcePacket([changed], publication, sources));
  }
  assert.throws(() => sourcePacket([fragment, fragment], publication, sources), /duplicate/);
});

test("reference selection is answer-aware privately, but the reader sees source only", () => {
  const { fragment, sources } = fixture();
  const annotation = { acceptable_sets: [{ set_id: "PRIVATE_ORACLE",
    required_source_atoms: [{ atom_id: "PRIVATE_ANSWER", source_range: {
      path: fragment.path, content_digest: fragment.content_digest,
      byte_range: fragment.byte_range, line_range: fragment.line_range } }],
    required_relation_atoms: [] }] };
  const result = referencePacket(annotation, [fragment], publication, sources);
  assert.equal(result.selection.complete_source_set, true);
  const prompt = readerPrompt("What happens here?", result.packet);
  assert.ok(prompt.includes("What happens here?"));
  assert.ok(!prompt.includes("PRIVATE_"));
  assert.ok(!prompt.includes("complete_source_set"));
});

test("public bytes, metadata and row limit are enforced before reader exposure", () => {
  const { fragment, sources } = fixture();
  assert.throws(() => sourcePacket([fragment], { ...publication, expected_answer: "PRIVATE_ORACLE" }, sources),
    /unexpected reader field/);
  const packet = sourcePacket([fragment], publication, sources);
  packet.publication.expected_answer = "PRIVATE_ORACLE";
  assert.throws(() => readerPrompt("question", packet), /unexpected reader field/);
  const source = "x".repeat(17000) + "\nend\n";
  const giant = fixture(source);
  assert.throws(() => sourcePacket([giant.fragment], publication, giant.sources), /budget/);
});

test("citations cannot escape packet ranges, paths or claim schema", () => {
  const { fragment, sources } = fixture(), packet = sourcePacket([fragment], publication, sources);
  for (const mutate of [
    a => { a.claims[0].citations[0].path = "other/a.rs"; },
    a => { a.claims[0].citations[0].end_line = 3; },
    a => { a.claims[0].citations = []; },
    a => { a.claims[0].citations[0].start_line = 1.5; },
    a => { a.contract_proven = true; },
  ]) { const changed = structuredClone(answer); mutate(changed);
    assert.throws(() => validateReaderAnswer(changed, packet)); }
  validateReaderAnswer({ claims: [], limitations: ["The supplied source does not establish an answer."] }, packet);
});

test("only a completed no-tool reader turn can be scored", () => {
  const { fragment, sources } = fixture(), packet = sourcePacket([fragment], publication, sources);
  const events = [{ type: "thread.started", thread_id: "t" }, { type: "turn.started" },
    { type: "item.completed", item: { id: "i", type: "agent_message", text: JSON.stringify(answer) } },
    { type: "turn.completed", usage: { input_tokens: 100, output_tokens: 20 } }];
  assert.deepEqual(validateReaderEvents(events, packet), answer);
  for (const itemType of ["command_execution", "mcp_tool_call", "web_search", "collab_tool_call"]) {
    const changed = structuredClone(events);
    changed.splice(2, 0, { type: "item.started", item: { id: "tool", type: itemType } });
    assert.throws(() => validateReaderEvents(changed, packet), /tool|item/);
  }
  assert.throws(() => validateReaderEvents(events.slice(0, -1), packet), /complete/);
  assert.throws(() => validateReaderEvents([...events, { type: "turn.failed", error: {} }], packet));
});

test("reader invocation excludes repository instructions, tools and persisted sessions", () => {
  const args = readerArgs("fixed-model", "/empty", "/schema");
  for (const flag of ["--ignore-user-config", "--ignore-rules", "--ephemeral", "--output-schema"])
    assert.ok(args.includes(flag));
  for (const feature of ["shell_tool", "apps", "plugins", "multi_agent", "hooks", "memories"])
    assert.equal(args[args.indexOf(feature) - 1], "--disable");
  assert.ok(args.includes('web_search="disabled"'));
  assert.equal(args.at(-1), "-");
});

test("reader process preserves split UTF-8 and refuses deadline or cancellation", async () => {
  const split = await readerProcess(process.execPath, ["-e",
    "process.stdout.write(Buffer.from([0xe2]));setTimeout(()=>process.stdout.write(Buffer.from([0x82,0xac])),20)"], "");
  assert.equal(split.stdout, "€");
  assert.equal(split.exit_code, 0);
  const deadline = await readerProcess(process.execPath, ["-e", "setInterval(()=>{},100)"], "", { timeoutMs: 50 });
  assert.equal(deadline.failure, "reader_deadline_exceeded");
  const controller = new AbortController();
  const pending = readerProcess(process.execPath, ["-e", "setInterval(()=>{},100)"], "", { signal: controller.signal });
  controller.abort();
  assert.equal((await pending).failure, "reader_cancelled");
});

test("the exact serialized limit includes metadata and admits sixteen unique rows", () => {
  const fragments = [], sources = new Map();
  for (let i = 0; i < 17; i++) {
    const f = fixture("first();\nsecond();\n", `src/${i}.rs`);
    fragments.push(f.fragment); for (const entry of f.sources) sources.set(...entry);
  }
  const pin = { ...publication };
  const packet = sourcePacket(fragments.slice(0, 16), pin, sources);
  pin.core_generation_id += "x".repeat(16384 - Buffer.byteLength(JSON.stringify(packet)));
  assert.equal(Buffer.byteLength(JSON.stringify(sourcePacket(fragments.slice(0, 16), pin, sources))), 16384);
  assert.throws(() => sourcePacket(fragments.slice(0, 16), { ...pin, core_generation_id: pin.core_generation_id + "x" }, sources));
  assert.throws(() => sourcePacket(fragments, publication, sources), /row budget/);
});

test("synthetic caller inputs cannot mint frozen visible-corpus authority", async () => {
  await assert.rejects(prepareReader({ preparationDigest: "0".repeat(64),
    annotationsDigest: "1".repeat(64), questionsDigest: "2".repeat(64) }), /independently frozen/);
});

test("reader children execute the captured environment and directory only", async () => {
  const env = readerEnvironment();
  assert.ok(Object.keys(env).every(k => ["HOME", "PATH", "TMPDIR", "LANG", "LC_ALL", "CODEX_HOME", "__CF_USER_TEXT_ENCODING"].includes(k)));
  const captured = { READER_TEST_VALUE: "captured" };
  const result = await readerProcess(process.execPath, ["-e",
    "process.stdout.write(JSON.stringify({cwd:process.cwd(),env:process.env}))"], "",
  { cwd: "/private/tmp", env: captured });
  const actual = JSON.parse(result.stdout);
  // CoreFoundation can install its native encoding after process launch; it is not inherited task configuration.
  if (process.platform === "darwin") delete actual.env.__CF_USER_TEXT_ENCODING;
  assert.deepEqual(actual, { cwd: "/private/tmp", env: captured });
});

test("corpus canary authority binds status, source, executable and model", async t => {
  const directory = await mkdtemp(path.join(tmpdir(), "packet-reader-canary-test-"));
  t.after(() => rm(directory, { recursive: true }));
  const build = { commit: "commit", tree: "tree", status: "" }, binary = { path: "/reader", sha256: "digest" };
  const receipt = { contract: "codestory.packet-reader-run/v1", authority: "synthetic_canary_only",
    experiment_status: "execution_valid", build, reader_binary: binary, model: "fixed", rows: [{}] };
  for (const [index, mutate] of [
    r => { r.authority = "visible_reference_diagnostic_only"; },
    r => { r.experiment_status = "invalid"; },
    r => { r.build.commit = "other"; },
    r => { r.reader_binary.sha256 = "other"; },
    r => { r.model = "other"; },
  ].entries()) {
    const changed = structuredClone(receipt); mutate(changed);
    const bytes = JSON.stringify(changed), file = path.join(directory, `${index}.json`);
    await writeFile(file, bytes);
    await assert.rejects(validateCanary(file, sha256(bytes), build, binary, "fixed"));
  }
});
