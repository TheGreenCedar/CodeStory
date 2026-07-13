use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Version of the repository identity hashing contract.
pub const REPOSITORY_IDENTITY_SCHEMA_VERSION: u32 = 1;
/// Lossless repository identity hashing contract available for migration.
pub const REPOSITORY_IDENTITY_V2_SCHEMA_VERSION: u32 = 2;

/// Shared project identity contract version.
pub const PROJECT_IDENTITY_SCHEMA_VERSION: u32 = 2;
/// Lossless shared project identity contract available for migration.
pub const PROJECT_IDENTITY_V3_SCHEMA_VERSION: u32 = 3;
const PROJECT_IDENTITY_OBSERVATION_CACHE_TTL: Duration = Duration::from_secs(1);

static PROJECT_IDENTITY_OBSERVATION_CACHE: OnceLock<
    Mutex<HashMap<PathBuf, (Instant, ProjectIdentityV2)>>,
> = OnceLock::new();
static PROJECT_IDENTITY_V3_OBSERVATION_CACHE: OnceLock<
    Mutex<HashMap<PathBuf, (Instant, ProjectIdentityV3)>>,
> = OnceLock::new();

/// Git-derived identity used to decide whether portable sidecar cache reuse is
/// safe for a project root.
#[derive(Debug, Clone, Serialize)]
pub struct RepositoryIdentity {
    pub canonical_repository_id: Option<String>,
    pub repository_identity_schema_version: u32,
    pub normalized_repository_identity: Option<String>,
    pub git_remote: Option<String>,
    pub git_tree: Option<String>,
    pub portable_reuse_eligible: bool,
    pub portable_reuse_reason: String,
}

/// Lossless repository identity contract for staged schema-2 migrations.
#[derive(Debug, Clone, Serialize)]
pub struct RepositoryIdentityV2 {
    pub canonical_repository_id: Option<String>,
    pub legacy_canonical_repository_id: Option<String>,
    pub repository_identity_schema_version: u32,
    pub normalized_repository_identity: Option<String>,
    pub git_remote: Option<String>,
    pub git_tree: Option<String>,
    pub portable_reuse_eligible: bool,
    pub portable_reuse_reason: String,
}

/// Stable logical, workspace, and artifact identities for a project root.
///
/// `project_id` identifies the repository independently of checkout state when
/// a canonical repository identity is available. `workspace_id` always scopes
/// to one canonical root. `artifact_scope_id` fails closed to that workspace
/// whenever portable reuse is not eligible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectIdentityV2 {
    pub project_identity_schema_version: u32,
    pub project_id: String,
    pub workspace_id: String,
    pub artifact_scope_id: String,
    pub canonical_repository_id: Option<String>,
    pub legacy_raw_root_project_id: Option<String>,
    pub normalized_root_project_id_alias: Option<String>,
    pub portable_reuse_eligible: bool,
    pub portable_reuse_reason: String,
}

/// Lossless project identity contract for staged schema-3 migrations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectIdentityV3 {
    pub project_identity_schema_version: u32,
    pub project_id: String,
    pub workspace_id: String,
    pub artifact_scope_id: String,
    pub canonical_repository_id: Option<String>,
    #[serde(default)]
    pub legacy_canonical_repository_id: Option<String>,
    pub legacy_raw_root_project_id: Option<String>,
    pub normalized_root_project_id_alias: Option<String>,
    pub portable_reuse_eligible: bool,
    pub portable_reuse_reason: String,
}

/// Project id decision for sidecar artifacts.
///
/// Clean, identifiable Git repositories use a stable repository-derived id.
/// Dirty, missing, or non-Git roots fall back to the root-derived id so cached
/// sidecars do not cross an unsafe freshness boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarProjectIdentity {
    pub project_id: String,
    pub canonical_repository_id: Option<String>,
    pub root_derived_project_id: String,
    pub portable_reuse_eligible: bool,
    pub portable_reuse_reason: String,
}

/// Inspect Git remote, tree, and dirtiness for portable cache reuse.
pub fn inspect_repository_identity(project_root: &Path) -> RepositoryIdentity {
    let remote = git_output(project_root, &["config", "--get", "remote.origin.url"]).ok();
    let tree = git_output(project_root, &["rev-parse", "HEAD^{tree}"]).ok();
    let dirty = git_output(project_root, &["status", "--porcelain"])
        .map(|status| !status.trim().is_empty())
        .unwrap_or(true);
    let normalized = remote.as_deref().and_then(normalize_repository_identity_v1);
    let canonical_repository_id = normalized.as_deref().map(legacy_canonical_repository_id);
    let (portable_reuse_eligible, portable_reuse_reason) =
        portable_reuse_status(normalized.as_deref(), tree.as_deref(), dirty);

    RepositoryIdentity {
        canonical_repository_id,
        repository_identity_schema_version: REPOSITORY_IDENTITY_SCHEMA_VERSION,
        normalized_repository_identity: normalized,
        git_remote: remote,
        git_tree: tree,
        portable_reuse_eligible,
        portable_reuse_reason,
    }
}

