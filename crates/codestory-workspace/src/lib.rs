use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
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
        }
    }

    pub fn load(path: PathBuf) -> Result<Self> {
        let content = fs::read_to_string(&path)?;
        let settings: WorkspaceSettings = serde_json::from_str(&content)?;
        Ok(Self {
            settings,
            manifest_path: path,
        })
    }

    pub fn save(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(&self.settings)?;
        fs::write(&self.manifest_path, content)?;
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
        }
    }

    pub fn open(root_path: PathBuf) -> Result<Self> {
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
            Ok(manifest)
        }
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
        let mut all_files = Vec::new();

        for group in &manifest.settings.source_groups {
            let mut group_files = Vec::new();
            for source_path in &group.source_paths {
                let full_path = if source_path.is_absolute() {
                    source_path.clone()
                } else {
                    manifest
                        .manifest_path
                        .parent()
                        .unwrap_or(Path::new("."))
                        .join(source_path)
                };

                if full_path.is_file() {
                    group_files.push(full_path);
                } else if full_path.is_dir() {
                    let mut builder = ignore::WalkBuilder::new(&full_path);
                    builder.follow_links(true);
                    builder.require_git(false);
                    for entry in builder.build().filter_map(|e| e.ok()) {
                        if entry.file_type().is_some_and(|kind| kind.is_file()) {
                            group_files.push(entry.into_path());
                        }
                    }
                }
            }

            for pattern in &group.exclude_patterns {
                let glob_pattern = glob::Pattern::new(pattern)?;
                group_files.retain(|path| !glob_pattern.matches_path(path));
            }

            all_files.extend(group_files);
        }

        all_files.sort();
        all_files.dedup();
        Ok(all_files)
    }

    pub fn build_refresh_plan(
        &self,
        manifest: &WorkspaceManifest,
        inputs: &RefreshInputs,
    ) -> Result<RefreshPlan> {
        let current_files = self.source_files(manifest)?;
        let stored_map = inputs.inventory_map();

        let mut files_to_index = Vec::new();
        let mut files_to_remove = Vec::new();
        let mut existing_file_ids = HashMap::new();
        let mut current_file_set = HashSet::with_capacity(current_files.len());

        for path in current_files {
            current_file_set.insert(path.clone());
            let needs_index = match stored_map.get(&path) {
                Some(file) => {
                    existing_file_ids.insert(path.clone(), file.id);
                    let mtime = modification_time_nanos(&path)?;
                    mtime > file.modification_time || !file.indexed
                }
                None => true,
            };

            if needs_index {
                files_to_index.push(path);
            }
        }

        for stored in &inputs.stored_files {
            if !current_file_set.contains(&stored.path) {
                files_to_remove.push(stored.id);
            }
        }

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

fn modification_time_nanos(path: &Path) -> Result<i64> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?;
    let duration = modified.duration_since(std::time::UNIX_EPOCH)?;
    Ok(duration.as_nanos().min(i64::MAX as u128) as i64)
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
}
