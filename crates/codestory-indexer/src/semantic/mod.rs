use anyhow::Result;
use codestory_contracts::graph::EdgeKind;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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

#[derive(Debug, Clone)]
struct SemanticCandidateNode {
    id: i64,
    kind: i32,
    serialized_name: String,
    serialized_name_ascii_lower: String,
    qualified_name: Option<String>,
    qualified_name_ascii_lower: Option<String>,
    file_node_id: Option<i64>,
    language_family: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SemanticCandidateNodeSnapshot {
    pub id: i64,
    pub kind: i32,
    pub serialized_name: String,
    pub qualified_name: Option<String>,
    pub file_node_id: Option<i64>,
    pub language_family: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SemanticCandidateIndex {
    nodes: Vec<SemanticCandidateNode>,
    nodes_by_kind: HashMap<i32, Vec<usize>>,
    serialized_exact: HashMap<String, Vec<usize>>,
    qualified_exact: HashMap<String, Vec<usize>>,
    tail_ascii_lower: HashMap<String, Vec<usize>>,
}

impl SemanticCandidateIndex {
    pub fn load(conn: &Connection, kinds: &[i32]) -> Result<Self> {
        let query = if kinds.is_empty() {
            "SELECT n.id, n.kind, n.serialized_name, n.qualified_name, n.file_node_id, file_node.serialized_name
             FROM node n
             LEFT JOIN node file_node ON file_node.id = n.file_node_id
             ORDER BY COALESCE(n.start_line, -9223372036854775808), n.id"
                .to_string()
        } else {
            let kind_clause = kind_clause(kinds);
            format!(
                "SELECT n.id, n.kind, n.serialized_name, n.qualified_name, n.file_node_id, file_node.serialized_name
                 FROM node n
                 LEFT JOIN node file_node ON file_node.id = n.file_node_id
                 WHERE n.kind IN ({kind_clause})
                 ORDER BY COALESCE(n.start_line, -9223372036854775808), n.id"
            )
        };
        let mut stmt = conn.prepare(&query)?;
        let rows = stmt.query_map([], |row| {
            let serialized_name: String = row.get(2)?;
            let file_path: Option<String> = row.get(5)?;
            let qualified_name: Option<String> = row.get(3)?;
            Ok(SemanticCandidateNode {
                id: row.get(0)?,
                kind: row.get(1)?,
                serialized_name_ascii_lower: serialized_name.to_ascii_lowercase(),
                serialized_name,
                qualified_name_ascii_lower: qualified_name
                    .as_ref()
                    .map(|qualified_name| qualified_name.to_ascii_lowercase()),
                qualified_name,
                file_node_id: row.get(4)?,
                language_family: detect_language(file_path.as_deref())
                    .map(language_family_bucket)
                    .map(str::to_string),
            })
        })?;

        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row?);
        }

        Ok(Self::from_nodes(nodes))
    }

    pub(crate) fn from_snapshot_nodes(nodes: Vec<SemanticCandidateNodeSnapshot>) -> Self {
        Self::from_nodes(
            nodes
                .into_iter()
                .map(|node| SemanticCandidateNode {
                    id: node.id,
                    kind: node.kind,
                    serialized_name_ascii_lower: node.serialized_name.to_ascii_lowercase(),
                    serialized_name: node.serialized_name,
                    qualified_name_ascii_lower: node
                        .qualified_name
                        .as_ref()
                        .map(|qualified_name| qualified_name.to_ascii_lowercase()),
                    qualified_name: node.qualified_name,
                    file_node_id: node.file_node_id,
                    language_family: node.language_family,
                })
                .collect(),
        )
    }

    pub(crate) fn snapshot_nodes(&self) -> Vec<SemanticCandidateNodeSnapshot> {
        self.nodes
            .iter()
            .map(|node| SemanticCandidateNodeSnapshot {
                id: node.id,
                kind: node.kind,
                serialized_name: node.serialized_name.clone(),
                qualified_name: node.qualified_name.clone(),
                file_node_id: node.file_node_id,
                language_family: node.language_family.clone(),
            })
            .collect()
    }

