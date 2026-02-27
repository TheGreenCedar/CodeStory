use super::{
    SemanticResolutionCandidate, SemanticResolutionRequest, SemanticResolver, kind_clause,
    resolve_call_candidates, resolve_import_candidates,
};
use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind};
use rusqlite::Connection;

pub struct PythonSemanticResolver;

impl SemanticResolver for PythonSemanticResolver {
    fn language(&self) -> &'static str {
        "python"
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

impl PythonSemanticResolver {
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
            .rsplit(['.', '/', ':'])
            .next()
            .unwrap_or(target)
            .trim();
        if symbol.is_empty() {
            return Ok(Vec::new());
        }

        let kinds = [
            NodeKind::MODULE as i32,
            NodeKind::PACKAGE as i32,
            NodeKind::CLASS as i32,
            NodeKind::FUNCTION as i32,
            NodeKind::METHOD as i32,
        ];
        let kind_clause = kind_clause(&kinds);

        resolve_import_candidates(conn, &kind_clause, symbol, request.file_id, 0.59)
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
        resolve_call_candidates(conn, &kind_clause, call_name, request.file_id, 0.82, 0.72)
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

        let resolver = PythonSemanticResolver;
        let request = SemanticResolutionRequest {
            edge_kind: EdgeKind::IMPORT,
            file_id: Some(2),
            file_path: Some("app.py".to_string()),
            caller_qualified: None,
            target_name: "collections".to_string(),
        };

        let out = resolver.resolve(&conn, &request)?;
        assert!(!out.is_empty());
        assert_eq!(out[0].target_node_id, 11_i64);
        Ok(())
    }
}
