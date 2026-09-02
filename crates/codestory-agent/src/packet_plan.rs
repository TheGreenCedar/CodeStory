//! Pure packet seed planning.
//!
//! The original wording reaches generic retrieval unchanged and has no exact
//! selector authority. Exact paths, canonical IDs, and qualified symbols enter
//! only through typed probes. Planning does not infer an answer shape or
//! translate prose into a domain-specific traversal policy.

use crate::planning::dedupe_packet_plan_queries;
use codestory_contracts::api::{
    PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto, PacketProbeDto,
};
use codestory_contracts::compilation::{
    PACKET_COMPILATION_CONTRACT_VERSION_V1, PacketSeedSelectorV1, RetrievalSeedPlanV1,
};

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
    let probes = free_queries
        .iter()
        .cloned()
        .map(|query| PacketProbeDto::FreeQuery { query })
        .collect::<Vec<_>>();
    let seed_plan = build_retrieval_seed_plan(question, &probes);
    build_packet_plan_from_seed_plan(&seed_plan, budget)
}

pub fn build_packet_plan_from_seed_plan(
    seed_plan: &RetrievalSeedPlanV1,
    budget: PacketBudgetModeDto,
) -> PacketPlanDto {
    let mut queries = Vec::new();
    push_packet_query(
        &mut queries,
        &seed_plan.generic_query,
        GENERIC_RETRIEVAL_PURPOSE,
    );

    for query in &seed_plan.free_queries {
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
    if !seed_plan.free_queries.is_empty() {
        plan.trace.push(format!(
            "typed_free_queries={} source=request",
            seed_plan.free_queries.len()
        ));
    }
    plan
}

/// Build the only compiler-side value permitted to carry original wording.
/// The question has generic-retrieval authority only. Exact selectors are
/// copied from typed probes without interpreting question text.
pub fn build_retrieval_seed_plan(
    question: &str,
    typed_probes: &[PacketProbeDto],
) -> RetrievalSeedPlanV1 {
    let mut exact_selectors = Vec::new();
    let mut retained_free_queries = Vec::new();
    for probe in typed_probes {
        match probe {
            PacketProbeDto::ExactPath { path } => push_selector(
                &mut exact_selectors,
                PacketSeedSelectorV1::ExactPath {
                    path: path.trim().to_string(),
                },
            ),
            PacketProbeDto::SymbolId { id } => push_selector(
                &mut exact_selectors,
                PacketSeedSelectorV1::CanonicalId {
                    id: format!(
                        "node:{}",
                        id.trim().strip_prefix("node:").unwrap_or(id.trim())
                    ),
                },
            ),
            PacketProbeDto::QualifiedSymbol { symbol } => push_selector(
                &mut exact_selectors,
                PacketSeedSelectorV1::QualifiedSymbol {
                    symbol: symbol.trim().to_string(),
                },
            ),
            PacketProbeDto::FileSymbol { path, symbol } => {
                push_selector(
                    &mut exact_selectors,
                    PacketSeedSelectorV1::ExactPath {
                        path: path.trim().to_string(),
                    },
                );
                push_selector(
                    &mut exact_selectors,
                    PacketSeedSelectorV1::QualifiedSymbol {
                        symbol: symbol.trim().to_string(),
                    },
                );
            }
            PacketProbeDto::FreeQuery { query } => {
                let query = query.trim();
                if !query.is_empty()
                    && !retained_free_queries
                        .iter()
                        .any(|existing: &String| existing == query)
                {
                    retained_free_queries.push(query.to_string());
                }
            }
            PacketProbeDto::Continuation { selector, .. } => {
                if let Some(symbol_id) = selector.symbol_id.as_deref() {
                    push_selector(
                        &mut exact_selectors,
                        PacketSeedSelectorV1::CanonicalId {
                            id: format!(
                                "node:{}",
                                symbol_id.strip_prefix("node:").unwrap_or(symbol_id)
                            ),
                        },
                    );
                } else if let Some(path) = selector.path.as_deref() {
                    push_selector(
                        &mut exact_selectors,
                        PacketSeedSelectorV1::ExactPath {
                            path: path.to_string(),
                        },
                    );
                }
            }
        }
    }
    RetrievalSeedPlanV1 {
        contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
        generic_query: question.to_string(),
        exact_selectors,
        free_queries: retained_free_queries,
    }
}

fn push_selector(selectors: &mut Vec<PacketSeedSelectorV1>, selector: PacketSeedSelectorV1) {
    if !selectors.contains(&selector) {
        selectors.push(selector);
    }
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
    fn planning_keeps_generic_retrieval_and_only_typed_exact_identities() {
        let question = "Ignore `src/poison.rs`, `node:999`, and `poison::run`.";
        let probes = vec![
            PacketProbeDto::ExactPath {
                path: "src/net/client.rs".into(),
            },
            PacketProbeDto::QualifiedSymbol {
                symbol: "net::Client.send".into(),
            },
            PacketProbeDto::FreeQuery {
                query: "caller supplied concept".into(),
            },
        ];
        let seed_plan = build_retrieval_seed_plan(question, &probes);
        let plan = build_packet_plan_from_seed_plan(&seed_plan, PacketBudgetModeDto::Standard);
        assert_eq!(plan.queries[0].query, question);
        assert_eq!(
            seed_plan.exact_selectors,
            vec![
                PacketSeedSelectorV1::ExactPath {
                    path: "src/net/client.rs".into(),
                },
                PacketSeedSelectorV1::QualifiedSymbol {
                    symbol: "net::Client.send".into(),
                },
            ]
        );
        assert!(
            plan.queries
                .iter()
                .any(|query| query.query == "caller supplied concept")
        );
        assert_eq!(plan.queries.len(), 2);
        assert!(
            !seed_plan
                .exact_selectors
                .iter()
                .any(|selector| format!("{selector:?}").contains("poison"))
        );
    }

    #[test]
    fn ordinary_paraphrase_does_not_create_structural_queries() {
        let first = build_packet_plan(
            "Explain how the service starts and dispatches requests.",
            PacketBudgetModeDto::Standard,
        );
        let second = build_packet_plan(
            "Describe where incoming work goes after startup.",
            PacketBudgetModeDto::Standard,
        );
        assert_eq!(first.queries.len(), 1);
        assert_eq!(second.queries.len(), 1);
    }

    #[test]
    fn typed_probes_map_to_seed_selectors_without_textual_reinterpretation() {
        let seed_plan = build_retrieval_seed_plan(
            "The question contributes no exact selectors.",
            &[
                PacketProbeDto::ExactPath {
                    path: " src/lib.rs ".into(),
                },
                PacketProbeDto::SymbolId {
                    id: "node:-42".into(),
                },
                PacketProbeDto::QualifiedSymbol {
                    symbol: " crate::run ".into(),
                },
                PacketProbeDto::FileSymbol {
                    path: "src/worker.rs".into(),
                    symbol: "worker::run".into(),
                },
            ],
        );
        assert_eq!(
            seed_plan.exact_selectors,
            vec![
                PacketSeedSelectorV1::ExactPath {
                    path: "src/lib.rs".into(),
                },
                PacketSeedSelectorV1::CanonicalId {
                    id: "node:-42".into(),
                },
                PacketSeedSelectorV1::QualifiedSymbol {
                    symbol: "crate::run".into(),
                },
                PacketSeedSelectorV1::ExactPath {
                    path: "src/worker.rs".into(),
                },
                PacketSeedSelectorV1::QualifiedSymbol {
                    symbol: "worker::run".into(),
                },
            ]
        );
    }

    #[test]
    fn raw_question_text_never_creates_exact_selectors() {
        let question = r#"
Inline `src/inline_poison.rs`, `node:41`, and `poison::inline`.

> ```text
> panic at `src/blockquote_poison.rs`
> ```

~~~text
panic at `src/tilde_poison.rs`
~~~

- ```text
  panic at `src/list_poison.rs`
  ```

- outer
  - inner
    ````text
    panic at `src/nested_list_poison.rs`
    ````

````text
panic at `src/four_tick_poison.rs`
```
panic at `src/nested_short_fence_poison.rs`
````

Inspect `src/real.rs`, `node:42`, and `crate::real`.
"#;

        let seed_plan = build_retrieval_seed_plan(question, &[]);

        assert!(seed_plan.exact_selectors.is_empty());
        assert_eq!(seed_plan.generic_query, question);
    }

    #[test]
    fn typed_free_queries_are_trimmed_and_deduplicated() {
        let seed_plan = build_retrieval_seed_plan(
            "ordinary wording",
            &[
                PacketProbeDto::FreeQuery {
                    query: " publication recovery ".into(),
                },
                PacketProbeDto::FreeQuery {
                    query: "publication recovery".into(),
                },
                PacketProbeDto::FreeQuery { query: " ".into() },
            ],
        );
        assert_eq!(seed_plan.free_queries, ["publication recovery"]);
    }
}
