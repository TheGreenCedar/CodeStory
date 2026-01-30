//! Metrics Panel - Visualizations for codebase metrics using egui_plot

use eframe::egui;
use egui_plot::{Bar, BarChart, Legend, Plot, PlotPoints};
use serde::{Deserialize, Serialize};

/// Metrics for a single file
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileMetrics {
    pub file_path: String,
    pub lines_of_code: usize,
    pub comment_lines: usize,
    pub blank_lines: usize,
    pub total_lines: usize,
    pub symbol_count: usize,
    pub complexity: f64,
    pub dependencies: usize,
    pub dependents: usize,
}

/// Metrics for the entire codebase
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodebaseMetrics {
    pub total_files: usize,
    pub total_lines: usize,
    pub total_symbols: usize,
    pub avg_file_size: f64,
    pub avg_complexity: f64,
    pub file_metrics: Vec<FileMetrics>,
}

impl CodebaseMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate aggregate metrics from file metrics
    pub fn calculate_aggregates(&mut self) {
        self.total_files = self.file_metrics.len();
        self.total_lines = self.file_metrics.iter().map(|f| f.total_lines).sum();
        self.total_symbols = self.file_metrics.iter().map(|f| f.symbol_count).sum();

        if self.total_files > 0 {
            self.avg_file_size = self.total_lines as f64 / self.total_files as f64;
            self.avg_complexity = self.file_metrics.iter().map(|f| f.complexity).sum::<f64>()
                / self.total_files as f64;
        }
    }

    /// Get top N largest files
    pub fn top_files_by_size(&self, n: usize) -> Vec<&FileMetrics> {
        let mut files: Vec<&FileMetrics> = self.file_metrics.iter().collect();
        files.sort_by(|a, b| b.total_lines.cmp(&a.total_lines));
        files.truncate(n);
        files
    }

    /// Get top N most complex files
    pub fn top_files_by_complexity(&self, n: usize) -> Vec<&FileMetrics> {
        let mut files: Vec<&FileMetrics> = self.file_metrics.iter().collect();
        files.sort_by(|a, b| {
            b.complexity
                .partial_cmp(&a.complexity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        files.truncate(n);
        files
    }

    /// Get files with most dependencies
    pub fn most_coupled_files(&self, n: usize) -> Vec<&FileMetrics> {
        let mut files: Vec<&FileMetrics> = self.file_metrics.iter().collect();
        files.sort_by(|a, b| {
            let total_a = a.dependencies + a.dependents;
            let total_b = b.dependencies + b.dependents;
            total_b.cmp(&total_a)
        });
        files.truncate(n);
        files
    }

    /// Compute metrics from storage
    pub fn compute_from_storage(storage: &codestory_storage::Storage) -> Self {
        use std::collections::HashMap;
        let mut metrics = Self::new();

        // Get all nodes and filter for FILE type
        let file_nodes: Vec<_> = storage
            .get_nodes()
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n.kind == codestory_core::NodeKind::FILE)
            .collect();

        // Get all nodes for symbol counting
        let all_nodes = storage.get_nodes().unwrap_or_default();
        let total_symbols = all_nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.kind,
                    codestory_core::NodeKind::FUNCTION
                        | codestory_core::NodeKind::METHOD
                        | codestory_core::NodeKind::CLASS
                        | codestory_core::NodeKind::STRUCT
                        | codestory_core::NodeKind::INTERFACE
                        | codestory_core::NodeKind::ENUM
                        | codestory_core::NodeKind::FIELD
                        | codestory_core::NodeKind::GLOBAL_VARIABLE
                        | codestory_core::NodeKind::CONSTANT
                )
            })
            .count();

        // Count occurrences per file for symbol distribution
        let occurrences = storage.get_occurrences().unwrap_or_default();
        let mut symbols_per_file: HashMap<i64, usize> = HashMap::new();
        for occ in &occurrences {
            *symbols_per_file
                .entry(occ.location.file_node_id.0)
                .or_insert(0) += 1;
        }

        // Build file metrics
        for file_node in &file_nodes {
            let file_path = file_node.serialized_name.clone();
            let symbol_count = symbols_per_file.get(&file_node.id.0).copied().unwrap_or(0);

            // Estimate lines from occurrences (max end_line in file)
            let lines: usize = occurrences
                .iter()
                .filter(|o| o.location.file_node_id.0 == file_node.id.0)
                .map(|o| o.location.end_line as usize)
                .max()
                .unwrap_or(0);

            // Simple complexity heuristic based on symbol density
            let complexity = if lines > 0 {
                (symbol_count as f64 / lines as f64) * 100.0
            } else {
                0.0
            };

            metrics.file_metrics.push(FileMetrics {
                file_path,
                lines_of_code: lines,
                total_lines: lines,
                symbol_count,
                complexity,
                ..Default::default()
            });
        }

        // Calculate dependency coupling from edges
        if let Ok(edges) = storage.get_edges() {
            let mut deps: HashMap<i64, usize> = HashMap::new();
            let mut dependents: HashMap<i64, usize> = HashMap::new();

            for edge in &edges {
                if edge.kind == codestory_core::EdgeKind::IMPORT
                    || edge.kind == codestory_core::EdgeKind::INCLUDE
                    || edge.kind == codestory_core::EdgeKind::CALL
                {
                    let (eff_source, eff_target) = edge.effective_endpoints();
                    *deps.entry(eff_source.0).or_insert(0) += 1;
                    *dependents.entry(eff_target.0).or_insert(0) += 1;
                }
            }

            // Update file metrics with coupling data
            for _file_metric in &mut metrics.file_metrics {
                // For files, find related coupling via nodes in that file
                // This is a simplified approach
            }
        }

        // Set totals
        metrics.total_files = file_nodes.len();
        metrics.total_symbols = total_symbols;
        metrics.total_lines = metrics.file_metrics.iter().map(|f| f.total_lines).sum();

        // Calculate aggregates
        metrics.calculate_aggregates();
        metrics
    }
}

