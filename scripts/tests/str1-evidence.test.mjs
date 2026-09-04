import test from "node:test";
import assert from "node:assert/strict";
import { structuralFrontier, validateStructuralGraph } from "../lib/str1-evidence.mjs";

const fixture = () => ({
  nodes: [
    { id: 1, fragment_ids: ["seed"] },
    ...Array.from({ length: 12 }, (_, i) => ({ id: i + 2, fragment_ids: [`f${String(i).padStart(2, "0")}`] })),
    { id: 90, fragment_ids: ["second-hop"] }, { id: 91, fragment_ids: [] },
  ],
  relations: [
    ...Array.from({ length: 12 }, (_, i) => ({ id: i + 20, source: i % 2 ? i + 2 : 1,
      target: i % 2 ? 1 : i + 2, certainty: "Certain", occurrence: { path: "a.rs", start_line: 1, end_line: 1,
        content_digest: "a".repeat(64), source: "call();\n" }, occurrence_fragment_ids: ["seed"] })),
    { id: 80, source: 2, target: 90, certainty: "Certain", occurrence: { path: "a.rs", start_line: 2, end_line: 2,
      content_digest: "a".repeat(64), source: "later();\n" }, occurrence_fragment_ids: ["f00"] },
    { id: 81, source: 1, target: 91, certainty: "Certain", occurrence: { path: "a.rs", start_line: 1, end_line: 1,
      content_digest: "a".repeat(64), source: "call();\n" }, occurrence_fragment_ids: ["seed"] },
  ],
});

test("one-hop discovery is directed in provenance, symmetric in eligibility, and bounded after exclusions", () => {
  const graph = fixture();
  const scores = new Map(graph.nodes.flatMap((node) => node.fragment_ids).map((id) => [id, 1]));
  const first = structuralFrontier(graph, ["seed"], scores);
  assert.deepEqual(first.successors, Array.from({ length: 8 }, (_, i) => `f0${i}`));
  assert.ok(!first.successors.includes("second-hop"));
  assert.ok(!first.steps[0].eligible.includes("second-hop"), "an observed edge must not promote its remote endpoint to a new traversal root");
  assert.ok(first.steps[0].relations.some((edge) => edge.source !== 1));
  assert.ok(first.steps[0].boundary_gaps.some((gap) => gap.node_id === 91));
  const paired = structuralFrontier(graph, ["seed", "f00"], scores);
  assert.equal(new Set(paired.successors).size, paired.successors.length);
  assert.ok(!paired.successors.includes("f00"));
  assert.ok(paired.successors.includes("second-hop"), "only an original second seed opens its neighborhood");
});

test("uncertain, unwitnessed, duplicate and nonfinite graph inputs cannot gain authority", () => {
  for (const mutate of [
    (g) => { g.relations[0].certainty = "Probable"; },
    (g) => { g.relations[0].occurrence = null; },
    (g) => { g.relations[0].target = 9999; },
    (g) => { g.relations.push(g.relations[0]); },
    (g) => { g.relations[0].occurrence.source = ""; },
  ]) { const graph = fixture(); mutate(graph); assert.throws(() => validateStructuralGraph(graph)); }
  assert.throws(() => structuralFrontier(fixture(), ["seed"], new Map([["f00", NaN]])));
});

test("empty and naturally underfilled frontiers are retained; IDs alone break score ties", () => {
  assert.deepEqual(structuralFrontier(fixture(), [], new Map()).successors, []);
  const graph = fixture(); graph.relations = graph.relations.slice(0, 2);
  const scores = new Map([["seed", 0], ["f00", 0.4], ["f01", 0.8]]);
  assert.deepEqual(structuralFrontier(graph, ["seed"], scores).successors, ["f01", "f00"]);
  assert.deepEqual(structuralFrontier(graph, ["seed"], new Map([["seed", 0], ["f00", 1], ["f01", 1]])).successors,
    ["f00", "f01"]);
});
