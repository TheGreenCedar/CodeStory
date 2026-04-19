use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Language {
    Cxx,
    Java,
    Python,
    Rust,
    JavaScript,
    TypeScript,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LanguageStandard {
    Default,
    Cxx11,
    Cxx14,
    Cxx17,
    Cxx20,
    Java8,
    Java11,
    Java17,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    pub name: String,
    pub version: u32,
    pub source_groups: Vec<SourceGroupSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceGroupSettings {
    pub id: Uuid,
    pub language: Language,
    pub standard: LanguageStandard,
    pub source_paths: Vec<PathBuf>,
    pub exclude_patterns: Vec<String>,
    pub include_paths: Vec<PathBuf>,
    pub defines: HashMap<String, String>,
    pub language_specific: LanguageSpecificSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LanguageSpecificSettings {
    Cxx {
        cdb_path: Option<PathBuf>,
        header_paths: Vec<PathBuf>,
        precompiled_header: Option<PathBuf>,
    },
    Java {
        classpath: Vec<PathBuf>,
        maven_path: Option<PathBuf>,
        gradle_path: Option<PathBuf>,
    },
    Python {
        python_path: Option<PathBuf>,
        virtual_env: Option<PathBuf>,
    },
    Other,
}

#[derive(Debug, Clone)]
pub struct WorkspaceManifest {
    settings: WorkspaceSettings,
    manifest_path: PathBuf,
    is_synthetic_default: Cell<bool>,
    members: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceMembersManifest {
    pub members: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RefreshMode {
    Incremental,
    FullRefresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFileState {
    pub id: i64,
    pub path: PathBuf,
    pub modification_time: i64,
    pub indexed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshInputs {
    pub stored_files: Vec<StoredFileState>,
    pub inventory: WorkspaceInventory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFileRecord {
    pub file_id: i64,
    pub modification_time: i64,
    pub indexed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceInventory {
    files: HashMap<PathBuf, IndexedFileRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshPlan {
    pub mode: RefreshMode,
    pub files_to_index: Vec<PathBuf>,
    pub files_to_remove: Vec<i64>,
    pub existing_file_ids: HashMap<PathBuf, i64>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkspaceDiscovery;

impl WorkspaceManifest {
    pub fn from_parts(settings: WorkspaceSettings, manifest_path: PathBuf) -> Self {
        Self {
            settings,
            manifest_path,
            is_synthetic_default: Cell::new(false),
            members: Vec::new(),
        }
    }

    pub fn load(path: PathBuf) -> Result<Self> {
        let content = fs::read_to_string(&path)?;
        let settings: WorkspaceSettings = serde_json::from_str(&content)?;
        Ok(Self {
            settings,
            manifest_path: path,
            is_synthetic_default: Cell::new(false),
            members: Vec::new(),
        })
    }

    pub fn save(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(&self.settings)?;
        fs::write(&self.manifest_path, content)?;
        self.is_synthetic_default.set(false);
        Ok(())
    }

    pub fn new(name: String, manifest_path: PathBuf) -> Self {
        Self {
            settings: WorkspaceSettings {
                name,
                version: 1,
                source_groups: Vec::new(),
            },
            manifest_path,
            is_synthetic_default: Cell::new(false),
            members: Vec::new(),
        }
    }

    pub fn open(root_path: PathBuf) -> Result<Self> {
        let workspace_file = root_path.join("codestory_workspace.json");
        if workspace_file.exists() {
            return Self::load_workspace_members(root_path, workspace_file);
        }

        let project_file = root_path.join("codestory_project.json");
        if project_file.exists() {
            Self::load(project_file)
        } else {
            let mut manifest = Self::new(
                root_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Project")
                    .to_string(),
                project_file,
            );
            manifest.settings.source_groups.push(SourceGroupSettings {
                id: Uuid::new_v4(),
                language: Language::Rust,
                standard: LanguageStandard::Default,
                source_paths: vec![root_path],
                exclude_patterns: vec![
                    "**/node_modules/**".to_string(),
                    "**/target/**".to_string(),
                    "**/.git/**".to_string(),
                    "**/dist/**".to_string(),
                    "**/build/**".to_string(),
                ],
                include_paths: Vec::new(),
                defines: HashMap::new(),
                language_specific: LanguageSpecificSettings::Other,
            });
            manifest.is_synthetic_default.set(true);
            Ok(manifest)
        }
    }

    fn load_workspace_members(root_path: PathBuf, manifest_path: PathBuf) -> Result<Self> {
        let content = fs::read_to_string(&manifest_path)?;
        let workspace: WorkspaceMembersManifest = serde_json::from_str(&content)?;
        let mut manifest = Self::new(
            root_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Workspace")
                .to_string(),
            manifest_path,
        );
        manifest.members = workspace.members.clone();
        manifest.is_synthetic_default.set(true);
        manifest.settings.source_groups = workspace
            .members
            .iter()
            .map(|member| SourceGroupSettings {
                id: Uuid::new_v4(),
                language: Language::Rust,
                standard: LanguageStandard::Default,
                source_paths: vec![member.clone()],
                exclude_patterns: vec![
                    "**/node_modules/**".to_string(),
                    "**/target/**".to_string(),
                    "**/.git/**".to_string(),
                    "**/dist/**".to_string(),
                    "**/build/**".to_string(),
                ],
                include_paths: Vec::new(),
                defines: HashMap::new(),
                language_specific: LanguageSpecificSettings::Other,
            })
            .collect();
        Ok(manifest)
    }

    pub fn settings(&self) -> &WorkspaceSettings {
        &self.settings
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn root_dir(&self) -> PathBuf {
        self.manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    }

    pub fn members(&self) -> &[PathBuf] {
        &self.members
    }

    fn should_filter_source_group_language(&self) -> bool {
        !self.is_synthetic_default.get()
    }

    pub fn source_files(&self) -> Result<Vec<PathBuf>> {
        WorkspaceDiscovery.source_files(self)
    }

    pub fn full_refresh_plan(&self) -> Result<RefreshPlan> {
        Ok(RefreshPlan {
            mode: RefreshMode::FullRefresh,
            files_to_index: self.source_files()?,
            files_to_remove: Vec::new(),
            existing_file_ids: HashMap::new(),
        })
    }

    pub fn full_refresh_execution_plan(&self) -> Result<RefreshPlan> {
        self.full_refresh_plan()
    }

    pub fn build_execution_plan(&self, inputs: &RefreshInputs) -> Result<RefreshPlan> {
        WorkspaceDiscovery.build_refresh_plan(self, inputs)
    }
}

impl WorkspaceDiscovery {
    pub fn source_files(&self, manifest: &WorkspaceManifest) -> Result<Vec<PathBuf>> {
        let workspace_root = workspace_root(manifest);
        let mut all_files = Vec::new();
        let mut seen = HashSet::new();

        for group in &manifest.settings.source_groups {
            let exclude_patterns = compile_exclude_patterns(&group.exclude_patterns)?;
            let filter_by_language = manifest.should_filter_source_group_language();
            for source_path in &group.source_paths {
                let full_path = if source_path.is_absolute() {
                    source_path.clone()
                } else {
                    manifest.root_dir().join(source_path)
                };
                let full_path = normalize_lexical_path(&full_path);
                let source_root = discovery_root(&full_path);

                if full_path.is_file() {
                    if should_include_discovered_path(
                        &full_path,
                        false,
                        &workspace_root,
                        &source_root,
                        filter_by_language,
                        &group.language,
                        &exclude_patterns,
                    ) {
                        push_discovered_file(&mut all_files, &mut seen, full_path, &workspace_root);
                    }
                } else if full_path.is_dir() {
                    let mut builder = ignore::WalkBuilder::new(&full_path);
                    builder.follow_links(true);
                    builder.require_git(false);
                    let workspace_root_for_filter = workspace_root.clone();
                    let source_root_for_filter = source_root.clone();
                    let exclude_patterns = exclude_patterns.clone();
                    let language = group.language.clone();
                    builder.filter_entry(move |entry| {
                        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
                        should_include_discovered_path(
                            entry.path(),
                            is_dir,
                            &workspace_root_for_filter,
                            &source_root_for_filter,
                            filter_by_language,
                            &language,
                            &exclude_patterns,
                        )
                    });
                    for entry in builder.build().filter_map(|e| e.ok()) {
                        if entry.file_type().is_some_and(|kind| kind.is_file()) {
                            push_discovered_file(
                                &mut all_files,
                                &mut seen,
                                entry.into_path(),
                                &workspace_root,
                            );
                        }
                    }
                }
            }
        }

        all_files.sort();
        Ok(all_files)
    }

    pub fn build_refresh_plan(
        &self,
        manifest: &WorkspaceManifest,
        inputs: &RefreshInputs,
    ) -> Result<RefreshPlan> {
        let current_files = self.source_files(manifest)?;
        let workspace_root = manifest.root_dir();
        let stored_map = inputs.inventory_map();
        let normalized_stored_map = stored_map
            .into_values()
            .map(|file| (normalized_compare_key(&workspace_root, &file.path), file))
            .collect::<HashMap<_, _>>();

        let mut files_to_index = Vec::new();
        let mut files_to_remove = Vec::new();
        let mut existing_file_ids = HashMap::new();
        let mut current_file_keys = HashSet::with_capacity(current_files.len());

        for path in current_files {
            let normalized_key = normalized_compare_key(&workspace_root, &path);
            current_file_keys.insert(normalized_key.clone());
            let needs_index = match normalized_stored_map.get(&normalized_key) {
                Some(file) => {
                    existing_file_ids.insert(path.clone(), file.id);
                    match modification_time_millis(&path) {
                        Ok(mtime) => mtime != file.modification_time || !file.indexed,
                        Err(_) => true,
                    }
                }
                None => true,
            };

            if needs_index {
                files_to_index.push(path);
            }
        }

        for (normalized_key, stored) in normalized_stored_map {
            if !current_file_keys.contains(&normalized_key) {
                files_to_remove.push(stored.id);
            }
        }
        files_to_remove.sort_unstable();
        files_to_remove.dedup();

        Ok(RefreshPlan {
            mode: RefreshMode::Incremental,
            files_to_index,
            files_to_remove,
            existing_file_ids,
        })
    }
}

impl RefreshInputs {
    pub fn inventory_map(&self) -> HashMap<PathBuf, StoredFileState> {
        if !self.stored_files.is_empty() {
            return self
                .stored_files
                .iter()
                .cloned()
                .map(|file| (file.path.clone(), file))
                .collect();
        }

        self.inventory
            .files
            .clone()
            .into_iter()
            .map(|(path, record)| {
                (
                    path.clone(),
                    StoredFileState {
                        id: record.file_id,
                        path,
                        modification_time: record.modification_time,
                        indexed: record.indexed,
                    },
                )
            })
            .collect()
    }
}

impl WorkspaceInventory {
    pub fn from_records<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = (PathBuf, IndexedFileRecord)>,
    {
        Self {
            files: iter.into_iter().collect(),
        }
    }
}

fn modification_time_millis(path: &Path) -> Result<i64> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?;
    let duration = modified.duration_since(std::time::UNIX_EPOCH)?;
    Ok(duration.as_millis().min(i64::MAX as u128) as i64)
}

fn workspace_root(manifest: &WorkspaceManifest) -> PathBuf {
    manifest
        .root_dir()
        .canonicalize()
        .unwrap_or_else(|_| normalize_lexical_path(&manifest.root_dir()))
}

fn compile_exclude_patterns(patterns: &[String]) -> Result<Vec<glob::Pattern>> {
    patterns
        .iter()
        .map(|pattern| glob::Pattern::new(pattern).map_err(anyhow::Error::from))
        .collect()
}

fn discovery_root(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| normalize_lexical_path(path))
}

fn should_include_discovered_path(
    path: &Path,
    is_dir: bool,
    workspace_root: &Path,
    source_root: &Path,
    filter_by_language: bool,
    language: &Language,
    exclude_patterns: &[glob::Pattern],
) -> bool {
    let normalized = normalize_lexical_path(path);
    if let Ok(canonical) = normalized.canonicalize()
        && !canonical.starts_with(source_root)
    {
        return false;
    }
    if is_excluded_path(&normalized, workspace_root, source_root, exclude_patterns) {
        return false;
    }
    if is_dir {
        return true;
    }
    !filter_by_language || matches_source_group_language(&normalized, language)
}

fn is_excluded_path(
    path: &Path,
    workspace_root: &Path,
    source_root: &Path,
    exclude_patterns: &[glob::Pattern],
) -> bool {
    exclude_patterns.iter().any(|pattern| {
        pattern.matches_path(path)
            || path
                .strip_prefix(workspace_root)
                .ok()
                .is_some_and(|relative| pattern.matches_path(relative))
            || path
                .strip_prefix(source_root)
                .ok()
                .is_some_and(|relative| pattern.matches_path(relative))
    })
}

fn matches_source_group_language(path: &Path, language: &Language) -> bool {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    matches!(
        (language, extension.as_deref()),
        (&Language::Rust, Some("rs"))
            | (&Language::Python, Some("py" | "pyi"))
            | (&Language::Java, Some("java"))
            | (&Language::JavaScript, Some("js" | "jsx" | "mjs" | "cjs"))
            | (&Language::TypeScript, Some("ts" | "tsx" | "mts" | "cts"))
            | (
                &Language::Cxx,
                Some("c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx")
            )
    )
}

fn push_discovered_file(
    files: &mut Vec<PathBuf>,
    seen: &mut HashSet<String>,
    path: PathBuf,
    workspace_root: &Path,
) {
    let normalized = normalize_lexical_path(&path);
    let key = normalized_compare_key(workspace_root, &normalized);
    if seen.insert(key) {
        files.push(normalized);
    }
}

fn normalized_compare_key(root: &Path, path: &Path) -> String {
    let absolute = if path.is_absolute() {
        normalize_lexical_path(path)
    } else {
        normalize_lexical_path(&root.join(path))
    };
    let stable = absolute.canonicalize().unwrap_or(absolute);
    normalize_path_key(&stable)
}

fn normalize_path_key(path: &Path) -> String {
    #[cfg(windows)]
    {
        path.to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
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

pub type BuildMode = RefreshMode;
pub type Project = WorkspaceManifest;
pub type ProjectSettings = WorkspaceSettings;
pub type RefreshExecutionPlan = RefreshPlan;
pub type RefreshInfo = RefreshPlan;
pub type StoredFileRecord = StoredFileState;
pub type Workspace = WorkspaceManifest;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn builds_incremental_refresh_plan_without_storage_dependency() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let file = root.join("main.rs");
        fs::write(&file, "fn main() {}\n")?;

        let manifest = WorkspaceManifest::open(root)?;
        let plan = WorkspaceDiscovery.build_refresh_plan(
            &manifest,
            &RefreshInputs {
                stored_files: vec![StoredFileState {
                    id: 7,
                    path: file.clone(),
                    modification_time: 0,
                    indexed: true,
                }],
                inventory: WorkspaceInventory::default(),
            },
        )?;

        assert_eq!(plan.mode, RefreshMode::Incremental);
        assert_eq!(plan.files_to_index, vec![file]);
        assert!(plan.files_to_remove.is_empty());
        Ok(())
    }

    #[test]
    fn incremental_refresh_uses_millisecond_precision_for_unchanged_files() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let file = root.join("main.rs");
        fs::write(&file, "fn main() {}\n")?;

        let manifest = WorkspaceManifest::open(root)?;
        let plan = WorkspaceDiscovery.build_refresh_plan(
            &manifest,
            &RefreshInputs {
                stored_files: vec![StoredFileState {
                    id: 7,
                    path: file.clone(),
                    modification_time: modification_time_millis(&file)?,
                    indexed: true,
                }],
                inventory: WorkspaceInventory::default(),
            },
        )?;

        assert!(
            plan.files_to_index.is_empty(),
            "unchanged files should not look dirty when stored mtimes use file-table millisecond precision"
        );
        assert_eq!(plan.existing_file_ids.get(&file), Some(&7));
        Ok(())
    }

    #[test]
    fn incremental_refresh_normalizes_paths_and_removes_deleted_inventory_entries() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(&root)?;
        let file = root.join("src").join("..").join("main.rs");
        fs::write(&file, "fn main() {}\n")?;
        let deleted = root.join("deleted.rs");

        let manifest = WorkspaceManifest::open(root.clone())?;
        let plan = WorkspaceDiscovery.build_refresh_plan(
            &manifest,
            &RefreshInputs {
                stored_files: Vec::new(),
                inventory: WorkspaceInventory::from_records([
                    (
                        file.clone(),
                        IndexedFileRecord {
                            file_id: 11,
                            modification_time: modification_time_millis(&file)?,
                            indexed: true,
                        },
                    ),
                    (
                        deleted.clone(),
                        IndexedFileRecord {
                            file_id: 19,
                            modification_time: 0,
                            indexed: true,
                        },
                    ),
                ]),
            },
        )?;

        let normalized_main = root.join("main.rs");
        assert!(
            plan.files_to_index.is_empty(),
            "path-normalized inventory matches should not force reindex"
        );
        assert_eq!(plan.existing_file_ids.get(&normalized_main), Some(&11));
        assert_eq!(plan.files_to_remove, vec![19]);
        Ok(())
    }

    #[test]
    fn source_files_apply_language_filter_and_prune_ignored_directories() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(root.join("target"))?;
        fs::create_dir_all(root.join("node_modules"))?;
        fs::write(root.join("src").join("lib.rs"), "pub fn keep() {}\n")?;
        fs::write(root.join("app.tsx"), "export const App = () => <div />;\n")?;
        fs::write(
            root.join("target").join("generated.rs"),
            "pub fn ignored() {}\n",
        )?;
        fs::write(
            root.join("node_modules").join("dep.rs"),
            "pub fn also_ignored() {}\n",
        )?;

        let manifest = WorkspaceManifest::from_parts(
            WorkspaceSettings {
                name: "repo".to_string(),
                version: 1,
                source_groups: vec![SourceGroupSettings {
                    id: Uuid::new_v4(),
                    language: Language::Rust,
                    standard: LanguageStandard::Default,
                    source_paths: vec![root.clone()],
                    exclude_patterns: vec![
                        "**/target/**".to_string(),
                        "**/node_modules/**".to_string(),
                    ],
                    include_paths: Vec::new(),
                    defines: HashMap::new(),
                    language_specific: LanguageSpecificSettings::Other,
                }],
            },
            root.join("codestory_project.json"),
        );

        let files = WorkspaceDiscovery.source_files(&manifest)?;
        assert_eq!(files, vec![root.join("src").join("lib.rs")]);
        Ok(())
    }

    #[test]
    fn source_files_keep_non_rust_files_when_manifest_is_synthetic() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("app.ts"), "export const app = true;\n")?;
        fs::write(root.join("src").join("main.py"), "print('hello')\n")?;

        let manifest = WorkspaceManifest::open(root.clone())?;
        let files = WorkspaceDiscovery.source_files(&manifest)?;

        assert!(files.contains(&root.join("app.ts")));
        assert!(files.contains(&root.join("src").join("main.py")));
        Ok(())
    }

    #[test]
    fn workspace_manifest_discovers_all_member_roots() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        fs::create_dir_all(root.join("backend").join("src"))?;
        fs::create_dir_all(root.join("frontend"))?;
        fs::create_dir_all(root.join("backend").join("target"))?;
        fs::write(
            root.join("codestory_workspace.json"),
            r#"{"members":["backend","frontend"]}"#,
        )?;
        fs::write(
            root.join("backend").join("src").join("lib.rs"),
            "pub fn api() {}\n",
        )?;
        fs::write(
            root.join("backend").join("target").join("generated.rs"),
            "pub fn ignored() {}\n",
        )?;
        fs::write(
            root.join("frontend").join("app.ts"),
            "export const app = true;\n",
        )?;

        let manifest = WorkspaceManifest::open(root.clone())?;
        let files = WorkspaceDiscovery.source_files(&manifest)?;

        assert_eq!(
            manifest.members(),
            &[PathBuf::from("backend"), PathBuf::from("frontend")]
        );
        assert!(files.contains(&root.join("backend").join("src").join("lib.rs")));
        assert!(files.contains(&root.join("frontend").join("app.ts")));
        assert!(!files.contains(&root.join("backend").join("target").join("generated.rs")));
        Ok(())
    }

    #[test]
    fn source_files_allow_explicit_roots_outside_workspace_root() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let shared = temp.path().join("shared");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&shared)?;
        fs::write(shared.join("shared.ts"), "export const shared = true;\n")?;
        fs::write(shared.join("ignored.js"), "export const ignored = true;\n")?;

        let manifest = WorkspaceManifest::from_parts(
            WorkspaceSettings {
                name: "repo".to_string(),
                version: 1,
                source_groups: vec![SourceGroupSettings {
                    id: Uuid::new_v4(),
                    language: Language::TypeScript,
                    standard: LanguageStandard::Default,
                    source_paths: vec![PathBuf::from("../shared")],
                    exclude_patterns: Vec::new(),
                    include_paths: Vec::new(),
                    defines: HashMap::new(),
                    language_specific: LanguageSpecificSettings::Other,
                }],
            },
            root.join("codestory_project.json"),
        );

        let files = WorkspaceDiscovery.source_files(&manifest)?;
        assert_eq!(files, vec![shared.join("shared.ts")]);
        Ok(())
    }

    #[test]
    fn source_files_reject_symlinked_directories_outside_workspace_root() -> Result<()> {
        let temp = tempdir()?;
        let root = temp.path().join("repo");
        let outside = temp.path().join("outside");
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(&outside)?;
        fs::write(root.join("src").join("inside.rs"), "pub fn inside() {}\n")?;
        fs::write(outside.join("escape.rs"), "pub fn escape() {}\n")?;

        let link_path = root.join("linked-outside");
        if let Err(err) = try_create_dir_link(&link_path, &outside) {
            if err.kind() == io::ErrorKind::PermissionDenied {
                return Ok(());
            }
            return Err(err.into());
        }

        let manifest = WorkspaceManifest::from_parts(
            WorkspaceSettings {
                name: "repo".to_string(),
                version: 1,
                source_groups: vec![SourceGroupSettings {
                    id: Uuid::new_v4(),
                    language: Language::Rust,
                    standard: LanguageStandard::Default,
                    source_paths: vec![root.clone()],
                    exclude_patterns: Vec::new(),
                    include_paths: Vec::new(),
                    defines: HashMap::new(),
                    language_specific: LanguageSpecificSettings::Other,
                }],
            },
            root.join("codestory_project.json"),
        );

        let files = WorkspaceDiscovery.source_files(&manifest)?;
        assert_eq!(files, vec![root.join("src").join("inside.rs")]);
        Ok(())
    }

    #[cfg(windows)]
    fn try_create_dir_link(link: &Path, target: &Path) -> io::Result<()> {
        let status = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &link.display().to_string(),
                &target.display().to_string(),
            ])
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "mklink /J failed with status {status}"
            )))
        }
    }

    #[cfg(not(windows))]
    fn try_create_dir_link(link: &Path, target: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    fn modification_time_millis(path: &Path) -> Result<i64> {
        let metadata = fs::metadata(path)?;
        let modified = metadata.modified()?;
        let duration = modified.duration_since(std::time::UNIX_EPOCH)?;
        Ok(duration.as_millis().min(i64::MAX as u128) as i64)
    }
}
