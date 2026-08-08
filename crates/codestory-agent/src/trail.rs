use codestory_contracts::api::{EdgeKind, GraphEdgeDto};

pub fn is_speculative_trail_edge(edge: &GraphEdgeDto) -> bool {
    if is_speculative_certainty_label(edge.certainty.as_deref()) {
        return true;
    }
    is_runtime_bridge_edge(edge.kind)
        && (is_probable_certainty_label(edge.certainty.as_deref())
            || edge.confidence.is_some_and(|confidence| {
                confidence < codestory_contracts::graph::ResolutionCertainty::CERTAIN_MIN
            }))
}

fn is_speculative_certainty_label(certainty: Option<&str>) -> bool {
    matches!(
        certainty.map(|value| value.to_ascii_lowercase()).as_deref(),
        Some("uncertain" | "speculative")
    )
}

fn is_probable_certainty_label(certainty: Option<&str>) -> bool {
    certainty
        .map(|value| value.eq_ignore_ascii_case("probable"))
        .unwrap_or(false)
}

fn is_runtime_bridge_edge(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::CALL | EdgeKind::MACRO_USAGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{EdgeId, NodeId};

    fn edge(
        id: &str,
        kind: EdgeKind,
        certainty: Option<&str>,
        confidence: Option<f32>,
    ) -> GraphEdgeDto {
        GraphEdgeDto {
            id: EdgeId(id.to_string()),
            source: NodeId("source".to_string()),
            target: NodeId("target".to_string()),
            kind,
            confidence,
            certainty: certainty.map(str::to_string),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
    }

    #[test]
    fn speculative_trail_edge_filters_uncertain_speculative_and_bridge_only_evidence() {
        assert!(is_speculative_trail_edge(&edge(
            "uncertain",
            EdgeKind::USAGE,
            Some("uncertain"),
            Some(1.0),
        )));
        assert!(is_speculative_trail_edge(&edge(
            "speculative",
            EdgeKind::USAGE,
            Some("SpEcUlAtIvE"),
            Some(1.0),
        )));
        assert!(is_speculative_trail_edge(&edge(
            "probable-call",
            EdgeKind::CALL,
            Some("probable"),
            Some(0.70),
        )));
        assert!(is_speculative_trail_edge(&edge(
            "low-confidence-macro",
            EdgeKind::MACRO_USAGE,
            Some("certain"),
            Some(0.54),
        )));
        assert!(!is_speculative_trail_edge(&edge(
            "certain-call",
            EdgeKind::CALL,
            Some("certain"),
            Some(0.85),
        )));
        assert!(!is_speculative_trail_edge(&edge(
            "probable-usage",
            EdgeKind::USAGE,
            Some("probable"),
            Some(0.70),
        )));
    }
}
