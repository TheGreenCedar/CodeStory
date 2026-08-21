use super::cli::MaterializeArgs;
use super::contracts::{
    CohortPathFileV1, EnvironmentIdentityV1, EnvironmentReportV1, MaterializationFreshnessV1,
    OraclePathV1, OracleSourceRangeV1, ProjectMaterializationEvidenceV1,
    QUALIFICATION_REPOSITORIES, QualificationInvocationIdentityV1, QualificationOperationV1,
    QualificationProfileV1, canonical_cohort_path_file_sha256, canonical_corpus_sha256,
    validate_project_file,
};
use super::corpus::LoadedCorpusV1;
use anyhow::{Context, Result, bail};
use codestory_contracts::workspace::SourceIndexPolicy;
use codestory_runtime::{
    RetrievalProcessDefaults, RetrievalRuntimeDefaults, RetrievalRuntimeOverrides, Runtime,
    RuntimeProcessConfig, RuntimeRetrievalConfig, RuntimeRetrievalProfile,
};
use codestory_workspace::{
    WorkspacePathIdentity, WorkspacePathLexicalIdentity, workspace_path_identity,
    workspace_path_lexical_identity,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};

const SOURCE_ENVIRONMENT_SCHEMA: &str = "codestory.proof-availability-source-environment/v1";
const SOURCE_STAGING_OWNER_SCHEMA: &str = "codestory.proof-availability-source-staging-owner/v1";
const MAX_DESCRIPTOR_BYTES: usize = 65_536;
const INDEXED_ENVIRONMENT_SCHEMA: &str = "codestory.proof-availability-operational-environment/v1";
const INDEXED_CACHE_OWNER_SCHEMA: &str = "codestory.proof-availability-cache-owner/v1";
const MAX_INDEXED_DESCRIPTOR_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaterializationFailurePhase {
    BeforePublication,
    AfterPublication,
}

impl fmt::Display for MaterializationFailurePhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BeforePublication => "before_publication",
            Self::AfterPublication => "after_publication",
        })
    }
}

#[derive(Debug)]
struct MaterializationRecoveryError {
    code: &'static str,
    phase: MaterializationFailurePhase,
    staging_recovery_path: PathBuf,
    workspace_recovery_path: Option<PathBuf>,
    output_recovery_path: Option<PathBuf>,
    cause: String,
}

impl fmt::Display for MaterializationRecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} phase={} staging_recovery_path={} workspace_recovery_path={} output_recovery_path={} cause={}",
            self.code,
            self.phase,
            self.staging_recovery_path.display(),
            self.workspace_recovery_path
                .as_deref()
                .map_or_else(|| "none".into(), |path| path.display().to_string()),
            self.output_recovery_path
                .as_deref()
                .map_or_else(|| "none".into(), |path| path.display().to_string()),
            self.cause,
        )
    }
}

impl std::error::Error for MaterializationRecoveryError {}

