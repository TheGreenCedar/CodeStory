use super::{SemanticResolutionCandidate, SemanticResolutionRequest, SemanticResolver};
use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind};
use rusqlite::{params, Connection};

pub struct TypeScriptSemanticResolver;

impl SemanticResolver for TypeScriptSemanticResolver {
    fn language(&self) -> &'static str {
        "typescript"
    }

    fn resolve(
        &self,
        conn: &Connection,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        match request.edge_kind {
            EdgeKind::IMPORT => self.resolve_import(conn, request),
            EdgeKind::CALL => self.resolve_call(conn, request),
            _ => Ok(Vec::new()),
        }
    }
}

impl TypeScriptSemanticResolver {
    fn resolve_import(
        &self,
        conn: &Connection,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let mut candidates = Vec::new();
        let normalized = request.target_name.trim();
        if normalized.is_empty() {
            return Ok(candidates);
        }

        // Phase 1: derive likely exported symbol from import path/alias and suggest package/module nodes.
        let symbol = normalized
            .split_once(" as ")
            .map(|(_, rhs)| rhs.trim())
            .unwrap_or(normalized)
            .rsplit(['/', '.', ':'])
            .next()
            .unwrap_or(normalized)
            .trim();
        if symbol.is_empty() {
            return Ok(candidates);
        }

        let kinds = [
            NodeKind::MODULE as i32,
            NodeKind::NAMESPACE as i32,
            NodeKind::PACKAGE as i32,
            NodeKind::CLASS as i32,
            NodeKind::INTERFACE as i32,
        ];
        let kind_clause = kinds
            .iter()
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let query = format!(
            "SELECT id FROM node
             WHERE kind IN ({})
             AND (serialized_name = ?1 OR serialized_name LIKE ?2 OR qualified_name LIKE ?3)
             ORDER BY start_line
             LIMIT 4",
            kind_clause
        );

        let mut stmt = conn.prepare(&query)?;
        let suffix_dot = format!("%.{}", symbol);
        let suffix_colon = format!("%::{}", symbol);
        let rows = stmt.query_map(params![symbol, suffix_dot, suffix_colon], |row| row.get(0))?;
        for row in rows {
            candidates.push(SemanticResolutionCandidate {
                target_node_id: row?,
                confidence: 0.58,
            });
        }

        Ok(candidates)
    }

    fn resolve_call(
        &self,
        conn: &Connection,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let mut out = Vec::new();
        let target = request.target_name.trim();
        if target.is_empty() {
            return Ok(out);
        }

        let call_name = target
            .rsplit_once('.')
            .map(|(_, tail)| tail.trim())
            .unwrap_or(target);
        if call_name.is_empty() {
            return Ok(out);
        }

        let kinds = [NodeKind::METHOD as i32, NodeKind::FUNCTION as i32];
        let kind_clause = kinds
            .iter()
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let query_same_file = format!(
            "SELECT id FROM node
             WHERE kind IN ({})
             AND file_node_id = ?1
             AND (serialized_name = ?2 OR serialized_name LIKE ?3 OR qualified_name LIKE ?4)
             ORDER BY start_line
             LIMIT 3",
            kind_clause
        );
        let suffix_dot = format!("%.{}", call_name);
        let suffix_colon = format!("%::{}", call_name);
        if let Some(file_id) = request.file_id {
            let mut stmt = conn.prepare(&query_same_file)?;
            let rows = stmt.query_map(params![file_id, call_name, suffix_dot, suffix_colon], |row| {
                row.get(0)
            })?;
            for row in rows {
                out.push(SemanticResolutionCandidate {
                    target_node_id: row?,
                    confidence: 0.88,
                });
            }
        }

        if out.is_empty() {
            let query_global = format!(
                "SELECT id FROM node
                 WHERE kind IN ({})
                 AND (serialized_name = ?1 OR serialized_name LIKE ?2 OR qualified_name LIKE ?3)
                 ORDER BY start_line
                 LIMIT 3",
                kind_clause
            );
            let mut stmt = conn.prepare(&query_global)?;
            let rows = stmt.query_map(params![call_name, suffix_dot, suffix_colon], |row| row.get(0))?;
            for row in rows {
                out.push(SemanticResolutionCandidate {
                    target_node_id: row?,
                    confidence: 0.70,
                });
            }
        }

        Ok(out)
    }
}
