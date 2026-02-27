use super::{
    SemanticResolutionCandidate, SemanticResolutionRequest, SemanticResolver, kind_clause,
    resolve_call_candidates, resolve_import_candidates,
};
use anyhow::Result;
use codestory_core::{EdgeKind, NodeKind};
use rusqlite::Connection;

pub struct CppSemanticResolver;

impl SemanticResolver for CppSemanticResolver {
    fn language(&self) -> &'static str {
        "cpp"
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

impl CppSemanticResolver {
    fn resolve_import(
        &self,
        conn: &Connection,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let target = request.target_name.trim();
        if target.is_empty() {
            return Ok(Vec::new());
        }

        let symbol = normalize_include_symbol(target);
        if symbol.is_empty() {
            return Ok(Vec::new());
        }

        let kinds = [
            NodeKind::MODULE as i32,
            NodeKind::NAMESPACE as i32,
            NodeKind::CLASS as i32,
            NodeKind::STRUCT as i32,
            NodeKind::ENUM as i32,
            NodeKind::TYPEDEF as i32,
            NodeKind::FUNCTION as i32,
            NodeKind::METHOD as i32,
        ];
        let kind_clause = kind_clause(&kinds);

        resolve_import_candidates(conn, &kind_clause, &symbol, request.file_id, 0.58)
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
        resolve_call_candidates(conn, &kind_clause, call_name, request.file_id, 0.82, 0.70)
    }
}

fn normalize_include_symbol(target: &str) -> String {
    let trimmed = target
        .trim()
        .trim_matches('"')
        .trim_matches('<')
        .trim_matches('>');
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed).trim();
    let no_ext = base
        .strip_suffix(".hpp")
        .or_else(|| base.strip_suffix(".hxx"))
        .or_else(|| base.strip_suffix(".hh"))
        .or_else(|| base.strip_suffix(".h"))
        .unwrap_or(base);
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
    fn test_cpp_resolver_returns_call_candidate() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                22_i64,
                NodeKind::METHOD as i32,
                "push_back",
                "std::vector::push_back",
                9_i64,
                1_i64
            ],
        )?;

        let resolver = CppSemanticResolver;
        let request = SemanticResolutionRequest {
            edge_kind: EdgeKind::CALL,
            file_id: Some(9),
            file_path: Some("main.cpp".to_string()),
            caller_qualified: Some("main".to_string()),
            target_name: "push_back".to_string(),
        };

        let out = resolver.resolve(&conn, &request)?;
        assert!(!out.is_empty());
        assert_eq!(out[0].target_node_id, 22_i64);
        Ok(())
    }
}
