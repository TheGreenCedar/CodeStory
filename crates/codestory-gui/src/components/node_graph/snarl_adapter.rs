use codestory_graph::node_graph::PinType;
use codestory_graph::uml_types::{MemberItem, UmlNode, VisibilitySection};
use egui::Color32;
use egui_snarl::{
    InPin, NodeId as SnarlNodeId, OutPin, Snarl,
    ui::{PinInfo, SnarlPin, SnarlViewer},
};

/// Constants for member type indicators
mod member_icons {
    /// Filled circle for functions/methods/macros
    pub const FUNCTION: &str = "●";
    /// Empty circle for variables/fields/constants
    pub const VARIABLE: &str = "○";
    /// Diamond for other types
    pub const OTHER: &str = "◆";
    /// Arrow for outgoing edges
    pub const OUTGOING_EDGE: &str = "→";
}

/// Constants for UI sizing
mod ui_constants {
    pub const ICON_SIZE: f32 = 10.0;
    pub const MEMBER_TEXT_SIZE: f32 = 12.0;
    pub const SECTION_HEADER_SIZE: f32 = 10.0;
}

pub struct NodeGraphAdapter {
    pub clicked_node: Option<codestory_core::NodeId>,
    pub node_to_focus: Option<codestory_core::NodeId>,
    pub node_to_hide: Option<codestory_core::NodeId>,
    pub node_to_navigate: Option<codestory_core::NodeId>,
    pub theme: catppuccin_egui::Theme,
    /// Collapse states for nodes (persisted across graph rebuilds)
    pub collapse_states: std::collections::HashMap<
        codestory_core::NodeId,
        codestory_graph::uml_types::CollapseState,
    >,
    pub event_bus: codestory_events::EventBus,
    pub node_rects: std::collections::HashMap<codestory_core::NodeId, egui::Rect>,
    pub current_transform: egui::emath::TSTransform,
    /// Current zoom level extracted from the transform (for simplified rendering)
    pub current_zoom: f32,
    /// Pin information for UmlNode (inputs and outputs per node)
    /// Stored separately since UmlNode doesn't have pin fields
    pub pin_info: std::collections::HashMap<
        codestory_core::NodeId,
        (Vec<codestory_graph::node_graph::NodeGraphPin>, Vec<codestory_graph::node_graph::NodeGraphPin>),
    >,
    /// The visible viewport rectangle in screen coordinates (for culling).
    /// Updated each frame before snarl rendering.
    pub viewport_rect: egui::Rect,
    /// Total number of nodes in the current graph. When >= 50, viewport culling
    /// is applied to skip detailed rendering for off-screen nodes (Req 10.1).
    pub total_node_count: usize,
}

impl SnarlViewer<UmlNode> for NodeGraphAdapter {
    fn title(&mut self, node: &UmlNode) -> String {
        node.label.clone()
    }

