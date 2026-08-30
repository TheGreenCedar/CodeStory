-- Diagnostic only. The Rust Store scan and Task 6 admission leaf are authoritative.
--
-- Schema assumptions:
--   edge.kind = ?1 identifies CALL rows;
--   source_node_id and target_node_id are non-null stored placeholders;
--   effective endpoints are COALESCE(resolved_*, raw_*);
--   an exact-resolved row has a non-null resolved_target_node_id;
--   a null resolved_target_node_id remains an unresolved placeholder row;
--   proof_admitted_edge is a TEMP table populated from the actual Rust kernel,
--   so this query never reimplements the strict admission predicate.
WITH call_edges AS (
    SELECT
        id,
        source_node_id,
        target_node_id,
        resolved_source_node_id,
        resolved_target_node_id,
        certainty
    FROM edge
    WHERE kind = ?1
),
metrics(metric, count) AS (
    SELECT 'stored_call_rows', COUNT(*) FROM call_edges
    UNION ALL
    SELECT 'effective_endpoint_rows', COUNT(*)
    FROM call_edges
    WHERE COALESCE(resolved_source_node_id, source_node_id) IS NOT NULL
      AND COALESCE(resolved_target_node_id, target_node_id) IS NOT NULL
    UNION ALL
    SELECT 'exact_resolved_rows', COUNT(*)
    FROM call_edges WHERE resolved_target_node_id IS NOT NULL
    UNION ALL
    SELECT 'admitted_rows', COUNT(*)
    FROM call_edges JOIN proof_admitted_edge ON proof_admitted_edge.edge_id = call_edges.id
    UNION ALL
    SELECT 'unresolved_placeholder_rows', COUNT(*)
    FROM call_edges WHERE resolved_target_node_id IS NULL
    UNION ALL
    SELECT 'certainty_absent_rows', COUNT(*) FROM call_edges WHERE certainty IS NULL
    UNION ALL
    SELECT 'certainty_certain_rows', COUNT(*) FROM call_edges WHERE certainty = 'certain'
    UNION ALL
    SELECT 'certainty_probable_rows', COUNT(*) FROM call_edges WHERE certainty = 'probable'
    UNION ALL
    SELECT 'certainty_uncertain_rows', COUNT(*) FROM call_edges WHERE certainty = 'uncertain'
)
SELECT metric, count
FROM metrics
ORDER BY metric;
