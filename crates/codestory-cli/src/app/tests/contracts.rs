use super::test_support::{
    sample_graph_edge, sample_graph_node, sample_node_details, test_search_hit_defaults,
};
use crate::app::artifacts::ensure_dot_only_for_trail;
use crate::app::rendering::hide_speculative_trail_edges;
use crate::app::resolution::{quote_command_path, quote_command_value};
use crate::app::source_commands::render_affected_invocation;
use crate::app::{
    map_embedding_preflight_error, stdio_prompts_list_json, stdio_resource_templates_list_json,
    stdio_resources_list_json, stdio_tools_list_json, validate_index_watch_output_file,
};
use crate::args::{self, IndexCommand, QuerySelectorOutput};
use crate::display::{clean_path_string, quote_command_argument_value, relative_path};
use crate::runtime::{self, cache_root_for_project};
use codestory_contracts::api::{
    AffectedFollowUpInvocationDto, GraphResponse, NodeId, NodeKind, SearchHit, TrailContextDto,
};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

#[test]
fn command_quoting_single_quotes_shell_sensitive_values() {
    #[cfg(windows)]
    assert_eq!(
        quote_command_value("Inspect $env:SECRET and $(Get-ChildItem) and 'literal'"),
        "'Inspect $env:SECRET and $(Get-ChildItem) and ''literal'''"
    );
    #[cfg(not(windows))]
    assert_eq!(
        quote_command_value("Inspect $env:SECRET and $(Get-ChildItem) and 'literal'"),
        r"'Inspect $env:SECRET and $(Get-ChildItem) and '\''literal'\'''"
    );
    assert_eq!(
        quote_command_path(Path::new("C:/repo/$hidden")),
        "'C:/repo/$hidden'"
    );
    assert_eq!(
        quote_command_path(Path::new("C:/repo/quoted\"path")),
        "'C:/repo/quoted\"path'"
    );
    assert_eq!(quote_command_path(Path::new("C:/repo")), "\"C:/repo\"");
}

#[test]
fn affected_structured_invocation_is_quoted_only_when_cli_renders_it() {
    let invocation = AffectedFollowUpInvocationDto {
        program: "codestory-cli".to_string(),
        args: vec![
            "files".to_string(),
            "--project".to_string(),
            "C:/repo/$hidden".to_string(),
            "--path".to_string(),
            "src/quoted'file.rs".to_string(),
        ],
    };

    let rendered = render_affected_invocation(&invocation);
    assert!(rendered.starts_with("codestory-cli "));
    assert!(rendered.contains(&quote_command_argument_value("C:/repo/$hidden")));
    assert!(rendered.contains(&quote_command_argument_value("src/quoted'file.rs")));
    assert!(!rendered.contains("{program}"));
}

