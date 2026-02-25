use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct ProjectSettings {
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

pub struct Project {
    pub settings: ProjectSettings,
    pub path: PathBuf,
}

impl Project {
    pub fn load(path: PathBuf) -> Result<Self> {
        let content = fs::read_to_string(&path)?;
        let settings: ProjectSettings = serde_json::from_str(&content)?;
        Ok(Self { settings, path })
    }

    pub fn save(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(&self.settings)?;
        fs::write(&self.path, content)?;
        Ok(())
    }

    pub fn new(name: String, path: PathBuf) -> Self {
        Self {
            settings: ProjectSettings {
                name,
                version: 1,
                source_groups: Vec::new(),
            },
            path,
        }
    }

    pub fn get_source_files(&self) -> Result<Vec<PathBuf>> {
        let mut all_files = Vec::new();

        for group in &self.settings.source_groups {
            let mut group_files = Vec::new();
            for source_path in &group.source_paths {
                let full_path = if source_path.is_absolute() {
                    source_path.clone()
                } else {
                    self.path
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

            // Apply excludes
            for pattern in &group.exclude_patterns {
                let glob_pattern = glob::Pattern::new(pattern)?;
                group_files.retain(|f| !glob_pattern.matches_path(f));
            }

            all_files.extend(group_files);
        }

        all_files.sort();
        all_files.dedup();
        Ok(all_files)
    }

    pub fn generate_refresh_info(
        &self,
        storage: &codestory_storage::Storage,
    ) -> Result<RefreshInfo> {
        self.build_refresh_plan(storage)
    }

    /// Build a single-pass refresh plan with files to index and file projections to remove.
    pub fn build_refresh_plan(&self, storage: &codestory_storage::Storage) -> Result<RefreshPlan> {
        let current_files = self.get_source_files()?;
        let stored_files = storage.get_files()?;

        let stored_map: HashMap<PathBuf, codestory_storage::FileInfo> = stored_files
            .into_iter()
            .map(|f| (f.path.clone(), f))
            .collect();

        let mut files_to_index = Vec::new();
        let mut files_to_remove = Vec::new();
        let mut current_file_set = HashSet::with_capacity(current_files.len());

        for path in current_files {
            current_file_set.insert(path.clone());
            let needs_index = match stored_map.get(&path) {
                Some(info) => {
                    let metadata = fs::metadata(&path)?;
                    let mtime = metadata
                        .modified()?
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs() as i64;
                    mtime > info.modification_time || !info.indexed
                }
                None => true,
            };

            if needs_index {
                files_to_index.push(path);
            }
        }

        // Files in storage but not in project
        for (path, info) in stored_map {
            if !current_file_set.contains(&path) {
                files_to_remove.push(info.id);
            }
        }

        Ok(RefreshInfo {
            files_to_index,
            files_to_remove,
        })
    }

    /// Opens a project from a directory. If a project file exists, loads it;
    /// otherwise creates a new project that indexes all source files in the directory.
    pub fn open(root_path: PathBuf) -> Result<Self> {
        let project_file = root_path.join("codestory_project.json");
        if project_file.exists() {
            Self::load(project_file)
        } else {
            // Create a default project that indexes the entire directory
            let mut project = Self::new(
                root_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Project")
                    .to_string(),
                project_file,
            );
            // Add a default source group that includes all files under the root
            project.settings.source_groups.push(SourceGroupSettings {
                id: Uuid::new_v4(),
                language: Language::Rust, // Default, actual language is determined per-file
                standard: LanguageStandard::Default,
                source_paths: vec![root_path],
                exclude_patterns: vec![
                    "**/node_modules/**".to_string(),
                    "**/target/**".to_string(),
                    "**/.git/**".to_string(),
                    "**/build/**".to_string(),
                ],
                include_paths: vec![],
                defines: HashMap::new(),
                language_specific: LanguageSpecificSettings::Other,
            });
            Ok(project)
        }
    }

    /// Returns a RefreshInfo for a full re-index (all source files, no removals).
    pub fn full_refresh(&self) -> Result<RefreshInfo> {
        Ok(RefreshInfo {
            files_to_index: self.get_source_files()?,
            files_to_remove: vec![],
        })
    }
}

#[derive(Debug, Clone)]
pub struct RefreshPlan {
    pub files_to_index: Vec<PathBuf>,
    pub files_to_remove: Vec<i64>, // List of IDs to remove from storage
}

/// Public alias retained for existing call sites while aligning naming with
/// workspace refresh plans in the project architecture plan.
pub type RefreshInfo = RefreshPlan;

use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_storage::{FileInfo, Storage};
    use tempfile::tempdir;

    #[test]
    fn test_project_lifecycle() -> Result<()> {
        let dir = tempdir()?;
        let project_path = dir.path().join("test_project.csproj.json");

        let mut project = Project::new("Test".to_string(), project_path.clone());
        project.settings.version = 2;
        project.save()?;

        let loaded = Project::load(project_path)?;
        assert_eq!(loaded.settings.name, "Test");
        assert_eq!(loaded.settings.version, 2);
        Ok(())
    }

    #[test]
    fn test_file_discovery() -> Result<()> {
        let dir = tempdir()?;
        let src_dir = dir.path().join("src");
        fs::create_dir(&src_dir)?;

        let f1 = src_dir.join("main.rs");
        let f2 = src_dir.join("lib.rs");
        let f3 = src_dir.join("ignore.txt");
        fs::write(&f1, "main")?;
        fs::write(&f2, "lib")?;
        fs::write(&f3, "ignore")?;

        let project_path = dir.path().join("proj.json");
        let mut project = Project::new("Test".to_string(), project_path);

        project.settings.source_groups.push(SourceGroupSettings {
            id: Uuid::new_v4(),
            language: Language::Rust,
            standard: LanguageStandard::Default,
            source_paths: vec![PathBuf::from("src")],
            exclude_patterns: vec!["**/*.txt".to_string()],
            include_paths: vec![],
            defines: HashMap::new(),
            language_specific: LanguageSpecificSettings::Other,
        });

        let files = project.get_source_files()?;
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.ends_with("main.rs")));
        assert!(files.iter().any(|f| f.ends_with("lib.rs")));
        assert!(!files.iter().any(|f| f.ends_with("ignore.txt")));

        Ok(())
    }

    #[test]
    fn test_file_discovery_respects_gitignore() -> Result<()> {
        let dir = tempdir()?;
        let src_dir = dir.path().join("src");
        fs::create_dir(&src_dir)?;

        let tracked = src_dir.join("main.rs");
        let ignored = src_dir.join("generated.ts");
        fs::write(dir.path().join(".gitignore"), "src/generated.ts\n")?;
        fs::write(&tracked, "fn main() {}\n")?;
        fs::write(&ignored, "export const generated = true;\n")?;

        let project_path = dir.path().join("proj.json");
        let mut project = Project::new("Test".to_string(), project_path);

        project.settings.source_groups.push(SourceGroupSettings {
            id: Uuid::new_v4(),
            language: Language::Rust,
            standard: LanguageStandard::Default,
            source_paths: vec![PathBuf::from("src")],
            exclude_patterns: vec![],
            include_paths: vec![],
            defines: HashMap::new(),
            language_specific: LanguageSpecificSettings::Other,
        });

        let files = project.get_source_files()?;
        assert!(files.iter().any(|f| f.ends_with("main.rs")));
        assert!(!files.iter().any(|f| f.ends_with("generated.ts")));
        Ok(())
    }

    #[test]
    fn test_refresh_info() -> Result<()> {
        let dir = tempdir()?;
        let storage = Storage::new_in_memory().map_err(|e| anyhow::anyhow!(e))?;

        // 1. Setup project with 1 file
        let f1 = dir.path().join("f1.rs");
        fs::write(&f1, "content")?;
        let project_path = dir.path().join("proj.json");
        let mut project = Project::new("Test".to_string(), project_path);
        project.settings.source_groups.push(SourceGroupSettings {
            id: Uuid::new_v4(),
            language: Language::Rust,
            standard: LanguageStandard::Default,
            source_paths: vec![f1.clone()],
            exclude_patterns: vec![],
            include_paths: vec![],
            defines: HashMap::new(),
            language_specific: LanguageSpecificSettings::Other,
        });

        // 2. Refresh info on empty storage
        let info = project.generate_refresh_info(&storage)?;
        assert_eq!(info.files_to_index.len(), 1);
        assert_eq!(info.files_to_index[0], f1);

        // 3. Mark file as indexed in storage
        let mtime = fs::metadata(&f1)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: f1.clone(),
                language: "rust".to_string(),
                modification_time: mtime,
                indexed: true,
                complete: true,
                line_count: 10,
            })
            .map_err(|e| anyhow::anyhow!(e))?;

        // 4. Refresh should now return 0 files to index
        let info2 = project.generate_refresh_info(&storage)?;
        assert_eq!(info2.files_to_index.len(), 0);

        // 5. Modify file (update mtime)
        // Wait a bit to ensure mtime changes if filesystem resolution is low?
        // Or just set it manually using filetime? For simplicity in test we just re-write.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(&f1, "content modified")?;

        let info3 = project.generate_refresh_info(&storage)?;
        assert_eq!(info3.files_to_index.len(), 1);

        // 6. Removed files in storage should be flagged for deletion
        let missing = dir.path().join("old.rs");
        storage
            .insert_file(&FileInfo {
                id: 9999,
                path: missing,
                language: "rust".to_string(),
                modification_time: 0,
                indexed: true,
                complete: true,
                line_count: 1,
            })
            .map_err(|e| anyhow::anyhow!(e))?;

        let info4 = project.generate_refresh_info(&storage)?;
        assert_eq!(info4.files_to_remove, vec![9999]);

        Ok(())
    }
}
