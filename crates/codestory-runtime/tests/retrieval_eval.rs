use codestory_contracts::api::{
    GroundingBudgetDto, IndexMode, LayoutDirection, RetrievalModeDto, SearchRequest,
    TrailCallerScope, TrailConfigDto, TrailDirection, TrailMode,
};
use codestory_runtime::AppController;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use tempfile::{TempDir, tempdir};

static HYBRID_EVAL_ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

struct HybridEvalEnv {
    guards: Option<Vec<EnvGuard>>,
    _lock: MutexGuard<'static, ()>,
}

impl Drop for HybridEvalEnv {
    fn drop(&mut self) {
        let _ = self.guards.take();
    }
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(value) = self.previous.as_deref() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

fn hybrid_eval_env() -> HybridEvalEnv {
    let lock = HYBRID_EVAL_ENV_LOCK
        .lock()
        .expect("hybrid eval env lock poisoned");
    let guards = vec![
        EnvGuard::set("CODESTORY_HYBRID_RETRIEVAL_ENABLED", "true"),
        EnvGuard::set("CODESTORY_EMBED_RUNTIME_MODE", "hash"),
        EnvGuard::remove("CODESTORY_EMBED_MODEL_PATH"),
        EnvGuard::remove("CODESTORY_EMBED_TOKENIZER_PATH"),
    ];
    HybridEvalEnv {
        guards: Some(guards),
        _lock: lock,
    }
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

pub fn follow_edges() {
    exact_symbol_anchor();
    let _ = build_snapshot_digest();
}
"#,
    )
    .expect("write fixture source");
}

fn indexed_controller() -> (AppController, TempDir, TempDir) {
    let workspace = tempdir().expect("workspace dir");
    write_retrieval_fixture(workspace.path());

    let storage = tempdir().expect("storage dir");
    let controller = AppController::new();
    controller
        .open_project_with_storage_path(
            workspace.path().to_path_buf(),
            storage.path().join("codestory.db"),
        )
        .expect("open project");
    controller
        .run_indexing_blocking(IndexMode::Full)
        .expect("index workspace");

    (controller, workspace, storage)
}

#[test]
fn retrieval_eval_exact_symbol_queries_prefer_exact_symbol_hits() {
    let _env = hybrid_eval_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let hits = controller
        .search_hybrid(
            SearchRequest {
                query: "exact_symbol_anchor".to_string(),
                repo_text: codestory_contracts::api::SearchRepoTextMode::Off,
                limit_per_source: 5,
                hybrid_weights: None,
                hybrid_limits: None,
            },
            None,
            Some(5),
            None,
        )
        .expect("search exact symbol");

    let top = hits.first().expect("top hit");
    assert_eq!(top.display_name, "exact_symbol_anchor");
    assert!(top.resolvable);
}

#[test]
fn retrieval_eval_natural_language_queries_hit_semantic_symbol_docs() {
    let _env = hybrid_eval_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let hits = controller
        .search_hybrid(
            SearchRequest {
                query: "compressed grounding summary for oss users".to_string(),
                repo_text: codestory_contracts::api::SearchRepoTextMode::Off,
                limit_per_source: 5,
                hybrid_weights: None,
                hybrid_limits: None,
            },
            None,
            Some(5),
            None,
        )
        .expect("semantic search");

    let top = hits.first().expect("top hit");
    assert_eq!(top.display_name, "build_snapshot_digest");
}

#[test]
fn retrieval_eval_grounding_snapshot_reports_hybrid_state() {
    let _env = hybrid_eval_env();
    let (controller, _workspace, _storage) = indexed_controller();

    let snapshot = controller
        .grounding_snapshot(GroundingBudgetDto::Balanced)
        .expect("grounding snapshot");
    let retrieval = snapshot.retrieval.expect("retrieval state");

    assert_eq!(retrieval.mode, RetrievalModeDto::Hybrid);
    assert!(retrieval.semantic_ready);
    assert!(retrieval.semantic_doc_count >= 3);
    assert!(
        snapshot
            .notes
            .iter()
            .any(|note| note.contains("Retrieval mode: hybrid"))
    );
}

#[test]
fn retrieval_eval_trail_context_keeps_grounded_neighbors() {
    let _env = hybrid_eval_env();
    let (controller, _workspace, _storage) = indexed_controller();
    let focus = controller
        .search_hybrid(
            SearchRequest {
                query: "follow_edges".to_string(),
                repo_text: codestory_contracts::api::SearchRepoTextMode::Off,
                limit_per_source: 3,
                hybrid_weights: None,
                hybrid_limits: None,
            },
            None,
            Some(3),
            None,
        )
        .expect("resolve trail focus")
        .into_iter()
        .next()
        .expect("follow_edges hit")
        .node_id;

    let trail = controller
        .trail_context(TrailConfigDto {
            root_id: focus,
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: 1,
            direction: TrailDirection::Outgoing,
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: Vec::new(),
            show_utility_calls: true,
            node_filter: Vec::new(),
            max_nodes: 12,
            layout_direction: LayoutDirection::Horizontal,
        })
        .expect("trail context");

    let labels = trail
        .trail
        .nodes
        .iter()
        .map(|node| node.label.clone())
        .collect::<Vec<_>>();
    assert!(labels.iter().any(|label| label.contains("follow_edges")));
    assert!(
        labels
            .iter()
            .any(|label| label.contains("exact_symbol_anchor"))
    );
    assert!(
        labels
            .iter()
            .any(|label| label.contains("build_snapshot_digest"))
    );
    assert!(!trail.trail.truncated);
}
