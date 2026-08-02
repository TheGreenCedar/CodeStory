//! Non-executing reads of repository metadata owned by the workspace boundary.
//!
//! Repository contents and repository metadata are both untrusted input. This
//! reader uses `gix` with isolated configuration permissions, refuses metadata
//! redirects outside the resolved repository roots, never inspects nested
//! submodule worktrees, and reports a conservative observation on failure.

use anyhow::{Context, Result, anyhow, bail};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static OBSERVATION_LIMIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static OBSERVATION_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static METADATA_TREE_TRAVERSAL_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    static AFTER_BOUNDARY_VALIDATION_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_GIX_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static BEFORE_LOCAL_CONFIG_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_BOUNDED_IDENTITY_CAPTURE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn run_after_boundary_validation_hook() {
    AFTER_BOUNDARY_VALIDATION_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn run_before_gix_open_hook() {
    BEFORE_GIX_OPEN_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn run_before_local_config_open_hook() {
    BEFORE_LOCAL_CONFIG_OPEN_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn run_after_bounded_identity_capture_hook() {
    AFTER_BOUNDED_IDENTITY_CAPTURE_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
pub(crate) fn set_after_bounded_identity_capture_hook_for_test(hook: impl FnOnce() + 'static) {
    AFTER_BOUNDED_IDENTITY_CAPTURE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

const MAX_GITDIR_POINTER_BYTES: u64 = 16 * 1024;
const MAX_COMMONDIR_POINTER_BYTES: u64 = 16 * 1024;
const MAX_ALTERNATES_BYTES: u64 = 1024 * 1024;
const MAX_LOCAL_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_METADATA_TREE_ENTRIES: usize = 100_000;
const MAX_REWRITE_BLOB_BYTES: u64 = codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP;
const MAX_REWRITE_SIMILARITY_PERMUTATIONS: usize = 1_000;

/// One problem that prevented a complete repository metadata observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryMetadataIssue {
    pub code: String,
    pub path: PathBuf,
    pub message: String,
}

impl RepositoryMetadataIssue {
    fn new(code: &str, path: &Path, error: impl std::fmt::Display) -> Self {
        Self {
            code: code.to_string(),
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}

/// A fail-closed snapshot of the Git metadata needed by CodeStory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryMetadata {
    pub remote_url: Option<String>,
    pub head_tree: Option<String>,
    pub dirty: bool,
    pub tracked_paths: Vec<Vec<u8>>,
    pub issues: Vec<RepositoryMetadataIssue>,
}

impl RepositoryMetadata {
    fn unavailable(root: &Path, code: &str, error: impl std::fmt::Display) -> Self {
        Self {
            remote_url: None,
            head_tree: None,
            dirty: true,
            tracked_paths: Vec::new(),
            issues: vec![RepositoryMetadataIssue::new(code, root, error)],
        }
    }
}

/// Which Git comparison supplies paths for an affected analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryChangeScope {
    Head,
    Staged,
    Unstaged,
    Untracked,
}

/// Portable change kinds exposed without leaking the `gix` API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepositoryChangeKind {
    Added,
    Copied,
    Deleted,
    Modified,
    Renamed,
    TypeChanged,
    Unmerged,
    Untracked,
}

/// One repository-relative changed path, preserved as Git path bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RepositoryChange {
    pub path: Vec<u8>,
    pub previous_path: Option<Vec<u8>>,
    pub kind: RepositoryChangeKind,
}

/// Read repository identity, dirtiness, and tracked paths without executing Git.
///
/// Failures remain visible in `issues`; `dirty` is `true` whenever cleanliness
/// could not be proven.
pub fn read_repository_metadata(project_root: &Path) -> RepositoryMetadata {
    #[cfg(any(test, feature = "test-support"))]
    record_repository_metadata_observation();
    let reader = match RepositoryReader::open(project_root) {
        Ok(reader) => reader,
        Err(error) => {
            return RepositoryMetadata::unavailable(project_root, "repository_open_failed", error);
        }
    };
    reader.metadata()
}

/// One bounded repeated-admission observation. The repository instance uses
/// only native identities for the resolved gitdir and common-dir directories.
pub(crate) enum BoundedRepositoryIdentityObservation {
    Absent,
    Present {
        remote_url: Option<String>,
        git_dir_identity: crate::repository_identity::WorkspacePathIdentity,
        common_dir_identity: crate::repository_identity::WorkspacePathIdentity,
    },
}

/// Read only `.git`/`commondir`, local config bytes, and the native identities
/// of the two resolved metadata roots. Includes are never followed. This path
/// deliberately does not open a gix repository or inspect HEAD, the index,
/// status, refs, objects, alternates, tracked paths, or nested metadata trees.
pub(crate) fn observe_bounded_repository_identity(
    project_root: &Path,
) -> Result<BoundedRepositoryIdentityObservation> {
    let root = match fs::canonicalize(project_root) {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            #[cfg(test)]
            run_after_bounded_identity_capture_hook();
            return match fs::symlink_metadata(project_root) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    Ok(BoundedRepositoryIdentityObservation::Absent)
                }
                _ => bail!("project root changed during bounded identity observation"),
            };
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("canonicalize project root {}", project_root.display()));
        }
    };
    let dot_git = root.join(".git");
    match fs::symlink_metadata(&dot_git) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            #[cfg(test)]
            run_after_bounded_identity_capture_hook();
            return match fs::symlink_metadata(&dot_git) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Ok(BoundedRepositoryIdentityObservation::Absent)
                }
                _ => bail!("repository .git entry changed during bounded identity observation"),
            };
        }
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {}", dot_git.display()));
        }
    }

    let captured = MetadataRoots::resolve(&root)?;
    captured.validate_configured_worktree()?;
    let remote_url = captured.remote_url()?;
    let git_dir_identity = crate::repository_identity::workspace_path_identity(&captured.git_dir)
        .context("observe repository gitdir native identity")?;
    let common_dir_identity =
        crate::repository_identity::workspace_path_identity(&captured.common_dir)
            .context("observe repository common-dir native identity")?;

    #[cfg(test)]
    run_after_bounded_identity_capture_hook();

    let current = MetadataRoots::resolve(&root)
        .map_err(|_| anyhow!("repository metadata changed during bounded identity observation"))?;
    current
        .validate_configured_worktree()
        .map_err(|_| anyhow!("repository metadata changed during bounded identity observation"))?;
    if current != captured {
        bail!("repository pointer or local config changed during bounded identity observation");
    }
    let current_git_dir_identity =
        crate::repository_identity::workspace_path_identity(&current.git_dir)
            .context("revalidate repository gitdir native identity")?;
    let current_common_dir_identity =
        crate::repository_identity::workspace_path_identity(&current.common_dir)
            .context("revalidate repository common-dir native identity")?;
    if current_git_dir_identity != git_dir_identity
        || current_common_dir_identity != common_dir_identity
    {
        bail!("repository metadata root identity changed during bounded identity observation");
    }

    Ok(BoundedRepositoryIdentityObservation::Present {
        remote_url,
        git_dir_identity,
        common_dir_identity,
    })
}

#[cfg(any(test, feature = "test-support"))]
fn record_repository_metadata_observation() {
    OBSERVATION_LIMIT.with(|limit| {
        let Some(limit) = limit.get() else {
            return;
        };
        OBSERVATION_COUNT.with(|count| {
            let observed = count.get();
            assert!(
                observed < limit,
                "repository metadata observed more than the test limit of {limit}"
            );
            count.set(observed + 1);
        });
    });
}

/// Run test support with a thread-local ceiling on repository observations.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn with_repository_metadata_observation_limit_for_test<T>(
    limit: usize,
    task: impl FnOnce() -> T,
) -> T {
    assert!(limit > 0, "repository observation limit must be positive");
    struct Reset {
        previous_limit: Option<usize>,
        previous_count: usize,
    }
    impl Drop for Reset {
        fn drop(&mut self) {
            OBSERVATION_LIMIT.with(|limit| limit.set(self.previous_limit));
            OBSERVATION_COUNT.with(|count| count.set(self.previous_count));
        }
    }

    let previous_limit = OBSERVATION_LIMIT.with(|cell| cell.replace(Some(limit)));
    let previous_count = OBSERVATION_COUNT.with(|cell| cell.replace(0));
    let _reset = Reset {
        previous_limit,
        previous_count,
    };
    task()
}

