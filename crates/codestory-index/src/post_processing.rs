use anyhow::Result;
use codestory_core::{NodeId, NodeKind};
use codestory_storage::Storage;
use rusqlite::params;

pub struct PostProcessor;

impl Default for PostProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl PostProcessor {
    pub fn new() -> Self {
        Self
    }

    /// Run post-processing to resolve ambiguous references.
    /// This implementation looks for UNKNOWN nodes and tries to link them to
    /// defined nodes based on their usage (CALL -> FUNCTION, TYPE_USAGE -> CLASS, etc.)
    pub fn run(&self, storage: &mut Storage) -> Result<usize> {
        let conn = storage.get_connection();

        // 1. Find all UNKNOWN nodes and their usage counts/kinds
        let mut unknown_info = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT n.id, n.serialized_name, e.kind
                 FROM node n
                 JOIN edge e ON n.id = e.target_node_id
                 WHERE n.kind = ?1",
            )?;

            let unknown_kind = NodeKind::UNKNOWN as i32;
            let rows = stmt.query_map([unknown_kind], |row| {
                Ok((
                    NodeId(row.get(0)?),
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?,
                ))
            })?;

            for r in rows {
                unknown_info.push(r?);
            }
        }

        let mut resolved_count = 0;

        // 2. Resolve based on usage
        for (unknown_id, name, edge_kind_int) in unknown_info {
            let edge_kind = match edge_kind_int {
                3 => Some(codestory_core::EdgeKind::CALL),
                1 => Some(codestory_core::EdgeKind::TYPE_USAGE),
                4 => Some(codestory_core::EdgeKind::INHERITANCE),
                _ => None,
            };

            // Define candidate kinds based on usage
            let candidate_kinds: Vec<i32> = match edge_kind {
                Some(codestory_core::EdgeKind::CALL) => vec![
                    NodeKind::FUNCTION as i32,
                    NodeKind::METHOD as i32,
                    NodeKind::MACRO as i32,
                ],
                Some(codestory_core::EdgeKind::TYPE_USAGE)
                | Some(codestory_core::EdgeKind::INHERITANCE) => vec![
                    NodeKind::CLASS as i32,
                    NodeKind::STRUCT as i32,
                    NodeKind::INTERFACE as i32,
                    NodeKind::ENUM as i32,
                    NodeKind::TYPEDEF as i32,
                ],
                _ => vec![], // Any non-unknown
            };

            let pattern = format!("%::{}", name);
            let unknown_kind = NodeKind::UNKNOWN as i32;

            let mut candidates = Vec::new();
            if candidate_kinds.is_empty() {
                let mut stmt = conn.prepare("SELECT id FROM node WHERE (serialized_name = ?1 OR serialized_name LIKE ?2) AND kind != ?3")?;
                let rows = stmt.query_map(params![name, pattern, unknown_kind], |row| {
                    Ok(NodeId(row.get(0)?))
                })?;
                for r in rows {
                    candidates.push(r?);
                }
            } else {
                // Build parameterized query with the correct number of placeholders
                // Safety: candidate_kinds contains only i32 enum values, but we still use
                // proper SQL parameters to avoid any injection risk and follow best practices
                let placeholders = candidate_kinds
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", i + 3)) // Start at ?3 since ?1 and ?2 are name/pattern
                    .collect::<Vec<_>>()
                    .join(",");
                let query = format!(
                    "SELECT id FROM node WHERE (serialized_name = ?1 OR serialized_name LIKE ?2) AND kind IN ({})",
                    placeholders
                );
                let mut stmt = conn.prepare(&query)?;

                // Build params array: [name, pattern, kind1, kind2, ...]
                let mut query_params: Vec<Box<dyn rusqlite::ToSql>> =
                    vec![Box::new(name.clone()), Box::new(pattern.clone())];
                for kind in &candidate_kinds {
                    query_params.push(Box::new(*kind));
                }

                let param_refs: Vec<&dyn rusqlite::ToSql> =
                    query_params.iter().map(|p| p.as_ref()).collect();
                let rows = stmt.query_map(&param_refs[..], |row| Ok(NodeId(row.get(0)?)))?;
                for r in rows {
                    candidates.push(r?);
                }
            };

            if let Some(best_match) = candidates.first() {
                // Update edges: point to the resolved node
                conn.execute(
                    "UPDATE edge SET target_node_id = ?1 WHERE target_node_id = ?2",
                    params![best_match.0, unknown_id.0],
                )?;

                // Delete the unknown node and its occurrences
                conn.execute("DELETE FROM node WHERE id = ?1", params![unknown_id.0])?;
                conn.execute(
                    "DELETE FROM occurrence WHERE element_id = ?1",
                    params![unknown_id.0],
                )?;

                resolved_count += 1;
            }
        }

        Ok(resolved_count)
    }
}
