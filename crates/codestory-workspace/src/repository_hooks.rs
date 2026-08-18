//! Effective Git hook resolution and handle-pinned dirty-hook mutation.
//!
//! Repository configuration and hook files are untrusted input. This module
//! resolves the bounded Git configuration stack without invoking Git, keeps
//! repository-controlled configuration behind the metadata reader's captured
//! no-follow proof, and performs all hook access relative to one pinned hooks
//! directory.

use crate::{
    WorkspacePathIdentity, WorkspacePathLexicalIdentity, repo_metadata::MetadataRoots,
    repo_metadata::bytes_to_path, workspace_file_identity, workspace_path_identity,
    workspace_path_lexical_identity,
};
use fs_at::{OpenOptions as AtOpenOptions, OpenOptionsWriteMode};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

const HOOK_NAMES: [&str; 3] = ["post-checkout", "post-merge", "post-rewrite"];
const MANAGED_START: &[u8] = b"# >>> codestory dirty marker >>>";
const MANAGED_END: &[u8] = b"# <<< codestory dirty marker <<<";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_HOOK_BYTES: u64 = 1024 * 1024;
const MAX_ENV_CONFIG_ENTRIES: usize = 64;
const MAX_ENV_CONFIG_KEY_BYTES: usize = 1024;
const MAX_ENV_CONFIG_VALUE_BYTES: usize = 16 * 1024;
const MAX_TRANSACTION_JOURNAL_BYTES: u64 = 64 * 1024;
const TRANSACTION_SCHEMA_VERSION: u32 = 1;
const TRANSACTION_LOCK_NAME: &str = ".codestory-hooks-transaction.lock";
const TRANSACTION_JOURNAL_NAME: &str = ".codestory-hooks-transaction-v1.json";
const TRANSACTION_READY_NAME: &str = ".codestory-hooks-transaction-v1.ready";
const TRANSACTION_COMMIT_NAME: &str = ".codestory-hooks-transaction-v1.commit";
const OWNED_DISPATCHER_MARKER: &[u8] = b"# codestory-owned-dispatcher";

/// Mutation requested by the plugin hook wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryHookAction {
    Install,
    Uninstall,
    Status,
}

/// Exact invocation embedded in each managed dispatcher.
#[derive(Debug, Clone)]
pub struct RepositoryHookRequest {
    pub action: RepositoryHookAction,
    pub project_root: PathBuf,
    pub plugin_data_dir: PathBuf,
    pub node_path: PathBuf,
    pub script_path: PathBuf,
}

/// One hook target's typed state.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepositoryHookTargetReport {
    pub hook: String,
    pub state: String,
    pub path: PathBuf,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Machine-readable result returned through the hidden CLI seam.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepositoryHookReport {
    pub schema_version: u32,
    pub status: String,
    pub project_root: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks_path: Option<PathBuf>,
    pub hooks: Vec<RepositoryHookTargetReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug)]
struct HookFailure {
    code: &'static str,
    message: String,
    project_root: PathBuf,
    hooks_path: Option<PathBuf>,
}

impl HookFailure {
    fn new(
        code: &'static str,
        message: impl Into<String>,
        project_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            project_root: project_root.into(),
            hooks_path: None,
        }
    }

    fn at_hooks_path(mut self, hooks_path: &Path) -> Self {
        self.hooks_path = Some(hooks_path.to_path_buf());
        self
    }

    fn report(self) -> RepositoryHookReport {
        RepositoryHookReport {
            schema_version: 1,
            status: self.code.to_string(),
            project_root: self.project_root,
            hooks_path: self.hooks_path,
            hooks: Vec::new(),
            message: Some(self.message),
        }
    }
}

type HookResult<T> = Result<T, HookFailure>;

/// Resolve and optionally mutate the effective Git hook trio.
///
/// Operational refusals are returned as typed reports. This keeps the wrapper
/// observational for `status` and lets callers distinguish an absent hook from
/// an unprovable or unsafe destination.
pub fn manage_repository_hooks(request: &RepositoryHookRequest) -> RepositoryHookReport {
    let environment = match HookConfigEnvironment::capture() {
        Ok(environment) => environment,
        Err(message) => {
            return HookFailure::new("hooks_config_unresolved", message, &request.project_root)
                .report();
        }
    };
    manage_repository_hooks_inner(request, &environment).unwrap_or_else(HookFailure::report)
}

fn manage_repository_hooks_inner(
    request: &RepositoryHookRequest,
    environment: &HookConfigEnvironment,
) -> HookResult<RepositoryHookReport> {
    let roots = MetadataRoots::resolve(&request.project_root).map_err(|error| {
        let code = if request.project_root.join(".git").exists() {
            "hooks_config_unresolved"
        } else {
            "not_a_git_repository"
        };
        HookFailure::new(
            code,
            format!("repository metadata is unavailable: {error:#}"),
            &request.project_root,
        )
    })?;
    let project_root = roots.root.clone();
    validate_invocation(request, &project_root)?;
    roots.validate_hook_metadata().map_err(|error| {
        HookFailure::new(
            "hooks_config_unresolved",
            format!("repository configuration changed or is unsafe: {error:#}"),
            &project_root,
        )
    })?;

    let effective = EffectiveHooksPath::resolve(&roots, environment)?;
    let mut pinned = effective.pin_directory()?;

    roots.validate_hook_metadata().map_err(|error| {
        HookFailure::new(
            "hooks_config_unresolved",
            format!("repository configuration changed before hook inspection: {error:#}"),
            &project_root,
        )
        .at_hooks_path(&effective.path)
    })?;
    effective
        .config_capture
        .validate(&project_root, &effective.path)?;
    effective.validate_root_identities()?;

    let _transaction_lock = if request.action == RepositoryHookAction::Status {
        if transaction_recovery_required(&pinned.handle).map_err(|error| {
            HookFailure::new(
                "hook_recovery_required",
                format!("inspect hook transaction state: {error}"),
                &project_root,
            )
            .at_hooks_path(&effective.path)
        })? {
            return Err(HookFailure::new(
                "hook_recovery_required",
                "an interrupted hook transaction requires recovery by install or uninstall",
                &project_root,
            )
            .at_hooks_path(&effective.path));
        }
        None
    } else {
        let lock = acquire_transaction_lock(&pinned.handle).map_err(|error| {
            HookFailure::new(
                "hook_mutation_failed",
                format!("acquire hook transaction lock: {error}"),
                &project_root,
            )
            .at_hooks_path(&effective.path)
        })?;
        #[cfg(test)]
        run_after_transaction_lock_hook();
        validate_transaction_lock_binding(&pinned.handle, &lock).map_err(|error| {
            HookFailure::new(
                "hook_mutation_failed",
                format!("validate hook transaction lock name: {error}"),
                &project_root,
            )
            .at_hooks_path(&effective.path)
        })?;
        recover_pending_transaction(&pinned.handle, &lock).map_err(|error| {
            HookFailure::new(
                "hook_recovery_required",
                format!("recover interrupted hook transaction: {error}"),
                &project_root,
            )
            .at_hooks_path(&effective.path)
        })?;
        Some(lock)
    };

    let invocation = HookInvocation::from_request(request, &project_root)?;
    let mut targets = HOOK_NAMES
        .iter()
        .map(|name| preflight_target(&mut pinned, name, request.action, &invocation))
        .collect::<Vec<_>>();

    if let Some((failure_code, failure_message)) = targets
        .iter()
        .find_map(|target| target.as_ref().err())
        .map(|failure| (failure.code, failure.message.clone()))
    {
        let hooks = targets
            .drain(..)
            .map(|target| match target {
                Ok(target) => target.report(false),
                Err(failure) => *failure.target_report,
            })
            .collect::<Vec<_>>();
        return Ok(RepositoryHookReport {
            schema_version: 1,
            status: failure_code.to_string(),
            project_root,
            hooks_path: Some(effective.path),
            hooks,
            message: Some(failure_message),
        });
    }
    let targets = targets.into_iter().map(Result::unwrap).collect::<Vec<_>>();

    if request.action == RepositoryHookAction::Status {
        let hooks = targets
            .iter()
            .map(|target| target.report(false))
            .collect::<Vec<_>>();
        return Ok(RepositoryHookReport {
            schema_version: 1,
            status: summarize_hook_states(&hooks),
            project_root,
            hooks_path: Some(effective.path),
            hooks,
            message: None,
        });
    }

    roots.validate_hook_metadata().map_err(|error| {
        HookFailure::new(
            "hooks_config_unresolved",
            format!("repository configuration changed before hook mutation: {error:#}"),
            &project_root,
        )
        .at_hooks_path(&effective.path)
    })?;
    effective
        .config_capture
        .validate(&project_root, &effective.path)?;
    effective.validate_root_identities()?;
    let transaction_lock = _transaction_lock
        .as_ref()
        .expect("mutating hook action holds its transaction lock");
    validate_transaction_lock_binding(&pinned.handle, transaction_lock).map_err(|error| {
        HookFailure::new(
            "hook_mutation_failed",
            format!("hook transaction lock name changed before mutation: {error}"),
            &project_root,
        )
        .at_hooks_path(&effective.path)
    })?;

    if let Err(message) = apply_plans(&pinned, &targets, transaction_lock) {
        return Ok(RepositoryHookReport {
            schema_version: 1,
            status: "hook_mutation_failed".to_string(),
            project_root,
            hooks_path: Some(effective.path),
            hooks: targets.iter().map(|target| target.report(false)).collect(),
            message: Some(message),
        });
    }

    let hooks = targets
        .iter()
        .map(|target| target.report_after_success(request.action))
        .collect::<Vec<_>>();
    Ok(RepositoryHookReport {
        schema_version: 1,
        status: summarize_hook_states(&hooks),
        project_root,
        hooks_path: Some(effective.path),
        hooks,
        message: None,
    })
}

