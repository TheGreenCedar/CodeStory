use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Instant;
use tempfile::tempdir;

fn write_retrieval_fixture(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src dir");
    fs::write(
        src.join("lib.rs"),
        r#"
/// Build a compressed grounding summary for OSS users.
/// Include trust notes and semantic fallback details in the snapshot.
pub fn build_snapshot_digest() -> &'static str {
    "compressed grounding summary for oss users"
}

pub fn exact_symbol_anchor() {}
"#,
    )
    .expect("write fixture source");
}

fn write_search_quality_fixture(root: &Path) {
    write_retrieval_fixture(root);
    let src = root.join("src");
    fs::write(
        src.join("routes.ts"),
        r#"
const app = express();
app.get("/api/users", listUsers);
function listUsers() {
  return [];
}
"#,
    )
    .expect("write route fixture");
    fs::write(
        src.join("architecture.ts"),
        r#"
import type { CollectionConfig } from "payload";

// Project source groups create indexing commands and storage access.
export class SourceGroupCxxCdb {
  getIndexerCommands() { return []; }
}

// Full indexing flows through workspace indexer, search service, trails, and snippets.
export class WorkspaceIndexer {
  run() { return "indexed"; }
}
export class SearchService {
  search() { return []; }
}
export interface TrailResult {
  nodes: string[];
}

// Public writing social surfaces connect posts, comment auth, and elsewhere feed.
export const Posts: CollectionConfig = {
  slug: "posts",
};
export function getElsewhereFeed() {
  return [];
}
export function getCommentAuth() {
  return null;
}
export function renderPublicWritingSurface() {
  payload.find({
    collection: "posts",
  });
  return getElsewhereFeed().concat(getCommentAuth() ? ["comments"] : []);
}
"#,
    )
    .expect("write architecture fixture");
}

fn write_openapi_route_fixture(root: &Path) {
    fs::write(
        root.join("openapi.json"),
        r#"{
  "openapi": "3.1.0",
  "info": { "title": "Route metadata fixture", "version": "1.0.0" },
  "paths": {
    "/api/schema-users/{id}": {
      "get": { "operationId": "getSchemaUser" }
    }
  }
}"#,
    )
    .expect("write openapi route fixture");
}

fn run_cli(workspace: &Path, args: &[&str]) -> std::process::Output {
    let mut command = test_support::cli_command();
    command.args(args);
    command.arg("--project").arg(workspace);
    command.env("CODESTORY_HYBRID_RETRIEVAL_ENABLED", "true");
    command.env_remove("CODESTORY_EMBED_RUNTIME_MODE");
    command.env("CODESTORY_EMBED_BACKEND", "llamacpp");
    command.env("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");
    command.output().expect("run codestory-cli")
}

fn assert_framework_route_metadata(framework: &Value) -> String {
    let route = &framework["symbol"]["node"]["route_endpoint"];
    assert_eq!(route["kind"], "framework_route");
    assert_eq!(route["framework"], "express");
    assert_eq!(route["method"], "GET");
    assert_eq!(route["path"], "/api/users");
    assert_eq!(route["raw_path"], "/api/users");
    assert_eq!(route["confidence"], "heuristic");
    assert_eq!(route["source_convention"], "heuristic");
    assert_eq!(route["handler"]["display_name"], "listUsers");
    assert!(
        route["handler"]["certainty"].is_string(),
        "route handler should expose edge certainty: {route:#}"
    );
    assert_eq!(route["provenance"][0], "framework:express");
    assert!(
        route["provenance"]
            .as_array()
            .expect("route provenance array")
            .iter()
            .any(|entry| entry.as_str() == Some("extraction:ast_indexed")),
        "route provenance should expose AST-indexed extraction: {route:#}"
    );
    assert!(
        route["provenance"]
            .as_array()
            .expect("route provenance array")
            .iter()
            .any(|entry| entry.as_str() == Some("graph:handler_edge")),
        "route provenance should expose handler graph edge: {route:#}"
    );
    framework["symbol"]["node"]["id"]
        .as_str()
        .expect("framework route node id")
        .to_string()
}

fn assert_route_explore_context(explore: &Value) {
    assert_eq!(explore["route_context"]["framework"], "express");
    assert_eq!(
        explore["route_context"]["handler"]["display_name"],
        "listUsers"
    );
}