/// Inspect the lossless repository identity without migrating current consumers.
pub fn inspect_repository_identity_v2(project_root: &Path) -> RepositoryIdentityV2 {
    let remote = git_output(project_root, &["config", "--get", "remote.origin.url"]).ok();
    let tree = git_output(project_root, &["rev-parse", "HEAD^{tree}"]).ok();
    let dirty = git_output(project_root, &["status", "--porcelain"])
        .map(|status| !status.trim().is_empty())
        .unwrap_or(true);
    let normalized = remote.as_deref().and_then(parse_repository_identity_v2);
    let canonical_repository_id = normalized
        .as_ref()
        .map(|identity| canonical_repository_id_v2(&identity.canonical));
    // A current remote spelling cannot prove which schema-1 identity owns
    // persisted artifacts. Parent #913 will supply provenance during migration.
    let legacy_canonical_repository_id = None;
    let (portable_reuse_eligible, portable_reuse_reason) = portable_reuse_status(
        normalized
            .as_ref()
            .map(|identity| identity.canonical.as_str()),
        tree.as_deref(),
        dirty,
    );

    RepositoryIdentityV2 {
        canonical_repository_id,
        legacy_canonical_repository_id,
        repository_identity_schema_version: REPOSITORY_IDENTITY_V2_SCHEMA_VERSION,
        normalized_repository_identity: normalized.map(|identity| identity.canonical),
        git_remote: remote,
        git_tree: tree,
        portable_reuse_eligible,
        portable_reuse_reason,
    }
}

/// Resolve the shared V2 identity contract for a project root.
pub fn project_identity_v2(project_root: &Path) -> ProjectIdentityV2 {
    let repository_identity = inspect_repository_identity(project_root);
    let root_identity = workspace_root_identity(project_root);
    let project_id = repository_identity
        .canonical_repository_id
        .clone()
        .unwrap_or_else(|| root_identity.workspace_id.clone());
    let artifact_scope_id = if repository_identity.portable_reuse_eligible {
        project_id.clone()
    } else {
        root_identity.workspace_id.clone()
    };

    ProjectIdentityV2 {
        project_identity_schema_version: PROJECT_IDENTITY_SCHEMA_VERSION,
        project_id,
        workspace_id: root_identity.workspace_id,
        artifact_scope_id,
        canonical_repository_id: repository_identity.canonical_repository_id,
        legacy_raw_root_project_id: root_identity.legacy_raw_root_project_id,
        normalized_root_project_id_alias: root_identity.normalized_root_project_id_alias,
        portable_reuse_eligible: repository_identity.portable_reuse_eligible,
        portable_reuse_reason: repository_identity.portable_reuse_reason,
    }
}

/// Resolve the lossless V3 identity without migrating current consumers.
pub fn project_identity_v3(project_root: &Path) -> ProjectIdentityV3 {
    let repository_identity = inspect_repository_identity_v2(project_root);
    let root_identity = workspace_root_identity_v3(project_root);
    let project_id = repository_identity
        .canonical_repository_id
        .clone()
        .unwrap_or_else(|| root_identity.workspace_id.clone());
    let artifact_scope_id = if repository_identity.portable_reuse_eligible {
        project_id.clone()
    } else {
        root_identity.workspace_id.clone()
    };

    ProjectIdentityV3 {
        project_identity_schema_version: PROJECT_IDENTITY_V3_SCHEMA_VERSION,
        project_id,
        workspace_id: root_identity.workspace_id,
        artifact_scope_id,
        canonical_repository_id: repository_identity.canonical_repository_id,
        legacy_canonical_repository_id: repository_identity.legacy_canonical_repository_id,
        legacy_raw_root_project_id: root_identity.legacy_raw_root_project_id,
        normalized_root_project_id_alias: root_identity.normalized_root_project_id_alias,
        portable_reuse_eligible: repository_identity.portable_reuse_eligible,
        portable_reuse_reason: repository_identity.portable_reuse_reason,
    }
}

/// Resolve identity for repeated observational status reads.
///
/// Mutating/indexing paths must use `project_identity_v2` so dirtiness changes
/// are observed immediately.
pub fn cached_project_identity_v2(project_root: &Path) -> ProjectIdentityV2 {
    let key = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let cache = PROJECT_IDENTITY_OBSERVATION_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached_at, identity)) = cache.get(&key)
        && cached_at.elapsed() < PROJECT_IDENTITY_OBSERVATION_CACHE_TTL
    {
        return identity.clone();
    }
    let identity = project_identity_v2(project_root);
    cache.insert(key, (Instant::now(), identity.clone()));
    identity
}

/// Resolve lossless identity for repeated observational status reads.
///
/// Mutating paths must use `project_identity_v3` so dirtiness changes are
/// observed immediately.
pub fn cached_project_identity_v3(project_root: &Path) -> ProjectIdentityV3 {
    let key = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let cache = PROJECT_IDENTITY_V3_OBSERVATION_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached_at, identity)) = cache.get(&key)
        && cached_at.elapsed() < PROJECT_IDENTITY_OBSERVATION_CACHE_TTL
    {
        return identity.clone();
    }
    let identity = project_identity_v3(project_root);
    cache.insert(key, (Instant::now(), identity.clone()));
    identity
}

/// Return the canonical-root FNV identity used to scope one workspace.
pub fn workspace_id_for_root(project_root: &Path) -> String {
    workspace_root_identity(project_root).workspace_id
}

/// Return the schema-3 workspace id hashed from native path data.
pub fn workspace_id_v3_for_root(project_root: &Path) -> String {
    workspace_root_identity_v3(project_root).workspace_id
}

