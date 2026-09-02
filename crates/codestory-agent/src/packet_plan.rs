//! Pure packet seed planning.
//!
//! The original wording may reach generic retrieval unchanged. Beyond that,
//! planning recognizes only identities the caller delimited as inline code:
//! repository paths, canonical IDs, and qualified symbols. Typed free-query
//! probes remain generic retrieval seeds. Planning does not infer an answer
//! shape or translate prose into a domain-specific traversal policy.

use crate::planning::dedupe_packet_plan_queries;
use crate::text::exact_symbol_query_terms;
use codestory_contracts::api::{PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto};
use codestory_contracts::compilation::{
    PACKET_COMPILATION_CONTRACT_VERSION_V1, PacketSeedSelectorV1, RetrievalSeedPlanV1,
};
use std::path::{Component, Path};

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
    let seed_plan = build_retrieval_seed_plan(question, free_queries);
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
/// Extraction is syntactic: inline-code repository paths, canonical `node:`
/// IDs, and qualified symbols only. Natural-language relations are not
/// inferred.
pub fn build_retrieval_seed_plan(question: &str, free_queries: &[String]) -> RetrievalSeedPlanV1 {
    let mut exact_selectors = Vec::new();
    let explicit_fragments = inline_code_spans(question);
    for path in explicit_source_paths(&explicit_fragments) {
        push_selector(
            &mut exact_selectors,
            PacketSeedSelectorV1::ExactPath { path },
        );
    }
    for id in explicit_canonical_ids(&explicit_fragments) {
        push_selector(
            &mut exact_selectors,
            PacketSeedSelectorV1::CanonicalId { id },
        );
    }
    for symbol in explicit_qualified_symbols(&explicit_fragments) {
        push_selector(
            &mut exact_selectors,
            PacketSeedSelectorV1::QualifiedSymbol { symbol },
        );
    }
    let mut retained_free_queries = Vec::new();
    for query in free_queries {
        let query = query.trim();
        if !query.is_empty()
            && !retained_free_queries
                .iter()
                .any(|existing: &String| existing == query)
        {
            retained_free_queries.push(query.to_string());
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

fn inline_code_spans(question: &str) -> Vec<&str> {
    #[derive(Clone, Copy)]
    struct Fence {
        marker: u8,
        length: usize,
    }

    let mut spans = Vec::new();
    let mut fence: Option<Fence> = None;
    for line in question.split_inclusive('\n') {
        let content = markdown_container_content(line);
        if let Some(active) = fence {
            if markdown_fence_run(content).is_some_and(|(marker, length, suffix)| {
                marker == active.marker && length >= active.length && suffix.trim().is_empty()
            }) {
                fence = None;
            }
            continue;
        }
        if let Some((marker, length, suffix)) = markdown_fence_run(content)
            && (marker != b'`' || !suffix.contains('`'))
        {
            fence = Some(Fence { marker, length });
            continue;
        }

        let bytes = line.as_bytes();
        let mut index = 0;
        let mut opening = None;
        while index < bytes.len() {
            if bytes[index] != b'`' {
                index += 1;
                continue;
            }
            let run_start = index;
            while index < bytes.len() && bytes[index] == b'`' {
                index += 1;
            }
            if index - run_start != 1 {
                opening = None;
                continue;
            }
            match opening.take() {
                Some(start) if start < run_start => spans.push(&line[start..run_start]),
                Some(_) => {}
                None => opening = Some(index),
            }
        }
    }
    spans
}

fn markdown_container_content(mut line: &str) -> &str {
    loop {
        let mut indent = 0;
        while indent < 3 && line.as_bytes().get(indent) == Some(&b' ') {
            indent += 1;
        }
        let after_indent = &line[indent..];
        if let Some(after_quote) = after_indent.strip_prefix('>') {
            line = strip_markdown_container_padding(after_quote);
            continue;
        }
        if let Some(after_list) = markdown_list_item_content(after_indent) {
            line = strip_markdown_container_padding(after_list);
            continue;
        }
        return after_indent;
    }
}

fn strip_markdown_container_padding(line: &str) -> &str {
    line.trim_start_matches([' ', '\t'])
}

fn markdown_list_item_content(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    if matches!(bytes.first(), Some(b'-' | b'+' | b'*'))
        && matches!(bytes.get(1), Some(b' ' | b'\t'))
    {
        return Some(&line[2..]);
    }

    let digits = bytes
        .iter()
        .take(9)
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0
        || !matches!(bytes.get(digits), Some(b'.' | b')'))
        || !matches!(bytes.get(digits + 1), Some(b' ' | b'\t'))
    {
        return None;
    }
    Some(&line[digits + 2..])
}

fn markdown_fence_run(line: &str) -> Option<(u8, usize, &str)> {
    let bytes = line.as_bytes();
    let marker = *bytes.first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let length = bytes.iter().take_while(|byte| **byte == marker).count();
    (length >= 3).then(|| (marker, length, &line[length..]))
}

fn explicit_canonical_ids(explicit_fragments: &[&str]) -> Vec<String> {
    explicit_fragments
        .iter()
        .flat_map(|fragment| fragment.split_whitespace())
        .map(|token| {
            token.trim_matches(|ch: char| {
                matches!(
                    ch,
                    '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '.'
                )
            })
        })
        .filter(|token| {
            token
                .strip_prefix("node:")
                .is_some_and(|id| id.parse::<i64>().is_ok())
        })
        .map(str::to_string)
        .collect()
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

fn explicit_qualified_symbols(explicit_fragments: &[&str]) -> Vec<String> {
    explicit_fragments
        .iter()
        .flat_map(|fragment| exact_symbol_query_terms(fragment))
        .filter(|candidate| {
            (candidate.contains("::") || candidate.contains('.'))
                && !candidate.contains("://")
                && !candidate.contains(['/', '\\'])
                && codestory_contracts::language_support::language_support_profile_for_path(Some(
                    source_path_without_location_suffix(candidate),
                ))
                .is_none()
        })
        .collect()
}

fn explicit_source_paths(explicit_fragments: &[&str]) -> Vec<String> {
    let mut paths = Vec::new();
    for token in explicit_fragments
        .iter()
        .flat_map(|fragment| fragment.split_whitespace())
    {
        let mut candidate = token.trim_matches(|ch: char| {
            matches!(
                ch,
                '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';'
            )
        });
        if candidate.is_empty() || candidate.contains("://") {
            continue;
        }
        if codestory_contracts::language_support::language_support_profile_for_path(Some(candidate))
            .is_none()
        {
            candidate = candidate.trim_end_matches('.');
        }
        candidate = source_path_without_location_suffix(candidate);
        if codestory_contracts::language_support::language_support_profile_for_path(Some(candidate))
            .is_some()
            && is_project_relative_source_path(candidate)
            && !paths.iter().any(|path: &String| path == candidate)
        {
            paths.push(candidate.to_string());
        }
    }
    paths
}

fn is_project_relative_source_path(candidate: &str) -> bool {
    if candidate.starts_with(['/', '\\'])
        || candidate
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':')
    {
        return false;
    }
    let path = Path::new(candidate);
    !path.is_absolute()
        && path.components().all(|component| {
            !matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn source_path_without_location_suffix(candidate: &str) -> &str {
    if let Some((path, line)) = candidate.rsplit_once(':')
        && !path.is_empty()
        && line.chars().all(|ch| ch.is_ascii_digit())
    {
        return path;
    }
    candidate
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
    fn planning_keeps_only_generic_retrieval_and_explicit_identities() {
        let question =
            "Explain the client transport through `src/net/client.rs:42` and `net::Client.send`.";
        let seed_plan =
            build_retrieval_seed_plan(question, &["caller supplied concept".to_string()]);
        let plan = build_packet_plan_from_seed_plan(&seed_plan, PacketBudgetModeDto::Standard);
        assert_eq!(plan.queries[0].query, question);
        assert!(seed_plan.exact_selectors.iter().any(|selector| selector
            == &PacketSeedSelectorV1::ExactPath {
                path: "src/net/client.rs".to_string(),
            }));
        assert!(seed_plan.exact_selectors.iter().any(|selector| selector
            == &PacketSeedSelectorV1::QualifiedSymbol {
                symbol: "net::Client.send".to_string(),
            }));
        assert!(
            plan.queries
                .iter()
                .any(|query| query.query == "caller supplied concept")
        );
        assert_eq!(plan.queries.len(), 2);
        assert!(!plan.queries.iter().any(|query| {
            query.query == "src/net/client.rs" || query.query == "net::Client.send"
        }));
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
    fn prose_extraction_rejects_urls_and_paths_outside_the_project() {
        let seed_plan = build_retrieval_seed_plan(
            "Compare https://example.invalid/api with /tmp/secret.rs and ../outside.rs.",
            &[],
        );
        assert!(
            seed_plan.exact_selectors.is_empty(),
            "unsafe prose tokens became exact selectors: {:?}",
            seed_plan.exact_selectors
        );
    }

    #[test]
    fn source_location_is_not_also_treated_as_a_qualified_symbol() {
        let seed_plan = build_retrieval_seed_plan("Inspect `lib.rs:42` and `crate::run`.", &[]);
        assert_eq!(
            seed_plan.exact_selectors,
            vec![
                PacketSeedSelectorV1::ExactPath {
                    path: "lib.rs".into(),
                },
                PacketSeedSelectorV1::QualifiedSymbol {
                    symbol: "crate::run".into(),
                },
            ]
        );
    }

    #[test]
    fn canonical_identity_survives_sentence_punctuation() {
        let seed_plan = build_retrieval_seed_plan("Inspect `node:42`.", &[]);
        assert_eq!(
            seed_plan.exact_selectors,
            vec![PacketSeedSelectorV1::CanonicalId {
                id: "node:42".into(),
            }]
        );
    }

    #[test]
    fn unquoted_dotted_prose_cannot_consume_exact_admission_slots() {
        let question = (0..20)
            .map(|index| format!("Example{index}.js"))
            .chain(["e.g.".to_string(), "Node.js".to_string()])
            .collect::<Vec<_>>()
            .join(" ");
        let seed_plan = build_retrieval_seed_plan(&question, &[]);
        assert!(
            seed_plan.exact_selectors.is_empty(),
            "ordinary prose became exact selectors: {:?}",
            seed_plan.exact_selectors
        );
    }

    #[test]
    fn signed_node_ids_use_the_real_node_id_grammar() {
        let seed_plan = build_retrieval_seed_plan("Compare `node:-42` with `node:7`.", &[]);
        assert_eq!(
            seed_plan.exact_selectors,
            vec![
                PacketSeedSelectorV1::CanonicalId {
                    id: "node:-42".into(),
                },
                PacketSeedSelectorV1::CanonicalId {
                    id: "node:7".into(),
                },
            ]
        );
    }

    #[test]
    fn fenced_diagnostics_cannot_create_exact_selectors() {
        let fenced = (0..20)
            .map(|index| format!("src/generated_{index}.rs net::Generated{index} node:{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let question = format!(
            "Ignore this diagnostic:\n```text\n{fenced}\n```\nInspect `src/real.rs` and `crate::real` instead."
        );

        let seed_plan = build_retrieval_seed_plan(&question, &[]);

        assert_eq!(
            seed_plan.exact_selectors,
            vec![
                PacketSeedSelectorV1::ExactPath {
                    path: "src/real.rs".into(),
                },
                PacketSeedSelectorV1::QualifiedSymbol {
                    symbol: "crate::real".into(),
                },
            ]
        );
    }

    #[test]
    fn every_markdown_fence_shape_keeps_diagnostics_out_of_exact_selectors() {
        let question = r#"
> ```text
> panic at `src/blockquote_poison.rs`
> ```

~~~text
panic at `src/tilde_poison.rs`
~~~

- ```text
  panic at `src/list_poison.rs`
  ```

````text
panic at `src/four_tick_poison.rs`
```
panic at `src/nested_short_fence_poison.rs`
````

Inspect `src/real.rs`.
"#;

        let seed_plan = build_retrieval_seed_plan(question, &[]);

        assert_eq!(
            seed_plan.exact_selectors,
            vec![PacketSeedSelectorV1::ExactPath {
                path: "src/real.rs".into(),
            }]
        );
    }
}
