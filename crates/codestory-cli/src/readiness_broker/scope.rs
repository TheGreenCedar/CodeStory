use std::path::Path;

use codestory_workspace::{ProjectIdentityV2, project_identity_v2};

use crate::display;

use super::paths::{clean_path_text, hash_text, install_id};
use super::types::BrokerScope;

pub(crate) const BROKER_SCHEMA_VERSION: u32 = 2;
pub(crate) const LEGACY_BROKER_SCHEMA_VERSION: u32 = 1;

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
    let identity = project_identity_v2(project_root);
    let run_id = run_id
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    BrokerScope {
        project_id: identity.project_id.clone(),
        identity: Some(identity),
        install_id,
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
    let workspace_id = effective_scope_identity(scope)
        .map(|identity| identity.workspace_id)
        .unwrap_or_else(|| "invalid-workspace".to_string());
    format!(
        "{}:{}:{}:{}",
        scope.operation_kind, workspace_id, scope.profile, run
    )
}

pub(crate) fn effective_scope_identity(scope: &BrokerScope) -> Option<ProjectIdentityV2> {
    let identity = effective_identity(
        scope.schema_version,
        scope.identity.as_ref(),
        &scope.workspace_root,
    )?;
    if scope.schema_version == BROKER_SCHEMA_VERSION && scope.project_id != identity.project_id {
        return None;
    }
    Some(identity)
}

pub(crate) fn effective_identity(
    schema_version: u32,
    identity: Option<&ProjectIdentityV2>,
    workspace_root: &str,
) -> Option<ProjectIdentityV2> {
    match schema_version {
        BROKER_SCHEMA_VERSION => {
            let identity = identity?.clone();
            identity_matches_workspace_root(&identity, workspace_root).then_some(identity)
        }
        LEGACY_BROKER_SCHEMA_VERSION => identity_from_workspace_root(workspace_root),
        _ => None,
    }
}

pub(crate) fn identity_from_workspace_root(workspace_root: &str) -> Option<ProjectIdentityV2> {
    let root = Path::new(workspace_root.trim());
    if workspace_root.trim().is_empty() || !root.is_absolute() {
        return None;
    }
    Some(project_identity_v2(root))
}

pub(crate) fn normalized_workspace_root(workspace_root: &str) -> Option<String> {
    let root = Path::new(workspace_root.trim());
    if workspace_root.trim().is_empty() || !root.is_absolute() {
        return None;
    }
    Some(clean_path_text(root))
}

fn identity_matches_workspace_root(identity: &ProjectIdentityV2, workspace_root: &str) -> bool {
    let root = Path::new(workspace_root.trim());
    !workspace_root.trim().is_empty()
        && root.is_absolute()
        && codestory_workspace::workspace_id_for_root(root) == identity.workspace_id
}