/// Choose the sidecar project id while preserving the fallback reason.
pub fn sidecar_project_identity(
    project_root: &Path,
    root_derived_project_id: String,
) -> SidecarProjectIdentity {
    let identity = project_identity_v2(project_root);
    let project_id = if identity.portable_reuse_eligible {
        identity.project_id
    } else {
        root_derived_project_id.clone()
    };

    SidecarProjectIdentity {
        project_id,
        canonical_repository_id: identity.canonical_repository_id,
        root_derived_project_id,
        portable_reuse_eligible: identity.portable_reuse_eligible,
        portable_reuse_reason: identity.portable_reuse_reason,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceRootIdentity {
    workspace_id: String,
    legacy_raw_root_project_id: Option<String>,
    normalized_root_project_id_alias: Option<String>,
}

fn workspace_root_identity(project_root: &Path) -> WorkspaceRootIdentity {
    let raw_root = project_root.to_string_lossy();
    let canonical_root = fs::canonicalize(project_root)
        .unwrap_or_else(|_| project_root.to_path_buf())
        .to_string_lossy()
        .into_owned();
    workspace_root_identity_from_text(&raw_root, &canonical_root)
}

fn workspace_root_identity_v3(project_root: &Path) -> WorkspaceRootIdentity {
    let canonical_root =
        fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let workspace_id = fnv1a_path_hex(&canonical_root);
    let legacy_raw_root_project_id = project_root
        .to_str()
        .and_then(|root| fnv_alias(root, &workspace_id));
    let normalized_root_project_id_alias = canonical_root
        .to_str()
        .and_then(|root| fnv_alias(&normalize_root_identity_text(root), &workspace_id));

    WorkspaceRootIdentity {
        workspace_id,
        legacy_raw_root_project_id,
        normalized_root_project_id_alias,
    }
}

fn workspace_root_identity_from_text(
    raw_root: &str,
    canonical_root: &str,
) -> WorkspaceRootIdentity {
    let workspace_id = fnv1a_hex(canonical_root.as_bytes());
    let legacy_raw_root_project_id = fnv_alias(raw_root, &workspace_id);
    let normalized_root_project_id_alias =
        fnv_alias(&normalize_root_identity_text(canonical_root), &workspace_id);

    WorkspaceRootIdentity {
        workspace_id,
        legacy_raw_root_project_id,
        normalized_root_project_id_alias,
    }
}

fn fnv_alias(root: &str, workspace_id: &str) -> Option<String> {
    let legacy_id = fnv1a_hex(root.as_bytes());
    (legacy_id != workspace_id).then_some(legacy_id)
}

fn normalize_root_identity_text(root: &str) -> String {
    let trimmed = root.trim();
    let windows_style = trimmed.contains('\\')
        || trimmed.starts_with("//")
        || trimmed.as_bytes().get(1) == Some(&b':');
    let mut normalized = trimmed.replace('\\', "/");

    if normalized
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("//?/unc/"))
    {
        normalized = format!("//{}", &normalized[8..]);
    } else if normalized.starts_with("//?/") {
        normalized.drain(..4);
    }

    let preserve_unc = normalized.starts_with("//");
    let mut collapsed = String::with_capacity(normalized.len());
    let mut previous_was_separator = false;
    for ch in normalized.chars() {
        if ch == '/' {
            if !previous_was_separator {
                collapsed.push(ch);
            }
            previous_was_separator = true;
        } else {
            collapsed.push(ch);
            previous_was_separator = false;
        }
    }
    if preserve_unc && !collapsed.starts_with("//") {
        collapsed.insert(0, '/');
    }

    while collapsed.len() > 1 && collapsed.ends_with('/') && !is_windows_drive_root(&collapsed) {
        collapsed.pop();
    }

    if windows_style {
        collapsed.make_ascii_lowercase();
    }
    collapsed
}

fn is_windows_drive_root(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() == 3 && bytes[0].is_ascii_alphabetic() && bytes[1..] == *b":/"
}

fn portable_reuse_status(
    normalized: Option<&str>,
    tree: Option<&str>,
    dirty: bool,
) -> (bool, String) {
    if normalized.is_none() {
        return (false, "git_remote_missing".into());
    }
    if tree.is_none() {
        return (false, "git_tree_unavailable".into());
    }
    if dirty {
        return (false, "git_worktree_dirty".into());
    }
    (true, "eligible".into())
}

fn canonical_repository_id_v2(normalized_repository_identity: &str) -> String {
    versioned_repository_id(
        REPOSITORY_IDENTITY_V2_SCHEMA_VERSION,
        normalized_repository_identity,
    )
}

fn legacy_canonical_repository_id(normalized_repository_identity: &str) -> String {
    versioned_repository_id(1, normalized_repository_identity)
}

fn versioned_repository_id(schema_version: u32, normalized_repository_identity: &str) -> String {
    let mut state = 0xcbf29ce484222325_u64;
    mix_str(&mut state, "codestory-repository-identity");
    mix_u32(&mut state, schema_version);
    mix_str(&mut state, normalized_repository_identity);
    format!("repo-v{schema_version}-{state:016x}")
}

fn normalize_repository_identity_v1(remote: &str) -> Option<String> {
    let value = remote.trim().replace('\\', "/");
    if value.is_empty() {
        return None;
    }

    if let Some((_, rest)) = value.split_once("://") {
        return normalize_url_repository_identity_v1(rest);
    }

    let without_user = strip_userinfo(&value);
    let scp_like = without_user.find(':').is_some_and(|colon| {
        without_user[..colon].find('/').is_none() && without_user[colon + 1..].contains('/')
    });
    let normalized = if scp_like {
        without_user.replacen(':', "/", 1)
    } else {
        without_user.to_string()
    };
    normalize_repository_path_v1(&normalized)
}

fn normalize_url_repository_identity_v1(rest: &str) -> Option<String> {
    let rest = strip_userinfo(rest);
    let (authority, path) = rest.split_once('/')?;
    let host = authority
        .split_once(':')
        .map_or(authority, |(host, _)| host);
    normalize_repository_path_v1(&format!("{host}/{path}"))
}

fn normalize_repository_path_v1(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let normalized = lower
        .trim_start_matches('/')
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .trim_end_matches('/')
        .to_string();
    (!normalized.is_empty()).then_some(normalized)
}

#[cfg(test)]
fn normalize_repository_identity_v2(remote: &str) -> Option<String> {
    parse_repository_identity_v2(remote).map(|identity| identity.canonical)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedRepositoryIdentity {
    canonical: String,
}

fn parse_repository_identity_v2(remote: &str) -> Option<NormalizedRepositoryIdentity> {
    let value = remote.trim().replace('\\', "/");
    if value.is_empty()
        || value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || (value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && value.as_bytes().get(1) == Some(&b':'))
    {
        return None;
    }

    let (scheme, host, port, path, is_url) = if let Some((scheme, rest)) = value.split_once("://") {
        let (scheme, host, port, path) = normalize_url_repository_identity(scheme, rest)?;
        (scheme, host, port, path, true)
    } else {
        let (scheme, host, port, path) = normalize_scp_repository_identity(&value)?;
        (scheme, host, port, path, false)
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let path = normalize_repository_path(if is_url {
        &path
    } else {
        path.strip_prefix('/').unwrap_or(&path)
    })?;
    let scheme = scheme.to_ascii_lowercase();
    let canonical = match port.as_deref() {
        Some(port) => format!("{scheme}://{host}:{port}/{path}"),
        None => format!("{scheme}://{host}/{path}"),
    };

    Some(NormalizedRepositoryIdentity { canonical })
}

fn normalize_scp_repository_identity(
    value: &str,
) -> Option<(String, String, Option<String>, String)> {
    let without_user = strip_userinfo(value);
    if let Some((host, path)) = split_scp_host_path(without_user) {
        let path = if path.starts_with('/') || path.starts_with('~') {
            path.to_string()
        } else {
            format!("~/{path}")
        };
        return Some(("ssh".into(), host.into(), Some("22".into()), path));
    }
    None
}

fn split_scp_host_path(value: &str) -> Option<(&str, &str)> {
    if value.starts_with('[') {
        let end = value.find(']')?;
        let host = &value[..=end];
        let path = value[end + 1..].strip_prefix(':')?;
        return (!path.is_empty()).then_some((host, path));
    }
    let (host, path) = value.split_once(':')?;
    (!host.contains('/') && !path.is_empty()).then_some((host, path))
}

fn normalize_url_repository_identity(
    scheme: &str,
    rest: &str,
) -> Option<(String, String, Option<String>, String)> {
    let scheme = scheme.to_ascii_lowercase();
    if scheme == "file" {
        return None;
    }
    let rest = strip_userinfo(rest);
    let (authority, path) = rest.split_once('/')?;
    let (host, explicit_port) = split_host_port(authority)?;
    let default_port = match scheme.as_str() {
        "http" => Some("80"),
        "https" => Some("443"),
        "ssh" => Some("22"),
        "git" => Some("9418"),
        _ => None,
    };
    let port = explicit_port.or_else(|| default_port.map(str::to_string));
    Some((scheme, host.to_string(), port, path.to_string()))
}

fn split_host_port(authority: &str) -> Option<(&str, Option<String>)> {
    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let host = &authority[..=end];
        let suffix = &authority[end + 1..];
        if suffix.is_empty() {
            return Some((host, None));
        }
        let port = suffix.strip_prefix(':')?;
        return valid_port(port).map(|port| (host, Some(port)));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => valid_port(port).map(|port| (host, Some(port))),
        None => Some((authority, None)),
    }
}

fn valid_port(port: &str) -> Option<String> {
    let port = port.parse::<u16>().ok()?;
    (port != 0).then(|| port.to_string())
}

fn strip_userinfo(value: &str) -> &str {
    value.split_once('@').map(|(_, rest)| rest).unwrap_or(value)
}

fn normalize_repository_path(value: &str) -> Option<String> {
    let mut normalized = value.trim_end_matches('/').to_string();
    if normalized.ends_with(".git") {
        normalized.truncate(normalized.len() - 4);
        normalized = normalized.trim_end_matches('/').to_string();
    }
    (!normalized.is_empty()).then_some(normalized)
}

/// Compare workspace paths without merging distinct case-sensitive roots.
///
/// Existing paths use the filesystem's stable file identity. Only two missing
/// paths use platform lexical rules. Windows missing-path comparison normalizes
/// separators and dot segments and uses ordinal Unicode case comparison, but
/// cannot observe a missing path's future per-directory case-sensitivity flag.
pub fn same_workspace_path(left: &Path, right: &Path) -> bool {
    match (fs::metadata(left), fs::metadata(right)) {
        (Ok(left_metadata), Ok(right_metadata)) => {
            same_existing_path(left, &left_metadata, right, &right_metadata)
        }
        (Err(left_error), Err(right_error))
            if left_error.kind() == std::io::ErrorKind::NotFound
                && right_error.kind() == std::io::ErrorKind::NotFound =>
        {
            same_missing_path(left, right)
        }
        _ => false,
    }
}

#[cfg(unix)]
fn same_existing_path(
    _left_path: &Path,
    left: &fs::Metadata,
    _right_path: &Path,
    right: &fs::Metadata,
) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(windows)]
fn same_existing_path(
    left_path: &Path,
    _left: &fs::Metadata,
    right_path: &Path,
    _right: &fs::Metadata,
) -> bool {
    windows_file_identity(left_path)
        .zip(windows_file_identity(right_path))
        .is_some_and(|(left, right)| left == right)
}

#[cfg(not(any(unix, windows)))]
fn same_existing_path(
    _left_path: &Path,
    _left: &fs::Metadata,
    _right_path: &Path,
    _right: &fs::Metadata,
) -> bool {
    false
}

#[cfg(windows)]
fn windows_file_identity(path: &Path) -> Option<(u64, [u8; 16])> {
    use std::ffi::c_void;
    use std::mem::MaybeUninit;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct FileIdInfo {
        volume_serial_number: u64,
        file_id: [u8; 16],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut c_void,
            information_class: i32,
            information: *mut c_void,
            information_size: u32,
        ) -> i32;
    }

    const FILE_ID_INFO_CLASS: i32 = 18;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let file = fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .ok()?;
    let mut information = MaybeUninit::<FileIdInfo>::uninit();
    // SAFETY: `file` owns a valid handle for the duration of the call and the
    // output points to correctly sized, writable storage.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle().cast(),
            FILE_ID_INFO_CLASS,
            information.as_mut_ptr().cast(),
            std::mem::size_of::<FileIdInfo>() as u32,
        )
    } == 0
    {
        return None;
    }
    // SAFETY: a successful `GetFileInformationByHandleEx` initializes all fields.
    let information = unsafe { information.assume_init() };
    Some((information.volume_serial_number, information.file_id))
}

