use std::path::Path;

use codestory_workspace::{ProjectIdentityV3, project_identity_v2, project_identity_v3};

use crate::display;

use super::paths::{clean_path_text, hash_text, install_id};
use super::types::BrokerScope;

pub(crate) const BROKER_SCHEMA_VERSION: u32 = 3;
pub(crate) const BROKER_SCHEMA_VERSION_V2: u32 = 2;
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
    let identity = project_identity_v3(project_root);
    let canonical_root_hash = identity.workspace_id.clone();
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

pub(crate) fn effective_scope_identity(scope: &BrokerScope) -> Option<ProjectIdentityV3> {
    effective_record_identity(
        scope.schema_version,
        scope.identity.as_ref(),
        &scope.project_id,
        &scope.canonical_root_hash,
        &scope.workspace_root,
    )
}

pub(crate) fn effective_record_identity(
    schema_version: u32,
    stored_identity: Option<&ProjectIdentityV3>,
    project_id: &str,
    canonical_root_hash: &str,
    workspace_root: &str,
) -> Option<ProjectIdentityV3> {
    let identity = effective_identity(schema_version, stored_identity, workspace_root)?;
    match schema_version {
        BROKER_SCHEMA_VERSION => {
            if project_id != identity.project_id || canonical_root_hash != identity.workspace_id {
                return None;
            }
        }
        BROKER_SCHEMA_VERSION_V2 => {
            let legacy = stored_identity?;
            if project_id != legacy.project_id
                || canonical_root_hash != legacy_canonical_root_hash(workspace_root)?
            {
                return None;
            }
        }
        LEGACY_BROKER_SCHEMA_VERSION => {
            let legacy_hash = legacy_canonical_root_hash(workspace_root)?;
            if stored_identity.is_some()
                || canonical_root_hash != legacy_hash
                || project_id != format!("codestory-{}", &legacy_hash[..16])
            {
                return None;
            }
        }
        _ => return None,
    }
    Some(identity)
}

pub(crate) fn effective_identity(
    schema_version: u32,
    identity: Option<&ProjectIdentityV3>,
    workspace_root: &str,
) -> Option<ProjectIdentityV3> {
    match schema_version {
        BROKER_SCHEMA_VERSION => {
            let identity = identity?.clone();
            identity_v3_matches_workspace_root(&identity, workspace_root).then_some(identity)
        }
        BROKER_SCHEMA_VERSION_V2 => {
            let identity = identity?;
            identity_v2_matches_workspace_root(identity, workspace_root)
                .then(|| identity_from_workspace_root(workspace_root))?
        }
        LEGACY_BROKER_SCHEMA_VERSION => identity_from_workspace_root(workspace_root),
        _ => None,
    }
}

pub(crate) fn identity_from_workspace_root(workspace_root: &str) -> Option<ProjectIdentityV3> {
    let root = Path::new(workspace_root.trim());
    if workspace_root.trim().is_empty() || !root.is_absolute() {
        return None;
    }
    Some(project_identity_v3(root))
}

pub(crate) fn legacy_canonical_root_hash(workspace_root: &str) -> Option<String> {
    let root = Path::new(workspace_root.trim());
    if workspace_root.trim().is_empty() || !root.is_absolute() {
        return None;
    }
    Some(hash_text(&clean_path_text(root)))
}

fn identity_v3_matches_workspace_root(identity: &ProjectIdentityV3, workspace_root: &str) -> bool {
    let root = Path::new(workspace_root.trim());
    if workspace_root.trim().is_empty() || !root.is_absolute() {
        return false;
    }
    let current = project_identity_v3(root);
    identity.project_identity_schema_version
        == codestory_workspace::PROJECT_IDENTITY_V3_SCHEMA_VERSION
        && identity.workspace_id == current.workspace_id
        && identity.canonical_repository_id == current.canonical_repository_id
        && identity.legacy_canonical_repository_id == current.legacy_canonical_repository_id
        && identity_has_consistent_scope(
            &identity.project_id,
            &identity.artifact_scope_id,
            &identity.workspace_id,
            identity.canonical_repository_id.as_deref(),
            identity.portable_reuse_eligible,
        )
}

fn identity_v2_matches_workspace_root(identity: &ProjectIdentityV3, workspace_root: &str) -> bool {
    let root = Path::new(workspace_root.trim());
    if workspace_root.trim().is_empty() || !root.is_absolute() {
        return false;
    }
    let current = project_identity_v2(root);
    identity.project_identity_schema_version == codestory_workspace::PROJECT_IDENTITY_SCHEMA_VERSION
        && identity.legacy_canonical_repository_id.is_none()
        && identity.workspace_id == current.workspace_id
        && identity.canonical_repository_id == current.canonical_repository_id
        && identity_has_consistent_scope(
            &identity.project_id,
            &identity.artifact_scope_id,
            &identity.workspace_id,
            identity.canonical_repository_id.as_deref(),
            identity.portable_reuse_eligible,
        )
}

fn identity_has_consistent_scope(
    project_id: &str,
    artifact_scope_id: &str,
    workspace_id: &str,
    canonical_repository_id: Option<&str>,
    portable_reuse_eligible: bool,
) -> bool {
    let portable_id = canonical_repository_id.unwrap_or(workspace_id);
    let expected = if portable_reuse_eligible {
        portable_id
    } else {
        workspace_id
    };
    project_id == expected && artifact_scope_id == expected
}
