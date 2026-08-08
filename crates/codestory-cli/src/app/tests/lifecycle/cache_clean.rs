use crate::app::lifecycle::run_cache;
use crate::args::{CacheAction, CacheCleanCommand, CacheCommand, OutputFormat};
use codestory_retrieval::{CacheCleanPlan, CacheCleanReport};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use tempfile::tempdir;

const OTHER_DIGEST: &str = codestory_retrieval::test_support::CACHE_CLEAN_OTHER_MODEL_DIGEST;
const LIVE_WORKSPACE: &str = codestory_retrieval::test_support::CACHE_CLEAN_LIVE_WORKSPACE;
const DEAD_WORKSPACE: &str = codestory_retrieval::test_support::CACHE_CLEAN_DEAD_WORKSPACE;
const UNREGISTERED_WORKSPACE: &str =
    codestory_retrieval::test_support::CACHE_CLEAN_UNREGISTERED_WORKSPACE;

fn run_cache_clean(cache_root: &Path, apply: bool, format: OutputFormat, output: &Path) {
    codestory_retrieval::with_test_cache_root(cache_root, || {
        run_cache(CacheCommand {
            action: CacheAction::Clean(CacheCleanCommand {
                apply,
                format,
                output_file: Some(output.to_path_buf()),
            }),
        })
        .expect("run cache clean fixture")
    });
}

fn normalized_text(text: &str, cache_root: &Path, worktrees_root: &Path) -> String {
    text.replace('\\', "/")
        .replace(
            &cache_root.to_string_lossy().replace('\\', "/"),
            "<CACHE_ROOT>",
        )
        .replace(
            &worktrees_root.to_string_lossy().replace('\\', "/"),
            "<WORKTREES>",
        )
        .replace(codestory_llama_sys::MODEL_SHA256, "<CURRENT_MODEL_DIGEST>")
}

fn normalize_plan_value(value: &mut Value, cache_root: &Path, worktrees_root: &Path) {
    value["cache_root"] = json!("<CACHE_ROOT>");
    value["current_model_digest"] = json!("<CURRENT_MODEL_DIGEST>");
    value["reclaimable_bytes"] = json!("<RECLAIMABLE_BYTES>");
    for candidate in value["candidates"].as_array_mut().expect("plan candidates") {
        candidate["bytes"] = match candidate["kind"].as_str() {
            Some("abandoned_project_cache") => json!("<ABANDONED_BYTES>"),
            Some("superseded_model_digest") => json!("<SUPERSEDED_BYTES>"),
            other => panic!("unexpected candidate kind {other:?}"),
        };
        let proof = candidate["proof"].as_str().expect("candidate proof");
        candidate["proof"] = json!(normalized_text(proof, cache_root, worktrees_root));
    }
    for retained in value["retained"].as_array_mut().expect("plan retained") {
        let relative_path = retained["relative_path"]
            .as_str()
            .expect("retained relative path");
        retained["relative_path"] =
            json!(normalized_text(relative_path, cache_root, worktrees_root));
        let detail = retained["detail"].as_str().expect("retained detail");
        retained["detail"] = json!(normalized_text(detail, cache_root, worktrees_root));
    }
}

fn normalize_report_value(value: &mut Value, cache_root: &Path, worktrees_root: &Path) {
    normalize_plan_value(&mut value["plan"], cache_root, worktrees_root);
    value["removed_bytes"] = json!("<REMOVED_BYTES>");
    for removal in value["removals"].as_array_mut().expect("report removals") {
        removal["bytes"] = match removal["kind"].as_str() {
            Some("abandoned_project_cache") => json!("<ABANDONED_BYTES>"),
            Some("superseded_model_digest") => json!("<SUPERSEDED_BYTES>"),
            other => panic!("unexpected removal kind {other:?}"),
        };
    }
}

fn normalized_plan_markdown(
    markdown: &str,
    plan: &CacheCleanPlan,
    cache_root: &Path,
    worktrees_root: &Path,
) -> String {
    let mut markdown = normalized_text(markdown, cache_root, worktrees_root).replace(
        &format!("reclaimable_bytes: {}", plan.reclaimable_bytes),
        "reclaimable_bytes: <RECLAIMABLE_BYTES>",
    );
    for candidate in &plan.candidates {
        let replacement = match candidate.kind {
            codestory_retrieval::CacheCleanKind::AbandonedProjectCache => "<ABANDONED_BYTES>",
            codestory_retrieval::CacheCleanKind::SupersededModelDigest => "<SUPERSEDED_BYTES>",
        };
        markdown = markdown.replace(
            &format!("`{}` ({} bytes)", candidate.relative_path, candidate.bytes),
            &format!("`{}` ({replacement} bytes)", candidate.relative_path),
        );
    }
    markdown
}