#[cfg(any(test, feature = "test-support"))]
fn record_repository_metadata_tree_traversal() {
    METADATA_TREE_TRAVERSAL_COUNT.with(|count| count.set(count.get().saturating_add(1)));
}

/// Run test support with an isolated counter for metadata-directory walks.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn with_repository_metadata_tree_traversal_count_for_test<T>(
    task: impl FnOnce() -> T,
) -> (T, usize) {
    struct Reset(usize);
    impl Drop for Reset {
        fn drop(&mut self) {
            METADATA_TREE_TRAVERSAL_COUNT.with(|count| count.set(self.0));
        }
    }

    let previous = METADATA_TREE_TRAVERSAL_COUNT.with(|count| count.replace(0));
    let reset = Reset(previous);
    let result = task();
    let observed = METADATA_TREE_TRAVERSAL_COUNT.with(std::cell::Cell::get);
    drop(reset);
    (result, observed)
}

/// Read changed paths without invoking the Git executable.
pub fn read_repository_changes(
    project_root: &Path,
    scope: RepositoryChangeScope,
) -> Result<Vec<RepositoryChange>> {
    RepositoryReader::open(project_root)?.changes(scope)
}

struct RepositoryReader {
    root: PathBuf,
    repository: gix::Repository,
    metadata_roots: MetadataRoots,
    metadata_boundary: MetadataBoundarySnapshot,
}

impl RepositoryReader {
    fn open(project_root: &Path) -> Result<Self> {
        let roots = MetadataRoots::resolve(project_root)?;
        let metadata_boundary = roots.validate_redirects()?;

        #[cfg(test)]
        run_after_boundary_validation_hook();

        let pre_open_boundary = roots
            .validate_redirects()
            .map_err(|_| anyhow!("repository metadata changed before confined gix open"))?;
        if pre_open_boundary != metadata_boundary {
            bail!("repository metadata changed before confined gix open");
        }
        roots.validate_configured_worktree()?;
        let config_overrides = [
            format!("core.bigFileThreshold={MAX_REWRITE_BLOB_BYTES}").into(),
            core_worktree_override(&roots.root)?,
        ];

        #[cfg(test)]
        run_before_gix_open_hook();

        let repository = gix::open_opts(
            &roots.git_dir,
            gix::open::Options::isolated()
                .strict_config(true)
                .open_path_as_is(true)
                .config_overrides(config_overrides),
        );
        let post_open_boundary = roots
            .validate_redirects()
            .map_err(|_| anyhow!("repository metadata changed during confined repository open"))?;
        if post_open_boundary != metadata_boundary {
            bail!("repository metadata changed during confined repository open");
        }
        let repository = repository
            .with_context(|| format!("open repository metadata at {}", roots.root.display()))?;

        let opened_git_dir = canonical_existing(repository.git_dir())
            .context("canonicalize gix repository gitdir")?;
        if opened_git_dir != roots.git_dir {
            bail!(
                "repository gitdir changed during open: expected {}, found {}",
                roots.git_dir.display(),
                opened_git_dir.display()
            );
        }
        let opened_common_dir = canonical_existing(repository.common_dir())
            .context("canonicalize gix repository common dir")?;
        if opened_common_dir != roots.common_dir {
            bail!(
                "repository common dir changed during open: expected {}, found {}",
                roots.common_dir.display(),
                opened_common_dir.display()
            );
        }
        let opened_work_dir = repository
            .workdir()
            .context("bare repositories are not project workspaces")?;
        if opened_work_dir != roots.root {
            bail!(
                "repository worktree changed during open: expected {}, found {}",
                roots.root.display(),
                opened_work_dir.display()
            );
        }

        Ok(Self {
            root: roots.root.clone(),
            repository,
            metadata_roots: roots,
            metadata_boundary,
        })
    }

    fn metadata(&self) -> RepositoryMetadata {
        let mut issues = Vec::new();
        let remote_url = self
            .repository
            .config_snapshot()
            .string("remote.origin.url")
            .map(|value| String::from_utf8_lossy(value.as_ref()).trim().to_string())
            .filter(|value| !value.is_empty());

        let head_tree = match self.repository.head_tree_id() {
            Ok(id) => Some(id.to_string()),
            Err(error) => {
                issues.push(RepositoryMetadataIssue::new(
                    "head_tree_unavailable",
                    &self.root,
                    error,
                ));
                None
            }
        };

        let tracked_paths = match self.repository.index_or_load_from_head_or_empty() {
            Ok(index) => {
                let mut paths = index
                    .entries()
                    .iter()
                    .map(|entry| entry.path(&index).to_vec())
                    .collect::<Vec<_>>();
                paths.sort();
                paths.dedup();
                paths
            }
            Err(error) => {
                issues.push(RepositoryMetadataIssue::new(
                    "index_unavailable",
                    &self.root,
                    error,
                ));
                Vec::new()
            }
        };

        let dirty = match self.status_items(false) {
            Ok(items) => !items.is_empty(),
            Err(error) => {
                issues.push(RepositoryMetadataIssue::new(
                    "worktree_status_unavailable",
                    &self.root,
                    error,
                ));
                true
            }
        };

        if let Err(error) = self.ensure_metadata_boundary_unchanged() {
            return RepositoryMetadata::unavailable(
                &self.root,
                "repository_metadata_changed",
                error,
            );
        }

        RepositoryMetadata {
            remote_url,
            head_tree,
            dirty,
            tracked_paths,
            issues,
        }
    }

    fn changes(&self, scope: RepositoryChangeScope) -> Result<Vec<RepositoryChange>> {
        let items = self.status_items(matches!(
            scope,
            RepositoryChangeScope::Head | RepositoryChangeScope::Staged
        ))?;
        let mut changes = items
            .into_iter()
            .filter_map(|item| change_from_status_item(item, scope))
            .collect::<Vec<_>>();
        changes.sort();
        changes.dedup();
        self.ensure_metadata_boundary_unchanged()?;
        Ok(changes)
    }

    fn ensure_metadata_boundary_unchanged(&self) -> Result<()> {
        let current = self
            .metadata_roots
            .validate_redirects()
            .map_err(|_| anyhow!("repository metadata changed during confined repository read"))?;
        if current != self.metadata_boundary {
            bail!("repository metadata changed during confined repository read");
        }
        Ok(())
    }

    fn status_items(&self, track_tree_index_rewrites: bool) -> Result<Vec<gix::status::Item>> {
        if self.has_external_worktree_filters() {
            bail!(
                "repository config declares an external clean or process filter; worktree status was not inspected"
            );
        }
        let index = self
            .repository
            .index_or_load_from_head_or_empty()
            .context("load repository index before status")?;
        if index
            .entries()
            .iter()
            .any(|entry| entry.mode == gix::index::entry::Mode::COMMIT)
        {
            bail!(
                "repository index contains a submodule; nested worktree status was not inspected"
            );
        }
        let platform = self
            .repository
            .status(gix::progress::Discard)
            .context("prepare repository status")?
            .untracked_files(gix::status::UntrackedFiles::Files)
            .index_worktree_submodules(None);
        let platform = if track_tree_index_rewrites {
            platform.tree_index_track_renames(gix::status::tree_index::TrackRenames::Given(
                bounded_tree_index_rewrites(),
            ))
        } else {
            platform.tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
        };
        let mut iterator = platform
            .into_iter(Vec::<gix::bstr::BString>::new())
            .context("start repository status")?;
        let mut items = Vec::new();
        for item in &mut iterator {
            let item = item.context("read repository status item")?;
            if matches!(&item, gix::status::Item::TreeIndex(_))
                || matches!(
                    &item,
                    gix::status::Item::IndexWorktree(change) if change.summary().is_some()
                )
            {
                items.push(item);
            }
        }
        Ok(items)
    }

    fn has_external_worktree_filters(&self) -> bool {
        self.repository
            .config_snapshot()
            .plumbing()
            .sections_by_name("filter")
            .into_iter()
            .flatten()
            .any(|section| section.value("clean").is_some() || section.value("process").is_some())
    }
}

