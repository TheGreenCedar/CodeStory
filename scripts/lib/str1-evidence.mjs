import assert from "node:assert/strict";

export function validateStructuralGraph(graph) {
  const nodes = new Map(graph.nodes.map((node) => [node.id, node]));
  assert.equal(nodes.size, graph.nodes.length, "duplicate node identity");
  const edges = new Set();
  for (const edge of graph.relations) {
    assert.ok(!edges.has(edge.id), "duplicate relationship identity"); edges.add(edge.id);
    assert.equal(edge.certainty, "Certain", "uncertain relationship");
    assert.ok(nodes.has(edge.source) && nodes.has(edge.target), "missing effective endpoint");
    const occurrence = edge.occurrence;
    assert.ok(occurrence && occurrence.path && occurrence.source.trim(), "missing positive occurrence");
    assert.match(occurrence.content_digest, /^[0-9a-f]{64}$/u);
    assert.ok(occurrence.start_line > 0 && occurrence.end_line >= occurrence.start_line);
  }
}

export function structuralFrontier(graph, seeds, scores) {
  validateStructuralGraph(graph);
  assert.ok(seeds.length <= 16 && new Set(seeds).size === seeds.length, "seed budget/identity");
  const nodes = new Map(graph.nodes.map((node) => [node.id, node]));
  const prior = new Set(), steps = [];
  for (const seed of seeds) {
    const anchors = new Set(graph.nodes.filter((node) => node.fragment_ids.includes(seed)).map((node) => node.id));
    const relations = graph.relations.filter((edge) => anchors.has(edge.source) || anchors.has(edge.target)
      || edge.occurrence_fragment_ids.includes(seed));
    const eligible = new Set(), gaps = [];
    for (const edge of relations) {
      for (const nodeId of [edge.source, edge.target]) {
        const node = nodes.get(nodeId);
        if (!node.fragment_ids.length) gaps.push({ relation_id: edge.id, node_id: nodeId, kind: "endpoint_outside_fragment_universe" });
        node.fragment_ids.forEach((id) => eligible.add(id));
      }
      if (!edge.occurrence_fragment_ids.length) gaps.push({ relation_id: edge.id, kind: "occurrence_outside_fragment_universe" });
      edge.occurrence_fragment_ids.forEach((id) => eligible.add(id));
    }
    const excluded = [...new Set([...seeds, ...prior])].sort();
    const ranked = [...eligible].filter((id) => !excluded.includes(id));
    for (const id of ranked) assert.ok(Number.isFinite(scores.get(id)), "missing/nonfinite similarity");
    ranked.sort((a, b) => scores.get(b) - scores.get(a) || (a < b ? -1 : a > b ? 1 : 0));
    const selected = ranked.slice(0, 8);
    selected.forEach((id) => prior.add(id));
    steps.push({ seed_fragment_id: seed, anchors: [...anchors].sort((a, b) => a - b),
      relations, eligible: [...eligible].sort(), excluded_before: excluded,
      retained_successors: selected, boundary_gaps: gaps });
  }
  return { successors: [...prior], steps };
}
