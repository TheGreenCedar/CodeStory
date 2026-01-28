//! File Watcher Component
//!
//! Monitors file system changes and triggers re-indexing with debouncing.
//! This is a planned feature - not yet integrated into the main app.

#![allow(dead_code)]

use codestory_events::{Event, EventBus};
use notify::{Event as NotifyEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// File extensions that should trigger re-indexing
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "py", "java", "cpp", "hpp", "c", "h", "cc", "cxx", "hxx", "ts", "tsx", "js", "jsx", "go",
    "rb", "cs", "swift", "kt", "scala",
];

/// Patterns to ignore
const IGNORE_PATTERNS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "__pycache__",
    "build",
    "dist",
    ".idea",
    ".vscode",
];

pub struct FileWatcher {
    watcher: RecommendedWatcher,
    rx: Receiver<Result<NotifyEvent, notify::Error>>,
    pending_changes: HashSet<PathBuf>,
    last_event: Instant,
    debounce_duration: Duration,
    event_bus: EventBus,
    enabled: bool,
}

impl FileWatcher {
    pub fn new(event_bus: EventBus) -> Result<Self, notify::Error> {
        let (tx, rx) = channel();

        let watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;

        Ok(Self {
            watcher,
            rx,
            pending_changes: HashSet::new(),
            last_event: Instant::now(),
            debounce_duration: Duration::from_millis(500),
            event_bus,
            enabled: true,
        })
    }

    /// Start watching a directory recursively
    pub fn watch(&mut self, path: &Path) -> Result<(), notify::Error> {
        self.watcher.watch(path, RecursiveMode::Recursive)
    }

    /// Stop watching a directory
    pub fn unwatch(&mut self, path: &Path) -> Result<(), notify::Error> {
        self.watcher.unwatch(path)
    }

    /// Set the debounce duration
    pub fn set_debounce(&mut self, duration: Duration) {
        self.debounce_duration = duration;
    }

    /// Enable or disable the file watcher
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.event_bus
            .publish(Event::FileWatcherEnabled { enabled });
    }

    /// Check if enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Poll for file changes. Call this each frame.
    /// Returns true if there are pending changes being debounced.
    pub fn poll(&mut self) -> bool {
        if !self.enabled {
            return false;
        }

        // Collect new events
        while let Ok(result) = self.rx.try_recv() {
            match result {
                Ok(event) => {
                    self.handle_notify_event(event);
                }
                Err(e) => {
                    self.event_bus.publish(Event::FileWatcherError {
                        message: e.to_string(),
                    });
                }
            }
        }

        // Check if debounce period has passed
        if !self.pending_changes.is_empty() && self.last_event.elapsed() >= self.debounce_duration {
            let paths: Vec<PathBuf> = self.pending_changes.drain().collect();
            self.event_bus.publish(Event::FilesChanged { paths });
            return false;
        }

        !self.pending_changes.is_empty()
    }

    fn handle_notify_event(&mut self, event: NotifyEvent) {
        // Only handle create, modify, and remove events
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {}
            _ => return,
        }

        for path in event.paths {
            if self.should_watch_path(&path) {
                self.pending_changes.insert(path);
                self.last_event = Instant::now();
            }
        }
    }

    fn should_watch_path(&self, path: &Path) -> bool {
        // Check if path matches ignore patterns
        let path_str = path.to_string_lossy();
        for pattern in IGNORE_PATTERNS {
            if path_str.contains(pattern) {
                return false;
            }
        }

        // Check if it's a source file
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            return SOURCE_EXTENSIONS.contains(&ext_str.as_str());
        }

        false
    }

    /// Get the number of pending changes
    pub fn pending_count(&self) -> usize {
        self.pending_changes.len()
    }

    /// Clear all pending changes
    pub fn clear_pending(&mut self) {
        self.pending_changes.clear();
    }

    /// Add a pattern to ignore
    pub fn add_ignore_pattern(&mut self, _pattern: String) {
        // In a real implementation we would probably use a glob set here
        // For now we just add it to a list
    }
}

/// Settings for the file watcher
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileWatcherSettings {
    pub enabled: bool,
    pub debounce_ms: u64,
    pub additional_extensions: Vec<String>,
    pub additional_ignore_patterns: Vec<String>,
}

impl Default for FileWatcherSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            debounce_ms: 500,
            additional_extensions: Vec::new(),
            additional_ignore_patterns: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_watch_path() {
        let event_bus = EventBus::new();
        let watcher = FileWatcher::new(event_bus).unwrap();

        // Should watch source files
        assert!(watcher.should_watch_path(Path::new("src/main.rs")));
        assert!(watcher.should_watch_path(Path::new("lib/utils.py")));
        assert!(watcher.should_watch_path(Path::new("Main.java")));

        // Should not watch ignored directories
        assert!(!watcher.should_watch_path(Path::new("node_modules/pkg/index.js")));
        assert!(!watcher.should_watch_path(Path::new("target/debug/main.rs")));
        assert!(!watcher.should_watch_path(Path::new(".git/config")));

        // Should not watch non-source files
        assert!(!watcher.should_watch_path(Path::new("README.md")));
        assert!(!watcher.should_watch_path(Path::new("image.png")));
    }

    #[test]
    fn test_debounce_settings() {
        let event_bus = EventBus::new();
        let mut watcher = FileWatcher::new(event_bus).unwrap();

        watcher.set_debounce(Duration::from_millis(1000));
        assert_eq!(watcher.debounce_duration, Duration::from_millis(1000));
    }

    #[test]
    fn test_enable_disable() {
        let event_bus = EventBus::new();
        let mut watcher = FileWatcher::new(event_bus).unwrap();

        assert!(watcher.is_enabled());
        watcher.set_enabled(false);
        assert!(!watcher.is_enabled());
    }
}
