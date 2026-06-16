use super::{
    SemanticCandidateIndex, SemanticResolutionCandidate, SemanticResolutionRequest,
    SemanticResolver, alias_target, call_target_name, request_language, request_target,
    resolve_call_candidates, resolve_import_candidates, tail_segment,
};
use anyhow::Result;
use codestory_contracts::{
    graph::{EdgeKind, NodeKind, ResolutionCertainty},
    language_support::supported_extensions,
};

pub struct GenericSemanticResolver {
    language: &'static str,
}

impl GenericSemanticResolver {
    pub const fn new(language: &'static str) -> Self {
        Self { language }
    }

    fn resolve_import(
        &self,
        index: &SemanticCandidateIndex,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let Some(target) = request_target(request) else {
            return Ok(Vec::new());
        };

        let Some(symbol) = normalized_import_symbol(target) else {
            return Ok(Vec::new());
        };

        let kinds = [
            NodeKind::MODULE as i32,
            NodeKind::NAMESPACE as i32,
            NodeKind::PACKAGE as i32,
            NodeKind::CLASS as i32,
            NodeKind::INTERFACE as i32,
            NodeKind::ENUM as i32,
            NodeKind::TYPEDEF as i32,
            NodeKind::FUNCTION as i32,
        ];
        resolve_import_candidates(
            index,
            &kinds,
            symbol,
            request.file_id,
            request_language(request),
            ResolutionCertainty::PROBABLE_MIN,
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
            0.80,
            0.68,
        )
    }
}

impl SemanticResolver for GenericSemanticResolver {
    fn language(&self) -> &'static str {
        self.language
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

fn normalized_import_symbol(target: &str) -> Option<&str> {
    let unquoted = alias_target(target)
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`'))
        .trim();
    if unquoted.contains(['/', '\\']) {
        return tail_segment(unquoted, &['/', '\\']).map(strip_known_extension);
    }

    let without_extension = strip_known_extension(unquoted);
    tail_segment(without_extension, &['.', ':']).map(strip_known_extension)
}

fn strip_known_extension(symbol: &str) -> &str {
    supported_extensions()
        .find_map(|extension| symbol.strip_suffix(extension)?.strip_suffix('.'))
        .unwrap_or(symbol)
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
    fn test_generic_resolver_normalizes_import_path() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                41_i64,
                NodeKind::MODULE as i32,
                "helpers",
                "app.helpers",
                7_i64,
                1_i64
            ],
        )?;

        let index = SemanticCandidateIndex::load(&conn, &[NodeKind::MODULE as i32])?;
        let resolver = GenericSemanticResolver::new("kotlin");
        let request = SemanticResolutionRequest {
            edge_kind: EdgeKind::IMPORT,
            file_id: Some(1),
            file_path: Some("Main.kt".to_string()),
            caller_qualified: None,
            target_name: "\"./app/helpers.kt\"".to_string(),
        };

        let out = resolver.resolve(&index, &request)?;
        assert!(!out.is_empty());
        assert_eq!(out[0].target_node_id, 41_i64);
        Ok(())
    }

    #[test]
    fn test_generic_extension_stripping_uses_public_language_profiles() {
        for extension in supported_extensions() {
            let path = format!("./app/helpers.{extension}");
            assert_eq!(
                normalized_import_symbol(&path),
                Some("helpers"),
                "generic import normalization should follow public language profile for .{extension}"
            );
        }
    }

    #[test]
    fn test_generic_import_normalization_preserves_dotted_file_stems() {
        assert_eq!(
            normalized_import_symbol("./app/helpers.min.kt"),
            Some("helpers.min")
        );
        assert_eq!(
            normalized_import_symbol("com.example.Helpers"),
            Some("Helpers")
        );
    }
}