    fn from_nodes(nodes: Vec<SemanticCandidateNode>) -> Self {
        let mut index = Self {
            nodes,
            ..Self::default()
        };
        for (offset, node) in index.nodes.iter().enumerate() {
            index
                .nodes_by_kind
                .entry(node.kind)
                .or_default()
                .push(offset);
            index
                .serialized_exact
                .entry(node.serialized_name.clone())
                .or_default()
                .push(offset);
            if let Some(qualified_name) = node.qualified_name.as_ref() {
                index
                    .qualified_exact
                    .entry(qualified_name.clone())
                    .or_default()
                    .push(offset);
            }
            for tail in tail_variants(node) {
                index.tail_ascii_lower.entry(tail).or_default().push(offset);
            }
        }
        index
    }

    fn nodes_for_name<'a>(
        &'a self,
        kinds: &[i32],
        name: &str,
        name_ascii_lower: &str,
        caller_language: Option<&'static str>,
        allow_fuzzy: bool,
        limit: usize,
    ) -> Vec<&'a SemanticCandidateNode> {
        let kind_set = kinds.iter().copied().collect::<HashSet<_>>();
        let mut out = Vec::with_capacity(limit);
        let mut seen = HashSet::with_capacity(limit.saturating_mul(2));
        let caller_language_family = caller_language.map(language_family_bucket);

        let mut push_offset = |offset: usize| {
            if out.len() >= limit {
                return;
            }
            let Some(node) = self.nodes.get(offset) else {
                return;
            };
            if !kind_set.contains(&node.kind)
                || !compatible_language_families(
                    caller_language_family,
                    node.language_family.as_deref(),
                )
                || !seen.insert(node.id)
            {
                return;
            }
            out.push(node);
        };

        if let Some(offsets) = self.serialized_exact.get(name) {
            for &offset in offsets {
                push_offset(offset);
            }
        }
        if let Some(offsets) = self.qualified_exact.get(name) {
            for &offset in offsets {
                push_offset(offset);
            }
        }
        if let Some(offsets) = self.tail_ascii_lower.get(name_ascii_lower) {
            for &offset in offsets {
                push_offset(offset);
            }
        }

        if allow_fuzzy && out.is_empty() {
            for kind in kinds {
                let Some(offsets) = self.nodes_by_kind.get(kind) else {
                    continue;
                };
                for &offset in offsets {
                    let Some(node) = self.nodes.get(offset) else {
                        continue;
                    };
                    if seen.contains(&node.id)
                        || !compatible_language_families(
                            caller_language_family,
                            node.language_family.as_deref(),
                        )
                    {
                        continue;
                    }
                    if node.serialized_name_ascii_lower.contains(name_ascii_lower)
                        || node
                            .qualified_name_ascii_lower
                            .as_deref()
                            .is_some_and(|qualified_name| qualified_name.contains(name_ascii_lower))
                    {
                        seen.insert(node.id);
                        out.push(node);
                        if out.len() >= limit {
                            break;
                        }
                    }
                }
                if out.len() >= limit {
                    break;
                }
            }
        }

        out
    }
}

pub trait SemanticResolver: Send + Sync {
    fn language(&self) -> &'static str;
    fn resolve(
        &self,
        index: &SemanticCandidateIndex,
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
        index: &SemanticCandidateIndex,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        if !self.enabled {
            return Ok(Vec::new());
        }

        match detect_language(request.file_path.as_deref()) {
            Some("c") => self.c.resolve(index, request),
            Some("cpp") => self.cpp.resolve(index, request),
            Some("javascript") => self.javascript.resolve(index, request),
            Some("python") => self.python.resolve(index, request),
            Some("rust") => self.rust.resolve(index, request),
            Some("typescript") => self.ts.resolve(index, request),
            Some("java") => self.java.resolve(index, request),
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

fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

pub(super) fn request_target(request: &SemanticResolutionRequest) -> Option<&str> {
    non_empty_trimmed(&request.target_name)
}

pub(super) fn request_language(request: &SemanticResolutionRequest) -> Option<&'static str> {
    detect_language(request.file_path.as_deref())
}

pub(super) fn alias_target(target: &str) -> &str {
    target
        .split_once(" as ")
        .map(|(_, rhs)| rhs.trim())
        .unwrap_or(target)
        .trim()
}

pub(super) fn tail_segment<'a>(value: &'a str, separators: &[char]) -> Option<&'a str> {
    let tail = value.rsplit(separators).next().unwrap_or(value);
    non_empty_trimmed(tail)
}

