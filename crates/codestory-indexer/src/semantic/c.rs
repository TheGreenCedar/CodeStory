use super::{
    SemanticCandidateIndex, SemanticResolutionCandidate, SemanticResolutionRequest,
    SemanticResolver, call_target_name, request_language, request_target, resolve_call_candidates,
    resolve_import_candidates,
};
use anyhow::Result;
use codestory_contracts::graph::{EdgeKind, NodeKind};

pub struct CSemanticResolver;

impl SemanticResolver for CSemanticResolver {
    fn language(&self) -> &'static str {
        "c"
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

impl CSemanticResolver {
    fn resolve_import(
        &self,
        index: &SemanticCandidateIndex,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let Some(target) = request_target(request) else {
            return Ok(Vec::new());
        };

        let symbol = normalize_include_symbol(target);
        if symbol.is_empty() {
            return Ok(Vec::new());
        }

        let kinds = [
            NodeKind::MODULE as i32,
            NodeKind::STRUCT as i32,
            NodeKind::UNION as i32,
            NodeKind::ENUM as i32,
            NodeKind::TYPEDEF as i32,
            NodeKind::FUNCTION as i32,
        ];
        resolve_import_candidates(
            index,
            &kinds,
            &symbol,
            request.file_id,
            request_language(request),
            0.54,
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

        let kinds = [NodeKind::FUNCTION as i32, NodeKind::METHOD as i32];
        resolve_call_candidates(
            index,
            &kinds,
            call_name,
            request.file_id,
            request_language(request),
            0.82,
            0.66,
        )
    }
}

fn normalize_include_symbol(target: &str) -> String {
    let trimmed = target
        .trim()
        .trim_matches('"')
        .trim_matches('<')
        .trim_matches('>');
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed).trim();
    let no_ext = base.strip_suffix(".h").unwrap_or(base);
    no_ext.to_string()
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
    fn test_c_resolver_returns_import_candidate() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                21_i64,
                NodeKind::MODULE as i32,
                "stdio",
                "stdio",
                4_i64,
                1_i64
            ],
        )?;

        let index = SemanticCandidateIndex::load(&conn, &[NodeKind::MODULE as i32])?;
        let resolver = CSemanticResolver;
        let request = SemanticResolutionRequest {
            edge_kind: EdgeKind::IMPORT,
            file_id: Some(3),
            file_path: Some("main.c".to_string()),
            caller_qualified: None,
            target_name: "<stdio.h>".to_string(),
        };

        let out = resolver.resolve(&index, &request)?;
        assert!(!out.is_empty());
        assert_eq!(out[0].target_node_id, 21_i64);
        Ok(())
    }
}
