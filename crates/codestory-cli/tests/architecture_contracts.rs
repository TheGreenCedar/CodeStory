use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate has workspace parent")
        .parent()
        .expect("workspace root exists")
        .to_path_buf()
}

fn read(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).expect("file should be readable")
}

#[test]
fn workspace_crate_stays_decoupled_from_store_and_runtime() {
    let cargo_toml = read("crates/codestory-workspace/Cargo.toml");
    assert!(
        !cargo_toml.contains("codestory-store")
            && !cargo_toml.contains("codestory-runtime")
            && !cargo_toml.contains("codestory-cli"),
        "workspace crate should only own discovery and planning inputs"
    );
}

#[test]
fn indexer_crate_stays_decoupled_from_runtime_and_cli() {
    let cargo_toml = read("crates/codestory-indexer/Cargo.toml");
    assert!(
        !cargo_toml.contains("codestory-runtime") && !cargo_toml.contains("codestory-cli"),
        "indexer crate should not depend on runtime or cli"
    );
}

#[test]
fn runtime_crate_depends_on_v2_surfaces_only() {
    let cargo_toml = read("crates/codestory-runtime/Cargo.toml");
    assert!(
        cargo_toml.contains("codestory-contracts")
            && cargo_toml.contains("codestory-indexer")
            && cargo_toml.contains("codestory-store"),
        "runtime should depend on contracts, indexer, and store"
    );
    for legacy in [
        "codestory-app",
        "codestory-search",
        "codestory-storage",
        "codestory-api",
        "codestory-events",
        "codestory-core",
    ] {
        assert!(
            !cargo_toml.contains(legacy),
            "runtime should not depend on removed legacy crate {legacy}"
        );
    }
    assert!(
        !cargo_toml.contains("codestory-index ="),
        "runtime should not depend on removed legacy crate codestory-index"
    );
}

#[test]
fn store_crate_owns_persistence_without_legacy_escape_hatches() {
    let cargo_toml = read("crates/codestory-store/Cargo.toml");
    for legacy in [
        "codestory-storage",
        "codestory-core",
        "codestory-api",
        "codestory-events",
    ] {
        assert!(
            !cargo_toml.contains(legacy),
            "store should not depend on removed legacy crate {legacy}"
        );
    }

    let store_src = read("crates/codestory-store/src/lib.rs");
    assert!(
        !store_src.contains("from_storage(")
            && !store_src.contains("into_inner(")
            && !store_src.contains("storage_mut(")
            && !store_src.contains("as_inner(")
            && !store_src.contains("Deref for Store")
            && !store_src.contains("DerefMut for Store"),
        "store facade should not expose raw storage escape hatches"
    );
}

#[test]
fn cli_stays_thin() {
    let cargo_toml = read("crates/codestory-cli/Cargo.toml");
    assert!(
        cargo_toml.contains("codestory-runtime"),
        "cli should depend on runtime surface"
    );
    assert!(
        !cargo_toml.contains("codestory-store") && !cargo_toml.contains("codestory-indexer"),
        "cli should not reach directly into store or indexer"
    );
}

#[test]
fn runtime_snapshot_lifecycle_flows_through_store_snapshot_surface() {
    let runtime = read("crates/codestory-runtime/src/lib.rs");
    assert!(
        runtime.contains("SnapshotStore::open_staged(storage_path)")
            && runtime.contains("finalize_staged()")
            && runtime.contains("staged.publish(storage_path)"),
        "full refresh should stage, finalize, and publish snapshots through the store snapshot surface"
    );
    assert!(
        runtime.contains("store.snapshots().refresh_all_with_stats()"),
        "incremental refresh should use the snapshot surface for summary/detail refresh"
    );
    assert!(
        !runtime.contains("create_deferred_secondary_indexes()")
            && !runtime.contains("refresh_grounding_summary_snapshots()")
            && !runtime.contains("hydrate_grounding_detail_snapshots()"),
        "snapshot lifecycle should not be orchestrated directly outside the store snapshot surface"
    );
}

#[test]
fn legacy_crates_are_removed_from_the_workspace() {
    let workspace = read("Cargo.toml");
    for legacy in [
        "codestory-app",
        "codestory-project",
        "codestory-search",
        "codestory-core",
        "codestory-api",
        "codestory-events",
        "codestory-storage",
        "codestory-index",
    ] {
        assert!(
            !workspace.contains(&format!("\"crates/{legacy}\""))
                && !workspace.contains(&format!("{legacy} = {{ path = \"crates/{legacy}\" }}")),
            "workspace should not register removed crate {legacy}"
        );
    }
}