#[cfg(windows)]
fn same_missing_path(left: &Path, right: &Path) -> bool {
    windows_ordinal_case_eq(
        &normalize_windows_lexical_path(left),
        &normalize_windows_lexical_path(right),
    )
}

#[cfg(windows)]
fn normalize_windows_lexical_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    let mut units = path
        .as_os_str()
        .encode_wide()
        .map(|unit| {
            if unit == u16::from(b'/') {
                u16::from(b'\\')
            } else {
                unit
            }
        })
        .collect::<Vec<_>>();
    let separator = u16::from(b'\\');
    let extended = [separator, separator, u16::from(b'?'), separator];
    if units.starts_with(&extended) {
        units.drain(..extended.len());
        let unc = [u16::from(b'U'), u16::from(b'N'), u16::from(b'C'), separator];
        if units
            .get(..unc.len())
            .is_some_and(|prefix| windows_ordinal_case_eq(prefix, &unc))
        {
            units.splice(..unc.len(), [separator, separator]);
        }
    }

    let unc = units.starts_with(&[separator, separator]);
    let drive = units.len() >= 2 && units[1] == u16::from(b':');
    let rooted =
        unc || units.first() == Some(&separator) || (drive && units.get(2) == Some(&separator));
    let prefix_len = if unc {
        2
    } else if drive {
        2 + if rooted { 1 } else { 0 }
    } else {
        if rooted { 1 } else { 0 }
    };
    let prefix = units[..prefix_len.min(units.len())].to_vec();
    let protected_segments = if unc { 2 } else { 0 };
    let mut segments = Vec::<Vec<u16>>::new();
    for segment in units[prefix_len.min(units.len())..]
        .split(|unit| *unit == separator)
        .filter(|segment| !segment.is_empty())
    {
        if segment == [u16::from(b'.')] {
            continue;
        }
        if segment == [u16::from(b'.'), u16::from(b'.')] {
            if segments.len() > protected_segments
                && segments
                    .last()
                    .is_some_and(|last| last.as_slice() != segment)
            {
                segments.pop();
            } else if !rooted {
                segments.push(segment.to_vec());
            }
            continue;
        }
        segments.push(segment.to_vec());
    }

    let mut normalized = prefix;
    for (index, segment) in segments.into_iter().enumerate() {
        if !normalized.is_empty()
            && normalized.last() != Some(&separator)
            && !(drive && !rooted && index == 0)
        {
            normalized.push(separator);
        }
        normalized.extend(segment);
    }
    normalized
}

