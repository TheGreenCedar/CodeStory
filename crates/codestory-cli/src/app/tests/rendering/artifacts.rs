use super::super::test_support::sample_agent_answer_with_graph;
use crate::app::artifacts::{CONTEXT_BUNDLE_OUTPUT_BYTE_CAP, write_context_bundle};
use crate::http_transport::search_repo_text_mode_param;
use codestory_contracts::api::{GraphArtifactDto, SearchRepoTextMode};
use std::fs;
use tempfile::tempdir;

#[test]
fn write_context_bundle_caps_disk_artifacts_and_writes_manifest() {
    let temp = tempdir().expect("bundle dir");
    fs::write(
        temp.path().join("big-mermaid.mmd"),
        "stale oversized artifact",
    )
    .expect("write stale artifact");
    fs::write(
        temp.path().join("previously-omitted.mmd"),
        "stale upstream-omitted artifact",
    )
    .expect("write stale upstream-omitted artifact");
    let answer = sample_agent_answer_with_graph(GraphArtifactDto::Mermaid {
        id: "big-mermaid".to_string(),
        title: "Big Mermaid".to_string(),
        diagram: "graph TD".to_string(),
        mermaid_syntax: format!(
            "graph TD\nA[{}]\n",
            "x".repeat(CONTEXT_BUNDLE_OUTPUT_BYTE_CAP + 1024)
        ),
    });
    let output = serde_json::json!({
        "_meta": {
            "codestory_publication": {
                "served_from": "complete_publication",
                "operation": {"operation_id": "public-context", "attempt": 1}
            }
        },
        "target": {"selector": "id", "requested": "big-mermaid"},
        "context": crate::output::context_packet_json(&answer),
    });

    write_context_bundle(temp.path(), &output, &answer.graphs, "short context")
        .expect("write capped bundle");

    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(temp.path().join("bundle_manifest.json"))
            .expect("read bundle manifest"),
    )
    .expect("parse bundle manifest");
    let context_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(temp.path().join("context.json")).expect("read context json"),
    )
    .expect("parse context json");

    assert_eq!(manifest["truncated"], serde_json::Value::Bool(true));
    assert_eq!(
        manifest["omitted_mermaid_artifacts"].as_u64(),
        Some(1),
        "{manifest}"
    );
    assert!(
        manifest["written_bytes_excluding_manifest"]
            .as_u64()
            .is_some_and(|bytes| bytes <= CONTEXT_BUNDLE_OUTPUT_BYTE_CAP as u64),
        "{manifest}"
    );
    assert_eq!(context_json["truncated"], serde_json::Value::Bool(true));
    assert_eq!(
        context_json.pointer("/_meta/codestory_publication/operation/operation_id"),
        Some(&serde_json::json!("public-context"))
    );
    assert!(
        !temp.path().join("big-mermaid.mmd").exists(),
        "oversized Mermaid artifact should be omitted"
    );
    assert!(
        !temp.path().join("previously-omitted.mmd").exists(),
        "stale Mermaid artifacts from prior runs should be removed"
    );
}

#[test]
fn http_search_repo_text_param_accepts_cli_modes() {
    assert_eq!(
        search_repo_text_mode_param("auto"),
        Some(SearchRepoTextMode::Auto)
    );
    assert_eq!(
        search_repo_text_mode_param("off"),
        Some(SearchRepoTextMode::Off)
    );
    assert_eq!(
        search_repo_text_mode_param("0"),
        Some(SearchRepoTextMode::Off)
    );
    assert_eq!(
        search_repo_text_mode_param("on"),
        Some(SearchRepoTextMode::On)
    );
    assert_eq!(search_repo_text_mode_param("bogus"), None);
}
