use codestory_retrieval::{
    ComponentHealth, ComponentStatus, RetrievalDegradedMode, RetrievalStageKind, classify_query,
    derive_degraded_mode, plan_query,
};

fn component(
    name: &str,
    status: ComponentStatus,
    reason: Option<&str>,
    capabilities: codestory_retrieval::SidecarCapabilities,
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
fn degraded_mode_planner_matrix() {
    let production = codestory_retrieval::SidecarCapabilities::production_stack();
    let zoekt_up = component("zoekt", ComponentStatus::Healthy, None, production);
    let qdrant_up = component("qdrant", ComponentStatus::Healthy, None, production);
    let scip_up = component("scip", ComponentStatus::Healthy, None, production);
    let features = classify_query("handler");

    let (full, _) = derive_degraded_mode(&zoekt_up, &qdrant_up, &scip_up);
    assert_eq!(full, RetrievalDegradedMode::Full);
    assert!(
        plan_query(&features, full)
            .stages
            .iter()
            .any(|s| s.kind == RetrievalStageKind::Stage1bQdrantSemantic)
    );

    let qdrant_down = component(
        "qdrant",
        ComponentStatus::Unavailable,
        Some("qdrant_unreachable"),
        codestory_retrieval::SidecarCapabilities::NONE,
    );
    let (no_semantic, _) = derive_degraded_mode(&zoekt_up, &qdrant_down, &scip_up);
    assert_eq!(no_semantic, RetrievalDegradedMode::NoSemantic);
    assert!(
        !plan_query(&features, no_semantic)
            .stages
            .iter()
            .any(|s| s.kind == RetrievalStageKind::Stage1bQdrantSemantic)
    );

    let scip_down = component(
        "scip",
        ComponentStatus::Unavailable,
        Some("scip_unavailable"),
        codestory_retrieval::SidecarCapabilities::NONE,
    );
    let (no_scip, _) = derive_degraded_mode(&zoekt_up, &qdrant_up, &scip_down);
    assert_eq!(no_scip, RetrievalDegradedMode::NoScip);
    assert!(
        !plan_query(&features, no_scip)
            .stages
            .iter()
            .any(|s| s.kind == RetrievalStageKind::Stage0ScipAnchor)
    );

    let (lexical_only, _) = derive_degraded_mode(&zoekt_up, &qdrant_down, &scip_down);
    assert_eq!(lexical_only, RetrievalDegradedMode::LexicalOnly);

    let zoekt_down = component(
        "zoekt",
        ComponentStatus::Unavailable,
        Some("zoekt_unreachable"),
        codestory_retrieval::SidecarCapabilities::NONE,
    );
    let (unavailable, _) = derive_degraded_mode(&zoekt_down, &qdrant_up, &scip_up);
    assert_eq!(unavailable, RetrievalDegradedMode::Unavailable);
    assert!(plan_query(&features, unavailable).stages.is_empty());
}