fn materialization_recovery_error(
    code: &'static str,
    phase: MaterializationFailurePhase,
    staging_recovery_path: &Path,
    workspace_recovery_path: Option<&Path>,
    output_recovery_path: Option<&Path>,
    cause: impl fmt::Display,
) -> anyhow::Error {
    MaterializationRecoveryError {
        code,
        phase,
        staging_recovery_path: staging_recovery_path.to_path_buf(),
        workspace_recovery_path: workspace_recovery_path.map(Path::to_path_buf),
        output_recovery_path: output_recovery_path.map(Path::to_path_buf),
        cause: cause.to_string(),
    }
    .into()
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceStagingOwnerV1 {
    schema: &'static str,
    workspace: String,
    output: String,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceEnvironmentDescriptorV1 {
    schema: &'static str,
    corpus_sha256: String,
    workspace_root: String,
    repositories: Vec<VerifiedSourceRepositoryV1>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct VerifiedSourceRepositoryV1 {
    repository_id: String,
    repository: String,
    commit: String,
    workspace: String,
    checkout_root: String,
    project_root: String,
    source_tree_sha256: String,
    path_file_sha256: String,
    verified_path_count: usize,
    verified_file_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperationalEnvironmentV1 {
    pub(crate) schema: String,
    pub(crate) corpus_sha256: String,
    pub(crate) workspace_root: PathBuf,
    pub(crate) cache_root: PathBuf,
    pub(crate) environment: EnvironmentReportV1,
    pub(crate) repositories: Vec<OperationalRepositoryV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperationalRepositoryV1 {
    pub(crate) repository_id: String,
    pub(crate) checkout_root: PathBuf,
    pub(crate) project_root: PathBuf,
    pub(crate) database_path: PathBuf,
    pub(crate) path_file_sha256: String,
}

#[derive(Debug, Clone)]
struct QualificationBinaryIdentity {
    binary_sha256: String,
    source_commit: String,
    source_tree: String,
    recorded_at: String,
    rust_host: String,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct IndexedCacheOwnerV1<'a> {
    schema: &'static str,
    corpus_sha256: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexedCacheOwnerV1Owned {
    schema: String,
    corpus_sha256: String,
}

#[derive(Debug, Clone)]
struct TreeEntry {
    mode: String,
    kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedDestination {
    real_path: PathBuf,
    lexical_identity: WorkspacePathLexicalIdentity,
    existing_ancestor_identity: WorkspacePathIdentity,
    suffix_identity: WorkspacePathLexicalIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DestinationPlan {
    workspace: ObservedDestination,
    cache: ObservedDestination,
    out: ObservedDestination,
    workspace_parent: ObservedDestination,
    output_parent: ObservedDestination,
}

impl DestinationPlan {
    fn observe(arguments: &MaterializeArgs) -> Result<Self> {
        let workspace = observe_destination(&arguments.workspace)?;
        let cache = observe_destination(&arguments.cache_root)?;
        let out = observe_destination(&arguments.out)?;
        ensure_absent(&workspace.real_path)?;
        ensure_absent(&out.real_path)?;
        if destinations_overlap(&workspace, &cache)
            || destinations_overlap(&workspace, &out)
            || destinations_overlap(&cache, &out)
        {
            bail!("proof_availability_materialize_path_overlap")
        }
        let workspace_parent = observe_existing_directory(
            workspace
                .real_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("proof_availability_workspace_parent_missing"))?,
        )?;
        let output_parent = observe_existing_directory(
            out.real_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("proof_availability_output_parent_missing"))?,
        )?;
        Ok(Self {
            workspace,
            cache,
            out,
            workspace_parent,
            output_parent,
        })
    }

    fn revalidate(&self, arguments: &MaterializeArgs) -> Result<()> {
        let current = Self::observe(arguments)?;
        if current != *self {
            bail!("proof_availability_materialize_destination_changed")
        }
        Ok(())
    }

    fn revalidate_output_parent(&self) -> Result<()> {
        let current = observe_existing_directory(&self.output_parent.real_path)?;
        if current != self.output_parent {
            bail!("proof_availability_materialize_destination_changed")
        }
        ensure_absent(&self.out.real_path)
    }
}

pub fn verify_only(arguments: &MaterializeArgs, loaded: &LoadedCorpusV1) -> Result<()> {
    verify_only_with_registry(arguments, loaded, &QUALIFICATION_REPOSITORIES, false)
}

pub(crate) fn materialize_indexed(
    arguments: &MaterializeArgs,
    loaded: &LoadedCorpusV1,
) -> Result<()> {
    materialize_indexed_with_registry(
        arguments,
        loaded,
        &QUALIFICATION_REPOSITORIES,
        false,
        qualification_binary_identity()?,
    )
}

fn materialize_indexed_with_registry(
    arguments: &MaterializeArgs,
    loaded: &LoadedCorpusV1,
    registry: &[(&str, &str, &str, &str)],
    allow_local_repositories: bool,
    qualification: QualificationBinaryIdentity,
) -> Result<()> {
    if arguments.verify_only {
        bail!("proof_availability_indexed_materialization_rejects_verify_only")
    }
    loaded
        .corpus
        .validate_with_path_files_and_registry(&loaded.path_files, registry)?;
    let destinations = DestinationPlan::observe(arguments)?;
    ensure_absent(&destinations.cache.real_path)?;
    let corpus_sha256 = canonical_corpus_sha256(&loaded.corpus)?;

    let staging = tempfile::Builder::new()
        .prefix(".codestory-proof-indexed-")
        .disable_cleanup(true)
        .tempdir_in(&destinations.workspace_parent.real_path)?;
    let staging_path = staging.path().to_path_buf();
    let execution = (|| -> Result<()> {
        write_private_json_noclobber(
            &staging_path.join("owner.json"),
            &SourceStagingOwnerV1 {
                schema: SOURCE_STAGING_OWNER_SCHEMA,
                workspace: destinations.workspace.real_path.display().to_string(),
                output: destinations.out.real_path.display().to_string(),
            },
        )?;
        let staged_workspaces = staging_path.join("workspaces");
        fs::create_dir(&staged_workspaces)?;
        let hooks = staging_path.join("empty-hooks");
        fs::create_dir(&hooks)?;

        for path_file in &loaded.path_files {
            let cohort = loaded
                .corpus
                .cohorts
                .iter()
                .find(|cohort| cohort.repository_id == path_file.repository_id)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_materialize_cohort_missing"))?;
            if !allow_local_repositories && !path_file.repository.starts_with("https://") {
                bail!("proof_availability_repository_transport_invalid")
            }
            let checkout = staged_workspaces.join(&path_file.repository_id);
            let (tree_digest, tree) =
                stage_repository(&path_file.repository, &path_file.commit, &checkout, &hooks)?;
            if tree_digest != path_file.source_tree_sha256
                || tree_digest != cohort.source_tree_sha256
            {
                bail!("proof_availability_source_tree_mismatch")
            }
            let project_root = resolve_workspace(&checkout, &path_file.workspace)?;
            verify_oracle_sources(path_file, &project_root, &tree)?;
            require_head_and_clean(&checkout, &path_file.commit, &hooks)?;
        }
        destinations.revalidate(arguments)?;
        let staged_identity = workspace_path_identity(&staged_workspaces)?;
        rename_directory_noreplace(&staged_workspaces, &destinations.workspace.real_path)?;
        if workspace_path_identity(&destinations.workspace.real_path)? != staged_identity {
            bail!("proof_availability_materialize_installed_identity_mismatch")
        }

        create_private_directory(&destinations.cache.real_path)?;
        write_private_json_noclobber(
            &destinations.cache.real_path.join("owner.json"),
            &IndexedCacheOwnerV1 {
                schema: INDEXED_CACHE_OWNER_SCHEMA,
                corpus_sha256: &corpus_sha256,
            },
        )?;
        let database_root = destinations.cache.real_path.join("databases");
        create_private_directory(&database_root)?;

        let mut projects = Vec::with_capacity(loaded.path_files.len());
        let mut repositories = Vec::with_capacity(loaded.path_files.len());
        for path_file in &loaded.path_files {
            let checkout_root = destinations
                .workspace
                .real_path
                .join(&path_file.repository_id);
            let project_root = resolve_workspace(&checkout_root, &path_file.workspace)?;
            revalidate_repository(path_file, &checkout_root, &project_root, &hooks)?;
            let database_path = database_root.join(format!("{}.sqlite3", path_file.repository_id));
            let runtime =
                core_only_runtime(&project_root, &destinations.cache.real_path.join("runtime"));
            runtime
                .project_service()
                .open_project_summary_with_storage_path(project_root.clone(), database_path.clone())
                .map_err(|error| anyhow::anyhow!(error.message))?;
            runtime
                .index_service()
                .run_indexing_blocking_without_runtime_refresh(
                    codestory_contracts::api::IndexMode::Full,
                )
                .map_err(|error| anyhow::anyhow!(error.message))?;
            let summary = runtime
                .project_service()
                .open_project_summary_with_storage_path(project_root.clone(), database_path.clone())
                .map_err(|error| anyhow::anyhow!(error.message))?;
            let freshness = match summary.freshness.as_ref().map(|value| value.status) {
                Some(codestory_contracts::api::IndexFreshnessStatusDto::Fresh) => {
                    MaterializationFreshnessV1::Fresh
                }
                Some(codestory_contracts::api::IndexFreshnessStatusDto::Stale) => {
                    MaterializationFreshnessV1::Stale
                }
                None | Some(codestory_contracts::api::IndexFreshnessStatusDto::NotChecked) => {
                    MaterializationFreshnessV1::Missing
                }
            };
            if !matches!(freshness, MaterializationFreshnessV1::Fresh) {
                bail!("proof_availability_materialized_index_not_fresh")
            }
            drop(runtime);
            revalidate_repository(path_file, &checkout_root, &project_root, &hooks)?;

            let store = codestory_store::Store::open_observational(&database_path)
                .context("open materialized proof store observationally")?;
            let publication_before = store
                .get_complete_index_publication()?
                .ok_or_else(|| anyhow::anyhow!("proof_availability_core_publication_missing"))?;
            let schema =
                codestory_store::Store::database_schema_version_observational(&database_path)?;
            if schema != codestory_store::CURRENT_SCHEMA_VERSION {
                bail!("proof_availability_store_schema_mismatch")
            }
            let file_count = u64::try_from(store.get_files()?.len())?;
            let node_count = u64::try_from(store.get_node_count()?)?;
            let edge_count = u64::try_from(store.get_edge_count()?)?;
            let project_identity = codestory_workspace::project_identity_v3(&project_root);
            if store
                .get_retrieval_index_publication(&project_identity.project_id)?
                .is_some()
            {
                bail!("proof_availability_retrieval_publication_forbidden")
            }
            drop(store);
            let database_sha256 = sha256(&fs::read(&database_path)?);
            let store = codestory_store::Store::open_observational(&database_path)
                .context("reopen materialized proof store observationally")?;
            let publication_after = store
                .get_complete_index_publication()?
                .ok_or_else(|| anyhow::anyhow!("proof_availability_core_publication_missing"))?;
            drop(store);
            if publication_before != publication_after {
                bail!("proof_availability_mixed_core_generation")
            }
            projects.push(ProjectMaterializationEvidenceV1 {
                repository_id: path_file.repository_id.clone(),
                source_head: path_file.commit.clone(),
                source_tree: path_file.source_tree_sha256.clone(),
                store_schema: schema.to_string(),
                file_count,
                node_count,
                edge_count,
                freshness,
                database_sha256,
                core_generation: publication_before.generation,
                identity: EnvironmentIdentityV1 {
                    project_id: project_identity.project_id,
                    core_generation_id: publication_before.generation_id,
                    core_run_id: publication_before.run_id,
                },
            });
            repositories.push(OperationalRepositoryV1 {
                repository_id: path_file.repository_id.clone(),
                checkout_root,
                project_root,
                database_path,
                path_file_sha256: canonical_cohort_path_file_sha256(path_file)?,
            });
        }
        projects.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
        repositories.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
        let environment_id = domain_sha256(
            b"codestory.proof-availability-environment/v1\0",
            format!(
                "{}\0{}\0{}\0{}",
                qualification.binary_sha256,
                qualification.source_commit,
                qualification.source_tree,
                corpus_sha256
            )
            .as_bytes(),
        );
        let environment = EnvironmentReportV1 {
            environment_id,
            os: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            rust_host: qualification.rust_host,
            binary_sha256: qualification.binary_sha256,
            qualification_source_commit: qualification.source_commit,
            qualification_source_tree: qualification.source_tree,
            recorded_at: qualification.recorded_at,
            invocation: QualificationInvocationIdentityV1 {
                binary_name: "codestory-proof-availability".to_owned(),
                operation: QualificationOperationV1::Run,
                profile: QualificationProfileV1::LocalCoreOnly,
                corpus_sha256: corpus_sha256.clone(),
                thresholds_sha256: loaded.corpus.thresholds_sha256.clone(),
            },
            projects,
        };
        let descriptor = OperationalEnvironmentV1 {
            schema: INDEXED_ENVIRONMENT_SCHEMA.to_owned(),
            corpus_sha256,
            workspace_root: destinations.workspace.real_path.clone(),
            cache_root: destinations.cache.real_path.clone(),
            environment,
            repositories,
        };
        destinations.revalidate_output_parent()?;
        write_private_json_noclobber(&destinations.out.real_path, &descriptor)?;
        Ok(())
    })();
    execution.map_err(|cause| {
        let workspace_published = fs::symlink_metadata(&destinations.workspace.real_path).is_ok();
        materialization_recovery_error(
            "proof_availability_indexed_materialization_failed",
            if workspace_published {
                MaterializationFailurePhase::AfterPublication
            } else {
                MaterializationFailurePhase::BeforePublication
            },
            &staging_path,
            workspace_published.then_some(destinations.workspace.real_path.as_path()),
            fs::symlink_metadata(&destinations.out.real_path)
                .is_ok()
                .then_some(destinations.out.real_path.as_path()),
            cause,
        )
    })
}

fn verify_only_with_registry(
    arguments: &MaterializeArgs,
    loaded: &LoadedCorpusV1,
    registry: &[(&str, &str, &str, &str)],
    allow_local_repositories: bool,
) -> Result<()> {
    if !arguments.verify_only {
        bail!("proof_availability_source_only_requires_verify_only")
    }
    loaded
        .corpus
        .validate_with_path_files_and_registry(&loaded.path_files, registry)?;
    let destinations = DestinationPlan::observe(arguments)?;

    let staging = tempfile::Builder::new()
        .prefix(".codestory-proof-source-")
        .disable_cleanup(true)
        .tempdir_in(&destinations.workspace_parent.real_path)?;
    let staging_path = staging.path().to_path_buf();
    let owner = SourceStagingOwnerV1 {
        schema: SOURCE_STAGING_OWNER_SCHEMA,
        workspace: destinations.workspace.real_path.display().to_string(),
        output: destinations.out.real_path.display().to_string(),
    };
    let owner_bytes = match serde_json::to_vec_pretty(&owner) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Err(materialization_recovery_error(
                "proof_availability_materialize_prepublication_failed",
                MaterializationFailurePhase::BeforePublication,
                &staging_path,
                None,
                None,
                error,
            ));
        }
    };
    if let Err(error) = fs::write(staging_path.join("owner.json"), owner_bytes) {
        return Err(materialization_recovery_error(
            "proof_availability_materialize_prepublication_failed",
            MaterializationFailurePhase::BeforePublication,
            &staging_path,
            None,
            None,
            error,
        ));
    }

    let preparation = (|| -> Result<(PathBuf, Vec<u8>)> {
        let staged_workspaces = staging_path.join("workspaces");
        fs::create_dir(&staged_workspaces)?;
        let hooks = staging_path.join("empty-hooks");
        fs::create_dir(&hooks)?;

        let mut repositories = Vec::with_capacity(loaded.path_files.len());
        for path_file in &loaded.path_files {
            let cohort = loaded
                .corpus
                .cohorts
                .iter()
                .find(|cohort| cohort.repository_id == path_file.repository_id)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_materialize_cohort_missing"))?;
            if !allow_local_repositories && !path_file.repository.starts_with("https://") {
                bail!("proof_availability_repository_transport_invalid")
            }
            let checkout = staged_workspaces.join(&path_file.repository_id);
            let (tree_digest, tree) =
                stage_repository(&path_file.repository, &path_file.commit, &checkout, &hooks)?;
            if tree_digest != path_file.source_tree_sha256
                || tree_digest != cohort.source_tree_sha256
            {
                bail!("proof_availability_source_tree_mismatch")
            }
            let project_root = resolve_workspace(&checkout, &path_file.workspace)?;
            let verified_file_count = verify_oracle_sources(path_file, &project_root, &tree)?;
            require_head_and_clean(&checkout, &path_file.commit, &hooks)?;
            repositories.push(VerifiedSourceRepositoryV1 {
                repository_id: path_file.repository_id.clone(),
                repository: path_file.repository.clone(),
                commit: path_file.commit.clone(),
                workspace: path_file.workspace.clone(),
                checkout_root: destinations
                    .workspace
                    .real_path
                    .join(&path_file.repository_id)
                    .display()
                    .to_string(),
                project_root: destinations
                    .workspace
                    .real_path
                    .join(&path_file.repository_id)
                    .join(workspace_suffix(&path_file.workspace)?)
                    .display()
                    .to_string(),
                source_tree_sha256: tree_digest,
                path_file_sha256: canonical_cohort_path_file_sha256(path_file)?,
                verified_path_count: path_file.paths.len(),
                verified_file_count,
            });
        }
        repositories.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
        let descriptor = SourceEnvironmentDescriptorV1 {
            schema: SOURCE_ENVIRONMENT_SCHEMA,
            corpus_sha256: canonical_corpus_sha256(&loaded.corpus)?,
            workspace_root: destinations.workspace.real_path.display().to_string(),
            repositories,
        };
        let bytes = serde_json::to_vec_pretty(&descriptor)?;
        if bytes.len() > MAX_DESCRIPTOR_BYTES {
            bail!("proof_availability_source_descriptor_too_large")
        }
        destinations.revalidate(arguments)?;
        Ok((staged_workspaces, bytes))
    })();
    let (staged_workspaces, bytes) = match preparation {
        Ok(prepared) => prepared,
        Err(error) => {
            return Err(materialization_recovery_error(
                "proof_availability_materialize_prepublication_failed",
                MaterializationFailurePhase::BeforePublication,
                &staging_path,
                None,
                None,
                error,
            ));
        }
    };

    let mut output = match tempfile::Builder::new()
        .prefix(".codestory-proof-source-descriptor-")
        .disable_cleanup(true)
        .tempfile_in(&destinations.output_parent.real_path)
    {
        Ok(output) => output,
        Err(error) => {
            return Err(materialization_recovery_error(
                "proof_availability_materialize_prepublication_failed",
                MaterializationFailurePhase::BeforePublication,
                &staging_path,
                None,
                None,
                error,
            ));
        }
    };
    let output_recovery_path = output.path().to_path_buf();
    let prepare_output = (|| -> Result<WorkspacePathIdentity> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        output.write_all(&bytes)?;
        output.write_all(b"\n")?;
        output.as_file_mut().sync_all()?;

        destinations.revalidate(arguments)?;
        Ok(workspace_path_identity(&staged_workspaces)?)
    })();
    let installed_workspace_identity = match prepare_output {
        Ok(identity) => identity,
        Err(error) => {
            return Err(materialization_recovery_error(
                "proof_availability_materialize_prepublication_failed",
                MaterializationFailurePhase::BeforePublication,
                &staging_path,
                None,
                Some(&output_recovery_path),
                error,
            ));
        }
    };
    if let Err(error) =
        rename_directory_noreplace(&staged_workspaces, &destinations.workspace.real_path)
    {
        return Err(materialization_recovery_error(
            "proof_availability_materialize_publication_failed",
            MaterializationFailurePhase::BeforePublication,
            &staging_path,
            None,
            Some(&output_recovery_path),
            error,
        ));
    }
    match workspace_path_identity(&destinations.workspace.real_path) {
        Ok(identity) if identity == installed_workspace_identity => {}
        Ok(_) => {
            return Err(materialization_recovery_error(
                "proof_availability_materialize_installed_identity_mismatch",
                MaterializationFailurePhase::AfterPublication,
                &staging_path,
                Some(&destinations.workspace.real_path),
                Some(&output_recovery_path),
                "published workspace identity differs from the staged workspace",
            ));
        }
        Err(error) => {
            return Err(materialization_recovery_error(
                "proof_availability_materialize_installed_identity_unavailable",
                MaterializationFailurePhase::AfterPublication,
                &staging_path,
                Some(&destinations.workspace.real_path),
                Some(&output_recovery_path),
                error,
            ));
        }
    }
    if let Err(error) = destinations.revalidate_output_parent() {
        return Err(materialization_recovery_error(
            "proof_availability_materialize_output_parent_changed",
            MaterializationFailurePhase::AfterPublication,
            &staging_path,
            Some(&destinations.workspace.real_path),
            Some(&output_recovery_path),
            error,
        ));
    }
    if let Err(error) = output.persist_noclobber(&destinations.out.real_path) {
        return Err(materialization_recovery_error(
            "proof_availability_materialize_output_persist_failed",
            MaterializationFailurePhase::AfterPublication,
            &staging_path,
            Some(&destinations.workspace.real_path),
            Some(error.file.path()),
            error.error,
        ));
    }
    Ok(())
}

fn ensure_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!("proof_availability_output_exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn load_operational_environment(path: &Path) -> Result<OperationalEnvironmentV1> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("proof_availability_operational_environment_not_regular")
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("proof_availability_operational_environment_not_private")
        }
    }
    let bytes = fs::read(path).context("read proof availability operational environment")?;
    if bytes.len() > MAX_INDEXED_DESCRIPTOR_BYTES {
        bail!("proof_availability_operational_environment_too_large")
    }
    let descriptor: OperationalEnvironmentV1 = serde_json::from_slice(&bytes)
        .context("parse proof availability operational environment")?;
    if descriptor.schema != INDEXED_ENVIRONMENT_SCHEMA {
        bail!("proof_availability_operational_environment_schema_mismatch")
    }
    if !descriptor.workspace_root.is_absolute()
        || !descriptor.cache_root.is_absolute()
        || descriptor.repositories.iter().any(|repository| {
            !repository.checkout_root.is_absolute()
                || !repository.project_root.is_absolute()
                || !repository.database_path.is_absolute()
        })
    {
        bail!("proof_availability_operational_environment_path_invalid")
    }
    Ok(descriptor)
}

pub(crate) fn validate_operational_environment(
    loaded: &LoadedCorpusV1,
    descriptor: &OperationalEnvironmentV1,
) -> Result<()> {
    validate_operational_environment_with_identity(
        loaded,
        descriptor,
        &qualification_binary_identity()?,
        &QUALIFICATION_REPOSITORIES,
    )
}

fn validate_operational_environment_with_identity(
    loaded: &LoadedCorpusV1,
    descriptor: &OperationalEnvironmentV1,
    qualification: &QualificationBinaryIdentity,
    registry: &[(&str, &str, &str, &str)],
) -> Result<()> {
    loaded
        .corpus
        .validate_with_path_files_and_registry(&loaded.path_files, registry)?;
    let corpus_sha256 = canonical_corpus_sha256(&loaded.corpus)?;
    if descriptor.corpus_sha256 != corpus_sha256
        || descriptor.environment.invocation.corpus_sha256 != corpus_sha256
        || descriptor.repositories.len() != loaded.path_files.len()
        || descriptor.environment.projects.len() != loaded.path_files.len()
    {
        bail!("proof_availability_operational_environment_binding_invalid")
    }
    let workspace_identity =
        workspace_path_lexical_identity(&descriptor.workspace_root.canonicalize()?)?;
    let cache_identity = workspace_path_lexical_identity(&descriptor.cache_root.canonicalize()?)?;
    let cache_owner: IndexedCacheOwnerV1Owned =
        serde_json::from_slice(&fs::read(descriptor.cache_root.join("owner.json"))?)?;
    if cache_owner.schema != INDEXED_CACHE_OWNER_SCHEMA
        || cache_owner.corpus_sha256 != corpus_sha256
    {
        bail!("proof_availability_cache_owner_mismatch")
    }
    if descriptor.environment.binary_sha256 != qualification.binary_sha256
        || descriptor.environment.qualification_source_commit != qualification.source_commit
        || descriptor.environment.qualification_source_tree != qualification.source_tree
    {
        bail!("proof_availability_qualification_binary_mismatch")
    }
    for path_file in &loaded.path_files {
        let repository = descriptor
            .repositories
            .iter()
            .find(|repository| repository.repository_id == path_file.repository_id)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_operational_repository_missing"))?;
        let project = descriptor
            .environment
            .projects
            .iter()
            .find(|project| project.repository_id == path_file.repository_id)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_environment_project_missing"))?;
        if repository.path_file_sha256 != canonical_cohort_path_file_sha256(path_file)?
            || project.source_head != path_file.commit
            || project.source_tree != path_file.source_tree_sha256
            || !matches!(project.freshness, MaterializationFreshnessV1::Fresh)
        {
            bail!("proof_availability_operational_repository_binding_invalid")
        }
        if !workspace_path_lexical_identity(&repository.checkout_root.canonicalize()?)?
            .is_within(&workspace_identity)
            || !workspace_path_lexical_identity(&repository.project_root.canonicalize()?)?
                .is_within(&workspace_identity)
            || !workspace_path_lexical_identity(&repository.database_path.canonicalize()?)?
                .is_within(&cache_identity)
        {
            bail!("proof_availability_operational_repository_path_invalid")
        }
        revalidate_case_source(
            path_file,
            &repository.checkout_root,
            &repository.project_root,
        )?;
        if sha256(&fs::read(&repository.database_path)?) != project.database_sha256 {
            bail!("proof_availability_database_mismatch")
        }
        let schema = codestory_store::Store::database_schema_version_observational(
            &repository.database_path,
        )?;
        if schema.to_string() != project.store_schema
            || schema != codestory_store::CURRENT_SCHEMA_VERSION
        {
            bail!("proof_availability_store_schema_mismatch")
        }
        let store = codestory_store::Store::open_observational(&repository.database_path)?;
        let publication = store
            .get_complete_index_publication()?
            .ok_or_else(|| anyhow::anyhow!("proof_availability_core_publication_missing"))?;
        if publication.generation != project.core_generation
            || publication.generation_id != project.identity.core_generation_id
            || publication.run_id != project.identity.core_run_id
            || codestory_workspace::project_identity_v3(&repository.project_root).project_id
                != project.identity.project_id
            || store.get_files()?.len() as u64 != project.file_count
            || store.get_node_count()? as u64 != project.node_count
            || store.get_edge_count()? as u64 != project.edge_count
        {
            bail!("proof_availability_core_publication_mismatch")
        }
    }
    Ok(())
}

pub(crate) fn core_only_runtime(project_root: &Path, cache_root: &Path) -> Runtime {
    let defaults = RetrievalProcessDefaults::new(
        cache_root.to_path_buf(),
        RetrievalRuntimeDefaults::default(),
    );
    let retrieval = RuntimeRetrievalConfig::for_project_profile_with_process_defaults(
        Some(project_root),
        RuntimeRetrievalProfile::Local,
        None,
        &defaults,
        &RetrievalRuntimeOverrides::default(),
    );
    Runtime::new_with_process_config(RuntimeProcessConfig::new_with_retrieval_config(
        retrieval,
        SourceIndexPolicy::default(),
    ))
}

pub(crate) fn revalidate_case_source(
    path_file: &CohortPathFileV1,
    checkout_root: &Path,
    project_root: &Path,
) -> Result<()> {
    let hooks = checkout_root
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join(".qualification-empty-hooks"))
        .ok_or_else(|| anyhow::anyhow!("proof_availability_checkout_layout_invalid"))?;
    if !hooks.exists() {
        fs::create_dir_all(&hooks)?;
    }
    revalidate_repository(path_file, checkout_root, project_root, &hooks)
}

