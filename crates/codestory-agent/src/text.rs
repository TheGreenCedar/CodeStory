//! Pure lexical and path classification used by packet planning.
//!
//! These helpers were `codestory-runtime`'s `symbol_query` internals. They read
//! nothing: no controller, no store, no publication, no filesystem. Planning
//! needs them on every prompt, so they move with planning rather than staying
//! behind a runtime import that would make `codestory-agent` depend on the
//! crate it was extracted from.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalFileRole {
    Source,
    Test,
    Docs,
    Benchmark,
    Generated,
    Vendor,
}

impl RetrievalFileRole {
    pub fn is_non_primary(self) -> bool {
        !matches!(self, Self::Source)
    }
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

pub fn retrieval_file_role_from_path(path: &str) -> RetrievalFileRole {
    let normalized_raw = normalize_retrieval_path(path);
    let normalized = strip_materialized_repo_cache_prefix(&normalized_raw).to_string();
    let marked = format!("/{normalized}");
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());

    if path_contains_any(
        &marked,
        &[
            "/node_modules/",
            "/src/external/",
            "/external/",
            "/deps/",
            "/vendor/",
            "/vendors/",
            "/third_party/",
            "/third-party/",
        ],
    ) {
        return RetrievalFileRole::Vendor;
    }

    if path_contains_any(&marked, &["/target/", "/dist/", "/build/", "/generated/"])
        || marked.contains("/schema/typescript/")
        || marked.contains(".generated.")
        || file_name.contains("generated")
        || file_name.ends_with(".g.cs")
    {
        return RetrievalFileRole::Generated;
    }

    if path_contains_any(
        &marked,
        &["/benches/", "/bench/", "/benchmarks/", "/benchmark/"],
    ) || (marked.contains("/scripts/")
        && (marked.contains("bench") || marked.contains("benchmark")))
    {
        return RetrievalFileRole::Benchmark;
    }

    if path_contains_any(
        &marked,
        &[
            "/bin/test/",
            "/test/data/",
            "/tests/",
            "/test/",
            "/spec/",
            "/fixtures/",
            "/fixture/",
            "/examples/",
            "/example/",
            "/__tests__/",
            "/__test__/",
            "-test-client/",
            "_test_client/",
        ],
    ) || file_name.contains(".test.")
        || file_name.contains(".spec.")
        || file_name.ends_with("_test.rs")
        || file_name.ends_with("_tests.rs")
        || file_name.ends_with("_test.py")
        || file_name.ends_with("_tests.py")
        || file_name.ends_with("_test.ts")
        || file_name.ends_with("_tests.ts")
        || file_name.ends_with("_test.tsx")
        || file_name.ends_with("_tests.tsx")
        || file_name.ends_with("test.java")
        || file_name.ends_with("tests.java")
    {
        return RetrievalFileRole::Test;
    }

    if path_contains_any(&marked, &["/docs/", "/doc/"])
        || matches!(file_name, "readme.md" | "changelog.md")
    {
        return RetrievalFileRole::Docs;
    }

    RetrievalFileRole::Source
}

fn normalize_retrieval_path(path: &str) -> String {
    path.trim_start_matches("\\\\?\\")
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_ascii_lowercase()
}

fn strip_materialized_repo_cache_prefix(path: &str) -> &str {
    let mut best_match: Option<(usize, &str)> = None;
    for marker in ["/source/repos/", "source/repos/", "/repos/", "repos/"] {
        let Some(index) = path.rfind(marker) else {
            continue;
        };
        let after_marker = &path[index + marker.len()..];
        if let Some((_, repo_relative)) = after_marker.split_once('/')
            && !repo_relative.is_empty()
            && best_match
                .as_ref()
                .is_none_or(|(best_index, _)| index > *best_index)
        {
            best_match = Some((index, repo_relative));
        }
    }
    best_match
        .map(|(_, repo_relative)| repo_relative)
        .unwrap_or(path)
}

fn path_contains_any(path: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| path.contains(marker))
}

pub fn terms_contain_phrase(terms: &[String], phrase: &[&str]) -> bool {
    terms
        .windows(phrase.len())
        .any(|window| window.iter().map(String::as_str).eq(phrase.iter().copied()))
}

pub fn query_mentions_non_primary_source(query: &str) -> bool {
    let terms = query
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .map(|term| term.to_ascii_lowercase())
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();

    terms.iter().enumerate().any(|(index, term)| {
        is_non_primary_source_term(term) && !is_non_primary_source_exclusion_context(&terms, index)
    })
}

pub fn is_non_primary_source_term(term: &str) -> bool {
    matches!(
        term,
        "test"
            | "tests"
            | "testing"
            | "doc"
            | "docs"
            | "documentation"
            | "example"
            | "examples"
            | "sample"
            | "samples"
            | "script"
            | "scripts"
            | "bench"
            | "benchmark"
            | "benchmarks"
            | "fixture"
            | "fixtures"
            | "external"
            | "vendor"
            | "vendors"
            | "vendored"
            | "generated"
            | "thirdparty"
            | "third_party"
            | "third-party"
    )
}

fn is_non_primary_source_exclusion_context(terms: &[String], index: usize) -> bool {
    let start = index.saturating_sub(8);
    let end = (index + 9).min(terms.len());
    terms[start..end].iter().any(|term| {
        matches!(
            term.as_str(),
            "avoid"
                | "demote"
                | "demotes"
                | "demoted"
                | "downrank"
                | "downranking"
                | "exclude"
                | "excluding"
                | "hide"
                | "ignore"
                | "omit"
                | "pollute"
                | "pollution"
                | "precision"
                | "primary"
                | "prod"
                | "production"
                | "role"
                | "roles"
                | "skip"
                | "without"
        )
    })
}
