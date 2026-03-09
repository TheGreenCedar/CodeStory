use super::{
    SemanticCandidateIndex, SemanticResolutionCandidate, SemanticResolutionRequest,
    SemanticResolver, detect_language, resolve_call_candidates, resolve_import_candidates,
};
use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind};

pub struct TypeScriptSemanticResolver;

impl SemanticResolver for TypeScriptSemanticResolver {
    fn language(&self) -> &'static str {
        "typescript"
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

impl TypeScriptSemanticResolver {
    fn resolve_import(
        &self,
        index: &SemanticCandidateIndex,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let normalized = request.target_name.trim();
        if normalized.is_empty() {
            return Ok(Vec::new());
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
            return Ok(Vec::new());
        }

        let kinds = [
            NodeKind::MODULE as i32,
            NodeKind::NAMESPACE as i32,
            NodeKind::PACKAGE as i32,
            NodeKind::CLASS as i32,
            NodeKind::INTERFACE as i32,
            NodeKind::ENUM as i32,
            NodeKind::TYPEDEF as i32,
        ];
        resolve_import_candidates(
            index,
            &kinds,
            symbol,
            request.file_id,
            detect_language(request.file_path.as_deref()),
            0.58,
        )
    }

    fn resolve_call(
        &self,
        index: &SemanticCandidateIndex,
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
        resolve_call_candidates(
            index,
            &kinds,
            call_name,
            request.file_id,
            detect_language(request.file_path.as_deref()),
            0.88,
            0.70,
        )
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
    fn test_typescript_resolver_same_file_common_call_is_certain() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                31_i64,
                NodeKind::METHOD as i32,
                "clone",
                "pkg.foo.clone",
                7_i64,
                1_i64
            ],
        )?;

        let index = SemanticCandidateIndex::load(&conn, &[NodeKind::METHOD as i32])?;
        let resolver = TypeScriptSemanticResolver;
        let request = SemanticResolutionRequest {
            edge_kind: EdgeKind::CALL,
            file_id: Some(7),
            file_path: Some("foo.ts".to_string()),
            caller_qualified: Some("pkg.foo.call".to_string()),
            target_name: "clone".to_string(),
        };

        let out = resolver.resolve(&index, &request)?;
        assert!(!out.is_empty());
        assert!(out[0].confidence >= ResolutionCertainty::CERTAIN_MIN);
        Ok(())
    }
}
