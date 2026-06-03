//! Regression ranking checks for OSS holdout retrieval prompts.

use codestory_retrieval::{CandidateHit, CandidateSource, classify_query, rank_candidates};

fn holdout_candidates_ripgrep() -> Vec<CandidateHit> {
    vec![
        CandidateHit::with_source(
            "crates/core/search.rs",
            Some("SearchWorker".into()),
            0.82,
            CandidateSource::Zoekt,
        ),
        CandidateHit::with_source(
            "crates/core/flags/mod.rs",
            Some("parse".into()),
            0.65,
            CandidateSource::Zoekt,
        ),
        CandidateHit::with_source("zoekt:search pipeline", None, 0.95, CandidateSource::Zoekt),
        CandidateHit::with_source("README.md", None, 0.2, CandidateSource::Zoekt),
    ]
}

#[test]
fn holdout_ripgrep_prompt_prefers_search_driver_files() {
    let features = classify_query(
        "Explain how ripgrep parses CLI flags, walks candidate files, and executes search through matcher, searcher, and printer components.",
    );
    let ranked = rank_candidates(&features, holdout_candidates_ripgrep());
    assert!(!ranked.is_empty());
    assert_eq!(ranked[0].file_path, "crates/core/search.rs");
    assert!(
        ranked
            .iter()
            .all(|hit| !hit.file_path.starts_with("zoekt:")),
        "phantom hits must be dropped"
    );
}

#[test]
fn holdout_axios_prompt_prefers_dispatch_path() {
    let features = classify_query(
        "Explain how the default axios instance is created and how an HTTP request flows through interceptors, dispatchRequest, and the transport adapter.",
    );
    let candidates = vec![
        CandidateHit::with_source(
            "lib/core/dispatchRequest.js",
            Some("dispatchRequest".into()),
            0.8,
            CandidateSource::Zoekt,
        ),
        CandidateHit::with_source("lib/defaults.js", None, 0.75, CandidateSource::Zoekt),
        CandidateHit::with_source("semantic:axios", None, 0.95, CandidateSource::Qdrant),
    ];
    let ranked = rank_candidates(&features, candidates);
    assert_eq!(ranked[0].file_path, "lib/core/dispatchRequest.js");
}

#[test]
fn holdout_redis_prompt_prefers_server_event_loop_files() {
    let features = classify_query(
        "Explain how the Redis server starts its event loop, reads client commands from the network, and dispatches them through processCommand.",
    );
    let candidates = vec![
        CandidateHit::with_source(
            "src/server.c",
            Some("main".into()),
            0.82,
            CandidateSource::Zoekt,
        ),
        CandidateHit::with_source(
            "src/ae.c",
            Some("aeMain".into()),
            0.78,
            CandidateSource::Zoekt,
        ),
        CandidateHit::with_source("README.md", None, 0.3, CandidateSource::Zoekt),
    ];
    let ranked = rank_candidates(&features, candidates);
    assert_eq!(ranked[0].file_path, "src/server.c");
}

#[test]
fn holdout_path_like_query_boosts_matching_file() {
    let features = classify_query("crates/core/main.rs search");
    let candidates = vec![
        CandidateHit::lexical_stub("crates/core/main.rs", 0.85),
        CandidateHit::lexical_stub("crates/ignore/walk.rs", 0.4),
    ];
    let ranked = rank_candidates(&features, candidates);
    assert_eq!(ranked[0].file_path, "crates/core/main.rs");
}
