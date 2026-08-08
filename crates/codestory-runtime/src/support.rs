#[cfg(test)]
use crate::search_runtime::HybridSearchConfig;
#[cfg(test)]
use codestory_contracts::api::{AgentHybridWeightsDto, SearchHybridLimitsDto};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

pub(crate) use codestory_contracts::config_registry::HYBRID_RETRIEVAL_ENABLED_ENV;
/// Default semantic file-text cap. Product projections use the active
/// source-index policy retained by the runtime.
pub(crate) const SEMANTIC_FILE_TEXT_MAX_BYTES: u64 =
    codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP;
pub(crate) const SEMANTIC_FILE_TEXT_CACHE_MAX_BYTES: usize = 64 * 1_024 * 1_024;

/// Whether hybrid lexical/semantic ranking is on for this process.
///
/// `CODESTORY_HYBRID_RETRIEVAL_ENABLED` is declared to
/// `codestory-retrieval/src/config.rs`, which is where the value is read and
/// interpreted; the runtime asks rather than parsing the variable a second
/// time. Query paths that already hold a `SidecarRuntimeConfig` should prefer
/// `runtime.retrieval.hybrid_enabled`, which additionally honours the
/// `.codestory.toml` override.
pub(crate) fn hybrid_retrieval_enabled() -> bool {
    codestory_retrieval::hybrid_retrieval_enabled_from_process_env()
}

#[cfg(test)]
pub(crate) fn normalized_hybrid_weights(
    request_weights: Option<AgentHybridWeightsDto>,
    fallback: &HybridSearchConfig,
) -> (f32, f32, f32) {
    let lexical = request_weights
        .as_ref()
        .and_then(|weights| weights.lexical)
        .unwrap_or(fallback.lexical_weight)
        .clamp(0.0, 1.0);
    let semantic = request_weights
        .as_ref()
        .and_then(|weights| weights.semantic)
        .unwrap_or(fallback.semantic_weight)
        .clamp(0.0, 1.0);
    let graph = request_weights
        .and_then(|weights| weights.graph)
        .unwrap_or(fallback.graph_weight)
        .clamp(0.0, 1.0);

    let sum = lexical + semantic + graph;
    if sum <= f32::EPSILON {
        return (
            fallback.lexical_weight,
            fallback.semantic_weight,
            fallback.graph_weight,
        );
    }

    (lexical / sum, semantic / sum, graph / sum)
}

#[cfg(test)]
pub(crate) fn apply_hybrid_limits(
    request_limits: Option<SearchHybridLimitsDto>,
    config: &mut HybridSearchConfig,
) {
    const MAX_CANDIDATE_LIMIT: u32 = 1_000;
    let Some(limits) = request_limits else {
        return;
    };
    if let Some(lexical) = limits.lexical {
        config.lexical_limit = lexical.min(MAX_CANDIDATE_LIMIT) as usize;
    }
    if let Some(semantic) = limits.semantic {
        config.semantic_limit = semantic.min(MAX_CANDIDATE_LIMIT) as usize;
    }
}

pub(crate) fn node_display_name(node: &codestory_contracts::graph::Node) -> String {
    node.qualified_name
        .clone()
        .unwrap_or_else(|| node.serialized_name.clone())
}

pub(crate) fn clamp_i64_to_u32(v: i64) -> u32 {
    if v <= 0 {
        0
    } else if v > u32::MAX as i64 {
        u32::MAX
    } else {
        v as u32
    }
}

pub(crate) fn clamp_u64_to_u32(v: u64) -> u32 {
    v.min(u32::MAX as u64) as u32
}

pub(crate) fn clamp_u128_to_u32(v: u128) -> u32 {
    v.min(u32::MAX as u128) as u32
}

pub(crate) fn clamp_usize_to_u32(v: usize) -> u32 {
    v.min(u32::MAX as usize) as u32
}

/// Publish the source-freshness counters for the public operation currently
/// running on this thread.
///
/// Returns `None` when no public operation armed a scope, so a response built
/// outside the runtime's operation boundary reports nothing rather than a
/// misleading zero.
pub(crate) fn source_freshness_telemetry_for_operation()
-> Option<codestory_contracts::api::SourceFreshnessTelemetryDto> {
    codestory_workspace::source_freshness_counts().map(|counts| {
        codestory_contracts::api::SourceFreshnessTelemetryDto {
            content_hash_reads: clamp_u64_to_u32(counts.content_hash_reads),
            verdict_reuses: clamp_u64_to_u32(counts.verdict_reuses),
            readiness_fingerprint_passes: clamp_u64_to_u32(counts.readiness_fingerprint_passes),
        }
    })
}

