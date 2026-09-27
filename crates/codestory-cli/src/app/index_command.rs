use super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::diagnostics::{build_summary_readiness, doctor_sidecar_status};
use crate::args;
use crate::args::{IndexCommand, IndexDryRunOutput, IndexOutput};
use crate::output::{emit, render_index_dry_run_markdown, render_index_markdown};
use crate::runtime::{
    RuntimeContext, annotate_refresh_error, index_mode_name, map_api_error,
    map_api_error_for_project, refresh_label, refresh_mode_name,
};
use crate::{display, readiness};
use anyhow::Context;
use anyhow::{Result, bail};
use codestory_contracts::api::{AppEventPayload, IndexMode};
use std::fs;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

pub(super) fn run_index(cmd: IndexCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "index")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    validate_index_watch_output_file(&cmd)?;
    run_index_once(&cmd)?;
    if cmd.watch {
        run_index_watch(cmd)?;
    }
    Ok(())
}

pub(in crate::app) fn validate_index_watch_output_file(cmd: &IndexCommand) -> Result<()> {
    if !cmd.watch {
        return Ok(());
    }
    let Some(output_file) = cmd.output_file.as_deref() else {
        return Ok(());
    };

    let project_root = fs::canonicalize(&cmd.project.project).with_context(|| {
        format!(
            "Failed to resolve project root {}",
            display::clean_path_string(&cmd.project.project.to_string_lossy())
        )
    })?;
    let output_path = if output_file.is_absolute() {
        output_file.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to resolve current directory")?
            .join(output_file)
    };
    let Some(output_parent) = output_path.parent() else {
        return Ok(());
    };
    if !output_parent.exists() {
        return Ok(());
    }
    let resolved_parent = fs::canonicalize(output_parent).with_context(|| {
        format!(
            "Failed to resolve output parent {}",
            display::clean_path_string(&output_parent.to_string_lossy())
        )
    })?;
    let resolved_output = output_path
        .file_name()
        .map(|file_name| resolved_parent.join(file_name))
        .unwrap_or(resolved_parent);

    if resolved_output.starts_with(&project_root) {
        bail!(
            "--watch cannot write --output-file inside the watched project tree: {}",
            display::clean_path_string(&resolved_output.to_string_lossy())
        );
    }

    Ok(())
}

