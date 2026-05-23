use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;
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

fn write_payload_collection_consumer_fixture(root: &Path) {
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src dir");
    let scripts = root.join("scripts");
    fs::create_dir_all(&scripts).expect("create scripts dir");
    fs::write(
        src.join("Posts.ts"),
        r#"
import type { CollectionConfig } from "payload";

export const Posts: CollectionConfig = {
  slug: "posts",
};

export function renderPublicWritingSurface() {
  payload.find({
    collection: "posts",
  });
  return [];
}
"#,
    )
    .expect("write payload fixture");
    fs::write(
        scripts.join("import-posts.ts"),
        r#"
export async function importPosts(payload: any) {
  return payload.find({
    collection: "posts",
  });
}
"#,
    )
    .expect("write payload script fixture");
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command.args(args);
    command.arg("--project").arg(workspace);
    command.env("CODESTORY_HYBRID_RETRIEVAL_ENABLED", "true");
    command.env("CODESTORY_EMBED_RUNTIME_MODE", "hash");
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
fn search_json_emits_search_results_dto_after_repo_text_merge() {
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
    assert_eq!(json["repo_text_enabled"], Value::Bool(true));
    assert!(
        json["retrieval"].is_object(),
        "search json should include retrieval metadata"
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
        "search json should include repo-text hits"
    );
    assert_eq!(
        json["repo_text_hits"].as_array().map(Vec::len),
        Some(1),
        "repo-text hits should respect the per-source limit"
    );
    assert!(
        json["repo_text_stats"].is_object(),
        "search json should include repo-text scan cap telemetry"
    );
    assert_eq!(
        json["repo_text_stats"]["file_cap"].as_u64(),
        Some(2000),
        "repo-text scan stats should surface the configured file cap"
    );
    assert_eq!(json["limit_per_source"], Value::from(1));
}

#[test]
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
    assert!(channels.contains(&"repo_text"), "{plan:#}");
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
        plan["next_commands"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "search plan should provide next commands: {plan:#}"
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
                item["anchor"] == "WorkspaceIndexer.run"
                    && item["promotion_status"] == "typed_anchor"
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
fn drill_question_search_artifact_keeps_broad_plan_partial() {
    let workspace = tempdir().expect("workspace dir");
    write_search_quality_fixture(workspace.path());
    let output_dir = tempdir().expect("drill output dir");

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
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
        "drill command failed: {}",
        String::from_utf8_lossy(&drill.stderr)
    );
    let question_search_path = output_dir.path().join("question-search.json");
    let question_search: Value = serde_json::from_slice(
        &fs::read(&question_search_path).expect("read question-search artifact"),
    )
    .expect("parse question-search json");
    assert!(
        question_search["search_plan"].is_object(),
        "question-search artifact should include the same broad search plan: {question_search:#}"
    );
    let report_path = output_dir.path().join("drill-report.json");
    let report: Value = serde_json::from_slice(&fs::read(&report_path).expect("read drill report"))
        .expect("parse drill report");
    let summary_path = output_dir.path().join("drill-summary.json");
    let summary: Value =
        serde_json::from_slice(&fs::read(&summary_path).expect("read drill summary"))
            .expect("parse drill summary");
    assert_eq!(summary["summary_version"], 1);
    assert_eq!(summary["full_report_json"], "drill-report.json");
    assert_eq!(summary["mechanical"]["refresh"], "none");
    assert_eq!(summary["mechanical"]["index_ready"], true);
    assert_eq!(summary["anchors"]["requested"], 1);
    assert_eq!(summary["anchors"]["resolved"], 1);
    assert_eq!(summary["anchors"]["unresolved"], 0);
    assert_eq!(
        summary["anchors"]["statuses"][0]["anchor"],
        "WorkspaceIndexer"
    );
    assert_eq!(summary["anchors"]["statuses"][0]["status"], "resolved");
    assert_eq!(summary["bridges"]["total"], 0);
    assert_eq!(summary["source_truth"]["required"], true);
    assert!(
        summary["source_truth"]["target_files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|file| file
                .as_str()
                .is_some_and(|path| path.replace('\\', "/").ends_with("src/architecture.ts")))),
        "drill summary should expose compact source-truth target files: {summary:#}"
    );
    assert_eq!(summary["open_gaps"]["overall_status"], "partial");
    assert_eq!(summary["open_gaps"]["open_gap_friendly"], true);
    assert_eq!(summary["open_gaps"]["status"], "open_gaps_explicit");
    assert_eq!(report["question_search"]["status"], "ok");
    let evidence_items = report["evidence_packet"]["items"]
        .as_array()
        .expect("evidence packet items");
    let question_item = evidence_items
        .iter()
        .find(|item| item["id"] == "question-search")
        .expect("question-search evidence item");
    assert_eq!(
        question_item["verification_status"], "partial",
        "question search should remain partial discovery evidence: {question_item:#}"
    );
    assert!(
        question_item["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().any(|note| note
                .as_str()
                .is_some_and(|text| text.contains("broad discovery")))),
        "question-search evidence should preserve broad discovery guidance: {question_item:#}"
    );

    let snippet_item = evidence_items
        .iter()
        .find(|item| {
            item["evidence_type"] == "snippet" && item["verification_status"] == "supported"
        })
        .expect("drill evidence should classify snippet artifacts as supported source context");
    assert!(
        snippet_item["notes"].as_array().is_some_and(|notes| {
            notes.iter().any(|note| {
                note.as_str()
                    .is_some_and(|text| text.contains("source-backed local context"))
            })
        }),
        "snippet evidence should tell agents it is source-backed local context: {snippet_item:#}"
    );

    let trail_item = evidence_items
        .iter()
        .find(|item| item["evidence_type"] == "trail")
        .expect("drill evidence should include trail artifacts");
    assert_eq!(
        trail_item["verification_status"], "partial",
        "trail evidence should remain partial because graph paths can be noisy: {trail_item:#}"
    );
    assert!(
        trail_item["notes"].as_array().is_some_and(|notes| {
            notes.iter().any(|note| {
                note.as_str().is_some_and(|text| {
                    text.contains("trail evidence may omit speculative edges")
                        && text.contains("checked against snippets/source")
                })
            })
        }),
        "trail evidence should carry source-checking/noise guidance: {trail_item:#}"
    );

    let readiness = &report["evidence_packet"]["readiness"];
    assert_eq!(
        readiness["overall_status"], "partial",
        "drill readiness should keep broad/trail architecture evidence partial: {readiness:#}"
    );
    assert!(
        readiness["source_truth_checks"]
            .as_array()
            .is_some_and(|checks| checks.iter().any(|check| {
                check["required"] == true
                    && check["path"].as_str().is_some_and(|path| {
                        path.replace('\\', "/").ends_with("src/architecture.ts")
                    })
                    && check["reason"]
                        .as_str()
                        .is_some_and(|reason| reason.contains("primary source occurrence"))
            })),
        "drill readiness should carry required source-truth checks for selected anchors: {readiness:#}"
    );

    let snippet_artifact = output_dir.path().join("WorkspaceIndexer-snippet.json");
    let snippet: Value =
        serde_json::from_slice(&fs::read(&snippet_artifact).expect("read snippet artifact"))
            .expect("parse snippet artifact");
    assert!(
        snippet["snippet"]["snippet"]
            .as_str()
            .is_some_and(|text| text.contains("export class WorkspaceIndexer")
                && text.contains("return \"indexed\"")),
        "drill snippet artifact should include the selected symbol body, not only its header: {snippet:#}"
    );
}

