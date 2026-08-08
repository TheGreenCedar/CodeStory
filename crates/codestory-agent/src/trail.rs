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