#[test]
fn stdio_metadata_lists_tools_resources_and_prompts() {
    let tools = stdio_tools_list_json();
    let tool_names = tools["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert!(tool_names.contains(&"definition"));
    assert!(tool_names.contains(&"references"));

    let resources = stdio_resources_list_json();
    assert!(
        resources["result"]["resources"]
            .as_array()
            .expect("resources")
            .iter()
            .any(|resource| resource["uri"] == "codestory://agent-guide")
    );
    assert_eq!(
        resources["result"]["resources"]
            .as_array()
            .expect("resources")
            .len(),
        1,
        "only static project-free resources belong in resources/list"
    );

    let templates = stdio_resource_templates_list_json();
    assert!(
        templates["result"]["resourceTemplates"]
            .as_array()
            .expect("resource templates")
            .iter()
            .any(|resource| { resource["uriTemplate"] == "codestory://grounding{?project}" })
    );

    let prompts = stdio_prompts_list_json();
    assert!(
        prompts["result"]["prompts"]
            .as_array()
            .expect("prompts")
            .iter()
            .any(|prompt| prompt["name"] == "impact_analysis")
    );
}

#[test]
fn index_watch_rejects_output_file_inside_project_tree() {
    let temp = tempdir().expect("create temp dir");
    let project = temp.path().join("repo");
    fs::create_dir_all(&project).expect("create project");
    let cmd = IndexCommand {
        project: args::ProjectArgs {
            project: project.clone(),
            cache_dir: None,
        },
        refresh: args::RefreshMode::Auto,
        format: args::OutputFormat::Markdown,
        output_file: Some(project.join("index.md")),
        dry_run: false,
        summarize: false,
        progress: false,
        watch: true,
    };

    let error =
        validate_index_watch_output_file(&cmd).expect_err("in-tree output should be rejected");

    assert!(
        error
            .to_string()
            .contains("--watch cannot write --output-file inside the watched project"),
        "{error:#}"
    );
}

#[test]
fn non_trail_commands_reject_dot_format_before_running() {
    let error =
        ensure_dot_only_for_trail(args::OutputFormat::Dot, "search").expect_err("reject dot");

    assert!(
        error
            .to_string()
            .contains("--format dot is only supported by `trail`"),
        "{error:#}"
    );
}

#[test]
fn hide_speculative_trail_edges_prunes_disconnected_nodes() {
    let context = TrailContextDto {
        focus: sample_node_details("a", "A"),
        trail: GraphResponse {
            center_id: NodeId("a".to_string()),
            nodes: vec![
                sample_graph_node("a", "A"),
                sample_graph_node("b", "B"),
                sample_graph_node("c", "C"),
                sample_graph_node("d", "D"),
            ],
            edges: vec![
                sample_graph_edge("e1", "a", "b", Some("certain")),
                sample_graph_edge("e2", "b", "c", Some("uncertain")),
                sample_graph_edge("e3", "c", "d", Some("certain")),
            ],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        },
        story: None,
    };

    let filtered = hide_speculative_trail_edges(context);
    let node_ids = filtered
        .trail
        .nodes
        .iter()
        .map(|node| node.id.0.as_str())
        .collect::<Vec<_>>();
    let edge_ids = filtered
        .trail
        .edges
        .iter()
        .map(|edge| edge.id.0.as_str())
        .collect::<Vec<_>>();

    assert_eq!(node_ids, vec!["a", "b"]);
    assert_eq!(edge_ids, vec!["e1"]);
    assert_eq!(filtered.trail.omitted_edge_count, 2);
}

#[test]
fn default_cache_root_uses_workspace_identity() {
    let root = Path::new("C:/repo");
    let cache_root = cache_root_for_project(root, None).expect("cache root");
    let cache_root = cache_root.to_string_lossy();
    assert!(
        cache_root.ends_with(&codestory_workspace::workspace_id_v3_for_root(root)),
        "default cache root should end with the workspace identity"
    );
}

fn sample_runtime_hit(
    id: &str,
    display_name: &str,
    kind: NodeKind,
    file_path: &Path,
    line: u32,
) -> SearchHit {
    SearchHit {
        node_id: NodeId(id.to_string()),
        display_name: display_name.to_string(),
        kind,
        file_path: Some(file_path.to_string_lossy().to_string()),
        line: Some(line),
        score: 1.0,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        score_breakdown: None,
        ..test_search_hit_defaults()
    }
}

#[test]
fn function_body_promotion_keeps_non_callable_selected_anchor() {
    let temp = tempdir().expect("create temp dir");
    let source_path = temp.path().join("posts.tsx");
    fs::write(
            &source_path,
            "export const Posts = { slug: \"posts\" };\n\nexport function PostsIndexPage() {\n  return \"posts\";\n}\n",
        )
        .expect("write source");

    let selected = sample_runtime_hit("posts", "Posts", NodeKind::CLASS, &source_path, 1);
    let alternative = sample_runtime_hit(
        "page",
        "PostsIndexPage",
        NodeKind::FUNCTION,
        &source_path,
        3,
    );
    let target = runtime::ResolvedTarget {
        selector: QuerySelectorOutput::Query,
        requested: "Posts".to_string(),
        file_filter: None,
        selected,
        alternatives: vec![alternative],
    };

    let promoted = runtime::prefer_function_body_target(temp.path(), target);

    assert_eq!(promoted.selected.node_id.0, "posts");
    assert_eq!(promoted.selected.display_name, "Posts");
}

#[test]
fn function_body_promotion_keeps_same_callable_implementation() {
    let temp = tempdir().expect("create temp dir");
    let declaration_path = temp.path().join("Project.h");
    let implementation_path = temp.path().join("Project.cpp");
    fs::write(&declaration_path, "void Project::buildIndex();\n").expect("write declaration");
    fs::write(
        &implementation_path,
        "void Project::buildIndex()\n{\n    runIndexer();\n}\n",
    )
    .expect("write implementation");

    let selected = sample_runtime_hit(
        "declaration",
        "Project::buildIndex",
        NodeKind::METHOD,
        &declaration_path,
        1,
    );
    let alternative = sample_runtime_hit(
        "implementation",
        "Project::buildIndex",
        NodeKind::FUNCTION,
        &implementation_path,
        1,
    );
    let target = runtime::ResolvedTarget {
        selector: QuerySelectorOutput::Query,
        requested: "Project::buildIndex".to_string(),
        file_filter: None,
        selected,
        alternatives: vec![alternative],
    };

    let promoted = runtime::prefer_function_body_target(temp.path(), target);

    assert_eq!(promoted.selected.node_id.0, "implementation");
}

#[test]
fn clean_path_unix_noop() {
    assert_eq!(clean_path_string("src/lib.rs"), "src/lib.rs");
}

#[test]
fn clean_path_backslash_normalization() {
    assert_eq!(clean_path_string("C:\\foo\\bar"), "C:/foo/bar");
}

#[test]
fn clean_path_extended_prefix_stripped() {
    assert_eq!(clean_path_string("\\\\?\\C:\\foo\\bar"), "C:/foo/bar");
}

#[test]
fn clean_path_extended_prefix_unc() {
    assert_eq!(
        clean_path_string("\\\\?\\UNC\\server\\share"),
        "//server/share"
    );
}

#[test]
fn relative_path_strips_root() {
    let root = Path::new("C:/repo");
    assert_eq!(relative_path(root, "C:/repo/src/lib.rs"), "src/lib.rs");
}

#[test]
fn relative_path_outside_root() {
    let root = Path::new("C:/repo");
    assert_eq!(
        relative_path(root, "D:\\other\\file.rs"),
        "D:/other/file.rs"
    );
}

#[test]
fn relative_path_extended_prefix_unc_keeps_share_format() {
    let root = Path::new("C:/repo");
    assert_eq!(
        relative_path(root, "\\\\?\\UNC\\server\\share\\file.rs"),
        "//server/share/file.rs"
    );
}

#[test]
fn embedding_preflight_preserves_typed_capacity_for_json_failures() {
    let error = anyhow::Error::new(codestory_retrieval::PerUserEmbeddingError {
        code: "embedding_capacity".into(),
        message: "query queue is full".into(),
        retry_class: "after_capacity_change".into(),
        retry_after_ms: 25,
        retry_condition: "a query slot becomes available".into(),
        capacity: Some(codestory_retrieval::EmbeddingCapacityPressureWire {
            reason: "queue_full".into(),
            queue_class: "query".into(),
            capacity: 64,
            depth: 64,
            retry_after_ms: 25,
            retry_condition: "a query slot becomes available".into(),
            owner_state: "ready".into(),
            active_scope_id: None,
            active_request_id: None,
            active_request_class: None,
        }),
    });

    let mapped = map_embedding_preflight_error(error);
    let api = runtime::api_error_in_chain(&mapped).expect("typed CLI API error");
    assert_eq!(api.code, "embedding_capacity");
    assert_eq!(
        api.details
            .as_deref()
            .and_then(|details| details.embedding_capacity.as_ref())
            .map(|pressure| pressure.retry_condition.as_str()),
        Some("a query slot becomes available")
    );
}

#[test]
fn cli_sources_do_not_depend_on_index_or_storage_layers_directly() {
    let src_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let forbidden = [
        ["codestory_", "index::"].concat(),
        ["codestory_", "storage::"].concat(),
        ["codestory_", "project::"].concat(),
    ];

    for entry in fs::read_dir(src_dir).expect("read cli src dir") {
        let entry = entry.expect("src entry");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }

        let contents = fs::read_to_string(&path).expect("read source");
        for needle in &forbidden {
            assert!(
                !contents.contains(needle),
                "CLI source {} should not depend directly on {needle}",
                path.display()
            );
        }
    }
}