fn validate_invocation(request: &RepositoryHookRequest, project_root: &Path) -> HookResult<()> {
    for (label, path) in [
        ("plugin data", &request.plugin_data_dir),
        ("Node executable", &request.node_path),
        ("hook wrapper", &request.script_path),
    ] {
        if !path.is_absolute() {
            return Err(HookFailure::new(
                "invalid_hook_invocation",
                format!("{label} path must be absolute: {}", path.display()),
                project_root,
            ));
        }
        if path.to_str().is_none() {
            return Err(HookFailure::new(
                "invalid_hook_invocation",
                format!("{label} path must be valid UTF-8"),
                project_root,
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct HookConfigEnvironment {
    values: BTreeMap<String, OsString>,
    current_dir: PathBuf,
}

impl HookConfigEnvironment {
    fn capture() -> Result<Self, String> {
        let current_dir = std::env::current_dir()
            .map_err(|error| format!("capture current directory for Git config: {error}"))?;
        let mut values = BTreeMap::new();
        for key in [
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "XDG_CONFIG_HOME",
            "GIT_CONFIG_NOSYSTEM",
            "GIT_CONFIG_SYSTEM",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_PARAMETERS",
            "PATH",
            "EXEPATH",
            "DEVELOPER_DIR",
        ] {
            if let Some(value) = std::env::var_os(key) {
                values.insert(key.to_string(), value);
            }
        }
        if let Some(raw_count) = values.get("GIT_CONFIG_COUNT") {
            let count = raw_count
                .to_str()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(MAX_ENV_CONFIG_ENTRIES.saturating_add(1));
            for index in 0..count.min(MAX_ENV_CONFIG_ENTRIES.saturating_add(1)) {
                for prefix in ["GIT_CONFIG_KEY", "GIT_CONFIG_VALUE"] {
                    let key = format!("{prefix}_{index}");
                    if let Some(value) = std::env::var_os(&key) {
                        values.insert(key, value);
                    }
                }
            }
        }
        Ok(Self {
            values,
            current_dir,
        })
    }

    #[cfg(test)]
    fn empty(current_dir: &Path) -> Self {
        Self {
            values: BTreeMap::new(),
            current_dir: current_dir.to_path_buf(),
        }
    }

    #[cfg(test)]
    fn set(&mut self, key: &str, value: impl Into<OsString>) {
        self.values.insert(key.to_string(), value.into());
    }

    #[cfg(test)]
    fn remove(&mut self, key: &str) {
        self.values.remove(key);
    }

    fn get(&self, key: &str) -> Option<&OsStr> {
        self.values.get(key).map(OsString::as_os_str)
    }

    fn absolute_selector_path(&self, value: &OsStr) -> Result<PathBuf, String> {
        let path = PathBuf::from(value);
        if path.as_os_str().is_empty() {
            return Err("Git config selector path is empty".to_string());
        }
        Ok(if path.is_absolute() {
            normalize_lexical_path(&path)
        } else {
            normalize_lexical_path(&self.current_dir.join(path))
        })
    }

    fn home_dir(&self) -> Option<PathBuf> {
        self.get("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                self.get("USERPROFILE")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
            })
            .or_else(|| {
                let drive = self.get("HOMEDRIVE")?;
                let path = self.get("HOMEPATH")?;
                let mut home = OsString::from(drive);
                home.push(path);
                Some(PathBuf::from(home))
            })
    }
}

#[derive(Debug, Clone)]
struct ConfigFileCapture {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

impl ConfigFileCapture {
    fn capture(path: PathBuf) -> Result<Self, String> {
        let contents = read_bounded_optional_config(&path)?;
        Ok(Self { path, contents })
    }

    fn validate(&self) -> Result<(), String> {
        let current = read_bounded_optional_config(&self.path)?;
        if current != self.contents {
            return Err(format!(
                "Git configuration changed during hook resolution: {}",
                self.path.display()
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct HookConfigCapture {
    files: Vec<ConfigFileCapture>,
}

impl HookConfigCapture {
    fn validate(&self, project_root: &Path, hooks_path: &Path) -> HookResult<()> {
        for capture in &self.files {
            capture.validate().map_err(|message| {
                HookFailure::new("hooks_config_unresolved", message, project_root)
                    .at_hooks_path(hooks_path)
            })?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct TrustedRoot {
    path: PathBuf,
    identity: WorkspacePathIdentity,
    lexical: WorkspacePathLexicalIdentity,
}

impl TrustedRoot {
    fn capture(path: PathBuf, project_root: &Path) -> HookResult<Self> {
        let identity = workspace_path_identity(&path).map_err(|error| {
            HookFailure::new(
                "hooks_path_unproven",
                format!("capture native identity for {}: {error}", path.display()),
                project_root,
            )
        })?;
        let lexical = workspace_path_lexical_identity(&path).map_err(|error| {
            HookFailure::new(
                "hooks_path_unproven",
                format!("capture lexical identity for {}: {error}", path.display()),
                project_root,
            )
        })?;
        Ok(Self {
            path,
            identity,
            lexical,
        })
    }

    fn validate(&self, project_root: &Path, hooks_path: &Path) -> HookResult<()> {
        let current = workspace_path_identity(&self.path).map_err(|error| {
            HookFailure::new(
                "hooks_path_unproven",
                format!(
                    "revalidate native identity for {}: {error}",
                    self.path.display()
                ),
                project_root,
            )
            .at_hooks_path(hooks_path)
        })?;
        if current != self.identity {
            return Err(HookFailure::new(
                "hooks_path_unproven",
                format!("trusted repository root changed: {}", self.path.display()),
                project_root,
            )
            .at_hooks_path(hooks_path));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct EffectiveHooksPath {
    path: PathBuf,
    project_root: PathBuf,
    trusted_roots: Vec<TrustedRoot>,
    selected_root: usize,
    relative: PathBuf,
    config_capture: HookConfigCapture,
}

impl EffectiveHooksPath {
    fn resolve(roots: &MetadataRoots, environment: &HookConfigEnvironment) -> HookResult<Self> {
        let project_root = roots.root.clone();
        let (configured, config_capture) = resolve_hooks_config(roots, environment)?;
        let path = match configured {
            Some(value) => resolve_configured_hooks_path(&value, roots, environment)?,
            None => roots.common_dir.join("hooks"),
        };

        if is_null_device(&path) {
            return Err(HookFailure::new(
                "hooks_path_disabled",
                "Git hooks are disabled by core.hooksPath",
                &project_root,
            )
            .at_hooks_path(&path));
        }

        let mut trusted_roots = Vec::new();
        for root in [&roots.root, &roots.git_dir, &roots.common_dir] {
            let captured = TrustedRoot::capture(root.clone(), &project_root)?;
            if trusted_roots
                .iter()
                .any(|existing: &TrustedRoot| existing.identity == captured.identity)
            {
                continue;
            }
            trusted_roots.push(captured);
        }
        let candidate_lexical = workspace_path_lexical_identity(&path).map_err(|error| {
            HookFailure::new(
                "hooks_path_unproven",
                format!("normalize effective hooks path: {error}"),
                &project_root,
            )
            .at_hooks_path(&path)
        })?;
        let selected_root = trusted_roots
            .iter()
            .enumerate()
            .filter(|(_, root)| candidate_lexical.is_within(&root.lexical))
            .max_by_key(|(_, root)| root.path.components().count())
            .map(|(index, _)| index)
            .ok_or_else(|| {
                HookFailure::new(
                    "hooks_path_external",
                    "effective core.hooksPath is outside the validated project and repository roots",
                    &project_root,
                )
                .at_hooks_path(&path)
            })?;
        let root = &trusted_roots[selected_root];
        let relative = relative_components_after(&path, &root.path).ok_or_else(|| {
            HookFailure::new(
                "hooks_path_unproven",
                "effective hooks path could not be made relative to its validated root",
                &project_root,
            )
            .at_hooks_path(&path)
        })?;

        Ok(Self {
            path,
            project_root,
            trusted_roots,
            selected_root,
            relative,
            config_capture,
        })
    }

    fn validate_root_identities(&self) -> HookResult<()> {
        for root in &self.trusted_roots {
            root.validate(&self.project_root, &self.path)?;
        }
        Ok(())
    }

    fn pin_directory(&self) -> HookResult<PinnedHooksDirectory> {
        self.validate_root_identities()?;
        let root = &self.trusted_roots[self.selected_root];
        let mut directory = open_root_no_follow(&root.path).map_err(|error| {
            HookFailure::new(
                "hooks_path_unproven",
                format!(
                    "pin validated repository root {}: {error}",
                    root.path.display()
                ),
                &self.project_root,
            )
            .at_hooks_path(&self.path)
        })?;
        let opened_identity = workspace_file_identity(&directory).map_err(|error| {
            HookFailure::new(
                "hooks_path_unproven",
                format!("inspect pinned repository root: {error}"),
                &self.project_root,
            )
            .at_hooks_path(&self.path)
        })?;
        if opened_identity != root.identity {
            return Err(HookFailure::new(
                "hooks_path_unproven",
                "validated repository root changed while it was being pinned",
                &self.project_root,
            )
            .at_hooks_path(&self.path));
        }

        #[cfg(test)]
        run_before_hooks_directory_open_hook();

        for component in self.relative.components() {
            let Component::Normal(name) = component else {
                return Err(HookFailure::new(
                    "hooks_path_traversal",
                    "effective hooks path contains traversal",
                    &self.project_root,
                )
                .at_hooks_path(&self.path));
            };
            directory = open_child_directory(&directory, name).map_err(|error| {
                HookFailure::new(
                    "hooks_path_unproven",
                    format!("pin effective hooks directory component: {error}"),
                    &self.project_root,
                )
                .at_hooks_path(&self.path)
            })?;
        }
        Ok(PinnedHooksDirectory {
            handle: directory,
            display_path: self.path.clone(),
        })
    }
}

fn resolve_hooks_config(
    roots: &MetadataRoots,
    environment: &HookConfigEnvironment,
) -> HookResult<(Option<Vec<u8>>, HookConfigCapture)> {
    let project_root = &roots.root;
    let mut hooks_path = None;
    let mut worktree_config_enabled = false;
    let mut capture = HookConfigCapture::default();

    if let Some(parameters) = environment.get("GIT_CONFIG_PARAMETERS")
        && !parameters.is_empty()
    {
        return Err(HookFailure::new(
            "hooks_config_unresolved",
            "GIT_CONFIG_PARAMETERS cannot be proven without invoking Git",
            project_root,
        ));
    }

    for (path, source) in system_config_paths(environment)
        .map_err(|message| HookFailure::new("hooks_config_unresolved", message, project_root))?
    {
        apply_captured_config(
            path,
            source,
            false,
            &mut capture,
            &mut hooks_path,
            &mut worktree_config_enabled,
            project_root,
        )?;
    }
    for (path, source) in global_config_paths(environment)
        .map_err(|message| HookFailure::new("hooks_config_unresolved", message, project_root))?
    {
        apply_captured_config(
            path,
            source,
            false,
            &mut capture,
            &mut hooks_path,
            &mut worktree_config_enabled,
            project_root,
        )?;
    }

    let local_path = roots.common_dir.join("config");
    let local = roots.captured_local_config(&local_path).ok_or_else(|| {
        HookFailure::new(
            "hooks_config_unresolved",
            "repository-local configuration was not captured by the metadata boundary",
            project_root,
        )
    })?;
    if let Some(contents) = local {
        apply_config_bytes(
            contents,
            &local_path,
            gix::config::Source::Local,
            true,
            &mut hooks_path,
            &mut worktree_config_enabled,
            project_root,
        )?;
    }

    if worktree_config_enabled {
        let worktree_path = roots.git_dir.join("config.worktree");
        let worktree = roots.captured_local_config(&worktree_path).ok_or_else(|| {
            HookFailure::new(
                "hooks_config_unresolved",
                "worktree configuration was not captured by the metadata boundary",
                project_root,
            )
        })?;
        if let Some(contents) = worktree {
            apply_config_bytes(
                contents,
                &worktree_path,
                gix::config::Source::Worktree,
                false,
                &mut hooks_path,
                &mut worktree_config_enabled,
                project_root,
            )?;
        }
    }

    apply_environment_config(environment, &mut hooks_path, project_root)?;
    Ok((hooks_path, capture))
}

fn apply_captured_config(
    path: PathBuf,
    source: gix::config::Source,
    allow_worktree_activation: bool,
    capture: &mut HookConfigCapture,
    hooks_path: &mut Option<Vec<u8>>,
    worktree_config_enabled: &mut bool,
    project_root: &Path,
) -> HookResult<()> {
    let observed = ConfigFileCapture::capture(path)
        .map_err(|message| HookFailure::new("hooks_config_unresolved", message, project_root))?;
    if let Some(contents) = observed.contents.as_deref() {
        apply_config_bytes(
            contents,
            &observed.path,
            source,
            allow_worktree_activation,
            hooks_path,
            worktree_config_enabled,
            project_root,
        )?;
    }
    capture.files.push(observed);
    Ok(())
}

fn apply_config_bytes(
    contents: &[u8],
    path: &Path,
    source: gix::config::Source,
    allow_worktree_activation: bool,
    hooks_path: &mut Option<Vec<u8>>,
    worktree_config_enabled: &mut bool,
    project_root: &Path,
) -> HookResult<()> {
    let config = gix::config::File::from_bytes_no_includes(
        contents,
        gix::config::file::Metadata {
            path: Some(path.to_path_buf()),
            source,
            level: 0,
            trust: gix::sec::Trust::Full,
        },
        gix::config::file::init::Options {
            lossy: true,
            ..Default::default()
        },
    )
    .map_err(|error| {
        HookFailure::new(
            "hooks_config_unresolved",
            format!("parse Git configuration {}: {error}", path.display()),
            project_root,
        )
    })?;
    if config.sections_by_name("include").is_some()
        || config.sections_by_name("includeif").is_some()
    {
        return Err(HookFailure::new(
            "hooks_config_unresolved",
            format!(
                "Git configuration includes are disabled for hook resolution: {}",
                path.display()
            ),
            project_root,
        ));
    }
    if let Some(value) = config.string("core.hooksPath") {
        *hooks_path = Some(value.to_vec());
    }
    if allow_worktree_activation {
        match config.try_value::<gix::config::Boolean>("extensions.worktreeConfig") {
            Ok(Some(value)) => *worktree_config_enabled = bool::from(value),
            Ok(None) => {}
            Err(error) => {
                return Err(HookFailure::new(
                    "hooks_config_unresolved",
                    format!(
                        "invalid extensions.worktreeConfig in {}: {error}",
                        path.display()
                    ),
                    project_root,
                ));
            }
        }
    }
    Ok(())
}

fn apply_environment_config(
    environment: &HookConfigEnvironment,
    hooks_path: &mut Option<Vec<u8>>,
    project_root: &Path,
) -> HookResult<()> {
    let Some(raw_count) = environment.get("GIT_CONFIG_COUNT") else {
        return Ok(());
    };
    let count = raw_count
        .to_str()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|count| *count <= MAX_ENV_CONFIG_ENTRIES)
        .ok_or_else(|| {
            HookFailure::new(
                "hooks_config_unresolved",
                format!(
                    "GIT_CONFIG_COUNT must be an integer no greater than {MAX_ENV_CONFIG_ENTRIES}"
                ),
                project_root,
            )
        })?;
    for index in 0..count {
        let key_name = format!("GIT_CONFIG_KEY_{index}");
        let value_name = format!("GIT_CONFIG_VALUE_{index}");
        let key = environment.get(&key_name).ok_or_else(|| {
            HookFailure::new(
                "hooks_config_unresolved",
                format!("{key_name} is missing"),
                project_root,
            )
        })?;
        let value = environment.get(&value_name).ok_or_else(|| {
            HookFailure::new(
                "hooks_config_unresolved",
                format!("{value_name} is missing"),
                project_root,
            )
        })?;
        let key = key.to_str().ok_or_else(|| {
            HookFailure::new(
                "hooks_config_unresolved",
                format!("{key_name} is not UTF-8"),
                project_root,
            )
        })?;
        if key.len() > MAX_ENV_CONFIG_KEY_BYTES
            || os_str_bytes_len(value) > MAX_ENV_CONFIG_VALUE_BYTES
        {
            return Err(HookFailure::new(
                "hooks_config_unresolved",
                "environment Git configuration exceeds the bounded input limit",
                project_root,
            ));
        }
        let normalized = key.to_ascii_lowercase();
        if normalized.starts_with("include.") || normalized.starts_with("includeif.") {
            return Err(HookFailure::new(
                "hooks_config_unresolved",
                "environment Git configuration contains an include",
                project_root,
            ));
        }
        if normalized == "extensions.worktreeconfig" {
            return Err(HookFailure::new(
                "hooks_config_unresolved",
                "environment overrides cannot change worktree config discovery during hook resolution",
                project_root,
            ));
        }
        if normalized == "core.hookspath" {
            *hooks_path = Some(os_str_bytes(value).map_err(|message| {
                HookFailure::new("hooks_config_unresolved", message, project_root)
            })?);
        }
    }
    Ok(())
}

fn system_config_paths(
    environment: &HookConfigEnvironment,
) -> Result<Vec<(PathBuf, gix::config::Source)>, String> {
    if environment
        .get("GIT_CONFIG_NOSYSTEM")
        .map(|value| gix::config::Boolean::try_from(value.to_os_string()))
        .transpose()
        .map_err(|error| format!("invalid GIT_CONFIG_NOSYSTEM: {error}"))?
        .is_some_and(bool::from)
    {
        return Ok(Vec::new());
    }
    let (installation, default_system) = default_lower_config_paths(environment)?;
    let system = match environment.get("GIT_CONFIG_SYSTEM") {
        Some(value) => {
            let path = environment.absolute_selector_path(value)?;
            (!is_null_device(&path)).then_some(path)
        }
        None => default_system,
    };
    Ok(ordered_lower_config_paths(installation, system))
}

#[cfg(unix)]
fn default_lower_config_paths(
    environment: &HookConfigEnvironment,
) -> Result<(Option<PathBuf>, Option<PathBuf>), String> {
    let git = find_active_git_executable(environment)?;
    if !native_executable(&git)? {
        return Err(format!(
            "the active Git command is not a native executable, so its system config cannot be proven: {}",
            git.display()
        ));
    }
    #[cfg(target_os = "macos")]
    if git == Path::new("/usr/bin/git") {
        return Ok((
            Some(apple_developer_system_config(environment)?),
            Some(PathBuf::from("/etc/gitconfig")),
        ));
    }
    #[cfg(target_os = "macos")]
    if let Some(developer) = apple_direct_developer_root(&git) {
        return Ok((
            Some(developer.join("usr/share/git-core/gitconfig")),
            Some(PathBuf::from("/etc/gitconfig")),
        ));
    }

    let bin = git
        .parent()
        .filter(|parent| parent.file_name() == Some(OsStr::new("bin")))
        .ok_or_else(|| {
            format!(
                "the active Git installation prefix cannot be inferred without executing Git: {}",
                git.display()
            )
        })?;
    let prefix = bin.parent().ok_or_else(|| {
        format!(
            "the active Git executable has no installation prefix: {}",
            git.display()
        )
    })?;
    Ok((
        None,
        Some(if prefix == Path::new("/usr") {
            PathBuf::from("/etc/gitconfig")
        } else {
            prefix.join("etc/gitconfig")
        }),
    ))
}

#[cfg(target_os = "macos")]
fn apple_direct_developer_root(git: &Path) -> Option<PathBuf> {
    let bin = git.parent()?;
    if bin.file_name()? != OsStr::new("bin") {
        return None;
    }
    let usr = bin.parent()?;
    if usr.file_name()? != OsStr::new("usr") {
        return None;
    }
    let developer = usr.parent()?.to_path_buf();
    developer
        .join("usr/share/git-core")
        .is_dir()
        .then_some(developer)
}

#[cfg(target_os = "macos")]
fn apple_developer_system_config(environment: &HookConfigEnvironment) -> Result<PathBuf, String> {
    if let Some(developer) = environment
        .get("DEVELOPER_DIR")
        .filter(|value| !value.is_empty())
    {
        let developer = environment.absolute_selector_path(developer)?;
        if !developer.is_dir() {
            return Err(format!(
                "DEVELOPER_DIR does not identify an installed Apple developer toolchain: {}",
                developer.display()
            ));
        }
        return Ok(developer.join("usr/share/git-core/gitconfig"));
    }

    let selection = Path::new("/var/db/xcode_select_link");
    if let Ok(target) = fs::read_link(selection) {
        let target = if target.is_absolute() {
            target
        } else {
            selection.parent().unwrap_or(Path::new("/")).join(target)
        };
        let developer = fs::canonicalize(&target).map_err(|error| {
            format!(
                "resolve the active Apple developer directory {}: {error}",
                target.display()
            )
        })?;
        return Ok(developer.join("usr/share/git-core/gitconfig"));
    }

    let candidates = [
        PathBuf::from("/Library/Developer/CommandLineTools"),
        PathBuf::from("/Applications/Xcode.app/Contents/Developer"),
    ]
    .into_iter()
    .filter(|candidate| candidate.join("usr/share/git-core").is_dir())
    .collect::<Vec<_>>();
    match candidates.as_slice() {
        [developer] => Ok(developer.join("usr/share/git-core/gitconfig")),
        [] => Err("the active Apple developer Git system config cannot be located".to_string()),
        _ => Err(
            "multiple Apple developer toolchains are installed and the active Git system config cannot be proven"
                .to_string(),
        ),
    }
}

#[cfg(unix)]
fn find_active_git_executable(environment: &HookConfigEnvironment) -> Result<PathBuf, String> {
    let path = environment.get("PATH").ok_or_else(|| {
        "PATH is unavailable, so the active Git executable cannot be proven".to_string()
    })?;
    for directory in std::env::split_paths(path) {
        let directory = if directory.as_os_str().is_empty() {
            environment.current_dir.clone()
        } else if directory.is_absolute() {
            directory
        } else {
            environment.current_dir.join(directory)
        };
        let candidate = normalize_lexical_path(&directory.join("git"));
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            let canonical = fs::canonicalize(&candidate).map_err(|error| {
                format!(
                    "resolve the active Git executable {}: {error}",
                    candidate.display()
                )
            })?;
            if normalize_lexical_path(&canonical) != candidate {
                return Err(format!(
                    "the active Git executable is reached through a symlinked or aliased path, so its installation config cannot be proven: {}",
                    candidate.display()
                ));
            }
            return Ok(candidate);
        }
    }
    Err("the active Git executable cannot be found in PATH without executing it".to_string())
}

fn native_executable(path: &Path) -> Result<bool, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("open active Git executable {}: {error}", path.display()))?;
    let mut magic = [0_u8; 4];
    file.read_exact(&mut magic).map_err(|error| {
        format!(
            "read active Git executable header {}: {error}",
            path.display()
        )
    })?;
    Ok(matches!(
        magic,
        [0x7f, b'E', b'L', b'F']
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xce]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
            | [b'M', b'Z', _, _]
    ))
}

#[cfg(windows)]
fn default_lower_config_paths(
    environment: &HookConfigEnvironment,
) -> Result<(Option<PathBuf>, Option<PathBuf>), String> {
    let git = find_git_executable(environment).ok_or_else(|| {
        "the Git for Windows system configuration location could not be proven".to_string()
    })?;
    if !native_executable(&git)? {
        return Err(format!(
            "the active Git command is not a native executable, so its system config cannot be proven: {}",
            git.display()
        ));
    }
    let parent = git
        .parent()
        .ok_or_else(|| "the Git executable has no parent directory".to_string())?;
    let root = if parent
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("cmd") || name.eq_ignore_ascii_case("bin"))
    {
        parent.parent().unwrap_or(parent)
    } else {
        parent
    };
    Ok((None, Some(root.join("etc/gitconfig"))))
}

#[cfg(not(any(unix, windows)))]
fn default_lower_config_paths(
    _: &HookConfigEnvironment,
) -> Result<(Option<PathBuf>, Option<PathBuf>), String> {
    Err("the platform system Git configuration location cannot be proven".to_string())
}

fn ordered_lower_config_paths(
    installation: Option<PathBuf>,
    system: Option<PathBuf>,
) -> Vec<(PathBuf, gix::config::Source)> {
    let mut paths = Vec::with_capacity(2);
    if installation.as_ref() != system.as_ref()
        && let Some(path) = installation
        && !is_null_device(&path)
    {
        paths.push((path, gix::config::Source::GitInstallation));
    }
    if let Some(path) = system
        && !is_null_device(&path)
    {
        paths.push((path, gix::config::Source::System));
    }
    paths
}

#[cfg(windows)]
fn find_git_executable(environment: &HookConfigEnvironment) -> Option<PathBuf> {
    if let Some(exepath) = environment.get("EXEPATH") {
        let root = PathBuf::from(exepath);
        for candidate in [
            root.join("cmd/git.exe"),
            root.join("bin/git.exe"),
            root.join("git.exe"),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    environment.get("PATH").and_then(|value| {
        std::env::split_paths(value)
            .map(|directory| directory.join("git.exe"))
            .find(|candidate| candidate.is_file())
    })
}

fn global_config_paths(
    environment: &HookConfigEnvironment,
) -> Result<Vec<(PathBuf, gix::config::Source)>, String> {
    if let Some(value) = environment.get("GIT_CONFIG_GLOBAL") {
        let path = environment.absolute_selector_path(value)?;
        return Ok((!is_null_device(&path))
            .then_some((path, gix::config::Source::User))
            .into_iter()
            .collect());
    }
    let home = environment.home_dir();
    let explicit_xdg = environment
        .get("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty());
    let xdg = match explicit_xdg {
        Some(value) => Some(environment.absolute_selector_path(value)?),
        None => home.as_ref().map(|home| home.join(".config")),
    };
    let mut paths = Vec::with_capacity(2);
    if let Some(xdg) = xdg {
        paths.push((xdg.join("git/config"), gix::config::Source::Git));
    }
    if let Some(home) = home {
        paths.push((home.join(".gitconfig"), gix::config::Source::User));
    }
    Ok(paths)
}

fn resolve_configured_hooks_path(
    raw: &[u8],
    roots: &MetadataRoots,
    environment: &HookConfigEnvironment,
) -> HookResult<PathBuf> {
    if raw.is_empty() {
        return Err(HookFailure::new(
            "hooks_config_unresolved",
            "core.hooksPath is empty",
            &roots.root,
        ));
    }
    if raw
        .windows(b"%(prefix)".len())
        .any(|window| window == b"%(prefix)")
    {
        return Err(HookFailure::new(
            "hooks_config_unresolved",
            "core.hooksPath uses unsupported %(prefix) interpolation",
            &roots.root,
        ));
    }
    if raw.starts_with(b"~") && !supported_home_prefix(raw) {
        return Err(HookFailure::new(
            "hooks_config_unresolved",
            "core.hooksPath uses unsupported named-user home expansion",
            &roots.root,
        ));
    }
    let mut path = bytes_to_path(raw).map_err(|error| {
        HookFailure::new(
            "hooks_config_unresolved",
            format!("decode core.hooksPath: {error:#}"),
            &roots.root,
        )
    })?;
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(HookFailure::new(
            "hooks_path_traversal",
            "core.hooksPath contains parent traversal",
            &roots.root,
        ));
    }
    if let Some(expanded) = expand_home_path(&path, environment, &roots.root) {
        path = expanded?;
    }
    let path = if path.is_absolute() {
        normalize_lexical_path(&path)
    } else {
        normalize_lexical_path(&roots.root.join(path))
    };
    Ok(path)
}

fn supported_home_prefix(raw: &[u8]) -> bool {
    if raw == b"~" || raw.starts_with(b"~/") {
        return true;
    }
    #[cfg(windows)]
    {
        raw.starts_with(b"~\\")
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn expand_home_path(
    path: &Path,
    environment: &HookConfigEnvironment,
    project_root: &Path,
) -> Option<HookResult<PathBuf>> {
    let text = path.to_str()?;
    if text == "~" {
        return Some(environment.home_dir().ok_or_else(|| {
            HookFailure::new(
                "hooks_config_unresolved",
                "core.hooksPath uses ~ but the home directory is unavailable",
                project_root,
            )
        }));
    }
    #[cfg(windows)]
    let remainder = text.strip_prefix("~/").or(text.strip_prefix("~\\"));
    #[cfg(not(windows))]
    let remainder = text.strip_prefix("~/");
    remainder.map(|remainder| {
        environment
            .home_dir()
            .map(|home| home.join(remainder))
            .ok_or_else(|| {
                HookFailure::new(
                    "hooks_config_unresolved",
                    "core.hooksPath uses ~ but the home directory is unavailable",
                    project_root,
                )
            })
    })
}

fn read_bounded_optional_config(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let mut file = match open_config_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "open Git configuration {}: {error}",
                path.display()
            ));
        }
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("inspect Git configuration {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "Git configuration is not a regular file: {}",
            path.display()
        ));
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "Git configuration exceeds the size limit: {}",
            path.display()
        ));
    }
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_CONFIG_BYTES.saturating_add(1))
        .read_to_end(&mut contents)
        .map_err(|error| format!("read Git configuration {}: {error}", path.display()))?;
    if contents.len() as u64 > MAX_CONFIG_BYTES {
        return Err(format!(
            "Git configuration exceeds the size limit: {}",
            path.display()
        ));
    }
    Ok(Some(contents))
}

#[cfg(unix)]
fn open_config_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(not(unix))]
fn open_config_file(path: &Path) -> io::Result<File> {
    File::open(path)
}

fn os_str_bytes_len(value: &OsStr) -> usize {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        value.as_bytes().len()
    }
    #[cfg(not(unix))]
    {
        value.to_string_lossy().len()
    }
}

fn os_str_bytes(value: &OsStr) -> Result<Vec<u8>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(value.as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        value
            .to_str()
            .map(|value| value.as_bytes().to_vec())
            .ok_or_else(|| "environment Git config value is not UTF-8".to_string())
    }
}

fn is_null_device(path: &Path) -> bool {
    #[cfg(unix)]
    {
        path == Path::new("/dev/null")
    }
    #[cfg(windows)]
    {
        path.as_os_str().eq_ignore_ascii_case("NUL")
            || path.as_os_str().eq_ignore_ascii_case("\\\\.\\NUL")
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        false
    }
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn relative_components_after(candidate: &Path, root: &Path) -> Option<PathBuf> {
    let root_count = root.components().count();
    let candidate_count = candidate.components().count();
    if candidate_count < root_count {
        return None;
    }
    Some(candidate.components().skip(root_count).collect())
}

struct PinnedHooksDirectory {
    handle: File,
    display_path: PathBuf,
}

#[cfg(unix)]
fn open_root_no_follow(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(windows)]
fn open_root_no_follow(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let root = options.open(path)?;
    validate_directory_handle(&root)?;
    Ok(root)
}

#[cfg(not(any(unix, windows)))]
fn open_root_no_follow(path: &Path) -> io::Result<File> {
    fs::OpenOptions::new().read(true).open(path)
}

fn open_child_directory(parent: &File, name: &OsStr) -> io::Result<File> {
    let mut options = AtOpenOptions::default();
    options.read(true).follow(false);
    let child = options.open_dir_at(parent, Path::new(name))?;
    validate_directory_handle(&child)?;
    Ok(child)
}

fn validate_directory_handle(directory: &File) -> io::Result<()> {
    let metadata = directory.metadata()?;
    if !metadata.is_dir() {
        return Err(io::Error::other("hooks path component is not a directory"));
    }
    reject_reparse(&metadata)?;
    Ok(())
}

#[cfg(unix)]
fn reject_reparse(_: &fs::Metadata) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn reject_reparse(metadata: &fs::Metadata) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Err(io::Error::other("hooks path contains a reparse point"))
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn reject_reparse(_: &fs::Metadata) -> io::Result<()> {
    Ok(())
}

#[derive(Debug)]
struct HookInvocation {
    project_root: String,
    plugin_data: String,
    node_path: String,
    script_path: String,
}

impl HookInvocation {
    fn from_request(request: &RepositoryHookRequest, project_root: &Path) -> HookResult<Self> {
        let utf8 = |label: &str, path: &Path| {
            path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
                HookFailure::new(
                    "invalid_hook_invocation",
                    format!("{label} path must be valid UTF-8"),
                    project_root,
                )
            })
        };
        Ok(Self {
            project_root: utf8("project", project_root)?,
            plugin_data: utf8("plugin data", &request.plugin_data_dir)?,
            node_path: utf8("Node executable", &request.node_path)?,
            script_path: utf8("hook wrapper", &request.script_path)?,
        })
    }

    fn managed_segment(&self, hook_name: &str, eol: &[u8], owned: bool) -> Vec<u8> {
        let command = format!(
            "{} {} mark --project {} --plugin-data {} --source {} >/dev/null 2>&1 || true",
            shell_quote(&self.node_path),
            shell_quote(&self.script_path),
            shell_quote(&self.project_root),
            shell_quote(&self.plugin_data),
            shell_quote(&format!("git-hook:{hook_name}")),
        );
        let mut block = Vec::new();
        block.extend_from_slice(MANAGED_START);
        block.extend_from_slice(eol);
        if owned {
            block.extend_from_slice(OWNED_DISPATCHER_MARKER);
            block.extend_from_slice(eol);
        }
        block.extend_from_slice(command.as_bytes());
        block.extend_from_slice(eol);
        block.extend_from_slice(MANAGED_END);
        block.extend_from_slice(eol);
        block
    }

    fn owned_template(&self, hook_name: &str) -> Vec<u8> {
        let mut template = b"#!/bin/sh\n".to_vec();
        template.extend_from_slice(&self.managed_segment(hook_name, b"\n", true));
        template
    }
}

fn shell_quote(value: &str) -> String {
    #[cfg(windows)]
    let value = value.replace('\\', "/");
    #[cfg(not(windows))]
    let value = value.to_string();
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[derive(Debug)]
enum HookPlan {
    NoChange,
    Create(Vec<u8>),
    Write(Vec<u8>),
    Delete,
}

impl HookPlan {
    fn changes(&self) -> bool {
        !matches!(self, Self::NoChange)
    }
}

#[derive(Debug)]
struct PreflightTarget {
    name: String,
    path: PathBuf,
    state: &'static str,
    message: Option<String>,
    file: Option<File>,
    original: Option<Vec<u8>>,
    plan: HookPlan,
    #[cfg(unix)]
    mode: u32,
}

impl PreflightTarget {
    fn report(&self, changed: bool) -> RepositoryHookTargetReport {
        RepositoryHookTargetReport {
            hook: self.name.clone(),
            state: self.state.to_string(),
            path: self.path.clone(),
            changed,
            message: self.message.clone(),
        }
    }

    fn report_after_success(&self, action: RepositoryHookAction) -> RepositoryHookTargetReport {
        let state = match (&self.plan, action) {
            (HookPlan::Create(_) | HookPlan::Write(_), RepositoryHookAction::Install) => {
                "installed"
            }
            (HookPlan::Delete, RepositoryHookAction::Uninstall) => "not_installed",
            (HookPlan::Write(_), RepositoryHookAction::Uninstall) => "foreign_hook_present",
            _ => self.state,
        };
        RepositoryHookTargetReport {
            hook: self.name.clone(),
            state: state.to_string(),
            path: self.path.clone(),
            changed: self.plan.changes(),
            message: None,
        }
    }
}

#[derive(Debug)]
struct TargetFailure {
    code: &'static str,
    message: String,
    target_report: Box<RepositoryHookTargetReport>,
}

fn target_failure(
    directory: &PinnedHooksDirectory,
    name: &str,
    code: &'static str,
    message: impl Into<String>,
) -> TargetFailure {
    let message = message.into();
    TargetFailure {
        code,
        message: message.clone(),
        target_report: Box::new(RepositoryHookTargetReport {
            hook: name.to_string(),
            state: code.to_string(),
            path: directory.display_path.join(name),
            changed: false,
            message: Some(message),
        }),
    }
}

fn preflight_target(
    directory: &mut PinnedHooksDirectory,
    name: &str,
    action: RepositoryHookAction,
    invocation: &HookInvocation,
) -> Result<PreflightTarget, TargetFailure> {
    let file = match open_hook_target(&directory.handle, name, false) {
        Ok(file) => file,
        Err(error) => {
            return Err(target_failure(
                directory,
                name,
                open_target_failure_code(&error),
                format!("open hook target without following links: {error}"),
            ));
        }
    };
    let Some(mut inspected_file) = file else {
        return Ok(PreflightTarget {
            name: name.to_string(),
            path: directory.display_path.join(name),
            state: "not_installed",
            message: None,
            file: None,
            original: None,
            plan: if action == RepositoryHookAction::Install {
                HookPlan::Create(invocation.owned_template(name))
            } else {
                HookPlan::NoChange
            },
            #[cfg(unix)]
            mode: 0o755,
        });
    };
    let metadata = inspected_file.metadata().map_err(|error| {
        target_failure(
            directory,
            name,
            "hook_target_uninspectable",
            format!("inspect open hook target: {error}"),
        )
    })?;
    validate_hook_metadata(&metadata)
        .map_err(|(code, message)| target_failure(directory, name, code, message))?;
    let original = read_hook_bytes(&mut inspected_file, &metadata)
        .map_err(|(code, message)| target_failure(directory, name, code, message))?;
    let parsed = parse_hook(&original, name, invocation)
        .map_err(|(code, message)| target_failure(directory, name, code, message))?;
    let (state, plan) = match action {
        RepositoryHookAction::Status => (parsed.state, HookPlan::NoChange),
        RepositoryHookAction::Install => match parsed.kind {
            ParsedHookKind::Foreign { shebang_end, eol } => {
                let mut next = Vec::with_capacity(
                    original.len() + invocation.managed_segment(name, eol, false).len(),
                );
                next.extend_from_slice(&original[..shebang_end]);
                next.extend_from_slice(&invocation.managed_segment(name, eol, false));
                next.extend_from_slice(&original[shebang_end..]);
                ("foreign_hook_present", HookPlan::Write(next))
            }
            ParsedHookKind::Installed { .. } => ("installed", HookPlan::NoChange),
        },
        RepositoryHookAction::Uninstall => match parsed.kind {
            ParsedHookKind::Installed { owned } => {
                let expected = invocation.managed_segment(name, parsed.eol, owned);
                if owned && original == invocation.owned_template(name) {
                    ("installed", HookPlan::Delete)
                } else {
                    let mut restored = Vec::with_capacity(original.len() - expected.len());
                    restored.extend_from_slice(&original[..parsed.shebang_end]);
                    restored.extend_from_slice(&original[parsed.shebang_end + expected.len()..]);
                    ("installed", HookPlan::Write(restored))
                }
            }
            ParsedHookKind::Foreign { .. } => ("foreign_hook_present", HookPlan::NoChange),
        },
    };
    Ok(PreflightTarget {
        name: name.to_string(),
        path: directory.display_path.join(name),
        state,
        message: None,
        file: Some(inspected_file),
        original: Some(original),
        plan,
        #[cfg(unix)]
        mode: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.mode()
        },
    })
}

fn open_target_failure_code(error: &io::Error) -> &'static str {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return "hook_target_symlink";
    }
    if error.kind() == io::ErrorKind::PermissionDenied {
        "hook_target_unwritable"
    } else {
        "hook_target_uninspectable"
    }
}