fn byte_snapshot(root: &Path) -> BTreeMap<String, String> {
    fn collect(root: &Path, current: &Path, entries: &mut BTreeMap<String, String>) {
        let mut paths = std::fs::read_dir(current)
            .expect("read snapshot directory")
            .map(|entry| entry.expect("read snapshot entry").path())
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let relative = path
                .strip_prefix(root)
                .expect("snapshot path below root")
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = std::fs::symlink_metadata(&path).expect("snapshot metadata");
            if metadata.is_dir() {
                entries.insert(format!("{relative}/"), "directory".into());
                collect(root, &path, entries);
            } else {
                let bytes = std::fs::read(&path).expect("snapshot file bytes");
                entries.insert(
                    relative,
                    format!("{}:{:x}", bytes.len(), Sha256::digest(&bytes)),
                );
            }
        }
    }

    let mut entries = BTreeMap::new();
    collect(root, root, &mut entries);
    entries
}

fn directory_listing(root: &Path) -> BTreeMap<String, String> {
    fn collect(root: &Path, current: &Path, entries: &mut BTreeMap<String, String>) {
        let mut paths = std::fs::read_dir(current)
            .expect("read listing directory")
            .map(|entry| entry.expect("read listing entry").path())
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let relative = path
                .strip_prefix(root)
                .expect("listing path below root")
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = std::fs::symlink_metadata(&path).expect("listing metadata");
            if metadata.is_dir() {
                entries.insert(format!("{relative}/"), "directory".into());
                collect(root, &path, entries);
            } else {
                entries.insert(relative, format!("file: {} bytes", metadata.len()));
            }
        }
    }

    let mut entries = BTreeMap::new();
    collect(root, root, &mut entries);
    entries
}

fn expected_normalized_plan(dry_run: bool) -> Value {
    json!({
        "schema_version": 1,
        "dry_run": dry_run,
        "cache_root": "<CACHE_ROOT>",
        "current_model_digest": "<CURRENT_MODEL_DIGEST>",
        "reclaimable_bytes": "<RECLAIMABLE_BYTES>",
        "candidates": [
            {
                "kind": "abandoned_project_cache",
                "relative_path": DEAD_WORKSPACE,
                "bytes": "<ABANDONED_BYTES>",
                "proof": "registered project root <WORKTREES>/dead no longer exists"
            },
            {
                "kind": "superseded_model_digest",
                "relative_path": format!("embedded-models/sha256/{OTHER_DIGEST}"),
                "bytes": "<SUPERSEDED_BYTES>",
                "proof": "digest differs from the compiled model digest <CURRENT_MODEL_DIGEST> and no peer holds its materialization lock"
            }
        ],
        "retained": [
            {
                "relative_path": LIVE_WORKSPACE,
                "reason": "live_workspace",
                "detail": "registered project root <WORKTREES>/live is live_workspace"
            },
            {
                "relative_path": UNREGISTERED_WORKSPACE,
                "reason": "unregistered_workspace",
                "detail": "no schema 2 retention marker registers this workspace cache"
            },
            {
                "relative_path": "embedded-models/sha256/<CURRENT_MODEL_DIGEST>",
                "reason": "current_model_digest",
                "detail": "digest matches the model compiled into this executable"
            },
            {
                "relative_path": "embedded-models/sha256/not-a-digest",
                "reason": "unrecognized_entry",
                "detail": "directory name is not a SHA-256 digest"
            }
        ],
        "errors": []
    })
}

#[test]
fn cache_clean_plan_cli_golden_is_observational_and_non_vacuous() {
    let fixture = tempdir().expect("cache-clean plan fixture");
    let cache_root = fixture.path().join("cache");
    let worktrees_root = fixture.path().join("worktrees");
    codestory_retrieval::test_support::populate_cache_clean_fixture(&cache_root, &worktrees_root)
        .expect("populate cache-clean plan fixture");
    let before = byte_snapshot(&cache_root);

    let json_output = fixture.path().join("plan.json");
    run_cache_clean(&cache_root, false, OutputFormat::Json, &json_output);
    let plan: CacheCleanPlan =
        serde_json::from_slice(&std::fs::read(&json_output).expect("read cache-clean JSON plan"))
            .expect("parse cache-clean JSON plan");
    assert_eq!(plan.candidates.len(), 2, "both cleanup proofs must fire");
    assert_eq!(plan.retained.len(), 4, "all refusal classes must render");
    assert!(
        plan.reclaimable_bytes > 12,
        "dead core bytes must be measured"
    );
    assert!(plan.errors.is_empty(), "fixture must be fully observable");
    let mut normalized = serde_json::to_value(&plan).expect("serialize normalized plan");
    normalize_plan_value(&mut normalized, &cache_root, &worktrees_root);
    assert_eq!(normalized, expected_normalized_plan(true));

    let markdown_output = fixture.path().join("plan.md");
    run_cache_clean(&cache_root, false, OutputFormat::Markdown, &markdown_output);
    let markdown = normalized_plan_markdown(
        &std::fs::read_to_string(&markdown_output).expect("read cache-clean Markdown plan"),
        &plan,
        &cache_root,
        &worktrees_root,
    );
    assert_eq!(
        markdown,
        format!(
            "# Cache Clean\ndry_run: `true`\ncache_root: `<CACHE_ROOT>`\ncurrent_model_digest: `<CURRENT_MODEL_DIGEST>`\nreclaimable_bytes: <RECLAIMABLE_BYTES>\n\n## Candidates\n- `{DEAD_WORKSPACE}` (<ABANDONED_BYTES> bytes): registered project root <WORKTREES>/dead no longer exists\n- `embedded-models/sha256/{OTHER_DIGEST}` (<SUPERSEDED_BYTES> bytes): digest differs from the compiled model digest <CURRENT_MODEL_DIGEST> and no peer holds its materialization lock\n\n## Retained\n- `{LIVE_WORKSPACE}` [live_workspace]: registered project root <WORKTREES>/live is live_workspace\n- `{UNREGISTERED_WORKSPACE}` [unregistered_workspace]: no schema 2 retention marker registers this workspace cache\n- `embedded-models/sha256/<CURRENT_MODEL_DIGEST>` [current_model_digest]: digest matches the model compiled into this executable\n- `embedded-models/sha256/not-a-digest` [unrecognized_entry]: directory name is not a SHA-256 digest\n"
        )
    );
    assert_eq!(
        byte_snapshot(&cache_root),
        before,
        "cache-clean planning must leave every cache byte unchanged"
    );
}

