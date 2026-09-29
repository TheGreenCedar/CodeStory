//! Regression ranking checks for OSS holdout retrieval prompts.

use codestory_retrieval::{
    CandidateHit, CandidateLane, CandidateSource, classify_query, rank_candidates,
};

/// Pin an equal producer rank so a raw-score tie cannot hand one fixture a
/// better lane rank through path-sort tie-breaking; the query overlap decides.
fn with_lexical_rank(hit: &mut CandidateHit, raw_score: f32) {
    hit.record_lane(CandidateLane::Lexical, raw_score, 1, "lexical_source");
}

fn holdout_candidates_ripgrep() -> Vec<CandidateHit> {
    let mut readme = CandidateHit::with_source("README.md", None, 0.4, CandidateSource::Lexical);
    with_lexical_rank(&mut readme, 0.4);
    let mut search = CandidateHit::with_source(
        "crates/core/search.rs",
        Some("SearchWorker".into()),
        0.82,
        CandidateSource::Lexical,
    );
    search.qualified_name = Some("core::search::searcher".into());
    with_lexical_rank(&mut search, 0.82);
    let mut flags = CandidateHit::with_source(
        "crates/core/flags.rs",
        Some("parse".into()),
        0.82,
        CandidateSource::Lexical,
    );
    with_lexical_rank(&mut flags, 0.82);
    vec![
        readme,
        CandidateHit::with_source(
            "lexical:search pipeline",
            None,
            0.95,
            CandidateSource::Lexical,
        ),
        search,
        flags,
    ]
}

#[test]
fn holdout_ripgrep_prompt_prefers_search_driver_files() {
    // The desired winner is neither first in input order nor ahead on raw
    // score, and the paired flags query flips the winner, so an identity
    // ranker or raw-score sort cannot satisfy both directions.
    let search_query = classify_query(
        "Explain how ripgrep parses CLI flags, walks candidate files, and executes search through matcher, searcher, and printer components.",
    );
    let flags_query = classify_query("Explain how ripgrep parses CLI flags into the config struct");

    let search_ranked = rank_candidates(&search_query, holdout_candidates_ripgrep());
    assert_eq!(
        search_ranked.first().map(|hit| hit.file_path.as_str()),
        Some("crates/core/search.rs"),
        "the search-driver file must win the search query: {search_ranked:#?}"
    );
    assert!(
        search_ranked
            .iter()
            .all(|hit| !hit.file_path.starts_with("lexical:")),
        "phantom hits must be dropped"
    );

    let flags_ranked = rank_candidates(&flags_query, holdout_candidates_ripgrep());
    assert_eq!(
        flags_ranked.first().map(|hit| hit.file_path.as_str()),
        Some("crates/core/flags.rs"),
        "the flag-parsing file must win the flags query: {flags_ranked:#?}"
    );
}

#[test]
fn holdout_axios_prompt_prefers_dispatch_path() {
    let dispatch_query = classify_query(
        "Explain how the default axios instance is created and how an HTTP request flows through interceptors, dispatchRequest, and the transport adapter.",
    );
    let defaults_query =
        classify_query("Explain where axios keeps the default configuration and instance defaults");
    let mut defaults =
        CandidateHit::with_source("lib/defaults.js", None, 0.8, CandidateSource::Lexical);
    with_lexical_rank(&mut defaults, 0.8);
    let mut dispatch = CandidateHit::with_source(
        "lib/core/dispatchRequest.js",
        Some("dispatchRequest".into()),
        0.8,
        CandidateSource::Lexical,
    );
    with_lexical_rank(&mut dispatch, 0.8);
    let candidates = vec![
        defaults,
        CandidateHit::with_source("semantic:axios", None, 0.95, CandidateSource::Semantic),
        dispatch,
    ];

    let dispatch_ranked = rank_candidates(&dispatch_query, candidates.clone());
    assert_eq!(
        dispatch_ranked.first().map(|hit| hit.file_path.as_str()),
        Some("lib/core/dispatchRequest.js"),
        "the dispatch implementation must win the dispatch query: {dispatch_ranked:#?}"
    );
    assert!(
        dispatch_ranked
            .iter()
            .all(|hit| !hit.file_path.starts_with("semantic:")),
        "phantom hits must be dropped"
    );

    let defaults_ranked = rank_candidates(&defaults_query, candidates);
    assert_eq!(
        defaults_ranked.first().map(|hit| hit.file_path.as_str()),
        Some("lib/defaults.js"),
        "the defaults module must win the defaults query: {defaults_ranked:#?}"
    );
}

#[test]
fn holdout_redis_prompt_prefers_server_event_loop_files() {
    let server_query = classify_query(
        "Explain how the Redis server starts its event loop, reads client commands from the network, and dispatches them through processCommand.",
    );
    let event_query =
        classify_query("Explain how aeMain arms event timers inside the Redis event loop");
    let mut event_loop = CandidateHit::with_source(
        "src/ae.c",
        Some("aeMain".into()),
        0.82,
        CandidateSource::Lexical,
    );
    with_lexical_rank(&mut event_loop, 0.82);
    let mut readme = CandidateHit::with_source("README.md", None, 0.4, CandidateSource::Lexical);
    with_lexical_rank(&mut readme, 0.4);
    let mut server = CandidateHit::with_source(
        "src/server.c",
        Some("processCommand".into()),
        0.82,
        CandidateSource::Lexical,
    );
    with_lexical_rank(&mut server, 0.82);
    let candidates = vec![event_loop, readme, server];

    let server_ranked = rank_candidates(&server_query, candidates.clone());
    assert_eq!(
        server_ranked.first().map(|hit| hit.file_path.as_str()),
        Some("src/server.c"),
        "the command-dispatch server file must win its query: {server_ranked:#?}"
    );

    let event_ranked = rank_candidates(&event_query, candidates);
    assert_eq!(
        event_ranked.first().map(|hit| hit.file_path.as_str()),
        Some("src/ae.c"),
        "the event-loop file must win its query: {event_ranked:#?}"
    );
}

#[test]
fn holdout_path_like_query_boosts_matching_file() {
    let mut walk = CandidateHit::lexical_stub("crates/ignore/walk.rs", 0.5);
    with_lexical_rank(&mut walk, 0.5);
    let mut main = CandidateHit::lexical_stub("crates/core/main.rs", 0.5);
    with_lexical_rank(&mut main, 0.5);
    let candidates = vec![walk, main];

    let main_ranked = rank_candidates(
        &classify_query("crates/core/main.rs search"),
        candidates.clone(),
    );
    assert_eq!(
        main_ranked.first().map(|hit| hit.file_path.as_str()),
        Some("crates/core/main.rs"),
        "the path-matching file must win: {main_ranked:#?}"
    );

    let walk_ranked = rank_candidates(&classify_query("crates/ignore/walk.rs"), candidates);
    assert_eq!(
        walk_ranked.first().map(|hit| hit.file_path.as_str()),
        Some("crates/ignore/walk.rs"),
        "the same candidate set must flip with the queried path: {walk_ranked:#?}"
    );
}