fn revalidate_repository(
    path_file: &CohortPathFileV1,
    checkout_root: &Path,
    project_root: &Path,
    hooks: &Path,
) -> Result<()> {
    require_head_and_clean(checkout_root, &path_file.commit, hooks)?;
    let raw_tree = git(
        Some(checkout_root),
        hooks,
        ["ls-tree", "-r", "-z", "--full-tree", &path_file.commit],
    )?
    .stdout;
    if sha256(&raw_tree) != path_file.source_tree_sha256 {
        bail!("proof_availability_source_tree_mismatch")
    }
    let tree = parse_tree(&raw_tree)?;
    let resolved = resolve_workspace(checkout_root, &path_file.workspace)?;
    if workspace_path_identity(&resolved)? != workspace_path_identity(project_root)? {
        bail!("proof_availability_project_identity_changed")
    }
    verify_oracle_sources(path_file, project_root, &tree)?;
    Ok(())
}

fn qualification_binary_identity() -> Result<QualificationBinaryIdentity> {
    let executable = std::env::current_exe()?.canonicalize()?;
    let binary_sha256 = sha256(&fs::read(executable)?);
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("proof_availability_repository_root_missing"))?;
    let hooks = tempfile::tempdir()?;
    let source_commit = String::from_utf8(
        git(
            Some(repository),
            hooks.path(),
            ["rev-parse", "HEAD^{commit}"],
        )?
        .stdout,
    )?
    .trim()
    .to_owned();
    let source_tree = String::from_utf8(
        git(Some(repository), hooks.path(), ["rev-parse", "HEAD^{tree}"])?.stdout,
    )?
    .trim()
    .to_owned();
    require_head_and_clean(repository, &source_commit, hooks.path())?;
    let rustc = Command::new("rustc")
        .args(["--version", "--verbose"])
        .output()?;
    if !rustc.status.success() {
        bail!("proof_availability_rustc_identity_unavailable")
    }
    let rust_host = String::from_utf8(rustc.stdout)?
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or_else(|| anyhow::anyhow!("proof_availability_rust_host_missing"))?
        .to_owned();
    let date = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()?;
    if !date.status.success() {
        bail!("proof_availability_recorded_at_unavailable")
    }
    Ok(QualificationBinaryIdentity {
        binary_sha256,
        source_commit,
        source_tree,
        recorded_at: String::from_utf8(date.stdout)?.trim().to_owned(),
        rust_host,
    })
}

fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private_json_noclobber<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_INDEXED_DESCRIPTOR_BYTES {
        bail!("proof_availability_operational_environment_too_large")
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("proof_availability_output_parent_missing"))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".proof-availability-output-")
        .tempfile_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(&bytes)?;
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    sync_parent(parent)?;
    Ok(())
}

fn sync_parent(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

fn domain_sha256(domain: &[u8], bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn observe_existing_directory(path: &Path) -> Result<ObservedDestination> {
    let observation = observe_destination(path)?;
    if !fs::metadata(&observation.real_path)?.is_dir() {
        bail!("proof_availability_materialize_parent_missing")
    }
    Ok(observation)
}

fn observe_destination(path: &Path) -> Result<ObservedDestination> {
    let normalized = normalized_absolute_destination(path)?;
    let mut current = PathBuf::new();
    let mut existing_ancestor = None;
    let mut missing_seen = false;
    for component in normalized.components() {
        current.push(component.as_os_str());
        if missing_seen {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() && !is_platform_root_alias(&current) {
                    bail!("proof_availability_materialize_destination_alias")
                }
                existing_ancestor = Some(current.clone());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing_seen = true;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let existing_ancestor = existing_ancestor
        .ok_or_else(|| anyhow::anyhow!("proof_availability_materialize_ancestor_missing"))?;
    let suffix = normalized.strip_prefix(&existing_ancestor)?.to_path_buf();
    let canonical_ancestor = existing_ancestor.canonicalize()?;
    let real_path = canonical_ancestor.join(&suffix);
    Ok(ObservedDestination {
        lexical_identity: workspace_path_lexical_identity(&real_path)?,
        existing_ancestor_identity: workspace_path_identity(&canonical_ancestor)?,
        suffix_identity: workspace_path_lexical_identity(&suffix)?,
        real_path,
    })
}

fn normalized_absolute_destination(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        bail!("proof_availability_materialize_destination_invalid")
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                bail!("proof_availability_materialize_destination_parent_component")
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    if !normalized.is_absolute() {
        bail!("proof_availability_materialize_destination_invalid")
    }
    Ok(normalized)
}

fn is_platform_root_alias(path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| parent.parent().is_none())
}

fn destinations_overlap(left: &ObservedDestination, right: &ObservedDestination) -> bool {
    left.lexical_identity.is_within(&right.lexical_identity)
        || right.lexical_identity.is_within(&left.lexical_identity)
        || (left.existing_ancestor_identity == right.existing_ancestor_identity
            && (left.suffix_identity.is_within(&right.suffix_identity)
                || right.suffix_identity.is_within(&left.suffix_identity)))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_directory_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("rename source contains NUL"))?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("rename target contains NUL"))?;
    // SAFETY: both paths are live NUL-terminated byte strings. RENAME_NOREPLACE
    // makes destination nonexistence part of the atomic filesystem operation.
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn rename_directory_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("rename source contains NUL"))?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("rename target contains NUL"))?;
    // SAFETY: both paths are live NUL-terminated byte strings. RENAME_EXCL
    // makes destination nonexistence part of the atomic filesystem operation.
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn rename_directory_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    // std::fs::rename uses MoveFileExW without MOVEFILE_REPLACE_EXISTING, so a
    // destination that appears after observation makes the operation fail.
    fs::rename(from, to)
}

#[cfg(all(
    unix,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))
))]
fn rename_directory_noreplace(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace directory rename is unsupported on this platform",
    ))
}

fn stage_repository(
    fetch: &str,
    commit: &str,
    checkout: &Path,
    hooks: &Path,
) -> Result<(String, BTreeMap<String, TreeEntry>)> {
    git(
        None,
        hooks,
        [
            "init",
            "--quiet",
            checkout.to_str().context("checkout UTF-8")?,
        ],
    )?;
    git(Some(checkout), hooks, ["remote", "add", "origin", fetch])?;
    git(
        Some(checkout),
        hooks,
        [
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-recurse-submodules",
            "--depth=1",
            "origin",
            commit,
        ],
    )?;
    let raw_tree = git(
        Some(checkout),
        hooks,
        ["ls-tree", "-r", "-z", "--full-tree", commit],
    )?
    .stdout;
    let tree_digest = sha256(&raw_tree);
    let tree = parse_tree(&raw_tree)?;
    git(
        Some(checkout),
        hooks,
        ["checkout", "--quiet", "--detach", commit],
    )?;
    require_head_and_clean(checkout, commit, hooks)?;
    Ok((tree_digest, tree))
}

