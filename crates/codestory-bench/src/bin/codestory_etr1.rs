//! Benchmark-only implementation of the frozen ETR-1 frontier experiment.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ByteRangeV1 {
    start: u64,
    end: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LineRangeV1 {
    start: u32,
    end: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FrozenFragmentV1 {
    fragment_id: String,
    project_id: String,
    path: String,
    content_digest: String,
    byte_range: ByteRangeV1,
    line_range: LineRangeV1,
    source: String,
    serialized_row_bytes: u32,
}

fn sha256(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

fn fragment_id(project_id: &str, path: &str, content_digest: &str, range: ByteRangeV1) -> String {
    let mut digest = Sha256::new();
    digest.update(b"codestory.frozen-fragment/v1\0");
    for value in [
        project_id.as_bytes(),
        path.as_bytes(),
        content_digest.as_bytes(),
    ] {
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value);
    }
    digest.update(range.start.to_le_bytes());
    digest.update(range.end.to_le_bytes());
    format!("{:x}", digest.finalize())
}

fn select_successors(
    score_order: &[(String, f32)],
    seeds: &BTreeSet<String>,
    prior: &BTreeSet<String>,
    limit: usize,
) -> Vec<String> {
    let mut selected = Vec::with_capacity(limit.min(score_order.len()));
    let mut seen = BTreeSet::new();
    for (fragment_id, _) in score_order {
        if !seeds.contains(fragment_id) && !prior.contains(fragment_id) && seen.insert(fragment_id)
        {
            selected.push(fragment_id.clone());
            if selected.len() == limit {
                break;
            }
        }
    }
    selected
}

fn candidate_query_with_shortening<F>(
    question: &str,
    source: &str,
    fits: F,
) -> Result<(String, u32)>
where
    F: Fn(&str) -> bool,
{
    ensure!(!question.trim().is_empty(), "question_is_empty");
    ensure!(!source.is_empty(), "seed_source_is_empty");
    let lines = source.split_inclusive('\n').collect::<Vec<_>>();
    ensure!(!lines.is_empty(), "seed_source_has_no_lines");
    for retained in (1..=lines.len()).rev() {
        let retained_source = lines[..retained].concat();
        if retained_source.trim().is_empty() {
            continue;
        }
        let query = format!("{question}\n\n{retained_source}");
        if fits(&query) {
            return Ok((query, u32::try_from(lines.len() - retained)?));
        }
    }
    anyhow::bail!("no_complete_seed_source_line_fits")
}

fn natural_seed_prefix<T: Clone>(matches: &[T]) -> Vec<T> {
    matches.iter().take(16).cloned().collect()
}

fn main() -> Result<()> {
    anyhow::bail!("ETR-1 commands are not implemented yet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_identity_binds_project_path_digest_and_range() {
        let range = ByteRangeV1 { start: 7, end: 19 };
        let baseline = fragment_id("project-a", "src/lib.rs", &"a".repeat(64), range);
        assert_eq!(baseline.len(), 64);
        assert_ne!(
            baseline,
            fragment_id("project-b", "src/lib.rs", &"a".repeat(64), range)
        );
        assert_ne!(
            baseline,
            fragment_id("project-a", "src/main.rs", &"a".repeat(64), range)
        );
        assert_ne!(
            baseline,
            fragment_id("project-a", "src/lib.rs", &"b".repeat(64), range)
        );
        assert_ne!(
            baseline,
            fragment_id(
                "project-a",
                "src/lib.rs",
                &"a".repeat(64),
                ByteRangeV1 { start: 8, end: 19 }
            )
        );
    }

    #[test]
    fn natural_seed_prefix_preserves_underfill_and_order() {
        assert_eq!(natural_seed_prefix(&[3, 1, 2]), vec![3, 1, 2]);
        assert_eq!(
            natural_seed_prefix(&(0..20).collect::<Vec<_>>()),
            (0..16).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cumulative_exclusions_produce_unique_successors() {
        let scores = vec![
            ("seed".into(), 1.0),
            ("prior".into(), 0.9),
            ("new-a".into(), 0.8),
            ("new-b".into(), 0.8),
            ("new-c".into(), 0.7),
        ];
        let selected = select_successors(
            &scores,
            &BTreeSet::from(["seed".into()]),
            &BTreeSet::from(["prior".into()]),
            2,
        );
        assert_eq!(selected, ["new-a", "new-b"]);
    }

    #[test]
    fn query_shortening_keeps_utf8_and_removes_complete_trailing_lines() {
        let source = "first α line\nsecond β line\nthird γ line\n";
        let maximum = "question\n\nfirst α line\nsecond β line\n".len();
        let (query, removed) =
            candidate_query_with_shortening("question", source, |value| value.len() <= maximum)
                .unwrap();
        assert_eq!(query, "question\n\nfirst α line\nsecond β line\n");
        assert_eq!(removed, 1);
        assert!(std::str::from_utf8(query.as_bytes()).is_ok());
        assert!(candidate_query_with_shortening("question", source, |_| false).is_err());
    }

    #[test]
    fn lexical_contract_uses_the_product_normalizer_and_stop_words() {
        assert_eq!(
            codestory_retrieval::benchmark_support::etr1_lexical_document("HTTPServer run_pending"),
            "http server run pending"
        );
        assert_eq!(
            codestory_retrieval::benchmark_support::etr1_lexical_query_terms(
                "How does HTTPServer run_pending work?"
            ),
            vec!["http", "server", "run", "pending", "work"]
        );
    }
}
