use super::{
    SemanticCandidateIndex, SemanticResolutionCandidate, SemanticResolutionRequest,
    SemanticResolver, alias_target, call_target_name, request_language, request_target,
    resolve_call_candidates, resolve_import_candidates, tail_segment,
};
use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind};

pub struct PythonSemanticResolver;

impl SemanticResolver for PythonSemanticResolver {
    fn language(&self) -> &'static str {
        "python"
    }

    fn resolve(
        &self,
        index: &SemanticCandidateIndex,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        match request.edge_kind {
            EdgeKind::IMPORT => self.resolve_import(index, request),
            EdgeKind::CALL => self.resolve_call(index, request),
            _ => Ok(Vec::new()),
        }
    }
}

impl PythonSemanticResolver {
    fn resolve_import(
        &self,
        index: &SemanticCandidateIndex,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let Some(target) = request_target(request) else {
            return Ok(Vec::new());
        };

        let Some(symbol) = tail_segment(alias_target(target), &['.', '/', ':']) else {
            return Ok(Vec::new());
        };

        let kinds = [
            NodeKind::MODULE as i32,
            NodeKind::PACKAGE as i32,
            NodeKind::CLASS as i32,
            NodeKind::FUNCTION as i32,
            NodeKind::METHOD as i32,
        ];
        resolve_import_candidates(
            index,
            &kinds,
            symbol,
            request.file_id,
            request_language(request),
            0.59,
        )
    }

    fn resolve_call(
        &self,
        index: &SemanticCandidateIndex,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let Some(target) = request_target(request) else {
            return Ok(Vec::new());
        };

        let Some(call_name) = call_target_name(target) else {
            return Ok(Vec::new());
        };

        let kinds = [NodeKind::METHOD as i32, NodeKind::FUNCTION as i32];
        resolve_call_candidates(
            index,
            &kinds,
            call_name,
            request.file_id,
            request_language(request),
            0.82,
            0.72,
        )
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
    fn test_python_resolver_returns_import_candidate() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                11_i64,
                NodeKind::MODULE as i32,
                "collections",
                "collections",
                3_i64,
                1_i64
            ],
        )?;

        let index = SemanticCandidateIndex::load(&conn, &[NodeKind::MODULE as i32])?;
        let resolver = PythonSemanticResolver;
        let request = SemanticResolutionRequest {
            edge_kind: EdgeKind::IMPORT,
            file_id: Some(2),
            file_path: Some("app.py".to_string()),
            caller_qualified: None,
            target_name: "collections".to_string(),
        };

        let out = resolver.resolve(&index, &request)?;
        assert!(!out.is_empty());
        assert_eq!(out[0].target_node_id, 11_i64);
        Ok(())
    }
}
