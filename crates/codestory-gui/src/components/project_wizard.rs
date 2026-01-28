//! Project Wizard Component
//!
//! Multi-step wizard for creating and configuring new projects.

use codestory_project::{
    Language, LanguageSpecificSettings, LanguageStandard, ProjectSettings, SourceGroupSettings,
};
use eframe::egui;
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

use crate::components::file_dialog::FileDialogManager;
use crate::theme::{self, error_box, progress_bar, spacing, warning_box};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    SelectDirectory,
    DetectLanguages,
    ConfigureSourceGroups,
    ExcludePatterns,
    Review,
}

impl WizardStep {
    fn title(&self) -> &'static str {
        match self {
            Self::SelectDirectory => "Select Project Directory",
            Self::DetectLanguages => "Detect Languages",
            Self::ConfigureSourceGroups => "Configure Source Groups",
            Self::ExcludePatterns => "Exclude Patterns",
            Self::Review => "Review & Create",
        }
    }

    fn next(&self) -> Option<Self> {
        match self {
            Self::SelectDirectory => Some(Self::DetectLanguages),
            Self::DetectLanguages => Some(Self::ConfigureSourceGroups),
            Self::ConfigureSourceGroups => Some(Self::ExcludePatterns),
            Self::ExcludePatterns => Some(Self::Review),
            Self::Review => None,
        }
    }

    fn prev(&self) -> Option<Self> {
        match self {
            Self::SelectDirectory => None,
            Self::DetectLanguages => Some(Self::SelectDirectory),
            Self::ConfigureSourceGroups => Some(Self::DetectLanguages),
            Self::ExcludePatterns => Some(Self::ConfigureSourceGroups),
            Self::Review => Some(Self::ExcludePatterns),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DetectedLanguage {
    pub language: Language,
    pub file_count: usize,
    pub enabled: bool,
}

pub struct ProjectWizard {
    pub open: bool,
    pub step: WizardStep,
    pub project_name: String,
    pub project_root: PathBuf,
    pub detected_languages: Vec<DetectedLanguage>,
    pub exclude_patterns: Vec<String>,
    pub custom_pattern: String,
    pub error_message: Option<String>,
    pub scanning: bool,
    pub total_files: usize,
}

impl Default for ProjectWizard {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectWizard {
    pub fn new() -> Self {
        Self {
            open: false,
            step: WizardStep::SelectDirectory,
            project_name: String::new(),
            project_root: PathBuf::new(),
            detected_languages: Vec::new(),
            exclude_patterns: vec![
                "**/node_modules/**".to_string(),
                "**/target/**".to_string(),
                "**/.git/**".to_string(),
                "**/build/**".to_string(),
                "**/__pycache__/**".to_string(),
            ],
            custom_pattern: String::new(),
            error_message: None,
            scanning: false,
            total_files: 0,
        }
    }

    pub fn open(&mut self) {
        self.open = true;
        self.step = WizardStep::SelectDirectory;
        self.error_message = None;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Main UI rendering
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        file_dialog: &mut FileDialogManager,
        settings: &crate::settings::AppSettings,
    ) -> Option<ProjectSettings> {
        if !self.open {
            return None;
        }

        let mut result = None;
        let mut should_close = false;

        egui::Window::new("New Project Wizard")
            .collapsible(false)
            .resizable(true)
            .min_width(500.0)
            .min_height(400.0)
            .show(ctx, |ui| {
                // Progress indicator
                ui.horizontal(|ui| {
                    let steps = [
                        WizardStep::SelectDirectory,
                        WizardStep::DetectLanguages,
                        WizardStep::ConfigureSourceGroups,
                        WizardStep::ExcludePatterns,
                        WizardStep::Review,
                    ];
                    for (i, step) in steps.iter().enumerate() {
                        let is_current = self.step == *step;
                        let is_past = (self.step as usize) > (*step as usize);
                        let color = if is_current {
                            egui::Color32::from_rgb(100, 150, 255)
                        } else if is_past {
                            egui::Color32::from_rgb(100, 200, 100)
                        } else {
                            egui::Color32::GRAY
                        };
                        ui.label(
                            egui::RichText::new(format!("{}. {}", i + 1, step.title()))
                                .color(color),
                        );
                        if i < steps.len() - 1 {
                            ui.label(" > ");
                        }
                    }
                });
                ui.separator();

                // Step content
                egui::ScrollArea::vertical().show(ui, |ui| match self.step {
                    WizardStep::SelectDirectory => {
                        self.render_select_directory(ui, file_dialog, settings)
                    }
                    WizardStep::DetectLanguages => self.render_detect_languages(ui),
                    WizardStep::ConfigureSourceGroups => self.render_configure_groups(ui),
                    WizardStep::ExcludePatterns => self.render_exclude_patterns(ui),
                    WizardStep::Review => {
                        if let Some(settings) = self.render_review(ui) {
                            result = Some(settings);
                            should_close = true;
                        }
                    }
                });

                ui.separator();

                // Error message
                if let Some(ref error) = self.error_message {
                    error_box(ui, error);
                }

                // Navigation buttons
                ui.horizontal(|ui| {
                    if let Some(prev) = self.step.prev()
                        && ui.add(theme::secondary_button(ui, "< Back")).clicked()
                    {
                        self.step = prev;
                        self.error_message = None;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::danger_button(ui, "Cancel")).clicked() {
                            should_close = true;
                        }

                        if let Some(next) = self.step.next()
                            && ui.add(theme::primary_button(ui, "Next >")).clicked()
                            && self.validate_step()
                        {
                            self.step = next;
                            self.on_step_enter();
                        }
                    });
                });
            });

        if should_close {
            self.close();
        }

        result
    }

    fn render_select_directory(
        &mut self,
        ui: &mut egui::Ui,
        file_dialog: &mut FileDialogManager,
        settings: &crate::settings::AppSettings,
    ) {
        ui.heading("Select Project Directory");
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.label("Project Name:");
            ui.text_edit_singleline(&mut self.project_name);
        });

        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.label("Root Directory:");
            let mut path_str = self.project_root.to_string_lossy().to_string();
            if ui
                .add(egui::TextEdit::singleline(&mut path_str).desired_width(300.0))
                .changed()
            {
                self.project_root = PathBuf::from(path_str);
            }
            if ui.button("Browse...").clicked() {
                file_dialog.open_directory(
                    "Select Project Root Directory",
                    "select_project_directory",
                    &settings.file_dialog,
                );
            }
        });

        ui.add_space(20.0);
        ui.label(
            "The wizard will scan this directory for source files and help you configure indexing.",
        );
    }

    fn render_detect_languages(&mut self, ui: &mut egui::Ui) {
        ui.heading("Detected Languages");
        ui.add_space(spacing::ITEM_SPACING);

        if self.scanning {
            ui.label(egui::RichText::new("Scanning directory...").color(ui.visuals().text_color()));
            ui.spinner();
            progress_bar(ui, 0.5, Some("Scanning files..."));
            return;
        }

        theme::info_box(ui, &format!("Found {} source files", self.total_files));
        ui.add_space(spacing::ITEM_SPACING);

        for lang in &mut self.detected_languages {
            ui.horizontal(|ui| {
                ui.checkbox(&mut lang.enabled, "");
                ui.label(format!("{:?} ({} files)", lang.language, lang.file_count));
            });
        }

        if self.detected_languages.is_empty() {
            ui.label(egui::RichText::new("No source files found").weak());
        }
    }

    fn render_configure_groups(&mut self, ui: &mut egui::Ui) {
        ui.heading("Configure Source Groups");
        ui.add_space(10.0);

        ui.label("Each enabled language will create a source group. You can customize paths here.");

        for lang in &self.detected_languages {
            if !lang.enabled {
                continue;
            }

            ui.collapsing(format!("{:?}", lang.language), |ui| {
                ui.label(format!("Files: {}", lang.file_count));
                ui.label("Source paths: (using project root)");
                // In a full implementation, we'd allow editing paths here
            });
        }
    }

    fn render_exclude_patterns(&mut self, ui: &mut egui::Ui) {
        ui.heading("Exclude Patterns");
        ui.add_space(spacing::ITEM_SPACING);

        warning_box(
            ui,
            "Files matching these patterns will be excluded from indexing",
        );
        ui.add_space(spacing::ITEM_SPACING);

        // Quick add buttons
        ui.horizontal_wrapped(|ui| {
            let common_patterns = [
                ("node_modules", "**/node_modules/**"),
                ("target", "**/target/**"),
                (".git", "**/.git/**"),
                ("build", "**/build/**"),
                ("dist", "**/dist/**"),
                ("vendor", "**/vendor/**"),
            ];

            for (name, pattern) in common_patterns {
                if !self.exclude_patterns.contains(&pattern.to_string())
                    && ui.add(theme::secondary_button(ui, name)).clicked()
                {
                    self.exclude_patterns.push(pattern.to_string());
                }
            }
        });

        ui.add_space(spacing::ITEM_SPACING);

        // Pattern list
        let mut to_remove = None;
        for (i, pattern) in self.exclude_patterns.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(pattern).color(ui.visuals().text_color()));
                if ui.add(theme::icon_button("×")).clicked() {
                    to_remove = Some(i);
                }
            });
        }
        if let Some(i) = to_remove {
            self.exclude_patterns.remove(i);
        }

        ui.add_space(spacing::ITEM_SPACING);

        // Add custom pattern
        ui.horizontal(|ui| {
            ui.label("Add pattern:");
            ui.text_edit_singleline(&mut self.custom_pattern);
            if ui.add(theme::primary_button(ui, "+")).clicked() && !self.custom_pattern.is_empty() {
                self.exclude_patterns.push(self.custom_pattern.clone());
                self.custom_pattern.clear();
            }
        });
    }

    fn render_review(&mut self, ui: &mut egui::Ui) -> Option<ProjectSettings> {
        ui.heading("Review & Create");
        ui.add_space(10.0);

        ui.label(format!("Project Name: {}", self.project_name));
        ui.label(format!("Root: {}", self.project_root.display()));

        ui.add_space(10.0);
        ui.label("Source Groups:");
        for lang in &self.detected_languages {
            if lang.enabled {
                ui.label(format!(
                    "  - {:?}: {} files",
                    lang.language, lang.file_count
                ));
            }
        }

        ui.add_space(10.0);
        ui.label(format!("Exclude Patterns: {}", self.exclude_patterns.len()));

        ui.add_space(20.0);

        if ui.button("Create Project").clicked() {
            return Some(self.create_project_settings());
        }

        None
    }

    fn validate_step(&mut self) -> bool {
        match self.step {
            WizardStep::SelectDirectory => {
                if self.project_name.is_empty() {
                    self.error_message = Some("Please enter a project name".to_string());
                    return false;
                }
                if !self.project_root.exists() {
                    self.error_message = Some("Directory does not exist".to_string());
                    return false;
                }
                true
            }
            WizardStep::DetectLanguages => {
                if !self.detected_languages.iter().any(|l| l.enabled) {
                    self.error_message = Some("Please enable at least one language".to_string());
                    return false;
                }
                true
            }
            _ => true,
        }
    }

    fn on_step_enter(&mut self) {
        if self.step == WizardStep::DetectLanguages {
            self.scan_directory();
        }
    }

    fn scan_directory(&mut self) {
        self.detected_languages.clear();
        self.total_files = 0;

        let mut counts: HashMap<String, usize> = HashMap::new();

        // Simple synchronous scan
        if let Ok(walker) = ignore::WalkBuilder::new(&self.project_root)
            .standard_filters(true)
            .build()
            .collect::<Result<Vec<_>, _>>()
        {
            for entry in walker {
                if entry.file_type().is_some_and(|ft| ft.is_file())
                    && let Some(ext) = entry.path().extension().and_then(|e| e.to_str())
                {
                    *counts.entry(ext.to_string()).or_insert(0) += 1;
                    self.total_files += 1;
                }
            }
        }

        // Map extensions to languages
        let ext_to_lang: HashMap<&str, Language> = [
            ("rs", Language::Rust),
            ("py", Language::Python),
            ("java", Language::Java),
            ("cpp", Language::Cxx),
            ("hpp", Language::Cxx),
            ("c", Language::Cxx),
            ("h", Language::Cxx),
            ("cc", Language::Cxx),
            ("js", Language::JavaScript),
            ("jsx", Language::JavaScript),
            ("ts", Language::TypeScript),
            ("tsx", Language::TypeScript),
        ]
        .into_iter()
        .collect();

        let mut lang_counts: HashMap<Language, usize> = HashMap::new();
        for (ext, count) in counts {
            if let Some(lang) = ext_to_lang.get(ext.as_str()) {
                *lang_counts.entry(lang.clone()).or_insert(0) += count;
            }
        }

        for (lang, count) in lang_counts {
            self.detected_languages.push(DetectedLanguage {
                language: lang,
                file_count: count,
                enabled: true,
            });
        }

        // Sort by file count descending
        self.detected_languages
            .sort_by(|a, b| b.file_count.cmp(&a.file_count));
    }

    fn create_project_settings(&self) -> ProjectSettings {
        let mut source_groups = Vec::new();

        for lang in &self.detected_languages {
            if !lang.enabled {
                continue;
            }

            source_groups.push(SourceGroupSettings {
                id: Uuid::new_v4(),
                language: lang.language.clone(),
                standard: LanguageStandard::Default,
                source_paths: vec![self.project_root.clone()],
                exclude_patterns: self.exclude_patterns.clone(),
                include_paths: vec![],
                defines: HashMap::new(),
                language_specific: LanguageSpecificSettings::Other,
            });
        }

        ProjectSettings {
            name: self.project_name.clone(),
            version: 1,
            source_groups,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wizard_step_navigation() {
        assert_eq!(
            WizardStep::SelectDirectory.next(),
            Some(WizardStep::DetectLanguages)
        );
        assert_eq!(WizardStep::SelectDirectory.prev(), None);
        assert_eq!(WizardStep::Review.next(), None);
    }

    #[test]
    fn test_wizard_creation() {
        let wizard = ProjectWizard::new();
        assert!(!wizard.open);
        assert_eq!(wizard.step, WizardStep::SelectDirectory);
        assert!(!wizard.exclude_patterns.is_empty());
    }
}