#[cfg(unix)]
fn open_hook_target(parent: &File, name: &str, writable: bool) -> io::Result<Option<File>> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    let name = CString::new(name).map_err(|_| io::Error::other("hook name contains NUL"))?;
    let access = if writable {
        libc::O_RDWR
    } else {
        libc::O_RDONLY
    };
    // SAFETY: `parent` owns a live directory descriptor, `name` is NUL-terminated,
    // and a successful descriptor is transferred exactly once into `File`.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            access | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error)
        };
    }
    // SAFETY: `fd` was returned uniquely by `openat` above.
    Ok(Some(unsafe { File::from_raw_fd(fd) }))
}

#[cfg(windows)]
fn open_hook_target(parent: &File, name: &str, writable: bool) -> io::Result<Option<File>> {
    let mut options = AtOpenOptions::default();
    options.read(true).follow(false);
    if writable {
        options.write(OpenOptionsWriteMode::Write);
    }
    match options.open_at(parent, name) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(not(any(unix, windows)))]
fn open_hook_target(parent: &File, name: &str, writable: bool) -> io::Result<Option<File>> {
    let mut options = AtOpenOptions::default();
    options.read(true).follow(false);
    if writable {
        options.write(OpenOptionsWriteMode::Write);
    }
    match options.open_at(parent, name) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn validate_hook_metadata(metadata: &fs::Metadata) -> Result<(), (&'static str, String)> {
    reject_reparse(metadata).map_err(|error| ("hook_target_reparse", error.to_string()))?;
    if !metadata.is_file() {
        return Err((
            "hook_target_not_regular",
            "hook target is not a regular file".to_string(),
        ));
    }
    if hard_link_count(metadata)? != 1 {
        return Err((
            "hook_target_hardlinked",
            "hook target has a hard-linked alias".to_string(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.mode() & 0o111 == 0 {
            return Err((
                "hook_not_executable",
                "foreign hook is not executable".to_string(),
            ));
        }
    }
    if metadata.len() > MAX_HOOK_BYTES {
        return Err((
            "hook_too_large",
            format!("hook exceeds the {MAX_HOOK_BYTES}-byte limit"),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn hard_link_count(metadata: &fs::Metadata) -> Result<u64, (&'static str, String)> {
    use std::os::unix::fs::MetadataExt as _;
    Ok(metadata.nlink())
}

#[cfg(windows)]
fn hard_link_count_for_file(file: &File) -> io::Result<u32> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: the file handle remains valid and the output points to correctly
    // sized writable storage for the duration of this call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: success initializes the complete structure.
    Ok(unsafe { information.assume_init() }.nNumberOfLinks)
}

#[cfg(windows)]
fn hard_link_count(_: &fs::Metadata) -> Result<u64, (&'static str, String)> {
    // Windows link count is checked against the already-open handle in
    // `read_hook_bytes`, where path replacement cannot affect the answer.
    Ok(1)
}

#[cfg(not(any(unix, windows)))]
fn hard_link_count(_: &fs::Metadata) -> Result<u64, (&'static str, String)> {
    Err((
        "hook_target_unproven",
        "hard-link identity is unsupported on this platform".to_string(),
    ))
}

fn read_hook_bytes(
    file: &mut File,
    metadata: &fs::Metadata,
) -> Result<Vec<u8>, (&'static str, String)> {
    #[cfg(windows)]
    if hard_link_count_for_file(file)
        .map_err(|error| ("hook_target_unproven", error.to_string()))?
        != 1
    {
        return Err((
            "hook_target_hardlinked",
            "hook target has a hard-linked alias".to_string(),
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| ("hook_target_unreadable", error.to_string()))?;
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_HOOK_BYTES.saturating_add(1))
        .read_to_end(&mut contents)
        .map_err(|error| ("hook_target_unreadable", error.to_string()))?;
    if contents.len() as u64 > MAX_HOOK_BYTES {
        return Err((
            "hook_too_large",
            format!("hook exceeds the {MAX_HOOK_BYTES}-byte limit"),
        ));
    }
    if contents.contains(&0) {
        return Err((
            "hook_binary_content",
            "hook contains a NUL byte".to_string(),
        ));
    }
    Ok(contents)
}

#[derive(Debug)]
enum ParsedHookKind {
    Foreign {
        shebang_end: usize,
        eol: &'static [u8],
    },
    Installed {
        owned: bool,
    },
}

#[derive(Debug)]
struct ParsedHook {
    kind: ParsedHookKind,
    state: &'static str,
    shebang_end: usize,
    eol: &'static [u8],
}

fn parse_hook(
    contents: &[u8],
    hook_name: &str,
    invocation: &HookInvocation,
) -> Result<ParsedHook, (&'static str, String)> {
    let (shebang, shebang_end, eol) = shell_shebang(contents)?;
    if ![
        b"#!/bin/sh".as_slice(),
        b"#!/usr/bin/sh".as_slice(),
        b"#!/bin/bash".as_slice(),
        b"#!/usr/bin/bash".as_slice(),
        b"#!/usr/bin/env sh".as_slice(),
        b"#!/usr/bin/env bash".as_slice(),
    ]
    .contains(&shebang)
    {
        return Err((
            "unsupported_hook_shell",
            "foreign hook must use an allowlisted sh or bash shebang".to_string(),
        ));
    }
    let starts = occurrences(contents, MANAGED_START);
    let ends = occurrences(contents, MANAGED_END);
    if starts.is_empty() && ends.is_empty() {
        return Ok(ParsedHook {
            kind: ParsedHookKind::Foreign { shebang_end, eol },
            state: "foreign_hook_present",
            shebang_end,
            eol,
        });
    }
    if starts.len() != 1 || ends.len() != 1 {
        return Err((
            "managed_block_malformed",
            "hook contains duplicate or unmatched CodeStory markers".to_string(),
        ));
    }
    let start = starts[0];
    let foreign = invocation.managed_segment(hook_name, eol, false);
    let owned = invocation.managed_segment(hook_name, eol, true);
    let installed = if contents
        .get(start..start.saturating_add(owned.len()))
        .is_some_and(|block| block == owned)
    {
        Some(true)
    } else if contents
        .get(start..start.saturating_add(foreign.len()))
        .is_some_and(|block| block == foreign)
    {
        Some(false)
    } else {
        None
    };
    if start != shebang_end || installed.is_none() {
        return Err((
            "uninstall_required",
            "the CodeStory block is stale, edited, or not immediately after the shebang"
                .to_string(),
        ));
    }
    Ok(ParsedHook {
        kind: ParsedHookKind::Installed {
            owned: installed.expect("validated managed block"),
        },
        state: "installed",
        shebang_end,
        eol,
    })
}

type ShellShebang<'a> = Result<(&'a [u8], usize, &'static [u8]), (&'static str, String)>;

fn shell_shebang(contents: &[u8]) -> ShellShebang<'_> {
    let newline = contents
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or_else(|| {
            (
                "unterminated_hook_shebang",
                "foreign hook shebang must end with a newline".to_string(),
            )
        })?;
    let (line_end, eol): (usize, &'static [u8]) = if newline > 0 && contents[newline - 1] == b'\r' {
        (newline - 1, b"\r\n")
    } else {
        (newline, b"\n")
    };
    Ok((&contents[..line_end], newline + 1, eol))
}

fn occurrences(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }
    haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, window)| (window == needle).then_some(index))
        .collect()
}

fn apply_plans(
    directory: &PinnedHooksDirectory,
    targets: &[PreflightTarget],
    transaction_lock: &File,
) -> Result<(), String> {
    let transaction = HookTransactionJournal::for_targets(targets);
    if transaction.entries.is_empty() {
        return Ok(());
    }
    validate_transaction_lock_binding(&directory.handle, transaction_lock)
        .map_err(|error| format!("validate hook transaction lock: {error}"))?;
    let mut reserved =
        reserve_transaction(&directory.handle, &transaction, targets, transaction_lock)
            .map_err(|error| format!("reserve hook transaction: {error}"))?;
    validate_transaction_lock_binding(&directory.handle, transaction_lock)
        .map_err(|error| format!("validate hook transaction lock after reservation: {error}"))?;
    if let Err(error) =
        publish_transaction_journal(&directory.handle, &transaction, &reserved.journal)
    {
        let cleanup = cleanup_reserved_transaction(&directory.handle, &reserved, transaction_lock);
        return Err(format_mutation_failure(
            "publish hook transaction journal",
            error,
            cleanup,
        ));
    }
    #[cfg(test)]
    run_transaction_step_hook("after_journal", usize::MAX)
        .map_err(|error| format!("after-journal fault seam: {error}"))?;

    let ready = match publish_transaction_marker(
        &directory.handle,
        TRANSACTION_READY_NAME,
        &transaction.transaction_id,
    ) {
        Ok(ready) => ready,
        Err(error) => {
            let cleanup = cleanup_transaction(
                &directory.handle,
                &transaction,
                transaction_lock,
                &HashSet::new(),
            );
            return Err(format_mutation_failure(
                "prepare hook transaction",
                error,
                cleanup,
            ));
        }
    };
    if let Err(error) = sync_hooks_directory(&directory.handle) {
        let cleanup = cleanup_transaction(
            &directory.handle,
            &transaction,
            transaction_lock,
            &HashSet::new(),
        );
        return Err(format_mutation_failure(
            "persist hook transaction readiness",
            error,
            cleanup,
        ));
    }
    if let Err(error) =
        validate_named_file_binding(&directory.handle, TRANSACTION_READY_NAME, &ready)
    {
        let cleanup = cleanup_transaction(
            &directory.handle,
            &transaction,
            transaction_lock,
            &HashSet::new(),
        );
        return Err(format_mutation_failure(
            "validate hook transaction readiness",
            error,
            cleanup,
        ));
    }
    #[cfg(test)]
    run_transaction_step_hook("after_ready", usize::MAX)
        .map_err(|error| format!("after-ready fault seam: {error}"))?;

    let applied = (|| -> io::Result<()> {
        for (_index, ((target, entry), artifacts)) in targets
            .iter()
            .filter(|target| target.plan.changes())
            .zip(&transaction.entries)
            .zip(&mut reserved.entries)
            .enumerate()
        {
            validate_transaction_lock_binding(&directory.handle, transaction_lock)?;
            #[cfg(test)]
            run_transaction_step_hook("before_apply", _index)?;
            match target.plan {
                HookPlan::Create(_) => {
                    revalidate_preflight_target(&directory.handle, target)?;
                    #[cfg(test)]
                    run_before_hook_target_write_open_hook();
                    let stage = artifacts.stage.as_mut().expect("create stage");
                    validate_transaction_artifact_binding(
                        &directory.handle,
                        stage,
                        entry.next_sha256.as_deref(),
                        entry.next_mode,
                    )?;
                    #[cfg(windows)]
                    let staged = lock_windows_reserved_source(&directory.handle, stage)?;
                    #[cfg(windows)]
                    let staged = &staged;
                    #[cfg(not(windows))]
                    let staged = &stage.file;
                    rename_bound_no_replace(&directory.handle, &stage.name, &entry.hook, staged)?;
                }
                HookPlan::Write(_) => {
                    publish_write_atomically(&directory.handle, target, entry, artifacts, _index)?;
                }
                HookPlan::Delete => {
                    let backup = artifacts.backup.as_mut().expect("displacement backup");
                    let displaced = displace_hook_into_backup(
                        &directory.handle,
                        &transaction.transaction_id,
                        target,
                        entry,
                        backup,
                    )?;
                    backup.file = displaced;
                }
                HookPlan::NoChange => unreachable!("filtered transaction target"),
            }
            #[cfg(test)]
            run_transaction_step_hook("after_apply", _index)?;
        }
        sync_hooks_directory(&directory.handle)?;
        validate_transaction_lock_binding(&directory.handle, transaction_lock)?;
        let committed = publish_transaction_marker(
            &directory.handle,
            TRANSACTION_COMMIT_NAME,
            &transaction.transaction_id,
        )?;
        validate_named_file_binding(&directory.handle, TRANSACTION_COMMIT_NAME, &committed)?;
        sync_hooks_directory(&directory.handle)?;
        #[cfg(test)]
        run_transaction_step_hook("after_commit", usize::MAX)?;
        Ok(())
    })();
    if let Err(error) = applied {
        let rollback = rollback_ready_transaction(
            &directory.handle,
            &transaction,
            transaction_lock,
        )
        .and_then(|preserved| {
            cleanup_transaction(
                &directory.handle,
                &transaction,
                transaction_lock,
                &preserved,
            )?;
            if preserved.is_empty() {
                Ok(())
            } else {
                Err(io::Error::other(format!(
                    "a concurrent hook edit was preserved; displaced originals remain in {}",
                    preserved.into_iter().collect::<Vec<_>>().join(", ")
                )))
            }
        });
        return Err(format_mutation_failure(
            "apply hook transaction",
            error,
            rollback,
        ));
    }

    cleanup_transaction(
        &directory.handle,
        &transaction,
        transaction_lock,
        &HashSet::new(),
    )
    .map_err(|error| format!("commit hook transaction but cleanup failed: {error}"))
}

impl HookPlan {
    fn next_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Create(contents) | Self::Write(contents) => Some(contents),
            Self::NoChange | Self::Delete => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TransactionOperation {
    Create,
    Write,
    Delete,
}

#[derive(Debug, Serialize, Deserialize)]
struct HookTransactionEntry {
    hook: String,
    operation: TransactionOperation,
    original_sha256: Option<String>,
    next_sha256: Option<String>,
    original_mode: Option<u32>,
    next_mode: Option<u32>,
    backup_name: Option<String>,
    stage_name: Option<String>,
    discard_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct HookTransactionJournal {
    schema_version: u32,
    transaction_id: String,
    entries: Vec<HookTransactionEntry>,
}

impl HookTransactionJournal {
    fn for_targets(targets: &[PreflightTarget]) -> Self {
        let transaction_id = Uuid::new_v4().simple().to_string();
        let mut entries = Vec::new();
        for target in targets.iter().filter(|target| target.plan.changes()) {
            let operation = match target.plan {
                HookPlan::Create(_) => TransactionOperation::Create,
                HookPlan::Write(_) => TransactionOperation::Write,
                HookPlan::Delete => TransactionOperation::Delete,
                HookPlan::NoChange => unreachable!("filtered transaction target"),
            };
            let original_sha256 = target.original.as_deref().map(sha256_hex);
            let next_sha256 = target.plan.next_bytes().map(sha256_hex);
            let original_mode = target.original.as_ref().and_then(|_| target_mode(target));
            let next_mode = match target.plan {
                HookPlan::Create(_) => executable_mode(),
                HookPlan::Write(_) => original_mode,
                HookPlan::NoChange | HookPlan::Delete => None,
            };
            let stem = format!(".codestory-hooks-{transaction_id}-{}", target.name);
            entries.push(HookTransactionEntry {
                hook: target.name.clone(),
                operation,
                original_sha256,
                next_sha256,
                original_mode,
                next_mode,
                backup_name: target.original.is_some().then(|| format!("{stem}.backup")),
                stage_name: target
                    .plan
                    .next_bytes()
                    .is_some()
                    .then(|| format!("{stem}.next")),
                discard_name: format!("{stem}.discard"),
            });
        }
        Self {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            transaction_id,
            entries,
        }
    }

    fn validate(&self) -> io::Result<()> {
        if self.schema_version != TRANSACTION_SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported hook transaction journal version",
            ));
        }
        let id = Uuid::parse_str(&self.transaction_id)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid transaction id"))?
            .simple()
            .to_string();
        if id != self.transaction_id
            || self.entries.is_empty()
            || self.entries.len() > HOOK_NAMES.len()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid hook transaction journal shape",
            ));
        }
        let mut hooks = HashSet::new();
        for entry in &self.entries {
            if !HOOK_NAMES.contains(&entry.hook.as_str()) || !hooks.insert(entry.hook.as_str()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "hook transaction contains an invalid or duplicate target",
                ));
            }
            let stem = format!(".codestory-hooks-{id}-{}", entry.hook);
            let expected_backup = match entry.operation {
                TransactionOperation::Create => None,
                TransactionOperation::Write | TransactionOperation::Delete => {
                    Some(format!("{stem}.backup"))
                }
            };
            let expected_stage = match entry.operation {
                TransactionOperation::Create | TransactionOperation::Write => {
                    Some(format!("{stem}.next"))
                }
                TransactionOperation::Delete => None,
            };
            let expected_discard = format!("{stem}.discard");
            let hash_shape = |value: &Option<String>| {
                value.as_ref().is_some_and(|value| {
                    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            };
            let hashes_valid = match entry.operation {
                TransactionOperation::Create => {
                    entry.original_sha256.is_none() && hash_shape(&entry.next_sha256)
                }
                TransactionOperation::Write => {
                    hash_shape(&entry.original_sha256) && hash_shape(&entry.next_sha256)
                }
                TransactionOperation::Delete => {
                    hash_shape(&entry.original_sha256) && entry.next_sha256.is_none()
                }
            };
            if entry.backup_name != expected_backup
                || entry.stage_name != expected_stage
                || entry.discard_name != expected_discard
                || !hashes_valid
                || !transaction_modes_valid(entry)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "hook transaction entry is inconsistent",
                ));
            }
        }
        Ok(())
    }

    fn journal_temp_name(&self) -> String {
        format!(".codestory-hooks-{}.journal", self.transaction_id)
    }
}

#[derive(Debug)]
struct ReservedArtifact {
    name: String,
    file: File,
}

#[derive(Debug)]
struct ReservedTransactionEntry {
    backup: Option<ReservedArtifact>,
    stage: Option<ReservedArtifact>,
    discard: Option<ReservedArtifact>,
}

#[derive(Debug)]
struct ReservedTransaction {
    journal: ReservedArtifact,
    entries: Vec<ReservedTransactionEntry>,
}

#[cfg(unix)]
fn target_mode(target: &PreflightTarget) -> Option<u32> {
    Some(target.mode & 0o7777)
}

#[cfg(not(unix))]
fn target_mode(_: &PreflightTarget) -> Option<u32> {
    None
}

#[cfg(unix)]
fn executable_mode() -> Option<u32> {
    Some(0o755)
}

#[cfg(not(unix))]
fn executable_mode() -> Option<u32> {
    None
}

