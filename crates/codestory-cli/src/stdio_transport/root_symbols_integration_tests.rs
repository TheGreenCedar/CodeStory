use super::*;

#[test]
fn root_symbols_resource_pages_round_trip_the_public_dispatcher() {
    let project = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let source: String = (0..137)
        .map(|i| format!("pub fn item_{i:05}() {{}}\n"))
        .collect();
    std::fs::write(project.path().join("items.rs"), source).unwrap();
    let args = args::ProjectArgs {
        project: project.path().to_path_buf(),
        cache_dir: Some(cache.path().to_path_buf()),
    };
    let startup = stdio_multi_project_startup_for_root_tests(cache.path());
    let writer = RuntimeContext::new_inspect_only_with_startup(&args, &startup).unwrap();
    writer.ensure_core_open(args::RefreshMode::Full).unwrap();
    let mut session = StdioServerSession::new(Some(writer));
    session.startup = startup;
    let base = stdio_resource_uri_for_project(&StdioResource::RootSymbols, project.path());
    let mut uri = format!("{base}&limit=17");
    let mut all = std::collections::HashSet::new();
    let mut count = 0usize;
    let mut binding = None;
    loop {
        let request = serde_json::json!({"jsonrpc":"2.0", "id":"root-page", "method":"resources/read", "params":{"uri":uri}});
        let response = handle_stdio_message(
            &mut session,
            &request.to_string(),
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(response.get("error").is_none(), "{response}");
        assert!(serde_json::to_vec(&response).unwrap().len() <= 8192);
        let text = response
            .pointer("/result/contents/0/text")
            .unwrap()
            .as_str()
            .unwrap();
        let page: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(page["start_offset"].as_u64().unwrap() as usize, count);
        let current = serde_json::json!([
            page["project_id"],
            page["publication"],
            page["inventory_sha256"]
        ]);
        if let Some(expected) = &binding {
            assert_eq!(expected, &current);
        }
        binding = Some(current);
        for row in page["symbols"].as_array().unwrap() {
            assert!(all.insert(row["id"].clone()));
            count += 1;
        }
        assert_eq!(
            count + page["remaining_count"].as_u64().unwrap() as usize,
            page["total_count"].as_u64().unwrap() as usize
        );
        match page["continuation_uri"].as_str() {
            Some(next) => uri = next.into(),
            None => {
                assert_eq!(page["truncated"], false);
                break;
            }
        }
    }
    assert!(count >= 137);
    assert_eq!(count, all.len());
    assert!(
        session
            .active_project
            .as_ref()
            .unwrap()
            .runtime
            .activation
            .snapshot()
            .is_none()
    );
}

fn stdio_multi_project_startup_for_root_tests(cache: &Path) -> crate::config::CliStartupConfig {
    crate::config::CliStartupConfig {
        user_home: None,
        allow_sensitive_project_root: false,
        project_network_config_allowed: false,
        stdio_cache_root: Some(cache.to_path_buf()),
        sidecar_defaults: codestory_retrieval::SidecarProcessDefaults::new(
            cache.to_path_buf(),
            codestory_retrieval::SidecarRuntimeDefaults::default(),
        ),
        source_index_policy: codestory_contracts::workspace::SourceIndexPolicy::default(),
    }
}