fn bounded_tree_index_rewrites() -> gix::diff::Rewrites {
    gix::diff::Rewrites {
        copies: None,
        percentage: Some(0.5),
        limit: MAX_REWRITE_SIMILARITY_PERMUTATIONS,
        track_empty: false,
    }
}

fn change_from_status_item(
    item: gix::status::Item,
    scope: RepositoryChangeScope,
) -> Option<RepositoryChange> {
    match item {
        gix::status::Item::TreeIndex(change)
            if matches!(
                scope,
                RepositoryChangeScope::Head | RepositoryChangeScope::Staged
            ) =>
        {
            Some(change_from_tree_index(change))
        }
        gix::status::Item::IndexWorktree(change)
            if matches!(
                scope,
                RepositoryChangeScope::Head
                    | RepositoryChangeScope::Unstaged
                    | RepositoryChangeScope::Untracked
            ) =>
        {
            let is_untracked = matches!(
                change,
                gix::status::index_worktree::Item::DirectoryContents { .. }
            );
            let include = match scope {
                RepositoryChangeScope::Untracked => is_untracked,
                RepositoryChangeScope::Head | RepositoryChangeScope::Unstaged => !is_untracked,
                RepositoryChangeScope::Staged => false,
            };
            if !include {
                return None;
            }
            change_from_index_worktree(change)
        }
        _ => None,
    }
}

fn change_from_tree_index(change: gix::diff::index::Change) -> RepositoryChange {
    use gix::diff::index::Change;
    match change {
        Change::Addition { location, .. } => RepositoryChange {
            path: location.into_owned().to_vec(),
            previous_path: None,
            kind: RepositoryChangeKind::Added,
        },
        Change::Deletion { location, .. } => RepositoryChange {
            path: location.into_owned().to_vec(),
            previous_path: None,
            kind: RepositoryChangeKind::Deleted,
        },
        Change::Modification { location, .. } => RepositoryChange {
            path: location.into_owned().to_vec(),
            previous_path: None,
            kind: RepositoryChangeKind::Modified,
        },
        Change::Rewrite {
            source_location,
            location,
            copy,
            ..
        } => RepositoryChange {
            path: location.into_owned().to_vec(),
            previous_path: Some(source_location.into_owned().to_vec()),
            kind: if copy {
                RepositoryChangeKind::Copied
            } else {
                RepositoryChangeKind::Renamed
            },
        },
    }
}

