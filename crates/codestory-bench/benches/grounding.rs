use codestory_api::{GroundingBudgetDto, OpenProjectRequest};
use codestory_app::AppController;
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_project::Project;
use codestory_storage::Storage;
use criterion::{Criterion, criterion_group, criterion_main};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn write_fixture(root: &Path, file_count: usize) -> anyhow::Result<()> {
    let src = root.join("src");
    std::fs::create_dir_all(&src)?;
    std::fs::write(
        src.join("main.rs"),
        "mod shared;\nfn main() { println!(\"hello\"); }\n",
    )?;
    std::fs::write(
        src.join("shared.rs"),
        "pub struct SharedConfig { pub enabled: bool }\npub fn shared_helper() -> bool { true }\n",
    )?;

    for idx in 0..file_count {
        let path = src.join(format!("module_{idx}.rs"));
        let content = format!(
            r#"
pub struct Controller{idx} {{
    pub enabled: bool,
}}

impl Controller{idx} {{
    pub fn new() -> Self {{
        Self {{ enabled: shared::shared_helper() }}
    }}

    pub fn check_winner_{idx}(&self) -> bool {{
        self.enabled && helper_{idx}()
    }}
}}

pub fn helper_{idx}() -> bool {{
    true
}}
"#
        );
        std::fs::write(path, content)?;
    }

    Ok(())
}

fn build_indexed_controller(
    file_count: usize,
) -> anyhow::Result<(TempDir, PathBuf, AppController)> {
    let temp = tempfile::tempdir()?;
    write_fixture(temp.path(), file_count)?;

    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path)?;
    let project = Project::open(temp.path().to_path_buf())?;
    let refresh_info = project.full_refresh()?;
    let event_bus = EventBus::new();
    let indexer = WorkspaceIndexer::new(temp.path().to_path_buf());
    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;
    drop(storage);

    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .map_err(|error| anyhow::anyhow!("open_project failed: {:?}", error))?;

    Ok((temp, storage_path, controller))
}

fn bench_grounding_snapshot(c: &mut Criterion) {
    let (_temp, storage_path, controller) =
        build_indexed_controller(120).expect("prepare benchmark workspace");
    let root = controller
        .open_project_summary_with_storage_path(
            storage_path.parent().expect("storage parent").to_path_buf(),
            storage_path.clone(),
        )
        .expect("project summary")
        .root;
    let project_root = PathBuf::from(root);

    c.bench_function("grounding_snapshot_balanced", |b| {
        b.iter(|| {
            let _ = controller
                .grounding_snapshot(GroundingBudgetDto::Balanced)
                .expect("grounding snapshot should succeed");
        })
    });

    c.bench_function("grounding_summary_open_plus_snapshot_balanced", |b| {
        b.iter(|| {
            controller
                .open_project_summary_with_storage_path(project_root.clone(), storage_path.clone())
                .expect("summary open should succeed");
            let _ = controller
                .grounding_snapshot(GroundingBudgetDto::Balanced)
                .expect("grounding snapshot should succeed");
        })
    });
}

criterion_group!(benches, bench_grounding_snapshot);
criterion_main!(benches);