#[test]
fn drill_summary_exposes_anchor_consumers_and_verdict() {
    let workspace = tempdir().expect("workspace dir");
    write_search_quality_fixture(workspace.path());
    let output_dir = tempdir().expect("drill output dir");

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    let drill = run_cli(
        workspace.path(),
        &[
            "drill",
            "--question",
            "how public writing social surfaces connect comments and elsewhere feed",
            "--anchors",
            "getElsewhereFeed,getCommentAuth",
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
        "drill command failed: {}",
        String::from_utf8_lossy(&drill.stderr)
    );

    let report_path = output_dir.path().join("drill-report.json");
    let report: Value = serde_json::from_slice(&fs::read(&report_path).expect("read drill report"))
        .expect("parse drill report");
    let anchor = &report["anchors"][0];
    assert!(
        anchor["consumer_summary"].is_object(),
        "drill should emit caller/consumer summary for resolved anchors: {anchor:#}"
    );
    assert!(
        anchor["consumer_summary"]["caller_count"]
            .as_u64()
            .is_some_and(|count| count >= 1),
        "caller summary should expose visible production callers: {anchor:#}"
    );
    assert!(
        anchor["consumer_summary"]["callers"]
            .as_array()
            .is_some_and(|callers| callers.iter().any(|caller| caller["name"]
                .as_str()
                .is_some_and(|name| name.contains("renderPublicWritingSurface")))),
        "caller summary should name the source symbol using the anchor: {anchor:#}"
    );
    assert!(
        report["execution_boundaries"]
            .as_array()
            .is_some_and(|boundaries| boundaries.iter().any(|boundary| {
                boundary["command"] == "drill"
                    && boundary["flow"].as_array().is_some_and(|flow| {
                        flow.iter().any(|step| {
                            step.as_str()
                                .is_some_and(|text| text.contains("run_drill_anchor"))
                        })
                    })
            })),
        "drill report should expose CLI/runtime execution boundaries: {report:#}"
    );

    let summary_path = output_dir.path().join("drill-summary.json");
    let summary: Value =
        serde_json::from_slice(&fs::read(&summary_path).expect("read drill summary"))
            .expect("parse drill summary");
    assert!(
        summary["anchors"]["statuses"][0]["caller_count"]
            .as_u64()
            .is_some_and(|count| count >= 1),
        "drill summary should compact caller counts: {summary:#}"
    );
    assert_eq!(
        summary["verdict"]["status"], "degraded",
        "summary should surface ready/degraded/blocked verdict separately from raw counts: {summary:#}"
    );
    assert!(
        summary["verdict"]["next_action"]
            .as_str()
            .is_some_and(
                |text| text.contains("degraded bridge") && text.contains("src/architecture.ts")
            ),
        "degraded verdict should name the bridge/source-truth work instead of giving a generic prompt: {summary:#}"
    );

    let readiness = &report["evidence_packet"]["readiness"];
    assert!(
        readiness["source_truth_checks"]
            .as_array()
            .is_some_and(|checks| checks
                .iter()
                .any(|check| check["reason"].as_str().is_some_and(
                    |reason| reason.contains("bridge getElsewhereFeed -> getCommentAuth")
                ))),
        "source-truth checks should include bridge endpoints/shared files for degraded bridges: {readiness:#}"
    );
    assert!(
        readiness["source_truth_checks"]
            .as_array()
            .is_some_and(|checks| checks.len() <= 2
                && checks
                    .iter()
                    .any(|check| check["reason"]
                        .as_str()
                        .is_some_and(|reason| reason.contains("consumer")
                            && reason.contains("plus")
                            && reason.contains("more signals")))),
        "source-truth checks should group repeated visible caller/consumer evidence without dropping the role: {readiness:#}"
    );
    assert!(
        readiness["next_commands"]
            .as_array()
            .is_some_and(|commands| commands.iter().any(|command| command
                .as_str()
                .is_some_and(|text| text.contains("getElsewhereFeed getCommentAuth")
                    && text.contains("--repo-text on")))),
        "next commands should include a bridge-specific follow-up search: {readiness:#}"
    );

    let bridge = &report["bridges"][0]["evidence"];
    assert!(
        bridge["endpoint_files"].as_array().is_some_and(|files| {
            files.iter().any(|file| {
                file.as_str()
                    .is_some_and(|path| path.replace('\\', "/").ends_with("src/architecture.ts"))
            })
        }),
        "bridge rows should name endpoint definition files directly, not only hint files: {bridge:#}"
    );
    assert!(
        bridge["evidence_files"].as_array().is_some_and(|files| {
            files.iter().any(|file| {
                file.as_str()
                    .is_some_and(|path| path.replace('\\', "/").ends_with("src/architecture.ts"))
            })
        }),
        "degraded bridge rows should preserve ranked consumer/text evidence files: {bridge:#}"
    );

    let report_markdown =
        fs::read_to_string(output_dir.path().join("drill-report.md")).expect("read drill markdown");
    assert!(
        report_markdown.contains("endpoint_files:"),
        "full drill markdown should expose bridge endpoint files: {report_markdown}"
    );
    assert!(
        report_markdown.contains("evidence_files:"),
        "full drill markdown should expose the same bridge evidence files as JSON: {report_markdown}"
    );
}

#[test]
fn drill_summary_surfaces_stale_freshness_and_refresh_followups() {
    let workspace = tempdir().expect("workspace dir");
    write_search_quality_fixture(workspace.path());
    let output_dir = tempdir().expect("drill output dir");

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    fs::write(
        workspace.path().join("src").join("architecture.ts"),
        r#"
export class WorkspaceIndexer {
  run() { return "changed after index"; }
}
"#,
    )
    .expect("make index stale");

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
        "drill command failed: {}",
        String::from_utf8_lossy(&drill.stderr)
    );

    let summary_path = output_dir.path().join("drill-summary.json");
    let summary: Value =
        serde_json::from_slice(&fs::read(&summary_path).expect("read drill summary"))
            .expect("parse drill summary");
    assert_eq!(summary["mechanical"]["freshness_status"], "stale");
    assert!(
        summary["mechanical"]["stale_file_count"]
            .as_u64()
            .is_some_and(|count| count >= 1),
        "stale summary should count changed files: {summary:#}"
    );
    assert!(
        summary["verdict"]["next_action"]
            .as_str()
            .is_some_and(|text| text.contains("refresh stale index evidence first")),
        "stale freshness should be a visible verdict action: {summary:#}"
    );

    let report_path = output_dir.path().join("drill-report.json");
    let report: Value = serde_json::from_slice(&fs::read(&report_path).expect("read drill report"))
        .expect("parse drill report");
    let next_commands = report["evidence_packet"]["readiness"]["next_commands"]
        .as_array()
        .expect("next commands");
    assert!(
        next_commands.iter().any(|command| command
            .as_str()
            .is_some_and(|text| text.contains("codestory-cli index")
                && text.contains("--refresh incremental"))),
        "stale drill should emit an index refresh follow-up: {report:#}"
    );
    assert!(
        next_commands.iter().any(|command| command
            .as_str()
            .is_some_and(|text| text.contains("codestory-cli search")
                && text.contains("--refresh incremental"))),
        "stale drill should stop recommending refresh none follow-up searches: {report:#}"
    );
    assert!(
        !next_commands.iter().any(|command| command
            .as_str()
            .is_some_and(|text| text.contains("--refresh none"))),
        "stale drill readiness should not leave stale --refresh none follow-ups beside refresh-first commands: {report:#}"
    );

    let report_markdown =
        fs::read_to_string(output_dir.path().join("drill-report.md")).expect("read drill markdown");
    assert!(
        report_markdown
            .to_ascii_lowercase()
            .contains("freshness: stale"),
        "stale freshness should appear in the user-facing markdown report: {report_markdown}"
    );
    assert!(
        report_markdown.contains("codestory-cli index")
            && report_markdown.contains("--refresh incremental"),
        "markdown readiness should advertise the refresh-first follow-up: {report_markdown}"
    );
    assert!(
        !report_markdown.contains("--refresh none"),
        "stale markdown readiness should not advertise --refresh none follow-ups: {report_markdown}"
    );
}