fn run_index_once(cmd: &IndexCommand) -> Result<()> {
    let runtime = if cmd.dry_run {
        RuntimeContext::new_inspect_only(&cmd.project)?
    } else {
        RuntimeContext::new(&cmd.project)?
    };
    if cmd.dry_run {
        let decision = runtime.resolve_refresh_decision_with_preflight(cmd.refresh)?;
        let refresh_mode = decision.effective_mode.unwrap_or(IndexMode::Incremental);
        runtime
            .index
            .bind_project_paths_for_refresh(
                runtime.project_root.clone(),
                runtime.storage_path.clone(),
            )
            .map_err(|error| map_api_error_for_project(error, &runtime.project_root))?;
        let dry_run = runtime.index.dry_run_index(refresh_mode).map_err(|error| {
            map_api_error_for_project(
                annotate_refresh_error(error, cmd.refresh, refresh_mode),
                &runtime.project_root,
            )
        })?;
        let output = IndexDryRunOutput {
            requested_refresh: refresh_mode_name(cmd.refresh),
            effective_refresh: index_mode_name(refresh_mode),
            compatibility_reason: decision.reason.as_deref(),
            dry_run: &dry_run,
        };
        let markdown = render_index_dry_run_markdown(&output);
        return emit(cmd.format, &output, markdown, cmd.output_file.as_deref());
    }

    let progress = if cmd.progress {
        Some(spawn_progress_printer(runtime.events.clone()))
    } else {
        None
    };
    let opened = runtime.ensure_open(cmd.refresh)?;
    if let Some(progress) = progress {
        progress.finish();
    }
    let summary_generation = if cmd.summarize {
        Some(
            runtime
                .index
                .summarize_symbols_blocking()
                .map_err(map_api_error)?,
        )
    } else {
        None
    };
    let retrieval = opened
        .summary
        .retrieval
        .as_ref()
        .context("Open project summary did not include retrieval state")?;
    let refresh_label = refresh_label(cmd.refresh, opened.refresh_mode);
    let storage_path = runtime.storage_path.to_string_lossy().to_string();
    let sidecar_retrieval = doctor_sidecar_status(&runtime);
    let readiness = build_summary_readiness(
        &opened.summary.root,
        &opened.summary.stats,
        opened.summary.freshness.as_ref(),
        &sidecar_retrieval,
    );
    let next_commands = readiness::compatibility_next_commands(&readiness);
    let output = IndexOutput {
        project: &opened.summary.root,
        storage_path: &storage_path,
        refresh: &refresh_label,
        refresh_reason: opened.refresh_reason.as_deref(),
        summary: &opened.summary,
        retrieval,
        phase_timings: opened.phase_timings.as_ref(),
        summary_generation: summary_generation.as_ref(),
        readiness,
        next_commands,
    };

    let markdown = render_index_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

struct ProgressPrinter {
    done: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

impl ProgressPrinter {
    fn finish(self) {
        self.done.store(true, Ordering::SeqCst);
        let _ = self.handle.join();
    }
}

fn spawn_progress_printer(rx: crossbeam_channel::Receiver<AppEventPayload>) -> ProgressPrinter {
    let done = Arc::new(AtomicBool::new(false));
    let worker_done = Arc::clone(&done);
    let handle = std::thread::spawn(move || {
        while !worker_done.load(Ordering::SeqCst) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => print_progress_event(event),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    ProgressPrinter { done, handle }
}

fn print_progress_event(event: AppEventPayload) {
    match event {
        AppEventPayload::IndexingProgress { current, total } => {
            eprintln!(
                "[{current}/{total}] {} indexing",
                format_progress_bar(current, total)
            );
        }
        AppEventPayload::IndexingStarted { file_count } => {
            eprintln!(
                "[0/{file_count}] {} indexing started",
                format_progress_bar(0, file_count)
            );
        }
        _ => {}
    }
}

fn format_progress_bar(current: u32, total: u32) -> String {
    const WIDTH: u32 = 18;
    let filled = if total == 0 {
        0
    } else {
        current.saturating_mul(WIDTH) / total.max(1)
    }
    .min(WIDTH);
    format!(
        "[{}{}]",
        "#".repeat(filled as usize),
        "-".repeat(WIDTH.saturating_sub(filled) as usize)
    )
}

fn run_index_watch(mut cmd: IndexCommand) -> Result<()> {
    use notify::{RecursiveMode, Watcher};

    cmd.dry_run = false;
    cmd.refresh = args::RefreshMode::Incremental;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    })?;
    watcher.watch(&cmd.project.project, RecursiveMode::Recursive)?;
    eprintln!(
        "watching {} for changes; press Ctrl+C to stop",
        cmd.project.project.display()
    );
    loop {
        match rx.recv() {
            Ok(Ok(_event)) => {
                std::thread::sleep(Duration::from_millis(250));
                while rx.try_recv().is_ok() {}
                eprintln!("change detected; running incremental index");
                run_index_once(&cmd)?;
            }
            Ok(Err(error)) => eprintln!("watch error: {error}"),
            Err(error) => anyhow::bail!("watch channel closed: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{OutputFormat, ProjectArgs, RefreshMode};

    #[test]
    fn index_command_reports_incremental_copy_when_clone_is_unavailable() {
        let temp = tempfile::tempdir().expect("isolated index command fixture");
        let project = temp.path().join("project");
        fs::create_dir(&project).expect("project root");
        let source = project.join("lib.rs");
        fs::write(&source, "pub fn alpha() -> i32 { 1 }\n").expect("seed source");
        let output_file = temp.path().join("index.json");
        let command = |refresh| IndexCommand {
            project: ProjectArgs {
                project: project.clone(),
                cache_dir: Some(temp.path().join("cache")),
            },
            refresh,
            format: OutputFormat::Json,
            output_file: Some(output_file.clone()),
            dry_run: false,
            summarize: false,
            progress: false,
            watch: false,
        };
        run_index(command(RefreshMode::Full)).expect("seed published core through index command");
        fs::write(&source, "pub fn alpha() -> i32 { 2 }\n").expect("change indexed source");
        codestory_runtime::with_core_clone_disabled_for_test(|| {
            run_index(command(RefreshMode::Auto))
        })
        .expect("auto refresh keeps the incremental publication");

        let output: serde_json::Value = serde_json::from_slice(
            &fs::read(&output_file).expect("read executed index command JSON"),
        )
        .expect("parse index command JSON");
        assert_eq!(output["refresh"], "auto(incremental)");
        assert!(output.get("refresh_reason").is_none());
        assert!(
            output["phase_timings"]["full_refresh_wall"].is_null(),
            "the command must report the incremental refresh that actually executed"
        );
        let storage_path = output["storage_path"]
            .as_str()
            .expect("command storage path");
        let observer = RuntimeContext::new_inspect_only(&command(RefreshMode::Auto).project)
            .expect("observe command publication through runtime");
        let published_mode = || {
            observer
                .project
                .complete_index_publication_at(std::path::Path::new(storage_path))
                .expect("read command publication")
                .expect("complete command publication")
                .mode
        };
        assert_eq!(
            published_mode(),
            codestory_contracts::api::IndexPublicationModeDto::Incremental
        );

        fs::write(&source, "pub fn alpha() -> i32 { 3 }\n").expect("change source again");
        run_index(command(RefreshMode::Auto)).expect("ordinary incremental index command");
        let ordinary: serde_json::Value = serde_json::from_slice(
            &fs::read(&output_file).expect("read ordinary index command JSON"),
        )
        .expect("parse ordinary index command JSON");
        assert_eq!(ordinary["refresh"], "auto(incremental)");
        assert!(ordinary.get("refresh_reason").is_none());
        assert_eq!(
            published_mode(),
            codestory_contracts::api::IndexPublicationModeDto::Incremental
        );
    }

    #[test]
    fn index_command_preserves_structured_insufficient_space_error() {
        let temp = tempfile::tempdir().expect("isolated index command fixture");
        let project = temp.path().join("project");
        fs::create_dir(&project).expect("project root");
        fs::write(project.join("lib.rs"), "pub fn alpha() {}\n").expect("source");
        let output_file = temp.path().join("index.json");
        let command = IndexCommand {
            project: ProjectArgs {
                project,
                cache_dir: Some(temp.path().join("cache")),
            },
            refresh: RefreshMode::Full,
            format: OutputFormat::Json,
            output_file: Some(output_file.clone()),
            dry_run: false,
            summarize: false,
            progress: false,
            watch: false,
        };
        let error = codestory_runtime::with_available_filesystem_bytes_for_test(0, || {
            run_index(command).expect_err("zero available bytes must fail")
        });
        let typed = crate::runtime::api_error_in_chain(&error).expect("typed CLI error");
        assert_eq!(typed.code, "insufficient_space");
        let detail = typed
            .details
            .as_ref()
            .and_then(|details| details.disk_space.as_ref())
            .expect("structured capacity detail");
        assert_eq!(detail.available_bytes, 0);
        assert!(detail.required_bytes > 0);
        assert!(!output_file.exists());
    }
}