pub(super) fn namespace_tail<'a>(value: &'a str, separator: &str) -> Option<&'a str> {
    let tail = value.rsplit(separator).next().unwrap_or(value);
    non_empty_trimmed(tail)
}

pub(super) fn call_target_name(target: &str) -> Option<&str> {
    let tail = target
        .rsplit_once("::")
        .map(|(_, tail)| tail)
        .or_else(|| target.rsplit_once('.').map(|(_, tail)| tail))
        .unwrap_or(target);
    non_empty_trimmed(tail)
}

fn kind_clause(kinds: &[i32]) -> String {
    kinds
        .iter()
        .map(|kind| kind.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn resolve_import_candidates(
    index: &SemanticCandidateIndex,
    kinds: &[i32],
    symbol: &str,
    file_id: Option<i64>,
    caller_language: Option<&'static str>,
    confidence: f32,
) -> Result<Vec<SemanticResolutionCandidate>> {
    let ids = index
        .nodes_for_name(
            kinds,
            symbol,
            &symbol.to_ascii_lowercase(),
            caller_language,
            true,
            4,
        )
        .into_iter()
        .filter(|node| {
            file_id.is_none() || node.file_node_id.is_none() || node.file_node_id != file_id
        })
        .map(|node| node.id)
        .collect::<Vec<_>>();
    Ok(to_candidates(ids, confidence))
}

pub(super) fn resolve_call_candidates(
    index: &SemanticCandidateIndex,
    kinds: &[i32],
    call_name: &str,
    file_id: Option<i64>,
    caller_language: Option<&'static str>,
    same_file_confidence: f32,
    global_confidence: f32,
) -> Result<Vec<SemanticResolutionCandidate>> {
    let mut out = Vec::new();
    let name_ascii_lower = call_name.to_ascii_lowercase();

    if let Some(file_id) = file_id {
        let ids = index
            .nodes_for_name(
                kinds,
                call_name,
                &name_ascii_lower,
                caller_language,
                false,
                3,
            )
            .into_iter()
            .filter(|node| node.file_node_id == Some(file_id))
            .map(|node| node.id)
            .collect::<Vec<_>>();
        out.extend(to_candidates(ids, same_file_confidence));
    }

    if out.is_empty() {
        let ids = index
            .nodes_for_name(
                kinds,
                call_name,
                &name_ascii_lower,
                caller_language,
                false,
                3,
            )
            .into_iter()
            .map(|node| node.id)
            .collect::<Vec<_>>();
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

fn tail_variants(node: &SemanticCandidateNode) -> Vec<String> {
    let mut tails = Vec::with_capacity(2);
    if let Some(tail) = tail_component(&node.serialized_name) {
        tails.push(tail.to_ascii_lowercase());
    }
    if let Some(qualified_name) = node.qualified_name.as_ref()
        && let Some(tail) = tail_component(qualified_name)
    {
        tails.push(tail.to_ascii_lowercase());
    }
    tails
}

fn tail_component(value: &str) -> Option<&str> {
    let dot_idx = value.rfind('.');
    let colon_idx = value.rfind("::");
    let start = match (dot_idx, colon_idx) {
        (Some(dot), Some(colon)) => {
            if dot > colon {
                dot + 1
            } else {
                colon + 2
            }
        }
        (Some(dot), None) => dot + 1,
        (None, Some(colon)) => colon + 2,
        (None, None) => return None,
    };
    let tail = &value[start..];
    if tail.is_empty() { None } else { Some(tail) }
}

fn language_family_bucket(language: &'static str) -> &'static str {
    match language {
        "c" | "cpp" => "native",
        "javascript" | "typescript" => "webscript",
        "python" => "python",
        "rust" => "rust",
        "java" => "java",
        _ => language,
    }
}

fn compatible_language_families(
    caller_language: Option<&str>,
    candidate_language: Option<&str>,
) -> bool {
    match (caller_language, candidate_language) {
        (Some(lhs), Some(rhs)) => lhs == rhs,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SemanticCandidateIndex, alias_target, call_target_name, detect_language, namespace_tail,
        resolve_call_candidates, resolve_import_candidates, tail_segment,
    };
    use anyhow::Result;
    use codestory_contracts::graph::NodeKind;
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

    fn insert_file_node(conn: &Connection, id: i64, path: &str) -> Result<()> {
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, NULL, NULL, 1)",
            params![id, NodeKind::FILE as i32, path],
        )?;
        Ok(())
    }

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

    #[test]
    fn test_shared_name_helpers_trim_aliases_and_tails() {
        assert_eq!(alias_target("pkg.mod as alias"), "alias");
        assert_eq!(tail_segment("pkg/mod/file.ts", &['/', '.']), Some("ts"));
        assert_eq!(namespace_tail("crate::module::Type", "::"), Some("Type"));
        assert_eq!(call_target_name("crate::module::call"), Some("call"));
        assert_eq!(call_target_name("obj.method"), Some("method"));
        assert_eq!(call_target_name("standalone"), Some("standalone"));
    }

    #[test]
    fn test_resolve_call_candidates_ignores_substring_fuzzy_matches() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        insert_file_node(&conn, 1, "app.tsx")?;
        insert_file_node(&conn, 2, "boundary.tsx")?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                10_i64,
                NodeKind::METHOD as i32,
                "getDerivedStateFromError",
                "ErrorBoundary.getDerivedStateFromError",
                2_i64,
                25_i64
            ],
        )?;

        let index = SemanticCandidateIndex::load(&conn, &[NodeKind::METHOD as i32])?;
        let out = resolve_call_candidates(
            &index,
            &[NodeKind::METHOD as i32],
            "error",
            Some(1),
            detect_language(Some("app.tsx")),
            0.82,
            0.70,
        )?;

        assert!(out.is_empty(), "unexpected fuzzy candidates: {out:?}");
        Ok(())
    }

    #[test]
    fn test_resolve_call_candidates_filter_cross_language_matches() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        insert_file_node(&conn, 1, "app.js")?;
        insert_file_node(&conn, 2, "lib.rs")?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                20_i64,
                NodeKind::FUNCTION as i32,
                "helper",
                "crate::helper",
                2_i64,
                5_i64
            ],
        )?;

        let index = SemanticCandidateIndex::load(&conn, &[NodeKind::FUNCTION as i32])?;
        let out = resolve_call_candidates(
            &index,
            &[NodeKind::FUNCTION as i32],
            "helper",
            Some(1),
            detect_language(Some("app.js")),
            0.82,
            0.70,
        )?;

        assert!(
            out.is_empty(),
            "unexpected cross-language candidates: {out:?}"
        );
        Ok(())
    }

    #[test]
    fn test_resolve_import_candidates_filter_cross_language_fuzzy_matches() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_node_table(&conn)?;
        insert_file_node(&conn, 1, "app.js")?;
        insert_file_node(&conn, 2, "lib.rs")?;
        conn.execute(
            "INSERT INTO node (id, kind, serialized_name, qualified_name, file_node_id, start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                30_i64,
                NodeKind::MODULE as i32,
                "error_boundary_helpers",
                "crate::error_boundary_helpers",
                2_i64,
                5_i64
            ],
        )?;

        let index = SemanticCandidateIndex::load(&conn, &[NodeKind::MODULE as i32])?;
        let out = resolve_import_candidates(
            &index,
            &[NodeKind::MODULE as i32],
            "error",
            Some(1),
            detect_language(Some("app.js")),
            0.55,
        )?;

        assert!(
            out.is_empty(),
            "unexpected cross-language fuzzy import candidates: {out:?}"
        );
        Ok(())
    }
}
