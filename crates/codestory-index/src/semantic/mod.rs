use anyhow::Result;
use codestory_core::EdgeKind;
use rusqlite::{Connection, params};

mod c;
mod cpp;
mod java;
mod javascript;
mod python;
mod rust;
mod typescript;

use c::CSemanticResolver;
use cpp::CppSemanticResolver;
use java::JavaSemanticResolver;
use javascript::JavaScriptSemanticResolver;
use python::PythonSemanticResolver;
use rust::RustSemanticResolver;
use typescript::TypeScriptSemanticResolver;

#[derive(Debug, Clone)]
pub struct SemanticResolutionRequest {
    pub edge_kind: EdgeKind,
    pub file_id: Option<i64>,
    pub file_path: Option<String>,
    pub caller_qualified: Option<String>,
    pub target_name: String,
}

#[derive(Debug, Clone, Copy)]
pub struct SemanticResolutionCandidate {
    pub target_node_id: i64,
    pub confidence: f32,
}

pub trait SemanticResolver: Send + Sync {
    fn language(&self) -> &'static str;
    fn resolve(
        &self,
        conn: &Connection,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>>;
}

pub struct SemanticResolverRegistry {
    enabled: bool,
    c: CSemanticResolver,
    cpp: CppSemanticResolver,
    javascript: JavaScriptSemanticResolver,
    python: PythonSemanticResolver,
    rust: RustSemanticResolver,
    ts: TypeScriptSemanticResolver,
    java: JavaSemanticResolver,
}

impl SemanticResolverRegistry {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            c: CSemanticResolver,
            cpp: CppSemanticResolver,
            javascript: JavaScriptSemanticResolver,
            python: PythonSemanticResolver,
            rust: RustSemanticResolver,
            ts: TypeScriptSemanticResolver,
            java: JavaSemanticResolver,
        }
    }

    pub fn resolve(
        &self,
        conn: &Connection,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        if !self.enabled {
            return Ok(Vec::new());
        }

        match detect_language(request.file_path.as_deref()) {
            Some("c") => self.c.resolve(conn, request),
            Some("cpp") => self.cpp.resolve(conn, request),
            Some("javascript") => self.javascript.resolve(conn, request),
            Some("python") => self.python.resolve(conn, request),
            Some("rust") => self.rust.resolve(conn, request),
            Some("typescript") => self.ts.resolve(conn, request),
            Some("java") => self.java.resolve(conn, request),
            _ => Ok(Vec::new()),
        }
    }
}

pub(crate) fn detect_language(path: Option<&str>) -> Option<&'static str> {
    let path = path?;
    let ext = path
        .rsplit('.')
        .next()?
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match ext.as_str() {
        "c" => Some("c"),
        "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx" => Some("cpp"),
        "java" => Some("java"),
        "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
        "py" | "pyi" => Some("python"),
        "rs" => Some("rust"),
        "ts" | "tsx" | "mts" | "cts" => Some("typescript"),
        _ => None,
    }
}

pub(super) fn kind_clause(kinds: &[i32]) -> String {
    kinds
        .iter()
        .map(|kind| kind.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn resolve_import_candidates(
    conn: &Connection,
    kind_clause: &str,
    symbol: &str,
    file_id: Option<i64>,
    confidence: f32,
) -> Result<Vec<SemanticResolutionCandidate>> {
    let query = format!(
        "SELECT id FROM node
         WHERE kind IN ({})
         AND (serialized_name = ?1 OR serialized_name LIKE ?2 OR qualified_name LIKE ?3)
         AND (?4 IS NULL OR file_node_id IS NULL OR file_node_id != ?4)
         ORDER BY start_line
         LIMIT 4",
        kind_clause
    );
    let (suffix_dot, suffix_colon) = qualified_name_suffixes(symbol);
    let ids = query_node_ids(
        conn,
        &query,
        params![symbol, &suffix_dot, &suffix_colon, file_id],
    )?;
    Ok(to_candidates(ids, confidence))
}

pub(super) fn resolve_call_candidates(
    conn: &Connection,
    kind_clause: &str,
    call_name: &str,
    file_id: Option<i64>,
    same_file_confidence: f32,
    global_confidence: f32,
) -> Result<Vec<SemanticResolutionCandidate>> {
    let (suffix_dot, suffix_colon) = qualified_name_suffixes(call_name);
    let mut out = Vec::new();

    if let Some(file_id) = file_id {
        let query_same_file = format!(
            "SELECT id FROM node
             WHERE kind IN ({})
             AND file_node_id = ?1
             AND (serialized_name = ?2 OR serialized_name LIKE ?3 OR qualified_name LIKE ?4)
             ORDER BY start_line
             LIMIT 3",
            kind_clause
        );
        let ids = query_node_ids(
            conn,
            &query_same_file,
            params![file_id, call_name, &suffix_dot, &suffix_colon],
        )?;
        out.extend(to_candidates(ids, same_file_confidence));
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
        let ids = query_node_ids(
            conn,
            &query_global,
            params![call_name, &suffix_dot, &suffix_colon],
        )?;
        out.extend(to_candidates(ids, global_confidence));
    }

    Ok(out)
}

fn to_candidates(ids: Vec<i64>, confidence: f32) -> Vec<SemanticResolutionCandidate> {
    ids.into_iter()
        .map(|target_node_id| SemanticResolutionCandidate {
            target_node_id,
            confidence,
        })
        .collect()
}

fn qualified_name_suffixes(symbol: &str) -> (String, String) {
    (format!("%.{}", symbol), format!("%::{}", symbol))
}

fn query_node_ids<P: rusqlite::Params>(
    conn: &Connection,
    query: &str,
    params: P,
) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(query)?;
    let rows = stmt.query_map(params, |row| row.get(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::detect_language;

    #[test]
    fn test_detect_language_extension_matrix() {
        let expected = [
            ("a.c", Some("c")),
            ("a.cpp", Some("cpp")),
            ("a.hxx", Some("cpp")),
            ("a.h", Some("cpp")),
            ("a.java", Some("java")),
            ("a.js", Some("javascript")),
            ("a.jsx", Some("javascript")),
            ("a.mjs", Some("javascript")),
            ("a.cjs", Some("javascript")),
            ("a.py", Some("python")),
            ("a.pyi", Some("python")),
            ("a.rs", Some("rust")),
            ("a.ts", Some("typescript")),
            ("a.tsx", Some("typescript")),
            ("a.mts", Some("typescript")),
            ("a.cts", Some("typescript")),
            ("a.unknown", None),
        ];
        for (path, language) in expected {
            assert_eq!(detect_language(Some(path)), language, "path={path}");
        }
    }
}
