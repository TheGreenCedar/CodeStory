use super::{
    SemanticResolutionCandidate, SemanticResolutionRequest, SemanticResolver, kind_clause,
    resolve_call_candidates, resolve_import_candidates,
};
use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind};
use rusqlite::Connection;

pub struct RustSemanticResolver;

impl SemanticResolver for RustSemanticResolver {
    fn language(&self) -> &'static str {
        "rust"
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

impl RustSemanticResolver {
    fn resolve_import(
        &self,
        conn: &Connection,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let target = request.target_name.trim();
        if target.is_empty() {
            return Ok(Vec::new());
        }

        let symbol = target
            .split_once(" as ")
            .map(|(_, rhs)| rhs.trim())
            .unwrap_or(target)
            .rsplit("::")
            .next()
            .unwrap_or(target)
            .trim();
        if symbol.is_empty() {
            return Ok(Vec::new());
        }

        let kinds = [
            NodeKind::MODULE as i32,
            NodeKind::NAMESPACE as i32,
            NodeKind::STRUCT as i32,
            NodeKind::ENUM as i32,
            NodeKind::INTERFACE as i32,
            NodeKind::TYPEDEF as i32,
            NodeKind::FUNCTION as i32,
            NodeKind::METHOD as i32,
        ];
        let kind_clause = kind_clause(&kinds);

        resolve_import_candidates(conn, &kind_clause, symbol, request.file_id, 0.61)
    }

    fn resolve_call(
        &self,
        conn: &Connection,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let target = request.target_name.trim();
        if target.is_empty() {
            return Ok(Vec::new());
        }

        let call_name = target
            .rsplit_once("::")
            .map(|(_, tail)| tail.trim())
            .or_else(|| target.rsplit_once('.').map(|(_, tail)| tail.trim()))
            .unwrap_or(target);
        if call_name.is_empty() {
            return Ok(Vec::new());
        }

        let kinds = [NodeKind::METHOD as i32, NodeKind::FUNCTION as i32];
        let kind_clause = kind_clause(&kinds);
        resolve_call_candidates(conn, &kind_clause, call_name, request.file_id, 0.82, 0.73)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, params};

    fn create_node_table(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE node (
                id INTEGER PRIMARY KEY,
                kind INTEGER NOT NULL,
                serialized_name TEXT NOT NULL,
                qualified_name TEXT,
                file_node_id INTEGER,
                start_line INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        Ok(())
    }

    #[test]
    fn test_rust_resolver_returns_call_candidate() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                12_i64,
                NodeKind::FUNCTION as i32,
                "dedup",
                "crate::utils::dedup",
                5_i64,
                1_i64
            ],
        )?;

        let resolver = RustSemanticResolver;
        let request = SemanticResolutionRequest {
            edge_kind: EdgeKind::CALL,
            file_id: Some(5),
            file_path: Some("src/lib.rs".to_string()),
            caller_qualified: Some("crate::main".to_string()),
            target_name: "dedup".to_string(),
        };

        let out = resolver.resolve(&conn, &request)?;
        assert!(!out.is_empty());
        assert_eq!(out[0].target_node_id, 12_i64);
        Ok(())
    }
}
