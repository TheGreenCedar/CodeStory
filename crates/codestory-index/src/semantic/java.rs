use super::{
    SemanticResolutionCandidate, SemanticResolutionRequest, SemanticResolver, kind_clause,
    resolve_call_candidates, resolve_import_candidates,
};
use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind};
use rusqlite::Connection;

pub struct JavaSemanticResolver;

impl SemanticResolver for JavaSemanticResolver {
    fn language(&self) -> &'static str {
        "java"
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

impl JavaSemanticResolver {
    fn resolve_import(
        &self,
        conn: &Connection,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let target = request.target_name.trim();
        if target.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 1: map `a.b.C` and static-like imports to likely package/type definitions.
        let symbol = target.rsplit('.').next().unwrap_or(target).trim();
        if symbol.is_empty() {
            return Ok(Vec::new());
        }

        let kinds = [
            NodeKind::PACKAGE as i32,
            NodeKind::MODULE as i32,
            NodeKind::NAMESPACE as i32,
            NodeKind::CLASS as i32,
            NodeKind::INTERFACE as i32,
            NodeKind::ANNOTATION as i32,
            NodeKind::ENUM as i32,
        ];
        let kind_clause = kind_clause(&kinds);
        resolve_import_candidates(conn, &kind_clause, symbol, request.file_id, 0.60)
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
            .rsplit_once('.')
            .map(|(_, tail)| tail.trim())
            .unwrap_or(target);
        if call_name.is_empty() {
            return Ok(Vec::new());
        }

        let kinds = [NodeKind::METHOD as i32, NodeKind::FUNCTION as i32];
        let kind_clause = kind_clause(&kinds);
        resolve_call_candidates(conn, &kind_clause, call_name, request.file_id, 0.89, 0.72)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_core::ResolutionCertainty;
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
    fn test_java_resolver_same_file_common_call_is_certain() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                21_i64,
                NodeKind::METHOD as i32,
                "clone",
                "pkg.Foo.clone",
                3_i64,
                1_i64
            ],
        )?;

        let resolver = JavaSemanticResolver;
        let request = SemanticResolutionRequest {
            edge_kind: EdgeKind::CALL,
            file_id: Some(3),
            file_path: Some("Foo.java".to_string()),
            caller_qualified: Some("pkg.Foo.call".to_string()),
            target_name: "clone".to_string(),
        };

        let out = resolver.resolve(&conn, &request)?;
        assert!(!out.is_empty());
        assert!(out[0].confidence >= ResolutionCertainty::CERTAIN_MIN);
        Ok(())
    }
}