fn change_from_index_worktree(
    change: gix::status::index_worktree::Item,
) -> Option<RepositoryChange> {
    use gix::status::index_worktree::Item;
    let path = change.rela_path().to_vec();
    let (previous_path, kind) = match &change {
        Item::Modification { .. } => {
            let summary = change.summary()?;
            use gix::status::index_worktree::iter::Summary;
            let kind = match summary {
                Summary::Added | Summary::IntentToAdd => RepositoryChangeKind::Added,
                Summary::Removed => RepositoryChangeKind::Deleted,
                Summary::Modified => RepositoryChangeKind::Modified,
                Summary::TypeChange => RepositoryChangeKind::TypeChanged,
                Summary::Conflict => RepositoryChangeKind::Unmerged,
                Summary::Renamed => RepositoryChangeKind::Renamed,
                Summary::Copied => RepositoryChangeKind::Copied,
            };
            (None, kind)
        }
        Item::DirectoryContents { .. } => (None, RepositoryChangeKind::Untracked),
        Item::Rewrite { source, copy, .. } => (
            Some(source.rela_path().to_vec()),
            if *copy {
                RepositoryChangeKind::Copied
            } else {
                RepositoryChangeKind::Renamed
            },
        ),
    };
    Some(RepositoryChange {
        path,
        previous_path,
        kind,
    })
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct MetadataRoots {
    pub(crate) root: PathBuf,
    pub(crate) git_dir: PathBuf,
    pub(crate) common_dir: PathBuf,
    git_dir_pointer: Option<MetadataPointer>,
    common_dir_pointer: Option<MetadataPointer>,
    local_configs: Vec<MetadataConfigFile>,
}

#[derive(Clone, PartialEq, Eq)]
struct MetadataPointer {
    path: PathBuf,
    expected_contents: Vec<u8>,
    max_bytes: u64,
    label: &'static str,
}

impl MetadataPointer {
    fn validate(&self) -> Result<()> {
        let current = read_bounded_metadata_pointer(&self.path, self.max_bytes, self.label)?;
        if current != self.expected_contents {
            bail!("{} changed after validation", self.label);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
struct MetadataConfigFile {
    path: PathBuf,
    expected_contents: Option<Vec<u8>>,
}

impl MetadataConfigFile {
    fn capture(path: PathBuf) -> Result<Self> {
        #[cfg(test)]
        run_before_local_config_open_hook();
        let expected_contents = read_bounded_optional_metadata_file(
            &path,
            MAX_LOCAL_CONFIG_BYTES,
            "repository local config",
        )?;
        Ok(Self {
            path,
            expected_contents,
        })
    }

    fn validate(&self) -> Result<()> {
        let current = read_bounded_optional_metadata_file(
            &self.path,
            MAX_LOCAL_CONFIG_BYTES,
            "repository local config",
        )?;
        if current != self.expected_contents {
            bail!("repository local config changed after validation");
        }
        Ok(())
    }

    fn configured_worktree(&self) -> Result<Option<PathBuf>> {
        let Some(config) = self.parsed()? else {
            return Ok(None);
        };
        config
            .string("core.worktree")
            .map(|value| bytes_to_path(value.as_ref()))
            .transpose()
    }

    fn parsed(&self) -> Result<Option<gix::config::File>> {
        let Some(contents) = self.expected_contents.as_deref() else {
            return Ok(None);
        };
        gix::config::File::from_bytes_no_includes(
            contents,
            gix::config::file::Metadata::default(),
            gix::config::file::init::Options {
                lossy: true,
                ..Default::default()
            },
        )
        .with_context(|| format!("parse repository local config {}", self.path.display()))
        .map(Some)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetadataBoundarySnapshot {
    directories: Vec<MetadataDirectoryFingerprint>,
    alternates: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MetadataDirectoryFingerprint {
    path: PathBuf,
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    change_seconds: i64,
    #[cfg(unix)]
    change_nanoseconds: i64,
    #[cfg(windows)]
    file_attributes: u32,
    #[cfg(windows)]
    creation_time: u64,
    #[cfg(windows)]
    last_write_time: u64,
}

fn configured_origin_url(config: &gix::config::File) -> Option<String> {
    config
        .string("remote.origin.url")
        .map(|value| String::from_utf8_lossy(value.as_ref()).trim().to_string())
        .filter(|value| !value.is_empty())
}

impl MetadataRoots {
    pub(crate) fn resolve(project_root: &Path) -> Result<Self> {
        let root = canonical_existing(project_root)
            .with_context(|| format!("canonicalize project root {}", project_root.display()))?;
        let dot_git = root.join(".git");
        let metadata = fs::symlink_metadata(&dot_git)
            .with_context(|| format!("inspect {}", dot_git.display()))?;
        if metadata.file_type().is_symlink() {
            bail!("repository .git entry must not be a symlink");
        }

        let (git_dir, git_dir_pointer) = if metadata.is_dir() {
            (canonical_existing(&dot_git)?, None)
        } else if metadata.is_file() {
            let pointer_contents = read_bounded_metadata_pointer(
                &dot_git,
                MAX_GITDIR_POINTER_BYTES,
                "repository .git pointer",
            )?;
            let pointer = std::str::from_utf8(&pointer_contents)
                .context("repository .git pointer is not UTF-8")?
                .trim();
            let target = pointer
                .strip_prefix("gitdir:")
                .map(str::trim)
                .filter(|target| !target.is_empty())
                .context("repository .git pointer is malformed")?;
            let target = Path::new(target);
            let target = if target.is_absolute() {
                target.to_path_buf()
            } else {
                root.join(target)
            };
            let git_dir = canonical_existing(&target)
                .with_context(|| format!("resolve repository gitdir {}", target.display()))?;
            (
                git_dir,
                Some(MetadataPointer {
                    path: dot_git.clone(),
                    expected_contents: pointer_contents,
                    max_bytes: MAX_GITDIR_POINTER_BYTES,
                    label: "repository .git pointer",
                }),
            )
        } else {
            bail!("repository .git entry is neither a file nor a directory");
        };

        if metadata.is_dir() && !git_dir.starts_with(&root) {
            bail!("repository metadata directory resolves outside the project root");
        }

        let commondir_path = git_dir.join("commondir");
        let commondir_metadata = match fs::symlink_metadata(&commondir_path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", commondir_path.display()));
            }
        };
        let (common_dir, common_dir_pointer) = if let Some(metadata) = commondir_metadata {
            if metadata.file_type().is_symlink() {
                bail!("repository commondir pointer must not be a symlink");
            }
            if !metadata.is_file() {
                bail!("repository commondir pointer is not a regular file");
            }
            let contents = read_bounded_metadata_pointer(
                &commondir_path,
                MAX_COMMONDIR_POINTER_BYTES,
                "repository commondir pointer",
            )?;
            let value = std::str::from_utf8(&contents)
                .context("repository commondir pointer is not UTF-8")?
                .trim();
            if value.is_empty() {
                bail!("repository commondir pointer is empty");
            }
            let value = Path::new(value);
            let value = if value.is_absolute() {
                value.to_path_buf()
            } else {
                git_dir.join(value)
            };
            let common_dir = canonical_existing(&value)
                .with_context(|| format!("resolve repository commondir {}", value.display()))?;
            (
                common_dir,
                Some(MetadataPointer {
                    path: commondir_path,
                    expected_contents: contents,
                    max_bytes: MAX_COMMONDIR_POINTER_BYTES,
                    label: "repository commondir pointer",
                }),
            )
        } else {
            (git_dir.clone(), None)
        };

        let mut local_config_paths =
            vec![common_dir.join("config"), git_dir.join("config.worktree")];
        local_config_paths.sort();
        local_config_paths.dedup();
        let local_configs = local_config_paths
            .into_iter()
            .map(MetadataConfigFile::capture)
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            root,
            git_dir,
            common_dir,
            git_dir_pointer,
            common_dir_pointer,
            local_configs,
        })
    }

    fn remote_url(&self) -> Result<Option<String>> {
        let common_config = self
            .local_config(&self.common_dir.join("config"))
            .context("captured repository common config is missing")?
            .parsed()?;
        let mut remote_url = common_config.as_ref().and_then(configured_origin_url);
        let worktree_config_enabled = match common_config.as_ref() {
            Some(config) => config
                .boolean("extensions.worktreeConfig")
                .context("parse extensions.worktreeConfig from repository local config")?
                .unwrap_or(false),
            None => false,
        };
        if worktree_config_enabled
            && let Some(worktree_config) = self
                .local_config(&self.git_dir.join("config.worktree"))
                .context("captured repository worktree config is missing")?
                .parsed()?
            && let Some(worktree_remote_url) = configured_origin_url(&worktree_config)
        {
            remote_url = Some(worktree_remote_url);
        }
        Ok(remote_url)
    }

    fn local_config(&self, path: &Path) -> Option<&MetadataConfigFile> {
        self.local_configs.iter().find(|config| config.path == path)
    }

    fn validate_configured_worktree(&self) -> Result<()> {
        for config in &self.local_configs {
            let Some(configured) = config.configured_worktree()? else {
                continue;
            };
            let configured = if configured.is_absolute() {
                configured
            } else {
                self.git_dir.join(configured)
            };
            let configured = normalize_lexical_path(&configured);
            if !lexical_paths_equal(&configured, &self.root) {
                bail!(
                    "repository core.worktree points outside the selected project: {}",
                    configured.display()
                );
            }
        }
        Ok(())
    }

    pub(crate) fn validate_hook_metadata(&self) -> Result<()> {
        if let Some(pointer) = &self.git_dir_pointer {
            pointer.validate()?;
        }
        if let Some(pointer) = &self.common_dir_pointer {
            pointer.validate()?;
        }
        for config in &self.local_configs {
            config.validate()?;
        }
        self.validate_configured_worktree()
    }

    pub(crate) fn captured_local_config(&self, path: &Path) -> Option<Option<&[u8]>> {
        self.local_configs
            .iter()
            .find(|config| config.path == path)
            .map(|config| config.expected_contents.as_deref())
    }

    fn validate_redirects(&self) -> Result<MetadataBoundarySnapshot> {
        if let Some(pointer) = &self.git_dir_pointer {
            pointer.validate()?;
        }
        if let Some(pointer) = &self.common_dir_pointer {
            pointer.validate()?;
        }
        for config in &self.local_configs {
            config.validate()?;
        }
        let allowed = [&self.root, &self.git_dir, &self.common_dir];
        let mut directories = vec![
            metadata_directory_fingerprint(&self.git_dir)?,
            metadata_directory_fingerprint(&self.common_dir)?,
        ];
        for path in [
            self.git_dir.join("HEAD"),
            self.git_dir.join("index"),
            self.git_dir.join("config.worktree"),
            self.common_dir.join("config"),
            self.common_dir.join("packed-refs"),
            self.common_dir.join("refs"),
            self.common_dir.join("objects"),
        ] {
            validate_existing_path(&path, &allowed)?;
        }

        let mut nested_roots = vec![
            self.git_dir.join("refs"),
            self.git_dir.join("objects"),
            self.common_dir.join("refs"),
            self.common_dir.join("objects"),
        ];
        nested_roots.sort();
        nested_roots.dedup();
        let mut visited_entries = 0;
        for root in nested_roots {
            reject_nested_metadata_symlinks(
                &root,
                &mut visited_entries,
                MAX_METADATA_TREE_ENTRIES,
                &mut directories,
            )?;
        }
        reject_shared_index_symlinks(&self.git_dir)?;

        let alternates = self.common_dir.join("objects/info/alternates");
        let Some(contents) = read_bounded_optional_metadata_file(
            &alternates,
            MAX_ALTERNATES_BYTES,
            "repository alternates file",
        )?
        else {
            directories.sort();
            directories.dedup();
            return Ok(MetadataBoundarySnapshot {
                directories,
                alternates: None,
            });
        };
        for line in contents.split(|byte| *byte == b'\n') {
            let line = trim_ascii(line);
            if line.is_empty() {
                continue;
            }
            let path = bytes_to_path(line)?;
            let path = if path.is_absolute() {
                path
            } else {
                self.common_dir.join("objects").join(path)
            };
            let resolved = canonical_existing(&path)
                .with_context(|| format!("resolve repository alternate {}", path.display()))?;
            if !allowed.iter().any(|root| resolved.starts_with(root)) {
                bail!(
                    "repository alternate points outside the selected metadata roots: {}",
                    resolved.display()
                );
            }
        }
        directories.sort();
        directories.dedup();
        Ok(MetadataBoundarySnapshot {
            directories,
            alternates: Some(contents),
        })
    }
}

fn read_bounded_metadata_pointer(path: &Path, max_bytes: u64, label: &str) -> Result<Vec<u8>> {
    let file = open_metadata_file_no_follow(path)
        .with_context(|| format!("open {label} {}", path.display()))?;
    read_bounded_metadata_handle(file, path, max_bytes, label)
}

fn read_bounded_optional_metadata_file(
    path: &Path,
    max_bytes: u64,
    label: &str,
) -> Result<Option<Vec<u8>>> {
    let file = match open_metadata_file_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("open {label} {}", path.display()));
        }
    };
    read_bounded_metadata_handle(file, path, max_bytes, label).map(Some)
}

fn read_bounded_metadata_handle(
    mut file: File,
    path: &Path,
    max_bytes: u64,
    label: &str,
) -> Result<Vec<u8>> {
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect open {label} handle {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("{label} is not a regular file: {}", path.display());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            bail!("{label} is a reparse point: {}", path.display());
        }
    }
    if metadata.len() > max_bytes {
        bail!("{label} exceeds the size limit: {}", path.display());
    }
    let mut contents = Vec::with_capacity(metadata.len().min(max_bytes) as usize);
    (&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut contents)
        .with_context(|| format!("read open {label} handle {}", path.display()))?;
    if contents.len() as u64 > max_bytes {
        bail!("{label} exceeds the size limit: {}", path.display());
    }
    Ok(contents)
}