#[test]
fn cache_clean_apply_cli_matches_owner_report_and_post_state() {
    let fixture = tempdir().expect("cache-clean apply fixture");
    let direct_cache = fixture.path().join("direct-cache");
    let facade_cache = fixture.path().join("facade-cache");
    let direct_worktrees = fixture.path().join("direct-worktrees");
    let facade_worktrees = fixture.path().join("facade-worktrees");
    codestory_retrieval::test_support::populate_cache_clean_fixture(
        &direct_cache,
        &direct_worktrees,
    )
    .expect("populate direct cleanup fixture");
    codestory_retrieval::test_support::populate_cache_clean_fixture(
        &facade_cache,
        &facade_worktrees,
    )
    .expect("populate facade cleanup fixture");

    let direct = codestory_retrieval::with_test_cache_root(&direct_cache, || {
        codestory_retrieval::apply_cache_clean().expect("apply owner cleanup")
    });
    let facade_output = fixture.path().join("facade-report.json");
    run_cache_clean(&facade_cache, true, OutputFormat::Json, &facade_output);
    let facade: CacheCleanReport =
        serde_json::from_slice(&std::fs::read(&facade_output).expect("read facade cleanup report"))
            .expect("parse facade cleanup report");

    assert_eq!(
        direct.removals.len(),
        2,
        "owner fixture must remove both classes"
    );
    assert!(direct.removals.iter().all(|removal| removal.removed));
    assert!(direct.errors.is_empty(), "owner cleanup must be error-free");
    assert_eq!(facade.removals.len(), 2, "facade must remove both classes");
    assert!(facade.removals.iter().all(|removal| removal.removed));
    assert!(
        facade.errors.is_empty(),
        "facade cleanup must be error-free"
    );

    let mut direct_value = serde_json::to_value(&direct).expect("serialize owner report");
    normalize_report_value(&mut direct_value, &direct_cache, &direct_worktrees);
    let mut facade_value = serde_json::to_value(&facade).expect("serialize facade report");
    normalize_report_value(&mut facade_value, &facade_cache, &facade_worktrees);
    assert_eq!(facade_value, direct_value);
    assert_eq!(
        facade_value,
        json!({
            "schema_version": 1,
            "dry_run": false,
            "plan": expected_normalized_plan(false),
            "removed_bytes": "<REMOVED_BYTES>",
            "removals": [
                {
                    "kind": "abandoned_project_cache",
                    "relative_path": DEAD_WORKSPACE,
                    "bytes": "<ABANDONED_BYTES>",
                    "removed": true
                },
                {
                    "kind": "superseded_model_digest",
                    "relative_path": format!("embedded-models/sha256/{OTHER_DIGEST}"),
                    "bytes": "<SUPERSEDED_BYTES>",
                    "removed": true
                }
            ],
            "errors": []
        })
    );
    assert_eq!(
        directory_listing(&facade_cache),
        directory_listing(&direct_cache),
        "runtime-boundary cleanup must preserve the owner's exact post-state listing"
    );
    for cache_root in [&direct_cache, &facade_cache] {
        assert!(!cache_root.join(DEAD_WORKSPACE).exists());
        assert!(cache_root.join(LIVE_WORKSPACE).is_dir());
        assert!(cache_root.join(UNREGISTERED_WORKSPACE).is_dir());
        assert!(
            cache_root
                .join("embedded-models/sha256")
                .join(codestory_llama_sys::MODEL_SHA256)
                .is_dir()
        );
        assert!(
            !cache_root
                .join("embedded-models/sha256")
                .join(OTHER_DIGEST)
                .exists()
        );
        assert!(
            cache_root
                .join("retention/global_generation_gc.lock")
                .is_file()
        );
    }
}