#[cfg(windows)]
fn windows_ordinal_case_eq(left: &[u16], right: &[u16]) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CompareStringOrdinal(
            left: *const u16,
            left_len: i32,
            right: *const u16,
            right_len: i32,
            ignore_case: i32,
        ) -> i32;
    }

    if left == right {
        return true;
    }
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let (Ok(left_len), Ok(right_len)) = (i32::try_from(left.len()), i32::try_from(right.len()))
    else {
        return false;
    };
    const CSTR_EQUAL: i32 = 2;
    // SAFETY: both pointers remain valid for their supplied lengths, and the
    // API only reads them. Windows ordinal ignore-case is the closest lexical
    // fallback available when a missing path has no filesystem identity.
    unsafe {
        CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, 1) == CSTR_EQUAL
    }
}

#[cfg(not(windows))]
fn same_missing_path(left: &Path, right: &Path) -> bool {
    left == right
}

fn git_output(project: &Path, args: &[&str]) -> Result<String, ()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .output()
        .map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn mix_u32(state: &mut u64, value: u32) {
    for byte in value.to_le_bytes() {
        *state ^= u64::from(byte);
        *state = state.wrapping_mul(0x00000100000001B3);
    }
}

fn mix_str(state: &mut u64, value: &str) {
    for byte in value.as_bytes() {
        *state ^= u64::from(*byte);
        *state = state.wrapping_mul(0x00000100000001B3);
    }
}

fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(unix)]
fn fnv1a_path_hex(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    fnv1a_hex(path.as_os_str().as_bytes())
}

#[cfg(windows)]
fn fnv1a_path_hex(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;
    let mut state = 0xcbf29ce484222325_u64;
    for unit in path.as_os_str().encode_wide() {
        for byte in unit.to_le_bytes() {
            state ^= u64::from(byte);
            state = state.wrapping_mul(0x100000001b3);
        }
    }
    format!("{state:016x}")
}

#[cfg(not(any(unix, windows)))]
fn fnv1a_path_hex(path: &Path) -> String {
    fnv1a_hex(path.as_os_str().as_encoded_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn normalizes_common_git_remote_forms() {
        let https =
            normalize_repository_identity_v2("https://github.com/TheGreenCedar/CodeStory.git");
        assert_eq!(
            https.as_deref(),
            Some("https://github.com:443/TheGreenCedar/CodeStory")
        );
        assert_eq!(
            https,
            normalize_repository_identity_v2("HTTPS://GITHUB.COM:443/TheGreenCedar/CodeStory.git/")
        );

        let ssh =
            normalize_repository_identity_v2("ssh://git@github.com/TheGreenCedar/CodeStory.git");
        assert_eq!(
            ssh.as_deref(),
            Some("ssh://github.com:22/TheGreenCedar/CodeStory")
        );
        assert_eq!(
            ssh,
            normalize_repository_identity_v2("ssh://git@github.com:22/TheGreenCedar/CodeStory.git")
        );
        assert_eq!(
            ssh,
            normalize_repository_identity_v2("git@github.com:/TheGreenCedar/CodeStory.git")
        );
        assert_ne!(https, ssh);
    }

    #[test]
    fn repository_identity_preserves_meaningful_ports_and_path_case() {
        assert_eq!(
            normalize_repository_identity_v2("ssh://git@EXAMPLE.com:2222/Team/Repo.git").as_deref(),
            Some("ssh://example.com:2222/Team/Repo")
        );
        assert_eq!(
            normalize_repository_identity_v2("https://example.com:8443/Team/Repo.git").as_deref(),
            Some("https://example.com:8443/Team/Repo")
        );
        assert_ne!(
            normalize_repository_identity_v2("https://example.com/Team/Repo.git"),
            normalize_repository_identity_v2("https://example.com/team/repo.git")
        );
        assert_ne!(
            normalize_repository_identity_v2("https://example.com:443/team/repo.git"),
            normalize_repository_identity_v2("ssh://git@example.com:22/team/repo.git")
        );
        assert_eq!(
            normalize_repository_identity_v2("ssh://git@example.com:22/team/repo.git"),
            normalize_repository_identity_v2("git@example.com:/team/repo.git")
        );
        assert_ne!(
            normalize_repository_identity_v2("custom-a://example.com/team/repo.git"),
            normalize_repository_identity_v2("custom-b://example.com/team/repo.git")
        );
        assert_eq!(
            normalize_repository_identity_v2("https://example.com/team/repo.GIT").as_deref(),
            Some("https://example.com:443/team/repo.GIT")
        );
    }

    #[test]
    fn bracketed_scp_ipv6_matches_ssh_url() {
        assert_eq!(
            normalize_repository_identity_v2("git@[2001:DB8::1]:/team/repo.git"),
            normalize_repository_identity_v2("ssh://git@[2001:db8::1]:22/team/repo.git")
        );
    }

    #[test]
    fn rejects_unqualified_local_remote_paths() {
        for remote in [
            "repos/origin.git",
            "~/repo.git",
            "github.com/org/repo.git",
            "C:/source/repo.git",
            r"C:\source\repo.git",
            "C:repo.git",
            "file://localhost/source/repo.git",
            "file://server/share/repo.git",
        ] {
            assert!(
                normalize_repository_identity_v2(remote).is_none(),
                "local remote must fail closed: {remote}"
            );
        }
    }

    #[test]
    fn scp_absolute_and_home_relative_paths_remain_distinct() {
        let absolute = normalize_repository_identity_v2("git@example.com:/team/repo.git");
        let relative = normalize_repository_identity_v2("git@example.com:team/repo.git");

        assert_eq!(
            absolute,
            normalize_repository_identity_v2("ssh://git@example.com:22/team/repo.git")
        );
        assert_eq!(
            relative,
            normalize_repository_identity_v2("ssh://git@example.com:22/~/team/repo.git")
        );
        assert_ne!(absolute, relative);
    }

    #[test]
    fn url_path_leading_slashes_remain_distinct() {
        let standard = normalize_repository_identity_v2("https://example.com/org/repo.git");
        let doubled = normalize_repository_identity_v2("https://example.com//org/repo.git");

        assert_eq!(
            standard.as_deref(),
            Some("https://example.com:443/org/repo")
        );
        assert_eq!(
            doubled.as_deref(),
            Some("https://example.com:443//org/repo")
        );
        assert_ne!(standard, doubled);
    }

    #[test]
    fn repository_v2_never_guesses_a_legacy_alias_without_provenance() {
        let Some(project) = git_project() else {
            return;
        };
        for remote in [
            "https://example.com/team/repo.git",
            "https://example.com/Team/Repo.git",
            "ssh://example.com:2222/team/repo.git",
            "ssh://[::1]/team/repo.git",
        ] {
            git(project.path(), &["remote", "set-url", "origin", remote]);
            assert_eq!(
                inspect_repository_identity_v2(project.path()).legacy_canonical_repository_id,
                None,
                "remote: {remote}"
            );
        }
        assert!(parse_repository_identity_v2("C:/source/repo").is_none());
    }

    #[test]
    fn identity_schema_migration_changes_repository_id_without_guessing_alias() {
        assert_eq!(REPOSITORY_IDENTITY_SCHEMA_VERSION, 1);
        assert_eq!(PROJECT_IDENTITY_SCHEMA_VERSION, 2);
        assert_eq!(REPOSITORY_IDENTITY_V2_SCHEMA_VERSION, 2);
        assert_eq!(PROJECT_IDENTITY_V3_SCHEMA_VERSION, 3);
        let normalized = "example.com/team/repo";
        assert_ne!(
            canonical_repository_id_v2(normalized),
            legacy_canonical_repository_id(normalized)
        );
        assert!(canonical_repository_id_v2(normalized).starts_with("repo-v2-"));
        assert!(legacy_canonical_repository_id(normalized).starts_with("repo-v1-"));
    }

    #[test]
    fn legacy_entrypoints_remain_on_the_existing_contract() {
        assert_eq!(
            normalize_repository_identity_v1("ssh://git@EXAMPLE.com:2222/Team/Repo.git").as_deref(),
            Some("example.com/team/repo")
        );
        let Some(project) = git_project() else {
            return;
        };
        let repository = inspect_repository_identity(project.path());
        assert_eq!(repository.repository_identity_schema_version, 1);
        assert_eq!(
            repository.normalized_repository_identity.as_deref(),
            Some("github.com/thegreencedar/codestory")
        );
        assert_eq!(
            repository.canonical_repository_id.as_deref(),
            Some("repo-v1-670ad7db4da1546b")
        );
        assert_eq!(repository.portable_reuse_reason, "eligible");

        let project_identity = project_identity_v2(project.path());
        assert_eq!(project_identity.project_identity_schema_version, 2);
        assert_eq!(project_identity.project_id, "repo-v1-670ad7db4da1546b");
        assert_eq!(
            project_identity.canonical_repository_id.as_deref(),
            Some("repo-v1-670ad7db4da1546b")
        );
        assert_eq!(
            project_identity.artifact_scope_id,
            project_identity.project_id
        );
        assert_eq!(project_identity.portable_reuse_reason, "eligible");

        let root =
            workspace_root_identity_from_text(r"C:\Source\CodeStory\", r"\\?\C:\Source\CodeStory\");
        assert_eq!(root.workspace_id, "f6a770b628e5f7f2");
        assert_eq!(
            root.legacy_raw_root_project_id.as_deref(),
            Some("914a8e53209dde45")
        );
        assert_eq!(
            root.normalized_root_project_id_alias.as_deref(),
            Some("e2562715b2c4b441")
        );
    }

    #[test]
    fn portable_reuse_eligibility_fails_closed_without_identity_or_clean_tree() {
        assert_eq!(
            portable_reuse_status(None, Some("tree"), false),
            (false, "git_remote_missing".into())
        );
        assert_eq!(
            portable_reuse_status(Some("github.com/org/repo"), None, false),
            (false, "git_tree_unavailable".into())
        );
        assert_eq!(
            portable_reuse_status(Some("github.com/org/repo"), Some("tree"), true),
            (false, "git_worktree_dirty".into())
        );
    }

    #[test]
    fn sidecar_project_identity_uses_canonical_id_only_when_clean_and_identifiable() {
        let Some(project) = git_project() else {
            return;
        };

        let clean = sidecar_project_identity(project.path(), "root-id".into());
        assert!(clean.portable_reuse_eligible);
        assert_eq!(clean.project_id, clean.canonical_repository_id.unwrap());

        fs::write(project.path().join("lib.rs"), "pub fn dirty() {}\n").expect("dirty source");
        let dirty = sidecar_project_identity(project.path(), "root-id".into());
        assert!(!dirty.portable_reuse_eligible);
        assert_eq!(dirty.portable_reuse_reason, "git_worktree_dirty");
        assert_eq!(dirty.project_id, "root-id");
    }

    #[test]
    fn project_id_is_stable_across_dirty_transitions() {
        let Some(project) = git_project() else {
            return;
        };

        let clean = project_identity_v3(project.path());
        fs::write(project.path().join("lib.rs"), "pub fn dirty() {}\n").expect("dirty source");
        let dirty = project_identity_v3(project.path());

        assert_eq!(clean.project_id, dirty.project_id);
        assert_eq!(clean.workspace_id, dirty.workspace_id);
        assert!(clean.portable_reuse_eligible);
        assert!(!dirty.portable_reuse_eligible);
    }

    #[test]
    fn worktrees_share_project_id_but_not_workspace_id() {
        let Some(project) = git_project() else {
            return;
        };
        let worktree_parent = tempdir().expect("worktree parent");
        let worktree = worktree_parent.path().join("linked-worktree");
        git(
            project.path(),
            &[
                "worktree",
                "add",
                "--detach",
                worktree.to_str().expect("worktree path"),
                "HEAD",
            ],
        );

        let first = project_identity_v3(project.path());
        let second = project_identity_v3(&worktree);

        assert_eq!(first.project_id, second.project_id);
        assert_ne!(first.workspace_id, second.workspace_id);
        assert_eq!(first.artifact_scope_id, second.artifact_scope_id);
    }

    #[test]
    fn workspace_id_matches_existing_canonical_root_fnv_contract() {
        let project = tempdir().expect("project");
        let canonical = fs::canonicalize(project.path()).expect("canonical project root");
        let expected = fnv1a_path_hex(&canonical);

        assert_eq!(workspace_id_v3_for_root(project.path()), expected);
        assert_eq!(project_identity_v3(project.path()).workspace_id, expected);
    }

    #[test]
    fn canonicalization_equates_supported_path_spellings() {
        let project = tempdir().expect("project");
        let dotted = project.path().join(".");

        assert_eq!(
            workspace_id_v3_for_root(project.path()),
            workspace_id_v3_for_root(&dotted)
        );
    }

    #[test]
    fn existing_path_equality_uses_file_identity() {
        let project = tempdir().expect("project");
        let file = project.path().join("identity");
        let alias = project.path().join("identity-alias");
        fs::write(&file, "identity").expect("identity file");
        fs::hard_link(&file, &alias).expect("hard-link alias");

        assert!(same_workspace_path(&file, &alias));
        assert!(!same_workspace_path(&file, &project.path().join("missing")));
    }

    #[cfg(unix)]
    #[test]
    fn unix_paths_preserve_case_and_non_utf8_bytes() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let project = tempdir().expect("project");
        let upper = project.path().join("Repo");
        let lower = project.path().join("repo");
        fs::create_dir(&upper).expect("upper-case path");
        fs::create_dir(&lower).expect("lower-case path");
        assert!(!same_workspace_path(&upper, &lower));
        assert_ne!(
            workspace_id_v3_for_root(&upper),
            workspace_id_v3_for_root(&lower)
        );

        let first = project.path().join(OsString::from_vec(vec![b'r', 0x80]));
        let second = project.path().join(OsString::from_vec(vec![b'r', 0x81]));
        fs::create_dir(&first).expect("first non-UTF-8 path");
        fs::create_dir(&second).expect("second non-UTF-8 path");
        assert_ne!(
            workspace_id_v3_for_root(&first),
            workspace_id_v3_for_root(&second)
        );
        assert!(
            workspace_root_identity_v3(&first)
                .legacy_raw_root_project_id
                .is_none()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_and_missing_aliases_compare_case_insensitively() {
        let project = tempdir().expect("project");
        let existing = project.path().join("CodeStory");
        fs::create_dir(&existing).expect("mixed-case path");
        let existing_alias = project.path().join("codestory");
        assert!(same_workspace_path(&existing, &existing_alias));

        let missing = project.path().join("Missing");
        let missing_alias = project.path().join("missing");
        assert!(same_workspace_path(&missing, &missing_alias));

        let dotted = project
            .path()
            .join("Missing")
            .join(".")
            .join("child")
            .join("..")
            .join("Älias");
        let normalized = project.path().join("missing").join("äLIAS");
        assert!(same_workspace_path(&dotted, &normalized));

        let extended = PathBuf::from(format!(r"\\?\{}", project.path().display()));
        assert!(same_workspace_path(
            &extended.join("missing"),
            &project.path().join("MISSING")
        ));
        assert!(!same_missing_path(
            Path::new(r"C:missing"),
            Path::new(r"C:\missing")
        ));
    }

    #[test]
    fn windows_normalization_is_an_alias_not_the_workspace_id() {
        let identity =
            workspace_root_identity_from_text(r"C:\Source\CodeStory\", r"\\?\C:\Source\CodeStory\");
        let existing_canonical_id = fnv1a_hex(r"\\?\C:\Source\CodeStory\".as_bytes());
        let normalized_alias = fnv1a_hex("c:/source/codestory".as_bytes());

        assert_eq!(identity.workspace_id, existing_canonical_id);
        assert_eq!(
            identity.normalized_root_project_id_alias.as_deref(),
            Some(normalized_alias.as_str())
        );
        assert_eq!(
            normalize_root_identity_text(r"\\?\C:\Source\CodeStory\"),
            "c:/source/codestory"
        );
    }

    #[test]
    fn artifact_scope_fails_closed_when_worktree_becomes_dirty() {
        let Some(project) = git_project() else {
            return;
        };

        let clean = project_identity_v3(project.path());
        fs::write(project.path().join("lib.rs"), "pub fn dirty() {}\n").expect("dirty source");
        let dirty = project_identity_v3(project.path());

        assert_eq!(clean.artifact_scope_id, clean.project_id);
        assert_eq!(dirty.artifact_scope_id, dirty.workspace_id);
        assert_ne!(clean.artifact_scope_id, dirty.artifact_scope_id);
        assert_eq!(dirty.portable_reuse_reason, "git_worktree_dirty");
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
                "https://github.com/TheGreenCedar/CodeStory.git",
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
}
