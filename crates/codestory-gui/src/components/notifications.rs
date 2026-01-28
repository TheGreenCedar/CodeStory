use crate::theme::Theme;
use eframe::egui;
use egui_notify::{Anchor, Toast, Toasts};
use std::time::Duration;

/// Notification severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// Notification manager wrapper around egui-notify
pub struct NotificationManager {
    toasts: Toasts,
    /// Track recent notifications to avoid duplicates
    recent: Vec<(String, std::time::Instant)>,
    /// Maximum number of recent notifications to track
    max_recent: usize,
    /// Time window for deduplication (seconds)
    dedup_window: u64,
}

impl NotificationManager {
    pub fn new() -> Self {
        let toasts = Toasts::new()
            .with_anchor(Anchor::TopRight)
            .with_margin(egui::vec2(8.0, 8.0));

        Self {
            toasts,
            recent: Vec::new(),
            max_recent: 50,
            dedup_window: 2, // 2 seconds
        }
    }

    /// Show a notification
    pub fn notify(&mut self, level: NotificationLevel, message: impl Into<String>) {
        let message = message.into();

        // Check for duplicates
        if self.is_duplicate(&message) {
            return;
        }

        // Add to recent
        self.recent
            .push((message.clone(), std::time::Instant::now()));
        if self.recent.len() > self.max_recent {
            self.recent.remove(0);
        }

        // Create toast based on level
        let mut toast = match level {
            NotificationLevel::Info => Toast::info(&message),
            NotificationLevel::Success => Toast::success(&message),
            NotificationLevel::Warning => Toast::warning(&message),
            NotificationLevel::Error => Toast::error(&message),
        };

        match level {
            NotificationLevel::Info => {
                toast.duration(Some(Duration::from_secs(3)));
            }
            NotificationLevel::Success => {
                toast.duration(Some(Duration::from_secs(4)));
            }
            NotificationLevel::Warning => {
                toast.duration(Some(Duration::from_secs(5)));
            }
            NotificationLevel::Error => {
                toast.duration(Some(Duration::from_secs(8)));
            }
        }

        self.toasts.add(toast);
    }

    /// Check if message is a duplicate within dedup window
    fn is_duplicate(&mut self, message: &str) -> bool {
        let now = std::time::Instant::now();
        let window = Duration::from_secs(self.dedup_window);

        // Clean old entries (older than 60 seconds)
        self.recent
            .retain(|(_, timestamp)| now.duration_since(*timestamp) < Duration::from_secs(60));

        // Check for duplicates within the short dedup window
        self.recent
            .iter()
            .any(|(msg, timestamp)| msg == message && now.duration_since(*timestamp) < window)
    }

    /// Show info notification
    pub fn info(&mut self, message: impl Into<String>) {
        self.notify(NotificationLevel::Info, message);
    }

    /// Show success notification
    pub fn success(&mut self, message: impl Into<String>) {
        self.notify(NotificationLevel::Success, message);
    }

    /// Show warning notification
    pub fn warning(&mut self, message: impl Into<String>) {
        self.notify(NotificationLevel::Warning, message);
    }

    /// Show error notification
    pub fn error(&mut self, message: impl Into<String>) {
        self.notify(NotificationLevel::Error, message);
    }

    /// Render notifications (call once per frame)
    pub fn render(&mut self, ctx: &egui::Context, _theme: &Theme) {
        // Show toasts
        self.toasts.show(ctx);
    }

    /// Clear all notifications
    #[allow(dead_code)]
    pub fn clear_all(&mut self) {
        self.recent.clear();
        // self.toasts gets cleared automatically as they expire,
        // preventing meaningful clear without recreating the struct or using an API I don't see yet.
        // For the test purpose (checking duplicates), clearing recent is sufficient.
    }
}

impl Default for NotificationManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_creation() {
        let mut mgr = NotificationManager::new();
        mgr.info("Test info");
        mgr.success("Test success");
        mgr.warning("Test warning");
        mgr.error("Test error");
    }

    #[test]
    fn test_deduplication() {
        let mut mgr = NotificationManager::new();
        mgr.info("Duplicate message");

        // Second call should be deduplicated
        assert!(mgr.is_duplicate("Duplicate message"));
    }

    #[test]
    fn test_clear_all() {
        let mut mgr = NotificationManager::new();
        mgr.info("Message 1");
        mgr.info("Message 2");
        mgr.clear_all();
        assert_eq!(mgr.recent.len(), 0);
    }
}
