use crate::health::{ComponentHealth, ComponentStatus};

/// v2 mandatory sidecar mode matrix row.
///
/// Only [`RetrievalDegradedMode::Full`] is promotion-eligible for packet/search primary results.
/// All degraded rows carry failure-mode diagnostics so callers can repair sidecars without
/// silently falling back to partial lexical, graph, or vector evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalDegradedMode {
    Full,
    NoScip,
    NoSemantic,
    LexicalOnly,
    Unavailable,
}

impl RetrievalDegradedMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::NoScip => "no_scip",
            Self::NoSemantic => "no_semantic",
            Self::LexicalOnly => "lexical_only",
            Self::Unavailable => "unavailable",
        }
    }

    pub fn promotion_eligible(self) -> bool {
        matches!(self, Self::Full)
    }

    pub fn runs_scip_stages(self) -> bool {
        matches!(self, Self::Full)
    }

    pub fn runs_semantic_stage(self) -> bool {
        matches!(self, Self::Full)
    }

    pub fn runs_lexical_stage(self) -> bool {
        matches!(self, Self::Full)
    }
}

pub fn derive_degraded_mode(
    lexical: &ComponentHealth,
    semantic: &ComponentHealth,
    scip: &ComponentHealth,
) -> (RetrievalDegradedMode, Option<String>) {
    if lexical.status != ComponentStatus::Healthy || !lexical.capabilities.lexical {
        return (
            RetrievalDegradedMode::Unavailable,
            mandatory_failure_reason(lexical, "lexical"),
        );
    }
    if semantic.status != ComponentStatus::Healthy || !semantic.capabilities.semantic {
        let mode = if scip.capabilities.graph {
            RetrievalDegradedMode::NoSemantic
        } else {
            RetrievalDegradedMode::LexicalOnly
        };
        return (mode, mandatory_failure_reason(semantic, "semantic"));
    }
    if scip.status != ComponentStatus::Healthy || !scip.capabilities.graph {
        return (RetrievalDegradedMode::NoScip, scip.degraded_reason.clone());
    }
    (RetrievalDegradedMode::Full, None)
}

fn mandatory_failure_reason(component: &ComponentHealth, name: &str) -> Option<String> {
    let state = if component.status == ComponentStatus::Unavailable {
        "unavailable"
    } else {
        "degraded"
    };
    component
        .degraded_reason
        .clone()
        .or_else(|| Some(format!("mandatory_{name}_{state}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::SidecarCapabilities;

    fn component(
        name: &str,
        status: ComponentStatus,
        reason: Option<&str>,
        capabilities: crate::capabilities::SidecarCapabilities,
    ) -> ComponentHealth {
        ComponentHealth {
            name: name.into(),
            status,
            latency_ms: None,
            detail: String::new(),
            degraded_reason: reason.map(str::to_string),
            capabilities,
        }
    }

    #[test]
    fn matrix_rows_match_design_doc() {
        let production = crate::capabilities::SidecarCapabilities::production_stack();
        let lexical_up = component("lexical", ComponentStatus::Healthy, None, production);
        let semantic_up = component("semantic", ComponentStatus::Healthy, None, production);
        let scip_up = component("scip", ComponentStatus::Healthy, None, production);
        assert_eq!(
            derive_degraded_mode(&lexical_up, &semantic_up, &scip_up).0,
            RetrievalDegradedMode::Full
        );

        let scip_down = component(
            "scip",
            ComponentStatus::Unavailable,
            Some("scip_unavailable"),
            SidecarCapabilities::NONE,
        );
        assert_eq!(
            derive_degraded_mode(&lexical_up, &semantic_up, &scip_down).0,
            RetrievalDegradedMode::NoScip
        );

        let semantic_down = component(
            "semantic",
            ComponentStatus::Unavailable,
            Some("embedded_vector_index_unavailable"),
            SidecarCapabilities::NONE,
        );
        assert_eq!(
            derive_degraded_mode(&lexical_up, &semantic_down, &scip_up).0,
            RetrievalDegradedMode::NoSemantic
        );
        assert_eq!(
            derive_degraded_mode(&lexical_up, &semantic_down, &scip_down).0,
            RetrievalDegradedMode::LexicalOnly
        );

        let lexical_down = component(
            "lexical",
            ComponentStatus::Unavailable,
            Some("lexical_unreachable"),
            SidecarCapabilities::NONE,
        );
        assert_eq!(
            derive_degraded_mode(&lexical_down, &semantic_up, &scip_up).0,
            RetrievalDegradedMode::Unavailable
        );
    }

    #[test]
    fn stub_stack_never_reports_full() {
        let lexical_only = SidecarCapabilities {
            lexical: true,
            semantic: false,
            graph: false,
        };
        let lexical_stub = component(
            "lexical",
            ComponentStatus::Degraded,
            Some("lexical_stub"),
            lexical_only,
        );
        let semantic_stub = component(
            "semantic",
            ComponentStatus::Degraded,
            Some("embedded_vector_index_unavailable"),
            SidecarCapabilities::NONE,
        );
        let scip_stub = component(
            "scip",
            ComponentStatus::Degraded,
            Some("scip_stub"),
            SidecarCapabilities::NONE,
        );
        assert_ne!(
            derive_degraded_mode(&lexical_stub, &semantic_stub, &scip_stub).0,
            RetrievalDegradedMode::Full
        );
    }
}
