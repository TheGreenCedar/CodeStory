use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static INDETERMINATE_REPOSITORY_OBSERVATION_ID: AtomicU64 = AtomicU64::new(1);

/// Lossless repository identity hashing contract available for migration.
pub const REPOSITORY_IDENTITY_V2_SCHEMA_VERSION: u32 = 2;

/// Lossless shared project identity contract available for migration.
pub const PROJECT_IDENTITY_V3_SCHEMA_VERSION: u32 = 3;
/// Hashable native identity for one workspace path observation.
///
/// Existing paths use filesystem identity. Missing paths use normalized
/// platform-lexical identity and remain distinct from existing paths. The
/// representation is intentionally private so callers cannot manufacture an
/// identity that did not come from a filesystem observation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspacePathIdentity(WorkspacePathIdentityKind);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum WorkspacePathIdentityKind {
    #[cfg(unix)]
    ExistingUnix { device: u64, inode: u64 },
    #[cfg(windows)]
    ExistingWindows {
        volume_serial_number: u64,
        file_id: [u8; 16],
    },
    #[cfg(unix)]
    MissingUnix(PathBuf),
    #[cfg(windows)]
    MissingWindows(Vec<u16>),
}

/// Opaque native identity for one repository metadata instance.
///
/// A present identity is the pair of native gitdir and common-dir identities;
/// repository contents and mutable Git state never participate. An
/// indeterminate observation is deliberately unique so a failed bounded probe
/// cannot admit reuse.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepositoryInstanceIdentity(RepositoryInstanceIdentityKind);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RepositoryInstanceIdentityKind {
    Absent,
    Present {
        git_dir: WorkspacePathIdentity,
        common_dir: WorkspacePathIdentity,
    },
    Indeterminate(u64),
}

impl RepositoryInstanceIdentity {
    fn absent() -> Self {
        Self(RepositoryInstanceIdentityKind::Absent)
    }

    fn present(git_dir: WorkspacePathIdentity, common_dir: WorkspacePathIdentity) -> Self {
        Self(RepositoryInstanceIdentityKind::Present {
            git_dir,
            common_dir,
        })
    }

    fn indeterminate() -> Self {
        Self(RepositoryInstanceIdentityKind::Indeterminate(
            INDETERMINATE_REPOSITORY_OBSERVATION_ID.fetch_add(1, Ordering::Relaxed),
        ))
    }
}

/// Operation-scoped platform-lexical path spelling for containment checks.
///
/// Unix preserves case. Windows uses the same normalized invariant ordinal
/// ignore-case spelling as missing [`WorkspacePathIdentity`] values.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspacePathLexicalIdentity(WorkspacePathLexicalIdentityKind);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum WorkspacePathLexicalIdentityKind {
    #[cfg(unix)]
    Unix(PathBuf),
    #[cfg(windows)]
    Windows(Vec<u16>),
}

impl WorkspacePathLexicalIdentity {
    /// Whether this candidate is the root itself or one of its descendants.
    pub fn is_within(&self, root: &Self) -> bool {
        match (&self.0, &root.0) {
            #[cfg(unix)]
            (
                WorkspacePathLexicalIdentityKind::Unix(candidate),
                WorkspacePathLexicalIdentityKind::Unix(root),
            ) => candidate == root || candidate.starts_with(root),
            #[cfg(windows)]
            (
                WorkspacePathLexicalIdentityKind::Windows(candidate),
                WorkspacePathLexicalIdentityKind::Windows(root),
            ) => {
                candidate == root
                    || (candidate.starts_with(root)
                        && (root.last() == Some(&u16::from(b'\\'))
                            || candidate.get(root.len()) == Some(&u16::from(b'\\'))))
            }
        }
    }
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

/// Lossless project identity contract for staged schema-3 migrations.
///
/// `project_id` identifies the repository independently of checkout state when
/// a canonical repository identity is available. `workspace_id` always scopes
/// to one canonical root. `artifact_scope_id` fails closed to that workspace
/// whenever portable reuse is not eligible.
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

/// Bounded logical identity used on repeated same-root admissions.
///
/// This intentionally carries only the remote-derived project id and the
/// native-root workspace id. It never observes HEAD, dirtiness, tracked paths,
/// or artifact portability.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LogicalProjectIdentityV3 {
    pub project_id: String,
    pub workspace_id: String,
    pub repository_instance: RepositoryInstanceIdentity,
}

