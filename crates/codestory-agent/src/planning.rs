//! Packet plan query normalization and deduplication.

use codestory_contracts::api::{PacketPlanDto, PacketPlanQueryDto};
use std::collections::HashSet;

pub const PACKET_EXACT_SYMBOL_QUERY_PURPOSE: &str =
    "case-sensitive exact symbol identity from task wording";
pub const PACKET_FLOW_ROLE_QUERY_PURPOSE: &str =
    "flow-role symbol probe expanded from task wording";
pub const PACKET_CONCRETE_FILE_QUERY_PURPOSE: &str =
    "concrete file symbol probe expanded from task wording";
pub const PACKET_ADJACENT_VARIANT_QUERY_PURPOSE: &str = "synthetic adjacent-term symbol variant";
pub const PACKET_GENERIC_TERM_QUERY_PURPOSE: &str = "generic prompt-term symbol variant";
pub const PACKET_OWNER_MEMBER_QUERY_PURPOSE: &str = "owner/member phase probe";

pub fn packet_plan_query_is_exact_symbol_identity(query: &PacketPlanQueryDto) -> bool {
    query.purpose == PACKET_EXACT_SYMBOL_QUERY_PURPOSE
}

pub fn normalize_packet_subquery(query: &str) -> String {
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

pub fn dedupe_packet_plan_queries(plan: &mut PacketPlanDto) {
    let mut seen = HashSet::<String>::new();
    let mut deduped = Vec::with_capacity(plan.queries.len());
    for query in plan.queries.drain(..) {
        let key = if packet_plan_query_is_exact_symbol_identity(&query) {
            format!("exact-case:{}", query.query.trim())
        } else {
            normalize_packet_subquery(&query.query)
        };
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
            obligations: Default::default(),
            trace: Vec::new(),
        };
        dedupe_packet_plan_queries(&mut plan);
        assert_eq!(plan.queries.len(), 1);
    }
}