fn assert_affected_route_evidence(affected: &Value) {
    assert_eq!(affected["matched_files"][0]["path"], "src/routes.ts");
    assert_eq!(affected["unmatched_paths"][0]["path"], "missing/file.ts");
    assert_eq!(
        affected["impacted_routes"][0]["route"]["framework"],
        "express"
    );
    assert_eq!(
        affected["impacted_routes"][0]["route"]["handler"]["display_name"],
        "listUsers"
    );
    assert!(
        affected["impacted_routes"][0]["graph_depth"].is_number()
            && affected["impacted_routes"][0]["reason"].is_string()
            && affected["impacted_routes"][0]["confidence"].is_string(),
        "affected routes should expose graph evidence: {affected:#}"
    );
    assert!(
        affected["impacted_symbols"]
            .as_array()
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item["graph_depth"].is_number()
                        && item["reason"].is_string()
                        && item["confidence"].is_string()
                })
            }),
        "affected symbols should expose graph evidence: {affected:#}"
    );
    assert!(
        affected["blind_spots"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "affected should expose blind spots for unmatched paths: {affected:#}"
    );
    assert!(
        affected["next_commands"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "affected should expose next commands: {affected:#}"
    );
}

fn assert_openapi_route_metadata(openapi: &Value) {
    let route = &openapi["symbol"]["node"]["route_endpoint"];
    assert_eq!(route["kind"], "openapi_endpoint");
    assert_eq!(route["method"], "GET");
    assert_eq!(route["path"], "/api/schema-users/{id}");
    assert_eq!(route["params"], serde_json::json!(["id"]));
    assert_eq!(route["confidence"], "schema");
    assert_eq!(route["source_convention"], "openapi");
    assert_eq!(route["provenance"][0], "openapi");
}

#[test]
fn search_json_fails_closed_without_full_sidecars() {
    let workspace = tempdir().expect("workspace dir");
    write_retrieval_fixture(workspace.path());

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    let search = run_cli(
        workspace.path(),
        &[
            "search",
            "--query",
            "compressed grounding summary for oss users",
            "--limit",
            "1",
            "--refresh",
            "none",
            "--why",
            "--format",
            "json",
        ],
    );
    assert!(
        !search.status.success(),
        "mandatory sidecar search should fail without full sidecars: {}",
        String::from_utf8_lossy(&search.stdout)
    );
    assert!(
        search.stderr.is_empty(),
        "JSON failure must not emit stderr"
    );
    let failure: Value = serde_json::from_slice(&search.stdout).expect("parse failure envelope");
    let failure_text = failure.to_string();
    assert_eq!(failure["schema_version"], 1);
    assert_eq!(failure["error"]["code"], "command_failed");
    assert!(
        failure_text.contains(
            "retrieval_unavailable: sidecar retrieval primary is unavailable or degraded"
        ) && failure_text.contains("expected profile=agent mode=full"),
        "search should report mandatory agent sidecar full-mode boundary: {failure:#}"
    );
    assert!(
        failure_text.contains("Minimum next:")
            && failure_text.contains("Full repair:")
            && failure_text.contains("codestory-cli ready --goal agent --repair")
            && failure_text.contains("codestory-cli retrieval status")
            && failure_text.contains("codestory-cli doctor"),
        "search should include retrieval repair commands: {failure:#}"
    );
}

#[test]
fn search_json_rejects_removed_hybrid_tuning_flags_as_unknown_args() {
    let workspace = tempdir().expect("workspace dir");

    let search = run_cli(
        workspace.path(),
        &[
            "search",
            "--query",
            "compressed grounding summary for oss users",
            "--hybrid-lexical",
            "0.8",
            "--format",
            "json",
        ],
    );
    assert!(
        !search.status.success(),
        "removed hybrid tuning flags should be rejected by CLI parsing: {}",
        String::from_utf8_lossy(&search.stdout)
    );
    assert!(
        search.stderr.is_empty(),
        "JSON failure must not emit stderr"
    );
    let failure: Value = serde_json::from_slice(&search.stdout).expect("parse failure envelope");
    assert_eq!(failure["error"]["code"], "invalid_arguments");
    assert!(
        failure["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("unexpected argument '--hybrid-lexical'")),
        "search should reject removed hybrid tuning flags as unknown args: {failure:#}"
    );
}

#[test]
fn context_rejects_removed_hybrid_tuning_flags_as_unknown_args() {
    let workspace = tempdir().expect("workspace dir");

    let context = run_cli(
        workspace.path(),
        &[
            "context",
            "--query",
            "compressed grounding summary for oss users",
            "--hybrid-semantic",
            "0.8",
            "--format",
            "json",
        ],
    );
    assert!(
        !context.status.success(),
        "removed context hybrid tuning flags should be rejected by CLI parsing: {}",
        String::from_utf8_lossy(&context.stdout)
    );
    assert!(
        context.stderr.is_empty(),
        "JSON failure must not emit stderr"
    );
    let failure: Value = serde_json::from_slice(&context.stdout).expect("parse failure envelope");
    assert_eq!(failure["error"]["code"], "invalid_arguments");
    assert!(
        failure["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("unexpected argument '--hybrid-semantic'")),
        "context should reject removed hybrid tuning flags as unknown args: {failure:#}"
    );
}

#[test]
#[ignore = "live full-sidecar contract; requires Docker/services plus CODESTORY_EMBED_BACKEND=llamacpp real embeddings"]
fn search_json_emits_sidecar_primary_results_without_repo_text_fallback() {
    let workspace = tempdir().expect("workspace dir");
    write_retrieval_fixture(workspace.path());
    let run_id = "search-json-sidecar";

    let bootstrap = run_cli(
        workspace.path(),
        &[
            "retrieval",
            "bootstrap",
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--wait-secs",
            "30",
            "--format",
            "json",
        ],
    );
    assert!(
        bootstrap.status.success(),
        "retrieval bootstrap failed; live full-sidecar fixture is blocked: {}",
        String::from_utf8_lossy(&bootstrap.stderr)
    );

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    let retrieval_index = run_cli(
        workspace.path(),
        &[
            "retrieval",
            "index",
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        retrieval_index.status.success(),
        "retrieval index failed; live full-sidecar fixture is blocked: {}",
        String::from_utf8_lossy(&retrieval_index.stderr)
    );

    let status = run_cli(
        workspace.path(),
        &[
            "retrieval",
            "status",
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--format",
            "json",
        ],
    );
    assert!(
        status.status.success(),
        "retrieval status failed after retrieval index: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status.stdout).expect("parse status json");
    assert_eq!(
        status_json["retrieval_mode"],
        Value::String("full".to_string()),
        "live full-sidecar fixture must report agent retrieval_mode=full: {status_json:#}"
    );
    assert_eq!(
        status_json["ownership"]["profile"],
        Value::String("agent".to_string()),
        "live full-sidecar fixture must prepare the agent sidecar profile: {status_json:#}"
    );

    let search = run_cli(
        workspace.path(),
        &[
            "search",
            "--query",
            "compressed grounding summary for oss users",
            "--limit",
            "1",
            "--refresh",
            "none",
            "--why",
            "--format",
            "json",
        ],
    );
    assert!(
        search.status.success(),
        "search command failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );

    let json: Value = serde_json::from_slice(&search.stdout).expect("parse search json");
    assert_eq!(
        json["query"],
        Value::String("compressed grounding summary for oss users".to_string())
    );
    assert_eq!(json["repo_text_mode"], Value::String("auto".to_string()));
    assert_eq!(json["repo_text_enabled"], Value::Bool(false));
    assert!(
        json["retrieval"].is_object(),
        "search json should include retrieval metadata"
    );
    let shadow = &json["retrieval_shadow"];
    assert_eq!(
        shadow["retrieval_mode"],
        Value::String("full".to_string()),
        "mandatory sidecar search should expose full-mode sidecar diagnostics: {json:#}"
    );
    assert!(
        shadow["stage_timings"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "search --why json should expose sidecar stage timings: {json:#}"
    );
    assert!(
        shadow["candidates"].as_array().is_some_and(|items| {
            !items.is_empty()
                && items.iter().all(|item| {
                    item["source"].is_string()
                        && item["file_path"].is_string()
                        && item["resolution"].is_string()
                })
        }),
        "search --why json should expose sidecar candidate provenance and resolution labels: {json:#}"
    );
    assert!(
        shadow["candidate_count"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "search --why json should count sidecar candidates: {json:#}"
    );
    assert!(
        shadow["resolved_hit_count"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "search --why json should count resolved sidecar hits: {json:#}"
    );
    assert!(
        json["indexed_symbol_hits"].is_array(),
        "search json should include indexed symbol hits"
    );
    assert_eq!(
        json["indexed_symbol_hits"].as_array().map(Vec::len),
        Some(1),
        "search json should preserve the indexed symbol bucket"
    );
    assert_eq!(json["explain"], Value::Bool(true));
    assert!(
        json["indexed_symbol_hits"][0]["score_breakdown"].is_object(),
        "search json should expose hybrid score breakdowns for indexed hits"
    );
    assert!(
        json["indexed_symbol_hits"][0]["match_quality"].is_string(),
        "search json should classify hit match quality"
    );
    assert_eq!(
        json["query_assessment"]["exact_symbol_hit_count"].as_u64(),
        Some(0),
        "natural-language query should not be overstated as an exact symbol hit"
    );
    assert!(
        json["query_assessment"]["recommended_next_action"].is_string(),
        "search json should include a deterministic next-action assessment"
    );
    assert!(
        json["indexed_symbol_hits"][0]["why"].is_array(),
        "search --why json should carry compact explanation strings"
    );
    assert!(
        json["repo_text_hits"].is_array(),
        "search json should preserve the repo-text hits field"
    );
    assert_eq!(
        json["repo_text_hits"].as_array().map(Vec::len),
        Some(0),
        "mandatory sidecar search must not substitute repo-text fallback hits"
    );
    assert!(
        json["repo_text_stats"].is_null(),
        "mandatory sidecar search should not emit repo-text scan telemetry"
    );
    assert_eq!(json["limit_per_source"], Value::from(1));
}

#[test]
#[ignore = "live full-sidecar contract; requires finalized sidecar search-plan evidence"]
fn broad_search_json_and_markdown_expose_search_plan() {
    let workspace = tempdir().expect("workspace dir");
    write_search_quality_fixture(workspace.path());

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    let query = "how full indexing supports search trail and snippet commands";
    let search = run_cli(
        workspace.path(),
        &[
            "search",
            "--query",
            query,
            "--repo-text",
            "on",
            "--why",
            "--plan-details",
            "--format",
            "json",
            "--refresh",
            "none",
        ],
    );
    assert!(
        search.status.success(),
        "search command failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );
    let json: Value = serde_json::from_slice(&search.stdout).expect("parse search json");
    let plan = &json["search_plan"];
    assert!(
        plan.is_object(),
        "search json should expose search_plan: {json:#}"
    );
    assert_eq!(plan["original_query"], query);
    assert_eq!(plan["eligible"], true);
    let extracted = plan["terms"]["extracted"]
        .as_array()
        .expect("extracted terms")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    for expected in ["full", "indexing", "search", "trail", "snippet"] {
        assert!(
            extracted.contains(&expected),
            "search plan should extract `{expected}` from broad query: {plan:#}"
        );
    }
    assert!(
        plan["terms"]["dropped"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "search plan should expose dropped natural-language terms: {plan:#}"
    );
    let subqueries = plan["subqueries"].as_array().expect("subqueries");
    assert!(
        (3..=8).contains(&subqueries.len()),
        "broad query should produce bounded subqueries: {plan:#}"
    );
    let channels = subqueries
        .iter()
        .flat_map(|subquery| subquery["channels"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    assert!(channels.contains(&"typed_symbol"), "{plan:#}");
    assert!(
        channels.contains(&"lexical") || channels.contains(&"semantic"),
        "{plan:#}"
    );
    assert!(
        !channels.contains(&"repo_text"),
        "mandatory sidecar search plans must not use repo-text fallback channels: {plan:#}"
    );
    assert_eq!(
        json["repo_text_enabled"],
        Value::Bool(false),
        "mandatory sidecar search must ignore repo-text serving even when requested: {json:#}"
    );
    assert_eq!(
        json["repo_text_hits"].as_array().map(Vec::len),
        Some(0),
        "mandatory sidecar search plans must not serve repo-text hits: {json:#}"
    );
    assert!(
        json["repo_text_stats"].is_null(),
        "mandatory sidecar search plans must not run repo-text scan telemetry: {json:#}"
    );
    assert!(
        plan["candidate_windows"].as_array().is_some_and(|items| {
            items.iter().all(|item| {
                item["channel"].is_string()
                    && item["subquery"].is_string()
                    && item["limit"].is_number()
                    && item["returned_count"].is_number()
                    && item["truncated"].is_boolean()
            })
        }),
        "candidate windows should expose bounded retrieval state: {plan:#}"
    );
    assert!(
        plan["candidate_windows"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| {
                item["channel"] == "typed_symbol"
                    && item["subquery"]
                        .as_str()
                        .is_some_and(|subquery| subquery != query)
                    && item["returned_count"]
                        .as_u64()
                        .is_some_and(|count| count > 0)
            })),
        "candidate windows should come from executed planned subqueries, not only the original query: {plan:#}"
    );
    assert!(
        plan["candidate_windows"]
            .as_array()
            .is_some_and(|items| items.iter().all(|item| item["channel"] != "repo_text")),
        "mandatory sidecar search plans must not expose repo-text candidate windows: {plan:#}"
    );
    assert!(
        plan["anchor_groups"]
            .as_array()
            .is_some_and(|items| !items.is_empty()
                && items.iter().all(|item| {
                    item["anchor"].is_string()
                        && item["promotion_status"].is_string()
                        && item["confidence"].is_string()
                })),
        "search plan should expose anchor groups: {plan:#}"
    );
    assert!(
        plan.get("next_commands").is_none(),
        "search plan JSON should expose structured next actions, not rendered CLI commands: {plan:#}"
    );
    assert!(
        plan["next_actions"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item["action"] == "snippet"
                    && item["node_id"].is_string()
                    && item["options"].as_array().is_some_and(|options| {
                        options.iter().any(|option| option == "function_body")
                    })
            })
        }),
        "search plan should provide structured next actions for CLI renderers: {plan:#}"
    );
    assert!(
        plan["source_truth_checks"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "search plan should provide source-truth checks: {plan:#}"
    );
    assert!(
        plan["anchor_groups"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item["promotion_status"] == "typed_anchor"
                    && item["confidence"] == "medium"
                    && item["reasons"].as_array().is_some_and(|reasons| {
                        reasons.iter().any(|reason| {
                            reason
                                .as_str()
                                .is_some_and(|text| text.contains("typed indexed symbol"))
                        })
                    })
            })
        }),
        "search plan should expose typed-anchor ranking hints for active investigation paths: {plan:#}"
    );
    assert!(
        plan["bridges"].as_array().is_some_and(|items| {
            items
                .iter()
                .all(|item| item["confidence"].as_str() != Some("low"))
        }),
        "search plan should suppress low-confidence bridge/noise rows by default: {plan:#}"
    );
    assert!(
        plan["source_truth_checks"]
            .as_array()
            .is_some_and(|checks| {
                checks.iter().any(|check| {
                    check.as_str().is_some_and(|text| {
                        text.contains("Suppressed") && text.contains("low-confidence bridge")
                    })
                })
            }),
        "search plan should preserve a source-truth prompt for suppressed low-confidence bridges: {plan:#}"
    );
    assert!(
        plan["rejected_hits"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                matches!(
                    item["display_name"].as_str(),
                    Some("getElsewhereFeed" | "getCommentAuth" | "exact_symbol_anchor")
                ) && item["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("not selected after anchor grouping"))
            })
        }),
        "search plan should preserve rejected-hit reasons for unused exact anchors: {plan:#}"
    );

    let markdown = run_cli(
        workspace.path(),
        &[
            "search",
            "--query",
            query,
            "--repo-text",
            "on",
            "--why",
            "--plan-details",
            "--format",
            "markdown",
            "--refresh",
            "none",
        ],
    );
    assert!(
        markdown.status.success(),
        "markdown search command failed: {}",
        String::from_utf8_lossy(&markdown.stderr)
    );
    let markdown = String::from_utf8(markdown.stdout).expect("markdown utf8");
    for expected in [
        "## Search Plan",
        "Sidecar diagnostics:",
        "Sidecar stages:",
        "Sidecar candidate window:",
        "Subqueries:",
        "Extracted terms:",
        "Repo-text promotions:",
        "Source-truth checks:",
    ] {
        assert!(
            markdown.contains(expected),
            "search markdown should contain `{expected}`:\n{markdown}"
        );
    }
}

#[test]
#[ignore = "live full-sidecar contract; requires finalized sidecar search-plan evidence"]
fn broad_search_json_without_plan_details_does_not_emit_search_plan() {
    let workspace = tempdir().expect("workspace dir");
    write_search_quality_fixture(workspace.path());

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    let search = run_cli(
        workspace.path(),
        &[
            "search",
            "--query",
            "how full indexing supports search trail and snippet commands",
            "--repo-text",
            "on",
            "--why",
            "--format",
            "json",
            "--refresh",
            "none",
        ],
    );
    assert!(
        search.status.success(),
        "search command failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );
    let json: Value = serde_json::from_slice(&search.stdout).expect("parse search json");
    assert!(
        json["search_plan"].is_null(),
        "search should not emit Search Plan unless --why --plan-details is requested: {json:#}"
    );
}

#[test]
#[ignore = "live full-sidecar contract; requires finalized sidecar search-plan evidence"]
fn search_plan_honors_repo_text_off() {
    let workspace = tempdir().expect("workspace dir");
    write_search_quality_fixture(workspace.path());

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    let query = "how full indexing supports search trail and snippet commands";
    let search = run_cli(
        workspace.path(),
        &[
            "search",
            "--query",
            query,
            "--repo-text",
            "off",
            "--why",
            "--plan-details",
            "--format",
            "json",
            "--refresh",
            "none",
        ],
    );
    assert!(
        search.status.success(),
        "search command failed: {}",
        String::from_utf8_lossy(&search.stderr)
    );
    let json: Value = serde_json::from_slice(&search.stdout).expect("parse search json");
    assert_eq!(json["repo_text_mode"], "off");
    assert_eq!(json["repo_text_enabled"], false);
    assert!(
        json["repo_text_hits"]
            .as_array()
            .is_some_and(|hits| hits.is_empty()),
        "repo_text off should not return repo-text hits: {json:#}"
    );

    let plan = &json["search_plan"];
    assert!(
        plan.is_object(),
        "broad search should still expose an index-backed plan: {json:#}"
    );
    let subquery_channels = plan["subqueries"]
        .as_array()
        .expect("subqueries")
        .iter()
        .flat_map(|subquery| subquery["channels"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    assert!(
        !subquery_channels.contains(&"repo_text"),
        "repo_text off should strip repo-text plan channels: {plan:#}"
    );
    assert!(
        plan["candidate_windows"]
            .as_array()
            .expect("candidate windows")
            .iter()
            .all(|window| window["channel"] != "repo_text"),
        "repo_text off should not execute repo-text plan windows: {plan:#}"
    );
}

#[test]
#[ignore = "live full-sidecar contract; requires finalized sidecar ranking evidence"]
fn exact_symbol_queries_preserve_fast_path_and_top_rank() {
    let workspace = tempdir().expect("workspace dir");
    write_search_quality_fixture(workspace.path());

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    for anchor in [
        "WorkspaceIndexer",
        "SearchService",
        "TrailResult",
        "SourceGroupCxxCdb",
        "getCommentAuth",
    ] {
        let search = run_cli(
            workspace.path(),
            &[
                "search",
                "--query",
                anchor,
                "--repo-text",
                "on",
                "--why",
                "--format",
                "json",
                "--refresh",
                "none",
            ],
        );
        assert!(
            search.status.success(),
            "search command failed for {anchor}: {}",
            String::from_utf8_lossy(&search.stderr)
        );
        let json: Value = serde_json::from_slice(&search.stdout).expect("parse search json");
        assert_eq!(
            json["indexed_symbol_hits"][0]["display_name"], anchor,
            "exact query should keep the exact typed symbol first: {json:#}"
        );
        assert_eq!(json["indexed_symbol_hits"][0]["origin"], "indexed_symbol");
        assert!(
            json["query_assessment"]["exact_symbol_hit_count"]
                .as_u64()
                .is_some_and(|count| count >= 1),
            "exact query should report exact symbol hits: {json:#}"
        );
        assert!(
            json["search_plan"].is_null(),
            "exact-symbol fast path should not emit a broad search plan: {json:#}"
        );
    }
}

#[test]
#[ignore = "live full-sidecar contract; requires finalized packet/drill fixture evidence"]
fn drill_report_is_a_packet_backed_adapter() {
    let workspace = tempdir().expect("workspace dir");
    write_search_quality_fixture(workspace.path());
    let output_dir = tempdir().expect("drill output dir");
    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "{}",
        String::from_utf8_lossy(&index.stderr)
    );

    let drill = run_cli(
        workspace.path(),
        &[
            "drill",
            "--question",
            "how full indexing supports search trail and snippet commands",
            "--anchors",
            "WorkspaceIndexer",
            "--output-dir",
            output_dir.path().to_str().expect("output path"),
            "--format",
            "json",
            "--refresh",
            "none",
        ],
    );
    assert!(
        drill.status.success(),
        "{}",
        String::from_utf8_lossy(&drill.stderr)
    );

    assert!(!output_dir.path().join("question-search.json").exists());
    let report: Value = serde_json::from_slice(
        &fs::read(output_dir.path().join("drill-report.json")).expect("read drill report"),
    )
    .expect("parse drill report");
    let summary: Value = serde_json::from_slice(
        &fs::read(output_dir.path().join("drill-summary.json")).expect("read drill summary"),
    )
    .expect("parse drill summary");

    assert!(report["evidence_packet"]["packet_id"].is_string());
    assert_eq!(report["question_search"]["command"], "packet");
    assert_eq!(
        report["question_search"]["status"],
        report["evidence_packet"]["sufficiency"]["status"]
    );
    assert!(
        report["evidence_packet"]["plan"]["queries"]
            .as_array()
            .is_some_and(|queries| queries
                .iter()
                .any(|query| query["query"] == "WorkspaceIndexer"))
    );
    assert_eq!(
        report["anchors"][0]["commands"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(summary["summary_version"], 1);
    assert_eq!(summary["anchors"]["requested"], 1);
    assert!(output_dir.path().join("drill-report.md").is_file());
}

#[test]
#[ignore = "live full-sidecar contract; requires finalized symbol/route fixture evidence"]
fn symbol_json_exposes_typed_route_endpoint_metadata() {
    let workspace = tempdir().expect("workspace dir");
    write_search_quality_fixture(workspace.path());
    write_openapi_route_fixture(workspace.path());

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    let framework = run_cli(
        workspace.path(),
        &[
            "symbol",
            "--query",
            "/api/users",
            "--file",
            "src/routes.ts",
            "--choose",
            "1",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        framework.status.success(),
        "framework symbol command failed: {}",
        String::from_utf8_lossy(&framework.stderr)
    );
    let framework: Value =
        serde_json::from_slice(&framework.stdout).expect("parse framework symbol json");
    let route_node_id = assert_framework_route_metadata(&framework);
    let explore = run_cli(
        workspace.path(),
        &[
            "explore",
            "--id",
            &route_node_id,
            "--no-tui",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        explore.status.success(),
        "explore route command failed: {}",
        String::from_utf8_lossy(&explore.stderr)
    );
    let explore: Value = serde_json::from_slice(&explore.stdout).expect("parse explore json");
    assert_route_explore_context(&explore);
    let affected = run_cli(
        workspace.path(),
        &[
            "affected",
            "src/routes.ts",
            "missing/file.ts",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        affected.status.success(),
        "affected route command failed: {}",
        String::from_utf8_lossy(&affected.stderr)
    );
    let affected: Value = serde_json::from_slice(&affected.stdout).expect("parse affected json");
    assert_affected_route_evidence(&affected);

    let openapi = run_cli(
        workspace.path(),
        &[
            "symbol",
            "--query",
            "/api/schema-users/{id}",
            "--file",
            "openapi.json",
            "--choose",
            "1",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        openapi.status.success(),
        "OpenAPI symbol command failed: {}",
        String::from_utf8_lossy(&openapi.stderr)
    );
    let openapi: Value = serde_json::from_slice(&openapi.stdout).expect("parse openapi json");
    assert_openapi_route_metadata(&openapi);
}

#[test]
#[ignore = "live full-sidecar contract; requires finalized sidecar field-filtered search evidence"]
fn field_qualified_search_filters_kind_path_name_and_language() {
    let workspace = tempdir().expect("workspace dir");
    write_search_quality_fixture(workspace.path());

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    let cases = [
        (
            "kind:function name:listUsers",
            "listUsers",
            Some("FUNCTION"),
            Some("src/routes.ts"),
        ),
        (
            "path:routes.ts /api/users",
            "GET /api/users",
            None,
            Some("src/routes.ts"),
        ),
        ("lang:typescript /api/users", "GET /api/users", None, None),
    ];

    for (query, expected_name, expected_kind, expected_path) in cases {
        let search = run_cli(
            workspace.path(),
            &[
                "search",
                "--query",
                query,
                "--limit",
                "5",
                "--repo-text",
                "off",
                "--refresh",
                "none",
                "--format",
                "json",
            ],
        );
        assert!(
            search.status.success(),
            "search command failed for {query}: {}",
            String::from_utf8_lossy(&search.stderr)
        );
        let json: Value = serde_json::from_slice(&search.stdout).expect("parse search json");
        assert_eq!(json["query"], Value::String(query.to_string()));
        let hits = json["indexed_symbol_hits"]
            .as_array()
            .expect("indexed_symbol_hits");
        assert!(
            !hits.is_empty(),
            "field-qualified search should keep matching indexed hits for {query}: {json:#}"
        );
        assert!(
            hits.iter().any(|hit| {
                hit["display_name"]
                    .as_str()
                    .is_some_and(|name| name.contains(expected_name))
            }),
            "expected {expected_name} in filtered hits for {query}: {json:#}"
        );
        if let Some(kind) = expected_kind {
            assert!(
                hits.iter()
                    .all(|hit| hit["kind"].as_str().is_some_and(|value| value == kind)),
                "kind filter should remove non-{kind} hits for {query}: {json:#}"
            );
        }
        if let Some(path) = expected_path {
            assert!(
                hits.iter().all(|hit| hit["file_path"]
                    .as_str()
                    .is_some_and(|value| value.ends_with(path))),
                "path/language filter should remove unrelated paths for {query}: {json:#}"
            );
        }
    }
}

#[test]
#[ignore = "search-quality eval harness; run explicitly after changing search ranking or route indexing"]
fn search_quality_eval_reports_recall_mrr_and_latency_for_symbols_and_routes() {
    let workspace = tempdir().expect("workspace dir");
    write_search_quality_fixture(workspace.path());
    let run_id = "search-quality-eval";

    let bootstrap = run_cli(
        workspace.path(),
        &[
            "retrieval",
            "bootstrap",
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--wait-secs",
            "30",
            "--format",
            "json",
        ],
    );
    assert!(
        bootstrap.status.success(),
        "retrieval bootstrap failed; search quality eval fixture is blocked: {}",
        String::from_utf8_lossy(&bootstrap.stderr)
    );

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );
    let retrieval_index = run_cli(
        workspace.path(),
        &[
            "retrieval",
            "index",
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--refresh",
            "full",
            "--format",
            "json",
        ],
    );
    assert!(
        retrieval_index.status.success(),
        "retrieval index command failed: {}",
        String::from_utf8_lossy(&retrieval_index.stderr)
    );
    let status = run_cli(
        workspace.path(),
        &[
            "retrieval",
            "status",
            "--profile",
            "agent",
            "--run-id",
            run_id,
            "--format",
            "json",
        ],
    );
    assert!(
        status.status.success(),
        "agent retrieval status failed after retrieval index: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status.stdout).expect("parse status json");
    assert_eq!(
        status_json["retrieval_mode"],
        Value::String("full".to_string()),
        "search quality eval must report agent retrieval_mode=full before scoring: {status_json:#}"
    );
    assert_eq!(
        status_json["ownership"]["profile"],
        Value::String("agent".to_string()),
        "search quality eval must prepare the agent sidecar profile before scoring: {status_json:#}"
    );

    let expectations = [
        ("exact_symbol_anchor", "exact_symbol_anchor", "off"),
        ("build snapshot digest", "build_snapshot_digest", "off"),
        (
            "kind:function build snapshot digest",
            "build_snapshot_digest",
            "off",
        ),
        ("/api/users", "GET /api/users", "off"),
        ("path:routes.ts /api/users", "GET /api/users", "off"),
        ("lang:typescript /api/users", "GET /api/users", "off"),
        ("kind:function name:listUsers", "listUsers", "off"),
        (
            "compressed grounding summary for oss users",
            "build_snapshot_digest",
            "on",
        ),
        (
            "how project source groups create indexing commands and storage access",
            "SourceGroupCxxCdb",
            "on",
        ),
        (
            "how full indexing supports search trail and snippet commands",
            "WorkspaceIndexer",
            "on",
        ),
        (
            "how posts comments auth and elsewhere feed connect to public pages",
            "getElsewhereFeed",
            "on",
        ),
    ];
    let mut found = 0_u32;
    let mut reciprocal_rank_sum = 0.0_f64;
    let mut latency_ms = Vec::new();
    let mut anchor_buckets = BTreeMap::<String, u32>::new();
    let mut planned_broad_queries = 0_u32;

    for (query, expected, repo_text) in expectations {
        let started = Instant::now();
        let mut args = vec![
            "search",
            "--query",
            query,
            "--limit",
            "5",
            "--repo-text",
            repo_text,
            "--refresh",
            "none",
            "--format",
            "json",
        ];
        if query.starts_with("how ") {
            args.push("--why");
            args.push("--plan-details");
        }
        let search = run_cli(workspace.path(), &args);
        latency_ms.push(started.elapsed().as_millis() as u64);
        assert!(
            search.status.success(),
            "search command failed for {query}: {}",
            String::from_utf8_lossy(&search.stderr)
        );
        let json: Value = serde_json::from_slice(&search.stdout).expect("parse search json");
        let indexed_hits = json["indexed_symbol_hits"]
            .as_array()
            .expect("indexed_symbol_hits");
        let repo_text_hits = json["repo_text_hits"].as_array().expect("repo_text_hits");
        let indexed_position = indexed_hits.iter().position(|hit| {
            hit["display_name"]
                .as_str()
                .is_some_and(|name| name.contains(expected))
        });
        let repo_text_position = repo_text_hits.iter().position(|hit| {
            hit["display_name"]
                .as_str()
                .is_some_and(|name| name.contains(expected))
                || hit["file_path"]
                    .as_str()
                    .is_some_and(|path| path.contains("lib.rs"))
        });
        for hit in repo_text_hits {
            if let Some(why) = hit["why"].as_array() {
                let why_text = why
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(
                    !why_text.contains(
                        "matched repository text directly; this hit is evidence but not a resolvable symbol"
                    ),
                    "repo-text explanations should not present text hits as evidence: {hit:#}"
                );
                if !why_text.is_empty() {
                    assert!(
                        why_text.contains("repo-text diagnostic match"),
                        "repo-text explanations should be diagnostic/navigation hints: {hit:#}"
                    );
                }
            }
        }
        let anchor_bucket = match (indexed_position, repo_text_position) {
            (Some(_), Some(_)) => "both",
            (Some(_), None) => "indexed_symbol_hits",
            (None, Some(_)) => "repo_text_hits",
            (None, None) => "missing",
        };
        *anchor_buckets.entry(anchor_bucket.to_string()).or_default() += 1;
        if query.starts_with("how ") {
            let plan = &json["search_plan"];
            assert!(
                plan.is_object(),
                "broad architecture query should expose a search plan: {json:#}"
            );
            let planned_anchor = plan["anchor_groups"]
                .as_array()
                .expect("anchor groups")
                .iter()
                .any(|group| {
                    group["anchor"]
                        .as_str()
                        .is_some_and(|anchor| anchor.contains(expected))
                        || group["chosen_symbol"]["display_name"]
                            .as_str()
                            .is_some_and(|name| name.contains(expected))
                });
            assert!(
                planned_anchor || indexed_position.is_some() || repo_text_position.is_some(),
                "broad architecture query should find expected anchor through hits or plan: {json:#}"
            );
            assert!(
                plan["anchor_groups"]
                    .as_array()
                    .expect("anchor groups")
                    .iter()
                    .all(|group| {
                        !matches!(
                            group["promotion_status"].as_str(),
                            Some("needs_source_read" | "ambiguous")
                        ) || group["confidence"] != "high"
                    }),
                "unpromoted repo-text leads must not become high confidence: {json:#}"
            );
            planned_broad_queries += 1;
        }
        if let Some(position) = indexed_position.or(repo_text_position) {
            found += 1;
            reciprocal_rank_sum += 1.0 / (position as f64 + 1.0);
        }
    }
    let noisy = run_cli(
        workspace.path(),
        &[
            "search",
            "--query",
            "nonexistent noisy payment webhook route qxz",
            "--limit",
            "5",
            "--repo-text",
            "off",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        noisy.status.success(),
        "noisy search command failed: {}",
        String::from_utf8_lossy(&noisy.stderr)
    );
    let noisy: Value = serde_json::from_slice(&noisy.stdout).expect("parse noisy search json");
    let noisy_exact_hits = noisy["indexed_symbol_hits"]
        .as_array()
        .expect("noisy indexed hits")
        .iter()
        .filter(|hit| hit["match_quality"] == "exact")
        .count();
    assert_eq!(
        noisy_exact_hits, 0,
        "negative/noisy query should not report exact anchors: {noisy:#}"
    );

    let recall = found as f64 / expectations.len() as f64;
    let mrr = reciprocal_rank_sum / expectations.len() as f64;
    let max_latency_ms = latency_ms.into_iter().max().unwrap_or_default();
    let anchor_bucket_summary = anchor_buckets
        .iter()
        .map(|(bucket, count)| format!("{bucket}={count}"))
        .collect::<Vec<_>>()
        .join(",");
    eprintln!(
        "search_quality_eval recall={recall:.3} mrr={mrr:.3} max_latency_ms={max_latency_ms} anchor_buckets={anchor_bucket_summary}"
    );
    assert_eq!(
        found as usize,
        expectations.len(),
        "expected all eval anchors"
    );
    assert!(
        mrr >= 0.50,
        "expected useful search ordering, got mrr={mrr:.3}"
    );
    assert_eq!(
        planned_broad_queries, 3,
        "expected all broad architecture eval queries to expose search plans"
    );
    assert!(
        max_latency_ms < 3_000,
        "search latency should stay bounded on eval fixture, got {max_latency_ms}ms"
    );
}
mod test_support;