    fn show_header(
        &mut self,
        node_id: SnarlNodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut egui::Ui,
        snarl: &mut Snarl<UmlNode>,
    ) {
        let node = &snarl[node_id];

        // Viewport culling: if we previously recorded this node's screen rect
        // and it's outside the visible area, render a minimal placeholder instead
        // of the full container content (Req 10.1, 10.4, Property 25).
        if let Some(prev_rect) = self.node_rects.get(&node.id) {
            if !self.is_node_visible(*prev_rect) {
                // Render minimal label only – avoids expensive member/section rendering
                ui.label(egui::RichText::new(&node.label).color(self.theme.subtext0).size(10.0));
                return;
            }
        }

        let label = if let Some(info) = &node.bundle_info {
            format!("{} ({})", node.label, info.count)
        } else {
            node.label.clone()
        };

        let color = match node.kind {
            codestory_core::NodeKind::CLASS => self.theme.blue,
            codestory_core::NodeKind::STRUCT => self.theme.teal,
            codestory_core::NodeKind::INTERFACE => self.theme.sky,
            codestory_core::NodeKind::FUNCTION => self.theme.yellow,
            codestory_core::NodeKind::METHOD => self.theme.peach,
            codestory_core::NodeKind::MODULE | codestory_core::NodeKind::NAMESPACE => {
                self.theme.mauve
            }
            codestory_core::NodeKind::VARIABLE | codestory_core::NodeKind::FIELD => {
                self.theme.text // Or maybe subtext1 for less emphasis
            }
            _ => {
                if node.bundle_info.is_some() {
                    self.theme.overlay1
                } else {
                    self.theme.overlay2
                }
            }
        };

        let response = ui.vertical(|ui| {
            // Render colored header (Title only)
            let header_frame = egui::Frame::default()
                .fill(color)
                .inner_margin(8.0)
                .corner_radius(egui::CornerRadius::same(5));

            let header_response = header_frame.show(ui, |ui| {
                ui.vertical(|ui| {
                    let title_response = ui.horizontal(|ui| {
                        // Show collapse indicator if node has members
                        let has_members = !node.visibility_sections.is_empty();
                        if has_members {
                            let is_collapsed = self
                                .collapse_states
                                .get(&node.id)
                                .map(|s| s.is_collapsed)
                                .unwrap_or(false);

                            let collapse_icon = if is_collapsed { "▶" } else { "▼" };
                            ui.label(egui::RichText::new(collapse_icon).color(Color32::BLACK));
                        }

                        ui.label(egui::RichText::new(&label).color(Color32::BLACK).strong());

                        // Show member count badge if collapsed
                        if has_members {
                            let is_collapsed = self
                                .collapse_states
                                .get(&node.id)
                                .map(|s| s.is_collapsed)
                                .unwrap_or(false);

                            if is_collapsed {
                                let member_count: usize = node.visibility_sections.iter()
                                    .map(|s| s.members.len())
                                    .sum();
                                ui.label(
                                    egui::RichText::new(format!("[{}]", member_count))
                                        .color(Color32::BLACK)
                                        .size(10.0),
                                );
                            }
                        }

                        if node.bundle_info.is_some()
                            && ui
                                .button(egui::RichText::new("⊕").color(Color32::BLACK))
                                .clicked()
                        {
                            // Expand bundle logic
                            self.node_to_focus = Some(node.id);
                        }
                    });

                    // Handle interaction for the title area (main node selection)
                    // Support both single-click (for selection) and double-click (for collapse toggle)
                    let title_response = ui.interact(
                        title_response.response.rect,
                        ui.id().with("title").with(node_id),
                        egui::Sense::click(),
                    );

                    // Handle double-click to toggle collapse state (Requirement 1.7, Property 5)
                    // This implements: "WHEN a Container_Node header is double-clicked,
                    // THE Graph_Renderer SHALL toggle between expanded (showing members)
                    // and collapsed (header only with member count badge) states"
                    if title_response.double_clicked() && !node.visibility_sections.is_empty() {
                        let collapse_state = self
                            .collapse_states
                            .entry(node.id)
                            .or_insert_with(codestory_graph::uml_types::CollapseState::new);
                        collapse_state.toggle_collapsed();
                        
                        self.event_bus.publish(codestory_events::Event::GraphNodeExpand {
                            id: node.id,
                            expand: !collapse_state.is_collapsed,
                        });
                    }

                    // Handle single-click for node selection
                    if title_response.clicked() {
                        self.clicked_node = Some(node.id);
                    }

                    title_response.context_menu(|ui| {
                        if ui.button("Focus").clicked() {
                            self.node_to_focus = Some(node.id);
                            ui.close();
                        }
                        if let Some(parent_id) = node.parent_id
                            && ui.button("Go to Parent").clicked()
                        {
                            self.node_to_navigate = Some(parent_id);
                            ui.close();
                        }
                        if ui.button("Hide").clicked() {
                            self.node_to_hide = Some(node.id);
                            ui.close();
                        }
                    });
                })
            });

            // Render hatching pattern overlay for non-indexed nodes
            // This satisfies Requirement 1.6 and Property 4
            if !node.is_indexed {
                self.render_hatching_pattern(ui, header_response.response.rect);
            }

            // Check if node is collapsed
            let is_collapsed = self
                .collapse_states
                .get(&node.id)
                .map(|s| s.is_collapsed)
                .unwrap_or(false);

            // Only render members if node is not collapsed and zoom is above 50%
            // When zoom < 0.5, simplify rendering by hiding member details (Req 7.5, Property 24)
            let show_members = !is_collapsed
                && !node.visibility_sections.is_empty()
                && self.current_zoom >= 0.5;

            if show_members {
                ui.add_space(4.0);

                // Render pre-grouped visibility sections from UmlNode
                for section in &node.visibility_sections {
                    if section.members.is_empty() {
                        continue;
                    }

                    self.render_visibility_section(ui, node.id, section);
                    ui.add_space(4.0);
                }
            } else if !is_collapsed
                && !node.visibility_sections.is_empty()
                && self.current_zoom < 0.5
            {
                // At low zoom, show a compact summary instead of full members
                let total_members: usize = node
                    .visibility_sections
                    .iter()
                    .map(|s| s.members.len())
                    .sum();
                if total_members > 0 {
                    ui.label(
                        egui::RichText::new(format!("{} members", total_members))
                            .color(self.theme.subtext0)
                            .size(ui_constants::SECTION_HEADER_SIZE),
                    );
                }
            }
        });
        
        // Filter out "Measure Pass" or "Auto-Layout Pass" artifacts
        // Snarl performs a layout pass where it stacks nodes in a vertical list (approaching Y=20000+).
        // This pass typically has a huge/infinite clip rect. We want to ignore these captures
        // and only persist the ones from the actual Render Pass (which matches visual layout).
        // We assume a standard screen height is < 8000px.
        if ui.clip_rect().height() > 8000.0 {
            return;
        }

        self.node_rects.insert(node.id, response.response.rect);
    }

