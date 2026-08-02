//! Packet plan query normalization and deduplication.

use codestory_contracts::api::PacketPlanDto;
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

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{PacketPlanDto, PacketPlanQueryDto, PacketTaskClassDto};

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
            probe_resolutions: Vec::new(),
            trace: Vec::new(),
        };
        dedupe_packet_plan_queries(&mut plan);
        assert_eq!(plan.queries.len(), 1);
    }
}