#[cfg(unix)]
fn transaction_modes_valid(entry: &HookTransactionEntry) -> bool {
    match entry.operation {
        TransactionOperation::Create => entry.original_mode.is_none() && entry.next_mode.is_some(),
        TransactionOperation::Write => {
            entry.original_mode.is_some() && entry.next_mode == entry.original_mode
        }
        TransactionOperation::Delete => entry.original_mode.is_some() && entry.next_mode.is_none(),
    }
}

#[cfg(not(unix))]
fn transaction_modes_valid(entry: &HookTransactionEntry) -> bool {
    entry.original_mode.is_none() && entry.next_mode.is_none()
}

fn sha256_hex(contents: &[u8]) -> String {
    let digest = Sha256::digest(contents);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn acquire_transaction_lock(parent: &File) -> io::Result<File> {
    let mut options = AtOpenOptions::default();
    options
        .read(true)
        .write(OpenOptionsWriteMode::Write)
        .create(true)
        .follow(false);
    #[cfg(unix)]
    {
        use fs_at::os::unix::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let lock = options.open_at(parent, TRANSACTION_LOCK_NAME)?;
    validate_private_regular_file(&lock, "hook transaction lock")?;
    if !crate::locking::try_lock_exclusive_outliving_spawn_ghosts(&lock)? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "another hook transaction is active",
        ));
    }
    validate_transaction_lock_binding(parent, &lock)?;
    Ok(lock)
}

fn validate_transaction_lock_binding(parent: &File, lock: &File) -> io::Result<()> {
    validate_named_file_binding(parent, TRANSACTION_LOCK_NAME, lock).map_err(|error| {
        io::Error::other(format!(
            "the fixed transaction lock name is no longer bound to the locked file: {error}"
        ))
    })
}

fn transaction_recovery_required(parent: &File) -> io::Result<bool> {
    for name in [
        TRANSACTION_JOURNAL_NAME,
        TRANSACTION_READY_NAME,
        TRANSACTION_COMMIT_NAME,
    ] {
        if let Some(file) = open_hook_target(parent, name, false)? {
            validate_private_regular_file(&file, "hook transaction artifact")?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn recover_pending_transaction(parent: &File, transaction_lock: &File) -> io::Result<()> {
    validate_transaction_lock_binding(parent, transaction_lock)?;
    let Some(bytes) = read_relative_bounded(
        parent,
        TRANSACTION_JOURNAL_NAME,
        MAX_TRANSACTION_JOURNAL_BYTES,
    )?
    else {
        let ready = read_transaction_marker(parent, TRANSACTION_READY_NAME)?;
        let committed = read_transaction_marker(parent, TRANSACTION_COMMIT_NAME)?;
        if let (Some(ready), Some(committed)) = (&ready, &committed)
            && ready != committed
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "hook transaction markers identify different transactions",
            ));
        }
        if committed.is_some() {
            remove_relative_file_if_exists(parent, TRANSACTION_COMMIT_NAME)?;
        }
        if ready.is_some() {
            remove_relative_file_if_exists(parent, TRANSACTION_READY_NAME)?;
        }
        sync_hooks_directory(parent)?;
        return Ok(());
    };
    let transaction: HookTransactionJournal = serde_json::from_slice(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid hook transaction journal: {error}"),
        )
    })?;
    transaction.validate()?;
    let ready = read_transaction_marker(parent, TRANSACTION_READY_NAME)?;
    let committed = read_transaction_marker(parent, TRANSACTION_COMMIT_NAME)?;
    if ready
        .as_deref()
        .is_some_and(|id| id != transaction.transaction_id)
        || committed
            .as_deref()
            .is_some_and(|id| id != transaction.transaction_id)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "hook transaction marker does not match its journal",
        ));
    }
    if committed.is_some() && ready.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "hook transaction commit marker exists without its ready marker",
        ));
    }
    let preserved = if committed.is_some() {
        // Once COMMIT exists, the transaction owns only its artifacts. The hook
        // names may already contain later user edits and are intentionally not
        // inspected during cleanup.
        HashSet::new()
    } else if ready.is_some() {
        rollback_ready_transaction(parent, &transaction, transaction_lock)?
    } else {
        // No READY means no hook mutation was authorized. Concurrent edits are
        // unrelated state; recovery only removes the reserved transaction files.
        HashSet::new()
    };
    cleanup_transaction(parent, &transaction, transaction_lock, &preserved)
}

fn reserve_transaction(
    parent: &File,
    transaction: &HookTransactionJournal,
    targets: &[PreflightTarget],
    transaction_lock: &File,
) -> io::Result<ReservedTransaction> {
    validate_transaction_lock_binding(parent, transaction_lock)?;
    transaction.validate()?;
    let mut files = BTreeMap::<String, File>::new();
    let reservation = (|| -> io::Result<()> {
        for (target, entry) in targets
            .iter()
            .filter(|target| target.plan.changes())
            .zip(&transaction.entries)
        {
            if entry.operation == TransactionOperation::Delete
                && let Some(name) = &entry.backup_name
            {
                reserve_artifact_into(
                    parent,
                    &mut files,
                    name,
                    &transaction_placeholder(&transaction.transaction_id, &entry.hook, "backup"),
                    private_mode(),
                )?;
            }
            if let Some(name) = &entry.stage_name {
                reserve_artifact_into(
                    parent,
                    &mut files,
                    name,
                    target
                        .plan
                        .next_bytes()
                        .ok_or_else(|| io::Error::other("transaction stage bytes are missing"))?,
                    entry.next_mode,
                )?;
            }
            if entry.operation == TransactionOperation::Create {
                reserve_artifact_into(
                    parent,
                    &mut files,
                    &entry.discard_name,
                    &transaction_placeholder(&transaction.transaction_id, &entry.hook, "discard"),
                    private_mode(),
                )?;
            }
        }

        // The journal is created only after every object named by it is already
        // reserved. Publishing the fixed journal name is therefore the first point
        // at which recovery can discover a transaction nonce and trust its names.
        reserve_artifact_into(
            parent,
            &mut files,
            &transaction.journal_temp_name(),
            &serialize_transaction_journal(transaction)?,
            private_mode(),
        )?;
        for (name, file) in &files {
            validate_named_file_binding(parent, name, file)?;
        }
        sync_hooks_directory(parent)
    })();
    if let Err(error) = reservation {
        let cleanup = cleanup_reserved_files(parent, &files, transaction_lock);
        return Err(match cleanup {
            Ok(()) => error,
            Err(cleanup) => io::Error::other(format!(
                "{error}; cleanup of partially reserved transaction failed: {cleanup}"
            )),
        });
    }

    let mut entries = Vec::with_capacity(transaction.entries.len());
    for entry in &transaction.entries {
        let backup = entry.backup_name.as_ref().and_then(|name| {
            files.remove(name).map(|file| ReservedArtifact {
                name: name.clone(),
                file,
            })
        });
        let stage = entry.stage_name.as_ref().map(|name| ReservedArtifact {
            name: name.clone(),
            file: files.remove(name).expect("reserved stage"),
        });
        let discard = files
            .remove(&entry.discard_name)
            .map(|file| ReservedArtifact {
                name: entry.discard_name.clone(),
                file,
            });
        entries.push(ReservedTransactionEntry {
            backup,
            stage,
            discard,
        });
    }
    let journal_name = transaction.journal_temp_name();
    let journal = ReservedArtifact {
        name: journal_name.clone(),
        file: files.remove(&journal_name).expect("reserved journal"),
    };
    debug_assert!(files.is_empty());
    validate_transaction_lock_binding(parent, transaction_lock)?;
    Ok(ReservedTransaction { journal, entries })
}

fn reserve_artifact_into(
    parent: &File,
    reserved: &mut BTreeMap<String, File>,
    name: &str,
    contents: &[u8],
    mode: Option<u32>,
) -> io::Result<()> {
    if reserved.contains_key(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "duplicate hook transaction artifact name",
        ));
    }
    let file = create_private_file(parent, name, contents, mode)?;
    reserved.insert(name.to_string(), file);
    validate_named_file_binding(parent, name, reserved.get(name).expect("reserved artifact"))?;
    Ok(())
}

fn cleanup_reserved_files(
    parent: &File,
    files: &BTreeMap<String, File>,
    transaction_lock: &File,
) -> io::Result<()> {
    validate_transaction_lock_binding(parent, transaction_lock)?;
    let mut first_error = None;
    for (name, file) in files {
        validate_transaction_lock_binding(parent, transaction_lock)?;
        if let Err(error) = remove_bound_file(parent, name, file)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    if let Err(error) = sync_hooks_directory(parent)
        && first_error.is_none()
    {
        first_error = Some(error);
    }
    if let Some(error) = first_error {
        Err(error)
    } else {
        Ok(())
    }
}

fn transaction_placeholder(transaction_id: &str, hook: &str, role: &str) -> Vec<u8> {
    format!("codestory-hook-transaction-v1|{transaction_id}|{hook}|{role}\n").into_bytes()
}

fn serialize_transaction_journal(transaction: &HookTransactionJournal) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(transaction)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if bytes.len() as u64 > MAX_TRANSACTION_JOURNAL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "hook transaction journal exceeds its size limit",
        ));
    }
    Ok(bytes)
}

fn publish_transaction_journal(
    parent: &File,
    transaction: &HookTransactionJournal,
    journal: &ReservedArtifact,
) -> io::Result<()> {
    transaction.validate()?;
    validate_transaction_artifact_binding(
        parent,
        journal,
        Some(&sha256_hex(&serialize_transaction_journal(transaction)?)),
        private_mode(),
    )?;
    rename_bound_no_replace(
        parent,
        &journal.name,
        TRANSACTION_JOURNAL_NAME,
        &journal.file,
    )?;
    validate_named_file_binding(parent, TRANSACTION_JOURNAL_NAME, &journal.file)?;
    sync_hooks_directory(parent)
}

fn publish_transaction_marker(parent: &File, name: &str, transaction_id: &str) -> io::Result<File> {
    let file = create_private_file(
        parent,
        name,
        format!("{transaction_id}\n").as_bytes(),
        private_mode(),
    )?;
    validate_named_file_binding(parent, name, &file)?;
    Ok(file)
}

#[cfg(unix)]
fn private_mode() -> Option<u32> {
    Some(0o600)
}

#[cfg(not(unix))]
fn private_mode() -> Option<u32> {
    None
}

fn create_private_file(
    parent: &File,
    name: &str,
    contents: &[u8],
    mode: Option<u32>,
) -> io::Result<File> {
    #[cfg(not(unix))]
    let _ = mode;
    let mut options = AtOpenOptions::default();
    options
        .read(true)
        .write(OpenOptionsWriteMode::Write)
        .create_new(true)
        .follow(false);
    #[cfg(unix)]
    {
        use fs_at::os::unix::OpenOptionsExt as _;
        let open_mode = libc::mode_t::try_from(mode.unwrap_or(0o600)).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "hook mode exceeds the platform mode_t range",
            )
        })?;
        options.mode(open_mode);
    }
    #[cfg(windows)]
    {
        use fs_at::os::windows::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        };
        options.desired_access(DELETE | FILE_GENERIC_READ | FILE_GENERIC_WRITE);
    }
    let mut file = options.open_at(parent, name)?;
    validate_private_regular_file(&file, "new hook transaction file")?;
    file.write_all(contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(fs::Permissions::from_mode(mode.unwrap_or(0o600)))?;
    }
    file.sync_all()?;
    Ok(file)
}

fn validate_named_file_binding(parent: &File, name: &str, expected: &File) -> io::Result<()> {
    let current = open_hook_target(parent, name, false)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("bound transaction name disappeared: {name}"),
        )
    })?;
    validate_private_regular_file(&current, "bound hook transaction file")?;
    if workspace_file_identity(&current)? != workspace_file_identity(expected)? {
        return Err(io::Error::other(format!(
            "transaction name changed identity: {name}"
        )));
    }
    Ok(())
}

fn validate_private_regular_file(file: &File, label: &str) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || hard_link_count_io(file, &metadata)? != 1 {
        return Err(io::Error::other(format!(
            "{label} is not a private regular file"
        )));
    }
    reject_reparse(&metadata)
}

fn hard_link_count_io(file: &File, metadata: &fs::Metadata) -> io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let _ = file;
        Ok(metadata.nlink())
    }
    #[cfg(windows)]
    {
        let _ = metadata;
        hard_link_count_for_file(file).map(u64::from)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (file, metadata);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "hard-link identity is unsupported",
        ))
    }
}

fn revalidate_preflight_target(parent: &File, target: &PreflightTarget) -> io::Result<()> {
    let current = open_hook_target(parent, &target.name, false)?;
    match (&target.original, &target.file, current) {
        (None, None, None) => Ok(()),
        (Some(original), Some(inspected), Some(mut current)) => {
            let metadata = current.metadata()?;
            validate_hook_metadata(&metadata).map_err(|(_, message)| io::Error::other(message))?;
            if workspace_file_identity(&current)? != workspace_file_identity(inspected)? {
                return Err(io::Error::other(
                    "hook target identity changed after preflight",
                ));
            }
            let bytes = read_hook_bytes(&mut current, &metadata)
                .map_err(|(_, message)| io::Error::other(message))?;
            if &bytes != original || current_mode(&metadata) != target_mode(target) {
                return Err(io::Error::other(
                    "hook target bytes or mode changed after preflight",
                ));
            }
            Ok(())
        }
        _ => Err(io::Error::other(
            "hook target existence changed after preflight",
        )),
    }
}

fn validate_displaced_target(file: &mut File, target: &PreflightTarget) -> io::Result<()> {
    let inspected = target
        .file
        .as_ref()
        .ok_or_else(|| io::Error::other("preflight hook handle is missing"))?;
    if workspace_file_identity(file)? != workspace_file_identity(inspected)? {
        return Err(io::Error::other(
            "the object displaced from the hook name is not the preflight hook",
        ));
    }
    let metadata = file.metadata()?;
    validate_hook_metadata(&metadata).map_err(|(_, message)| io::Error::other(message))?;
    let bytes =
        read_hook_bytes(file, &metadata).map_err(|(_, message)| io::Error::other(message))?;
    if Some(bytes.as_slice()) != target.original.as_deref()
        || current_mode(&metadata) != target_mode(target)
    {
        return Err(io::Error::other(
            "the object displaced from the hook name changed bytes or mode after preflight",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn publish_write_atomically(
    parent: &File,
    target: &PreflightTarget,
    entry: &HookTransactionEntry,
    artifacts: &mut ReservedTransactionEntry,
    _index: usize,
) -> io::Result<()> {
    revalidate_preflight_target(parent, target)?;
    let stage = artifacts.stage.as_mut().expect("write stage");
    validate_transaction_artifact_binding(
        parent,
        stage,
        entry.next_sha256.as_deref(),
        entry.next_mode,
    )?;
    let mut displaced = target
        .file
        .as_ref()
        .ok_or_else(|| io::Error::other("preflight hook handle is missing"))?
        .try_clone()?;
    #[cfg(test)]
    run_before_hook_target_write_open_hook();
    revalidate_preflight_target(parent, target)?;

    // Exchange is the publication point: the hook name always denotes either
    // the complete old file or the complete staged file. The displaced object
    // remains at the stage name and is therefore also the rollback artifact.
    rename_relative_exchange(parent, &stage.name, &entry.hook)?;
    #[cfg(test)]
    run_transaction_step_hook("after_write_publish", _index)?;
    let published = validate_named_file_binding(parent, &entry.hook, &stage.file).and_then(|()| {
        validate_open_transaction_artifact(
            &stage.file,
            &entry.hook,
            entry.next_sha256.as_deref(),
            entry.next_mode,
        )
    });
    let displaced_valid = validate_displaced_target(&mut displaced, target)
        .and_then(|()| validate_named_file_binding(parent, &stage.name, &displaced));
    if let Err(error) = published.and(displaced_valid) {
        // Name validation and a later inverse exchange cannot form an atomic
        // conditional operation on Unix. Defer to the state-aware outer
        // rollback instead: it restores only while `hook` is still the staged
        // object, otherwise it leaves a concurrent edit visible and preserves
        // the displaced original under the nonce-qualified stage name.
        return Err(io::Error::other(format!(
            "hook changed at the atomic publication boundary: {error}"
        )));
    }
    stage.file = displaced;
    Ok(())
}

#[cfg(windows)]
fn publish_write_atomically(
    parent: &File,
    target: &PreflightTarget,
    entry: &HookTransactionEntry,
    artifacts: &mut ReservedTransactionEntry,
    _index: usize,
) -> io::Result<()> {
    let backup_name = entry
        .backup_name
        .as_deref()
        .expect("write displacement backup");
    if open_hook_target(parent, backup_name, false)?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "atomic write backup name is already occupied",
        ));
    }
    if open_hook_target(parent, &entry.discard_name, false)?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "atomic write discard name is already occupied",
        ));
    }

    let stage = artifacts.stage.as_mut().expect("write stage");
    validate_transaction_artifact_binding(
        parent,
        stage,
        entry.next_sha256.as_deref(),
        entry.next_mode,
    )?;
    let staged = lock_windows_reserved_source(parent, stage)?;
    validate_open_transaction_artifact(
        &staged,
        &stage.name,
        entry.next_sha256.as_deref(),
        entry.next_mode,
    )?;

    let mut displaced = open_windows_atomic_replace_source(parent, &entry.hook, false)?;
    validate_displaced_target(&mut displaced, target)?;
    #[cfg(test)]
    run_before_hook_target_write_open_hook();

    // Keep the old object alive under a nonce-qualified, parent-relative hard
    // link before replacing its visible name. Both the old hook and the staged
    // file remain locked against writes and renames across the publication
    // syscall. No absolute path is reconstructed from the pinned directory.
    let relaxed_displaced = reopen_windows_read_source(&displaced)?;
    link_open_file_windows(parent, &displaced, backup_name)?;
    validate_windows_link_binding(parent, &entry.hook, &displaced, 2)?;
    validate_windows_link_binding(parent, backup_name, &displaced, 2)?;
    artifacts.backup = Some(ReservedArtifact {
        name: backup_name.to_string(),
        file: relaxed_displaced,
    });
    #[cfg(test)]
    run_transaction_step_hook("after_write_backup", _index)?;

    // Windows share modes do not distinguish our rename from a competing one:
    // retaining the no-share-delete preflight handle would also block the
    // POSIX replacement below. The verified hard link now pins the exact old
    // object for rollback, and the relaxed handle keeps that identity open.
    drop(displaced);
    rename_open_file_windows_posix_replace(parent, &staged, &entry.hook)?;
    validate_named_file_binding(parent, &entry.hook, &stage.file)?;
    validate_open_transaction_artifact(
        &stage.file,
        &entry.hook,
        entry.next_sha256.as_deref(),
        entry.next_mode,
    )?;
    let backup = &artifacts.backup.as_ref().expect("published backup").file;
    let mut displaced = backup.try_clone()?;
    validate_displaced_target(&mut displaced, target)?;
    validate_named_file_binding(parent, backup_name, backup)
}

#[cfg(not(any(unix, windows)))]
fn publish_write_atomically(
    _: &File,
    _: &PreflightTarget,
    _: &HookTransactionEntry,
    _: &mut ReservedTransactionEntry,
    _: usize,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic hook replacement is unsupported on this platform",
    ))
}

fn displace_hook_into_backup(
    parent: &File,
    transaction_id: &str,
    target: &PreflightTarget,
    entry: &HookTransactionEntry,
    backup: &ReservedArtifact,
) -> io::Result<File> {
    validate_transaction_artifact_binding(
        parent,
        backup,
        Some(&sha256_hex(&transaction_placeholder(
            transaction_id,
            &entry.hook,
            "backup",
        ))),
        private_mode(),
    )?;
    #[cfg(windows)]
    let mut displaced = {
        let mut source = open_windows_rename_source(parent, &entry.hook)?;
        validate_displaced_target(&mut source, target)?;
        #[cfg(test)]
        run_before_hook_target_write_open_hook();
        rename_open_file_windows(parent, &source, &backup.name, true)?;
        validate_named_file_binding(parent, &backup.name, &source)?;
        source
    };
    #[cfg(not(windows))]
    let mut displaced = {
        revalidate_preflight_target(parent, target)?;
        #[cfg(test)]
        run_before_hook_target_write_open_hook();
        rename_relative_replace(parent, &entry.hook, &backup.name)?;
        match open_hook_target(parent, &backup.name, false) {
            Ok(Some(displaced)) => displaced,
            result => {
                let restore = rename_relative_no_replace(parent, &backup.name, &entry.hook);
                return Err(io::Error::other(format!(
                    "displaced hook could not be proven and was restored ({result:?}; restore: {restore:?})"
                )));
            }
        }
    };

    if let Err(error) = validate_displaced_target(&mut displaced, target)
        .and_then(|()| validate_named_file_binding(parent, &backup.name, &displaced))
    {
        let restore = rename_bound_no_replace(parent, &backup.name, &entry.hook, &displaced);
        return Err(match restore {
            Ok(()) => io::Error::other(format!(
                "hook changed at the displacement boundary and was preserved: {error}"
            )),
            Err(restore) => io::Error::other(format!(
                "hook changed at the displacement boundary: {error}; preserve the displaced object from {}: {restore}",
                backup.name
            )),
        });
    }
    #[cfg(windows)]
    {
        // The no-share-delete handle pins the final displacement boundary, but
        // retaining it after publication would block committed cleanup from
        // deleting the backup. Reopen the exact object read-only with delete
        // sharing before the transaction advances.
        return reopen_windows_read_source(&displaced);
    }
    #[cfg(not(windows))]
    Ok(displaced)
}

fn validate_transaction_artifact_binding(
    parent: &File,
    artifact: &ReservedArtifact,
    sha256: Option<&str>,
    mode: Option<u32>,
) -> io::Result<()> {
    validate_named_file_binding(parent, &artifact.name, &artifact.file)?;
    validate_open_transaction_artifact(&artifact.file, &artifact.name, sha256, mode)
}

fn validate_open_transaction_artifact(
    file: &File,
    name: &str,
    sha256: Option<&str>,
    mode: Option<u32>,
) -> io::Result<()> {
    let mut file = file.try_clone()?;
    let metadata = file.metadata()?;
    validate_private_regular_file(&file, "hook transaction artifact")?;
    if metadata.len() > MAX_HOOK_BYTES.max(MAX_TRANSACTION_JOURNAL_BYTES) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("hook transaction artifact exceeds its size limit: {name}"),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_HOOK_BYTES.max(MAX_TRANSACTION_JOURNAL_BYTES) + 1)
        .read_to_end(&mut bytes)?;
    if sha256.is_some_and(|expected| sha256_hex(&bytes) == expected)
        && current_mode(&metadata) == mode
    {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "hook transaction artifact is missing or changed: {name}"
        )))
    }
}