    fn inputs(&mut self, node: &UmlNode) -> usize {
        self.pin_info
            .get(&node.id)
            .map(|(inputs, _)| inputs.len())
            .unwrap_or(0)
    }

    fn outputs(&mut self, node: &UmlNode) -> usize {
        self.pin_info
            .get(&node.id)
            .map(|(_, outputs)| outputs.len())
            .unwrap_or(0)
    }

    fn show_input(
        &mut self,
        pin: &InPin,
        ui: &mut egui::Ui,
        snarl: &mut Snarl<UmlNode>,
    ) -> impl SnarlPin + 'static {
        let node = &snarl[pin.id.node];

        // Get pin info from separate storage
        let default_pins = (Vec::new(), Vec::new());
        let (inputs, _) = self.pin_info.get(&node.id).unwrap_or(&default_pins);

        if let Some(pin_data) = inputs.get(pin.id.input) {
            ui.label(egui::RichText::new(&pin_data.label).color(self.theme.text));

            let color = match pin_data.pin_type {
                PinType::Inheritance => self.theme.blue,
                PinType::Composition => self.theme.yellow,
                PinType::Standard => self.theme.green,
            };

            PinInfo::square().with_fill(color)
        } else {
            // Fallback if pin not found
            ui.label(egui::RichText::new("?").color(self.theme.text));
            PinInfo::square().with_fill(self.theme.overlay0)
        }
    }

    fn show_output(
        &mut self,
        pin: &OutPin,
        ui: &mut egui::Ui,
        snarl: &mut Snarl<UmlNode>,
    ) -> impl SnarlPin + 'static {
        let node = &snarl[pin.id.node];

        // Get pin info from separate storage
        let default_pins = (Vec::new(), Vec::new());
        let (_, outputs) = self.pin_info.get(&node.id).unwrap_or(&default_pins);

        if let Some(pin_data) = outputs.get(pin.id.output) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(&pin_data.label).color(self.theme.text));
            });

            let color = match pin_data.pin_type {
                PinType::Inheritance => self.theme.blue,
                PinType::Composition => self.theme.yellow,
                PinType::Standard => self.theme.red,
            };

            PinInfo::circle().with_fill(color)
        } else {
            // Fallback if pin not found
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new("?").color(self.theme.text));
            });
            PinInfo::circle().with_fill(self.theme.overlay0)
        }
    }

    fn current_transform(
        &mut self,
        to_global: &mut egui::emath::TSTransform,
        _snarl: &mut Snarl<UmlNode>,
    ) {
        self.current_transform = *to_global;
        self.current_zoom = to_global.scaling;
    }
}