const NL_STOPWORDS: &[&str] = &[
    "a",
    "an",
    "and",
    "are",
    "as",
    "at",
    "be",
    "by",
    "can",
    "do",
    "does",
    "for",
    "from",
    "how",
    "in",
    "is",
    "it",
    "of",
    "on",
    "or",
    "repo",
    "repository",
    "show",
    "tell",
    "that",
    "the",
    "this",
    "to",
    "what",
    "where",
    "which",
    "why",
    "with",
    "work",
    "works",
];

pub(crate) fn extract_symbol_search_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut seen = HashSet::new();

    for ch in query.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch.to_ascii_lowercase());
            continue;
        }

        if current.len() >= 3
            && !NL_STOPWORDS.contains(&current.as_str())
            && seen.insert(current.clone())
        {
            terms.push(current.clone());
        }
        current.clear();
    }

    if current.len() >= 3
        && !NL_STOPWORDS.contains(&current.as_str())
        && seen.insert(current.clone())
    {
        terms.push(current);
    }

    terms.truncate(8);
    terms
}

pub(crate) fn should_expand_symbol_query(query: &str, direct_hit_count: usize) -> bool {
    if direct_hit_count >= 3 {
        return false;
    }

    let word_count = query.split_whitespace().count();
    let has_text_punctuation = query
        .chars()
        .any(|ch| matches!(ch, '.' | ',' | ':' | ';' | '!' | '?' | '"' | '\''));
    if word_count > 1 && has_text_punctuation {
        return true;
    }

    word_count > 2 || query.len() > 28
}

pub(crate) fn looks_like_repo_text_query(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return false;
    }

    let word_count = trimmed.split_whitespace().count();
    let has_text_punctuation = query
        .chars()
        .any(|ch| matches!(ch, '.' | ',' | ':' | ';' | '!' | '?' | '"' | '\''));
    if (word_count > 1 && has_text_punctuation) || trimmed.len() > 28 || word_count >= 4 {
        return true;
    }

    if word_count < 2 {
        return false;
    }

    trimmed.split_whitespace().any(|term| {
        matches!(
            term.to_ascii_lowercase().as_str(),
            "how"
                | "what"
                | "why"
                | "where"
                | "when"
                | "which"
                | "who"
                | "does"
                | "do"
                | "is"
                | "are"
                | "should"
                | "can"
        )
    })
}

pub(crate) fn query_has_symbol_or_literal_signal(query: &str) -> bool {
    !high_signal_query_literals(query).is_empty()
}

pub(crate) fn file_text_match_line(contents: &str, query: &str, terms: &[String]) -> Option<u32> {
    let normalized_query = query.trim().to_ascii_lowercase();
    let high_signal_literals = high_signal_query_literals(query);
    for (index, line) in contents.lines().enumerate() {
        let normalized_line = line.to_ascii_lowercase();
        if !normalized_query.is_empty() && normalized_line.contains(&normalized_query) {
            return Some((index + 1).min(u32::MAX as usize) as u32);
        }
        if high_signal_literals
            .iter()
            .any(|literal| normalized_line.contains(literal))
        {
            return Some((index + 1).min(u32::MAX as usize) as u32);
        }
        if !terms.is_empty() && terms.iter().all(|term| normalized_line.contains(term)) {
            return Some((index + 1).min(u32::MAX as usize) as u32);
        }
    }
    None
}

fn high_signal_query_literals(query: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut seen = HashSet::new();

    for delimiter in ['`', '"', '\''] {
        for literal in delimited_query_segments(query, delimiter) {
            push_high_signal_literal(&mut literals, &mut seen, &literal, true);
        }
    }

    for token in query.split(|ch: char| {
        !(ch.is_ascii_alphanumeric()
            || matches!(ch, '_' | ':' | '.' | '-' | '/' | '\\' | '`' | '"' | '\''))
    }) {
        push_high_signal_literal(&mut literals, &mut seen, token, false);
    }

    literals
}

fn delimited_query_segments(query: &str, delimiter: char) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut inside = false;

    for ch in query.chars() {
        if ch == delimiter {
            if inside && !current.trim().is_empty() {
                segments.push(current.clone());
            }
            current.clear();
            inside = !inside;
            continue;
        }
        if inside {
            current.push(ch);
        }
    }

    segments
}

