//! Prompt-blind Horizon A packet seed planning.
//!
//! Ordinary wording is forwarded unchanged to generic retrieval. Typed
//! free-query probes may add retrieval queries, but neither source receives
//! protection, materiality, sufficiency, or structural-traversal authority.

use crate::planning::dedupe_packet_plan_queries;
use codestory_contracts::api::{PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto};

const GENERIC_RETRIEVAL_PURPOSE: &str =
    "unchanged question for generic lexical and semantic retrieval";
const TYPED_FREE_QUERY_PURPOSE: &str = "typed free-query retrieval seed";

pub fn build_packet_plan(question: &str, budget: PacketBudgetModeDto) -> PacketPlanDto {
    build_packet_plan_with_extra(question, budget, &[])
}

pub fn build_packet_plan_with_extra(
    question: &str,
    budget: PacketBudgetModeDto,
    free_queries: &[String],
) -> PacketPlanDto {
    let mut queries = Vec::new();
    push_packet_query(&mut queries, question, GENERIC_RETRIEVAL_PURPOSE);
    for query in free_queries {
        push_packet_query(&mut queries, query, TYPED_FREE_QUERY_PURPOSE);
    }
    queries.truncate(packet_plan_query_cap(budget));

    let mut plan = PacketPlanDto {
        queries,
        probe_resolutions: Vec::new(),
        trace: vec!["retrieval=generic source=question".to_string()],
    };
    dedupe_packet_plan_queries(&mut plan);
    plan.trace
        .push(format!("planned_queries={}", plan.queries.len()));
    if !free_queries.is_empty() {
        plan.trace.push(format!(
            "typed_free_queries={} source=request",
            free_queries.len()
        ));
    }
    plan
}

pub fn packet_explicit_request_probe_queries(plan: &PacketPlanDto) -> Vec<String> {
    plan.queries
        .iter()
        .filter(|query| query.purpose == TYPED_FREE_QUERY_PURPOSE)
        .map(|query| query.query.clone())
        .collect()
}

pub fn packet_plan_query_is_typed_free_query(query: &PacketPlanQueryDto) -> bool {
    query.purpose == TYPED_FREE_QUERY_PURPOSE
}

pub fn packet_plan_annotation(plan: &PacketPlanDto) -> String {
    format!(
        "packet_plan retrieval=generic query_count={}",
        plan.queries.len()
    )
}

fn packet_plan_query_cap(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 20,
        PacketBudgetModeDto::Compact => 32,
        PacketBudgetModeDto::Standard => 48,
        PacketBudgetModeDto::Deep => 56,
    }
}

fn push_packet_query(queries: &mut Vec<PacketPlanQueryDto>, query: &str, purpose: &str) {
    let query = query.trim();
    if query.is_empty()
        || queries
            .iter()
            .any(|existing| existing.query.eq_ignore_ascii_case(query))
    {
        return;
    }
    queries.push(PacketPlanQueryDto {
        query: query.to_string(),
        purpose: purpose.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_question_is_the_only_implicit_query() {
        let question = "Trace Alpha::beta in src/alpha.rs through Gamma::delta";
        let plan = build_packet_plan(question, PacketBudgetModeDto::Standard);
        assert_eq!(plan.queries.len(), 1);
        assert_eq!(plan.queries[0].query, question);
        assert_eq!(plan.queries[0].purpose, GENERIC_RETRIEVAL_PURPOSE);
    }

    #[test]
    fn typed_free_queries_are_additional_generic_seeds() {
        let plan = build_packet_plan_with_extra(
            "ordinary wording",
            PacketBudgetModeDto::Standard,
            &["caller supplied concept".into()],
        );
        assert_eq!(plan.queries.len(), 2);
        assert_eq!(plan.queries[1].purpose, TYPED_FREE_QUERY_PURPOSE);
    }

    #[test]
    fn domain_vocabulary_does_not_change_plan_shape() {
        let first = build_packet_plan(
            "Explain client cache request finalization",
            PacketBudgetModeDto::Standard,
        );
        let second = build_packet_plan(
            "Explain alpha beta gamma delta",
            PacketBudgetModeDto::Standard,
        );
        assert_eq!(first.queries.len(), second.queries.len());
        assert_eq!(first.queries[0].purpose, second.queries[0].purpose);
    }
}