fn rollback_ready_transaction(
    parent: &File,
    transaction: &HookTransactionJournal,
    transaction_lock: &File,
) -> io::Result<HashSet<String>> {
    let mut preserved = HashSet::new();
    for entry in transaction.entries.iter().rev() {
        validate_transaction_lock_binding(parent, transaction_lock)?;
        match entry.operation {
            TransactionOperation::Create => {
                rollback_created_hook(parent, transaction, entry)?;
            }
            TransactionOperation::Write => {
                rollback_written_hook(parent, transaction, entry, &mut preserved)?;
            }
            TransactionOperation::Delete => {
                rollback_displaced_hook(parent, transaction, entry, &mut preserved)?;
            }
        }
    }
    sync_hooks_directory(parent)?;
    Ok(preserved)
}

#[cfg(unix)]
fn rollback_written_hook(
    parent: &File,
    _: &HookTransactionJournal,
    entry: &HookTransactionEntry,
    preserved: &mut HashSet<String>,
) -> io::Result<()> {
    let stage = entry.stage_name.as_deref().expect("write stage");
    let stage_state = read_current_state(parent, stage, MAX_HOOK_BYTES)?;

    if stage_state.matches(entry.next_sha256.as_deref(), entry.next_mode) {
        // Publication did not happen, or inverse publication already restored
        // the prior hook-name binding and parked the transaction's next bytes
        // at `stage`. The hook name may now be a concurrent symlink or other
        // uninspectable object and is deliberately outside transaction cleanup.
        return Ok(());
    }
    let target_state = read_current_state(parent, &entry.hook, MAX_HOOK_BYTES)?;
    if target_state.matches(entry.next_sha256.as_deref(), entry.next_mode) {
        if matches!(stage_state, CurrentFileState::Absent) {
            return Err(io::Error::other(
                "atomic write rollback is missing its displaced hook",
            ));
        }
        let expected_restored = stage_state.clone();
        rename_relative_exchange(parent, stage, &entry.hook)?;
        let restored = read_current_state(parent, &entry.hook, MAX_HOOK_BYTES)?;
        let parked = read_current_state(parent, stage, MAX_HOOK_BYTES)?;
        if restored != expected_restored
            || !parked.matches(entry.next_sha256.as_deref(), entry.next_mode)
        {
            return Err(io::Error::other(
                "atomic write rollback could not prove its exchanged result",
            ));
        }
        return Ok(());
    }

    if matches!(target_state, CurrentFileState::Absent)
        && stage_state.matches(entry.original_sha256.as_deref(), entry.original_mode)
    {
        restore_named_artifact(parent, stage, &entry.hook)?;
        return Ok(());
    }
    if matches!(stage_state, CurrentFileState::Present { .. }) {
        // The hook was edited after publication. Keep the current hook in
        // place and retain the displaced object under the nonce-qualified
        // stage name so cleanup cannot destroy either object.
        preserved.insert(stage.to_string());
        return Ok(());
    }
    if target_state.matches(entry.original_sha256.as_deref(), entry.original_mode) {
        return Ok(());
    }
    Err(io::Error::other(
        "atomic write rollback is missing its displaced hook",
    ))
}

#[cfg(windows)]
fn rollback_written_hook(
    parent: &File,
    _: &HookTransactionJournal,
    entry: &HookTransactionEntry,
    preserved: &mut HashSet<String>,
) -> io::Result<()> {
    let stage_name = entry.stage_name.as_deref().expect("write stage");
    let backup_name = entry
        .backup_name
        .as_deref()
        .expect("write displacement backup");
    let hook = read_windows_recovery_file(parent, &entry.hook, MAX_HOOK_BYTES)?;
    let stage = read_windows_recovery_file(parent, stage_name, MAX_HOOK_BYTES)?;
    let backup = read_windows_recovery_file(parent, backup_name, MAX_HOOK_BYTES)?;
    let discard = read_windows_recovery_file(parent, &entry.discard_name, MAX_HOOK_BYTES)?;

    let stage_is_next = stage.as_ref().is_some_and(|file| {
        file.is_private()
            && file
                .state
                .matches(entry.next_sha256.as_deref(), entry.next_mode)
    });
    let backup_is_original = backup.as_ref().is_some_and(|file| {
        file.state
            .matches(entry.original_sha256.as_deref(), entry.original_mode)
    });
    let hook_is_original = hook.as_ref().is_some_and(|file| {
        file.state
            .matches(entry.original_sha256.as_deref(), entry.original_mode)
    });
    let hook_is_next = hook.as_ref().is_some_and(|file| {
        file.state
            .matches(entry.next_sha256.as_deref(), entry.next_mode)
    });
    let discard_is_next = discard.as_ref().is_some_and(|file| {
        file.state
            .matches(entry.next_sha256.as_deref(), entry.next_mode)
    });
    let hook_discard_match = match (hook.as_ref(), discard.as_ref()) {
        (Some(hook), Some(discard)) => windows_recovery_files_match(hook, discard)?,
        _ => false,
    };

    if stage_is_next && backup.is_none() && discard.is_none() {
        // The hard-link backup is the first publication step. Its absence
        // proves this transaction never changed the hook name.
        return Ok(());
    }
    if stage_is_next && backup_is_original && discard.is_none() {
        let backup = backup.as_ref().expect("matched backup");
        let Some(hook) = hook.as_ref() else {
            if backup.is_private() {
                restore_named_artifact(parent, backup_name, &entry.hook)?;
                return Ok(());
            }
            return Err(io::Error::other(
                "atomic write preparation lost the original hook name",
            ));
        };
        if hook_is_original
            && hook.links == 2
            && backup.links == 2
            && windows_recovery_files_match(hook, backup)?
        {
            remove_windows_recovery_link(parent, backup_name, backup, 2)?;
            return Ok(());
        }
        if !backup.is_private() {
            return Err(io::Error::other(
                "atomic write preparation has an ambiguous hard-link layout",
            ));
        }
        if hook_is_next {
            return Err(io::Error::other(
                "atomic write publication left both the stage and hook names bound to next bytes",
            ));
        }
        if hook.is_private() {
            preserved.insert(backup_name.to_string());
            return Ok(());
        }
        return Err(io::Error::other(
            "atomic write preparation has an ambiguous hook layout",
        ));
    }
    if stage.is_some() {
        return Err(io::Error::other(
            "atomic write stage has an unexpected recovery state",
        ));
    }

    if let Some(backup) = backup.as_ref() {
        if !backup_is_original || !backup.is_private() {
            return Err(io::Error::other(
                "atomic write backup has an unexpected recovery state",
            ));
        }
        if hook_is_next {
            match (hook.as_ref().expect("matched hook").links, discard.as_ref()) {
                (1, None) => restore_windows_write_atomically(parent, entry, false)?,
                (2, Some(discard))
                    if discard_is_next && discard.links == 2 && hook_discard_match =>
                {
                    restore_windows_write_atomically(parent, entry, true)?;
                }
                _ => {
                    return Err(io::Error::other(
                        "atomic write rollback has an ambiguous discard layout",
                    ));
                }
            }
            return Ok(());
        }
        if hook.is_none() {
            restore_named_artifact(parent, backup_name, &entry.hook)?;
        } else {
            // A later edit owns the hook name. Preserve both it and the actual
            // original instead of replacing either one during recovery.
            preserved.insert(backup_name.to_string());
        }
        return Ok(());
    }

    if discard.is_none()
        && hook_is_original
        && hook.as_ref().is_some_and(WindowsRecoveryFile::is_private)
    {
        return Ok(());
    }
    if discard_is_next
        && discard
            .as_ref()
            .is_some_and(WindowsRecoveryFile::is_private)
        && !hook_is_next
    {
        // The inverse replacement completed and the next object is parked at
        // discard. A subsequent hook edit, if any, remains untouched.
        return Ok(());
    }
    Err(io::Error::other(
        "atomic write transaction has an ambiguous partial layout",
    ))
}

#[cfg(windows)]
fn restore_windows_write_atomically(
    parent: &File,
    entry: &HookTransactionEntry,
    discard_prepared: bool,
) -> io::Result<()> {
    let backup_name = entry
        .backup_name
        .as_deref()
        .expect("write displacement backup");
    let current = open_windows_atomic_replace_source(parent, &entry.hook, discard_prepared)?;
    validate_windows_recovery_artifact(
        &current,
        &entry.hook,
        entry.next_sha256.as_deref(),
        entry.next_mode,
        if discard_prepared { 2 } else { 1 },
    )?;
    let relaxed_current = reopen_windows_read_source(&current)?;
    let original = open_windows_atomic_replace_source(parent, backup_name, false)?;
    validate_open_transaction_artifact(
        &original,
        backup_name,
        entry.original_sha256.as_deref(),
        entry.original_mode,
    )?;

    if discard_prepared {
        validate_windows_link_binding(parent, &entry.hook, &current, 2)?;
        validate_windows_link_binding(parent, &entry.discard_name, &current, 2)?;
    } else {
        if open_hook_target(parent, &entry.discard_name, false)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "atomic rollback discard name is already occupied",
            ));
        }
        link_open_file_windows(parent, &current, &entry.discard_name)?;
        validate_windows_link_binding(parent, &entry.hook, &current, 2)?;
        validate_windows_link_binding(parent, &entry.discard_name, &current, 2)?;
    }

    // Replacing the next hook from the original backup is the inverse
    // publication point. The hard-linked discard keeps the next object
    // recoverable if the syscall's reported result cannot be trusted. Release
    // the no-share-delete target handle only after that link is proven, because
    // Windows otherwise blocks our own POSIX replacement.
    drop(current);
    rename_open_file_windows_posix_replace(parent, &original, &entry.hook)?;
    drop(relaxed_current);
    let restored = read_current_state(parent, &entry.hook, MAX_HOOK_BYTES)?;
    let discarded = read_current_state(parent, &entry.discard_name, MAX_HOOK_BYTES)?;
    let backup = read_current_state(parent, backup_name, MAX_HOOK_BYTES)?;
    if restored.matches(entry.original_sha256.as_deref(), entry.original_mode)
        && discarded.matches(entry.next_sha256.as_deref(), entry.next_mode)
        && matches!(backup, CurrentFileState::Absent)
    {
        Ok(())
    } else {
        Err(io::Error::other(
            "atomic write rollback could not prove its replacement result",
        ))
    }
}

#[cfg(not(any(unix, windows)))]
fn rollback_written_hook(
    _: &File,
    _: &HookTransactionJournal,
    _: &HookTransactionEntry,
    _: &mut HashSet<String>,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic hook rollback is unsupported on this platform",
    ))
}

fn rollback_created_hook(
    parent: &File,
    transaction: &HookTransactionJournal,
    entry: &HookTransactionEntry,
) -> io::Result<()> {
    let stage_state = read_current_state(
        parent,
        entry.stage_name.as_deref().expect("create stage"),
        MAX_HOOK_BYTES,
    )?;
    if stage_state.matches(entry.next_sha256.as_deref(), entry.next_mode) {
        return Ok(());
    }
    let target_state = read_current_state(parent, &entry.hook, MAX_HOOK_BYTES)?;
    if target_state.matches(entry.next_sha256.as_deref(), entry.next_mode) {
        move_matching_target_to_discard(parent, transaction, entry)?;
    }
    // Absence and non-transaction bytes are both preserved. An ordinary edit
    // after READY must never be mistaken for a transaction-created hook.
    Ok(())
}

fn rollback_displaced_hook(
    parent: &File,
    transaction: &HookTransactionJournal,
    entry: &HookTransactionEntry,
    preserved: &mut HashSet<String>,
) -> io::Result<()> {
    let backup = entry.backup_name.as_deref().expect("displacement backup");
    let backup_state = read_current_state(parent, backup, MAX_HOOK_BYTES)?;
    let placeholder = transaction_placeholder(&transaction.transaction_id, &entry.hook, "backup");
    if backup_state.matches(Some(&sha256_hex(&placeholder)), private_mode())
        || matches!(&backup_state, CurrentFileState::Absent)
    {
        // The target was never displaced, or a prior recovery already restored it.
        return Ok(());
    }

    let target_state = read_current_state(parent, &entry.hook, MAX_HOOK_BYTES)?;
    if backup_state.matches(entry.original_sha256.as_deref(), entry.original_mode) {
        if matches!(&target_state, CurrentFileState::Absent) {
            restore_named_artifact(parent, backup, &entry.hook)?;
        } else if entry.operation == TransactionOperation::Write
            && target_state.matches(entry.next_sha256.as_deref(), entry.next_mode)
        {
            move_matching_target_to_discard(parent, transaction, entry)?;
            restore_named_artifact(parent, backup, &entry.hook)?;
        } else {
            // A user changed the hook after displacement. Keep their hook in
            // place and retain the actual original under its nonce-qualified
            // backup name so cleanup cannot destroy either object.
            preserved.insert(backup.to_string());
        }
        return Ok(());
    }

    // The hook changed at the final displacement boundary and that concurrent
    // object, rather than the preflight object, landed in the backup. Restore
    // it only into an empty hook name; otherwise preserve both objects.
    if matches!(&target_state, CurrentFileState::Absent) {
        restore_named_artifact(parent, backup, &entry.hook)?;
    } else {
        preserved.insert(backup.to_string());
    }
    Ok(())
}

fn move_matching_target_to_discard(
    parent: &File,
    transaction: &HookTransactionJournal,
    entry: &HookTransactionEntry,
) -> io::Result<()> {
    let placeholder = transaction_placeholder(&transaction.transaction_id, &entry.hook, "discard");
    let discard = open_hook_target(parent, &entry.discard_name, false)?.ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "transaction discard disappeared")
    })?;
    validate_open_transaction_artifact(
        &discard,
        &entry.discard_name,
        Some(&sha256_hex(&placeholder)),
        private_mode(),
    )?;
    validate_named_file_binding(parent, &entry.discard_name, &discard)?;

    #[cfg(windows)]
    let moved = {
        let source = open_windows_rename_source(parent, &entry.hook)?;
        validate_open_transaction_artifact(
            &source,
            &entry.hook,
            entry.next_sha256.as_deref(),
            entry.next_mode,
        )?;
        rename_open_file_windows(parent, &source, &entry.discard_name, true)?;
        validate_named_file_binding(parent, &entry.discard_name, &source)?;
        source
    };
    #[cfg(not(windows))]
    let moved = {
        let source = open_hook_target(parent, &entry.hook, false)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "transaction hook disappeared")
        })?;
        validate_open_transaction_artifact(
            &source,
            &entry.hook,
            entry.next_sha256.as_deref(),
            entry.next_mode,
        )?;
        rename_relative_replace(parent, &entry.hook, &entry.discard_name)?;
        let moved = open_hook_target(parent, &entry.discard_name, false)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "discarded hook disappeared"))?;
        if workspace_file_identity(&moved)? != workspace_file_identity(&source)? {
            let _ = rename_bound_no_replace(parent, &entry.discard_name, &entry.hook, &moved);
            return Err(io::Error::other(
                "hook changed while being displaced for rollback",
            ));
        }
        moved
    };
    validate_open_transaction_artifact(
        &moved,
        &entry.discard_name,
        entry.next_sha256.as_deref(),
        entry.next_mode,
    )
}

fn restore_named_artifact(parent: &File, from: &str, hook: &str) -> io::Result<()> {
    #[cfg(windows)]
    {
        let source = open_windows_rename_source(parent, from)?;
        validate_private_regular_file(&source, "hook transaction backup")?;
        rename_open_file_windows(parent, &source, hook, false)?;
        validate_named_file_binding(parent, hook, &source)
    }
    #[cfg(not(windows))]
    {
        let file = open_hook_target(parent, from, false)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("transaction backup disappeared: {from}"),
            )
        })?;
        validate_private_regular_file(&file, "hook transaction backup")?;
        rename_bound_no_replace(parent, from, hook, &file)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CurrentFileState {
    Absent,
    Present { sha256: String, mode: Option<u32> },
}

impl CurrentFileState {
    fn matches(&self, sha256: Option<&str>, mode: Option<u32>) -> bool {
        match (self, sha256) {
            (Self::Absent, None) => true,
            (
                Self::Present {
                    sha256: current,
                    mode: current_mode,
                },
                Some(expected),
            ) => current == expected && *current_mode == mode,
            _ => false,
        }
    }
}

#[cfg(windows)]
#[derive(Debug)]
struct WindowsRecoveryFile {
    file: File,
    state: CurrentFileState,
    links: u64,
}

#[cfg(windows)]
impl WindowsRecoveryFile {
    fn is_private(&self) -> bool {
        self.links == 1
    }
}

#[cfg(windows)]
fn read_windows_recovery_file(
    parent: &File,
    name: &str,
    limit: u64,
) -> io::Result<Option<WindowsRecoveryFile>> {
    let Some(mut file) = open_hook_target(parent, name, false)? else {
        return Ok(None);
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other(format!(
            "hook transaction recovery object is not a regular file: {name}"
        )));
    }
    reject_reparse(&metadata)?;
    let links = hard_link_count_io(&file, &metadata)?;
    if links == 0 || links > 2 {
        return Err(io::Error::other(format!(
            "hook transaction recovery object has an unexpected link count: {name}"
        )));
    }
    if metadata.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("hook transaction recovery object exceeds its size limit: {name}"),
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("hook transaction recovery object exceeds its size limit: {name}"),
        ));
    }
    Ok(Some(WindowsRecoveryFile {
        file,
        state: CurrentFileState::Present {
            sha256: sha256_hex(&bytes),
            mode: current_mode(&metadata),
        },
        links,
    }))
}

#[cfg(windows)]
fn windows_recovery_files_match(
    first: &WindowsRecoveryFile,
    second: &WindowsRecoveryFile,
) -> io::Result<bool> {
    Ok(workspace_file_identity(&first.file)? == workspace_file_identity(&second.file)?)
}

fn read_current_state(parent: &File, name: &str, limit: u64) -> io::Result<CurrentFileState> {
    let Some(mut file) = open_hook_target(parent, name, false)? else {
        return Ok(CurrentFileState::Absent);
    };
    validate_private_regular_file(&file, "hook transaction target")?;
    let metadata = file.metadata()?;
    if metadata.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "hook transaction target exceeds its size limit",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "hook transaction target exceeds its size limit",
        ));
    }
    Ok(CurrentFileState::Present {
        sha256: sha256_hex(&bytes),
        mode: current_mode(&metadata),
    })
}

#[cfg(unix)]
fn current_mode(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;
    Some(metadata.mode() & 0o7777)
}

#[cfg(not(unix))]
fn current_mode(_: &fs::Metadata) -> Option<u32> {
    None
}

fn read_relative_bounded(parent: &File, name: &str, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let Some(mut file) = open_hook_target(parent, name, false)? else {
        return Ok(None);
    };
    validate_private_regular_file(&file, "hook transaction journal")?;
    let metadata = file.metadata()?;
    if metadata.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "hook transaction journal exceeds its size limit",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "hook transaction journal exceeds its size limit",
        ));
    }
    Ok(Some(bytes))
}

fn read_transaction_marker(parent: &File, name: &str) -> io::Result<Option<String>> {
    let Some(bytes) = read_relative_bounded(parent, name, 64)? else {
        return Ok(None);
    };
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "transaction marker is not UTF-8",
            )
        })?
        .trim();
    let normalized = Uuid::parse_str(value)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid transaction marker"))?
        .simple()
        .to_string();
    if normalized != value {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "transaction marker is not canonical",
        ));
    }
    Ok(Some(normalized))
}

fn remove_relative_file_if_exists(parent: &File, name: &str) -> io::Result<()> {
    let Some(file) = open_hook_target(parent, name, false)? else {
        return Ok(());
    };
    validate_private_regular_file(&file, "hook transaction artifact")?;
    remove_bound_file(parent, name, &file)
}

fn remove_bound_file(parent: &File, name: &str, expected: &File) -> io::Result<()> {
    validate_named_file_binding(parent, name, expected)?;
    #[cfg(windows)]
    {
        use fs_at::os::windows::FileExt as _;

        let current = open_windows_delete_source(parent, name)?;
        if workspace_file_identity(&current)? != workspace_file_identity(expected)? {
            return Err(io::Error::other(format!(
                "transaction name changed before handle deletion: {name}"
            )));
        }
        current.delete_by_handle().map_err(|(_, error)| error)
    }
    #[cfg(unix)]
    {
        AtOpenOptions::default().unlink_at(parent, name)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (parent, name, expected);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "handle-bound file deletion is unsupported on this platform",
        ))
    }
}

fn cleanup_reserved_transaction(
    parent: &File,
    reserved: &ReservedTransaction,
    transaction_lock: &File,
) -> io::Result<()> {
    validate_transaction_lock_binding(parent, transaction_lock)?;
    for entry in &reserved.entries {
        if let Some(stage) = &entry.stage {
            remove_bound_file(parent, &stage.name, &stage.file)?;
        }
        if let Some(backup) = &entry.backup {
            remove_bound_file(parent, &backup.name, &backup.file)?;
        }
        if let Some(discard) = &entry.discard {
            remove_bound_file(parent, &discard.name, &discard.file)?;
        }
        validate_transaction_lock_binding(parent, transaction_lock)?;
    }
    remove_bound_file(parent, &reserved.journal.name, &reserved.journal.file)?;
    sync_hooks_directory(parent)
}

fn cleanup_transaction(
    parent: &File,
    transaction: &HookTransactionJournal,
    transaction_lock: &File,
    preserved: &HashSet<String>,
) -> io::Result<()> {
    validate_transaction_lock_binding(parent, transaction_lock)?;
    for entry in &transaction.entries {
        if let Some(stage) = &entry.stage_name
            && !preserved.contains(stage)
        {
            remove_relative_file_if_exists(parent, stage)?;
        }
        if let Some(backup) = &entry.backup_name
            && !preserved.contains(backup)
        {
            remove_relative_file_if_exists(parent, backup)?;
        }
        remove_relative_file_if_exists(parent, &entry.discard_name)?;
        validate_transaction_lock_binding(parent, transaction_lock)?;
    }
    remove_relative_file_if_exists(parent, &transaction.journal_temp_name())?;
    remove_relative_file_if_exists(parent, TRANSACTION_JOURNAL_NAME)?;
    sync_hooks_directory(parent)?;
    validate_transaction_lock_binding(parent, transaction_lock)?;
    remove_relative_file_if_exists(parent, TRANSACTION_COMMIT_NAME)?;
    remove_relative_file_if_exists(parent, TRANSACTION_READY_NAME)?;
    sync_hooks_directory(parent)
}