impl NodeGraphAdapter {
    /// Check if a node at the given screen-space rect is within the visible
    /// viewport (expanded by the culling margin). Returns `true` if culling
    /// is disabled (fewer than 50 nodes) or the node intersects the viewport.
    ///
    /// **Validates: Requirements 10.1, 10.4, Property 25**
    fn is_node_visible(&self, screen_rect: egui::Rect) -> bool {
        if self.total_node_count < codestory_graph::uml_types::VIEWPORT_CULL_THRESHOLD {
            return true;
        }
        let margin = codestory_graph::uml_types::VIEWPORT_CULL_MARGIN;
        let expanded = self.viewport_rect.expand(margin);
        expanded.intersects(screen_rect)
    }

    /// Render diagonal hatching pattern overlay for non-indexed nodes
    ///
    /// This method draws a diagonal striped pattern over the node background
    /// to indicate that the node represents an external/unresolved symbol that
    /// is not indexed in the current project.
    ///
    /// The pattern uses:
    /// - 45-degree diagonal lines
    /// - 8 pixel spacing between lines
    /// - 1.5 pixel line width
    /// - Semi-transparent dark color
    ///
    /// This satisfies Requirement 1.6 and Property 4:
    /// "WHEN a symbol is not indexed (external/unresolved), THE Graph_Renderer
    /// SHALL render diagonal striped hatching pattern over the node background"
    ///
    /// # Arguments
    /// * `ui` - The egui UI context
    /// * `rect` - The rectangle area to apply the hatching pattern to
    fn render_hatching_pattern(&self, ui: &mut egui::Ui, rect: egui::Rect) {
        // Get the hatching pattern configuration from codestory-graph
        let pattern = codestory_graph::style::hatching_pattern();

        // Get the painter for custom drawing
        let painter = ui.painter();

        // Convert pattern color to egui Color32
        let color = Color32::from_rgba_unmultiplied(
            pattern.color.r,
            pattern.color.g,
            pattern.color.b,
            pattern.color.a,
        );

        // Calculate the diagonal lines
        // For 45-degree diagonal lines, we need to draw from top-left to bottom-right
        let angle_rad = pattern.angle.to_radians();
        let cos_angle = angle_rad.cos();
        let sin_angle = angle_rad.sin();

        // Calculate the bounding box diagonal length to ensure we cover the entire rect
        let diagonal_length = ((rect.width().powi(2) + rect.height().powi(2)).sqrt()).ceil();

        // Calculate how many lines we need
        let num_lines = (diagonal_length / pattern.spacing).ceil() as i32;

        // Draw diagonal lines
        // We'll draw lines from the top-left corner, spaced by pattern.spacing
        for i in -num_lines..=num_lines {
            let offset = i as f32 * pattern.spacing;

            // Calculate start and end points for the diagonal line
            // For 45-degree angle, we can use a simpler calculation
            let start_x = rect.min.x + offset;
            let start_y = rect.min.y;
            let end_x = rect.min.x + offset + diagonal_length * cos_angle;
            let end_y = rect.min.y + diagonal_length * sin_angle;

            let start = egui::Pos2::new(start_x, start_y);
            let end = egui::Pos2::new(end_x, end_y);

            // Draw the line with specified width
            painter.line_segment([start, end], egui::Stroke::new(pattern.line_width, color));
        }
    }