fn push_high_signal_literal(
    literals: &mut Vec<String>,
    seen: &mut HashSet<String>,
    raw: &str,
    from_delimited_segment: bool,
) {
    let trimmed = raw.trim_matches(|ch: char| {
        ch.is_ascii_whitespace()
            || matches!(
                ch,
                '`' | '"' | '\'' | ',' | ';' | '?' | '!' | '(' | ')' | '[' | ']' | '{' | '}'
            )
    });
    if trimmed.len() < 3 {
        return;
    }

    let normalized = trimmed.to_ascii_lowercase();
    if (!from_delimited_segment && !is_high_signal_literal_token(trimmed))
        || !seen.insert(normalized.clone())
    {
        return;
    }

    literals.push(normalized);
}

fn is_high_signal_literal_token(token: &str) -> bool {
    let has_alnum = token.chars().any(|ch| ch.is_ascii_alphanumeric());
    if !has_alnum {
        return false;
    }
    if token.contains("::") || token.contains('_') || token.contains('/') || token.contains('\\') {
        return true;
    }
    if token.contains('.') && token.chars().any(|ch| ch.is_ascii_alphabetic()) {
        return true;
    }
    if token.len() >= 4
        && token.chars().any(|ch| ch.is_ascii_lowercase())
        && token.chars().skip(1).any(|ch| ch.is_ascii_uppercase())
    {
        return true;
    }
    token.len() >= 4
        && token
            .chars()
            .filter(|ch| ch.is_ascii_alphabetic())
            .all(|ch| ch.is_ascii_uppercase())
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoundedTextRead {
    Contents { contents: String, bytes_read: u64 },
    LimitExceeded { bytes_read: u64 },
    Unreadable { bytes_read: u64 },
}

impl BoundedTextRead {
    pub(crate) fn bytes_read(&self) -> u64 {
        match self {
            Self::Contents { bytes_read, .. }
            | Self::LimitExceeded { bytes_read }
            | Self::Unreadable { bytes_read } => *bytes_read,
        }
    }
}

pub(crate) fn read_searchable_file_contents_limited(path: &str, max_bytes: u64) -> BoundedTextRead {
    #[cfg(windows)]
    let fallback_path = path.strip_prefix(r"\\?\");
    #[cfg(not(windows))]
    let fallback_path = None;

    read_searchable_file_contents_limited_with(
        path,
        fallback_path,
        max_bytes,
        |read_path, read_limit| read_file_text_limited(Path::new(read_path), read_limit),
    )
}

fn read_searchable_file_contents_limited_with(
    path: &str,
    fallback_path: Option<&str>,
    max_bytes: u64,
    mut read: impl FnMut(&str, u64) -> BoundedTextRead,
) -> BoundedTextRead {
    let primary = read(path, max_bytes);
    if matches!(primary, BoundedTextRead::Unreadable { bytes_read: 0 })
        && let Some(fallback_path) = fallback_path
    {
        return read(fallback_path, max_bytes);
    }
    primary
}

pub(crate) fn read_file_text_limited(path: &Path, max_bytes: u64) -> BoundedTextRead {
    let Ok(metadata) = std::fs::metadata(path) else {
        return BoundedTextRead::Unreadable { bytes_read: 0 };
    };
    if metadata.len() > max_bytes {
        return BoundedTextRead::LimitExceeded { bytes_read: 0 };
    }

    let Ok(file) = std::fs::File::open(path) else {
        return BoundedTextRead::Unreadable { bytes_read: 0 };
    };
    read_text_limited(file, max_bytes)
}

pub(crate) fn read_text_limited(mut reader: impl Read, max_bytes: u64) -> BoundedTextRead {
    let mut bytes = Vec::new();
    let mut bytes_read = 0_u64;
    let read_limit = max_bytes.saturating_add(1);
    let mut buffer = [0_u8; 8 * 1_024];
    while (bytes.len() as u64) < read_limit {
        let remaining = read_limit.saturating_sub(bytes.len() as u64);
        let chunk_len = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(buffer.len()));
        match reader.read(&mut buffer[..chunk_len]) {
            Ok(0) => break,
            Ok(count) => {
                bytes_read = bytes_read.saturating_add(count as u64);
                if bytes.try_reserve_exact(count).is_err() {
                    return BoundedTextRead::Unreadable { bytes_read };
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                return BoundedTextRead::Unreadable { bytes_read };
            }
        }
    }
    if bytes_read > max_bytes {
        return BoundedTextRead::LimitExceeded { bytes_read };
    }
    match String::from_utf8(bytes) {
        Ok(contents) => BoundedTextRead::Contents {
            contents,
            bytes_read,
        },
        Err(_) => BoundedTextRead::Unreadable { bytes_read },
    }
}

pub(crate) fn aggregate_symbol_matches(
    primary: Vec<(codestory_contracts::graph::NodeId, f32)>,
    expanded: Vec<(codestory_contracts::graph::NodeId, f32)>,
) -> Vec<(codestory_contracts::graph::NodeId, f32)> {
    let mut scores = HashMap::<codestory_contracts::graph::NodeId, f32>::new();

    for (id, score) in expanded {
        scores.insert(id, score);
    }

    for (id, score) in primary {
        let preferred = score + 100.0;
        scores
            .entry(id)
            .and_modify(|existing| *existing = existing.max(preferred))
            .or_insert(preferred);
    }

    let mut merged = scores.into_iter().collect::<Vec<_>>();
    merged.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap_or(Ordering::Equal));
    merged.truncate(20);
    merged
}

pub(crate) fn preferred_occurrence(
    occurrences: &[codestory_contracts::graph::Occurrence],
) -> Option<&codestory_contracts::graph::Occurrence> {
    fn occurrence_rank(kind: codestory_contracts::graph::OccurrenceKind) -> u8 {
        match kind {
            codestory_contracts::graph::OccurrenceKind::DEFINITION
            | codestory_contracts::graph::OccurrenceKind::MACRO_DEFINITION => 4,
            codestory_contracts::graph::OccurrenceKind::DECLARATION => 3,
            codestory_contracts::graph::OccurrenceKind::REFERENCE
            | codestory_contracts::graph::OccurrenceKind::MACRO_REFERENCE => 2,
            codestory_contracts::graph::OccurrenceKind::UNKNOWN => 1,
        }
    }

    occurrences.iter().max_by(|left, right| {
        occurrence_rank(left.kind)
            .cmp(&occurrence_rank(right.kind))
            .then_with(|| right.location.start_line.cmp(&left.location.start_line))
            .then_with(|| right.location.start_col.cmp(&left.location.start_col))
    })
}

#[derive(Debug, Clone)]
pub(crate) struct FocusedSourceContext {
    pub(crate) path: String,
    pub(crate) line: u32,
    pub(crate) snippet: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{
        NodeId as CoreNodeId, Occurrence, OccurrenceKind, SourceLocation,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct CountingReader {
        remaining: usize,
        byte: u8,
        bytes_read: Arc<AtomicUsize>,
    }

    impl Read for CountingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = buffer.len().min(self.remaining);
            buffer[..count].fill(self.byte);
            self.remaining -= count;
            self.bytes_read.fetch_add(count, Ordering::SeqCst);
            Ok(count)
        }
    }

    struct FailingReader {
        remaining_before_error: usize,
        bytes_read: Arc<AtomicUsize>,
    }

    struct InterruptingReader {
        contents: &'static [u8],
        position: usize,
        interrupt_at: usize,
        interrupted: bool,
        bytes_read: Arc<AtomicUsize>,
    }

    impl Read for InterruptingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted && self.position == self.interrupt_at {
                self.interrupted = true;
                return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
            }
            let phase_end = if self.interrupted {
                self.contents.len()
            } else {
                self.interrupt_at
            };
            let count = buffer.len().min(phase_end.saturating_sub(self.position));
            if count == 0 {
                return Ok(0);
            }
            buffer[..count].copy_from_slice(&self.contents[self.position..self.position + count]);
            self.position += count;
            self.bytes_read.fetch_add(count, Ordering::SeqCst);
            Ok(count)
        }
    }

    impl Read for FailingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.remaining_before_error == 0 {
                return Err(std::io::Error::other("hostile read failure"));
            }
            let count = buffer.len().min(self.remaining_before_error);
            buffer[..count].fill(b'x');
            self.remaining_before_error -= count;
            self.bytes_read.fetch_add(count, Ordering::SeqCst);
            Ok(count)
        }
    }

    fn occurrence(kind: OccurrenceKind, line: u32) -> Occurrence {
        Occurrence {
            element_id: 1,
            kind,
            location: SourceLocation {
                file_node_id: CoreNodeId(10),
                start_line: line,
                start_col: 1,
                end_line: line,
                end_col: 10,
            },
        }
    }

    #[test]
    fn preferred_occurrence_prefers_definition_over_declaration() {
        let occurrences = vec![
            occurrence(OccurrenceKind::DECLARATION, 1),
            occurrence(OccurrenceKind::DEFINITION, 20),
            occurrence(OccurrenceKind::REFERENCE, 5),
        ];

        let preferred = preferred_occurrence(&occurrences).expect("preferred occurrence");

        assert_eq!(preferred.kind, OccurrenceKind::DEFINITION);
        assert_eq!(preferred.location.start_line, 20);
    }

    #[test]
    fn limited_text_reader_stops_after_cap_overflow_sentinel() {
        let bytes_read = Arc::new(AtomicUsize::new(0));
        let reader = CountingReader {
            remaining: 4_096,
            byte: b'x',
            bytes_read: Arc::clone(&bytes_read),
        };

        let outcome = read_text_limited(reader, 32);

        assert_eq!(outcome, BoundedTextRead::LimitExceeded { bytes_read: 33 });
        assert_eq!(bytes_read.load(Ordering::SeqCst), 33);
    }

    #[test]
    fn limited_text_reader_charges_invalid_utf8_and_partial_errors() {
        let invalid_bytes_read = Arc::new(AtomicUsize::new(0));
        let invalid = read_text_limited(
            CountingReader {
                remaining: 31,
                byte: 0xff,
                bytes_read: Arc::clone(&invalid_bytes_read),
            },
            64,
        );
        assert_eq!(invalid, BoundedTextRead::Unreadable { bytes_read: 31 });
        assert_eq!(invalid_bytes_read.load(Ordering::SeqCst), 31);

        let failed_bytes_read = Arc::new(AtomicUsize::new(0));
        let failed = read_text_limited(
            FailingReader {
                remaining_before_error: 19,
                bytes_read: Arc::clone(&failed_bytes_read),
            },
            64,
        );
        assert_eq!(failed, BoundedTextRead::Unreadable { bytes_read: 19 });
        assert_eq!(failed_bytes_read.load(Ordering::SeqCst), 19);
    }

    #[test]
    fn limited_text_reader_retries_interrupted_reads_without_double_charging() {
        for interrupt_at in [0, 3] {
            let bytes_read = Arc::new(AtomicUsize::new(0));
            let outcome = read_text_limited(
                InterruptingReader {
                    contents: b"valid text",
                    position: 0,
                    interrupt_at,
                    interrupted: false,
                    bytes_read: Arc::clone(&bytes_read),
                },
                64,
            );

            assert_eq!(
                outcome,
                BoundedTextRead::Contents {
                    contents: "valid text".to_string(),
                    bytes_read: 10,
                }
            );
            assert_eq!(bytes_read.load(Ordering::SeqCst), 10);
        }
    }

    #[test]
    fn searchable_file_fallback_never_retries_after_consuming_bytes() {
        let mut calls = Vec::new();
        let outcome = read_searchable_file_contents_limited_with(
            r"\\?\C:\source.rs",
            Some(r"C:\source.rs"),
            64,
            |path, _| {
                calls.push(path.to_string());
                BoundedTextRead::Unreadable { bytes_read: 19 }
            },
        );

        assert_eq!(outcome, BoundedTextRead::Unreadable { bytes_read: 19 });
        assert_eq!(calls, vec![r"\\?\C:\source.rs"]);

        let mut calls = Vec::new();
        let outcome = read_searchable_file_contents_limited_with(
            r"\\?\C:\source.rs",
            Some(r"C:\source.rs"),
            64,
            |path, _| {
                calls.push(path.to_string());
                if calls.len() == 1 {
                    BoundedTextRead::Unreadable { bytes_read: 0 }
                } else {
                    BoundedTextRead::Contents {
                        contents: "fallback".to_string(),
                        bytes_read: 8,
                    }
                }
            },
        );

        assert_eq!(
            outcome,
            BoundedTextRead::Contents {
                contents: "fallback".to_string(),
                bytes_read: 8,
            }
        );
        assert_eq!(calls, vec![r"\\?\C:\source.rs", r"C:\source.rs"]);
    }
}
