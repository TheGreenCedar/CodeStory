use super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use crate::args;
use crate::args::{CacheAction, CacheCommand, Command, InternalOwnedDeleteCommand, ProjectArgs};
use crate::embedding_server_transport;
use crate::output::emit;
use crate::runtime;
use crate::runtime::{RuntimeContext, ensure_index_ready, map_api_error};
use anyhow::Context;
use anyhow::Result;
use codestory_contracts::api::ProjectSummary;
use std::fmt::Write as _;

pub(super) fn embedding_client_transport_mode(
    command: &Command,
) -> Option<embedding_server_transport::ClientTransportMode> {
    match command {
        Command::Ground(_) => Some(embedding_server_transport::ClientTransportMode::ObserveOnly),
        Command::Retrieval(args::RetrievalCommand {
            action: args::RetrievalAction::Status(_),
        }) => Some(embedding_server_transport::ClientTransportMode::ObserveOnly),
        Command::InternalEmbeddingServer => None,
        // This is deliberately an allowlist for attested observe-only capture. New commands retain
        // fresh exact executable identity unless their transport behavior is reviewed explicitly.
        _ => Some(embedding_server_transport::ClientTransportMode::SpawnCapable),
    }
}

pub(super) fn run_internal_owned_delete(cmd: InternalOwnedDeleteCommand) -> Result<()> {
    let deletion = codestory_workspace::owned_deletion::OwnedDeletionRoot::open(&cmd.root)
        .with_context(|| format!("open owned deletion root {}", cmd.root.display()))?;
    deletion.remove(&cmd.relative).with_context(|| {
        format!(
            "remove owned relative path {} below {}",
            cmd.relative.display(),
            cmd.root.display()
        )
    })?;
    Ok(())
}

pub(super) fn new_agent_surface_runtime(
    project: &ProjectArgs,
    profile: Option<args::CliSidecarProfile>,
    run_id: Option<&str>,
) -> Result<RuntimeContext> {
    RuntimeContext::new_agent_sidecar_with_selection(project, profile, run_id)
}

pub(super) struct OpenedAgentSurface {
    pub(super) runtime: RuntimeContext,
    pub(super) before: Option<ProjectSummary>,
    pub(super) opened: runtime::OpenedProject,
}

pub(super) fn open_agent_surface(
    project: &ProjectArgs,
    profile: Option<args::CliSidecarProfile>,
    run_id: Option<&str>,
    refresh: args::RefreshMode,
    surface: &'static str,
) -> Result<OpenedAgentSurface> {
    let runtime = new_agent_surface_runtime(project, profile, run_id)?;
    let (before, opened) = runtime.ensure_open_with_before(refresh)?;
    ensure_index_ready(&opened, surface)?;
    codestory_retrieval::ensure_product_embedding_backend_for_runtime(&runtime.sidecar)
        .map_err(map_embedding_preflight_error)
        .with_context(|| format!("initialize retrieval for {surface}"))?;
    Ok(OpenedAgentSurface {
        runtime,
        before,
        opened,
    })
}

fn map_embedding_preflight_error(error: anyhow::Error) -> anyhow::Error {
    codestory_runtime::embedding_api_error(&error).map_or(error, map_api_error)
}

pub(super) fn run_cache(cmd: CacheCommand) -> Result<()> {
    match cmd.action {
        CacheAction::Identity(cmd) => run_cache_identity(cmd),
        CacheAction::Rehydrate(cmd) => run_cache_rehydrate(cmd),
    }
}