#[test]
fn drill_summary_exposes_related_payload_collection_consumers() {
    let workspace = tempdir().expect("workspace dir");
    write_payload_collection_consumer_fixture(workspace.path());
    let output_dir = tempdir().expect("drill output dir");

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
    );

    let drill = run_cli(
        workspace.path(),
        &[
            "drill",
            "--question",
            "how posts connect to payload collection consumers",
            "--anchors",
            "Posts",
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
        "drill command failed: {}",
        String::from_utf8_lossy(&drill.stderr)
    );

    let report_path = output_dir.path().join("drill-report.json");
    let report: Value = serde_json::from_slice(&fs::read(&report_path).expect("read drill report"))
        .expect("parse drill report");
    let anchor = &report["anchors"][0];
    assert!(
        anchor["consumer_summary"]["consumer_count"]
            .as_u64()
            .is_some_and(|count| count >= 1),
        "drill should include related Payload collection consumers for config anchors: {anchor:#}"
    );
    let consumers = anchor["consumer_summary"]["consumers"]
        .as_array()
        .expect("consumer rows");
    assert!(
        consumers.first().is_some_and(|consumer| {
            consumer["file_path"]
                .as_str()
                .is_some_and(|path| path.replace('\\', "/").starts_with("src/"))
        }),
        "production/source consumers should be listed before scripts in consumer summaries: {anchor:#}"
    );
    assert!(
        consumers.iter().any(|consumer| {
            consumer["file_path"]
                .as_str()
                .is_some_and(|path| path.replace('\\', "/").starts_with("scripts/"))
        }),
        "fixture should include a lower-ranked script consumer so ordering is meaningful: {anchor:#}"
    );
    assert!(
        consumers.iter().any(|consumer| {
            consumer["name"]
                .as_str()
                .is_some_and(|name| name.contains("renderPublicWritingSurface"))
                && consumer["edge_kind"] == "USAGE"
                && consumer["target_relation"]
                    .as_str()
                    .is_some_and(|relation| relation == "related_payload_collection:posts")
                && consumer["target_file_path"]
                    .as_str()
                    .is_some_and(|path| path.replace('\\', "/").ends_with("src/Posts.ts"))
                && consumer["target_name"]
                    .as_str()
                    .is_some_and(|name| name.contains("payload") && name.contains("posts"))
        }),
        "consumer rows should say who used the related payload collection target: {anchor:#}"
    );
    assert!(
        anchor["consumer_summary"]["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().any(|note| note
                .as_str()
                .is_some_and(|text| text.contains("related_payload_collection:posts")
                    || text.contains("payload collection posts")))),
        "summary notes should disclose the related collection lookup: {anchor:#}"
    );
}

#[test]
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

    let index = run_cli(
        workspace.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index.status.success(),
        "index command failed: {}",
        String::from_utf8_lossy(&index.stderr)
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
        let search = run_cli(
            workspace.path(),
            &[
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
            ],
        );
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
