//! Multi-file Code View Component
//!
//! Displays code snippets from multiple files, grouped by file with collapsible headers.
//! This mirrors Sourcetrail's QtCodeView snippet list mode.
//! This is a planned feature - not yet integrated into the main app.

#![allow(dead_code)]

use codestory_core::{NodeId, SourceLocation};
use eframe::egui;
use egui_phosphor::regular as ph;
use std::collections::HashMap;
use std::path::PathBuf;

/// A snippet from a single file
#[derive(Debug, Clone)]
pub struct FileSnippet {
    /// File path
    pub path: PathBuf,
    /// File content (full)
    /// File content (full)
    pub content: String,
    /// Occurrences to highlight within this file
    pub occurrences: Vec<codestory_core::Occurrence>,
    /// Line ranges to display (0-indexed)
    pub visible_ranges: Vec<std::ops::Range<usize>>,
    /// Whether this file section is expanded
    pub expanded: bool,
    /// Reference count (how many references in this file)
    pub reference_count: usize,
    /// The node ID associated with this file (for navigation)
    pub file_node_id: Option<NodeId>,
}

impl FileSnippet {
    pub fn new(path: PathBuf, content: String) -> Self {
        Self {
            path,
            content,
            occurrences: Vec::new(),
            visible_ranges: Vec::new(),
            expanded: true,
            reference_count: 0,
            file_node_id: None,
        }
    }

    /// Add an occurrence and update visible ranges
    pub fn add_occurrence(&mut self, occurrence: codestory_core::Occurrence, context_lines: usize) {
        let location = occurrence.location.clone();
        self.occurrences.push(occurrence);
        self.reference_count = self.occurrences.len();

        // Calculate visible range with context
        let start_line = location.start_line.saturating_sub(context_lines as u32) as usize;
        let end_line = (location.end_line + context_lines as u32) as usize;
        let line_count = self.content.lines().count();
        let end_line = end_line.min(line_count);

        // Merge with existing ranges if overlapping
        let new_range = start_line..end_line;
        self.merge_range(new_range);
    }

    fn merge_range(&mut self, new_range: std::ops::Range<usize>) {
        // Try to merge with existing ranges
        let mut merged = false;
        for range in &mut self.visible_ranges {
            if ranges_overlap_or_adjacent(range, &new_range) {
                range.start = range.start.min(new_range.start);
                range.end = range.end.max(new_range.end);
                merged = true;
                break;
            }
        }

        if !merged {
            self.visible_ranges.push(new_range);
        }

        // Sort and merge overlapping ranges
        self.visible_ranges.sort_by_key(|r| r.start);
        self.coalesce_ranges();
    }

    fn coalesce_ranges(&mut self) {
        if self.visible_ranges.len() <= 1 {
            return;
        }

        let mut result = Vec::new();
        let mut current = self.visible_ranges[0].clone();

        for range in self.visible_ranges.iter().skip(1) {
            if ranges_overlap_or_adjacent(&current, range) {
                current.end = current.end.max(range.end);
            } else {
                result.push(current);
                current = range.clone();
            }
        }
        result.push(current);

        self.visible_ranges = result;
    }

    /// Get the file name for display
    pub fn file_name(&self) -> &str {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
    }
}

fn ranges_overlap_or_adjacent(a: &std::ops::Range<usize>, b: &std::ops::Range<usize>) -> bool {
    // Check for overlap: max(start_a, start_b) < min(end_a, end_b)
    // Check for adjacency: end_a == start_b || end_b == start_a

    // Simplest logic: if they are NOT disjoint, they overlap/touch.
    // Disjoint means a is fully before b (a.end < b.start) or b is fully before a (b.end < a.start).
    // Note: Ranges are half-open [start, end), so adjacency counts as overlap for merging purposes?
    // Actually, if we want to merge lines 1-3 and 3-5, 3 is shared?
    // No, 1-3 means lines 1, 2. 3-5 means 3, 4. So 1-3 and 3-5 are ADJACENT, not overlapping.
    // But we want to merge them into 1-5.

    let disjoint = a.end < b.start || b.end < a.start;
    !disjoint
}

/// Multi-file code view with collapsible file sections
pub struct MultiFileCodeView {
    /// Files with their snippets, keyed by path
    pub files: HashMap<PathBuf, FileSnippet>,
    /// Order of files for display (most relevant first)
    pub file_order: Vec<PathBuf>,
    /// Number of context lines around each location
    pub context_lines: usize,
    /// Currently focused location
    pub focused_location: Option<SourceLocation>,
    /// Search state
    pub search_query: String,
    pub search_active: bool,
    pub error_counts: HashMap<NodeId, usize>,
}

