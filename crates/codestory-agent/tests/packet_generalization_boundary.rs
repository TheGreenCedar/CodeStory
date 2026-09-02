//! Black-box boundary tests for repository-derived packet compilation.

use codestory_agent::packet_plan::build_packet_plan_with_extra;
use codestory_contracts::api::PacketBudgetModeDto;

#[test]
fn domain_vocabulary_does_not_create_answer_shaped_queries() {
    for noun in [
        "client",
        "cache",
        "formatter",
        "mapper",
        "request",
        "animation",
        "database",
        "router",
    ] {
        let question = format!("Explain how the {noun} works in this repository.");
        let plan = build_packet_plan_with_extra(&question, PacketBudgetModeDto::Standard, &[]);
        assert_eq!(plan.queries.len(), 1, "unexpected policy for {noun}");
        assert_eq!(plan.queries[0].query, question);
    }
}

#[test]
fn encoded_or_literal_brand_tokens_have_no_special_seed_authority() {
    let brand = build_packet_plan_with_extra(
        "how does swr cache requests?",
        PacketBudgetModeDto::Standard,
        &[],
    );
    let neutral = build_packet_plan_with_extra(
        "how does xyz cache requests?",
        PacketBudgetModeDto::Standard,
        &[],
    );
    assert_eq!(brand.queries.len(), 1);
    assert_eq!(neutral.queries.len(), 1);
    assert_eq!(brand.queries[0].purpose, neutral.queries[0].purpose);
}

#[test]
fn bijective_renames_preserve_seed_shape() {
    let original = build_packet_plan_with_extra(
        "Trace Foo::bar in src/foo.rs calling Baz::qux",
        PacketBudgetModeDto::Standard,
        &[],
    );
    let renamed = build_packet_plan_with_extra(
        "Trace Alpha::beta in src/alpha.rs calling Gamma::delta",
        PacketBudgetModeDto::Standard,
        &[],
    );
    let purposes = |plan: &codestory_contracts::api::PacketPlanDto| {
        plan.queries
            .iter()
            .map(|query| query.purpose.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(purposes(&original), purposes(&renamed));
    assert_eq!(original.queries.len(), renamed.queries.len());
}

#[test]
fn free_queries_are_explicit_and_have_no_protection_role() {
    let plan = build_packet_plan_with_extra(
        "ordinary wording",
        PacketBudgetModeDto::Standard,
        &["caller supplied retrieval query".into()],
    );
    assert_eq!(plan.queries.len(), 2);
    assert_eq!(plan.queries[1].purpose, "typed free-query retrieval seed");
    let serialized = serde_json::to_value(plan).unwrap();
    assert!(serialized.get("coverage_role").is_none());
    assert!(serialized.get("task_class").is_none());
    assert!(serialized.get("obligations").is_none());
}
