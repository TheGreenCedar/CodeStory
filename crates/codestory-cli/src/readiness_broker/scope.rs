use std::path::Path;

use crate::display;

use super::paths::{clean_path_text, hash_text, install_id, project_id_from_hash};
use super::types::BrokerScope;

pub(crate) const BROKER_SCHEMA_VERSION: u32 = 1;

pub(crate) fn agent_repair_scope(
    project_root: &Path,
    run_id: Option<&str>,
    cli_version: &str,
) -> BrokerScope {
    operation_scope(
        project_root,
        "agent",
        run_id.or(Some(codestory_retrieval::DEFAULT_AGENT_RUN_ID)),
        "agent_repair",
        cli_version,
    )
}

pub(crate) fn operation_scope(
    project_root: &Path,
    profile: &str,
    run_id: Option<&str>,
    operation_kind: &str,
    cli_version: &str,
) -> BrokerScope {
    let install_id = install_id();
    let canonical_root = clean_path_text(project_root);
    let canonical_root_hash = hash_text(&canonical_root);
    let run_id = run_id
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    BrokerScope {
        install_id,
        project_id: project_id_from_hash(&canonical_root_hash),
        canonical_root_hash,
        workspace_root: display::clean_path_string(&project_root.to_string_lossy()),
        profile: profile.to_string(),
        run_id: run_id.clone(),
        agent_id: (profile == "agent").then_some(run_id).flatten(),
        operation_kind: operation_kind.to_string(),
        schema_version: BROKER_SCHEMA_VERSION,
        cli_version: cli_version.to_string(),
    }
}

pub(crate) fn broker_operation_id(scope: &BrokerScope) -> String {
    let run = scope.run_id.as_deref().unwrap_or("none");
    format!(
        "{}:{}:{}:{}",
        scope.operation_kind, scope.project_id, scope.profile, run
    )
}
