//! UNEXECUTED source-only draft. Integrate as a cfg(test) path module.
//! Known scores/IDs form the oracle; no expected ranking uses the merge helper.

use crate::embedded_vector::merge_query_window_hits;
use crate::query_windows::QueryWindowVector;
use crate::{CandidateHit, CandidateSource};

macro_rules! hit {
    ($id:expr, $score:expr) => {{
        let mut hit = CandidateHit::with_source(
            format!("/source/{}.rs", $id),
            Some(format!("symbol_{}", $id)),
            $score,
            CandidateSource::Semantic,
        );
        hit.node_id = Some($id.into());
        hit
    }};
}

fn windows(count: usize) -> Vec<QueryWindowVector> {
    (0..count)
        .map(|i| QueryWindowVector {
            range: i * 10..(i + 1) * 10,
            vector: vec![i as f32, 1.0],
        })
        .collect()
}

fn ids(hits: &[CandidateHit]) -> Vec<&str> {
    hits.iter()
        .map(|hit| hit.node_id.as_deref().unwrap())
        .collect()
}

#[test]
fn duplicated_node_keeps_the_maximum_score_instead_of_sum_or_last() {
    let result = merge_query_window_hits(
        &windows(3),
        vec![
            vec![hit!("a", 0.25), hit!("b", 0.90)],
            vec![hit!("a", 0.80), hit!("c", 0.70)],
            vec![hit!("a", 0.30)],
        ],
        true,
        10,
    )
    .unwrap();
    assert_eq!(ids(&result), ["b", "a", "c"]);
    assert_eq!(result[0].score, 0.90);
    assert_eq!(result[1].score, 0.80);
    assert_eq!(result[2].score, 0.70);
}

#[test]
fn repeating_a_window_does_not_inflate_scores_or_evidence_count() {
    let duplicated = vec![
        QueryWindowVector {
            range: 0..10,
            vector: vec![1.0, 0.0],
        },
        QueryWindowVector {
            range: 0..10,
            vector: vec![1.0, 0.0],
        },
    ];
    let once = merge_query_window_hits(
        &duplicated[..1],
        vec![vec![hit!("a", 0.40), hit!("b", 0.30)]],
        true,
        10,
    )
    .unwrap();
    let twice = merge_query_window_hits(
        &duplicated,
        vec![
            vec![hit!("a", 0.40), hit!("b", 0.30)],
            vec![hit!("a", 0.40), hit!("b", 0.30)],
        ],
        true,
        10,
    )
    .unwrap();
    assert_eq!(ids(&twice), ["a", "b"]);
    assert_eq!(twice.len(), once.len());
    assert_eq!(twice[0].score, 0.40);
    assert_eq!(twice[1].score, 0.30);
}

#[test]
fn descending_score_then_node_id_order_is_independent_of_input_order() {
    let result = merge_query_window_hits(
        &windows(2),
        vec![
            vec![hit!("z", 0.80), hit!("b", 0.50), hit!("a", 0.50)],
            vec![hit!("c", 0.50), hit!("z", 0.75)],
        ],
        true,
        10,
    )
    .unwrap();
    assert_eq!(ids(&result), ["z", "a", "b", "c"]);
    let reordered = merge_query_window_hits(
        &windows(2),
        vec![
            vec![hit!("z", 0.75), hit!("c", 0.50)],
            vec![hit!("a", 0.50), hit!("b", 0.50), hit!("z", 0.80)],
        ],
        true,
        10,
    )
    .unwrap();
    assert_eq!(ids(&reordered), ["z", "a", "b", "c"]);
    assert_eq!(reordered[0].score, 0.80);
}

#[test]
fn an_abstained_window_adds_no_node_and_changes_no_surviving_score() {
    let result = merge_query_window_hits(
        &windows(3),
        vec![vec![], vec![hit!("a", 0.60)], vec![]],
        true,
        10,
    )
    .unwrap();
    assert_eq!(ids(&result), ["a"]);
    assert_eq!(result[0].score, 0.60);
    assert!(
        merge_query_window_hits(&windows(2), vec![vec![], vec![]], true, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn original_query_limit_is_applied_after_union_and_deduplication() {
    // Truncating this first window to two before dedup loses b, while returning
    // two per window exceeds the single original-query budget.
    let result = merge_query_window_hits(
        &windows(2),
        vec![
            vec![
                hit!("a", 0.95),
                hit!("a", 0.94),
                hit!("a", 0.93),
                hit!("b", 0.90),
            ],
            vec![hit!("c", 0.40), hit!("d", 0.30)],
        ],
        true,
        2,
    )
    .unwrap();
    assert_eq!(ids(&result), ["a", "b"]);
    assert_eq!(result[0].score, 0.95);
    assert_eq!(result[1].score, 0.90);
}

#[test]
fn missing_node_identity_fails_closed_even_with_a_symbol_and_high_score() {
    let mut missing = hit!("symbol_only", 0.99);
    missing.node_id = None;
    let result = merge_query_window_hits(
        &windows(2),
        vec![vec![hit!("valid", 0.50)], vec![missing]],
        true,
        1,
    );
    assert!(
        result.is_err(),
        "unidentified hit silently accepted or discarded"
    );
}

#[test]
fn non_finite_scores_cannot_enter_expanded_union_ranking() {
    for score in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let result = merge_query_window_hits(
            &windows(2),
            vec![vec![hit!("valid", 0.50)], vec![hit!("invalid", score)]],
            true,
            10,
        );
        assert!(
            result.is_err(),
            "non-finite score was ranked or silently discarded"
        );
    }
}

#[test]
fn too_few_or_too_many_window_results_fail_closed() {
    for per_window in [
        vec![],
        vec![vec![hit!("a", 0.70)]],
        vec![vec![hit!("a", 0.70)], vec![], vec![hit!("b", 0.80)]],
    ] {
        assert!(merge_query_window_hits(&windows(2), per_window, true, 10).is_err());
    }
}

#[test]
fn unchanged_short_query_preserves_the_entire_result_in_existing_order() {
    // Deliberately unsorted: the unchanged path must not alter legacy ranking,
    // or metadata while expanded queries use the union policy.
    let expected = vec![hit!("z", 0.30), hit!("a", 0.90), hit!("m", 0.40)];
    let supplied = vec![hit!("z", 0.30), hit!("a", 0.90), hit!("m", 0.40)];
    let result = merge_query_window_hits(&windows(1), vec![supplied], false, 10).unwrap();
    assert_eq!(format!("{result:?}"), format!("{expected:?}"));
}