#[cfg(unix)]
fn sync_hooks_directory(parent: &File) -> io::Result<()> {
    parent.sync_all()
}

#[cfg(not(unix))]
fn sync_hooks_directory(_: &File) -> io::Result<()> {
    Ok(())
}

fn rename_bound_no_replace(parent: &File, from: &str, to: &str, expected: &File) -> io::Result<()> {
    validate_named_file_binding(parent, from, expected)?;
    #[cfg(windows)]
    rename_open_file_windows(parent, expected, to, false)?;
    #[cfg(not(windows))]
    rename_relative_no_replace(parent, from, to)?;

    if let Err(error) = validate_named_file_binding(parent, to, expected) {
        #[cfg(not(windows))]
        let _ = rename_relative_no_replace(parent, to, from);
        return Err(io::Error::other(format!(
            "renamed transaction object did not retain its destination name: {error}"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn rename_relative_replace(parent: &File, from: &str, to: &str) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd as _;
    let from = CString::new(from).map_err(|_| io::Error::other("rename source contains NUL"))?;
    let to = CString::new(to).map_err(|_| io::Error::other("rename target contains NUL"))?;
    // SAFETY: both names are rootless NUL-terminated strings and `parent` is a live directory.
    if unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_relative_exchange(parent: &File, first: &str, second: &str) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd as _;
    let first = CString::new(first).map_err(|_| io::Error::other("rename name contains NUL"))?;
    let second = CString::new(second).map_err(|_| io::Error::other("rename name contains NUL"))?;
    // SAFETY: both names are rootless NUL-terminated strings and `parent` is a live directory.
    if unsafe {
        libc::renameat2(
            parent.as_raw_fd(),
            first.as_ptr(),
            parent.as_raw_fd(),
            second.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn rename_relative_exchange(parent: &File, first: &str, second: &str) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd as _;
    let first = CString::new(first).map_err(|_| io::Error::other("rename name contains NUL"))?;
    let second = CString::new(second).map_err(|_| io::Error::other("rename name contains NUL"))?;
    // SAFETY: both names are rootless NUL-terminated strings and `parent` is a live directory.
    if unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            first.as_ptr(),
            parent.as_raw_fd(),
            second.as_ptr(),
            libc::RENAME_SWAP,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
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
fn rename_relative_exchange(_: &File, _: &str, _: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic exchange rename is unsupported on this platform",
    ))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_relative_no_replace(parent: &File, from: &str, to: &str) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd as _;
    let from = CString::new(from).map_err(|_| io::Error::other("rename source contains NUL"))?;
    let to = CString::new(to).map_err(|_| io::Error::other("rename target contains NUL"))?;
    // SAFETY: both names are rootless NUL-terminated strings and `parent` is a live directory.
    if unsafe {
        libc::renameat2(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn rename_relative_no_replace(parent: &File, from: &str, to: &str) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd as _;
    let from = CString::new(from).map_err(|_| io::Error::other("rename source contains NUL"))?;
    let to = CString::new(to).map_err(|_| io::Error::other("rename target contains NUL"))?;
    // SAFETY: both names are rootless NUL-terminated strings and `parent` is a live directory.
    if unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
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
fn rename_relative_no_replace(_: &File, _: &str, _: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace rename is unsupported on this platform",
    ))
}

#[cfg(windows)]
fn validate_windows_regular_file_links(
    file: &File,
    label: &str,
    expected_links: u64,
) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || hard_link_count_io(file, &metadata)? != expected_links {
        return Err(io::Error::other(format!(
            "{label} has an unexpected hard-link layout"
        )));
    }
    reject_reparse(&metadata)
}

#[cfg(windows)]
fn validate_windows_link_binding(
    parent: &File,
    name: &str,
    expected: &File,
    expected_links: u64,
) -> io::Result<()> {
    validate_windows_regular_file_links(expected, "hook transaction link", expected_links)?;
    let current = open_hook_target(parent, name, false)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("bound transaction link disappeared: {name}"),
        )
    })?;
    validate_windows_regular_file_links(&current, "hook transaction link", expected_links)?;
    if workspace_file_identity(&current)? != workspace_file_identity(expected)? {
        return Err(io::Error::other(format!(
            "transaction link changed identity: {name}"
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_windows_recovery_artifact(
    file: &File,
    name: &str,
    sha256: Option<&str>,
    mode: Option<u32>,
    expected_links: u64,
) -> io::Result<()> {
    validate_windows_regular_file_links(file, "hook transaction recovery object", expected_links)?;
    let mut file = file.try_clone()?;
    let metadata = file.metadata()?;
    if metadata.len() > MAX_HOOK_BYTES.max(MAX_TRANSACTION_JOURNAL_BYTES) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("hook transaction artifact exceeds its size limit: {name}"),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_HOOK_BYTES.max(MAX_TRANSACTION_JOURNAL_BYTES) + 1)
        .read_to_end(&mut bytes)?;
    if sha256.is_some_and(|expected| sha256_hex(&bytes) == expected)
        && current_mode(&metadata) == mode
    {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "hook transaction artifact is missing or changed: {name}"
        )))
    }
}

#[cfg(windows)]
fn open_windows_atomic_replace_source(
    parent: &File,
    name: &str,
    allow_transaction_link: bool,
) -> io::Result<File> {
    use fs_at::os::windows::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;

    let mut options = AtOpenOptions::default();
    options.follow(false).desired_access(FILE_GENERIC_READ);
    let source = options.open_at(parent, name)?;
    let metadata = source.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other(
            "hook transaction rename source is not a regular file",
        ));
    }
    reject_reparse(&metadata)?;
    let links = hard_link_count_io(&source, &metadata)?;
    if links != 1 && !(allow_transaction_link && links == 2) {
        return Err(io::Error::other(
            "hook transaction rename source has an unexpected hard-link layout",
        ));
    }
    let locked = reopen_windows_atomic_source(&source)?;
    validate_windows_link_binding(parent, name, &locked, links)?;
    Ok(locked)
}

#[cfg(windows)]
fn lock_windows_reserved_source(
    parent: &File,
    artifact: &mut ReservedArtifact,
) -> io::Result<File> {
    // The reservation handle was writable while its complete bytes were
    // staged. Replace it with a read-only handle before requesting a second
    // handle whose share mask denies both writes and deletes.
    let read_only = reopen_windows_read_source(&artifact.file)?;
    let writable = std::mem::replace(&mut artifact.file, read_only);
    drop(writable);
    let locked = reopen_windows_atomic_source(&artifact.file)?;
    validate_named_file_binding(parent, &artifact.name, &locked)?;
    Ok(locked)
}

#[cfg(windows)]
fn reopen_windows_read_source(source: &File) -> io::Result<File> {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{
            FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, ReOpenFile,
        },
    };

    // SAFETY: `source` remains live for the call and the returned handle is
    // independently owned.
    let handle = unsafe {
        ReOpenFile(
            source.as_raw_handle(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `ReOpenFile` returned a new owned handle which is transferred
    // exactly once into `File`.
    Ok(unsafe { File::from_raw_handle(handle) })
}

#[cfg(windows)]
fn reopen_windows_atomic_source(source: &File) -> io::Result<File> {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{DELETE, FILE_GENERIC_READ, FILE_SHARE_READ, ReOpenFile},
    };

    // The exact object is reopened with a share mask that denies competing
    // writes, renames, and deletes across the validation-to-publication seam.
    // SAFETY: `source` remains live for the call and the returned handle is
    // independently owned.
    let handle = unsafe {
        ReOpenFile(
            source.as_raw_handle(),
            DELETE | FILE_GENERIC_READ,
            FILE_SHARE_READ,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `ReOpenFile` returned a new owned handle which is transferred
    // exactly once into `File`.
    Ok(unsafe { File::from_raw_handle(handle) })
}

#[cfg(windows)]
fn windows_relative_name(name: &str) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt as _;

    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "transaction name must be one relative path component",
        ));
    }
    let name = OsStr::new(name).encode_wide().collect::<Vec<_>>();
    if name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "transaction name is empty",
        ));
    }
    Ok(name)
}

#[cfg(windows)]
fn link_open_file_windows(parent: &File, source: &File, to: &str) -> io::Result<()> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::{
        Wdk::Storage::FileSystem::{
            FILE_LINK_INFORMATION, FileLinkInformation, NtSetInformationFile,
        },
        Win32::{Foundation::RtlNtStatusToDosError, System::IO::IO_STATUS_BLOCK},
    };

    validate_private_regular_file(source, "hook transaction link source")?;
    let name = windows_relative_name(to)?;
    let name_bytes = name
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "link target is too long"))?;
    let information_size = std::mem::size_of::<FILE_LINK_INFORMATION>()
        .checked_add(name_bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "link target is too long"))?;
    let information_size_u32 = u32::try_from(information_size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "link target is too long"))?;
    let name_bytes_u32 = u32::try_from(name_bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "link target is too long"))?;
    let words = information_size.div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0_usize; words];
    let information = storage.as_mut_ptr().cast::<FILE_LINK_INFORMATION>();
    let mut io_status = MaybeUninit::<IO_STATUS_BLOCK>::zeroed();
    // SAFETY: `storage` is aligned and large enough for the fixed structure and
    // every UTF-16 unit. All handles and output storage remain live for the
    // synchronous call, and ReplaceIfExists=0 makes the nonce destination
    // fail closed if another object already occupies it.
    let status = unsafe {
        (*information).Anonymous.ReplaceIfExists = 0;
        (*information).RootDirectory = parent.as_raw_handle();
        (*information).FileNameLength = name_bytes_u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            name.len(),
        );
        NtSetInformationFile(
            source.as_raw_handle(),
            io_status.as_mut_ptr(),
            information.cast(),
            information_size_u32,
            FileLinkInformation,
        )
    };
    if status < 0 {
        // SAFETY: every NTSTATUS value may be translated to its Win32 error.
        let code = unsafe { RtlNtStatusToDosError(status) };
        Err(io::Error::from_raw_os_error(code as i32))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn rename_open_file_windows_posix_replace(
    parent: &File,
    source: &File,
    to: &str,
) -> io::Result<()> {
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_RENAME_POSIX_SEMANTICS, FILE_RENAME_REPLACE_IF_EXISTS,
    };

    rename_open_file_windows_with_flags(
        parent,
        source,
        to,
        FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS,
    )
}

