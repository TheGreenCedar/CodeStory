use codestory_api::{
    AgentCustomRetrievalConfigDto, AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto,
    AgentRetrievalProfileSelectionDto, EdgeKind, NodeKind, TrailCallerScope, TrailDirection,
    TrailMode,
};

#[derive(Debug, Clone)]
pub(crate) struct TrailPlan {
    pub mode: TrailMode,
    pub depth: u32,
    pub direction: TrailDirection,
    pub caller_scope: TrailCallerScope,
    pub edge_filter: Vec<EdgeKind>,
    pub node_filter: Vec<NodeKind>,
    pub max_nodes: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedProfile {
    pub preset: AgentRetrievalPresetDto,
    pub policy_mode: AgentRetrievalPolicyModeDto,
    pub trail_plans: Vec<TrailPlan>,
    pub include_edge_occurrences: bool,
    pub enable_source_reads: bool,
}

pub(crate) fn resolve_profile(
    prompt: &str,
    selection: &AgentRetrievalProfileSelectionDto,
) -> ResolvedProfile {
    match selection {
        AgentRetrievalProfileSelectionDto::Auto => {
            let preset = route_auto_preset(prompt);
            from_preset(preset)
        }
        AgentRetrievalProfileSelectionDto::Preset { preset } => from_preset(*preset),
        AgentRetrievalProfileSelectionDto::Custom { config } => from_custom(config),
    }
}

pub(crate) fn route_auto_preset(prompt: &str) -> AgentRetrievalPresetDto {
    let normalized = prompt.to_ascii_lowercase();

    if contains_any(&normalized, &["inherit", "override", "base class", "trait"]) {
        return AgentRetrievalPresetDto::Inheritance;
    }

    if contains_any(
        &normalized,
        &[
            "call flow",
            "callflow",
            "sequence",
            "who calls",
            "execution path",
            "runtime path",
        ],
    ) {
        return AgentRetrievalPresetDto::Callflow;
    }

    if contains_any(
        &normalized,
        &[
            "impact",
            "blast radius",
            "what breaks",
            "downstream",
            "upstream",
            "depend",
        ],
    ) {
        return AgentRetrievalPresetDto::Impact;
    }

    AgentRetrievalPresetDto::Architecture
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn from_preset(preset: AgentRetrievalPresetDto) -> ResolvedProfile {
    ResolvedProfile {
        preset,
        policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
        trail_plans: preset_trail_plans(preset),
        include_edge_occurrences: matches!(
            preset,
            AgentRetrievalPresetDto::Callflow | AgentRetrievalPresetDto::Impact
        ),
        enable_source_reads: true,
    }
}

fn from_custom(config: &AgentCustomRetrievalConfigDto) -> ResolvedProfile {
    let depth = if config.depth == 0 {
        0
    } else {
        config.depth.max(1)
    };
    let max_nodes = config.max_nodes.clamp(10, 100_000);

    ResolvedProfile {
        preset: AgentRetrievalPresetDto::Architecture,
        policy_mode: AgentRetrievalPolicyModeDto::CompletenessFirst,
        trail_plans: vec![TrailPlan {
            mode: TrailMode::Neighborhood,
            depth,
            direction: config.direction,
            caller_scope: TrailCallerScope::IncludeTestsAndBenches,
            edge_filter: config.edge_filter.clone(),
            node_filter: config.node_filter.clone(),
            max_nodes,
        }],
        include_edge_occurrences: config.include_edge_occurrences,
        enable_source_reads: config.enable_source_reads,
    }
}

fn preset_trail_plans(preset: AgentRetrievalPresetDto) -> Vec<TrailPlan> {
    match preset {
        AgentRetrievalPresetDto::Architecture => vec![TrailPlan {
            mode: TrailMode::Neighborhood,
            depth: 2,
            direction: TrailDirection::Both,
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: vec![],
            node_filter: vec![],
            max_nodes: 600,
        }],
        AgentRetrievalPresetDto::Callflow => vec![TrailPlan {
            mode: TrailMode::AllReferenced,
            depth: 4,
            direction: TrailDirection::Outgoing,
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: vec![EdgeKind::CALL, EdgeKind::OVERRIDE, EdgeKind::MEMBER],
            node_filter: vec![],
            max_nodes: 900,
        }],
        AgentRetrievalPresetDto::Inheritance => vec![TrailPlan {
            mode: TrailMode::AllReferenced,
            depth: 6,
            direction: TrailDirection::Both,
            caller_scope: TrailCallerScope::IncludeTestsAndBenches,
            edge_filter: vec![EdgeKind::INHERITANCE, EdgeKind::OVERRIDE, EdgeKind::MEMBER],
            node_filter: vec![],
            max_nodes: 900,
        }],
        AgentRetrievalPresetDto::Impact => vec![TrailPlan {
            mode: TrailMode::AllReferencing,
            depth: 4,
            direction: TrailDirection::Incoming,
            caller_scope: TrailCallerScope::IncludeTestsAndBenches,
            edge_filter: vec![
                EdgeKind::CALL,
                EdgeKind::USAGE,
                EdgeKind::TYPE_USAGE,
                EdgeKind::IMPORT,
                EdgeKind::INCLUDE,
            ],
            node_filter: vec![],
            max_nodes: 1200,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_profile_routes_callflow_keywords() {
        let profile = resolve_profile(
            "Show call flow and sequence for checkout",
            &AgentRetrievalProfileSelectionDto::Auto,
        );

        assert_eq!(profile.preset, AgentRetrievalPresetDto::Callflow);
        assert_eq!(
            profile.policy_mode,
            AgentRetrievalPolicyModeDto::LatencyFirst
        );
    }

    #[test]
    fn auto_profile_defaults_to_architecture() {
        let profile = resolve_profile(
            "Explain this subsystem",
            &AgentRetrievalProfileSelectionDto::Auto,
        );

        assert_eq!(profile.preset, AgentRetrievalPresetDto::Architecture);
        assert_eq!(
            profile.policy_mode,
            AgentRetrievalPolicyModeDto::LatencyFirst
        );
    }

    #[test]
    fn custom_profile_uses_completeness_policy() {
        let profile = resolve_profile(
            "Deep dive",
            &AgentRetrievalProfileSelectionDto::Custom {
                config: AgentCustomRetrievalConfigDto::default(),
            },
        );

        assert_eq!(
            profile.policy_mode,
            AgentRetrievalPolicyModeDto::CompletenessFirst
        );
    }
}