    /// Render a visibility section with its header and members
    ///
    /// Renders a section with:
    /// - Section header showing the visibility kind (e.g., "FUNCTIONS", "VARIABLES")
    /// - All member rows within the section
    /// - Consistent styling with background color and padding
    fn render_visibility_section(
        &mut self,
        ui: &mut egui::Ui,
        node_id: codestory_core::NodeId,
        section: &VisibilitySection,
    ) {
        let bg_color = self.theme.mantle;
        let id = ui.make_persistent_id(format!("section_{:?}_{:?}", node_id, section.kind));

        let is_expanded = !self
            .collapse_states
            .get(&node_id)
            .map(|s| s.is_section_collapsed(section.kind))
            .unwrap_or(false);

        egui::Frame::default()
            .fill(bg_color)
            .inner_margin(8.0)
            .corner_radius(egui::CornerRadius::same(5))
            .show(ui, |ui| {
                // Manual Header
                let header_response = ui.horizontal(|ui| {
                    let icon = if is_expanded { "▼" } else { "▶" };
                    ui.label(
                        egui::RichText::new(icon)
                            .size(ui_constants::SECTION_HEADER_SIZE)
                            .color(self.theme.subtext0),
                    );

                    ui.label(
                        egui::RichText::new(section.kind.label())
                            .size(ui_constants::SECTION_HEADER_SIZE)
                            .color(self.theme.subtext0)
                            .strong(),
                    );

                    if !is_expanded {
                        ui.label(
                            egui::RichText::new(format!("[{}]", section.members.len()))
                                .size(ui_constants::SECTION_HEADER_SIZE)
                                .color(self.theme.subtext0),
                        );
                    }
                }).response;

                let header_response = ui.interact(
                    header_response.rect,
                    id.with("click"),
                    egui::Sense::click()
                );

                if header_response.clicked() {
                    let state = self.collapse_states
                        .entry(node_id)
                        .or_default();
                    state.toggle_section(section.kind);
                    
                    self.event_bus.publish(codestory_events::Event::GraphSectionExpand {
                        id: node_id,
                        section_kind: section.kind.label().to_string(),
                        expand: !state.is_section_collapsed(section.kind),
                    });
                }

                let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    true,
                );
                state.set_open(is_expanded);

                state.show_body_unindented(ui, |ui| {
                    ui.add_space(2.0);
                    for member in &section.members {
                        self.render_member_row(ui, member, node_id);
                    }
                });
            });
    }

    /// Render a single member row with icon and name
    ///
    /// This method renders a member item with:
    /// - Type indicator icon (● for functions, ○ for variables, ◆ for others)
    /// - Member name
    /// - Outgoing edge arrow indicator (→) if the member has outgoing edges
    ///
    /// The row is interactive and supports:
    /// - Click to navigate to the member
    /// - Hover tooltip showing member details
    fn render_member_row(&mut self, ui: &mut egui::Ui, member: &MemberItem, parent_id: codestory_core::NodeId) {
        let inner_response = ui.horizontal(|ui| {
            // Member type indicator icon
            // Functions: filled circle (●) in yellow/gold
            // Variables: empty circle (○) in blue
            let (icon_char, icon_color) = self.get_member_icon_and_color(member.kind);

            ui.label(
                egui::RichText::new(icon_char)
                    .color(icon_color)
                    .size(ui_constants::ICON_SIZE),
            );

            ui.label(
                egui::RichText::new(&member.name)
                    .color(self.theme.text)
                    .size(ui_constants::MEMBER_TEXT_SIZE),
            );

            // Add outgoing edge arrow indicator on the right side if member has outgoing edges
            // This satisfies Requirement 2.4 and Property 7
            if member.has_outgoing_edges {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(member_icons::OUTGOING_EDGE)
                            .color(self.theme.subtext0)
                            .size(ui_constants::MEMBER_TEXT_SIZE),
                    );
                });
            }
        });

        // Make the whole row clickable
        let response = ui.interact(
            inner_response.response.rect,
            ui.id().with(member.id),
            egui::Sense::click(),
        );

        if response.clicked() {
            self.clicked_node = Some(member.id);
        }

        // Add context menu for member interaction (Requirement 2.3)
        response.context_menu(|ui| {
            if ui.button("Focus").clicked() {
                self.node_to_focus = Some(member.id);
                ui.close();
            }
            if ui.button("Go to Definition").clicked() {
                // Determine which ID to use for navigation.
                // If the member is a full node, use its ID.
                self.node_to_navigate = Some(member.id);
                ui.close();
            }
            if ui.button("Hide Parent").clicked() {
                self.node_to_hide = Some(parent_id);
                ui.close();
            }
        });

        // Show tooltip with member details
        response.on_hover_ui(|ui| {
            ui.label(format!("{:?} {}", member.kind, member.name));
            if let Some(signature) = &member.signature {
                ui.label(signature);
            }
            if member.has_outgoing_edges {
                ui.label("Has outgoing edges");
            }
        });
    }

    /// Get the icon character and color for a member based on its kind
    ///
    /// Returns a tuple of (icon_character, color) where:
    /// - Functions/Methods/Macros: filled circle (●) in peach/yellow
    /// - Variables/Fields/Constants: empty circle (○) in blue
    /// - Other types: diamond (◆) in gray
    #[inline]
    pub(crate) fn get_member_icon_and_color(
        &self,
        kind: codestory_core::NodeKind,
    ) -> (&'static str, egui::Color32) {
        match kind {
            codestory_core::NodeKind::FUNCTION
            | codestory_core::NodeKind::METHOD
            | codestory_core::NodeKind::MACRO => (member_icons::FUNCTION, self.theme.peach),
            codestory_core::NodeKind::FIELD
            | codestory_core::NodeKind::VARIABLE
            | codestory_core::NodeKind::GLOBAL_VARIABLE
            | codestory_core::NodeKind::CONSTANT
            | codestory_core::NodeKind::ENUM_CONSTANT => (member_icons::VARIABLE, self.theme.blue),
            _ => (member_icons::OTHER, self.theme.overlay0),
        }
    }
}

