use super::{
    SemanticCandidateIndex, SemanticResolutionCandidate, SemanticResolutionRequest,
    SemanticResolver, call_target_name, namespace_tail, request_language, request_target,
    resolve_call_candidates, resolve_import_candidates,
};
use anyhow::Result;
use codestory_contracts::graph::{EdgeKind, NodeKind};

pub struct CSharpSemanticResolver;

impl SemanticResolver for CSharpSemanticResolver {
    fn language(&self) -> &'static str {
        "csharp"
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

impl CSharpSemanticResolver {
    fn resolve_import(
        &self,
        index: &SemanticCandidateIndex,
        request: &SemanticResolutionRequest,
    ) -> Result<Vec<SemanticResolutionCandidate>> {
        let Some(target) = request_target(request) else {
            return Ok(Vec::new());
        };

        let Some(symbol) = namespace_tail(target, ".") else {
            return Ok(Vec::new());
        };

        let kinds = [
            NodeKind::NAMESPACE as i32,
            NodeKind::MODULE as i32,
            NodeKind::CLASS as i32,
            NodeKind::INTERFACE as i32,
            NodeKind::STRUCT as i32,
            NodeKind::METHOD as i32,
            NodeKind::FUNCTION as i32,
        ];
        resolve_import_candidates(
            index,
            &kinds,
            symbol,
            request.file_id,
            request_language(request),
            0.58,
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
            0.70,
        )
    }
}
