-- Diagnostic only. Rust owns the authoritative checked-u128 count.
--
-- Schema assumptions and semantics:
--   edge.kind = ?1 identifies CALL rows;
--   effective endpoints are COALESCE(resolved_*, raw_*), preserving unresolved
--   target placeholders in the effective set;
--   exact-resolved requires resolved_target_node_id IS NOT NULL;
--   proof_admitted_edge is populated from the actual Task 6 Rust kernel;
--   trails are connected by effective target -> effective source;
--   vertices may repeat, including self edges;
--   the comma-delimited used_edge_ids encoding prevents one edge ID from being
--   reused while retaining parallel rows with distinct IDs;
--   lengths are exactly 1 through 6.
WITH RECURSIVE
call_edges AS (
    SELECT
        id,
        COALESCE(resolved_source_node_id, source_node_id) AS effective_source,
        COALESCE(resolved_target_node_id, target_node_id) AS effective_target,
        resolved_target_node_id
    FROM edge
    WHERE kind = ?1
),
edge_sets(edge_set, edge_id, source_id, target_id) AS (
    SELECT 'effective_endpoint', id, effective_source, effective_target
    FROM call_edges
    UNION ALL
    SELECT 'exact_resolved', id, effective_source, effective_target
    FROM call_edges
    WHERE resolved_target_node_id IS NOT NULL
    UNION ALL
    SELECT 'strictly_admitted', call_edges.id, effective_source, effective_target
    FROM call_edges
    JOIN proof_admitted_edge ON proof_admitted_edge.edge_id = call_edges.id
),
walks(edge_set, length, target_id, used_edge_ids) AS (
    SELECT
        edge_set,
        1,
        target_id,
        printf(',%d,', edge_id)
    FROM edge_sets
    UNION ALL
    SELECT
        walks.edge_set,
        walks.length + 1,
        edge_sets.target_id,
        walks.used_edge_ids || printf('%d,', edge_sets.edge_id)
    FROM walks
    JOIN edge_sets
      ON edge_sets.edge_set = walks.edge_set
     AND edge_sets.source_id = walks.target_id
    WHERE walks.length < 6
      AND instr(walks.used_edge_ids, printf(',%d,', edge_sets.edge_id)) = 0
),
lengths(length) AS (
    VALUES (1), (2), (3), (4), (5), (6)
),
set_names(edge_set) AS (
    VALUES ('effective_endpoint'), ('exact_resolved'), ('strictly_admitted')
)
SELECT set_names.edge_set, lengths.length, COUNT(walks.length) AS count
FROM set_names
CROSS JOIN lengths
LEFT JOIN walks
  ON walks.edge_set = set_names.edge_set
 AND walks.length = lengths.length
GROUP BY set_names.edge_set, lengths.length
ORDER BY set_names.edge_set, lengths.length;