/// Inspect the lossless repository identity without migrating current consumers.
pub fn inspect_repository_identity_v2(project_root: &Path) -> RepositoryIdentityV2 {
    let metadata = crate::read_repository_metadata(project_root);
    let remote = metadata.remote_url;
    let tree = metadata.head_tree;
    let dirty = metadata.dirty;
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

/// Resolve the lossless V3 identity without migrating current consumers.
pub fn project_identity_v3(project_root: &Path) -> ProjectIdentityV3 {
    let repository_identity = inspect_repository_identity_v2(project_root);
    project_identity_v3_from_repository(project_root, &repository_identity)
}

/// Observe logical repository ownership without executing Git or scanning the
/// repository contents. An absent, invalid, or unreadable remote fails closed
/// to the native workspace identity. The irreducible blind spot is an in-place
/// metadata replacement that preserves both native gitdir/common-dir identities
/// and the exact local remote bytes; no bounded filesystem observation can
/// distinguish that event without reading mutable repository state.
pub fn observe_logical_project_identity_v3(project_root: &Path) -> LogicalProjectIdentityV3 {
    let workspace_id = workspace_id_v3_for_root(project_root);
    let (remote_url, repository_instance) =
        match crate::repo_metadata::observe_bounded_repository_identity(project_root) {
            Ok(crate::repo_metadata::BoundedRepositoryIdentityObservation::Absent) => {
                (None, RepositoryInstanceIdentity::absent())
            }
            Ok(crate::repo_metadata::BoundedRepositoryIdentityObservation::Present {
                remote_url,
                git_dir_identity,
                common_dir_identity,
            }) => (
                remote_url,
                RepositoryInstanceIdentity::present(git_dir_identity, common_dir_identity),
            ),
            Err(_) => (None, RepositoryInstanceIdentity::indeterminate()),
        };
    let project_id = remote_url
        .as_deref()
        .and_then(parse_repository_identity_v2)
        .map(|identity| canonical_repository_id_v2(&identity.canonical))
        .unwrap_or_else(|| workspace_id.clone());
    LogicalProjectIdentityV3 {
        project_id,
        workspace_id,
        repository_instance,
    }
}

/// Resolve the lossless V3 identity from an existing Git observation.
pub fn project_identity_v3_from_repository(
    project_root: &Path,
    repository_identity: &RepositoryIdentityV2,
) -> ProjectIdentityV3 {
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
        canonical_repository_id: repository_identity.canonical_repository_id.clone(),
        legacy_canonical_repository_id: repository_identity.legacy_canonical_repository_id.clone(),
        legacy_raw_root_project_id: root_identity.legacy_raw_root_project_id,
        normalized_root_project_id_alias: root_identity.normalized_root_project_id_alias,
        portable_reuse_eligible: repository_identity.portable_reuse_eligible,
        portable_reuse_reason: repository_identity.portable_reuse_reason.clone(),
    }
}

