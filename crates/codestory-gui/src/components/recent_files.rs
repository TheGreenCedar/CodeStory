use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Recent files manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentFiles {
    pub files: Vec<RecentFileEntry>,
    pub max_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecentFileEntry {
    pub path: PathBuf,
    pub last_opened: std::time::SystemTime,
}

impl RecentFiles {
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            max_files: 10,
        }
    }

    /// Add file to recent list
    pub fn add(&mut self, path: PathBuf) {
        // Remove if already exists
        self.files.retain(|entry| entry.path != path);

        // Add to front
        self.files.insert(
            0,
            RecentFileEntry {
                path,
                last_opened: std::time::SystemTime::now(),
            },
        );

        // Trim to max
        if self.files.len() > self.max_files {
            self.files.truncate(self.max_files);
        }
    }

    /// Get recent files
    pub fn get_recent(&self) -> &[RecentFileEntry] {
        &self.files
    }

    /// Save to disk
    pub fn save(&self) -> Result<(), std::io::Error> {
        if let Some(config_dir) = dirs::config_dir() {
            let app_dir = config_dir.join("codestory");
            if !app_dir.exists() {
                let _ = std::fs::create_dir_all(&app_dir);
            }
            let path = app_dir.join("recent_files.json");
            if let Ok(content) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(path, content);
            }
        }
        Ok(())
    }

    /// Load from disk
    pub fn load() -> Self {
        if let Some(config_dir) = dirs::config_dir() {
            let path = config_dir.join("codestory").join("recent_files.json");
            if path.exists()
                && let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(recent) = serde_json::from_str(&content)
            {
                return recent;
            }
        }
        Self::default()
    }
}

impl Default for RecentFiles {
    fn default() -> Self {
        Self::new()
    }
}
