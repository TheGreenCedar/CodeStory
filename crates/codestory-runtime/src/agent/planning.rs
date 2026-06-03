//! Packet plan construction, query deduplication, and subquery hybrid policy.

use codestory_contracts::api::{
    AgentHybridWeightsDto, PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto,
};
use std::collections::HashSet;

pub(crate) fn normalize_packet_subquery(query: &str) -> String {
    query
        .split_whitespace()
        .filter(|term| !PACKET_SUBQUERY_STOP_WORDS.contains(term))
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

const PACKET_SUBQUERY_STOP_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "for", "from", "with", "into", "about", "how", "what", "where",
    "when", "which", "that", "this", "these", "those", "does", "do", "is", "are", "was", "were",
];

pub(crate) fn dedupe_packet_plan_queries(plan: &mut PacketPlanDto) {
    let mut seen = HashSet::<String>::new();
    let mut deduped = Vec::with_capacity(plan.queries.len());
    for query in plan.queries.drain(..) {
        let key = normalize_packet_subquery(&query.query);
        if key.len() < 2 {
            deduped.push(query);
            continue;
        }
        if seen.insert(key) {
            deduped.push(query);
        }
    }
    plan.queries = deduped;
}

pub(crate) fn packet_subquery_hybrid_weights(
    budget: PacketBudgetModeDto,
    query: &PacketPlanQueryDto,
) -> Option<AgentHybridWeightsDto> {
    if !matches!(
        budget,
        PacketBudgetModeDto::Compact | PacketBudgetModeDto::Standard
    ) {
        return None;
    }
    let normalized = normalize_packet_subquery(&query.query);
    let exact_signal = query.purpose.contains("symbol")
        || query.purpose.contains("concrete")
        || query.purpose.contains("flow anchor")
        || normalized.contains("::")
        || (normalized.contains('/') && normalized.contains('.'))
        || (normalized.contains('/') && normalized.len() >= 12);
    if exact_signal {
        Some(AgentHybridWeightsDto {
            lexical: Some(1.0),
            semantic: Some(0.0),
            graph: Some(0.0),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{PacketPlanDto, PacketTaskClassDto};

    #[test]
    fn test_dedupe_packet_plan_queries_removes_stop_word_variants() {
        let mut plan = PacketPlanDto {
            task_class: PacketTaskClassDto::ArchitectureExplanation,
            inferred_task_class: true,
            queries: vec![
                PacketPlanQueryDto {
                    query: "extension host startup flow".to_string(),
                    purpose: "a".to_string(),
                },
                PacketPlanQueryDto {
                    query: "the extension host startup flow".to_string(),
                    purpose: "b".to_string(),
                },
            ],
            trace: Vec::new(),
        };
        dedupe_packet_plan_queries(&mut plan);
        assert_eq!(plan.queries.len(), 1);
    }

    #[test]
    fn test_packet_subquery_hybrid_weights_lexical_for_symbol_probe() {
        let query = PacketPlanQueryDto {
            query: "ExtHostCommands".to_string(),
            purpose: "concrete symbol probe".to_string(),
        };
        let weights = packet_subquery_hybrid_weights(PacketBudgetModeDto::Compact, &query)
            .expect("expected lexical-only weights");
        assert!(weights.semantic.is_some_and(|value| value <= f32::EPSILON));
    }
}
