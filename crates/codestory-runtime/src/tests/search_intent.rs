#[test]
fn parse_search_intent_query_extracts_supported_filters() {
    let parsed = parse_search_intent_query(
        "kind:function name:`listUsers` path:src/routes.ts lang:typescript",
    );

    assert_eq!(parsed.effective_query, "listUsers");
    assert_eq!(
        parsed.filters,
        vec![
            SearchIntentFilter::Kind("function".to_string()),
            SearchIntentFilter::Name("listUsers".to_string()),
            SearchIntentFilter::Path("src/routes.ts".to_string()),
            SearchIntentFilter::Language("typescript".to_string()),
        ]
    );

    let unknown_prefix = parse_search_intent_query("owner:web /api/users");
    assert_eq!(unknown_prefix.effective_query, "owner:web /api/users");
    assert!(unknown_prefix.filters.is_empty());
}

#[test]
fn search_intent_filters_hits_by_kind_path_name_and_language() {
    fn hit(
        id: &str,
        display_name: &str,
        kind: codestory_contracts::api::NodeKind,
        file_path: &str,
    ) -> SearchHit {
        SearchHit {
            node_id: codestory_contracts::api::NodeId(id.to_string()),
            display_name: display_name.to_string(),
            kind,
            file_path: Some(file_path.to_string()),
            line: Some(1),
            score: 1.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        }
    }

    let mut hits = vec![
        hit(
            "a",
            "listUsers",
            codestory_contracts::api::NodeKind::FUNCTION,
            "src/routes.ts",
        ),
        hit(
            "b",
            "Users",
            codestory_contracts::api::NodeKind::STRUCT,
            "src/routes.ts",
        ),
        hit(
            "c",
            "listUsers",
            codestory_contracts::api::NodeKind::FUNCTION,
            "src/routes.rs",
        ),
    ];

    apply_search_intent_filters(
        &mut hits,
        &[
            SearchIntentFilter::Kind("function".to_string()),
            SearchIntentFilter::Path("routes.ts".to_string()),
            SearchIntentFilter::Name("listUsers".to_string()),
            SearchIntentFilter::Language("typescript".to_string()),
        ],
    );

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].display_name, "listUsers");
    assert_eq!(hits[0].file_path.as_deref(), Some("src/routes.ts"));
}

#[test]
fn language_filter_uses_shared_registry_extensions() {
    for (requested, path) in [
        ("bash", "scripts/bootstrap.sh"),
        ("bash", "scripts/bootstrap.bash"),
        ("sh", "scripts/bootstrap.sh"),
        ("python", "pkg/types.pyi"),
        ("ts", "src/server.mts"),
        ("typescript", "src/server.cts"),
        ("dart", "lib/main.dart"),
        ("html", "templates/index.htm"),
        ("css", "assets/site.css"),
        ("sql", "db/schema.sql"),
        ("c++", "include/runtime.hh"),
        ("c#", "src/App.cs"),
        ("markdown", "docs/guide.mdx"),
    ] {
        assert!(
            language_filter_matches_path(requested, path),
            "expected language:{requested} to match {path}"
        );
    }

    assert!(!language_filter_matches_path("bash", "src/main.py"));
    assert!(!language_filter_matches_path(
        "sh",
        "scripts/bootstrap.bash"
    ));
    assert!(!language_filter_matches_path("tsx", "src/server.ts"));
    assert!(!language_filter_matches_path("jsx", "src/app.js"));

    assert!(indexed_file_matches_language_filter(
        "typescript",
        Path::new("src/Widget.tsx"),
        "tsx"
    ));
    assert!(indexed_file_matches_language_filter(
        "bash",
        Path::new("scripts/bootstrap.sh"),
        "bash"
    ));
    assert!(!indexed_file_matches_language_filter(
        "typescript",
        Path::new("src/server.ts"),
        "tsx"
    ));
}

#[test]
fn extract_symbol_search_terms_removes_stopwords_and_short_tokens() {
    let terms = extract_symbol_search_terms("How does the language parsing work in this repo?");
    assert_eq!(terms, vec!["language".to_string(), "parsing".to_string()]);
}

#[test]
fn should_expand_symbol_query_for_sentence_prompts() {
    assert!(should_expand_symbol_query(
        "How does the language parsing work in this repo?",
        0
    ));
    assert!(!should_expand_symbol_query("parser", 0));
    assert!(!should_expand_symbol_query(
        "how does the language parsing work in this repo",
        5
    ));
    assert!(!should_expand_symbol_query(
        "How does the language parsing work in this repo?",
        5
    ));
}

#[test]
fn mixed_natural_language_query_detects_embedded_symbol_prompts() {
    assert!(mixed_natural_language_query(
        "how ExtensionHostManager starts"
    ));
    assert!(!mixed_natural_language_query("Workbench"));
    assert!(!mixed_natural_language_query("Subcommand::Exec"));
}