/// Visualization types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetricView {
    #[default]
    Overview,
    FileSizes,
    Complexity,
    Coupling,
    Distribution,
}

/// Metrics panel component
pub struct MetricsPanel {
    current_view: MetricView,
    metrics: Option<CodebaseMetrics>,
    is_loading: bool,
    top_n: usize,
}

impl MetricsPanel {
    pub fn new() -> Self {
        Self {
            current_view: MetricView::Overview,
            metrics: None,
            is_loading: false,
            top_n: 10,
        }
    }

    /// Update metrics data
    pub fn set_metrics(&mut self, metrics: CodebaseMetrics) {
        self.metrics = Some(metrics);
        self.is_loading = false;
    }

    /// Render the metrics panel
    pub fn render(&mut self, ui: &mut egui::Ui) {
        // View selector
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.current_view, MetricView::Overview, "📊 Overview");
            ui.selectable_value(
                &mut self.current_view,
                MetricView::FileSizes,
                "📄 File Sizes",
            );
            ui.selectable_value(
                &mut self.current_view,
                MetricView::Complexity,
                "🔀 Complexity",
            );
            ui.selectable_value(&mut self.current_view, MetricView::Coupling, "🔗 Coupling");
            ui.selectable_value(
                &mut self.current_view,
                MetricView::Distribution,
                "📈 Distribution",
            );
        });

        ui.separator();

        // Show loading or content
        if self.is_loading {
            ui.centered_and_justified(|ui| {
                ui.spinner();
                ui.label("Computing metrics...");
            });
        } else if let Some(ref metrics) = self.metrics.clone() {
            match self.current_view {
                MetricView::Overview => self.render_overview(ui, metrics),
                MetricView::FileSizes => self.render_file_sizes(ui, metrics),
                MetricView::Complexity => self.render_complexity(ui, metrics),
                MetricView::Coupling => self.render_coupling(ui, metrics),
                MetricView::Distribution => self.render_distribution(ui, metrics),
            }
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("No metrics available. Open a project to see metrics.");
            });
        }
    }

    fn render_overview(&self, ui: &mut egui::Ui, metrics: &CodebaseMetrics) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Summary cards
            ui.heading("Codebase Summary");
            ui.add_space(10.0);

            egui::Grid::new("metrics_summary")
                .num_columns(2)
                .spacing([40.0, 10.0])
                .show(ui, |ui| {
                    self.metric_card(ui, "Total Files", &metrics.total_files.to_string());
                    self.metric_card(ui, "Total Lines", &metrics.total_lines.to_string());
                    ui.end_row();

                    self.metric_card(ui, "Total Symbols", &metrics.total_symbols.to_string());
                    self.metric_card(
                        ui,
                        "Avg File Size",
                        &format!("{:.0} lines", metrics.avg_file_size),
                    );
                    ui.end_row();

                    self.metric_card(
                        ui,
                        "Avg Complexity",
                        &format!("{:.2}", metrics.avg_complexity),
                    );
                    ui.end_row();
                });

            ui.add_space(20.0);

            // Top files summary
            ui.heading("Top Files");
            ui.add_space(10.0);

            ui.label("Largest Files:");
            for file in metrics.top_files_by_size(5) {
                ui.label(format!(
                    "  {} - {} lines",
                    Self::shorten_path(&file.file_path, 50),
                    file.total_lines
                ));
            }

            ui.add_space(10.0);

            ui.label("Most Complex Files:");
            for file in metrics.top_files_by_complexity(5) {
                ui.label(format!(
                    "  {} - complexity: {:.2}",
                    Self::shorten_path(&file.file_path, 50),
                    file.complexity
                ));
            }
        });
    }

    fn render_file_sizes(&mut self, ui: &mut egui::Ui, metrics: &CodebaseMetrics) {
        let top_files = metrics.top_files_by_size(self.top_n);

        if top_files.is_empty() {
            ui.label("No file data available.");
            return;
        }

        // Create bar chart
        let bars: Vec<Bar> = top_files
            .iter()
            .enumerate()
            .map(|(i, file)| {
                Bar::new(i as f64, file.total_lines as f64)
                    .width(0.7)
                    .name(Self::shorten_path(&file.file_path, 30))
            })
            .collect();

        Plot::new("file_sizes_plot")
            .legend(Legend::default())
            .height(300.0)
            .show(ui, |plot_ui| {
                plot_ui.bar_chart(BarChart::new("Lines of Code", bars));
            });

        // Settings
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label("Show top:");
            ui.add(egui::Slider::new(&mut self.top_n, 5..=50));
        });
    }

    fn render_complexity(&mut self, ui: &mut egui::Ui, metrics: &CodebaseMetrics) {
        let top_files = metrics.top_files_by_complexity(self.top_n);

        if top_files.is_empty() {
            ui.label("No complexity data available.");
            return;
        }

        let bars: Vec<Bar> = top_files
            .iter()
            .enumerate()
            .map(|(i, file)| {
                Bar::new(i as f64, file.complexity)
                    .width(0.7)
                    .name(Self::shorten_path(&file.file_path, 30))
            })
            .collect();

        Plot::new("complexity_plot")
            .legend(Legend::default())
            .height(300.0)
            .show(ui, |plot_ui| {
                plot_ui.bar_chart(BarChart::new("Complexity", bars));
            });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label("Show top:");
            ui.add(egui::Slider::new(&mut self.top_n, 5..=50));
        });
    }

    fn render_coupling(&mut self, ui: &mut egui::Ui, metrics: &CodebaseMetrics) {
        let coupled_files = metrics.most_coupled_files(self.top_n);

        if coupled_files.is_empty() {
            ui.label("No coupling data available.");
            return;
        }

        // Scatter plot: dependencies vs dependents
        let points: PlotPoints = PlotPoints::from_iter(
            coupled_files
                .iter()
                .map(|file| [file.dependencies as f64, file.dependents as f64]),
        );

        Plot::new("coupling_plot")
            .legend(Legend::default())
            .height(300.0)
            .show(ui, |plot_ui| {
                plot_ui.points(egui_plot::Points::new("Files", points).radius(4.0));
            });

        // List most coupled files
        ui.separator();
        ui.heading("Most Coupled Files");

        egui::ScrollArea::vertical()
            .max_height(150.0)
            .show(ui, |ui| {
                for file in coupled_files {
                    ui.horizontal(|ui| {
                        ui.label(Self::shorten_path(&file.file_path, 40));
                        ui.label(format!("→{} ←{}", file.dependencies, file.dependents));
                    });
                }
            });
    }

    fn render_distribution(&self, ui: &mut egui::Ui, metrics: &CodebaseMetrics) {
        if metrics.file_metrics.is_empty() {
            ui.label("No distribution data available.");
            return;
        }

        // Histogram of file sizes
        let mut size_buckets: Vec<usize> = vec![0; 10];
        let max_size = metrics
            .file_metrics
            .iter()
            .map(|f| f.total_lines)
            .max()
            .unwrap_or(1000);

        let bucket_size = (max_size / 10).max(1);

        for file in &metrics.file_metrics {
            let bucket = (file.total_lines / bucket_size).min(9);
            size_buckets[bucket] += 1;
        }

        let bars: Vec<Bar> = size_buckets
            .iter()
            .enumerate()
            .map(|(i, &count)| {
                Bar::new(i as f64, count as f64).width(0.9).name(format!(
                    "{}-{}",
                    i * bucket_size,
                    (i + 1) * bucket_size
                ))
            })
            .collect();

        ui.heading("File Size Distribution");

        Plot::new("distribution_plot")
            .legend(Legend::default())
            .height(300.0)
            .x_axis_label("Lines of Code")
            .y_axis_label("Number of Files")
            .show(ui, |plot_ui| {
                plot_ui.bar_chart(BarChart::new("Files", bars));
            });
    }

    fn metric_card(&self, ui: &mut egui::Ui, label: &str, value: &str) {
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(label)
                    .color(ui.visuals().weak_text_color())
                    .small(),
            );
            ui.label(egui::RichText::new(value).size(24.0).strong());
        });
    }

    fn shorten_path(path: &str, max_len: usize) -> String {
        if path.len() <= max_len {
            path.to_string()
        } else {
            let start = path.len() - max_len + 3;
            format!("...{}", &path[start..])
        }
    }
}

