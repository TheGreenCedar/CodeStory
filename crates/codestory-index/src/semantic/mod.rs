use anyhow::Result;
use codestory_core::EdgeKind;
use rusqlite::Connection;

mod java;
mod typescript;

use java::JavaSemanticResolver;
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
    ts: TypeScriptSemanticResolver,
    java: JavaSemanticResolver,
}

impl SemanticResolverRegistry {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
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
            Some("typescript") => self.ts.resolve(conn, request),
            Some("java") => self.java.resolve(conn, request),
            _ => Ok(Vec::new()),
        }
    }
}

fn detect_language(path: Option<&str>) -> Option<&'static str> {
    let path = path?;
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "ts" | "tsx" => Some("typescript"),
        "java" => Some("java"),
        _ => None,
    }
}