fn git<const N: usize>(cwd: Option<&Path>, hooks: &Path, args: [&str; N]) -> Result<Output> {
    let mut command = Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", hooks.join("empty-global.gitconfig"))
        .env("GIT_ASKPASS", "")
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.display()))
        .args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().context("execute isolated git command")?;
    if !output.status.success() {
        bail!(
            "proof_availability_git_failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(output)
}

fn require_head_and_clean(checkout: &Path, commit: &str, hooks: &Path) -> Result<()> {
    let head = git(
        Some(checkout),
        hooks,
        ["rev-parse", "--verify", "HEAD^{commit}"],
    )?;
    if String::from_utf8(head.stdout)?.trim() != commit {
        bail!("proof_availability_checkout_head_mismatch")
    }
    let status = git(
        Some(checkout),
        hooks,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    if !status.stdout.is_empty() {
        bail!("proof_availability_checkout_dirty")
    }
    Ok(())
}

fn parse_tree(raw: &[u8]) -> Result<BTreeMap<String, TreeEntry>> {
    let mut tree = BTreeMap::new();
    for record in raw
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| anyhow::anyhow!("proof_availability_git_tree_invalid"))?;
        let header = std::str::from_utf8(&record[..tab])?;
        let path = std::str::from_utf8(&record[tab + 1..])?.to_owned();
        let mut fields = header.split_ascii_whitespace();
        let mode = fields.next().unwrap_or_default().to_owned();
        let kind = fields.next().unwrap_or_default().to_owned();
        if fields.next().is_none() || fields.next().is_some() {
            bail!("proof_availability_git_tree_invalid")
        }
        if path == ".gitmodules"
            || path.ends_with("/.gitmodules")
            || mode == "160000"
            || kind == "commit"
        {
            bail!("proof_availability_submodule_forbidden")
        }
        if tree.insert(path, TreeEntry { mode, kind }).is_some() {
            bail!("proof_availability_git_tree_duplicate")
        }
    }
    Ok(tree)
}

fn resolve_workspace(checkout: &Path, workspace: &str) -> Result<PathBuf> {
    let suffix = workspace_suffix(workspace)?;
    reject_symlink_components(checkout, &suffix)?;
    let root = checkout.join(suffix);
    let canonical_checkout = checkout.canonicalize()?;
    let canonical_root = root.canonicalize()?;
    if !canonical_root.starts_with(&canonical_checkout) || !canonical_root.is_dir() {
        bail!("proof_availability_workspace_escape")
    }
    Ok(canonical_root)
}

fn workspace_suffix(workspace: &str) -> Result<PathBuf> {
    if workspace == "." {
        return Ok(PathBuf::new());
    }
    let components = parse_project_path(workspace)?;
    Ok(components.into_iter().collect())
}

fn parse_project_path(path: &str) -> Result<Vec<String>> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains("//")
        || path.contains('\\')
        || path.contains('\0')
        || path.as_bytes().get(1) == Some(&b':')
    {
        bail!("proof_availability_project_path_invalid")
    }
    let components = path.split('/').map(ToOwned::to_owned).collect::<Vec<_>>();
    validate_project_file(&components)?;
    Ok(components)
}

fn verify_oracle_sources(
    path_file: &CohortPathFileV1,
    project_root: &Path,
    tree: &BTreeMap<String, TreeEntry>,
) -> Result<usize> {
    let mut ranges_by_path = BTreeMap::<String, Vec<&OracleSourceRangeV1>>::new();
    for path in &path_file.paths {
        collect_ranges(path, &mut ranges_by_path);
    }
    let mut source_bytes = BTreeMap::new();
    for (relative, ranges) in &ranges_by_path {
        let components = parse_project_path(relative)?;
        reject_symlink_components(project_root, &components.iter().collect::<PathBuf>())?;
        let source_path = project_root.join(components.iter().collect::<PathBuf>());
        let project_relative = if path_file.workspace == "." {
            relative.clone()
        } else {
            format!("{}/{}", path_file.workspace, relative)
        };
        let entry = tree
            .get(&project_relative)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_source_not_tracked"))?;
        if entry.kind != "blob" || !matches!(entry.mode.as_str(), "100644" | "100755") {
            bail!("proof_availability_source_not_regular")
        }
        let canonical = source_path.canonicalize()?;
        if !canonical.starts_with(project_root) || !canonical.is_file() {
            bail!("proof_availability_source_escape")
        }
        let bytes = fs::read(&canonical)?;
        std::str::from_utf8(&bytes).context("proof_availability_source_invalid_utf8")?;
        for range in ranges {
            validate_source_range(range, &bytes)?;
        }
        source_bytes.insert(relative.clone(), bytes);
    }
    for path in &path_file.paths {
        for step in &path.oracle_steps {
            let bytes = source_bytes
                .get(&step.callsite_expression.path)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_source_buffer_missing"))?;
            validate_line_binding(
                path,
                step.callsite_line,
                &step.callsite_expression,
                &step.receipt_line_window,
                bytes,
            )?;
        }
    }
    Ok(ranges_by_path.len())
}

fn reject_symlink_components(root: &Path, suffix: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    for component in suffix.components() {
        current.push(component);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            bail!("proof_availability_source_symlink_forbidden")
        }
    }
    Ok(())
}

fn collect_ranges<'a>(
    path: &'a OraclePathV1,
    ranges: &mut BTreeMap<String, Vec<&'a OracleSourceRangeV1>>,
) {
    let mut push = |range: &'a OracleSourceRangeV1| {
        ranges.entry(range.path.clone()).or_default().push(range);
    };
    for step in &path.oracle_steps {
        push(&step.caller.range);
        push(&step.callsite_expression);
        push(&step.receipt_line_window);
        push(&step.target.range);
    }
    for mutation in &path.negative_mutations {
        push(&mutation.source_audit.caller.range);
        push(&mutation.source_audit.target.range);
        push(&mutation.source_audit.caller_body);
    }
}

fn validate_source_range(range: &OracleSourceRangeV1, bytes: &[u8]) -> Result<()> {
    if range.file_byte_length != u64::try_from(bytes.len())? {
        bail!("proof_availability_source_length_mismatch")
    }
    let start = usize::try_from(range.start_byte)?;
    let end = usize::try_from(range.end_byte)?;
    if start >= end || end > bytes.len() {
        bail!("proof_availability_source_range_invalid")
    }
    let text = std::str::from_utf8(bytes).context("proof_availability_source_invalid_utf8")?;
    if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
        bail!("proof_availability_source_range_not_utf8")
    }
    if sha256(&bytes[start..end]) != range.sha256 {
        bail!("proof_availability_source_range_hash_mismatch")
    }
    Ok(())
}

