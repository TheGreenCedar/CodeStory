//! Pure packet seed planning.
//!
//! The original wording reaches generic retrieval unchanged and has no exact
//! selector authority. Exact paths, canonical IDs, and qualified symbols enter
//! only through typed probes. Planning does not infer an answer shape or
//! translate prose into a domain-specific traversal policy.

use crate::planning::dedupe_packet_plan_queries;
use codestory_contracts::api::{
    PACKET_PROBE_MAX_TEXT_LENGTH, PacketBudgetModeDto, PacketPlanDto, PacketPlanQueryDto,
    PacketProbeDto,
};
use codestory_contracts::compilation::{
    PACKET_COMPILATION_CONTRACT_VERSION_V1, PacketSeedSelectorV1, RetrievalSeedPlanV1,
};
use std::collections::HashSet;

const GENERIC_RETRIEVAL_PURPOSE: &str =
    "unchanged question for generic lexical and semantic retrieval";
const TYPED_FREE_QUERY_PURPOSE: &str = "typed free-query retrieval seed";

/// Keep the whole query, its first slash-text atom, then literal paragraphs and
/// sentences within the existing bound. All views have generic retrieval
/// authority; slash text does not identify a filesystem path or a graph node.
pub fn generic_retrieval_queries(question: &str) -> Vec<&str> {
    const MAX_SUPPLEMENTAL_QUERIES: usize = 7;
    let whole = question.trim();
    if whole.is_empty() {
        return Vec::new();
    }
    let mut queries = vec![whole];
    if let Some(literal) = first_slash_text_literal(question)
        && !whole.eq_ignore_ascii_case(literal)
    {
        queries.push(literal);
    }
    let paragraph_cap = MAX_SUPPLEMENTAL_QUERIES - (queries.len() - 1);
    let paragraph_count = literal_paragraphs(question).count();
    let count = paragraph_count.min(paragraph_cap);
    let positions = (0..count)
        .map(|position| {
            if paragraph_count <= paragraph_cap {
                position
            } else {
                position * (paragraph_count - 1) / (paragraph_cap - 1)
            }
        })
        .collect::<Vec<_>>();
    let mut wanted = positions.into_iter().peekable();
    for (index, paragraph) in literal_paragraphs(question).enumerate() {
        if wanted.peek() != Some(&index) {
            continue;
        }
        wanted.next();
        if !queries
            .iter()
            .any(|query| query.eq_ignore_ascii_case(paragraph))
        {
            queries.push(paragraph);
        }
        if wanted.peek().is_none() {
            break;
        }
    }
    let remaining = (MAX_SUPPLEMENTAL_QUERIES + 1).saturating_sub(queries.len());
    if remaining == 0 {
        return queries;
    }
    let mut seen = queries
        .iter()
        .map(|query| query.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let sentences = literal_sentences(question)
        .into_iter()
        .filter(|sentence| seen.insert(sentence.to_ascii_lowercase()))
        .collect::<Vec<_>>();
    let count = remaining.min(sentences.len());
    for position in 0..count {
        let index = if count == 1 {
            sentences.len() - 1
        } else {
            position * (sentences.len() - 1) / (count - 1)
        };
        queries.push(sentences[index]);
    }
    queries
}

// Syntax alone chooses a fallible text view, including negated/incidental text.
// Quoted spans and fences must close before their contents can be considered.
fn first_slash_text_literal(question: &str) -> Option<&str> {
    let mut chars = question.char_indices().peekable();
    let mut fence = None;
    let mut wrapper = None;
    let mut escaped = false;
    let mut line_prefix = true;
    while let Some((index, character)) = chars.next() {
        let at_line_prefix = line_prefix;
        // Track prefix eligibility once per character, including inside fences.
        line_prefix = character == '\n'
            || (line_prefix
                && (character.is_whitespace()
                    || character.is_ascii_digit()
                    || matches!(character, '>' | '-' | '+' | '*' | '.' | ')')));
        let mut run = 1;
        let mut end = index + character.len_utf8();
        if matches!(character, '`' | '~') {
            while chars.peek().is_some_and(|(_, next)| *next == character) {
                let (index, next) = chars.next()?;
                run += 1;
                end = index + next.len_utf8();
            }
        }
        if let Some((delimiter, length)) = fence {
            if character == delimiter
                && run >= length
                && at_line_prefix
                && question[end..].split('\n').next()?.trim().is_empty()
            {
                fence = None;
            }
            continue;
        }
        if let Some((delimiter, length, start)) = wrapper {
            if escaped {
                escaped = false;
            } else if character == '\\' && delimiter != '`' {
                escaped = true;
            } else if character == delimiter && run == length {
                let atom = &question[start..index];
                if slash_text_atom(atom) {
                    return Some(atom);
                }
                wrapper = None;
            }
            continue;
        }
        if matches!(character, '`' | '~')
            && run >= 3
            && at_line_prefix
            && (character != '`' || !has_single_token_backtick_close(&question[end..], run))
        {
            fence = Some((character, run));
            continue;
        }
        let previous = question[..index].chars().next_back();
        if character == '`'
            || character == '"'
            || (character == '\'' && !previous.is_some_and(char::is_alphanumeric))
        {
            wrapper = Some((character, run, end));
            continue;
        }
        if character != '/'
            || previous.is_some_and(|previous| {
                !previous.is_whitespace() && !matches!(previous, '(' | '[' | '{' | '<')
            })
        {
            continue;
        }
        while let Some((index, next)) = chars.peek().copied() {
            if next.is_whitespace() {
                break;
            }
            chars.next();
            end = index + next.len_utf8();
        }
        let mut atom = question[index..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let mut excess_closers = [0_isize; 4];
        for character in atom.chars() {
            match character {
                '(' => excess_closers[0] -= 1,
                ')' => excess_closers[0] += 1,
                '[' => excess_closers[1] -= 1,
                ']' => excess_closers[1] += 1,
                '{' => excess_closers[2] -= 1,
                '}' => excess_closers[2] += 1,
                '<' => excess_closers[3] -= 1,
                '>' => excess_closers[3] += 1,
                _ => {}
            }
        }
        // Strip only unmatched closing prose brackets, retaining pairs inside
        // the atom, such as a placeholder in /items/{key}.
        while let Some(close) = atom.chars().next_back() {
            let pair = match close {
                ')' => 0,
                ']' => 1,
                '}' => 2,
                '>' => 3,
                _ => break,
            };
            if excess_closers[pair] <= 0 {
                break;
            }
            excess_closers[pair] -= 1;
            atom = atom[..atom.len() - close.len_utf8()]
                .trim_end_matches(['.', ',', ';', ':', '!', '?']);
        }
        if !atom.contains(['\'', '"', '`']) && slash_text_atom(atom) {
            return Some(atom);
        }
    }
    None
}

fn has_single_token_backtick_close(tail: &str, length: usize) -> bool {
    // Only a line-prefix run needs this distinction. Looking ahead over its
    // first token once keeps the scan linear even for arbitrarily long input.
    let mut chars = tail.chars().peekable();
    let mut nonempty = false;
    while let Some(character) = chars.next() {
        if character.is_whitespace() {
            return false;
        }
        if character == '`' {
            let mut run = 1;
            while chars.peek() == Some(&'`') {
                chars.next();
                run += 1;
            }
            if run == length {
                return nonempty;
            }
        }
        nonempty = true;
    }
    false
}

fn slash_text_atom(atom: &str) -> bool {
    atom.starts_with('/')
        && !atom.starts_with("//")
        && atom.chars().count() <= PACKET_PROBE_MAX_TEXT_LENGTH
        && !atom.chars().any(char::is_whitespace)
        && atom.chars().any(char::is_alphanumeric)
}

// Keep punctuation inside quoted strings and matching code delimiters literal.
// Unclosed delimiters conservatively retain the rest as one slice.
fn literal_sentences(question: &str) -> Vec<&str> {
    let mut slices = Vec::new();
    let mut chars = question.char_indices().peekable();
    let mut start = 0;
    let mut line_start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut code = None;
    while let Some((index, character)) = chars.next() {
        let blank_line = character == '\n' && question[line_start..index].trim().is_empty();
        if character == '\n' {
            line_start = index + 1;
        }
        if let Some((delimiter, length)) = code {
            if character == delimiter {
                let mut run = 1;
                while chars.peek().is_some_and(|(_, next)| *next == delimiter) {
                    chars.next();
                    run += 1;
                }
                if run == length || (length >= 3 && run > length) {
                    code = None;
                }
            }
            continue;
        }
        let previous = question[..index].chars().next_back();
        let next = chars.peek().map(|(_, character)| *character);
        let apostrophe = character == '\''
            && previous.is_some_and(char::is_alphanumeric)
            && next.is_some_and(char::is_alphanumeric);
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter && !apostrophe {
                quote = None;
            }
            continue;
        }
        if character == '"' || (character == '\'' && !previous.is_some_and(char::is_alphanumeric)) {
            quote = Some(character);
            continue;
        }
        if character == '`' || character == '~' {
            let mut run = 1;
            while chars.peek().is_some_and(|(_, next)| *next == character) {
                chars.next();
                run += 1;
            }
            if character == '`' || run >= 3 {
                code = Some((character, run));
            }
            continue;
        }
        let ordinal_marker = character == '.'
            && previous.is_some_and(char::is_numeric)
            && question[start.max(line_start)..index]
                .trim()
                .chars()
                .all(char::is_numeric);
        let terminator = matches!(character, '.' | '?' | '!')
            && next.is_none_or(char::is_whitespace)
            && !ordinal_marker;
        if blank_line || terminator {
            let end = index + character.len_utf8();
            let slice = question[start..end].trim();
            if !slice.is_empty() {
                slices.push(slice);
            }
            start = end;
        }
    }
    let tail = question[start..].trim();
    if !tail.is_empty() {
        slices.push(tail);
    }
    slices
}

fn literal_paragraphs(question: &str) -> impl Iterator<Item = &str> {
    let mut lines = question.split_inclusive('\n');
    let mut offset = 0;
    std::iter::from_fn(move || {
        let mut start = None;
        for line in lines.by_ref() {
            let end = offset;
            offset += line.len();
            if line.trim().is_empty() {
                if let Some(begin) = start.take() {
                    return Some(question[begin..end].trim());
                }
            } else {
                start.get_or_insert(end);
            }
        }
        start.map(|begin| question[begin..offset].trim())
    })
}

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
    let mut explicit_queries = Vec::new();
    for query in &seed_plan.free_queries {
        push_packet_query(&mut explicit_queries, query, TYPED_FREE_QUERY_PURPOSE);
    }
    let cap = packet_plan_query_cap(budget);
    // Preserve the whole question and reserve space for caller-supplied queries
    // before spending the remaining slots on supplemental literal views.
    let generic_cap = cap.saturating_sub(explicit_queries.len()).max(1);
    let mut queries = Vec::new();
    for query in generic_retrieval_queries(&seed_plan.generic_query)
        .into_iter()
        .take(generic_cap)
    {
        push_packet_query(&mut queries, query, GENERIC_RETRIEVAL_PURPOSE);
    }
    queries.extend(explicit_queries);
    queries.truncate(cap);
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
        || queries.iter().any(|existing| {
            existing.query.eq_ignore_ascii_case(query) && existing.purpose == purpose
        })
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
    fn slash_text_views_preserve_public_planner_boundaries() {
        let at_limit = format!("/{}", "界".repeat(239));
        let oversized = format!("/{}", "界".repeat(240));
        let many_closers = format!("Inspect /archive{} then /later.", ")]>".repeat(4096));
        let many_fence_markers = format!(
            "Before.\n~~~text\n{}{}\n~~~\nInspect /later.",
            " ".repeat(4096),
            "~~~ not-a-closer ".repeat(4096),
        );
        let cases = vec![
            ("first", "Inspect /archive then /later.".to_string(), Some("/archive")),
            ("punctuation", "Inspect (/archive), then /later.".into(), Some("/archive")),
            ("internal", "Inspect /archive/{id}?version=2 then /later.".into(), Some("/archive/{id}?version=2")),
            ("double quote", "Inspect \"/archive\" then /later.".into(), Some("/archive")),
            ("single quote", "Inspect '/archive' then /later.".into(), Some("/archive")),
            ("backtick", "Inspect `/archive` then /later.".into(), Some("/archive")),
            ("double backtick", "Inspect ``/archive`` then /later.".into(), Some("/archive")),
            ("inline triple backtick", "Inspect ```/archive``` then /later.".into(), Some("/archive")),
            ("line-leading triple wrapper", "```/archive``` before /later.".into(), Some("/archive")),
            ("line-leading four wrapper", "````/archive```` before /later.".into(), Some("/archive")),
            ("prefixed triple wrapper", "> - ```/archive``` before /later.".into(), Some("/archive")),
            ("prefixed four wrapper", "> 1. ````/archive```` before /later.".into(), Some("/archive")),
            ("escaped quote", r#"Inspect "/archive/\"part" then /later."#.into(), Some(r#"/archive/\"part"#)),
            ("unicode", "背景 `/café/資料` then /later.".into(), Some("/café/資料")),
            ("unicode whitespace", "Inspect\u{2003}/Archive\t/later.".into(), Some("/Archive")),
            ("negated", "Ignore /incidental but inspect /later.".into(), Some("/incidental")),
            ("repeat", "Inspect /Archive /archive /later.".into(), Some("/Archive")),
            ("url", "See https://example.invalid/archive and //host/path, then /later.".into(), Some("/later")),
            ("quoted url", "See `https://example.invalid/archive` and '//host/path', then /later.".into(), Some("/later")),
            ("non-slash", "Inspect `archive` and `node:123` and src/archive.rs.".into(), None),
            ("whole atom deduplicated", "/archive".into(), None),
            ("empty slash", "Inspect / and //host/path only.".into(), None),
            ("unclosed quote", "Inspect \"/archive then /later.".into(), None),
            ("unclosed backtick", "Inspect `/archive then /later.".into(), None),
            ("incomplete bare wrapper", "Inspect /archive\" then /later.".into(), Some("/later")),
            ("multiword quote", "Inspect \"/archive /hidden\" then /later.".into(), Some("/later")),
            ("fence", "Before.\n```text\n/archive\n```\nInspect /later.".into(), Some("/later")),
            ("fence type and length", "Before.\n````text\n/archive\n~~~\n/hidden\n```\n/still-hidden\n````\nInspect /later.".into(), Some("/later")),
            ("embedded fence marker", "Before.\n```text\nexample ``` /hidden\n``` trailing\n/still-hidden\n```\nInspect /later.".into(), Some("/later")),
            ("nested fence", "Before.\n> - ```text\n> /hidden\n> ```\nInspect /later.".into(), Some("/later")),
            ("CRLF nested fence", "Before.\r\n> 1. ````text\r\n> /hidden\r\n> ```\r\n> /still-hidden\r\n> ````\r\nInspect /later.".into(), Some("/later")),
            ("tilde fence", "Before.\n~~~text\n/archive\n~~~\nInspect /later.".into(), Some("/later")),
            ("unclosed fence", "Before.\n```text\n/archive\n/later".into(), None),
            ("limit", format!("Inspect {at_limit} then /later."), Some(at_limit.as_str())),
            ("oversize", format!("Inspect {oversized} then /later."), Some("/later")),
            ("quoted oversize", format!("Inspect `{oversized}` then /later."), Some("/later")),
            ("many unmatched closers", many_closers, Some("/archive")),
            ("many markers after long prefix", many_fence_markers, Some("/later")),
        ];
        for (name, question, expected) in cases {
            let seed = build_retrieval_seed_plan(&question, &[]);
            assert_eq!(seed.generic_query, question, "{name}: original bytes");
            assert!(
                seed.exact_selectors.is_empty(),
                "{name}: no identity authority"
            );
            assert!(seed.free_queries.is_empty(), "{name}: no caller authority");
            let plan = build_packet_plan_from_seed_plan(&seed, PacketBudgetModeDto::Standard);
            assert_eq!(
                plan.queries[0].query,
                question.trim(),
                "{name}: whole first"
            );
            let slash_views = plan
                .queries
                .iter()
                .skip(1)
                .filter(|query| {
                    query.query.starts_with('/') && !query.query.chars().any(char::is_whitespace)
                })
                .map(|query| query.query.as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                slash_views,
                expected.into_iter().collect::<Vec<_>>(),
                "{name}"
            );
            if let Some(expected) = expected {
                assert_eq!(
                    plan.queries[1].query, expected,
                    "{name}: literal before supplements"
                );
            }
            assert!(plan.queries.len() <= 8, "{name}: generic cap");
            assert!(
                plan.queries
                    .iter()
                    .all(|query| question.contains(&query.query))
            );
            assert!(
                plan.queries
                    .iter()
                    .all(|query| !packet_plan_query_is_typed_free_query(query))
            );
        }

        let question = "  Inspect /Archive and /later.\n\nSecond.\n\nThird.\n\nFourth.\n\nFifth.\n\nSixth.\n\nSeventh.\n\nEighth.  ";
        let mut probes = (0..13)
            .map(|index| PacketProbeDto::FreeQuery {
                query: format!("explicit {index}"),
            })
            .collect::<Vec<_>>();
        probes.push(PacketProbeDto::FreeQuery {
            query: "/Archive".into(),
        });
        probes.push(PacketProbeDto::ExactPath {
            path: "src/real.rs".into(),
        });
        probes.push(PacketProbeDto::SymbolId {
            id: "node:42".into(),
        });
        let seed = build_retrieval_seed_plan(question, &probes);
        assert_eq!(seed.generic_query, question);
        assert_eq!(
            seed.exact_selectors,
            [
                PacketSeedSelectorV1::ExactPath {
                    path: "src/real.rs".into()
                },
                PacketSeedSelectorV1::CanonicalId {
                    id: "node:42".into()
                },
            ]
        );
        let generic = build_packet_plan(question, PacketBudgetModeDto::Standard);
        assert_eq!(generic.queries.len(), 8);
        assert_eq!(generic.queries[1].query, "/Archive");
        let tiny = build_packet_plan_from_seed_plan(&seed, PacketBudgetModeDto::Tiny);
        assert_eq!(tiny.queries.len(), 20);
        assert_eq!(tiny.queries[0].query, question.trim());
        assert_eq!(tiny.queries[1].query, "/Archive");
        assert!(!packet_plan_query_is_typed_free_query(&tiny.queries[1]));
        assert_eq!(
            packet_explicit_request_probe_queries(&tiny),
            seed.free_queries
        );
        assert_eq!(
            tiny.queries
                .iter()
                .filter(|query| query.query == "/Archive")
                .count(),
            2
        );
    }

    #[test]
    fn sentence_views_preserve_whole_paragraph_order_and_literal_unicode() {
        let question = "背景 café first. 質問 second?\r\n\r\nвопрос third! End fourth.";
        let queries = generic_retrieval_queries(question);
        assert_eq!(queries[0], question);
        assert_eq!(
            &queries[1..3],
            &["背景 café first. 質問 second?", "вопрос third! End fourth."]
        );
        assert_eq!(
            &queries[3..],
            &[
                "背景 café first.",
                "質問 second?",
                "вопрос third!",
                "End fourth."
            ]
        );
        assert!(queries.iter().all(|query| question.contains(query)));
        let seed = build_retrieval_seed_plan(question, &[]);
        assert_eq!(seed.generic_query, question);
        assert!(seed.exact_selectors.is_empty());
        assert_eq!(
            build_packet_plan_from_seed_plan(&seed, PacketBudgetModeDto::Standard)
                .queries
                .into_iter()
                .map(|query| query.query)
                .collect::<Vec<_>>(),
            queries
        );
    }

    #[test]
    fn sentence_views_sample_positions_uniformly_including_the_last() {
        let question = (0..20)
            .map(|index| format!("item_{index:02} ends."))
            .collect::<Vec<_>>()
            .join(" ");
        let queries = generic_retrieval_queries(&question);
        assert_eq!(queries.len(), 8);
        assert_eq!(queries[0], question);
        assert_eq!(
            &queries[1..],
            &[
                "item_00 ends.",
                "item_03 ends.",
                "item_06 ends.",
                "item_09 ends.",
                "item_12 ends.",
                "item_15 ends.",
                "item_19 ends."
            ]
        );
        assert!(queries.iter().all(|query| question.contains(query)));
        let many_paragraphs = (0..12)
            .map(|index| format!("paragraph_{index}. Next."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let views = generic_retrieval_queries(&many_paragraphs);
        assert_eq!(views.len(), 8);
        assert_eq!(views[1], "paragraph_0. Next.");
        assert_eq!(views[7], "paragraph_11. Next.");
    }

    #[test]
    fn sentence_views_deduplicate_repetition_before_sampling_and_keep_plain_tail() {
        let question = "Repeat. Repeat. REPEAT. Other? Tail without punctuation";
        assert_eq!(
            generic_retrieval_queries(question),
            [question, "Repeat.", "Other?", "Tail without punctuation"]
        );
        let no_punctuation = "ordinary café wording\r\ncontinued wording";
        assert_eq!(generic_retrieval_queries(no_punctuation), [no_punctuation]);
        let question = "One. Two.\n\nThree.\n\nFour.\n\nFive.\n\nSix.\n\nSeven.";
        let views = generic_retrieval_queries(question);
        assert_eq!(views.len(), 8);
        assert_eq!(
            views.last(),
            Some(&"Two."),
            "the sole remaining slot keeps the final new sentence"
        );
    }

    #[test]
    fn sentence_views_preserve_quoted_strings_code_paths_decimals_and_apostrophes() {
        let question = r#"We're reading src/module.rs with threshold 3.14 safely. Keep "quoted? words." and 'quoted! words' literal. Preserve `value.one? next!` unchanged. End."#;
        let views = generic_retrieval_queries(question);
        assert_eq!(
            &views[1..],
            &[
                "We're reading src/module.rs with threshold 3.14 safely.",
                r#"Keep "quoted? words." and 'quoted! words' literal."#,
                "Preserve `value.one? next!` unchanged.",
                "End.",
            ]
        );
        let escaped = r#"Keep "inner \"quote?\" remains. inside" literal. Then finish!"#;
        assert_eq!(
            literal_sentences(escaped),
            [
                r#"Keep "inner \"quote?\" remains. inside" literal."#,
                "Then finish!"
            ]
        );
        assert_eq!(
            literal_sentences("Unclosed `one? two. three!"),
            ["Unclosed `one? two. three!"]
        );
        assert_eq!(
            literal_sentences("Unclosed \"one? two. three!"),
            ["Unclosed \"one? two. three!"]
        );
        assert_eq!(
            literal_sentences("1. first? 2. next!"),
            ["1. first?", "2. next!"]
        );
        for first in ["Use port 8080.", "Use threshold 3.14.", "Use version 2."] {
            let question = format!("{first} Check capacity.");
            assert_eq!(
                generic_retrieval_queries(&question),
                [question.as_str(), first, "Check capacity."]
            );
        }
    }

    #[test]
    fn sentence_views_keep_possessives_literal_and_close_quoted_strings() {
        let plural = "Operators' settings change. Validate counts. Show configuration.";
        assert_eq!(
            generic_retrieval_queries(plural),
            [
                plural,
                "Operators' settings change.",
                "Validate counts.",
                "Show configuration."
            ]
        );
        let singular = "Operator's settings change. It works.";
        assert_eq!(
            generic_retrieval_queries(singular),
            [singular, "Operator's settings change.", "It works."]
        );
        let quoted = "Keep 'inner! word' literal. Finish.";
        assert_eq!(
            generic_retrieval_queries(quoted),
            [quoted, "Keep 'inner! word' literal.", "Finish."]
        );
    }

    #[test]
    fn sentence_views_do_not_split_matching_fenced_code_or_create_exact_selectors() {
        let question = "Before.\n````text\nvalue.one? next!\n\n```\nvalue.two. next?\n````\nAfter.\n~~~text\nvalue.three! next?\n~~~\nEnd.";
        let slices = literal_sentences(question);
        assert_eq!(
            slices,
            [
                "Before.",
                "````text\nvalue.one? next!\n\n```\nvalue.two. next?\n````\nAfter.",
                "~~~text\nvalue.three! next?\n~~~\nEnd.",
            ]
        );
        assert!(slices.iter().all(|slice| question.contains(slice)));
        assert!(
            build_retrieval_seed_plan(question, &[])
                .exact_selectors
                .is_empty()
        );
    }

    #[test]
    fn generic_paragraph_views_keep_original_and_literal_unicode_text() {
        let question = "背景 café\n\n1. 質問 alpha?\n\n2. вопрос beta?\n";
        let queries = generic_retrieval_queries(question);
        assert_eq!(
            queries,
            [
                question.trim(),
                "背景 café",
                "1. 質問 alpha?",
                "2. вопрос beta?"
            ]
        );
        assert!(queries.iter().all(|query| question.contains(query)));
        let plan = build_packet_plan(question, PacketBudgetModeDto::Standard);
        assert_eq!(
            plan.queries
                .iter()
                .map(|query| query.query.as_str())
                .collect::<Vec<_>>(),
            queries
        );
        assert!(
            plan.queries
                .iter()
                .all(|query| !packet_plan_query_is_typed_free_query(query))
        );
        assert!(
            build_retrieval_seed_plan(question, &[])
                .exact_selectors
                .is_empty()
        );
    }

    #[test]
    fn generic_paragraph_views_handle_crlf_blank_space_and_duplicates() {
        assert!(generic_retrieval_queries(" \r\n\r\n").is_empty());
        assert_eq!(
            generic_retrieval_queries("alpha\r\nbeta\r\n"),
            ["alpha\r\nbeta"]
        );
        let question = "alpha\r\n \r\nALPHA\r\n\r\nbeta";
        assert_eq!(
            generic_retrieval_queries(question),
            [question, "alpha", "beta"]
        );
    }

    #[test]
    fn generic_paragraph_views_are_bounded_and_include_the_final_position() {
        let question = (0..30)
            .map(|index| format!("paragraph_{index:02}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let queries = generic_retrieval_queries(&question);
        assert_eq!(queries.len(), 8);
        assert_eq!(queries[0], question);
        assert_eq!(queries[1], "paragraph_00");
        assert_eq!(queries[7], "paragraph_29");
        assert!(queries.iter().all(|query| question.contains(query)));
    }

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
    fn supplemental_views_preserve_all_legal_typed_queries_in_tiny_plans() {
        let question = "First. Second. Third. Fourth. Fifth. Sixth. Seventh.";
        let free_queries = (0..16)
            .map(|index| format!("explicit input {index}"))
            .collect::<Vec<_>>();
        let plan = build_packet_plan_with_extra(question, PacketBudgetModeDto::Tiny, &free_queries);
        assert_eq!(plan.queries[0].query, question);
        assert_eq!(packet_explicit_request_probe_queries(&plan), free_queries);
        assert!(plan.queries.len() <= 20);
        assert!(plan.queries.iter().any(|query| query.query == "First."));
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
    fn same_generic_and_typed_free_text_survives_both_plan_dedup_stages() {
        let question = "text empty `absent`";
        let seed = build_retrieval_seed_plan(
            question,
            &[PacketProbeDto::FreeQuery {
                query: question.into(),
            }],
        );
        let mut plan = build_packet_plan_from_seed_plan(&seed, PacketBudgetModeDto::Standard);
        assert_eq!(plan.queries.len(), 2);
        assert_eq!(plan.queries[0].query, question);
        assert_eq!(plan.queries[1].query, question);
        assert_ne!(plan.queries[0].purpose, plan.queries[1].purpose);
        dedupe_packet_plan_queries(&mut plan);
        assert_eq!(plan.queries.len(), 2);
        assert!(seed.exact_selectors.is_empty());
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