/// Return the schema-3 workspace id hashed from native path data.
pub fn workspace_id_v3_for_root(project_root: &Path) -> String {
    workspace_root_identity_v3(project_root).workspace_id
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceRootIdentity {
    workspace_id: String,
    legacy_raw_root_project_id: Option<String>,
    normalized_root_project_id_alias: Option<String>,
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

fn versioned_repository_id(schema_version: u32, normalized_repository_identity: &str) -> String {
    let mut state = 0xcbf29ce484222325_u64;
    mix_str(&mut state, "codestory-repository-identity");
    mix_u32(&mut state, schema_version);
    mix_str(&mut state, normalized_repository_identity);
    format!("repo-v{schema_version}-{state:016x}")
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
    if let Some((authority, path)) = split_scp_host_path(value) {
        let host = strip_userinfo(authority);
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
    if let Some(bracket_start) = value.find('[') {
        let end = value[bracket_start..].find(']')? + bracket_start;
        let authority = &value[..=end];
        let path = value[end + 1..].strip_prefix(':')?;
        return (!path.is_empty()).then_some((authority, path));
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
    let (authority, path) = rest.split_once('/')?;
    let authority = strip_userinfo(authority);
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

/// Observe one path using native filesystem identity when it exists.
///
/// Existing Unix paths use device/inode identity and existing Windows paths
/// use volume/file identity. Missing Unix paths preserve normalized spelling;
/// missing Windows paths use normalized invariant ordinal ignore-case spelling.
/// A missing path cannot reveal a future Windows directory's case-sensitivity
/// flag, so callers must not retain this observation beyond their operation.
///
/// Only `NotFound` enters the lexical missing-path contract. Permission,
/// malformed-path, handle, and platform failures remain errors so callers can
/// downgrade completeness instead of guessing.
pub fn workspace_path_identity(path: &Path) -> io::Result<WorkspacePathIdentity> {
    match fs::metadata(path) {
        Ok(metadata) => existing_workspace_path_identity(path, &metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            missing_workspace_path_identity(path)
        }
        Err(error) => Err(error),
    }
}

/// Normalize one path for operation-scoped native lexical containment.
pub fn workspace_path_lexical_identity(path: &Path) -> io::Result<WorkspacePathLexicalIdentity> {
    #[cfg(unix)]
    {
        Ok(WorkspacePathLexicalIdentity(
            WorkspacePathLexicalIdentityKind::Unix(normalize_missing_unix_path(path)),
        ))
    }
    #[cfg(windows)]
    {
        let normalized = normalize_windows_lexical_path(path);
        Ok(WorkspacePathLexicalIdentity(
            WorkspacePathLexicalIdentityKind::Windows(windows_ordinal_case_fold(&normalized)?),
        ))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "lexical workspace path identity is unsupported on this platform",
        ))
    }
}

/// Durable text form of an existing path's native filesystem identity.
///
/// Unlike [`workspace_path_identity`], this observation is meant to be written
/// to a registry and compared across processes, so only existing paths produce
/// a token: a missing path's lexical spelling is not an identity and would let
/// a copy inherit the original's binding. A same-filesystem rename keeps the
/// token; a clone or a cross-volume copy does not.
pub fn workspace_path_identity_token(path: &Path) -> io::Result<Option<String>> {
    match fs::metadata(path) {
        Ok(metadata) => existing_workspace_path_identity_token(path, &metadata).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn existing_workspace_path_identity_token(
    _path: &Path,
    metadata: &fs::Metadata,
) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn existing_workspace_path_identity_token(
    path: &Path,
    _metadata: &fs::Metadata,
) -> io::Result<String> {
    let (volume_serial_number, file_id) = windows_file_identity(path)?;
    let mut rendered = String::with_capacity(file_id.len() * 2);
    for byte in file_id {
        rendered.push_str(&format!("{byte:02x}"));
    }
    Ok(format!("windows:{volume_serial_number}:{rendered}"))
}

#[cfg(not(any(unix, windows)))]
fn existing_workspace_path_identity_token(
    _path: &Path,
    _metadata: &fs::Metadata,
) -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native workspace path identity is unsupported on this platform",
    ))
}

/// Observe one already-open file using native filesystem identity.
pub fn workspace_file_identity(file: &fs::File) -> io::Result<WorkspacePathIdentity> {
    let metadata = file.metadata()?;
    existing_workspace_file_identity(file, &metadata)
}

/// Compare workspace paths through [`workspace_path_identity`].
///
/// The compatibility boolean fails closed when either identity is unavailable.
pub fn same_workspace_path(left: &Path, right: &Path) -> bool {
    match (
        workspace_path_identity(left),
        workspace_path_identity(right),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(unix)]
fn existing_workspace_path_identity(
    _path: &Path,
    metadata: &fs::Metadata,
) -> io::Result<WorkspacePathIdentity> {
    use std::os::unix::fs::MetadataExt;
    Ok(WorkspacePathIdentity(
        WorkspacePathIdentityKind::ExistingUnix {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
    ))
}

#[cfg(unix)]
fn existing_workspace_file_identity(
    _file: &fs::File,
    metadata: &fs::Metadata,
) -> io::Result<WorkspacePathIdentity> {
    existing_workspace_path_identity(Path::new(""), metadata)
}

#[cfg(windows)]
fn existing_workspace_path_identity(
    path: &Path,
    _metadata: &fs::Metadata,
) -> io::Result<WorkspacePathIdentity> {
    let (volume_serial_number, file_id) = windows_file_identity(path)?;
    Ok(WorkspacePathIdentity(
        WorkspacePathIdentityKind::ExistingWindows {
            volume_serial_number,
            file_id,
        },
    ))
}

#[cfg(windows)]
fn existing_workspace_file_identity(
    file: &fs::File,
    _metadata: &fs::Metadata,
) -> io::Result<WorkspacePathIdentity> {
    let (volume_serial_number, file_id) = windows_file_handle_identity(file)?;
    Ok(WorkspacePathIdentity(
        WorkspacePathIdentityKind::ExistingWindows {
            volume_serial_number,
            file_id,
        },
    ))
}

#[cfg(not(any(unix, windows)))]
fn existing_workspace_path_identity(
    _path: &Path,
    _metadata: &fs::Metadata,
) -> io::Result<WorkspacePathIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native workspace path identity is unsupported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn existing_workspace_file_identity(
    _file: &fs::File,
    _metadata: &fs::Metadata,
) -> io::Result<WorkspacePathIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native workspace file identity is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn missing_workspace_path_identity(path: &Path) -> io::Result<WorkspacePathIdentity> {
    Ok(WorkspacePathIdentity(
        WorkspacePathIdentityKind::MissingUnix(normalize_missing_unix_path(path)),
    ))
}

#[cfg(unix)]
fn normalize_missing_unix_path(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized
                    .file_name()
                    .is_some_and(|name| name != std::ffi::OsStr::new(".."))
                {
                    normalized.pop();
                } else if !normalized.has_root() {
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

#[cfg(windows)]
fn missing_workspace_path_identity(path: &Path) -> io::Result<WorkspacePathIdentity> {
    let normalized = normalize_windows_lexical_path(path);
    let folded = windows_ordinal_case_fold(&normalized)?;
    Ok(WorkspacePathIdentity(
        WorkspacePathIdentityKind::MissingWindows(folded),
    ))
}

#[cfg(not(any(unix, windows)))]
fn missing_workspace_path_identity(_path: &Path) -> io::Result<WorkspacePathIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "lexical workspace path identity is unsupported on this platform",
    ))
}

#[cfg(windows)]
fn windows_file_identity(path: &Path) -> io::Result<(u64, [u8; 16])> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let file = fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    windows_file_handle_identity(&file)
}

#[cfg(windows)]
fn windows_file_handle_identity(file: &fs::File) -> io::Result<(u64, [u8; 16])> {
    use std::ffi::c_void;
    use std::mem::MaybeUninit;
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
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful `GetFileInformationByHandleEx` initializes all fields.
    let information = unsafe { information.assume_init() };
    Ok((information.volume_serial_number, information.file_id))
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
            .is_some_and(|prefix| windows_ascii_case_eq(prefix, &unc))
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
fn windows_ascii_case_eq(left: &[u16], right: &[u16]) -> bool {
    fn uppercase(unit: u16) -> u16 {
        if (u16::from(b'a')..=u16::from(b'z')).contains(&unit) {
            unit - u16::from(b'a' - b'A')
        } else {
            unit
        }
    }

    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| uppercase(*left) == uppercase(*right))
}

#[cfg(windows)]
fn windows_ordinal_case_fold(source: &[u16]) -> io::Result<Vec<u16>> {
    use std::ptr;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LCMapStringEx(
            locale_name: *const u16,
            map_flags: u32,
            source: *const u16,
            source_len: i32,
            destination: *mut u16,
            destination_len: i32,
            version_information: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
            sort_handle: isize,
        ) -> i32;
    }

    if source.is_empty() {
        return Ok(Vec::new());
    }
    let source_len = i32::try_from(source.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace path exceeds the Windows identity length bound",
        )
    })?;
    const LCMAP_UPPERCASE: u32 = 0x0000_0200;
    let invariant_locale = [0_u16];
    // SAFETY: `source` remains valid for `source_len`, and null output asks
    // Windows for the required invariant-uppercase buffer size. The invariant
    // uppercase table is the same language-independent table used by ordinal
    // ignore-case comparison.
    let required = unsafe {
        LCMapStringEx(
            invariant_locale.as_ptr(),
            LCMAP_UPPERCASE,
            source.as_ptr(),
            source_len,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        )
    };
    if required <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mut folded = vec![0_u16; required as usize];
    // SAFETY: `folded` has the exact capacity reported by the query above.
    let written = unsafe {
        LCMapStringEx(
            invariant_locale.as_ptr(),
            LCMAP_UPPERCASE,
            source.as_ptr(),
            source_len,
            folded.as_mut_ptr(),
            required,
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        )
    };
    if written <= 0 {
        return Err(io::Error::last_os_error());
    }
    folded.truncate(written as usize);
    Ok(folded)
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
    use std::process::Command;
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
        assert_eq!(
            normalize_repository_identity_v2("https://user@example.com/team/repo@release.git")
                .as_deref(),
            Some("https://example.com:443/team/repo@release")
        );
        assert_eq!(
            normalize_repository_identity_v2("example.com:team/repo@release.git").as_deref(),
            Some("ssh://example.com:22/~/team/repo@release")
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
        assert_eq!(REPOSITORY_IDENTITY_V2_SCHEMA_VERSION, 2);
        assert_eq!(PROJECT_IDENTITY_V3_SCHEMA_VERSION, 3);
        let normalized = "example.com/team/repo";
        // Persisted artifacts remain keyed by schema-1 ids, so the current
        // contract must never collide with that retired derivation.
        assert_ne!(
            canonical_repository_id_v2(normalized),
            versioned_repository_id(1, normalized)
        );
        assert!(canonical_repository_id_v2(normalized).starts_with("repo-v2-"));
        assert!(versioned_repository_id(1, normalized).starts_with("repo-v1-"));
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
    fn bounded_logical_identity_is_stable_for_a_missing_project_root() {
        let parent = tempdir().expect("project parent");
        let project = parent.path().join("missing-project");

        let first = observe_logical_project_identity_v3(&project);
        let second = observe_logical_project_identity_v3(&project);

        assert!(!project.exists());
        assert_eq!(first, second);
        assert_eq!(first.project_id, first.workspace_id);
        assert!(matches!(
            &first.repository_instance.0,
            RepositoryInstanceIdentityKind::Absent
        ));
    }

    #[test]
    fn bounded_logical_identity_fails_closed_when_a_missing_root_appears() {
        let parent = tempdir().expect("project parent");
        let project = parent.path().join("appearing-project");
        let appearing_project = project.clone();
        crate::repo_metadata::set_after_bounded_identity_capture_hook_for_test(move || {
            fs::create_dir(&appearing_project).expect("create project after bounded capture");
        });

        let raced = observe_logical_project_identity_v3(&project);
        let stable = observe_logical_project_identity_v3(&project);
        let converged = observe_logical_project_identity_v3(&project);

        assert_eq!(raced.project_id, raced.workspace_id);
        // The workspace id hashes the raw path while the root is missing and
        // the canonical path once it exists, so the two ids coincide whenever
        // the temp parent is already canonical (Linux /tmp) and differ only
        // behind a symlinked parent (macOS /var). The fail-closed guarantee is
        // the per-observation-unique indeterminate instance below, not the
        // workspace id.
        assert!(matches!(
            &raced.repository_instance.0,
            RepositoryInstanceIdentityKind::Indeterminate(_)
        ));
        assert!(matches!(
            &stable.repository_instance.0,
            RepositoryInstanceIdentityKind::Absent
        ));
        assert_ne!(raced.repository_instance, stable.repository_instance);
        assert_eq!(stable, converged);
    }

    #[test]
    fn bounded_logical_identity_never_walks_metadata_and_ignores_dirty_or_commit_state() {
        let Some(project) = git_project() else {
            return;
        };

        let ((clean, dirty, committed, remote_b), traversals) =
            crate::with_repository_metadata_tree_traversal_count_for_test(|| {
                let clean = observe_logical_project_identity_v3(project.path());
                fs::write(project.path().join("lib.rs"), "pub fn dirty() {}\n")
                    .expect("dirty source");
                let dirty = observe_logical_project_identity_v3(project.path());
                git(project.path(), &["add", "lib.rs"]);
                git(project.path(), &["commit", "-m", "bounded identity commit"]);
                let committed = observe_logical_project_identity_v3(project.path());
                git(
                    project.path(),
                    &[
                        "remote",
                        "set-url",
                        "origin",
                        "https://example.com/team/repository-b.git",
                    ],
                );
                let remote_b = observe_logical_project_identity_v3(project.path());
                (clean, dirty, committed, remote_b)
            });

        assert_eq!(traversals, 0, "bounded admission walked Git metadata");
        assert_eq!(clean.project_id, dirty.project_id);
        assert_eq!(dirty.project_id, committed.project_id);
        assert_ne!(committed.project_id, remote_b.project_id);
        assert_eq!(clean.repository_instance, dirty.repository_instance);
        assert_eq!(dirty.repository_instance, committed.repository_instance);
        assert_eq!(committed.repository_instance, remote_b.repository_instance);
        let (_, full_observation_traversals) =
            crate::with_repository_metadata_tree_traversal_count_for_test(|| {
                crate::read_repository_metadata(project.path())
            });
        assert!(
            full_observation_traversals > 0,
            "the zero-traversal assertion needs an armed positive control"
        );
    }

    #[test]
    fn bounded_logical_identity_tracks_no_remote_metadata_recreation() {
        let Some(project) = git_project() else {
            return;
        };

        let remote = observe_logical_project_identity_v3(project.path());
        fs::rename(
            project.path().join(".git"),
            project.path().join(".git-retired"),
        )
        .expect("retire original repository metadata");
        git(project.path(), &["init"]);
        let no_remote = observe_logical_project_identity_v3(project.path());

        assert_eq!(remote.workspace_id, no_remote.workspace_id);
        assert_ne!(remote.project_id, no_remote.project_id);
        assert_eq!(no_remote.project_id, no_remote.workspace_id);
        assert_ne!(remote.repository_instance, no_remote.repository_instance);
    }

    #[test]
    fn bounded_logical_identity_tracks_same_remote_metadata_recreation() {
        let Some(project) = git_project() else {
            return;
        };

        let original = observe_logical_project_identity_v3(project.path());
        fs::rename(
            project.path().join(".git"),
            project.path().join(".git-retired"),
        )
        .expect("retire original repository metadata");
        git(project.path(), &["init"]);
        git(
            project.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/TheGreenCedar/CodeStory.git",
            ],
        );
        let recreated = observe_logical_project_identity_v3(project.path());

        assert_eq!(original.project_id, recreated.project_id);
        assert_eq!(original.workspace_id, recreated.workspace_id);
        assert_ne!(original.repository_instance, recreated.repository_instance);
    }

    #[test]
    fn bounded_logical_identity_fails_closed_on_local_config_toctou() {
        let Some(project) = git_project() else {
            return;
        };

        let original = observe_logical_project_identity_v3(project.path());
        let config = project.path().join(".git/config");
        crate::repo_metadata::set_after_bounded_identity_capture_hook_for_test(move || {
            let contents = fs::read_to_string(&config).expect("read captured local config");
            fs::write(
                &config,
                contents.replace(
                    "https://github.com/TheGreenCedar/CodeStory.git",
                    "https://example.com/team/repository-b.git",
                ),
            )
            .expect("replace local config after bounded capture");
        });
        let raced = observe_logical_project_identity_v3(project.path());
        let stable_replacement = observe_logical_project_identity_v3(project.path());

        assert_eq!(raced.project_id, raced.workspace_id);
        assert_ne!(raced.repository_instance, original.repository_instance);
        assert_ne!(
            raced.repository_instance,
            stable_replacement.repository_instance
        );
        assert_ne!(original.project_id, stable_replacement.project_id);
    }

    #[test]
    fn bounded_logical_identity_fails_closed_on_native_metadata_directory_replacement() {
        let Some(project) = git_project() else {
            return;
        };

        let original = observe_logical_project_identity_v3(project.path());
        let git_dir = project.path().join(".git");
        let retired_git_dir = project.path().join(".git-captured");
        crate::repo_metadata::set_after_bounded_identity_capture_hook_for_test(move || {
            fs::rename(&git_dir, &retired_git_dir)
                .expect("retire captured repository metadata directory");
            fs::create_dir(&git_dir).expect("create replacement repository metadata directory");
            fs::copy(retired_git_dir.join("config"), git_dir.join("config"))
                .expect("copy identical local config into replacement metadata directory");
        });
        let raced = observe_logical_project_identity_v3(project.path());
        let stable_replacement = observe_logical_project_identity_v3(project.path());

        assert_eq!(raced.project_id, raced.workspace_id);
        assert_eq!(original.project_id, stable_replacement.project_id);
        assert_ne!(
            original.repository_instance,
            stable_replacement.repository_instance
        );
        assert_ne!(
            raced.repository_instance,
            stable_replacement.repository_instance
        );
    }

    #[test]
    fn bounded_logical_identity_fails_closed_on_gitdir_pointer_toctou() {
        let Some(project) = git_project() else {
            return;
        };
        let Some(alternate) = git_project() else {
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
        let original = observe_logical_project_identity_v3(&worktree);
        let pointer = worktree.join(".git");
        let alternate_git_dir = alternate.path().join(".git");
        crate::repo_metadata::set_after_bounded_identity_capture_hook_for_test(move || {
            fs::write(
                &pointer,
                format!("gitdir: {}\n", alternate_git_dir.display()),
            )
            .expect("replace gitdir pointer after bounded capture");
        });
        let raced = observe_logical_project_identity_v3(&worktree);

        assert_eq!(raced.project_id, raced.workspace_id);
        assert_ne!(raced.repository_instance, original.repository_instance);
    }

    #[test]
    fn a_durable_identity_token_survives_a_rename_but_not_a_copy() {
        let parent = tempdir().expect("token parent");
        let original = parent.path().join("root");
        fs::create_dir(&original).expect("create root");
        let before = workspace_path_identity_token(&original)
            .expect("observe root")
            .expect("an existing root has a durable identity");

        let renamed = parent.path().join("renamed-root");
        fs::rename(&original, &renamed).expect("rename root");
        assert_eq!(
            workspace_path_identity_token(&renamed)
                .expect("observe renamed root")
                .as_deref(),
            Some(before.as_str()),
            "a same-filesystem rename keeps the durable identity"
        );

        let copy = parent.path().join("copied-root");
        fs::create_dir(&copy).expect("create copy");
        assert_ne!(
            workspace_path_identity_token(&copy)
                .expect("observe copied root")
                .as_deref(),
            Some(before.as_str()),
            "a separate directory must not inherit the original identity"
        );

        assert_eq!(
            workspace_path_identity_token(&parent.path().join("missing")).expect("observe missing"),
            None,
            "a missing path has no durable identity to bind to"
        );
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

        assert_eq!(
            workspace_path_identity(&file).expect("file identity"),
            workspace_path_identity(&alias).expect("alias identity")
        );
        assert!(same_workspace_path(&file, &alias));
        assert!(!same_workspace_path(&file, &project.path().join("missing")));
    }

    #[test]
    fn open_file_identity_matches_path_observation() {
        let project = tempdir().expect("project");
        let path = project.path().join("identity");
        fs::write(&path, "identity").expect("identity file");
        let file = fs::File::open(&path).expect("open identity file");

        assert_eq!(
            workspace_file_identity(&file).expect("open file identity"),
            workspace_path_identity(&path).expect("path identity")
        );
    }

    #[test]
    fn existing_and_missing_path_identities_are_distinct() {
        let project = tempdir().expect("project");
        let existing = project.path().join("entry");
        fs::write(&existing, "entry").expect("write entry");
        let missing = project.path().join("missing");

        assert_ne!(
            workspace_path_identity(&existing).expect("existing identity"),
            workspace_path_identity(&missing).expect("missing identity")
        );
    }

    #[test]
    fn unavailable_path_identity_is_an_error() {
        let malformed = Path::new("identity\0unavailable");
        assert!(workspace_path_identity(malformed).is_err());
        assert!(!same_workspace_path(malformed, malformed));
    }

    #[cfg(unix)]
    #[test]
    fn unix_paths_preserve_filesystem_case_identity_and_non_utf8_bytes() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let project = tempdir().expect("project");
        let upper = project.path().join("Repo");
        let lower = project.path().join("repo");
        fs::create_dir(&upper).expect("upper-case path");
        match fs::create_dir(&lower) {
            Ok(()) => {
                assert!(!same_workspace_path(&upper, &lower));
                assert_eq!(
                    crate::workspace_relative_path(&upper, &lower.join("src/lib.rs")),
                    None,
                    "case-distinct Unix roots must not be stripped as aliases"
                );
                assert_ne!(
                    workspace_id_v3_for_root(&upper),
                    workspace_id_v3_for_root(&lower)
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                assert!(
                    same_workspace_path(&upper, &lower),
                    "case-insensitive filesystems must identify case aliases as the same path"
                );
                assert_eq!(
                    crate::workspace_relative_path(&upper, &lower.join("src/lib.rs")),
                    Some(PathBuf::from("src/lib.rs"))
                );
            }
            Err(error) => panic!("lower-case path: {error}"),
        }

        let missing = project.path().join("Missing").join(".").join("child");
        let normalized = project.path().join("Missing").join("child");
        assert_eq!(
            workspace_path_identity(&missing).expect("missing dotted identity"),
            workspace_path_identity(&normalized).expect("missing normalized identity")
        );
        assert_ne!(
            workspace_path_identity(&project.path().join("Missing"))
                .expect("missing upper identity"),
            workspace_path_identity(&project.path().join("missing"))
                .expect("missing lower identity")
        );

        let first = project.path().join(OsString::from_vec(vec![b'r', 0x80]));
        let second = project.path().join(OsString::from_vec(vec![b'r', 0x81]));
        match fs::create_dir(&first) {
            Ok(()) => {}
            // Darwin filesystems and sandbox policies can reject non-UTF-8
            // path components with EILSEQ (92) or EPERM (1), so only exercise
            // byte-distinct identities where those names can be materialized.
            Err(error)
                if cfg!(target_os = "macos")
                    && matches!(error.raw_os_error(), Some(1) | Some(92)) =>
            {
                return;
            }
            Err(error) => panic!("first non-UTF-8 path: {error}"),
        }
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
        assert_eq!(
            workspace_path_identity(&existing).expect("existing identity"),
            workspace_path_identity(&existing_alias).expect("existing alias identity")
        );
        assert!(same_workspace_path(&existing, &existing_alias));
        assert_eq!(
            crate::workspace_relative_path(&existing, &existing_alias.join("src/lib.rs")),
            Some(PathBuf::from("src/lib.rs"))
        );

        let missing = project.path().join("Missing");
        let missing_alias = project.path().join("missing");
        assert_eq!(
            workspace_path_identity(&missing).expect("missing identity"),
            workspace_path_identity(&missing_alias).expect("missing alias identity")
        );
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
        assert_eq!(
            workspace_path_identity(&extended.join("missing")).expect("extended missing identity"),
            workspace_path_identity(&project.path().join("MISSING"))
                .expect("ordinary missing identity")
        );
        assert!(same_workspace_path(
            &extended.join("missing"),
            &project.path().join("MISSING")
        ));
        assert_ne!(
            workspace_path_identity(Path::new(r"C:missing")).expect("drive-relative identity"),
            workspace_path_identity(Path::new(r"C:\missing")).expect("drive-rooted identity")
        );
    }

    #[test]
    fn windows_normalization_is_an_alias_not_the_workspace_id() {
        let canonical_id = fnv1a_hex(r"\\?\C:\Source\CodeStory\".as_bytes());
        let normalized_alias = fnv1a_hex("c:/source/codestory".as_bytes());

        assert_eq!(
            normalize_root_identity_text(r"\\?\C:\Source\CodeStory\"),
            "c:/source/codestory"
        );
        assert_eq!(
            fnv_alias(
                &normalize_root_identity_text(r"\\?\C:\Source\CodeStory\"),
                &canonical_id
            )
            .as_deref(),
            Some(normalized_alias.as_str())
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

    #[test]
    #[cfg(unix)]
    fn hostile_fsmonitor_hook_never_executes_during_identity_inspection() {
        use std::os::unix::fs::PermissionsExt;

        let Some(project) = git_project() else {
            return;
        };

        let marker = project.path().join("fsmonitor-executed");
        let hook = project.path().join("fsmonitor-hook.sh");
        fs::write(
            &hook,
            format!("#!/bin/sh\ntouch \"{}\"\n", marker.display()),
        )
        .expect("write hook");
        let mut permissions = fs::metadata(&hook).expect("hook metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).expect("make hook executable");
        git(
            project.path(),
            &["config", "core.fsmonitor", &hook.display().to_string()],
        );

        // Prove the sentinel is live first: an unconstrained `git status` in
        // this repository executes the configured hook. Without that, a git
        // version that ignores `core.fsmonitor` would make the containment
        // assertion below pass vacuously.
        let unconstrained = Command::new("git")
            .arg("-C")
            .arg(project.path())
            .args(["status", "--porcelain"])
            .output()
            .expect("run unconstrained git status");
        assert!(unconstrained.status.success());
        assert!(
            marker.exists(),
            "unconstrained git status did not execute the configured core.fsmonitor hook"
        );
        fs::remove_file(&marker).expect("reset sentinel");

        let identity = project_identity_v3(project.path());
        assert!(!identity.project_id.is_empty());
        assert!(
            !marker.exists(),
            "core.fsmonitor hook executed during repository identity inspection"
        );
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