fn validate_line_binding(
    _path: &OraclePathV1,
    declared_line: u32,
    expression: &OracleSourceRangeV1,
    line: &OracleSourceRangeV1,
    bytes: &[u8],
) -> Result<()> {
    let start = usize::try_from(expression.start_byte)?;
    let end = usize::try_from(expression.end_byte)?;
    if bytes[start..end].contains(&b'\n') {
        bail!("proof_availability_call_expression_multiline")
    }
    let line_start = bytes[..start]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    let line_end = bytes[end..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |position| end + position + 1);
    let actual_line =
        u32::try_from(bytes[..start].iter().filter(|byte| **byte == b'\n').count() + 1)?;
    if declared_line != actual_line
        || line.start_byte != u64::try_from(line_start)?
        || line.end_byte != u64::try_from(line_end)?
        || expression.start_byte < line.start_byte
        || expression.end_byte > line.end_byte
    {
        bail!("proof_availability_receipt_line_window_mismatch")
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::super::contracts::{
        CORPUS_SCHEMA, CohortV1, CorpusV1, LengthDistributionEntryV1, PATH_FILE_SCHEMA,
    };
    use super::*;
    use serde_json::{Value, json};

    fn run_git(cwd: &Path, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn local_repository() -> (tempfile::TempDir, String, Vec<u8>) {
        let root = tempfile::tempdir().unwrap();
        run_git(root.path(), &["init", "--quiet"]);
        run_git(root.path(), &["config", "user.name", "Fixture"]);
        run_git(
            root.path(),
            &["config", "user.email", "fixture@example.invalid"],
        );
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(
            root.path().join("src/lib.rs"),
            b"pub fn run() { call(); }\n",
        )
        .unwrap();
        run_git(root.path(), &["add", "src/lib.rs"]);
        run_git(root.path(), &["commit", "--quiet", "-m", "fixture"]);
        let commit = String::from_utf8(run_git(root.path(), &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let tree = run_git(
            root.path(),
            &["ls-tree", "-r", "-z", "--full-tree", &commit],
        );
        (root, commit, tree)
    }

    const SOURCE: &[u8] = b"pub fn caller() { target(); }\n";

    fn cohort_repository() -> (tempfile::TempDir, String, Vec<u8>) {
        let root = tempfile::tempdir().unwrap();
        run_git(root.path(), &["init", "--quiet"]);
        run_git(root.path(), &["config", "user.name", "Fixture"]);
        run_git(
            root.path(),
            &["config", "user.email", "fixture@example.invalid"],
        );
        for area in 0..5 {
            let directory = root.path().join(format!("src/area{area}"));
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("file.rs"), SOURCE).unwrap();
        }
        run_git(root.path(), &["add", "src"]);
        run_git(root.path(), &["commit", "--quiet", "-m", "fixture"]);
        let commit = String::from_utf8(run_git(root.path(), &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let tree = run_git(
            root.path(),
            &["ls-tree", "-r", "-z", "--full-tree", &commit],
        );
        (root, commit, tree)
    }

    fn selector(symbol: &str, path: &str) -> Value {
        json!({
            "kind":"qualified_name",
            "qualified_name":symbol,
            "project_file_components":path.split('/').collect::<Vec<_>>()
        })
    }

    fn source_range(path: &str, start: usize, end: usize) -> Value {
        json!({
            "path":path,
            "start_byte":start,
            "end_byte":end,
            "file_byte_length":SOURCE.len(),
            "sha256":sha256(&SOURCE[start..end]),
        })
    }

    fn declaration(symbol: &str, path: &str, start: usize, end: usize) -> Value {
        json!({
            "symbol":symbol,
            "selector":selector(symbol, path),
            "range":source_range(path, start, end),
        })
    }

    fn oracle_path(repository_id: &str, length: usize, ordinal: usize) -> Value {
        let case_id = format!("{repository_id}-case-{ordinal:02}");
        let path = format!("src/area{}/file.rs", ordinal % 5);
        let start_symbol = format!("{case_id}::start");
        let targets = (0..length)
            .map(|step| format!("{case_id}::target_{step}"))
            .collect::<Vec<_>>();
        let steps = targets
            .iter()
            .map(|target| json!({"target":selector(target, &path)}))
            .collect::<Vec<_>>();
        let spec = json!({
            "start":selector(&start_symbol, &path),
            "steps":steps,
            "prohibit_traversal_through":[],
            "exclude_from_projection":[],
        });
        let oracle_steps = targets
            .iter()
            .enumerate()
            .map(|(step, target)| {
                let caller = if step == 0 {
                    start_symbol.clone()
                } else {
                    targets[step - 1].clone()
                };
                json!({
                    "caller":declaration(&caller, &path, 0, 13),
                    "callsite_line":1,
                    "callsite_expression":source_range(&path, 18, 26),
                    "receipt_line_window":source_range(&path, 0, SOURCE.len()),
                    "target":declaration(target, &path, 18, 26),
                })
            })
            .collect::<Vec<_>>();
        let mut fields = vec![json!({"kind":"start"})];
        for step in 0..length {
            for kind in ["step_target", "directness", "ordering", "relation"] {
                fields.push(json!({"kind":kind,"step":step}));
            }
        }
        let absent_target = format!("{case_id}::absent_target");
        let absent_source = format!("{case_id}::absent_source");
        let mut target_spec = spec.clone();
        target_spec["steps"][0]["target"] = selector(&absent_target, &path);
        let mut source_spec = spec.clone();
        source_spec["start"] = selector(&absent_source, &path);
        let source_text = "exact direct ordered call path";
        json!({
            "case_id":case_id,
            "language":"rust",
            "source_text":source_text,
            "clauses":[{
                "clause_id":"material",
                "start_byte":0,
                "end_byte_exclusive":source_text.len(),
                "quote":source_text,
                "classification":{"kind":"resolved_material","fields":fields},
            }],
            "spec":spec,
            "oracle_steps":oracle_steps,
            "negative_mutations":[
                {
                    "mutation_id":format!("{case_id}-target"),
                    "path_id":case_id,
                    "kind":"replace_step_target",
                    "step_index":0,
                    "mutated_spec":target_spec,
                    "source_audit":{
                        "caller":declaration(&start_symbol, &path, 0, 13),
                        "target":declaration(&absent_target, &path, 18, 26),
                        "caller_body":source_range(&path, 0, SOURCE.len()),
                        "finding":"no_direct_call",
                    },
                },
                {
                    "mutation_id":format!("{case_id}-source"),
                    "path_id":case_id,
                    "kind":"replace_step_source",
                    "step_index":0,
                    "mutated_spec":source_spec,
                    "source_audit":{
                        "caller":declaration(&absent_source, &path, 0, 13),
                        "target":declaration(&targets[0], &path, 18, 26),
                        "caller_body":source_range(&path, 0, SOURCE.len()),
                        "finding":"no_direct_call",
                    },
                },
            ],
            "audit":{
                "source_area":format!("area-{}", ordinal % 5),
                "curator":"path-curator@example.invalid",
                "reviewer":"path-reviewer@example.invalid",
                "review_date":"2026-08-21",
            },
        })
    }

    fn local_inputs(
        repository: &str,
        commit: &str,
        tree_sha256: &str,
    ) -> (LoadedCorpusV1, Vec<(String, String, String, String)>) {
        let ids = ["codestory-rust", "vite-ts-js", "flask-python", "gin-go"];
        let mut path_files = Vec::new();
        let mut cohorts = Vec::new();
        let mut registry = Vec::new();
        for id in ids {
            let mut ordinal = 0usize;
            let paths = [10usize, 7, 5, 3, 3, 2]
                .into_iter()
                .enumerate()
                .flat_map(|(index, count)| std::iter::repeat_n(index + 1, count))
                .map(|length| {
                    let path = oracle_path(id, length, ordinal);
                    ordinal += 1;
                    path
                })
                .collect::<Vec<_>>();
            let path_file: CohortPathFileV1 = serde_json::from_value(json!({
                "schema":PATH_FILE_SCHEMA,
                "repository_id":id,
                "repository":repository,
                "commit":commit,
                "workspace":".",
                "source_tree_sha256":tree_sha256,
                "curator":"cohort-curator@example.invalid",
                "reviewer":"cohort-reviewer@example.invalid",
                "review_date":"2026-08-21",
                "source_area_requirement":{"kind":"required_at_least_five"},
                "paths":paths,
            }))
            .unwrap();
            let path_file_sha256 = canonical_cohort_path_file_sha256(&path_file).unwrap();
            cohorts.push(CohortV1 {
                repository_id: id.into(),
                repository: repository.into(),
                commit: commit.into(),
                workspace: ".".into(),
                path_file: format!("paths/{id}.json"),
                path_file_sha256,
                source_tree_sha256: tree_sha256.into(),
                path_count: 30,
                positive_step_count: 78,
                path_length_distribution: [10u8, 7, 5, 3, 3, 2]
                    .into_iter()
                    .enumerate()
                    .map(|(index, count)| LengthDistributionEntryV1 {
                        path_length: (index + 1) as u8,
                        path_count: count,
                    })
                    .collect(),
            });
            registry.push((
                id.to_owned(),
                repository.to_owned(),
                commit.to_owned(),
                ".".to_owned(),
            ));
            path_files.push(path_file);
        }
        let corpus = CorpusV1 {
            schema: CORPUS_SCHEMA.into(),
            corpus_id: "proof-availability-v1".into(),
            thresholds_sha256: "a".repeat(64),
            methodology_sha256: "b".repeat(64),
            curator: "corpus-curator@example.invalid".into(),
            reviewer: "corpus-reviewer@example.invalid".into(),
            review_date: "2026-08-21".into(),
            cohorts,
            positive_request_count: 120,
            positive_step_count: 312,
            negative_request_count: 240,
        };
        (LoadedCorpusV1 { corpus, path_files }, registry)
    }

    #[test]
    fn project_paths_reject_root_escape_and_separator_attacks() {
        for invalid in [
            "",
            "/src/lib.rs",
            "src/",
            "src//lib.rs",
            ".",
            "..",
            "src/../lib.rs",
            "src\\lib.rs",
            "C:/src/lib.rs",
            "src/a:b.rs",
            "~/src/lib.rs",
            "src/\0lib.rs",
        ] {
            assert!(parse_project_path(invalid).is_err(), "{invalid:?}");
        }
        assert_eq!(parse_project_path("src/index").unwrap(), ["src", "index"]);
        assert_ne!(
            parse_project_path("src/index").unwrap(),
            parse_project_path("src/indexer").unwrap()
        );
    }

    #[test]
    fn raw_tree_digest_and_submodule_rejection_are_byte_exact() {
        let raw = b"100644 blob aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\tsrc/lib.rs\0";
        assert_eq!(sha256(raw), format!("{:x}", Sha256::digest(raw)));
        assert_eq!(parse_tree(raw).unwrap()["src/lib.rs"].mode, "100644");
        assert!(
            parse_tree(b"160000 commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\tdep\0").is_err()
        );
        assert!(
            parse_tree(b"100644 blob aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\t.gitmodules\0")
                .is_err()
        );
        assert!(
            parse_tree(
                b"100644 blob aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\tnested/.gitmodules\0"
            )
            .is_err()
        );
    }

    #[test]
    fn local_git_checkout_is_detached_exact_and_dirty_states_fail_closed() {
        let (origin, commit, raw_tree) = local_repository();
        let staging = tempfile::tempdir().unwrap();
        let checkout = staging.path().join("checkout");
        let hooks = staging.path().join("hooks");
        fs::create_dir(&hooks).unwrap();
        let (digest, tree) =
            stage_repository(origin.path().to_str().unwrap(), &commit, &checkout, &hooks).unwrap();
        assert_eq!(digest, sha256(&raw_tree));
        assert_eq!(tree["src/lib.rs"].kind, "blob");
        require_head_and_clean(&checkout, &commit, &hooks).unwrap();
        assert!(require_head_and_clean(&checkout, &"0".repeat(40), &hooks).is_err());
        fs::write(checkout.join("src/lib.rs"), b"dirty\n").unwrap();
        assert!(require_head_and_clean(&checkout, &commit, &hooks).is_err());
        run_git(&checkout, &["checkout", "--quiet", "--", "src/lib.rs"]);
        fs::write(checkout.join("untracked.txt"), b"untracked\n").unwrap();
        assert!(require_head_and_clean(&checkout, &commit, &hooks).is_err());
    }

    #[test]
    fn verify_only_materializes_sources_and_retains_typed_recovery_artifacts() {
        let (origin, commit, raw_tree) = cohort_repository();
        let tree_sha256 = sha256(&raw_tree);
        let (loaded, owned_registry) =
            local_inputs(origin.path().to_str().unwrap(), &commit, &tree_sha256);
        let registry = owned_registry
            .iter()
            .map(|(id, repository, commit, workspace)| {
                (
                    id.as_str(),
                    repository.as_str(),
                    commit.as_str(),
                    workspace.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let root = tempfile::tempdir().unwrap();
        let arguments = MaterializeArgs {
            corpus: root.path().join("unused-corpus.json"),
            workspace: root.path().join("workspace"),
            cache_root: root.path().join("cache-must-not-exist"),
            out: root.path().join("source-environment.json"),
            verify_only: true,
        };

        verify_only_with_registry(&arguments, &loaded, &registry, true).unwrap();

        assert!(!arguments.cache_root.exists());
        assert!(arguments.workspace.is_dir());
        let retained_staging = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".codestory-proof-source-"))
            })
            .expect("successful run retains its bounded owner-marked staging record");
        let owner: Value =
            serde_json::from_slice(&fs::read(retained_staging.join("owner.json")).unwrap())
                .unwrap();
        assert_eq!(owner["schema"], SOURCE_STAGING_OWNER_SCHEMA);
        assert!(owner["workspace"].as_str().unwrap().ends_with("/workspace"));
        let descriptor: Value = serde_json::from_slice(&fs::read(&arguments.out).unwrap()).unwrap();
        assert_eq!(descriptor["schema"], SOURCE_ENVIRONMENT_SCHEMA);
        assert_eq!(descriptor["repositories"].as_array().unwrap().len(), 4);
        assert_eq!(
            descriptor
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["corpus_sha256", "repositories", "schema", "workspace_root"]
        );
        for repository in descriptor["repositories"].as_array().unwrap() {
            assert_eq!(
                repository
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                [
                    "checkout_root",
                    "commit",
                    "path_file_sha256",
                    "project_root",
                    "repository",
                    "repository_id",
                    "source_tree_sha256",
                    "verified_file_count",
                    "verified_path_count",
                    "workspace",
                ]
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&arguments.out).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let failure_root = tempfile::tempdir().unwrap();
        let (mut invalid, owned_registry) =
            local_inputs(origin.path().to_str().unwrap(), &commit, &tree_sha256);
        invalid.path_files[3].source_tree_sha256 = "c".repeat(64);
        invalid.corpus.cohorts[3].source_tree_sha256 = "c".repeat(64);
        invalid.corpus.cohorts[3].path_file_sha256 =
            canonical_cohort_path_file_sha256(&invalid.path_files[3]).unwrap();
        let registry = owned_registry
            .iter()
            .map(|(id, repository, commit, workspace)| {
                (
                    id.as_str(),
                    repository.as_str(),
                    commit.as_str(),
                    workspace.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let failed = MaterializeArgs {
            corpus: failure_root.path().join("unused-corpus.json"),
            workspace: failure_root.path().join("workspace"),
            cache_root: failure_root.path().join("cache-must-not-exist"),
            out: failure_root.path().join("source-environment.json"),
            verify_only: true,
        };
        let error = verify_only_with_registry(&failed, &invalid, &registry, true)
            .expect_err("failed preparation must retain a typed recovery path");
        let recovery = error
            .downcast_ref::<MaterializationRecoveryError>()
            .expect("typed preparation recovery error");
        assert_eq!(
            recovery.code,
            "proof_availability_materialize_prepublication_failed"
        );
        assert_eq!(
            recovery.phase,
            MaterializationFailurePhase::BeforePublication
        );
        assert!(recovery.staging_recovery_path.join("owner.json").is_file());
        assert!(!failed.workspace.exists());
        assert!(!failed.out.exists());
        assert!(!failed.cache_root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn destination_symlink_parents_and_cache_aliases_fail_before_artifact_creation() {
        use std::os::unix::fs::symlink;

        let (origin, commit, raw_tree) = cohort_repository();
        let tree_sha256 = sha256(&raw_tree);
        for case in ["workspace-parent", "output-parent", "cache-alias"] {
            let (loaded, owned_registry) =
                local_inputs(origin.path().to_str().unwrap(), &commit, &tree_sha256);
            let registry = owned_registry
                .iter()
                .map(|(id, repository, commit, workspace)| {
                    (
                        id.as_str(),
                        repository.as_str(),
                        commit.as_str(),
                        workspace.as_str(),
                    )
                })
                .collect::<Vec<_>>();
            let root = tempfile::tempdir().unwrap();
            let real_workspace_parent = root.path().join("real-workspace-parent");
            let real_output_parent = root.path().join("real-output-parent");
            let real_cache_root = root.path().join("real-cache-root");
            fs::create_dir(&real_workspace_parent).unwrap();
            fs::create_dir(&real_output_parent).unwrap();
            fs::create_dir(&real_cache_root).unwrap();
            let workspace_alias = root.path().join("workspace-parent-alias");
            let output_alias = root.path().join("output-parent-alias");
            let cache_alias = root.path().join("cache-alias");
            symlink(&real_workspace_parent, &workspace_alias).unwrap();
            symlink(&real_output_parent, &output_alias).unwrap();
            symlink(&real_workspace_parent, &cache_alias).unwrap();

            let workspace = if case == "workspace-parent" {
                workspace_alias.join("workspace")
            } else {
                real_workspace_parent.join("workspace")
            };
            let out = if case == "output-parent" {
                output_alias.join("source-environment.json")
            } else {
                real_output_parent.join("source-environment.json")
            };
            let cache_root = if case == "cache-alias" {
                cache_alias.clone()
            } else {
                real_cache_root.clone()
            };
            let arguments = MaterializeArgs {
                corpus: root.path().join("unused-corpus.json"),
                workspace: workspace.clone(),
                cache_root,
                out: out.clone(),
                verify_only: true,
            };

            let error = verify_only_with_registry(&arguments, &loaded, &registry, true)
                .expect_err("destination alias must fail closed");
            assert!(
                format!("{error:#}").contains("proof_availability_materialize_destination_alias"),
                "unexpected {case} error: {error:#}"
            );
            assert!(!workspace.exists(), "{case} left a workspace");
            assert!(!out.exists(), "{case} left a descriptor");
        }
    }

    #[cfg(unix)]
    #[test]
    fn destination_overlap_detects_platform_root_aliases() {
        let root = tempfile::tempdir().unwrap();
        let spelled_root = root.path().to_path_buf();
        let canonical_root = spelled_root.canonicalize().unwrap();
        if spelled_root == canonical_root {
            return;
        }

        let workspace = spelled_root.join("workspace");
        let out = spelled_root.join("source-environment.json");
        let arguments = MaterializeArgs {
            corpus: spelled_root.join("unused-corpus.json"),
            workspace: workspace.clone(),
            cache_root: canonical_root,
            out: out.clone(),
            verify_only: true,
        };

        let error = DestinationPlan::observe(&arguments)
            .expect_err("native aliases must not evade overlap detection");
        assert!(
            format!("{error:#}").contains("proof_availability_materialize_path_overlap"),
            "unexpected alias-overlap error: {error:#}"
        );
        assert!(!workspace.exists());
        assert!(!out.exists());
    }

    #[test]
    fn workspace_publish_is_no_replace_and_leave_safe() {
        let root = tempfile::tempdir().unwrap();
        let staged = root.path().join("staged-workspace");
        let destination = root.path().join("workspace");
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("owned"), b"owned").unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("raced"), b"raced").unwrap();

        rename_directory_noreplace(&staged, &destination)
            .expect_err("workspace publication must not replace a raced destination");
        assert_eq!(fs::read(destination.join("raced")).unwrap(), b"raced");
        assert_eq!(fs::read(staged.join("owned")).unwrap(), b"owned");
    }

    #[test]
    fn post_publication_failure_preserves_replacement_and_owned_workspace() {
        let root = tempfile::tempdir().unwrap();
        let staged = root.path().join("staged-workspace");
        let published = root.path().join("workspace");
        let displaced = root.path().join("displaced-owned-workspace");
        let staging_recovery = root.path().join("staging-recovery");
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("owned"), b"owned").unwrap();
        fs::create_dir(&staging_recovery).unwrap();
        rename_directory_noreplace(&staged, &published).unwrap();
        workspace_path_identity(&published).expect("successful post-publish observation");

        fs::rename(&published, &displaced).unwrap();
        fs::create_dir(&published).unwrap();
        fs::write(published.join("unrelated"), b"unrelated").unwrap();

        let error = materialization_recovery_error(
            "proof_availability_materialize_output_persist_failed",
            MaterializationFailurePhase::AfterPublication,
            &staging_recovery,
            Some(&published),
            None,
            std::io::Error::other("injected output failure"),
        );
        let recovery = error
            .downcast_ref::<MaterializationRecoveryError>()
            .expect("typed recovery failure");
        assert_eq!(
            recovery.code,
            "proof_availability_materialize_output_persist_failed"
        );
        assert_eq!(
            recovery.phase,
            MaterializationFailurePhase::AfterPublication
        );
        assert_eq!(
            recovery.workspace_recovery_path.as_deref(),
            Some(published.as_path())
        );
        assert_eq!(fs::read(published.join("unrelated")).unwrap(), b"unrelated");
        assert_eq!(fs::read(displaced.join("owned")).unwrap(), b"owned");
    }

    #[test]
    fn range_validation_rejects_length_hash_bounds_and_utf8_drift() {
        let bytes = "aéz\n".as_bytes();
        let valid = OracleSourceRangeV1 {
            path: "src/lib.rs".into(),
            start_byte: 1,
            end_byte: 3,
            file_byte_length: bytes.len() as u64,
            sha256: sha256(&bytes[1..3]),
        };
        validate_source_range(&valid, bytes).unwrap();
        let mut invalid = valid.clone();
        invalid.file_byte_length += 1;
        assert!(validate_source_range(&invalid, bytes).is_err());
        let mut invalid = valid.clone();
        invalid.sha256 = "0".repeat(64);
        assert!(validate_source_range(&invalid, bytes).is_err());
        let mut invalid = valid.clone();
        invalid.start_byte = 2;
        assert!(validate_source_range(&invalid, bytes).is_err());
        let mut invalid = valid;
        invalid.end_byte = 99;
        assert!(validate_source_range(&invalid, bytes).is_err());
        let non_utf8 = [0xff, b'\n'];
        let invalid = OracleSourceRangeV1 {
            path: "src/lib.rs".into(),
            start_byte: 0,
            end_byte: 1,
            file_byte_length: 2,
            sha256: sha256(&non_utf8[..1]),
        };
        assert!(validate_source_range(&invalid, &non_utf8).is_err());
    }

    #[test]
    fn existing_and_symlink_outputs_are_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("existing");
        fs::write(&file, b"keep").unwrap();
        assert!(ensure_absent(&file).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = root.path().join("link");
            symlink(&file, &link).unwrap();
            assert!(ensure_absent(&link).is_err());
            let source_root = root.path().join("source-root");
            fs::create_dir(&source_root).unwrap();
            let real = source_root.join("real");
            fs::create_dir(&real).unwrap();
            symlink(&real, source_root.join("linked")).unwrap();
            assert!(reject_symlink_components(&source_root, Path::new("linked/file.rs")).is_err());
        }
        assert!(ensure_absent(&root.path().join("missing")).is_ok());
        assert_eq!(fs::read(file).unwrap(), b"keep");
    }

    #[test]
    fn exact_ranges_and_line_windows_cover_lf_crlf_and_final_lines() {
        for (bytes, expression, line, line_number) in [
            (b"call();\n".as_slice(), (0, 6), (0, 8), 1),
            (b"x\r\ncall();\r\n".as_slice(), (3, 9), (3, 12), 2),
            (b"x\ncall();".as_slice(), (2, 8), (2, 9), 2),
        ] {
            let expression = OracleSourceRangeV1 {
                path: "src/lib.rs".into(),
                start_byte: expression.0,
                end_byte: expression.1,
                file_byte_length: bytes.len() as u64,
                sha256: sha256(&bytes[expression.0 as usize..expression.1 as usize]),
            };
            let window = OracleSourceRangeV1 {
                path: "src/lib.rs".into(),
                start_byte: line.0,
                end_byte: line.1,
                file_byte_length: bytes.len() as u64,
                sha256: sha256(&bytes[line.0 as usize..line.1 as usize]),
            };
            validate_source_range(&expression, bytes).unwrap();
            validate_source_range(&window, bytes).unwrap();
            validate_line_binding_dummy(line_number, &expression, &window, bytes).unwrap();
        }

        let bytes = b"call();\nnext();\n";
        let expression = OracleSourceRangeV1 {
            path: "src/lib.rs".into(),
            start_byte: 0,
            end_byte: 6,
            file_byte_length: bytes.len() as u64,
            sha256: sha256(&bytes[..6]),
        };
        let window = OracleSourceRangeV1 {
            path: "src/lib.rs".into(),
            start_byte: 0,
            end_byte: 8,
            file_byte_length: bytes.len() as u64,
            sha256: sha256(&bytes[..8]),
        };
        assert!(validate_line_binding_dummy(2, &expression, &window, bytes).is_err());
        let mut short_window = window.clone();
        short_window.end_byte -= 1;
        assert!(validate_line_binding_dummy(1, &expression, &short_window, bytes).is_err());
        let mut multiline = expression;
        multiline.end_byte = 10;
        assert!(validate_line_binding_dummy(1, &multiline, &window, bytes).is_err());
    }

    fn validate_line_binding_dummy(
        line: u32,
        expression: &OracleSourceRangeV1,
        window: &OracleSourceRangeV1,
        bytes: &[u8],
    ) -> Result<()> {
        let dummy: OraclePathV1 = serde_json::from_value(serde_json::json!({
            "case_id":"dummy","language":"rust","source_text":"x","clauses":[],
            "spec":{"start":{"kind":"canonical_id","canonical_id":"x"},"steps":[{"target":{"kind":"canonical_id","canonical_id":"y"}}],"prohibit_traversal_through":[],"exclude_from_projection":[]},
            "oracle_steps":[],"negative_mutations":[],"audit":{"source_area":"x","curator":"a","reviewer":"b","review_date":"2026-08-21"}
        }))?;
        validate_line_binding(&dummy, line, expression, window, bytes)
    }

    #[test]
    fn materializer_source_has_no_product_execution_dependency() {
        let source = include_str!("materialize.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        for forbidden in [
            "proof_qualification_support",
            "execute_observed",
            "remove_dir_all",
            "RuntimeRetrievalProfile::Agent",
            "run_observed_call_path_public_operation",
        ] {
            assert!(
                !production.contains(forbidden),
                "forbidden source dependency {forbidden}"
            );
        }
    }

    #[test]
    fn non_verify_materialization_builds_a_fresh_core_only_index() {
        let (origin, commit, raw_tree) = cohort_repository();
        let tree_sha256 = sha256(&raw_tree);
        let (loaded, owned_registry) =
            local_inputs(origin.path().to_str().unwrap(), &commit, &tree_sha256);
        let registry = owned_registry
            .iter()
            .map(|(id, repository, commit, workspace)| {
                (
                    id.as_str(),
                    repository.as_str(),
                    commit.as_str(),
                    workspace.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let root = tempfile::tempdir().unwrap();
        let arguments = MaterializeArgs {
            corpus: root.path().join("unused-corpus.json"),
            workspace: root.path().join("workspace"),
            cache_root: root.path().join("cache"),
            out: root.path().join("environment.json"),
            verify_only: false,
        };
        let qualification = QualificationBinaryIdentity {
            binary_sha256: "1".repeat(64),
            source_commit: "2".repeat(40),
            source_tree: "3".repeat(40),
            recorded_at: "2026-08-21T12:00:00Z".into(),
            rust_host: "aarch64-apple-darwin".into(),
        };

        materialize_indexed_with_registry(
            &arguments,
            &loaded,
            &registry,
            true,
            qualification.clone(),
        )
        .unwrap();

        let descriptor = load_operational_environment(&arguments.out).unwrap();
        validate_operational_environment_with_identity(
            &loaded,
            &descriptor,
            &qualification,
            &registry,
        )
        .unwrap();
        assert_eq!(descriptor.repositories.len(), 4);
        assert_eq!(descriptor.environment.projects.len(), 4);
        assert!(
            descriptor
                .environment
                .projects
                .iter()
                .all(|project| matches!(project.freshness, MaterializationFreshnessV1::Fresh))
        );
        for repository in &descriptor.repositories {
            let store =
                codestory_store::Store::open_observational(&repository.database_path).unwrap();
            assert!(store.get_complete_index_publication().unwrap().is_some());
        }

        let mut wrong_binary = descriptor.clone();
        wrong_binary.environment.binary_sha256 = "f".repeat(64);
        assert!(
            validate_operational_environment_with_identity(
                &loaded,
                &wrong_binary,
                &qualification,
                &registry,
            )
            .unwrap_err()
            .to_string()
            .contains("qualification_binary_mismatch")
        );
        let mut stale = descriptor.clone();
        stale.environment.projects[0].freshness = MaterializationFreshnessV1::Stale;
        assert!(
            validate_operational_environment_with_identity(
                &loaded,
                &stale,
                &qualification,
                &registry,
            )
            .unwrap_err()
            .to_string()
            .contains("repository_binding_invalid")
        );
        let checkout = &descriptor.repositories[0].checkout_root;
        fs::write(checkout.join("src/area0/file.rs"), b"dirty\n").unwrap();
        assert!(
            validate_operational_environment_with_identity(
                &loaded,
                &descriptor,
                &qualification,
                &registry,
            )
            .unwrap_err()
            .to_string()
            .contains("checkout_dirty")
        );
        run_git(
            checkout,
            &["checkout", "--quiet", "--", "src/area0/file.rs"],
        );
        let mut mixed_generation = descriptor.clone();
        mixed_generation.environment.projects[0].core_generation += 1;
        assert!(
            validate_operational_environment_with_identity(
                &loaded,
                &mixed_generation,
                &qualification,
                &registry,
            )
            .unwrap_err()
            .to_string()
            .contains("core_publication_mismatch")
        );
        let database = &descriptor.repositories[0].database_path;
        let mut bytes = fs::read(database).unwrap();
        bytes.push(0);
        fs::write(database, bytes).unwrap();
        assert!(
            validate_operational_environment_with_identity(
                &loaded,
                &descriptor,
                &qualification,
                &registry,
            )
            .unwrap_err()
            .to_string()
            .contains("database_mismatch")
        );
    }
}