#[cfg(windows)]
fn rename_open_file_windows_with_flags(
    parent: &File,
    source: &File,
    to: &str,
    flags: u32,
) -> io::Result<()> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::{
        Wdk::Storage::FileSystem::{
            FILE_RENAME_INFORMATION, FileRenameInformationEx, NtSetInformationFile,
        },
        Win32::{Foundation::RtlNtStatusToDosError, System::IO::IO_STATUS_BLOCK},
    };

    validate_private_regular_file(source, "hook transaction rename source")?;
    let name = windows_relative_name(to)?;
    let name_bytes = name
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "rename target is too long"))?;
    let information_size = std::mem::size_of::<FILE_RENAME_INFORMATION>()
        .checked_add(name_bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "rename target is too long"))?;
    let information_size_u32 = u32::try_from(information_size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "rename target is too long"))?;
    let name_bytes_u32 = u32::try_from(name_bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "rename target is too long"))?;
    let words = information_size.div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0_usize; words];
    let information = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let mut io_status = MaybeUninit::<IO_STATUS_BLOCK>::zeroed();
    // SAFETY: `storage` is aligned and large enough for the fixed structure and
    // every UTF-16 unit. All handles and output storage remain live for the
    // synchronous call.
    let status = unsafe {
        (*information).Anonymous.Flags = flags;
        (*information).RootDirectory = parent.as_raw_handle();
        (*information).FileNameLength = name_bytes_u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            name.len(),
        );
        NtSetInformationFile(
            source.as_raw_handle(),
            io_status.as_mut_ptr(),
            information.cast(),
            information_size_u32,
            FileRenameInformationEx,
        )
    };
    if status < 0 {
        // SAFETY: every NTSTATUS value may be translated to its Win32 error.
        let code = unsafe { RtlNtStatusToDosError(status) };
        Err(io::Error::from_raw_os_error(code as i32))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn remove_windows_recovery_link(
    parent: &File,
    name: &str,
    expected: &WindowsRecoveryFile,
    expected_links: u64,
) -> io::Result<()> {
    use fs_at::os::windows::FileExt as _;

    let locked = open_windows_atomic_replace_source(parent, name, true)?;
    validate_windows_link_binding(parent, name, &locked, expected_links)?;
    if workspace_file_identity(&locked)? != workspace_file_identity(&expected.file)? {
        return Err(io::Error::other(
            "transaction recovery link changed before deletion",
        ));
    }
    locked.delete_by_handle().map_err(|(_, error)| error)?;
    if open_hook_target(parent, name, false)?.is_some() {
        return Err(io::Error::other(
            "transaction recovery link remained after deletion",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn open_windows_rename_source(parent: &File, name: &str) -> io::Result<File> {
    use fs_at::os::windows::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;

    let mut options = AtOpenOptions::default();
    options.follow(false).desired_access(FILE_GENERIC_READ);
    let source = options.open_at(parent, name)?;
    validate_private_regular_file(&source, "hook transaction rename source")?;
    let locked = reopen_windows_rename_source(&source)?;
    validate_private_regular_file(&locked, "hook transaction rename source")?;
    validate_named_file_binding(parent, name, &locked)?;
    Ok(locked)
}

#[cfg(windows)]
fn reopen_windows_rename_source(source: &File) -> io::Result<File> {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{
            DELETE, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE, ReOpenFile,
        },
    };

    // `fs_at` shares delete access on every parent-relative open. Reopen the
    // exact object without FILE_SHARE_DELETE before validating its name so no
    // competing rename/delete handle can cross the validation-to-rename seam.
    // The first handle requests only READ, which keeps these share modes
    // compatible while the reopened handle acquires DELETE access.
    // SAFETY: `source` remains live for the call, and `ReOpenFile` returns an
    // independent owned handle without borrowing any caller-provided storage.
    let handle = unsafe {
        ReOpenFile(
            source.as_raw_handle(),
            DELETE | FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `ReOpenFile` returned a new owned handle which is transferred
    // exactly once into `File`.
    Ok(unsafe { File::from_raw_handle(handle) })
}

#[cfg(windows)]
fn open_windows_delete_source(parent: &File, name: &str) -> io::Result<File> {
    use fs_at::os::windows::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_READ_ATTRIBUTES};

    let mut options = AtOpenOptions::default();
    options
        .follow(false)
        .desired_access(DELETE | FILE_READ_ATTRIBUTES);
    let source = options.open_at(parent, name)?;
    validate_private_regular_file(&source, "hook transaction deletion source")?;
    Ok(source)
}

#[cfg(windows)]
fn rename_open_file_windows(
    parent: &File,
    source: &File,
    to: &str,
    replace: bool,
) -> io::Result<()> {
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_RENAME_POSIX_SEMANTICS, FILE_RENAME_REPLACE_IF_EXISTS,
    };

    rename_open_file_windows_with_flags(
        parent,
        source,
        to,
        if replace {
            FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS
        } else {
            0
        },
    )
}

#[cfg(not(any(unix, windows)))]
fn rename_relative_replace(_: &File, _: &str, _: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "relative atomic rename is unsupported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn rename_relative_no_replace(_: &File, _: &str, _: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "relative atomic no-replace rename is unsupported on this platform",
    ))
}

fn format_mutation_failure(operation: &str, error: io::Error, rollback: io::Result<()>) -> String {
    match rollback {
        Ok(()) => format!("{operation} failed and all prior changes were rolled back: {error}"),
        Err(rollback) => {
            format!("{operation} failed: {error}; rollback was incomplete: {rollback}")
        }
    }
}

fn summarize_hook_states(hooks: &[RepositoryHookTargetReport]) -> String {
    let states = hooks
        .iter()
        .map(|hook| hook.state.as_str())
        .collect::<HashSet<_>>();
    if states.len() == 1 {
        return states.into_iter().next().unwrap_or("unknown").to_string();
    }
    for refusal in [
        "managed_block_malformed",
        "uninstall_required",
        "hook_target_uninspectable",
        "hook_target_not_regular",
        "hook_target_hardlinked",
        "hook_not_executable",
        "hook_binary_content",
        "hook_too_large",
        "unsupported_hook_shell",
        "unterminated_hook_shebang",
    ] {
        if states.contains(refusal) {
            return refusal.to_string();
        }
    }
    if states.contains("installed") {
        "partially_installed".to_string()
    } else if states.contains("foreign_hook_present") {
        "foreign_hook_present".to_string()
    } else {
        "not_installed".to_string()
    }
}

#[cfg(test)]
type TransactionStepHook = Box<dyn FnMut(&str, usize) -> io::Result<()>>;

#[cfg(test)]
thread_local! {
    static BEFORE_HOOKS_DIRECTORY_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_HOOK_TARGET_WRITE_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_TRANSACTION_LOCK_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static TRANSACTION_STEP_HOOK: std::cell::RefCell<Option<TransactionStepHook>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn run_before_hooks_directory_open_hook() {
    BEFORE_HOOKS_DIRECTORY_OPEN_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn run_before_hook_target_write_open_hook() {
    BEFORE_HOOK_TARGET_WRITE_OPEN_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn run_after_transaction_lock_hook() {
    AFTER_TRANSACTION_LOCK_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn run_transaction_step_hook(step: &str, index: usize) -> io::Result<()> {
    TRANSACTION_STEP_HOOK.with(|hook| {
        hook.borrow_mut()
            .as_mut()
            .map_or(Ok(()), |hook| hook(step, index))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{TempDir, tempdir};

    struct RepositoryFixture {
        _temp: TempDir,
        project: PathBuf,
        git_dir: PathBuf,
        hooks: PathBuf,
    }

    fn repository_fixture() -> RepositoryFixture {
        let temp = tempdir().expect("temporary repository parent");
        let project = temp.path().join("project with ' quote");
        let git_dir = project.join(".git");
        let hooks = git_dir.join("hooks");
        fs::create_dir_all(&hooks).expect("create repository hooks directory");
        fs::write(git_dir.join("config"), b"").expect("write repository config");
        RepositoryFixture {
            _temp: temp,
            project,
            git_dir,
            hooks,
        }
    }

    fn isolated_environment(fixture: &RepositoryFixture) -> HookConfigEnvironment {
        let home = fixture.project.join("test-home");
        fs::create_dir_all(&home).expect("create isolated home");
        let mut environment = HookConfigEnvironment::empty(&fixture.project);
        environment.set("HOME", home.into_os_string());
        environment.set("GIT_CONFIG_NOSYSTEM", "true");
        environment
    }

    fn request(fixture: &RepositoryFixture, action: RepositoryHookAction) -> RepositoryHookRequest {
        RepositoryHookRequest {
            action,
            project_root: fixture.project.clone(),
            plugin_data_dir: fixture.project.join("plugin data ' quoted"),
            node_path: fixture.project.join("node executable"),
            script_path: fixture.project.join("codestory-dirty-hook.cjs"),
        }
    }

    fn run(
        fixture: &RepositoryFixture,
        action: RepositoryHookAction,
        environment: &HookConfigEnvironment,
    ) -> RepositoryHookReport {
        manage_repository_hooks_inner(&request(fixture, action), environment)
            .unwrap_or_else(HookFailure::report)
    }

    fn write_executable(path: &Path, contents: &[u8]) {
        fs::write(path, contents).expect("write hook fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(path, fs::Permissions::from_mode(0o751))
                .expect("make hook executable");
        }
    }

    fn canonical(path: impl AsRef<Path>) -> PathBuf {
        fs::canonicalize(path).expect("canonical fixture path")
    }

    fn transaction_artifacts(hooks: &Path) -> Vec<OsString> {
        let mut names = fs::read_dir(hooks)
            .expect("read hooks directory")
            .map(|entry| entry.expect("hooks entry").file_name())
            .filter(|name| {
                let name = name.to_string_lossy();
                name.starts_with(".codestory-hooks-") && name != TRANSACTION_LOCK_NAME
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn clear_transaction_step_hook() {
        TRANSACTION_STEP_HOOK.with(|slot| *slot.borrow_mut() = None);
    }

    fn recover_transaction(fixture: &RepositoryFixture) -> io::Result<()> {
        let parent = open_root_no_follow(&fixture.hooks)?;
        let lock = acquire_transaction_lock(&parent)?;
        recover_pending_transaction(&parent, &lock)
    }

    #[test]
    fn default_install_is_idempotent_and_owned_uninstall_deletes_exact_templates() {
        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);

        let installed = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(installed.status, "installed", "{installed:?}");
        assert!(installed.hooks.iter().all(|hook| hook.changed));
        let first = HOOK_NAMES
            .iter()
            .map(|name| {
                let path = fixture.hooks.join(name);
                let bytes = fs::read(&path).expect("read installed hook");
                assert!(bytes.starts_with(b"#!/bin/sh\n# >>> codestory dirty marker >>>\n"));
                assert!(bytes.ends_with(b"# <<< codestory dirty marker <<<\n"));
                let modified = fs::metadata(&path).unwrap().modified().unwrap();
                (path, bytes, modified)
            })
            .collect::<Vec<_>>();

        let repeated = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(repeated.status, "installed", "{repeated:?}");
        assert!(repeated.hooks.iter().all(|hook| !hook.changed));
        for (path, bytes, modified) in &first {
            assert_eq!(&fs::read(path).unwrap(), bytes);
            assert_eq!(&fs::metadata(path).unwrap().modified().unwrap(), modified);
        }

        let removed = run(&fixture, RepositoryHookAction::Uninstall, &environment);
        assert_eq!(removed.status, "not_installed", "{removed:?}");
        assert!(removed.hooks.iter().all(|hook| hook.changed));
        assert!(
            HOOK_NAMES
                .iter()
                .all(|name| !fixture.hooks.join(name).exists())
        );
    }

    #[test]
    fn clean_status_is_strictly_observational() {
        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let before = fs::read_dir(&fixture.hooks)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();

        let report = run(&fixture, RepositoryHookAction::Status, &environment);

        assert_eq!(report.status, "not_installed", "{report:?}");
        let after = fs::read_dir(&fixture.hooks)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(after, before);
        assert!(!fixture.hooks.join(TRANSACTION_LOCK_NAME).exists());
    }

    #[cfg(unix)]
    #[test]
    fn replacing_the_fixed_lock_name_after_acquisition_refuses_mutation() {
        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let lock_path = fixture.hooks.join(TRANSACTION_LOCK_NAME);
        let displaced_lock = fixture.hooks.join("transaction-lock-displaced");
        AFTER_TRANSACTION_LOCK_HOOK.with(|slot| {
            let lock_path = lock_path.clone();
            let displaced_lock = displaced_lock.clone();
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::rename(&lock_path, &displaced_lock).expect("displace locked name");
                fs::write(&lock_path, b"replacement").expect("replace fixed lock name");
            }));
        });

        let refused = run(&fixture, RepositoryHookAction::Install, &environment);

        assert_eq!(refused.status, "hook_mutation_failed", "{refused:?}");
        assert!(
            refused
                .message
                .as_deref()
                .is_some_and(|message| message.contains("no longer bound")),
            "the held lock must remain attached to its fixed directory entry"
        );
        assert!(
            HOOK_NAMES
                .iter()
                .all(|name| !fixture.hooks.join(name).exists())
        );
        assert!(transaction_artifacts(&fixture.hooks).is_empty());
    }

    #[test]
    fn foreign_crlf_hook_is_spliced_after_shebang_and_restored_byte_for_byte() {
        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let hook = fixture.hooks.join("post-merge");
        let original = b"#!/usr/bin/env bash\r\nexit 0\r\nprintf 'never reached\\n'\r\n";
        write_executable(&hook, original);
        #[cfg(unix)]
        let original_mode = {
            use std::os::unix::fs::MetadataExt as _;
            fs::metadata(&hook).unwrap().mode()
        };

        let installed = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(installed.status, "installed", "{installed:?}");
        let bytes = fs::read(&hook).unwrap();
        let marker = occurrences(&bytes, MANAGED_START);
        assert_eq!(marker, vec![b"#!/usr/bin/env bash\r\n".len()]);
        assert!(
            bytes
                .windows(b"\r\nexit 0\r\n".len())
                .any(|window| window == b"\r\nexit 0\r\n")
        );
        assert!(
            bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| { *byte != b'\n' || index > 0 && bytes[index - 1] == b'\r' })
        );
        assert!(String::from_utf8_lossy(&bytes).contains("'\\''"));

        let removed = run(&fixture, RepositoryHookAction::Uninstall, &environment);
        assert_eq!(removed.status, "foreign_hook_present", "{removed:?}");
        assert_eq!(fs::read(&hook).unwrap(), original);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(fs::metadata(&hook).unwrap().mode(), original_mode);
        }
    }

    #[test]
    fn shebang_only_foreign_hook_survives_install_and_uninstall_byte_for_byte() {
        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let hook = fixture.hooks.join("post-checkout");
        let original = b"#!/bin/sh\n";
        write_executable(&hook, original);

        let installed = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(installed.status, "installed", "{installed:?}");
        let installed_bytes = fs::read(&hook).expect("installed foreign hook");
        assert!(
            installed_bytes
                .windows(MANAGED_START.len())
                .any(|part| part == MANAGED_START)
        );
        assert!(
            !installed_bytes
                .windows(OWNED_DISPATCHER_MARKER.len())
                .any(|part| part == OWNED_DISPATCHER_MARKER),
            "a pre-existing dispatcher must not acquire CodeStory-owned identity"
        );

        let removed = run(&fixture, RepositoryHookAction::Uninstall, &environment);
        assert_eq!(removed.status, "foreign_hook_present", "{removed:?}");
        assert_eq!(fs::read(&hook).unwrap(), original);
        assert!(hook.is_file(), "the foreign dispatcher remains present");
    }

    #[test]
    fn one_incompatible_hook_keeps_the_complete_trio_unchanged() {
        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let checkout = fixture.hooks.join("post-checkout");
        let merge = fixture.hooks.join("post-merge");
        write_executable(&checkout, b"#!/bin/sh\necho keep-me\n");
        write_executable(&merge, b"#!/usr/bin/env python3\nprint('foreign')\n");
        let checkout_before = fs::read(&checkout).unwrap();
        let merge_before = fs::read(&merge).unwrap();
        let checkout_mtime = fs::metadata(&checkout).unwrap().modified().unwrap();
        let merge_mtime = fs::metadata(&merge).unwrap().modified().unwrap();

        let refused = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(refused.status, "unsupported_hook_shell", "{refused:?}");
        assert_eq!(fs::read(&checkout).unwrap(), checkout_before);
        assert_eq!(fs::read(&merge).unwrap(), merge_before);
        assert_eq!(
            fs::metadata(&checkout).unwrap().modified().unwrap(),
            checkout_mtime
        );
        assert_eq!(
            fs::metadata(&merge).unwrap().modified().unwrap(),
            merge_mtime
        );
        assert!(!fixture.hooks.join("post-rewrite").exists());
    }

    #[test]
    fn every_apply_boundary_rolls_back_create_write_and_delete_plans() {
        for failure_index in 0..HOOK_NAMES.len() {
            let fixture = repository_fixture();
            let environment = isolated_environment(&fixture);
            TRANSACTION_STEP_HOOK.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move |step, index| {
                    if step == "after_apply" && index == failure_index {
                        Err(io::Error::other("injected create failure"))
                    } else {
                        Ok(())
                    }
                }));
            });
            let refused = run(&fixture, RepositoryHookAction::Install, &environment);
            clear_transaction_step_hook();
            assert_eq!(refused.status, "hook_mutation_failed", "{refused:?}");
            assert!(
                HOOK_NAMES
                    .iter()
                    .all(|name| !fixture.hooks.join(name).exists()),
                "create rollback {failure_index} restores absence"
            );
            assert!(transaction_artifacts(&fixture.hooks).is_empty());
        }

        for failure_index in 0..HOOK_NAMES.len() {
            let fixture = repository_fixture();
            let environment = isolated_environment(&fixture);
            let originals = HOOK_NAMES
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let bytes = format!("#!/bin/sh\necho foreign-{index}\n").into_bytes();
                    write_executable(&fixture.hooks.join(name), &bytes);
                    (name, bytes)
                })
                .collect::<Vec<_>>();
            TRANSACTION_STEP_HOOK.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move |step, index| {
                    if step == "after_apply" && index == failure_index {
                        Err(io::Error::other("injected write failure"))
                    } else {
                        Ok(())
                    }
                }));
            });
            let refused = run(&fixture, RepositoryHookAction::Install, &environment);
            clear_transaction_step_hook();
            assert_eq!(refused.status, "hook_mutation_failed", "{refused:?}");
            for (name, original) in originals {
                assert_eq!(fs::read(fixture.hooks.join(name)).unwrap(), original);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    assert_eq!(
                        fs::metadata(fixture.hooks.join(name))
                            .unwrap()
                            .permissions()
                            .mode()
                            & 0o7777,
                        0o751
                    );
                }
            }
            assert!(transaction_artifacts(&fixture.hooks).is_empty());
        }

        for failure_index in 0..HOOK_NAMES.len() {
            let fixture = repository_fixture();
            let environment = isolated_environment(&fixture);
            let installed = run(&fixture, RepositoryHookAction::Install, &environment);
            assert_eq!(installed.status, "installed", "{installed:?}");
            let originals = HOOK_NAMES
                .iter()
                .map(|name| (name, fs::read(fixture.hooks.join(name)).unwrap()))
                .collect::<Vec<_>>();
            TRANSACTION_STEP_HOOK.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move |step, index| {
                    if step == "after_apply" && index == failure_index {
                        Err(io::Error::other("injected delete failure"))
                    } else {
                        Ok(())
                    }
                }));
            });
            let refused = run(&fixture, RepositoryHookAction::Uninstall, &environment);
            clear_transaction_step_hook();
            assert_eq!(refused.status, "hook_mutation_failed", "{refused:?}");
            for (name, original) in originals {
                assert_eq!(fs::read(fixture.hooks.join(name)).unwrap(), original);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    assert_eq!(
                        fs::metadata(fixture.hooks.join(name))
                            .unwrap()
                            .permissions()
                            .mode()
                            & 0o7777,
                        0o755
                    );
                }
            }
            assert!(transaction_artifacts(&fixture.hooks).is_empty());
        }
    }

    #[test]
    fn hook_readers_observe_only_complete_old_or_new_bytes() {
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        use std::thread;
        use std::time::Duration;

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let hook = fixture.hooks.join("post-checkout");
        let mut original = b"#!/bin/sh\n".to_vec();
        original.resize(256 * 1024, b'x');
        original.push(b'\n');
        write_executable(&hook, &original);
        let invocation = HookInvocation::from_request(
            &request(&fixture, RepositoryHookAction::Install),
            &canonical(&fixture.project),
        )
        .expect("hook invocation");
        let shebang_end = b"#!/bin/sh\n".len();
        let mut next = Vec::new();
        next.extend_from_slice(&original[..shebang_end]);
        next.extend_from_slice(&invocation.managed_segment("post-checkout", b"\n", false));
        next.extend_from_slice(&original[shebang_end..]);

        let started = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let old_reads = Arc::new(AtomicUsize::new(0));
        let new_reads = Arc::new(AtomicUsize::new(0));
        let invalid = Arc::new(Mutex::new(None::<String>));
        let reader = {
            let hook = hook.clone();
            let original = original.clone();
            let next = next.clone();
            let started = Arc::clone(&started);
            let stopped = Arc::clone(&stopped);
            let old_reads = Arc::clone(&old_reads);
            let new_reads = Arc::clone(&new_reads);
            let invalid = Arc::clone(&invalid);
            thread::spawn(move || {
                while !started.load(Ordering::Acquire) {
                    thread::yield_now();
                }
                while !stopped.load(Ordering::Acquire) {
                    match fs::read(&hook) {
                        Ok(bytes) if bytes == original => {
                            old_reads.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(bytes) if bytes == next => {
                            new_reads.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(bytes) => {
                            *invalid.lock().unwrap() =
                                Some(format!("observed {} unexpected bytes", bytes.len()));
                            break;
                        }
                        Err(error) => {
                            *invalid.lock().unwrap() = Some(format!("read failed: {error}"));
                            break;
                        }
                    }
                }
            })
        };
        TRANSACTION_STEP_HOOK.with(|slot| {
            let started = Arc::clone(&started);
            *slot.borrow_mut() = Some(Box::new(move |step, index| {
                if index == 0 && step == "before_apply" {
                    started.store(true, Ordering::Release);
                    thread::sleep(Duration::from_millis(25));
                } else if index == 0 && step == "after_apply" {
                    thread::sleep(Duration::from_millis(25));
                }
                Ok(())
            }));
        });

        let installed = run(&fixture, RepositoryHookAction::Install, &environment);
        clear_transaction_step_hook();
        stopped.store(true, Ordering::Release);
        reader.join().expect("reader thread");

        assert_eq!(installed.status, "installed", "{installed:?}");
        assert_eq!(*invalid.lock().unwrap(), None);
        assert!(
            old_reads.load(Ordering::Relaxed) > 0,
            "reader saw old bytes"
        );
        assert!(
            new_reads.load(Ordering::Relaxed) > 0,
            "reader saw new bytes"
        );
        assert_eq!(fs::read(&hook).unwrap(), next);
    }

    #[test]
    fn interrupted_transaction_is_observational_in_status_and_recovers_on_mutation() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        TRANSACTION_STEP_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(|step, index| {
                if step == "after_apply" && index == 0 {
                    panic!("simulated process interruption");
                }
                Ok(())
            }));
        });
        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            run(&fixture, RepositoryHookAction::Install, &environment)
        }));
        clear_transaction_step_hook();
        assert!(
            interrupted.is_err(),
            "the fault seam interrupted the transaction"
        );
        assert!(fixture.hooks.join(TRANSACTION_JOURNAL_NAME).is_file());
        assert!(fixture.hooks.join(TRANSACTION_READY_NAME).is_file());
        let journal_before = fs::read(fixture.hooks.join(TRANSACTION_JOURNAL_NAME)).unwrap();
        let journal_mtime = fs::metadata(fixture.hooks.join(TRANSACTION_JOURNAL_NAME))
            .unwrap()
            .modified()
            .unwrap();
        let checkout_before = fs::read(fixture.hooks.join("post-checkout")).unwrap();

        let status = run(&fixture, RepositoryHookAction::Status, &environment);

        assert_eq!(status.status, "hook_recovery_required", "{status:?}");
        assert_eq!(
            fs::read(fixture.hooks.join(TRANSACTION_JOURNAL_NAME)).unwrap(),
            journal_before
        );
        assert_eq!(
            fs::metadata(fixture.hooks.join(TRANSACTION_JOURNAL_NAME))
                .unwrap()
                .modified()
                .unwrap(),
            journal_mtime
        );
        assert_eq!(
            fs::read(fixture.hooks.join("post-checkout")).unwrap(),
            checkout_before
        );

        let recovered = run(&fixture, RepositoryHookAction::Uninstall, &environment);
        assert_eq!(recovered.status, "not_installed", "{recovered:?}");
        assert!(
            HOOK_NAMES
                .iter()
                .all(|name| !fixture.hooks.join(name).exists())
        );
        assert!(transaction_artifacts(&fixture.hooks).is_empty());
    }

    #[test]
    fn no_ready_recovery_ignores_and_preserves_a_concurrent_hook_edit() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        TRANSACTION_STEP_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(|step, _| {
                if step == "after_journal" {
                    panic!("simulated interruption before READY");
                }
                Ok(())
            }));
        });
        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            run(&fixture, RepositoryHookAction::Install, &environment)
        }));
        clear_transaction_step_hook();
        assert!(interrupted.is_err());
        assert!(fixture.hooks.join(TRANSACTION_JOURNAL_NAME).is_file());
        assert!(!fixture.hooks.join(TRANSACTION_READY_NAME).exists());

        let concurrent = b"#!/bin/sh\necho no-ready-concurrent\n";
        write_executable(&fixture.hooks.join("post-checkout"), concurrent);
        recover_transaction(&fixture).expect("recover pre-READY transaction");

        assert_eq!(
            fs::read(fixture.hooks.join("post-checkout")).unwrap(),
            concurrent
        );
        assert!(transaction_artifacts(&fixture.hooks).is_empty());
    }

    #[test]
    fn ready_before_apply_recovery_preserves_a_concurrent_hook_edit() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        TRANSACTION_STEP_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(|step, _| {
                if step == "after_ready" {
                    panic!("simulated interruption after READY");
                }
                Ok(())
            }));
        });
        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            run(&fixture, RepositoryHookAction::Install, &environment)
        }));
        clear_transaction_step_hook();
        assert!(interrupted.is_err());
        assert!(fixture.hooks.join(TRANSACTION_READY_NAME).is_file());

        let concurrent = b"#!/bin/sh\necho ready-concurrent\n";
        write_executable(&fixture.hooks.join("post-checkout"), concurrent);
        recover_transaction(&fixture).expect("recover ready transaction before apply");

        assert_eq!(
            fs::read(fixture.hooks.join("post-checkout")).unwrap(),
            concurrent
        );
        assert!(transaction_artifacts(&fixture.hooks).is_empty());
    }

    #[test]
    fn committed_cleanup_does_not_inspect_or_revert_a_later_hook_edit() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        TRANSACTION_STEP_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(|step, _| {
                if step == "after_commit" {
                    panic!("simulated interruption after COMMIT");
                }
                Ok(())
            }));
        });
        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            run(&fixture, RepositoryHookAction::Install, &environment)
        }));
        clear_transaction_step_hook();
        assert!(interrupted.is_err());
        assert!(fixture.hooks.join(TRANSACTION_COMMIT_NAME).is_file());

        let concurrent = b"#!/bin/sh\necho after-commit-concurrent\n";
        write_executable(&fixture.hooks.join("post-checkout"), concurrent);
        recover_transaction(&fixture).expect("clean committed transaction");

        assert_eq!(
            fs::read(fixture.hooks.join("post-checkout")).unwrap(),
            concurrent
        );
        assert!(transaction_artifacts(&fixture.hooks).is_empty());
    }

    #[test]
    fn unsupported_or_missing_shell_shebangs_are_refused_without_changes() {
        for contents in [
            b"echo no-shebang\n".as_slice(),
            b"#!/usr/bin/env python3\nprint('foreign')\n".as_slice(),
            b"#!/usr/bin/env node\nprocess.exit(0)\n".as_slice(),
            b"#!/usr/bin/env ruby\nexit 0\n".as_slice(),
        ] {
            let fixture = repository_fixture();
            let environment = isolated_environment(&fixture);
            let hook = fixture.hooks.join("post-checkout");
            write_executable(&hook, contents);
            let before = fs::read(&hook).unwrap();
            let metadata = fs::metadata(&hook).unwrap();
            let modified = metadata.modified().unwrap();

            let refused = run(&fixture, RepositoryHookAction::Install, &environment);
            assert_eq!(refused.status, "unsupported_hook_shell", "{refused:?}");
            assert_eq!(fs::read(&hook).unwrap(), before);
            assert_eq!(fs::metadata(&hook).unwrap().modified().unwrap(), modified);
            assert!(!fixture.hooks.join("post-merge").exists());
            assert!(!fixture.hooks.join("post-rewrite").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_foreign_hook_is_refused_without_changes() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let hook = fixture.hooks.join("post-checkout");
        fs::write(&hook, b"#!/bin/sh\necho foreign\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o640)).unwrap();
        let before = fs::read(&hook).unwrap();
        let metadata = fs::metadata(&hook).unwrap();

        let refused = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(refused.status, "hook_not_executable", "{refused:?}");
        assert_eq!(fs::read(&hook).unwrap(), before);
        let after = fs::metadata(&hook).unwrap();
        assert_eq!(after.mode(), metadata.mode());
        assert_eq!(after.modified().unwrap(), metadata.modified().unwrap());
        assert!(!fixture.hooks.join("post-merge").exists());
        assert!(!fixture.hooks.join("post-rewrite").exists());
    }

    #[test]
    fn stale_and_duplicate_managed_blocks_are_actionable_and_untouched() {
        for (expected_status, managed) in [
            (
                "uninstall_required",
                b"#!/bin/sh\n# >>> codestory dirty marker >>>\nold command || true\n# <<< codestory dirty marker <<<\necho foreign\n".as_slice(),
            ),
            (
                "managed_block_malformed",
                b"#!/bin/sh\n# >>> codestory dirty marker >>>\none\n# >>> codestory dirty marker >>>\ntwo\n# <<< codestory dirty marker <<<\n".as_slice(),
            ),
        ] {
            let fixture = repository_fixture();
            let environment = isolated_environment(&fixture);
            let hook = fixture.hooks.join("post-checkout");
            write_executable(&hook, managed);
            let before = fs::read(&hook).unwrap();
            let modified = fs::metadata(&hook).unwrap().modified().unwrap();

            let refused = run(&fixture, RepositoryHookAction::Uninstall, &environment);
            assert_eq!(refused.status, expected_status, "{refused:?}");
            assert_eq!(fs::read(&hook).unwrap(), before);
            assert_eq!(fs::metadata(&hook).unwrap().modified().unwrap(), modified);
        }
    }

    #[cfg(unix)]
    #[test]
    fn terminal_symlink_and_hardlink_targets_are_refused_without_touching_aliases() {
        use std::os::unix::fs::symlink;

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let outside = fixture.project.parent().unwrap().join("outside-hook");
        write_executable(&outside, b"#!/bin/sh\necho outside\n");
        let outside_before = fs::read(&outside).unwrap();
        symlink(&outside, fixture.hooks.join("post-checkout")).expect("create terminal symlink");
        let refused = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(refused.status, "hook_target_symlink", "{refused:?}");
        assert_eq!(fs::read(&outside).unwrap(), outside_before);
        assert!(!fixture.hooks.join("post-merge").exists());

        fs::remove_file(fixture.hooks.join("post-checkout")).unwrap();
        fs::hard_link(&outside, fixture.hooks.join("post-merge")).expect("create hard link");
        let refused = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(refused.status, "hook_target_hardlinked", "{refused:?}");
        assert_eq!(fs::read(&outside).unwrap(), outside_before);
        assert!(!fixture.hooks.join("post-rewrite").exists());
    }

    #[cfg(unix)]
    #[test]
    fn fifo_directory_binary_and_oversized_targets_fail_without_blocking_or_mutating() {
        use std::os::unix::fs::PermissionsExt as _;

        for expected in [
            "hook_target_not_regular",
            "hook_binary_content",
            "hook_too_large",
        ] {
            let fixture = repository_fixture();
            let environment = isolated_environment(&fixture);
            let hook = fixture.hooks.join("post-merge");
            match expected {
                "hook_target_not_regular" => fs::create_dir(&hook).unwrap(),
                "hook_binary_content" => write_executable(&hook, b"#!/bin/sh\n\0binary\n"),
                "hook_too_large" => {
                    let mut bytes = b"#!/bin/sh\n".to_vec();
                    bytes.resize(MAX_HOOK_BYTES as usize + 1, b'x');
                    write_executable(&hook, &bytes);
                }
                _ => unreachable!(),
            }
            let refused = run(&fixture, RepositoryHookAction::Install, &environment);
            assert_eq!(refused.status, expected, "{refused:?}");
            assert!(!fixture.hooks.join("post-checkout").exists());
            assert!(!fixture.hooks.join("post-rewrite").exists());
        }

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let fifo = fixture.hooks.join("post-merge");
        use std::os::unix::ffi::OsStrExt as _;
        let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: the path is a valid NUL-terminated string and mode is ordinary permission data.
        assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o755) }, 0);
        fs::set_permissions(&fifo, fs::Permissions::from_mode(0o755)).unwrap();
        let refused = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(refused.status, "hook_target_not_regular", "{refused:?}");
    }

    #[cfg(unix)]
    #[test]
    fn inspection_to_write_open_swap_cannot_redirect_mutation() {
        use std::os::unix::fs::symlink;

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let hook = fixture.hooks.join("post-checkout");
        let moved = fixture.hooks.join("post-checkout-inspected");
        let outside = fixture.project.parent().unwrap().join("outside-swap-hook");
        write_executable(&hook, b"#!/bin/sh\necho original\n");
        write_executable(&outside, b"#!/bin/sh\necho outside\n");
        let outside_before = fs::read(&outside).unwrap();
        BEFORE_HOOK_TARGET_WRITE_OPEN_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::rename(&hook, &moved).expect("move inspected hook");
                symlink(&outside, &hook).expect("replace hook with symlink");
            }));
        });

        let refused = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(refused.status, "hook_mutation_failed", "{refused:?}");
        assert!(
            refused
                .message
                .as_deref()
                .is_some_and(|message| message.contains("all prior changes were rolled back")),
            "the swapped target is restored without wedging recovery: {refused:?}"
        );
        assert_eq!(
            fs::read(fixture.project.parent().unwrap().join("outside-swap-hook")).unwrap(),
            outside_before
        );
        assert!(
            fs::symlink_metadata(fixture.hooks.join("post-checkout"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the concurrent replacement remains bound to the hook name"
        );
        assert!(!fixture.hooks.join("post-merge").exists());
        assert!(!fixture.hooks.join("post-rewrite").exists());
        assert!(transaction_artifacts(&fixture.hooks).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn post_publication_edit_keeps_the_visible_hook_name() {
        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let hook = fixture.hooks.join("post-checkout");
        let published = fixture.hooks.join("post-checkout-published");
        let original = b"#!/bin/sh\necho original\n".to_vec();
        let concurrent = b"#!/bin/sh\necho concurrent\n".to_vec();
        write_executable(&hook, &original);
        TRANSACTION_STEP_HOOK.with(|slot| {
            let hook = hook.clone();
            let concurrent = concurrent.clone();
            *slot.borrow_mut() = Some(Box::new(move |step, index| {
                if step == "after_write_publish" && index == 0 {
                    fs::rename(&hook, &published)?;
                    write_executable(&hook, &concurrent);
                }
                Ok(())
            }));
        });

        let refused = run(&fixture, RepositoryHookAction::Install, &environment);
        clear_transaction_step_hook();

        assert_eq!(refused.status, "hook_mutation_failed", "{refused:?}");
        assert_eq!(fs::read(&hook).unwrap(), concurrent);
        assert!(
            refused.message.as_deref().is_some_and(|message| {
                message.contains("a concurrent hook edit was preserved")
            }),
            "the displaced original remains available without hiding the concurrent edit: {refused:?}"
        );
        let artifacts = transaction_artifacts(&fixture.hooks);
        assert_eq!(artifacts.len(), 1, "{artifacts:?}");
        assert_eq!(
            fs::read(fixture.hooks.join(&artifacts[0])).unwrap(),
            original
        );
    }

    #[cfg(unix)]
    #[test]
    fn delete_final_boundary_swap_preserves_the_concurrent_hook() {
        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let installed = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(installed.status, "installed", "{installed:?}");
        let hook = fixture.hooks.join("post-checkout");
        let moved = fixture.hooks.join("post-checkout-before-concurrent-edit");
        let concurrent = b"#!/bin/sh\necho concurrent-delete-edit\n".to_vec();
        BEFORE_HOOK_TARGET_WRITE_OPEN_HOOK.with(|slot| {
            let concurrent = concurrent.clone();
            let hook = hook.clone();
            let moved_for_swap = moved.clone();
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::rename(&hook, &moved_for_swap).expect("move inspected hook");
                write_executable(&hook, &concurrent);
            }));
        });

        let refused = run(&fixture, RepositoryHookAction::Uninstall, &environment);

        assert_eq!(refused.status, "hook_mutation_failed", "{refused:?}");
        assert_eq!(
            fs::read(fixture.hooks.join("post-checkout")).unwrap(),
            concurrent
        );
        assert!(moved.is_file(), "the preflight object was never unlinked");
        assert!(transaction_artifacts(&fixture.hooks).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn hooks_directory_swap_is_stopped_by_the_pinned_no_follow_walk() {
        use std::os::unix::fs::symlink;

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let configured = fixture.project.join("safe-hooks");
        let moved = fixture.project.join("safe-hooks-original");
        let outside = fixture.project.parent().unwrap().join("outside-hooks");
        fs::create_dir(&configured).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"outside").unwrap();
        fs::write(
            fixture.git_dir.join("config"),
            b"[core]\n\thooksPath = safe-hooks\n",
        )
        .unwrap();
        BEFORE_HOOKS_DIRECTORY_OPEN_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::rename(&configured, &moved).expect("move accepted hooks directory");
                symlink(&outside, &configured).expect("replace hooks directory with symlink");
            }));
        });

        let refused = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(refused.status, "hooks_path_unproven", "{refused:?}");
        assert_eq!(
            fs::read(
                fixture
                    .project
                    .parent()
                    .unwrap()
                    .join("outside-hooks/sentinel")
            )
            .unwrap(),
            b"outside"
        );
    }

    #[test]
    fn config_precedence_is_system_xdg_user_local_worktree_then_environment() {
        let fixture = repository_fixture();
        let mut environment = isolated_environment(&fixture);
        environment.remove("GIT_CONFIG_NOSYSTEM");
        environment.set(
            "PATH",
            std::env::var_os("PATH").expect("test process has an active Git search path"),
        );
        let system = fixture.project.join("system.gitconfig");
        environment.set("GIT_CONFIG_SYSTEM", system.clone().into_os_string());
        let home = PathBuf::from(environment.get("HOME").unwrap());
        let xdg = home.join(".config/git/config");
        let user = home.join(".gitconfig");
        for name in ["system", "xdg", "user", "local", "worktree", "environment"] {
            fs::create_dir_all(fixture.project.join(format!("{name}-hooks"))).unwrap();
        }

        fs::write(&system, b"[core]\n\thooksPath = system-hooks\n").unwrap();
        assert_eq!(
            run(&fixture, RepositoryHookAction::Status, &environment).hooks_path,
            Some(canonical(fixture.project.join("system-hooks")))
        );
        fs::create_dir_all(xdg.parent().unwrap()).unwrap();
        fs::write(&xdg, b"[core]\n\thooksPath = xdg-hooks\n").unwrap();
        assert_eq!(
            run(&fixture, RepositoryHookAction::Status, &environment).hooks_path,
            Some(canonical(fixture.project.join("xdg-hooks")))
        );
        fs::write(&user, b"[core]\n\thooksPath = user-hooks\n").unwrap();
        assert_eq!(
            run(&fixture, RepositoryHookAction::Status, &environment).hooks_path,
            Some(canonical(fixture.project.join("user-hooks")))
        );
        fs::write(
            fixture.git_dir.join("config"),
            b"[core]\n\thooksPath = local-hooks\n",
        )
        .unwrap();
        assert_eq!(
            run(&fixture, RepositoryHookAction::Status, &environment).hooks_path,
            Some(canonical(fixture.project.join("local-hooks")))
        );
        fs::write(
            fixture.git_dir.join("config"),
            b"[extensions]\n\tworktreeConfig = true\n[core]\n\thooksPath = local-hooks\n",
        )
        .unwrap();
        fs::write(
            fixture.git_dir.join("config.worktree"),
            b"[core]\n\thooksPath = worktree-hooks\n",
        )
        .unwrap();
        assert_eq!(
            run(&fixture, RepositoryHookAction::Status, &environment).hooks_path,
            Some(canonical(fixture.project.join("worktree-hooks")))
        );
        environment.set("GIT_CONFIG_COUNT", "1");
        environment.set("GIT_CONFIG_KEY_0", "core.hooksPath");
        environment.set("GIT_CONFIG_VALUE_0", "environment-hooks");
        assert_eq!(
            run(&fixture, RepositoryHookAction::Status, &environment).hooks_path,
            Some(canonical(fixture.project.join("environment-hooks")))
        );
    }

    #[cfg(unix)]
    #[test]
    fn global_system_and_local_config_fifos_are_opened_nonblocking() {
        use std::os::unix::ffi::OsStrExt as _;
        use std::time::{Duration, Instant};

        let fixture = repository_fixture();
        let lower = fixture.project.join("lower-config-fifo");
        let lower_name = std::ffi::CString::new(lower.as_os_str().as_bytes()).unwrap();
        // SAFETY: the fixture path is NUL-terminated and points inside the temporary repository.
        assert_eq!(unsafe { libc::mkfifo(lower_name.as_ptr(), 0o600) }, 0);
        let started = Instant::now();
        let error = read_bounded_optional_config(&lower).unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "global and system config capture must not wait for a FIFO writer"
        );
        assert!(error.contains("not a regular file"), "{error}");

        fs::remove_file(fixture.git_dir.join("config")).unwrap();
        let local = fixture.git_dir.join("config");
        let local_name = std::ffi::CString::new(local.as_os_str().as_bytes()).unwrap();
        // SAFETY: the fixture path is NUL-terminated and points inside the temporary repository.
        assert_eq!(unsafe { libc::mkfifo(local_name.as_ptr(), 0o600) }, 0);
        let started = Instant::now();
        let error = match MetadataRoots::resolve(&fixture.project) {
            Ok(_) => panic!("a local config FIFO is not a regular config file"),
            Err(error) => error,
        };
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "repository-local config capture must not wait for a FIFO writer"
        );
        assert!(
            error.to_string().contains("not a regular file"),
            "{error:#}"
        );
    }

    #[test]
    fn installation_config_precedes_system_and_nosystem_suppresses_both() {
        let fixture = repository_fixture();
        let installation = fixture.project.join("installation.gitconfig");
        let system = fixture.project.join("system.gitconfig");
        fs::write(&installation, b"[core]\n\thooksPath = installation-hooks\n").unwrap();
        fs::write(&system, b"[core]\n\thooksPath = system-hooks\n").unwrap();

        let paths = ordered_lower_config_paths(Some(installation.clone()), Some(system.clone()));
        assert_eq!(
            paths,
            vec![
                (installation, gix::config::Source::GitInstallation),
                (system, gix::config::Source::System),
            ]
        );
        let mut capture = HookConfigCapture::default();
        let mut hooks_path = None;
        let mut worktree_config_enabled = false;
        for (path, source) in paths {
            apply_captured_config(
                path,
                source,
                false,
                &mut capture,
                &mut hooks_path,
                &mut worktree_config_enabled,
                &fixture.project,
            )
            .unwrap();
        }
        assert_eq!(hooks_path.as_deref(), Some(b"system-hooks".as_slice()));

        let mut environment = HookConfigEnvironment::empty(&fixture.project);
        environment.set("GIT_CONFIG_NOSYSTEM", "true");
        environment.set(
            "GIT_CONFIG_SYSTEM",
            fixture.project.join("ignored.gitconfig"),
        );
        assert!(system_config_paths(&environment).unwrap().is_empty());
    }

    #[test]
    fn explicit_xdg_config_works_without_a_home_directory() {
        let fixture = repository_fixture();
        let mut environment = isolated_environment(&fixture);
        environment.remove("HOME");
        let xdg = fixture.project.join("xdg-only");
        fs::create_dir_all(xdg.join("git")).unwrap();
        fs::create_dir(fixture.project.join("xdg-hooks")).unwrap();
        fs::write(xdg.join("git/config"), b"[core]\n\thooksPath = xdg-hooks\n").unwrap();
        environment.set("XDG_CONFIG_HOME", xdg.into_os_string());

        let report = run(&fixture, RepositoryHookAction::Status, &environment);
        assert_eq!(
            report.hooks_path,
            Some(canonical(fixture.project.join("xdg-hooks")))
        );
    }

    #[test]
    fn global_worktree_extension_does_not_activate_worktree_config() {
        let fixture = repository_fixture();
        let mut environment = isolated_environment(&fixture);
        let global = fixture.project.join("global.gitconfig");
        environment.set("GIT_CONFIG_GLOBAL", global.clone().into_os_string());
        fs::create_dir(fixture.project.join("global-hooks")).unwrap();
        fs::create_dir(fixture.project.join("wrong-worktree-hooks")).unwrap();
        fs::write(
            &global,
            b"[extensions]\n\tworktreeConfig = true\n[core]\n\thooksPath = global-hooks\n",
        )
        .unwrap();
        fs::write(
            fixture.git_dir.join("config.worktree"),
            b"[core]\n\thooksPath = wrong-worktree-hooks\n",
        )
        .unwrap();

        let report = run(&fixture, RepositoryHookAction::Status, &environment);
        assert_eq!(
            report.hooks_path,
            Some(canonical(fixture.project.join("global-hooks")))
        );
    }

    #[test]
    fn include_external_traversal_null_and_unsupported_expansions_refuse() {
        #[cfg(not(unix))]
        let cases = vec![
            ("../outside-hooks", "hooks_path_traversal"),
            ("~another-user/hooks", "hooks_config_unresolved"),
            ("%(prefix)/hooks", "hooks_config_unresolved"),
        ];
        #[cfg(unix)]
        let cases = vec![
            ("../outside-hooks", "hooks_path_traversal"),
            ("~another-user/hooks", "hooks_config_unresolved"),
            ("%(prefix)/hooks", "hooks_config_unresolved"),
            ("/dev/null", "hooks_path_disabled"),
        ];
        for (value, expected) in cases {
            let fixture = repository_fixture();
            let environment = isolated_environment(&fixture);
            fs::write(
                fixture.git_dir.join("config"),
                format!("[core]\n\thooksPath = {value}\n"),
            )
            .unwrap();
            let report = run(&fixture, RepositoryHookAction::Status, &environment);
            assert_eq!(report.status, expected, "{value}: {report:?}");
        }

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        fs::write(
            fixture.git_dir.join("config"),
            b"[include]\n\tpath = ../outside.gitconfig\n",
        )
        .unwrap();
        let report = run(&fixture, RepositoryHookAction::Status, &environment);
        assert_eq!(report.status, "hooks_config_unresolved", "{report:?}");

        let fixture = repository_fixture();
        let mut environment = isolated_environment(&fixture);
        let outside = fixture.project.parent().unwrap().join("external-hooks");
        fs::create_dir(&outside).unwrap();
        let global = fixture.project.join("global.gitconfig");
        let outside_config_value = outside.to_string_lossy().replace('\\', "/");
        fs::write(
            &global,
            format!("[core]\n\thooksPath = \"{outside_config_value}\"\n"),
        )
        .unwrap();
        environment.set("GIT_CONFIG_GLOBAL", global.into_os_string());
        let report = run(&fixture, RepositoryHookAction::Status, &environment);
        assert_eq!(report.status, "hooks_path_external", "{report:?}");
    }

    #[test]
    fn linked_worktree_and_submodule_defaults_use_their_common_git_roots() {
        let temp = tempdir().unwrap();
        let common = temp.path().join("main/.git");
        let worktree_git = common.join("worktrees/linked");
        let worktree = temp.path().join("linked");
        fs::create_dir_all(common.join("hooks")).unwrap();
        fs::create_dir_all(&worktree_git).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        fs::write(common.join("config"), b"").unwrap();
        fs::write(worktree_git.join("commondir"), b"../..\n").unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )
        .unwrap();
        let fixture = RepositoryFixture {
            _temp: temp,
            project: worktree,
            git_dir: worktree_git,
            hooks: common.join("hooks"),
        };
        let environment = isolated_environment(&fixture);
        let report = run(&fixture, RepositoryHookAction::Status, &environment);
        assert_eq!(
            report.hooks_path,
            Some(canonical(&fixture.hooks)),
            "{report:?}"
        );

        let temp = tempdir().unwrap();
        let project = temp.path().join("parent/submodule");
        let module_git = temp.path().join("parent/.git/modules/submodule");
        fs::create_dir_all(module_git.join("hooks")).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(module_git.join("config"), b"").unwrap();
        fs::write(
            project.join(".git"),
            format!("gitdir: {}\n", module_git.display()),
        )
        .unwrap();
        let fixture = RepositoryFixture {
            _temp: temp,
            project,
            git_dir: module_git.clone(),
            hooks: module_git.join("hooks"),
        };
        let environment = isolated_environment(&fixture);
        let report = run(&fixture, RepositoryHookAction::Status, &environment);
        assert_eq!(
            report.hooks_path,
            Some(canonical(&fixture.hooks)),
            "{report:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn managed_block_runs_before_foreign_early_exit_and_is_fail_open() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::process::Command;

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let hook = fixture.hooks.join("post-merge");
        write_executable(&hook, b"#!/bin/sh\nexit 0\n");
        let marker = fixture.project.join("hook-ran");
        let wrapper = fixture.project.join("mark wrapper ' quoted.sh");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\ntouch {}\n",
                shell_quote(marker.to_str().expect("UTF-8 marker path"))
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let mut hook_request = request(&fixture, RepositoryHookAction::Install);
        hook_request.node_path = PathBuf::from("/bin/sh");
        hook_request.script_path = wrapper;
        let report = manage_repository_hooks_inner(&hook_request, &environment)
            .unwrap_or_else(HookFailure::report);
        assert_eq!(report.status, "installed", "{report:?}");

        let status = Command::new(&hook)
            .status()
            .expect("execute installed hook");
        assert!(status.success());
        assert!(
            marker.is_file(),
            "managed block must run before the foreign exit"
        );

        fs::remove_file(&marker).unwrap();
        fs::remove_file(&hook_request.script_path).unwrap();
        let status = Command::new(&hook)
            .status()
            .expect("execute fail-open hook");
        assert!(
            status.success(),
            "a missing marker command must not break Git"
        );
        assert!(!marker.exists());

        let removed = manage_repository_hooks_inner(
            &RepositoryHookRequest {
                action: RepositoryHookAction::Uninstall,
                ..hook_request.clone()
            },
            &environment,
        )
        .unwrap_or_else(HookFailure::report);
        assert_eq!(removed.status, "foreign_hook_present", "{removed:?}");
        assert_eq!(fs::read(&hook).unwrap(), b"#!/bin/sh\nexit 0\n");
    }

    #[cfg(unix)]
    #[test]
    fn unix_shell_quoting_preserves_backslashes_and_escapes_apostrophes() {
        assert_eq!(shell_quote(r"/tmp/a\b'c"), r"'/tmp/a\b'\''c'");
    }

    #[test]
    fn production_hook_resolver_does_not_spawn_git() {
        let source = include_str!("repository_hooks.rs");
        let production = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("production source prefix");
        let forbidden = ["Command::new(", "\"git\""].concat();
        assert!(
            !production.contains(&forbidden),
            "production hook resolution must not invoke Git"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_system_config_discovery_matches_the_selected_developer_root() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let temp = tempdir().unwrap();
        let temp_root = canonical(temp.path());
        let developer = temp_root.join("Developer");
        fs::create_dir_all(developer.join("usr/share/git-core")).unwrap();
        fs::create_dir_all(developer.join("usr/bin")).unwrap();
        let direct_git = developer.join("usr/bin/git");
        fs::write(&direct_git, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
        fs::set_permissions(&direct_git, fs::Permissions::from_mode(0o755)).unwrap();
        let mut environment = HookConfigEnvironment::empty(&temp_root);
        environment.set("DEVELOPER_DIR", developer.clone().into_os_string());
        assert_eq!(
            apple_developer_system_config(&environment).unwrap(),
            developer.join("usr/share/git-core/gitconfig")
        );
        environment.set("PATH", "/usr/bin");
        assert_eq!(
            default_lower_config_paths(&environment).unwrap(),
            (
                Some(developer.join("usr/share/git-core/gitconfig")),
                Some(PathBuf::from("/etc/gitconfig")),
            )
        );

        environment.remove("DEVELOPER_DIR");
        environment.set("PATH", developer.join("usr/bin").into_os_string());
        assert_eq!(
            default_lower_config_paths(&environment).unwrap(),
            (
                Some(developer.join("usr/share/git-core/gitconfig")),
                Some(PathBuf::from("/etc/gitconfig")),
            ),
            "a directly selected CLT or Xcode Git reads the developer-root installation config"
        );

        let alias_bin = temp_root.join("git-alias/bin");
        fs::create_dir_all(&alias_bin).unwrap();
        symlink(&direct_git, alias_bin.join("git")).unwrap();
        environment.set("PATH", alias_bin.into_os_string());
        let error = default_lower_config_paths(&environment).unwrap_err();
        assert!(error.contains("symlinked or aliased path"), "{error}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_locked_source_binding_blocks_final_boundary_swap() {
        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let installed = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(installed.status, "installed", "{installed:?}");
        let hook = fixture.hooks.join("post-checkout");
        let moved = fixture.hooks.join("post-checkout-before-concurrent-edit");
        let concurrent = b"#!/bin/sh\necho concurrent-delete-edit\n".to_vec();
        BEFORE_HOOK_TARGET_WRITE_OPEN_HOOK.with(|slot| {
            let concurrent = concurrent.clone();
            let hook = hook.clone();
            let moved = moved.clone();
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::rename(&hook, &moved)
                    .expect_err("the locked source must deny a concurrent rename");
                write_executable(&hook, &concurrent);
            }));
        });

        let refused = run(&fixture, RepositoryHookAction::Uninstall, &environment);

        assert_eq!(refused.status, "hook_mutation_failed", "{refused:?}");
        assert_eq!(fs::read(&hook).unwrap(), concurrent);
        assert!(!moved.exists(), "the locked preflight object never moved");
        assert!(transaction_artifacts(&fixture.hooks).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn windows_parent_relative_binding_detects_name_replacement() {
        let fixture = repository_fixture();
        let parent = open_root_no_follow(&fixture.hooks).unwrap();
        let reserved = create_private_file(&parent, "binding-proof", b"reserved", None).unwrap();
        let backup = create_private_file(&parent, "binding-backup", b"placeholder", None).unwrap();
        validate_named_file_binding(&parent, "binding-proof", &reserved).unwrap();

        rename_open_file_windows(&parent, &reserved, "binding-backup", true).unwrap();
        validate_named_file_binding(&parent, "binding-backup", &reserved).unwrap();
        assert!(validate_named_file_binding(&parent, "binding-backup", &backup).is_err());

        fs::rename(
            fixture.hooks.join("binding-backup"),
            fixture.hooks.join("binding-proof-moved"),
        )
        .unwrap();
        fs::write(fixture.hooks.join("binding-backup"), b"replacement").unwrap();

        assert!(validate_named_file_binding(&parent, "binding-backup", &reserved).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_junction_hooks_directory_is_refused_without_touching_outside() {
        use std::process::Command;

        let fixture = repository_fixture();
        let environment = isolated_environment(&fixture);
        let configured = fixture.project.join("junction-hooks");
        let outside = fixture
            .project
            .parent()
            .unwrap()
            .join("outside-junction-hooks");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"outside").unwrap();
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&configured)
            .arg(&outside)
            .status()
            .unwrap();
        assert!(status.success());
        fs::write(
            fixture.git_dir.join("config"),
            b"[core]\n\thooksPath = junction-hooks\n",
        )
        .unwrap();
        let report = run(&fixture, RepositoryHookAction::Install, &environment);
        assert_eq!(report.status, "hooks_path_unproven", "{report:?}");
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"outside");
    }
}
