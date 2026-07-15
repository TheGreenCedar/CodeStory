//! Sidecar primary rejection when candidates are phantom-only.

use codestory_retrieval::{CandidateHit, CandidateSource, QueryResult, QueryTrace, classify_query};

#[test]
fn phantom_only_candidates_are_detected() {
    let hits = vec![
        CandidateHit::with_source(
            "lexical:handler",
            Some("handler".into()),
            0.5,
            CandidateSource::Lexical,
        ),
        CandidateHit::with_source(
            "semantic:handler",
            Some("handler".into()),
            0.55,
            CandidateSource::Semantic,
        ),
    ];
    assert!(codestory_retrieval::phantom_sidecar_candidates_only(&hits));
    assert!(!codestory_retrieval::phantom_sidecar_candidates_only(&[
        CandidateHit::lexical_stub("src/lib.rs", 0.9,)
    ]));

    let _query = QueryResult {
        publication_identity: None,
        query: "handler".into(),
        features: classify_query("handler"),
        hits,
        trace: QueryTrace {
            retrieval_mode: "no_semantic".into(),
            degraded_reason: Some("semantic_hash_vectors_only".into()),
            total_budget_ms: 500,
            elapsed_ms: 1,
            cancel_reason: None,
            cache_hit: false,
            stages: Vec::new(),
        },
    };
}
