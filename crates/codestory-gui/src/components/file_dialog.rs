use crate::theme::Theme;
use eframe::egui;
use egui_file_dialog::FileDialog;
use std::path::PathBuf;

/// File dialog operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogMode {
    OpenDirectory,
}

/// Result of file dialog interaction
#[derive(Debug, Clone)]
pub enum DialogResult {
    Selected(PathBuf),
    Cancelled,
    Pending,
}

/// Manager for file dialogs throughout the application
pub struct FileDialogManager {
    dialog: FileDialog,
    current_mode: Option<DialogMode>,
    result: DialogResult,
    /// Callback identifier for knowing which operation requested the dialog
    callback_id: Option<String>,
}

impl FileDialogManager {
    pub fn new() -> Self {
        let dialog = FileDialog::new()
            .default_size([700.0, 500.0])
            .resizable(true)
            .movable(true);

        Self {
            dialog,
            current_mode: None,
            result: DialogResult::Pending,
            callback_id: None,
        }
    }

    /// Open directory selection dialog
    pub fn open_directory(
        &mut self,
        title: &str,
        callback_id: &str,
        settings: &crate::settings::FileDialogSettings,
    ) {
        if !settings.use_custom_dialogs {
            return self.open_native(title, callback_id, true, false);
        }

        // Recreate dialog with settings (can't clone)
        self.dialog = FileDialog::new()
            .default_size([settings.default_width, settings.default_height])
            .resizable(true)
            .movable(true)
            .title(title);

        self.current_mode = Some(DialogMode::OpenDirectory);
        self.callback_id = Some(callback_id.to_string());
        self.result = DialogResult::Pending;
        self.dialog.pick_directory();
    }

    /// Render the dialog (call every frame)
    pub fn render(&mut self, ctx: &egui::Context, _theme: &Theme) {
        // Update dialog state
        self.dialog.update(ctx);

        // Check for result
        if let Some(path) = self.dialog.take_picked() {
            self.result = DialogResult::Selected(path.to_path_buf());
        }
    }

    /// Helper for native fallback (using rfd)
    pub fn open_native(&mut self, title: &str, callback_id: &str, is_dir: bool, save: bool) {
        let rfd_dialog = rfd::FileDialog::new().set_title(title);

        // RFD pick_folder / pick_file / save_file
        let picked_path = if save {
            rfd_dialog.save_file()
        } else if is_dir {
            rfd_dialog.pick_folder()
        } else {
            rfd_dialog.pick_file()
        };

        if let Some(path) = picked_path {
            self.result = DialogResult::Selected(path);
            self.callback_id = Some(callback_id.to_string());
        } else {
            self.result = DialogResult::Cancelled;
            self.callback_id = Some(callback_id.to_string());
        }
    }

    /// Check if dialog is currently open
    pub fn is_open(&self) -> bool {
        *self.dialog.state() == egui_file_dialog::DialogState::Open
    }

    /// Take the result, consuming it
    pub fn take_result(&mut self) -> DialogResult {
        std::mem::replace(&mut self.result, DialogResult::Pending)
    }

    /// Get callback ID for the current dialog
    pub fn callback_id(&self) -> Option<&str> {
        self.callback_id.as_deref()
    }
}

impl Default for FileDialogManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper for common file dialog patterns
pub struct FileDialogPresets;

impl FileDialogPresets {
    /// Configure dialog for opening CodeStory projects
    pub fn open_project(_dialog: &mut FileDialogManager) {
        // Filters need to be added when creating the dialog, not after
        // We'll handle this in open_file/open_directory by recreating the dialog
    }
}