impl Default for MultiFileCodeView {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiFileCodeView {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            file_order: Vec::new(),
            context_lines: 3,
            focused_location: None,
            search_query: String::new(),
            search_active: false,
            error_counts: HashMap::new(),
        }
    }

    /// Clear all files
    pub fn clear(&mut self) {
        self.files.clear();
        self.file_order.clear();
        self.focused_location = None;
        self.error_counts.clear();
    }

    /// Add a file with its content
    pub fn add_file(&mut self, path: PathBuf, content: String, file_node_id: Option<NodeId>) {
        let mut snippet = FileSnippet::new(path.clone(), content);
        snippet.file_node_id = file_node_id;
        self.files.insert(path.clone(), snippet);
        if !self.file_order.contains(&path) {
            self.file_order.push(path);
        }
    }

    /// Add an occurrence to show within a file
    pub fn add_occurrence(&mut self, path: &PathBuf, occurrence: codestory_core::Occurrence) {
        if let Some(snippet) = self.files.get_mut(path) {
            snippet.add_occurrence(occurrence, self.context_lines);
        }
    }

    pub fn set_error_counts(&mut self, counts: HashMap<NodeId, usize>) {
        self.error_counts = counts;
    }

    pub fn sort_files_by_references(&mut self) {
        self.file_order.sort_by_key(|path| {
            self.files
                .get(path)
                .map(|snippet| usize::MAX - snippet.reference_count)
                .unwrap_or(usize::MAX)
        });
    }

    /// Set the focused location (will scroll to it)
    pub fn set_focus(&mut self, location: SourceLocation) {
        self.focused_location = Some(location);
    }

    /// Expand all files
    pub fn expand_all(&mut self) {
        for snippet in self.files.values_mut() {
            snippet.expanded = true;
        }
    }

    /// Collapse all files
    pub fn collapse_all(&mut self) {
        for snippet in self.files.values_mut() {
            snippet.expanded = false;
        }
    }

    /// Get total reference count across all files
    pub fn total_references(&self) -> usize {
        self.files.values().map(|f| f.reference_count).sum()
    }

    /// Render the multi-file view
    /// Returns the clicked location (if any) for navigation
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<ClickAction> {
        let mut action = None;

        // Header with stats
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} files, {} references",
                self.files.len(),
                self.total_references()
            ));

            ui.separator();

            if ui.button("Expand All").clicked() {
                self.expand_all();
            }
            if ui.button("Collapse All").clicked() {
                self.collapse_all();
            }
        });

        ui.separator();

        // Scroll area for file list
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for path in &self.file_order.clone() {
                    let error_count = self
                        .files
                        .get(path)
                        .and_then(|snippet| {
                            snippet
                                .file_node_id
                                .and_then(|id| self.error_counts.get(&id).copied())
                        })
                        .unwrap_or(0);

                    if let Some(snippet) = self.files.get_mut(path)
                        && let Some(click) = Self::render_file_section(
                            ui,
                            snippet,
                            &self.focused_location,
                            error_count,
                        )
                    {
                        action = Some(click);
                    }
                }
            });

        action
    }

    fn render_file_section(
        ui: &mut egui::Ui,
        snippet: &mut FileSnippet,
        focused: &Option<SourceLocation>,
        error_count: usize,
    ) -> Option<ClickAction> {
        let mut action = None;

        // File header (collapsible)
        let header_id = ui.make_persistent_id(snippet.path.to_string_lossy().to_string());
        let mut title = format!(
            "{} ({} references)",
            snippet.file_name(),
            snippet.reference_count
        );
        if error_count > 0 {
            title = format!("{} {} ({})", ph::WARNING_CIRCLE, title, error_count);
        }

        let header_response = egui::CollapsingHeader::new(egui::RichText::new(title).strong())
            .id_salt(header_id)
            .default_open(snippet.expanded)
            .show(ui, |ui| {
                // File path subtitle
                ui.label(
                    egui::RichText::new(snippet.path.to_string_lossy())
                        .small()
                        .color(egui::Color32::GRAY),
                );

                // Render visible ranges
                let lines: Vec<&str> = snippet.content.lines().collect();

                for (range_idx, range) in snippet.visible_ranges.iter().enumerate() {
                    if range_idx > 0 {
                        // Separator between non-contiguous ranges
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label(
                                egui::RichText::new(ph::DOTS_THREE_VERTICAL)
                                    .color(egui::Color32::DARK_GRAY),
                            );
                        });
                    }

                    for line_num in range.start..range.end.min(lines.len()) {
                        let line = lines.get(line_num).unwrap_or(&"");
                        // Note: line_num is 0-based (array index), but loc.start_line/end_line are 1-based
                        let display_line_num = line_num + 1;

                        // Determine if active and what color
                        let mut bg_color = egui::Color32::TRANSPARENT;

                        // Check focus first
                        let is_focused = focused.as_ref().is_some_and(|f| {
                            (f.start_line as usize..=f.end_line as usize)
                                .contains(&display_line_num)
                        });

                        if is_focused {
                            bg_color = egui::Color32::from_rgb(60, 80, 100);
                        } else {
                            // Check occurrences on this line
                            for occ in &snippet.occurrences {
                                let loc = &occ.location;
                                if (loc.start_line as usize..=loc.end_line as usize)
                                    .contains(&display_line_num)
                                {
                                    // Apply kind color
                                    let kind_color = match occ.kind {
                                        codestory_core::OccurrenceKind::DEFINITION => {
                                            egui::Color32::from_rgba_premultiplied(0, 100, 100, 60)
                                        }
                                        codestory_core::OccurrenceKind::REFERENCE
                                        | codestory_core::OccurrenceKind::MACRO_REFERENCE => {
                                            egui::Color32::from_rgba_premultiplied(100, 100, 0, 60)
                                        }
                                        _ => egui::Color32::from_rgba_premultiplied(60, 60, 60, 60),
                                    };
                                    // Mix or override? Override for now.
                                    bg_color = kind_color;
                                    break; // First match wins
                                }
                            }
                        }

                        let line_response = ui.horizontal(|ui| {
                            // Line number
                            let line_num_text = format!("{:4}", display_line_num);
                            ui.label(
                                egui::RichText::new(line_num_text)
                                    .monospace()
                                    .color(egui::Color32::GRAY),
                            );

                            // Line content with highlighting
                            let frame = egui::Frame::NONE.fill(bg_color);
                            frame.show(ui, |ui| {
                                ui.label(egui::RichText::new(*line).monospace());
                            });
                        });

                        // Check for click on line
                        if line_response.response.clicked() {
                            // Find the occurrence for this line
                            if let Some(occ) = snippet.occurrences.iter().find(|occ| {
                                let loc = &occ.location;
                                (loc.start_line as usize..=loc.end_line as usize)
                                    .contains(&display_line_num)
                            }) {
                                action =
                                    Some(ClickAction::NavigateToLocation(occ.location.clone()));
                            } else {
                                action = Some(ClickAction::NavigateToLine(
                                    snippet.path.clone(),
                                    line_num,
                                ));
                            }
                        }
                    }
                }
            });

        // Track expanded state
        snippet.expanded = header_response.fully_open();

        action
    }
}

