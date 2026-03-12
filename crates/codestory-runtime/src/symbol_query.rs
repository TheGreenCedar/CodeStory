use codestory_contracts::api::{NodeKind, SearchHit, SearchHitOrigin};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SymbolNameMatchRank {
    pub exact_display: u8,
    pub exact_terminal: u8,
    pub exact_leading: u8,
}

pub fn normalize_symbol_query(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

pub fn terminal_symbol_segment(value: &str) -> String {
    value
        .rsplit([':', '.', '/', '\\'])
        .next()
        .map(normalize_symbol_query)
        .unwrap_or_default()
}

pub fn leading_symbol_segment(value: &str) -> String {
    value
        .split("::")
        .next()
        .map(normalize_symbol_query)
        .unwrap_or_default()
}

pub fn symbol_name_match_rank(query: &str, display_name: &str) -> SymbolNameMatchRank {
    let query = normalize_symbol_query(query);
    let display = normalize_symbol_query(display_name);
    let terminal = terminal_symbol_segment(display_name);
    let leading = leading_symbol_segment(display_name);

    SymbolNameMatchRank {
        exact_display: u8::from(display == query),
        exact_terminal: u8::from(terminal == query),
        exact_leading: u8::from(leading == query),
    }
}

pub fn compare_ranked_hits<T: Ord>(
    left: &SearchHit,
    right: &SearchHit,
    left_rank: T,
    right_rank: T,
) -> Ordering {
    right_rank
        .cmp(&left_rank)
        .then_with(|| right.score.total_cmp(&left.score))
        .then_with(|| left.display_name.len().cmp(&right.display_name.len()))
        .then_with(|| left.display_name.cmp(&right.display_name))
}

fn search_kind_bucket(kind: NodeKind, origin: SearchHitOrigin) -> u8 {
    if origin == SearchHitOrigin::TextMatch {
        return 0;
    }

    match kind {
        NodeKind::MODULE
        | NodeKind::NAMESPACE
        | NodeKind::PACKAGE
        | NodeKind::STRUCT
        | NodeKind::CLASS
        | NodeKind::INTERFACE
        | NodeKind::ENUM
        | NodeKind::UNION
        | NodeKind::TYPEDEF => 3,
        NodeKind::FUNCTION
        | NodeKind::METHOD
        | NodeKind::MACRO
        | NodeKind::FIELD
        | NodeKind::VARIABLE
        | NodeKind::GLOBAL_VARIABLE
        | NodeKind::CONSTANT
        | NodeKind::ENUM_CONSTANT => 2,
        NodeKind::UNKNOWN => 0,
        _ => 1,
    }
}

fn search_kind_tiebreak(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::FUNCTION => 4,
        NodeKind::METHOD => 3,
        NodeKind::MACRO => 2,
        NodeKind::FIELD
        | NodeKind::VARIABLE
        | NodeKind::GLOBAL_VARIABLE
        | NodeKind::CONSTANT
        | NodeKind::ENUM_CONSTANT => 1,
        _ => 0,
    }
}

fn search_match_rank(query: &str, hit: &SearchHit) -> (u8, u8, u8, u8, u8, u8) {
    let rank = symbol_name_match_rank(query, &hit.display_name);

    (
        rank.exact_display,
        rank.exact_terminal,
        search_kind_bucket(hit.kind, hit.origin),
        search_kind_tiebreak(hit.kind),
        rank.exact_leading,
        u8::from(hit.origin == SearchHitOrigin::IndexedSymbol),
    )
}

pub(crate) fn compare_search_hits(query: &str, left: &SearchHit, right: &SearchHit) -> Ordering {
    compare_ranked_hits(
        left,
        right,
        search_match_rank(query, left),
        search_match_rank(query, right),
    )
}