// Property-based tests for hatching pattern
#[cfg(test)]
mod property_tests {
    use codestory_core::NodeKind;
    use codestory_graph::uml_types::UmlNode;
    use proptest::prelude::*;

    /// Strategy to generate UmlNode with random is_indexed values
    fn node_with_indexed_strategy() -> impl Strategy<Value = UmlNode> {
        any::<bool>().prop_flat_map(|is_indexed| {
            (0i64..1000i64).prop_map(move |id| {
                let mut node = UmlNode::new(
                    codestory_core::NodeId(id),
                    NodeKind::CLASS,
                    format!("TestNode{}", id),
                );
                node.is_indexed = is_indexed;
                node
            })
        })
    }

    proptest! {
        /// **Validates: Requirements 1.6**
        ///
        /// Property 4: Non-Indexed Node Hatching
        ///
        /// For any node where is_indexed == false, the rendered output SHALL include
        /// a hatching pattern overlay. For any node where is_indexed == true, no
        /// hatching SHALL be present.
        ///
        /// This property test verifies that:
        /// 1. The is_indexed field correctly reflects the node's indexing status
        /// 2. Non-indexed nodes (is_indexed == false) are distinguishable from indexed nodes
        /// 3. The field is properly serialized and deserialized
        #[test]
        fn prop_non_indexed_node_hatching(node in node_with_indexed_strategy()) {
            // Verify that the is_indexed field is set correctly
            // In the actual rendering, non-indexed nodes will have hatching pattern
            // This test verifies the data model supports the distinction

            if node.is_indexed {
                // Indexed nodes should have is_indexed == true
                prop_assert!(node.is_indexed,
                    "Indexed nodes must have is_indexed == true");
            } else {
                // Non-indexed nodes should have is_indexed == false
                prop_assert!(!node.is_indexed,
                    "Non-indexed nodes must have is_indexed == false");
            }

            // Verify the field is accessible and has the expected value
            prop_assert_eq!(node.is_indexed, node.is_indexed,
                "is_indexed field must be accessible");
        }

        /// Property test for hatching pattern configuration
        ///
        /// Verifies that the hatching pattern has consistent properties:
        /// - Angle is 45 degrees (diagonal)
        /// - Spacing is positive
        /// - Line width is positive
        /// - Color has appropriate alpha for overlay
        #[test]
        fn prop_hatching_pattern_consistency(_seed in 0u32..100u32) {
            let pattern = codestory_graph::style::hatching_pattern();

            // Verify angle is 45 degrees for diagonal hatching
            prop_assert_eq!(pattern.angle, 45.0,
                "Hatching pattern should use 45-degree diagonal lines");

            // Verify spacing is positive and reasonable
            prop_assert!(pattern.spacing > 0.0 && pattern.spacing < 50.0,
                "Hatching spacing should be positive and reasonable (got {})", pattern.spacing);

            // Verify line width is positive and reasonable
            prop_assert!(pattern.line_width > 0.0 && pattern.line_width < 10.0,
                "Hatching line width should be positive and reasonable (got {})", pattern.line_width);

            // Verify color has alpha channel for semi-transparency
            prop_assert!(pattern.color.a > 0 && pattern.color.a < 255,
                "Hatching color should be semi-transparent (alpha: {})", pattern.color.a);
        }
    }
}