#[cfg(unix)]
fn open_metadata_file_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(windows)]
fn open_metadata_file_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_metadata_file_no_follow(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

fn core_worktree_override(path: &Path) -> Result<gix::bstr::BString> {
    let mut assignment = b"core.worktree=".to_vec();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assignment.extend_from_slice(path.as_os_str().as_bytes());
    }
    #[cfg(not(unix))]
    {
        assignment.extend_from_slice(
            path.to_str()
                .context("repository worktree path is not UTF-8")?
                .as_bytes(),
        );
    }
    Ok(assignment.into())
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

fn lexical_paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .replace('\\', "/")
            .eq_ignore_ascii_case(&right.to_string_lossy().replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn metadata_directory_fingerprint(path: &Path) -> Result<MetadataDirectoryFingerprint> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect repository metadata directory {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "repository metadata directory changed type: {}",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(MetadataDirectoryFingerprint {
            path: path.to_path_buf(),
            len: metadata.len(),
            modified: metadata.modified().ok(),
            device: metadata.dev(),
            inode: metadata.ino(),
            change_seconds: metadata.ctime(),
            change_nanoseconds: metadata.ctime_nsec(),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(MetadataDirectoryFingerprint {
            path: path.to_path_buf(),
            len: metadata.len(),
            modified: metadata.modified().ok(),
            file_attributes: metadata.file_attributes(),
            creation_time: metadata.creation_time(),
            last_write_time: metadata.last_write_time(),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(MetadataDirectoryFingerprint {
            path: path.to_path_buf(),
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

fn reject_nested_metadata_symlinks(
    root: &Path,
    visited_entries: &mut usize,
    max_entries: usize,
    directories: &mut Vec<MetadataDirectoryFingerprint>,
) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() {
        bail!(
            "repository metadata tree must not be a symlink: {}",
            root.display()
        );
    }
    if !metadata.is_dir() {
        bail!(
            "repository metadata tree is not a directory: {}",
            root.display()
        );
    }
    directories.push(metadata_directory_fingerprint(root)?);

    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        #[cfg(any(test, feature = "test-support"))]
        record_repository_metadata_tree_traversal();
        for entry in fs::read_dir(&directory).with_context(|| {
            format!("read repository metadata directory {}", directory.display())
        })? {
            let entry = entry.with_context(|| {
                format!("read repository metadata entry in {}", directory.display())
            })?;
            *visited_entries = visited_entries.saturating_add(1);
            if *visited_entries > max_entries {
                bail!("repository metadata exceeds the nested-entry limit of {max_entries}");
            }
            let path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("inspect repository metadata path {}", path.display()))?;
            if file_type.is_symlink() {
                bail!("repository metadata contains a symlink: {}", path.display());
            }
            if file_type.is_dir() {
                directories.push(metadata_directory_fingerprint(&path)?);
                pending.push(path);
            }
        }
    }
    Ok(())
}

fn reject_shared_index_symlinks(git_dir: &Path) -> Result<()> {
    #[cfg(any(test, feature = "test-support"))]
    record_repository_metadata_tree_traversal();
    for entry in fs::read_dir(git_dir)
        .with_context(|| format!("read repository gitdir {}", git_dir.display()))?
    {
        let entry = entry
            .with_context(|| format!("read repository gitdir entry in {}", git_dir.display()))?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("sharedindex.")
        {
            continue;
        }
        let path = entry.path();
        if fs::symlink_metadata(&path)
            .with_context(|| format!("inspect shared index {}", path.display()))?
            .file_type()
            .is_symlink()
        {
            bail!(
                "repository shared index must not be a symlink: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn validate_existing_path(path: &Path, allowed: &[&PathBuf]) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() {
        let resolved = canonical_existing(path)
            .with_context(|| format!("resolve repository metadata link {}", path.display()))?;
        if !allowed.iter().any(|root| resolved.starts_with(root)) {
            bail!(
                "repository metadata link points outside the selected roots: {}",
                resolved.display()
            );
        }
    }
    Ok(())
}

fn canonical_existing(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|error| anyhow!(error))
}

#[cfg(unix)]
pub(crate) fn bytes_to_path(bytes: &[u8]) -> Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

#[cfg(windows)]
pub(crate) fn bytes_to_path(bytes: &[u8]) -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::str::from_utf8(bytes).context("repository alternate path is not UTF-8")?,
    ))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn bytes_to_path(bytes: &[u8]) -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::str::from_utf8(bytes).context("repository alternate path is not UTF-8")?,
    ))
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::tempdir;

    #[test]
    fn plain_repository_tracks_identity_cleanliness_and_paths() {
        let Some(project) = git_project() else {
            return;
        };
        let clean = read_repository_metadata(project.path());
        assert_eq!(
            clean.remote_url.as_deref(),
            Some("https://example.com/team/repo.git")
        );
        assert!(clean.head_tree.is_some());
        assert!(!clean.dirty, "issues: {:?}", clean.issues);
        assert_eq!(clean.tracked_paths, vec![b"lib.rs".to_vec()]);

        fs::write(project.path().join("lib.rs"), "pub fn dirty() {}\n").expect("dirty file");
        assert!(read_repository_metadata(project.path()).dirty);
        fs::write(project.path().join("untracked.rs"), "// untracked\n").expect("untracked file");
        assert!(read_repository_metadata(project.path()).dirty);
    }

    #[test]
    fn linked_worktree_and_detached_head_are_read_without_following_submodules() {
        let Some(project) = git_project() else {
            return;
        };
        let parent = tempdir().expect("worktree parent");
        let worktree = parent.path().join("linked");
        git(
            project.path(),
            &[
                "worktree",
                "add",
                "--detach",
                worktree.to_str().expect("UTF-8 worktree"),
                "HEAD",
            ],
        );

        let metadata = read_repository_metadata(&worktree);
        assert!(metadata.issues.is_empty(), "{:?}", metadata.issues);
        assert!(!metadata.dirty);
        assert_eq!(metadata.tracked_paths, vec![b"lib.rs".to_vec()]);
        assert_eq!(
            metadata.head_tree,
            read_repository_metadata(project.path()).head_tree
        );
        assert_eq!(metadata.dirty, git_is_dirty(&worktree));
    }

    #[test]
    fn linked_worktree_pointer_swap_does_not_redirect_the_validated_gix_open() {
        let Some(project) = git_project() else {
            return;
        };
        let Some(alternate) = git_project() else {
            return;
        };
        let parent = tempdir().expect("worktree parent");
        let worktree = parent.path().join("linked");
        git(
            project.path(),
            &[
                "worktree",
                "add",
                "--detach",
                worktree.to_str().expect("UTF-8 worktree"),
                "HEAD",
            ],
        );
        fs::write(
            alternate.path().join(".git/config"),
            b"[this is deliberately malformed\n",
        )
        .expect("poison alternate repository config");
        let pointer = worktree.join(".git");
        let alternate_git_dir = alternate.path().join(".git");
        BEFORE_GIX_OPEN_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::write(
                    &pointer,
                    format!("gitdir: {}\n", alternate_git_dir.display()),
                )
                .expect("swap linked-worktree pointer");
            }));
        });

        let metadata = read_repository_metadata(&worktree);

        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "repository_open_failed");
        assert_eq!(
            metadata.issues[0].message,
            "repository metadata changed during confined repository open"
        );
        assert!(
            !metadata.issues[0].message.contains("malformed"),
            "the alternate repository config must not be read"
        );
    }

    #[test]
    fn linked_worktree_commondir_rewrite_is_rejected_before_gix_open() {
        let Some(project) = git_project() else {
            return;
        };
        let Some(alternate) = git_project() else {
            return;
        };
        let parent = tempdir().expect("worktree parent");
        let worktree = parent.path().join("linked");
        git(
            project.path(),
            &[
                "worktree",
                "add",
                "--detach",
                worktree.to_str().expect("UTF-8 worktree"),
                "HEAD",
            ],
        );
        let pointer = fs::read_to_string(worktree.join(".git")).expect("worktree gitdir pointer");
        let git_dir = PathBuf::from(
            pointer
                .trim()
                .strip_prefix("gitdir:")
                .map(str::trim)
                .expect("gitdir pointer"),
        );
        let commondir = git_dir.join("commondir");
        let alternate_common_dir = alternate.path().join(".git");
        AFTER_BOUNDARY_VALIDATION_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::write(&commondir, format!("{}\n", alternate_common_dir.display()))
                    .expect("rewrite commondir pointer in place");
            }));
        });

        let metadata = read_repository_metadata(&worktree);

        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "repository_open_failed");
        assert_eq!(
            metadata.issues[0].message,
            "repository metadata changed before confined gix open"
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_config_swap_to_external_symlink_fails_at_the_no_follow_open() {
        use std::os::unix::fs::symlink;

        let Some(project) = git_project() else {
            return;
        };
        let outside = tempdir().expect("outside config directory");
        let outside_config = outside.path().join("config");
        fs::write(
            &outside_config,
            "[core]\n\tworktree = /outside/must-not-be-read\n",
        )
        .expect("write outside config");
        let local_config = project.path().join(".git/config");
        BEFORE_LOCAL_CONFIG_OPEN_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::remove_file(&local_config).expect("remove validated local config");
                symlink(&outside_config, &local_config)
                    .expect("replace local config with outside symlink");
            }));
        });
        let open_sentinel = project.path().join("gix-open-ran");
        let sentinel_for_hook = open_sentinel.clone();
        BEFORE_GIX_OPEN_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::write(&sentinel_for_hook, b"gix open reached")
                    .expect("write gix-open sentinel");
            }));
        });

        let metadata = read_repository_metadata(project.path());
        BEFORE_LOCAL_CONFIG_OPEN_HOOK.with(|slot| {
            slot.borrow_mut().take();
        });
        BEFORE_GIX_OPEN_HOOK.with(|slot| {
            slot.borrow_mut().take();
        });

        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "repository_open_failed");
        assert!(
            metadata.issues[0]
                .message
                .starts_with("open repository local config"),
            "unexpected error: {}",
            metadata.issues[0].message
        );
        assert!(
            !open_sentinel.exists(),
            "the swapped symlink must fail at the no-follow handle open before gix"
        );
    }

    #[cfg(unix)]
    #[test]
    fn hostile_core_worktree_is_rejected_before_gix_can_probe_the_outside_path() {
        use std::os::unix::fs::PermissionsExt;

        let Some(project) = git_project() else {
            return;
        };
        let outside = tempdir().expect("outside worktree parent");
        let forbidden = outside.path().join("forbidden");
        fs::create_dir(&forbidden).expect("create forbidden directory");
        fs::set_permissions(&forbidden, fs::Permissions::from_mode(0o000))
            .expect("forbid outside metadata access");
        let outside_worktree = forbidden.join("must-not-be-probed");
        git(
            project.path(),
            &[
                "config",
                "core.worktree",
                outside_worktree.to_str().expect("UTF-8 outside path"),
            ],
        );
        let open_sentinel = project.path().join("gix-open-ran");
        let sentinel_for_hook = open_sentinel.clone();
        BEFORE_GIX_OPEN_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::write(&sentinel_for_hook, b"gix open reached")
                    .expect("write gix-open sentinel");
            }));
        });

        let metadata = read_repository_metadata(project.path());
        BEFORE_GIX_OPEN_HOOK.with(|slot| {
            slot.borrow_mut().take();
        });
        fs::set_permissions(&forbidden, fs::Permissions::from_mode(0o700))
            .expect("restore outside permissions");

        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "repository_open_failed");
        assert!(
            metadata.issues[0]
                .message
                .starts_with("repository core.worktree points outside the selected project:"),
            "unexpected error: {}",
            metadata.issues[0].message
        );
        assert!(
            !open_sentinel.exists(),
            "core.worktree rejection must happen before any gix open can probe the forbidden path"
        );
    }

    #[test]
    fn submodule_checkout_resolves_its_gitdir_without_walking_nested_metadata() {
        let Some(submodule_source) = git_project() else {
            return;
        };
        let Some(parent) = git_project() else {
            return;
        };
        git(
            parent.path(),
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                submodule_source.path().to_str().expect("UTF-8 submodule"),
                "nested",
            ],
        );
        git(parent.path(), &["commit", "-am", "add submodule"]);
        let checkout = parent.path().join("nested");

        let clean = read_repository_metadata(&checkout);
        assert!(clean.issues.is_empty(), "{:?}", clean.issues);
        assert_eq!(clean.dirty, git_is_dirty(&checkout));
        assert!(clean.tracked_paths.contains(&b"lib.rs".to_vec()));

        fs::write(checkout.join("lib.rs"), "pub fn dirty_submodule() {}\n")
            .expect("dirty submodule");
        assert_eq!(
            read_repository_metadata(&checkout).dirty,
            git_is_dirty(&checkout)
        );
    }

    #[cfg(unix)]
    #[test]
    fn pre_epoch_mtimes_match_git_porcelain_without_panicking() {
        use std::time::{Duration, SystemTime};

        let Some(project) = git_project() else {
            return;
        };
        let source = project.path().join("lib.rs");
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .expect("open source timestamp");
        let pre_epoch = SystemTime::UNIX_EPOCH
            .checked_sub(Duration::from_secs(1))
            .expect("pre-epoch timestamp");
        if let Err(error) = file.set_times(fs::FileTimes::new().set_modified(pre_epoch)) {
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidInput | std::io::ErrorKind::Unsupported
            ) {
                return;
            }
            panic!("set pre-epoch timestamp: {error}");
        }

        assert_eq!(
            read_repository_metadata(project.path()).dirty,
            git_is_dirty(project.path())
        );
    }

    #[test]
    fn hostile_local_config_never_executes_fsmonitor() {
        let Some(project) = git_project() else {
            return;
        };
        let sentinel = project.path().join("fsmonitor-ran");
        let hook = project.path().join("hostile.sh");
        fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\n", sentinel.display()),
        )
        .expect("write hostile hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("chmod hook");
        }
        git(
            project.path(),
            &[
                "config",
                "core.fsmonitor",
                hook.to_str().expect("UTF-8 hook"),
            ],
        );

        let metadata = read_repository_metadata(project.path());
        assert!(metadata.issues.is_empty(), "{:?}", metadata.issues);
        assert!(!sentinel.exists());
    }

    #[test]
    fn repository_config_include_never_reads_outside_metadata_roots() {
        let Some(project) = git_project() else {
            return;
        };
        let outside = tempdir().expect("outside config directory");
        let outside_config = outside.path().join("hostile-config");
        fs::write(
            &outside_config,
            "[remote \"origin\"]\n\turl = https://attacker.invalid/outside.git\n\
             [filter \"hostile\"]\n\tclean = outside-command\n",
        )
        .expect("write outside config");
        git(
            project.path(),
            &[
                "config",
                "include.path",
                outside_config.to_str().expect("UTF-8 outside config"),
            ],
        );

        let metadata = read_repository_metadata(project.path());
        assert!(metadata.issues.is_empty(), "{:?}", metadata.issues);
        assert_eq!(
            metadata.remote_url.as_deref(),
            Some("https://example.com/team/repo.git")
        );
        assert!(!metadata.dirty);
    }

    #[cfg(unix)]
    #[test]
    fn external_clean_filter_fails_closed_without_executing() {
        use std::os::unix::fs::PermissionsExt;

        let Some(project) = git_project() else {
            return;
        };
        let sentinel = project.path().join("clean-filter-ran");
        let hook = project.path().join("hostile-clean.sh");
        fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\ncat\n", sentinel.display()),
        )
        .expect("write hostile clean filter");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("chmod hook");
        fs::write(
            project.path().join(".gitattributes"),
            "*.rs filter=hostile\n",
        )
        .expect("write attributes");
        git(
            project.path(),
            &[
                "config",
                "filter.hostile.clean",
                hook.to_str().expect("UTF-8 hook"),
            ],
        );
        git(project.path(), &["add", ".gitattributes"]);
        git(project.path(), &["commit", "-m", "add attributes"]);
        let _ = fs::remove_file(&sentinel);
        fs::write(project.path().join("lib.rs"), "pub fn dirty() {}\n").expect("dirty source");

        git(project.path(), &["add", "lib.rs"]);
        assert!(
            sentinel.exists(),
            "Git should prove the configured clean filter is live"
        );
        fs::remove_file(&sentinel).expect("remove Git sentinel");

        let metadata = read_repository_metadata(project.path());
        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "worktree_status_unavailable");
        assert!(
            !sentinel.exists(),
            "metadata reader executed the clean filter"
        );
    }

    #[test]
    fn parent_repository_with_submodule_status_fails_closed() {
        let Some(submodule_source) = git_project() else {
            return;
        };
        let Some(parent) = git_project() else {
            return;
        };
        git(
            parent.path(),
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                submodule_source.path().to_str().expect("UTF-8 submodule"),
                "nested",
            ],
        );
        git(parent.path(), &["commit", "-am", "add submodule"]);
        assert!(
            !git_is_dirty(parent.path()),
            "Git should prove the parent repository is clean"
        );

        let metadata = read_repository_metadata(parent.path());
        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "worktree_status_unavailable");
        assert!(metadata.issues[0].message.contains("submodule"));
    }

    #[test]
    fn external_alternate_fails_closed_before_object_reads() {
        let Some(project) = git_project() else {
            return;
        };
        let outside = tempdir().expect("outside alternate");
        let alternates = project.path().join(".git/objects/info/alternates");
        fs::write(&alternates, outside.path().as_os_str().as_encoded_bytes())
            .expect("write alternate");

        let metadata = read_repository_metadata(project.path());
        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "repository_open_failed");
    }

    #[cfg(unix)]
    #[test]
    fn commondir_symlink_fails_closed_before_reading_its_target() {
        use std::os::unix::fs::symlink;

        let Some(project) = git_project() else {
            return;
        };
        let outside = tempdir().expect("outside commondir pointer");
        let pointer = outside.path().join("commondir");
        fs::write(&pointer, ".\n").expect("write outside pointer");
        symlink(&pointer, project.path().join(".git/commondir"))
            .expect("link hostile commondir pointer");

        let metadata = read_repository_metadata(project.path());
        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "repository_open_failed");
        assert!(
            metadata.issues[0]
                .message
                .contains("commondir pointer must not be a symlink")
        );
    }

    #[test]
    fn oversized_commondir_pointer_fails_closed() {
        let Some(project) = git_project() else {
            return;
        };
        fs::write(
            project.path().join(".git/commondir"),
            vec![b'a'; (MAX_COMMONDIR_POINTER_BYTES + 1) as usize],
        )
        .expect("write oversized commondir pointer");

        let metadata = read_repository_metadata(project.path());
        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "repository_open_failed");
        assert!(
            metadata.issues[0]
                .message
                .contains("commondir pointer exceeds the size limit")
        );
    }

    #[cfg(unix)]
    #[test]
    fn current_head_ref_symlink_fails_closed_before_gix_reads_outside() {
        use std::os::unix::fs::symlink;

        let Some(project) = git_project() else {
            return;
        };
        let head = fs::read_to_string(project.path().join(".git/HEAD")).expect("read HEAD");
        let reference = head
            .trim()
            .strip_prefix("ref:")
            .map(str::trim)
            .expect("symbolic HEAD");
        let reference_path = project.path().join(".git").join(reference);
        let outside = tempdir().expect("outside ref directory");
        let outside_ref = outside.path().join("head-ref");
        fs::rename(&reference_path, &outside_ref).expect("move ref outside");
        symlink(&outside_ref, &reference_path).expect("link hostile ref");

        let metadata = read_repository_metadata(project.path());
        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "repository_open_failed");
        assert!(
            metadata.issues[0]
                .message
                .contains("metadata contains a symlink")
        );
    }

    #[cfg(unix)]
    #[test]
    fn metadata_mutation_between_validation_and_gix_open_fails_closed() {
        use std::os::unix::fs::symlink;

        let Some(project) = git_project() else {
            return;
        };
        let head = fs::read_to_string(project.path().join(".git/HEAD")).expect("read HEAD");
        let reference = head
            .trim()
            .strip_prefix("ref:")
            .map(str::trim)
            .expect("symbolic HEAD");
        let reference_path = project.path().join(".git").join(reference);
        let outside = tempdir().expect("outside ref directory");
        let outside_ref = outside.path().join("head-ref");
        let reference_for_hook = reference_path.clone();
        let outside_for_hook = outside_ref.clone();
        AFTER_BOUNDARY_VALIDATION_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::rename(&reference_for_hook, &outside_for_hook).expect("move ref outside");
                symlink(&outside_for_hook, &reference_for_hook).expect("link hostile ref");
            }));
        });

        let metadata = read_repository_metadata(project.path());

        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "repository_open_failed");
        assert_eq!(
            metadata.issues[0].message,
            "repository metadata changed before confined gix open"
        );
        assert!(
            !metadata.issues[0]
                .message
                .contains(outside.path().to_string_lossy().as_ref()),
            "mutation failures must not expose an outside path"
        );
    }

    #[cfg(unix)]
    #[test]
    fn current_head_tree_object_symlink_fails_closed_before_gix_reads_outside() {
        use std::os::unix::fs::symlink;

        let Some(project) = git_project() else {
            return;
        };
        let tree = git_stdout(project.path(), &["rev-parse", "HEAD^{tree}"]);
        let object_path = project
            .path()
            .join(".git/objects")
            .join(&tree[..2])
            .join(&tree[2..]);
        assert!(object_path.is_file(), "fixture tree object should be loose");
        let outside = tempdir().expect("outside object directory");
        let outside_object = outside.path().join("tree-object");
        fs::rename(&object_path, &outside_object).expect("move object outside");
        symlink(&outside_object, &object_path).expect("link hostile object");

        let metadata = read_repository_metadata(project.path());
        assert!(metadata.dirty);
        assert_eq!(metadata.issues[0].code, "repository_open_failed");
        assert!(
            metadata.issues[0]
                .message
                .contains("metadata contains a symlink")
        );
    }

    #[test]
    fn nested_metadata_walk_fails_closed_at_the_pinned_entry_ceiling() {
        assert_eq!(MAX_METADATA_TREE_ENTRIES, 100_000);

        let metadata = tempdir().expect("metadata tree");
        fs::create_dir(metadata.path().join("nested")).expect("nested directory");
        fs::write(metadata.path().join("one"), b"one").expect("first entry");
        fs::write(metadata.path().join("nested/two"), b"two").expect("second entry");

        let mut visited_entries = 0;
        let mut directories = Vec::new();
        let error = reject_nested_metadata_symlinks(
            metadata.path(),
            &mut visited_entries,
            2,
            &mut directories,
        )
        .expect_err("third visited entry must exceed the synthetic ceiling");

        assert_eq!(visited_entries, 3);
        assert!(
            error.to_string().contains("nested-entry limit of 2"),
            "{error:#}"
        );
    }

    #[test]
    fn changed_path_scopes_distinguish_staged_unstaged_and_untracked() {
        let Some(project) = git_project() else {
            return;
        };
        fs::write(project.path().join("lib.rs"), "pub fn staged() {}\n").expect("staged edit");
        git(project.path(), &["add", "lib.rs"]);
        fs::write(project.path().join("lib.rs"), "pub fn unstaged() {}\n").expect("unstaged edit");
        fs::write(project.path().join("new.rs"), "// new\n").expect("untracked file");

        assert_eq!(
            paths(read_repository_changes(
                project.path(),
                RepositoryChangeScope::Staged
            )),
            vec![b"lib.rs".to_vec()]
        );
        assert_eq!(
            paths(read_repository_changes(
                project.path(),
                RepositoryChangeScope::Unstaged
            )),
            vec![b"lib.rs".to_vec()]
        );
        assert_eq!(
            paths(read_repository_changes(
                project.path(),
                RepositoryChangeScope::Untracked
            )),
            vec![b"new.rs".to_vec()]
        );
        assert_eq!(
            paths(read_repository_changes(
                project.path(),
                RepositoryChangeScope::Head
            )),
            vec![b"lib.rs".to_vec()]
        );
    }

    #[test]
    fn staged_rename_preserves_the_public_rename_kind() {
        let Some(project) = git_project() else {
            return;
        };
        git(project.path(), &["mv", "lib.rs", "renamed.rs"]);

        let changes = read_repository_changes(project.path(), RepositoryChangeScope::Staged)
            .expect("read staged rename");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, RepositoryChangeKind::Renamed);
        assert_eq!(changes[0].path, b"renamed.rs".to_vec());
        assert_eq!(changes[0].previous_path, Some(b"lib.rs".to_vec()));
    }

    #[test]
    fn staged_modified_rename_uses_bounded_similarity_tracking() {
        let Some(project) = git_project() else {
            return;
        };
        fs::write(
            project.path().join("lib.rs"),
            "pub fn run() {}\npub fn stop() {}\npub fn pause() {}\npub fn resume() {}\n",
        )
        .expect("expand original source");
        git(project.path(), &["add", "lib.rs"]);
        git(project.path(), &["commit", "-m", "expand source"]);
        git(project.path(), &["mv", "lib.rs", "renamed.rs"]);
        fs::write(
            project.path().join("renamed.rs"),
            "pub fn run() {}\npub fn stop() {}\npub fn pause() {}\npub fn resume() {}\npub fn added() {}\n",
        )
        .expect("modify renamed source");
        git(project.path(), &["add", "renamed.rs"]);

        let changes = read_repository_changes(project.path(), RepositoryChangeScope::Staged)
            .expect("read modified staged rename");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, RepositoryChangeKind::Renamed);
        assert_eq!(changes[0].path, b"renamed.rs".to_vec());
        assert_eq!(changes[0].previous_path, Some(b"lib.rs".to_vec()));
    }

    #[test]
    fn tracked_sources_excluded_by_repo_rules_are_restored_without_flagging_untracked_output() {
        let Some(project) = git_project() else {
            return;
        };
        fs::write(project.path().join(".gitignore"), "lib.rs\nscratch.rs\n")
            .expect("write repository ignores");
        fs::write(project.path().join("scratch.rs"), "pub fn generated() {}\n")
            .expect("write ignored untracked output");
        git(project.path(), &["add", ".gitignore"]);
        git(
            project.path(),
            &["commit", "-m", "ignore tracked and generated sources"],
        );

        let manifest =
            crate::WorkspaceManifest::open(project.path().to_path_buf()).expect("open workspace");
        let inventory = manifest.source_inventory().expect("discover sources");

        assert!(inventory.files.contains(&project.path().join("lib.rs")));
        assert!(!inventory.files.contains(&project.path().join("scratch.rs")));
        // The restored file is present and indexed, so the inventory stays
        // complete and serviceable; the degraded discovery route is recorded
        // as a warning rather than an issue.
        assert_eq!(
            inventory.outcome,
            crate::WorkspaceInventoryOutcome::Complete
        );
        assert!(inventory.issues.is_empty(), "{inventory:?}");
        assert_eq!(inventory.warnings.len(), 1, "{inventory:?}");
        assert_eq!(inventory.warnings[0].path, project.path().join("lib.rs"));
        assert!(
            inventory.warnings[0]
                .message
                .contains("ignore rules excluded a tracked source")
        );
    }

    #[test]
    fn unavailable_repository_metadata_cannot_yield_a_complete_inventory() {
        let Some(project) = git_project() else {
            return;
        };
        fs::write(project.path().join(".gitignore"), "lib.rs\n").expect("write repository ignores");
        git(project.path(), &["add", ".gitignore"]);
        git(project.path(), &["commit", "-m", "ignore tracked source"]);
        fs::write(project.path().join(".git/config"), "[invalid\n")
            .expect("corrupt local config deterministically");

        let manifest =
            crate::WorkspaceManifest::open(project.path().to_path_buf()).expect("open workspace");
        let inventory = manifest.source_inventory().expect("discover sources");

        assert_eq!(inventory.outcome, crate::WorkspaceInventoryOutcome::Partial);
        assert!(
            inventory.issues.iter().any(|issue| {
                issue
                    .message
                    .contains("repository metadata observation repository_open_failed")
            }),
            "{inventory:?}"
        );
    }

    #[test]
    fn unavailable_head_tree_does_not_poison_a_complete_tracked_inventory() {
        let Some(project) = git_project() else {
            return;
        };
        fs::write(
            project.path().join(".git/HEAD"),
            "ref: refs/heads/missing\n",
        )
        .expect("make head-tree observation partial");

        let manifest =
            crate::WorkspaceManifest::open(project.path().to_path_buf()).expect("open workspace");
        let inventory = manifest.source_inventory().expect("discover sources");

        assert!(inventory.files.contains(&project.path().join("lib.rs")));
        assert_eq!(
            inventory.outcome,
            crate::WorkspaceInventoryOutcome::Complete
        );
        assert!(inventory.issues.is_empty(), "{inventory:?}");
    }

    #[test]
    fn unavailable_submodule_status_does_not_poison_a_complete_tracked_inventory() {
        let Some(submodule_source) = git_project() else {
            return;
        };
        let Some(parent) = git_project() else {
            return;
        };
        git(
            parent.path(),
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                submodule_source.path().to_str().expect("UTF-8 submodule"),
                "nested",
            ],
        );
        git(parent.path(), &["commit", "-am", "add submodule"]);

        let manifest =
            crate::WorkspaceManifest::open(parent.path().to_path_buf()).expect("open workspace");
        let inventory = manifest.source_inventory().expect("discover sources");

        assert!(inventory.files.contains(&parent.path().join("lib.rs")));
        assert_eq!(
            inventory.outcome,
            crate::WorkspaceInventoryOutcome::Complete
        );
        assert!(inventory.issues.is_empty(), "{inventory:?}");
    }

    #[test]
    fn ancestor_ignore_rules_do_not_hide_the_selected_workspace() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let parent = tempdir().expect("workspace parent");
        let project = parent.path().join("repo");
        fs::create_dir_all(project.join("src")).expect("create project");
        git(&project, &["init"]);
        git(
            &project,
            &["config", "user.email", "codestory@example.invalid"],
        );
        git(&project, &["config", "user.name", "CodeStory Test"]);
        fs::write(project.join("src/lib.rs"), "pub fn visible() {}\n")
            .expect("write tracked source");
        git(&project, &["add", "."]);
        git(&project, &["commit", "-m", "init"]);
        fs::write(parent.path().join(".gitignore"), "repo/src/lib.rs\n")
            .expect("write ancestor ignore");

        let manifest =
            crate::WorkspaceManifest::open(project.clone()).expect("open selected workspace");
        let inventory = manifest.source_inventory().expect("discover sources");

        assert_eq!(
            inventory.outcome,
            crate::WorkspaceInventoryOutcome::Complete
        );
        assert!(inventory.issues.is_empty(), "{:?}", inventory.issues);
        assert!(inventory.files.contains(&project.join("src/lib.rs")));
    }

    fn paths(changes: Result<Vec<RepositoryChange>>) -> Vec<Vec<u8>> {
        changes
            .expect("read changes")
            .into_iter()
            .map(|change| change.path)
            .collect()
    }

    fn git_project() -> Option<tempfile::TempDir> {
        if Command::new("git").arg("--version").output().is_err() {
            return None;
        }
        let project = tempdir().expect("project");
        git(project.path(), &["init"]);
        git(
            project.path(),
            &["config", "user.email", "codestory@example.invalid"],
        );
        git(project.path(), &["config", "user.name", "CodeStory Test"]);
        git(
            project.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://example.com/team/repo.git",
            ],
        );
        fs::write(project.path().join("lib.rs"), "pub fn run() {}\n").expect("write source");
        git(project.path(), &["add", "."]);
        git(project.path(), &["commit", "-m", "init"]);
        Some(project)
    }

    fn git(project: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(project)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(project: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(project)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("UTF-8 git output")
            .trim()
            .to_string()
    }

    fn git_is_dirty(project: &Path) -> bool {
        let output = Command::new("git")
            .arg("--no-optional-locks")
            .arg("-C")
            .arg(project)
            .args(["status", "--porcelain"])
            .output()
            .expect("run git status");
        assert!(output.status.success());
        !output.stdout.is_empty()
    }
}