/// Action from clicking in the multi-file view
#[derive(Debug, Clone)]
pub enum ClickAction {
    NavigateToLocation(SourceLocation),
    NavigateToLine(PathBuf, usize),
    OpenFile(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_range_merge() {
        let mut snippet = FileSnippet::new(
            PathBuf::from("test.rs"),
            "line1\nline2\nline3\nline4\nline5\n".into(),
        );

        snippet.visible_ranges.push(0..2);
        snippet.merge_range(1..4);

        assert_eq!(snippet.visible_ranges.len(), 1);
        assert_eq!(snippet.visible_ranges[0], 0..4);
    }

    #[test]
    fn test_reference_count() {
        use codestory_core::{NodeId, Occurrence, OccurrenceKind};

        let mut view = MultiFileCodeView::new();
        view.add_file(PathBuf::from("a.rs"), "content".into(), None);
        view.add_file(PathBuf::from("b.rs"), "content".into(), None);

        view.add_occurrence(
            &PathBuf::from("a.rs"),
            Occurrence {
                element_id: 1,
                kind: OccurrenceKind::DEFINITION,
                location: SourceLocation {
                    file_node_id: NodeId(1),
                    start_line: 1,
                    start_col: 0,
                    end_line: 1,
                    end_col: 5,
                },
            },
        );
        view.add_occurrence(
            &PathBuf::from("a.rs"),
            Occurrence {
                element_id: 1,
                kind: OccurrenceKind::REFERENCE,
                location: SourceLocation {
                    file_node_id: NodeId(1),
                    start_line: 5,
                    start_col: 0,
                    end_line: 5,
                    end_col: 5,
                },
            },
        );

        assert_eq!(view.total_references(), 2);
    }

    #[test]
    fn test_complex_merge() {
        let mut snippet = FileSnippet::new(
            std::path::PathBuf::from("test.rs"),
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n".into(),
        );

        // Add 3 ranges: 0-2 (lines 1-2), 3-5 (lines 4-5), 4-6 (lines 5-6)
        // 3-5 and 4-6 overlap. 0-2 is separate.
        snippet.visible_ranges.push(0..2);
        snippet.merge_range(3..5);
        snippet.merge_range(4..6);

        // Should result in: 0-2 and 3-6.
        assert_eq!(snippet.visible_ranges.len(), 2);
        assert_eq!(snippet.visible_ranges[0], 0..2);
        assert_eq!(snippet.visible_ranges[1], 3..6);
    }
}