fn run_cache_identity(cmd: args::CacheIdentityCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "cache identity")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let output = codestory_runtime::inspect_repository_identity(&runtime.project_root);
    let markdown = render_cache_identity_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn render_cache_identity_markdown(output: &codestory_runtime::RepositoryIdentityReport) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Cache Identity");
    let _ = writeln!(markdown, "project: `{}`", output.project);
    let _ = writeln!(
        markdown,
        "project_identity_schema_version: `{}`",
        output.project_identity_schema_version
    );
    let _ = writeln!(markdown, "project_id: `{}`", output.project_id);
    let _ = writeln!(markdown, "workspace_id: `{}`", output.workspace_id);
    let _ = writeln!(
        markdown,
        "artifact_scope_id: `{}`",
        output.artifact_scope_id
    );
    let _ = writeln!(
        markdown,
        "root_derived_project_id: `{}`",
        output.root_derived_project_id
    );
    let _ = writeln!(
        markdown,
        "canonical_repository_id: `{}`",
        output
            .canonical_repository_id
            .as_deref()
            .unwrap_or("unavailable")
    );
    let _ = writeln!(
        markdown,
        "repository_identity_schema_version: `{}`",
        output.repository_identity_schema_version
    );
    let _ = writeln!(
        markdown,
        "normalized_repository_identity: `{}`",
        output
            .normalized_repository_identity
            .as_deref()
            .unwrap_or("unavailable")
    );
    let _ = writeln!(
        markdown,
        "legacy_alias_disposition: `{}`",
        output.legacy_alias_disposition
    );
    let _ = writeln!(
        markdown,
        "legacy_project_id: `{}`",
        output.legacy_project_id.as_deref().unwrap_or("unavailable")
    );
    let _ = writeln!(
        markdown,
        "git_remote: `{}`",
        output.git_remote.as_deref().unwrap_or("unavailable")
    );
    let _ = writeln!(
        markdown,
        "git_tree: `{}`",
        output.git_tree.as_deref().unwrap_or("unavailable")
    );
    let _ = writeln!(
        markdown,
        "cache_schema_version: `{}`",
        output.cache_schema_version
    );
    let _ = writeln!(
        markdown,
        "portable_reuse_eligible: `{}`",
        output.portable_reuse_eligible
    );
    let _ = writeln!(
        markdown,
        "portable_reuse_reason: `{}`",
        output.portable_reuse_reason
    );
    let _ = writeln!(markdown, "freshness_inputs:");
    for input in &output.freshness_inputs {
        let _ = writeln!(markdown, "- `{input}`");
    }
    markdown
}

fn run_cache_rehydrate(cmd: args::CacheRehydrateCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "cache rehydrate")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let source_args = ProjectArgs {
        project: cmd.from_project,
        cache_dir: cmd.from_cache_dir,
    };
    let source = RuntimeContext::new_inspect_only(&source_args)?;
    let target = RuntimeContext::new_inspect_only(&cmd.project)?;
    let output = codestory_runtime::rehydrate_cache(codestory_runtime::CacheRehydrateRequest {
        source_project: &source.project_root,
        source_cache_dir: &source.cache_root,
        target_project: &target.project_root,
        target_cache_dir: &target.cache_root,
        dry_run: cmd.dry_run,
    })?;
    let markdown = render_cache_rehydrate_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn render_cache_rehydrate_markdown(output: &codestory_runtime::CacheRehydrateOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Cache Rehydrate");
    let _ = writeln!(markdown, "status: `{}`", output.status);
    if let Some(reason) = output.reason.as_deref() {
        let _ = writeln!(markdown, "reason: {reason}");
    }
    let _ = writeln!(markdown, "source_project: `{}`", output.source_project);
    let _ = writeln!(markdown, "target_project: `{}`", output.target_project);
    let _ = writeln!(markdown, "source_cache: `{}`", output.source_cache_dir);
    let _ = writeln!(markdown, "target_cache: `{}`", output.target_cache_dir);
    if let Some(schema_version) = output.schema_version {
        let _ = writeln!(markdown, "schema_version: `{schema_version}`");
    }
    if let Some(source_file_count) = output.source_file_count {
        let _ = writeln!(markdown, "source_files: `{source_file_count}`");
    }
    let _ = writeln!(markdown, "copied: `{}`", output.copied);
    let _ = writeln!(markdown, "preserved_scope: `{}`", output.preserved_scope);
    let _ = writeln!(
        markdown,
        "invalidated_retrieval_manifests: `{}`",
        output.invalidated_retrieval_manifests
    );
    let _ = writeln!(
        markdown,
        "invalidated_index_artifact_rows: `{}`",
        output.invalidated_index_artifact_rows
    );
    let _ = writeln!(
        markdown,
        "rebased_path_bound_rows: `{}`",
        output.rebased_path_bound_rows
    );
    let _ = writeln!(markdown, "retrieval: {}", output.retrieval);
    let _ = writeln!(markdown, "retrieval_status: `{}`", output.retrieval_status);
    let _ = writeln!(markdown, "retrieval_reason: {}", output.retrieval_reason);
    if let Some(command) = output.retrieval_next_command.as_deref() {
        let _ = writeln!(markdown, "retrieval_next_command: `{command}`");
    }
    if !output.next_commands.is_empty() {
        let _ = writeln!(markdown, "next_commands:");
        for command in &output.next_commands {
            let _ = writeln!(markdown, "- `{command}`");
        }
    }
    markdown
}