impl Default for MetricsPanel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_metrics() {
        let _metrics = FileMetrics {
            file_path: "test.rs".to_string(),
            lines_of_code: 80,
            comment_lines: 10,
            blank_lines: 10,
            total_lines: 100,
            symbol_count: 5,
            complexity: 10.0,
            dependencies: 2,
            dependents: 1,
        };
    }

    #[test]
    fn test_codebase_metrics_aggregates() {
        let mut metrics = CodebaseMetrics::new();

        metrics.file_metrics.push(FileMetrics {
            file_path: "file1.rs".to_string(),
            lines_of_code: 100,
            total_lines: 100,
            symbol_count: 10,
            complexity: 5.0,
            ..Default::default()
        });

        metrics.file_metrics.push(FileMetrics {
            file_path: "file2.rs".to_string(),
            lines_of_code: 200,
            total_lines: 200,
            symbol_count: 20,
            complexity: 15.0,
            ..Default::default()
        });

        metrics.calculate_aggregates();

        assert_eq!(metrics.total_files, 2);
        assert_eq!(metrics.total_lines, 300);
        assert_eq!(metrics.total_symbols, 30);
        assert_eq!(metrics.avg_file_size, 150.0);
        assert_eq!(metrics.avg_complexity, 10.0);
    }
}
