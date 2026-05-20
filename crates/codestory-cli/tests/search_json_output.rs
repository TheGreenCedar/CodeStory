use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;
use tempfile::tempdir;

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        restore_env_value(self.key, self.previous.as_deref());
    }
}

fn restore_env_value(key: &'static str, previous: Option<&str>) {
    unsafe {
        match previous {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

fn hybrid_cli_env() -> Vec<EnvGuard> {
    vec![
        EnvGuard::set("CODESTORY_HYBRID_RETRIEVAL_ENABLED", "true"),
        EnvGuard::set("CODESTORY_EMBED_RUNTIME_MODE", "hash"),
    ]
}

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
}

fn run_cli(workspace: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command.args(args);
    command.arg("--project").arg(workspace);
    command.env("CODESTORY_HYBRID_RETRIEVAL_ENABLED", "true");
    command.env("CODESTORY_EMBED_RUNTIME_MODE", "hash");
    command.output().expect("run codestory-cli")
}

#[test]
fn search_json_emits_search_results_dto_after_repo_text_merge() {
    let _env = hybrid_cli_env();
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
#[ignore = "search-quality eval harness; run explicitly after changing search ranking or route indexing"]
fn search_quality_eval_reports_recall_mrr_and_latency_for_symbols_and_routes() {
    let _env = hybrid_cli_env();
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
        ("exact_symbol_anchor", "exact_symbol_anchor"),
        ("build snapshot digest", "build_snapshot_digest"),
        ("/api/users", "GET /api/users"),
    ];
    let mut found = 0_u32;
    let mut reciprocal_rank_sum = 0.0_f64;
    let mut latency_ms = Vec::new();

    for (query, expected) in expectations {
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
                "off",
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
        let hits = json["indexed_symbol_hits"]
            .as_array()
            .expect("indexed_symbol_hits");
        if let Some(position) = hits.iter().position(|hit| {
            hit["display_name"]
                .as_str()
                .is_some_and(|name| name.contains(expected))
        }) {
            found += 1;
            reciprocal_rank_sum += 1.0 / (position as f64 + 1.0);
        }
    }

    let recall = found as f64 / expectations.len() as f64;
    let mrr = reciprocal_rank_sum / expectations.len() as f64;
    let max_latency_ms = latency_ms.into_iter().max().unwrap_or_default();
    eprintln!(
        "search_quality_eval recall={recall:.3} mrr={mrr:.3} max_latency_ms={max_latency_ms}"
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
    assert!(
        max_latency_ms < 3_000,
        "search latency should stay bounded on eval fixture, got {max_latency_ms}ms"
    );
}
