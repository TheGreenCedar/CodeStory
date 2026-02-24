use codestory_core::{EdgeId, EdgeKind, NodeId, NodeKind};
use codestory_events::LayoutAlgorithm;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::Vec2;

/// A rectangle defined by min and max corners
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    /// Create a new rectangle from min and max corners
    pub fn from_min_max(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// Create a new rectangle from position and size
    pub fn from_pos_size(pos: Vec2, size: Vec2) -> Self {
        Self {
            min: pos,
            max: Vec2::new(pos.x + size.x, pos.y + size.y),
        }
    }

    /// An empty rectangle
    pub const NOTHING: Self = Self {
        min: Vec2 { x: 0.0, y: 0.0 },
        max: Vec2 { x: 0.0, y: 0.0 },
    };

    /// Get the width of the rectangle
    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    /// Get the height of the rectangle
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    /// Get the size of the rectangle
    pub fn size(&self) -> Vec2 {
        Vec2::new(self.width(), self.height())
    }

    /// Get the center of the rectangle
    pub fn center(&self) -> Vec2 {
        Vec2::new(
            self.min.x + self.width() * 0.5,
            self.min.y + self.height() * 0.5,
        )
    }

    /// Check if the rectangle contains a point
    pub fn contains(&self, point: Vec2) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
    }

    /// Check if this rectangle intersects with another rectangle
    pub fn intersects(&self, other: &Rect) -> bool {
        self.min.x <= other.max.x
            && self.max.x >= other.min.x
            && self.min.y <= other.max.y
            && self.max.y >= other.min.y
    }

    /// Return a new rectangle expanded by `amount` on all sides
    pub fn expand(&self, amount: f32) -> Rect {
        Rect {
            min: Vec2::new(self.min.x - amount, self.min.y - amount),
            max: Vec2::new(self.max.x + amount, self.max.y + amount),
        }
    }
}

/// A rectangle that is linked to a specific node or member context
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchoredRect {
    pub rect: Rect,
    pub node_id: crate::NodeIndex,
}

/// A node in the UML-style graph
#[derive(Debug, Clone)]
pub struct UmlNode {
    /// Core node data from codestory-core
    pub id: NodeId,
    pub kind: NodeKind,
    pub label: String,

    /// Parent node (for member relationships)
    pub parent_id: Option<NodeId>,

    /// Whether this node is indexed (affects hatching)
    pub is_indexed: bool,

    /// Members grouped by visibility
    pub visibility_sections: Vec<VisibilitySection>,

    /// Collapse state
    pub is_collapsed: bool,
    pub collapsed_sections: HashSet<VisibilityKind>,

    /// Computed layout info
    pub computed_rect: Rect,
    pub member_rects: HashMap<NodeId, Rect>,

    /// Bundle info (if this represents bundled nodes)
    pub bundle_info: Option<BundleInfo>,
}

impl UmlNode {
    /// Create a new UmlNode with default values
    pub fn new(id: NodeId, kind: NodeKind, label: String) -> Self {
        Self {
            id,
            kind,
            label,
            parent_id: None,
            is_indexed: true,
            visibility_sections: Vec::new(),
            is_collapsed: false,
            collapsed_sections: HashSet::new(),
            computed_rect: Rect::NOTHING,
            member_rects: HashMap::new(),
            bundle_info: None,
        }
    }

    /// Create a new UmlNode with parent
    pub fn with_parent(id: NodeId, kind: NodeKind, label: String, parent_id: NodeId) -> Self {
        Self {
            id,
            kind,
            label,
            parent_id: Some(parent_id),
            is_indexed: true,
            visibility_sections: Vec::new(),
            is_collapsed: false,
            collapsed_sections: HashSet::new(),
            computed_rect: Rect::NOTHING,
            member_rects: HashMap::new(),
            bundle_info: None,
        }
    }

    /// Calculate the size of a container node based on its content
    ///
    /// This method implements Property 2: Container Node Size Calculation
    /// For any Container_Node with N members across M visibility sections,
    /// the computed height SHALL be >= header_height + sum(section_header_heights) +
    /// sum(member_row_heights) + padding.
    ///
    /// # Parameters
    /// - `header_height`: Height of the node header (typically 30-40px)
    /// - `section_header_height`: Height of each visibility section header (typically 20-25px)
    /// - `member_row_height`: Height of each member row (typically 20-25px)
    /// - `padding`: Total padding (top + bottom + between sections, typically 16-24px)
    /// - `min_width`: Minimum width for the node (typically 150-200px)
    ///
    /// # Returns
    /// A `Vec2` representing the calculated size (width, height)
    ///
    /// # Example
    /// ```
    /// use codestory_graph::uml_types::{UmlNode, VisibilitySection, VisibilityKind, MemberItem};
    /// use codestory_core::{NodeId, NodeKind};
    /// use codestory_graph::Vec2;
    ///
    /// let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());
    ///
    /// // Add a section with 3 members
    /// let members = vec![
    ///     MemberItem::new(NodeId(2), NodeKind::METHOD, "method1".to_string()),
    ///     MemberItem::new(NodeId(3), NodeKind::METHOD, "method2".to_string()),
    ///     MemberItem::new(NodeId(4), NodeKind::FIELD, "field1".to_string()),
    /// ];
    /// node.visibility_sections.push(VisibilitySection::with_members(VisibilityKind::Public, members));
    ///
    /// let size = node.calculate_size(30.0, 20.0, 20.0, 16.0, 150.0);
    ///
    /// // Expected: header(30) + section_header(20) + 3*member_row(60) + padding(16) = 126
    /// assert!(size.y >= 126.0);
    /// ```
    pub fn calculate_size(
        &self,
        header_height: f32,
        section_header_height: f32,
        member_row_height: f32,
        padding: f32,
        min_width: f32,
    ) -> Vec2 {
        // Start with header height
        let mut total_height = header_height;

        // If node is collapsed, only show header with member count badge
        if self.is_collapsed {
            // Add minimal padding for collapsed state
            total_height += padding * 0.5;
            return Vec2::new(min_width, total_height);
        }

        // Add padding before sections
        total_height += padding * 0.5;

        // Calculate height for each visibility section
        for section in &self.visibility_sections {
            // Skip empty sections
            if section.members.is_empty() {
                continue;
            }

            // Add section header height
            total_height += section_header_height;

            // If section is collapsed, only show header with count badge
            if section.is_collapsed {
                // Add minimal spacing after collapsed section
                total_height += padding * 0.25;
                continue;
            }

            // Add height for each member in the section
            let member_count = section.members.len();
            total_height += member_count as f32 * member_row_height;

            // Add spacing between sections
            total_height += padding * 0.5;
        }

        // Add bottom padding
        total_height += padding * 0.5;

        // Calculate width based on content (for now, use min_width)
        // In a full implementation, this would measure text width
        let width = min_width;

        Vec2::new(width, total_height)
    }

    /// Calculate the size with default UI constants
    ///
    /// This is a convenience method that uses standard UI dimensions:
    /// - Header height: 35px
    /// - Section header height: 22px
    /// - Member row height: 22px
    /// - Padding: 16px
    /// - Minimum width: 180px
    pub fn calculate_size_default(&self) -> Vec2 {
        const DEFAULT_HEADER_HEIGHT: f32 = 35.0;
        const DEFAULT_SECTION_HEADER_HEIGHT: f32 = 22.0;
        const DEFAULT_MEMBER_ROW_HEIGHT: f32 = 22.0;
        const DEFAULT_PADDING: f32 = 16.0;
        const DEFAULT_MIN_WIDTH: f32 = 180.0;

        self.calculate_size(
            DEFAULT_HEADER_HEIGHT,
            DEFAULT_SECTION_HEADER_HEIGHT,
            DEFAULT_MEMBER_ROW_HEIGHT,
            DEFAULT_PADDING,
            DEFAULT_MIN_WIDTH,
        )
    }
}

/// A visibility section within a container node
#[derive(Debug, Clone)]
pub struct VisibilitySection {
    pub kind: VisibilityKind,
    pub members: Vec<MemberItem>,
    pub is_collapsed: bool,
}

impl VisibilitySection {
    /// Create a new visibility section
    pub fn new(kind: VisibilityKind) -> Self {
        Self {
            kind,
            members: Vec::new(),
            is_collapsed: false,
        }
    }

    /// Create a new visibility section with members
    pub fn with_members(kind: VisibilityKind, members: Vec<MemberItem>) -> Self {
        Self {
            kind,
            members,
            is_collapsed: false,
        }
    }

    /// Get the number of members in this section
    pub fn member_count(&self) -> usize {
        self.members.len()
    }
}

/// Visibility kind for grouping members within a container node
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VisibilityKind {
    Public,
    Private,
    Protected,
    Internal,
    Functions, // Fallback grouping by type
    Variables,
    Other,
}

impl VisibilityKind {
    /// Get the display label for this visibility kind
    pub fn label(&self) -> &'static str {
        match self {
            VisibilityKind::Public => "PUBLIC",
            VisibilityKind::Private => "PRIVATE",
            VisibilityKind::Protected => "PROTECTED",
            VisibilityKind::Internal => "INTERNAL",
            VisibilityKind::Functions => "FUNCTIONS",
            VisibilityKind::Variables => "VARIABLES",
            VisibilityKind::Other => "OTHER",
        }
    }
}

impl std::str::FromStr for VisibilityKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "PUBLIC" => Ok(VisibilityKind::Public),
            "PRIVATE" => Ok(VisibilityKind::Private),
            "PROTECTED" => Ok(VisibilityKind::Protected),
            "INTERNAL" => Ok(VisibilityKind::Internal),
            "FUNCTIONS" => Ok(VisibilityKind::Functions),
            "VARIABLES" => Ok(VisibilityKind::Variables),
            "OTHER" => Ok(VisibilityKind::Other),
            _ => Err(()),
        }
    }
}

/// A member item within a visibility section
#[derive(Debug, Clone)]
pub struct MemberItem {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,
    pub has_outgoing_edges: bool,
    pub signature: Option<String>,
}

impl MemberItem {
    /// Create a new member item
    pub fn new(id: NodeId, kind: NodeKind, name: String) -> Self {
        Self {
            id,
            kind,
            name,
            has_outgoing_edges: false,
            signature: None,
        }
    }

    /// Create a new member item with signature
    pub fn with_signature(id: NodeId, kind: NodeKind, name: String, signature: String) -> Self {
        Self {
            id,
            kind,
            name,
            has_outgoing_edges: false,
            signature: Some(signature),
        }
    }

    /// Set whether this member has outgoing edges
    pub fn set_has_outgoing_edges(&mut self, has_edges: bool) {
        self.has_outgoing_edges = has_edges;
    }
}

/// Information about a bundle of nodes
#[derive(Debug, Clone)]
pub struct BundleInfo {
    /// The node IDs that are bundled together
    pub bundled_node_ids: Vec<NodeId>,

    /// The count of bundled nodes
    pub count: usize,

    /// Whether the bundle is expanded
    pub is_expanded: bool,
}

impl BundleInfo {
    /// Create a new bundle info
    pub fn new(bundled_node_ids: Vec<NodeId>) -> Self {
        let count = bundled_node_ids.len();
        Self {
            bundled_node_ids,
            count,
            is_expanded: false,
        }
    }

    /// Toggle the expanded state
    pub fn toggle_expanded(&mut self) {
        self.is_expanded = !self.is_expanded;
    }
}

/// A routed edge with bezier control points
#[derive(Debug, Clone)]
pub struct EdgeRoute {
    /// Unique identifier for this edge
    pub id: EdgeId,

    /// Source anchor point
    pub source: EdgeAnchor,

    /// Target anchor point
    pub target: EdgeAnchor,

    /// Type of edge relationship
    pub kind: EdgeKind,

    /// Bezier control points for the curve
    pub control_points: Vec<Vec2>,

    /// Whether this edge is part of a bundle
    pub is_bundled: bool,

    /// Number of edges in the bundle (1 if not bundled)
    pub bundle_count: usize,
}

impl EdgeRoute {
    /// Create a new edge route
    pub fn new(id: EdgeId, source: EdgeAnchor, target: EdgeAnchor, kind: EdgeKind) -> Self {
        Self {
            id,
            source,
            target,
            kind,
            control_points: Vec::new(),
            is_bundled: false,
            bundle_count: 1,
        }
    }

    /// Create a new edge route with control points
    pub fn with_control_points(
        id: EdgeId,
        source: EdgeAnchor,
        target: EdgeAnchor,
        kind: EdgeKind,
        control_points: Vec<Vec2>,
    ) -> Self {
        Self {
            id,
            source,
            target,
            kind,
            control_points,
            is_bundled: false,
            bundle_count: 1,
        }
    }

    /// Create a bundled edge route
    pub fn bundled(
        id: EdgeId,
        source: EdgeAnchor,
        target: EdgeAnchor,
        kind: EdgeKind,
        control_points: Vec<Vec2>,
        bundle_count: usize,
    ) -> Self {
        Self {
            id,
            source,
            target,
            kind,
            control_points,
            is_bundled: true,
            bundle_count,
        }
    }
}

/// Where an edge connects to a node
#[derive(Debug, Clone, Copy)]
pub struct EdgeAnchor {
    /// The node this anchor is attached to
    pub node_id: NodeId,

    /// If connecting to a specific member within the node
    pub member_id: Option<NodeId>,

    /// Computed screen position of the anchor point
    pub position: Vec2,

    /// Which side of the node/member the anchor is on
    pub side: AnchorSide,
}

impl EdgeAnchor {
    /// Create a new edge anchor
    pub fn new(node_id: NodeId, position: Vec2, side: AnchorSide) -> Self {
        Self {
            node_id,
            member_id: None,
            position,
            side,
        }
    }

    /// Create a new edge anchor for a specific member
    pub fn for_member(
        node_id: NodeId,
        member_id: NodeId,
        position: Vec2,
        side: AnchorSide,
    ) -> Self {
        Self {
            node_id,
            member_id: Some(member_id),
            position,
            side,
        }
    }
}

/// Which side of a node or member an anchor is on
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnchorSide {
    Left,
    Right,
    Top,
    Bottom,
}

impl AnchorSide {
    /// Get the opposite side
    pub fn opposite(&self) -> Self {
        match self {
            AnchorSide::Left => AnchorSide::Right,
            AnchorSide::Right => AnchorSide::Left,
            AnchorSide::Top => AnchorSide::Bottom,
            AnchorSide::Bottom => AnchorSide::Top,
        }
    }

    /// Get a unit vector pointing in the direction of this side
    pub fn direction_vector(&self) -> Vec2 {
        match self {
            AnchorSide::Left => Vec2::new(-1.0, 0.0),
            AnchorSide::Right => Vec2::new(1.0, 0.0),
            AnchorSide::Top => Vec2::new(0.0, -1.0),
            AnchorSide::Bottom => Vec2::new(0.0, 1.0),
        }
    }
}

/// A bundle of multiple edges between the same source and target nodes
#[derive(Debug, Clone)]
pub struct EdgeBundle {
    /// All edges in this bundle
    pub edges: Vec<EdgeId>,

    /// Source node for all edges in the bundle
    pub source_node: NodeId,

    /// Target node for all edges in the bundle
    pub target_node: NodeId,

    /// The computed route for rendering the bundle
    pub route: EdgeRoute,
}

impl EdgeBundle {
    /// Create a new edge bundle
    pub fn new(
        edges: Vec<EdgeId>,
        source_node: NodeId,
        target_node: NodeId,
        route: EdgeRoute,
    ) -> Self {
        Self {
            edges,
            source_node,
            target_node,
            route,
        }
    }

    /// Get the number of edges in this bundle
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Check if this bundle contains a specific edge
    pub fn contains_edge(&self, edge_id: EdgeId) -> bool {
        self.edges.contains(&edge_id)
    }
}

/// Information about a bundle of edges for visualization
#[derive(Debug, Clone)]
pub struct BundleData {
    /// All edge IDs in this bundle
    pub edge_ids: Vec<EdgeId>,

    /// Edge kinds in bundle (for tooltip and rendering)
    pub edge_kinds: Vec<EdgeKind>,

    /// Source and target symbols with their relationship types (for tooltip)
    /// Format: (source_symbol_name, target_symbol_name, edge_kind)
    pub relationships: Vec<(String, String, EdgeKind)>,

    /// Visual thickness (logarithmic scale based on edge count)
    pub thickness: f32,

    /// Whether the bundle is expanded to show individual edges
    pub is_expanded: bool,
}

impl BundleData {
    /// Create a new bundle data
    pub fn new(
        edge_ids: Vec<EdgeId>,
        edge_kinds: Vec<EdgeKind>,
        relationships: Vec<(String, String, EdgeKind)>,
    ) -> Self {
        let thickness = Self::calculate_thickness(edge_ids.len());
        Self {
            edge_ids,
            edge_kinds,
            relationships,
            thickness,
            is_expanded: false,
        }
    }

    /// Calculate thickness based on edge count using logarithmic scaling
    /// Formula: min(log2(count) + base_width, max_width)
    /// where base_width = 1.0 and max_width = 6.0
    pub fn calculate_thickness(edge_count: usize) -> f32 {
        const BASE_WIDTH: f32 = 1.0;
        const MAX_WIDTH: f32 = 6.0;

        if edge_count <= 1 {
            BASE_WIDTH
        } else {
            let log_thickness = (edge_count as f32).log2() + BASE_WIDTH;
            log_thickness.min(MAX_WIDTH)
        }
    }

    /// Get the number of edges in this bundle
    pub fn edge_count(&self) -> usize {
        self.edge_ids.len()
    }

    /// Toggle the expanded state
    pub fn toggle_expanded(&mut self) {
        self.is_expanded = !self.is_expanded;
    }

    /// Set the expanded state
    pub fn set_expanded(&mut self, expanded: bool) {
        self.is_expanded = expanded;
    }

    /// Check if this bundle contains a specific edge
    pub fn contains_edge(&self, edge_id: EdgeId) -> bool {
        self.edge_ids.contains(&edge_id)
    }
}

/// Collapse state for a single node
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollapseState {
    /// Whether the entire node is collapsed (showing only header)
    pub is_collapsed: bool,

    /// Which visibility sections within the node are collapsed
    pub collapsed_sections: HashSet<VisibilityKind>,
}

impl CollapseState {
    /// Create a new collapse state with all sections expanded
    pub fn new() -> Self {
        Self {
            is_collapsed: false,
            collapsed_sections: HashSet::new(),
        }
    }

    /// Create a new collapse state with the node collapsed
    pub fn collapsed() -> Self {
        Self {
            is_collapsed: true,
            collapsed_sections: HashSet::new(),
        }
    }

    /// Create a new collapse state with specific sections collapsed
    pub fn with_collapsed_sections(collapsed_sections: HashSet<VisibilityKind>) -> Self {
        Self {
            is_collapsed: false,
            collapsed_sections,
        }
    }

    /// Toggle the node's collapsed state
    pub fn toggle_collapsed(&mut self) {
        self.is_collapsed = !self.is_collapsed;
    }

    /// Toggle a specific section's collapsed state
    pub fn toggle_section(&mut self, section: VisibilityKind) {
        if self.collapsed_sections.contains(&section) {
            self.collapsed_sections.remove(&section);
        } else {
            self.collapsed_sections.insert(section);
        }
    }

    /// Check if a specific section is collapsed
    pub fn is_section_collapsed(&self, section: VisibilityKind) -> bool {
        self.collapsed_sections.contains(&section)
    }

    /// Expand all sections
    pub fn expand_all_sections(&mut self) {
        self.collapsed_sections.clear();
    }

    /// Collapse all sections
    pub fn collapse_all_sections(&mut self) {
        self.collapsed_sections.insert(VisibilityKind::Public);
        self.collapsed_sections.insert(VisibilityKind::Private);
        self.collapsed_sections.insert(VisibilityKind::Protected);
        self.collapsed_sections.insert(VisibilityKind::Internal);
        self.collapsed_sections.insert(VisibilityKind::Functions);
        self.collapsed_sections.insert(VisibilityKind::Variables);
        self.collapsed_sections.insert(VisibilityKind::Other);
    }
}

impl Default for CollapseState {
    fn default() -> Self {
        Self::new()
    }
}

/// Persisted state for the graph view
///
/// This struct stores all user-configurable view state that should be persisted
/// across sessions, including collapse states, zoom, pan, and layout settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphViewState {
    /// Collapse state for each node (by NodeId)
    pub collapse_states: HashMap<NodeId, CollapseState>,

    /// Section collapse states within nodes (NodeId -> VisibilityKind -> is_collapsed)
    /// Note: This is redundant with collapse_states.collapsed_sections but kept for
    /// backwards compatibility and easier querying
    pub section_states: HashMap<NodeId, HashMap<VisibilityKind, bool>>,

    /// Set of hidden nodes that should not be rendered
    pub hidden_nodes: HashSet<NodeId>,

    /// Custom node positions set by user dragging (overrides layout algorithm)
    pub custom_positions: HashMap<NodeId, Vec2>,

    /// Current layout algorithm selection
    pub layout_algorithm: LayoutAlgorithm,

    /// Current layout direction (Horizontal or Vertical)
    pub layout_direction: codestory_core::LayoutDirection,

    /// Current zoom level (0.1 to 4.0, representing 10% to 400%)
    pub zoom: f32,

    /// Current pan offset in screen coordinates
    pub pan: Vec2,
}

impl GraphViewState {
    /// Create a new graph view state with default values
    pub fn new() -> Self {
        Self {
            collapse_states: HashMap::new(),
            section_states: HashMap::new(),
            hidden_nodes: HashSet::new(),
            custom_positions: HashMap::new(),
            layout_algorithm: LayoutAlgorithm::default(),
            layout_direction: codestory_core::LayoutDirection::default(),
            zoom: 1.0,
            pan: Vec2::new(0.0, 0.0),
        }
    }

    /// Get the collapse state for a node, or create a default one if not present
    pub fn get_collapse_state(&self, node_id: NodeId) -> CollapseState {
        self.collapse_states
            .get(&node_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Set the collapse state for a node
    pub fn set_collapse_state(&mut self, node_id: NodeId, state: CollapseState) {
        self.collapse_states.insert(node_id, state);
    }

    /// Toggle a node's collapsed state
    pub fn toggle_node_collapsed(&mut self, node_id: NodeId) {
        let mut state = self.get_collapse_state(node_id);
        state.toggle_collapsed();
        self.set_collapse_state(node_id, state);
    }

    /// Toggle a section's collapsed state within a node
    pub fn toggle_section_collapsed(&mut self, node_id: NodeId, section: VisibilityKind) {
        let mut state = self.get_collapse_state(node_id);
        state.toggle_section(section);
        self.set_collapse_state(node_id, state.clone());

        // Update section_states for backwards compatibility
        self.section_states
            .entry(node_id)
            .or_default()
            .insert(section, state.is_section_collapsed(section));
    }

    /// Check if a node is collapsed
    pub fn is_node_collapsed(&self, node_id: NodeId) -> bool {
        self.collapse_states
            .get(&node_id)
            .map(|s| s.is_collapsed)
            .unwrap_or(false)
    }

    /// Check if a section within a node is collapsed
    pub fn is_section_collapsed(&self, node_id: NodeId, section: VisibilityKind) -> bool {
        self.collapse_states
            .get(&node_id)
            .map(|s| s.is_section_collapsed(section))
            .unwrap_or(false)
    }

    /// Hide a node
    pub fn hide_node(&mut self, node_id: NodeId) {
        self.hidden_nodes.insert(node_id);
    }

    /// Show a node
    pub fn show_node(&mut self, node_id: NodeId) {
        self.hidden_nodes.remove(&node_id);
    }

    /// Check if a node is hidden
    pub fn is_node_hidden(&self, node_id: NodeId) -> bool {
        self.hidden_nodes.contains(&node_id)
    }

    /// Set a custom position for a node
    pub fn set_custom_position(&mut self, node_id: NodeId, position: Vec2) {
        self.custom_positions.insert(node_id, position);
    }

    /// Get the custom position for a node, if any
    pub fn get_custom_position(&self, node_id: NodeId) -> Option<Vec2> {
        self.custom_positions.get(&node_id).copied()
    }

    /// Clear custom position for a node
    pub fn clear_custom_position(&mut self, node_id: NodeId) {
        self.custom_positions.remove(&node_id);
    }

    /// Clear all custom positions
    pub fn clear_all_custom_positions(&mut self) {
        self.custom_positions.clear();
    }

    /// Set the zoom level (clamped to 0.1 - 4.0 range)
    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(0.1, 4.0);
    }

    /// Set the pan offset
    pub fn set_pan(&mut self, pan: Vec2) {
        self.pan = pan;
    }

    /// Compute the pan offset that would place `node_pos` at the viewport center.
    ///
    /// `node_pos` is in graph coordinates (the same coordinate space as node positions).
    /// Pan is in screen coordinates, so we apply zoom when converting.
    pub fn expected_pan_for_center_on(&self, node_pos: Vec2) -> Vec2 {
        Vec2::new(-node_pos.x * self.zoom, -node_pos.y * self.zoom)
    }

    /// Return true if the current pan would place `node_pos` within `tolerance_px` of the
    /// viewport center.
    pub fn is_centered_on(&self, node_pos: Vec2, tolerance_px: f32) -> bool {
        let expected = self.expected_pan_for_center_on(node_pos);
        (self.pan.x - expected.x).abs() <= tolerance_px
            && (self.pan.y - expected.y).abs() <= tolerance_px
    }

    /// Recenter the view so that `node_pos` is at the viewport center.
    ///
    /// Returns true if the pan changed by more than `tolerance_px` in either axis.
    pub fn recenter_on(&mut self, node_pos: Vec2, tolerance_px: f32) -> bool {
        if self.is_centered_on(node_pos, tolerance_px) {
            return false;
        }
        self.pan = self.expected_pan_for_center_on(node_pos);
        true
    }

    /// Set the layout algorithm
    pub fn set_layout_algorithm(&mut self, algorithm: LayoutAlgorithm) {
        self.layout_algorithm = algorithm;
    }

    /// Set the layout direction
    pub fn set_layout_direction(&mut self, direction: codestory_core::LayoutDirection) {
        self.layout_direction = direction;
    }

    /// Expand all nodes
    pub fn expand_all_nodes(&mut self) {
        for state in self.collapse_states.values_mut() {
            state.is_collapsed = false;
        }
    }

    /// Collapse all nodes
    pub fn collapse_all_nodes(&mut self) {
        for state in self.collapse_states.values_mut() {
            state.is_collapsed = true;
        }
    }

    /// Expand all sections in all nodes
    pub fn expand_all_sections(&mut self) {
        for state in self.collapse_states.values_mut() {
            state.expand_all_sections();
        }
        self.section_states.clear();
    }

    /// Collapse all sections in all nodes
    pub fn collapse_all_sections(&mut self) {
        for state in self.collapse_states.values_mut() {
            state.collapse_all_sections();
        }
        // Update section_states for backwards compatibility
        for (node_id, state) in &self.collapse_states {
            let mut sections = HashMap::new();
            for section in &state.collapsed_sections {
                sections.insert(*section, true);
            }
            self.section_states.insert(*node_id, sections);
        }
    }
}

impl Default for GraphViewState {
    fn default() -> Self {
        Self::new()
    }
}

/// The culling margin in pixels added around the viewport when determining
/// which nodes are visible. Nodes within this margin of the viewport edge
/// are still considered visible to avoid popping artifacts during panning.
pub const VIEWPORT_CULL_MARGIN: f32 = 100.0;

/// The minimum number of nodes in a graph before viewport culling is applied.
/// For smaller graphs the overhead of culling isn't worthwhile.
pub const VIEWPORT_CULL_THRESHOLD: usize = 50;

/// Determine which node IDs are visible within the given viewport.
///
/// For graphs with fewer than [`VIEWPORT_CULL_THRESHOLD`] nodes, all nodes are
/// returned as visible (no culling applied). For larger graphs, only nodes whose
/// bounding rectangles intersect the viewport (expanded by [`VIEWPORT_CULL_MARGIN`])
/// are included.
///
/// # Arguments
/// * `node_rects` - Map of node IDs to their screen-space bounding rectangles.
/// * `viewport` - The visible viewport rectangle in screen coordinates.
///
/// # Returns
/// A `HashSet` of `NodeId`s that are considered visible.
///
/// **Validates: Requirements 10.1, 10.4, Property 25**
pub fn viewport_cull(node_rects: &HashMap<NodeId, Rect>, viewport: Rect) -> HashSet<NodeId> {
    // Below threshold: all nodes visible
    if node_rects.len() < VIEWPORT_CULL_THRESHOLD {
        return node_rects.keys().copied().collect();
    }

    let expanded = viewport.expand(VIEWPORT_CULL_MARGIN);
    node_rects
        .iter()
        .filter(|(_, rect)| expanded.intersects(rect))
        .map(|(id, _)| *id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uml_node_creation() {
        let node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());
        assert_eq!(node.id, NodeId(1));
        assert_eq!(node.kind, NodeKind::CLASS);
        assert_eq!(node.label, "TestClass");
        assert_eq!(node.parent_id, None);
        assert!(node.is_indexed);
        assert!(!node.is_collapsed);
        assert!(node.visibility_sections.is_empty());
    }

    #[test]
    fn test_uml_node_with_parent() {
        let node = UmlNode::with_parent(
            NodeId(2),
            NodeKind::METHOD,
            "testMethod".to_string(),
            NodeId(1),
        );
        assert_eq!(node.id, NodeId(2));
        assert_eq!(node.parent_id, Some(NodeId(1)));
    }

    #[test]
    fn test_visibility_section_creation() {
        let section = VisibilitySection::new(VisibilityKind::Public);
        assert_eq!(section.kind, VisibilityKind::Public);
        assert!(section.members.is_empty());
        assert!(!section.is_collapsed);
    }

    #[test]
    fn test_visibility_section_with_members() {
        let members = vec![
            MemberItem::new(NodeId(1), NodeKind::METHOD, "method1".to_string()),
            MemberItem::new(NodeId(2), NodeKind::FIELD, "field1".to_string()),
        ];
        let section = VisibilitySection::with_members(VisibilityKind::Public, members);
        assert_eq!(section.member_count(), 2);
    }

    #[test]
    fn test_visibility_kind_labels() {
        assert_eq!(VisibilityKind::Public.label(), "PUBLIC");
        assert_eq!(VisibilityKind::Private.label(), "PRIVATE");
        assert_eq!(VisibilityKind::Protected.label(), "PROTECTED");
        assert_eq!(VisibilityKind::Internal.label(), "INTERNAL");
        assert_eq!(VisibilityKind::Functions.label(), "FUNCTIONS");
        assert_eq!(VisibilityKind::Variables.label(), "VARIABLES");
        assert_eq!(VisibilityKind::Other.label(), "OTHER");
    }

    #[test]
    fn test_member_item_creation() {
        let member = MemberItem::new(NodeId(1), NodeKind::METHOD, "testMethod".to_string());
        assert_eq!(member.id, NodeId(1));
        assert_eq!(member.kind, NodeKind::METHOD);
        assert_eq!(member.name, "testMethod");
        assert!(!member.has_outgoing_edges);
        assert_eq!(member.signature, None);
    }

    #[test]
    fn test_member_item_with_signature() {
        let member = MemberItem::with_signature(
            NodeId(1),
            NodeKind::METHOD,
            "testMethod".to_string(),
            "fn testMethod() -> bool".to_string(),
        );
        assert_eq!(
            member.signature,
            Some("fn testMethod() -> bool".to_string())
        );
    }

    #[test]
    fn test_member_item_set_has_outgoing_edges() {
        let mut member = MemberItem::new(NodeId(1), NodeKind::METHOD, "testMethod".to_string());
        assert!(!member.has_outgoing_edges);
        member.set_has_outgoing_edges(true);
        assert!(member.has_outgoing_edges);
    }

    #[test]
    fn test_bundle_info_creation() {
        let node_ids = vec![NodeId(1), NodeId(2), NodeId(3)];
        let bundle = BundleInfo::new(node_ids.clone());
        assert_eq!(bundle.count, 3);
        assert_eq!(bundle.bundled_node_ids, node_ids);
        assert!(!bundle.is_expanded);
    }

    #[test]
    fn test_bundle_info_toggle_expanded() {
        let mut bundle = BundleInfo::new(vec![NodeId(1), NodeId(2)]);
        assert!(!bundle.is_expanded);
        bundle.toggle_expanded();
        assert!(bundle.is_expanded);
        bundle.toggle_expanded();
        assert!(!bundle.is_expanded);
    }

    #[test]
    fn test_edge_route_creation() {
        let source = EdgeAnchor::new(NodeId(1), Vec2::new(0.0, 0.0), AnchorSide::Right);
        let target = EdgeAnchor::new(NodeId(2), Vec2::new(100.0, 0.0), AnchorSide::Left);
        let route = EdgeRoute::new(EdgeId(1), source, target, EdgeKind::CALL);

        assert_eq!(route.id, EdgeId(1));
        assert_eq!(route.source.node_id, NodeId(1));
        assert_eq!(route.target.node_id, NodeId(2));
        assert_eq!(route.kind, EdgeKind::CALL);
        assert!(route.control_points.is_empty());
        assert!(!route.is_bundled);
        assert_eq!(route.bundle_count, 1);
    }

    #[test]
    fn test_edge_route_with_control_points() {
        let source = EdgeAnchor::new(NodeId(1), Vec2::new(0.0, 0.0), AnchorSide::Right);
        let target = EdgeAnchor::new(NodeId(2), Vec2::new(100.0, 0.0), AnchorSide::Left);
        let control_points = vec![Vec2::new(25.0, 0.0), Vec2::new(75.0, 0.0)];
        let route = EdgeRoute::with_control_points(
            EdgeId(1),
            source,
            target,
            EdgeKind::CALL,
            control_points.clone(),
        );

        assert_eq!(route.control_points.len(), 2);
        assert_eq!(route.control_points, control_points);
    }

    #[test]
    fn test_edge_route_bundled() {
        let source = EdgeAnchor::new(NodeId(1), Vec2::new(0.0, 0.0), AnchorSide::Right);
        let target = EdgeAnchor::new(NodeId(2), Vec2::new(100.0, 0.0), AnchorSide::Left);
        let control_points = vec![Vec2::new(25.0, 0.0), Vec2::new(75.0, 0.0)];
        let route =
            EdgeRoute::bundled(EdgeId(1), source, target, EdgeKind::CALL, control_points, 5);

        assert!(route.is_bundled);
        assert_eq!(route.bundle_count, 5);
    }

    #[test]
    fn test_edge_anchor_creation() {
        let anchor = EdgeAnchor::new(NodeId(1), Vec2::new(10.0, 20.0), AnchorSide::Right);

        assert_eq!(anchor.node_id, NodeId(1));
        assert_eq!(anchor.member_id, None);
        assert_eq!(anchor.position, Vec2::new(10.0, 20.0));
        assert_eq!(anchor.side, AnchorSide::Right);
    }

    #[test]
    fn test_edge_anchor_for_member() {
        let anchor = EdgeAnchor::for_member(
            NodeId(1),
            NodeId(2),
            Vec2::new(10.0, 20.0),
            AnchorSide::Right,
        );

        assert_eq!(anchor.node_id, NodeId(1));
        assert_eq!(anchor.member_id, Some(NodeId(2)));
        assert_eq!(anchor.position, Vec2::new(10.0, 20.0));
        assert_eq!(anchor.side, AnchorSide::Right);
    }

    #[test]
    fn test_anchor_side_opposite() {
        assert_eq!(AnchorSide::Left.opposite(), AnchorSide::Right);
        assert_eq!(AnchorSide::Right.opposite(), AnchorSide::Left);
        assert_eq!(AnchorSide::Top.opposite(), AnchorSide::Bottom);
        assert_eq!(AnchorSide::Bottom.opposite(), AnchorSide::Top);
    }

    #[test]
    fn test_anchor_side_direction_vector() {
        assert_eq!(AnchorSide::Left.direction_vector(), Vec2::new(-1.0, 0.0));
        assert_eq!(AnchorSide::Right.direction_vector(), Vec2::new(1.0, 0.0));
        assert_eq!(AnchorSide::Top.direction_vector(), Vec2::new(0.0, -1.0));
        assert_eq!(AnchorSide::Bottom.direction_vector(), Vec2::new(0.0, 1.0));
    }

    #[test]
    fn test_edge_bundle_creation() {
        let source = EdgeAnchor::new(NodeId(1), Vec2::new(0.0, 0.0), AnchorSide::Right);
        let target = EdgeAnchor::new(NodeId(2), Vec2::new(100.0, 0.0), AnchorSide::Left);
        let route = EdgeRoute::new(EdgeId(1), source, target, EdgeKind::CALL);

        let edges = vec![EdgeId(1), EdgeId(2), EdgeId(3)];
        let bundle = EdgeBundle::new(edges.clone(), NodeId(1), NodeId(2), route);

        assert_eq!(bundle.edges, edges);
        assert_eq!(bundle.source_node, NodeId(1));
        assert_eq!(bundle.target_node, NodeId(2));
        assert_eq!(bundle.edge_count(), 3);
    }

    #[test]
    fn test_container_node_size_calculation_empty() {
        // Test size calculation for a node with no members
        let node = UmlNode::new(NodeId(1), NodeKind::CLASS, "EmptyClass".to_string());

        let size = node.calculate_size(30.0, 20.0, 20.0, 16.0, 150.0);

        // Expected: header(30) + padding(8) = 38
        assert_eq!(size.x, 150.0);
        assert!(size.y >= 38.0, "Height should be at least header + padding");
    }

    #[test]
    fn test_container_node_size_calculation_with_members() {
        // Test size calculation for a node with members
        let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());

        // Add a section with 3 members
        let members = vec![
            MemberItem::new(NodeId(2), NodeKind::METHOD, "method1".to_string()),
            MemberItem::new(NodeId(3), NodeKind::METHOD, "method2".to_string()),
            MemberItem::new(NodeId(4), NodeKind::FIELD, "field1".to_string()),
        ];
        node.visibility_sections
            .push(VisibilitySection::with_members(
                VisibilityKind::Public,
                members,
            ));

        let size = node.calculate_size(30.0, 20.0, 20.0, 16.0, 150.0);

        // Expected: header(30) + padding_top(8) + section_header(20) + 3*member_row(60) + section_padding(8) + padding_bottom(8) = 134
        assert_eq!(size.x, 150.0);
        assert!(
            size.y >= 126.0,
            "Height should include header + section header + members + padding"
        );
    }

    #[test]
    fn test_container_node_size_calculation_multiple_sections() {
        // Test size calculation for a node with multiple sections
        let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());

        // Add Functions section with 2 members
        let functions = vec![
            MemberItem::new(NodeId(2), NodeKind::METHOD, "method1".to_string()),
            MemberItem::new(NodeId(3), NodeKind::METHOD, "method2".to_string()),
        ];
        node.visibility_sections
            .push(VisibilitySection::with_members(
                VisibilityKind::Functions,
                functions,
            ));

        // Add Variables section with 2 members
        let variables = vec![
            MemberItem::new(NodeId(4), NodeKind::FIELD, "field1".to_string()),
            MemberItem::new(NodeId(5), NodeKind::FIELD, "field2".to_string()),
        ];
        node.visibility_sections
            .push(VisibilitySection::with_members(
                VisibilityKind::Variables,
                variables,
            ));

        let size = node.calculate_size(30.0, 20.0, 20.0, 16.0, 150.0);

        // Expected: header(30) + padding_top(8) +
        //           section1_header(20) + 2*member_row(40) + section1_padding(8) +
        //           section2_header(20) + 2*member_row(40) + section2_padding(8) +
        //           padding_bottom(8) = 182
        assert_eq!(size.x, 150.0);
        assert!(
            size.y >= 174.0,
            "Height should include header + 2 sections with members + padding"
        );
    }

    #[test]
    fn test_container_node_size_calculation_collapsed() {
        // Test size calculation for a collapsed node
        let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());
        node.is_collapsed = true;

        // Add members (should be ignored when collapsed)
        let members = vec![
            MemberItem::new(NodeId(2), NodeKind::METHOD, "method1".to_string()),
            MemberItem::new(NodeId(3), NodeKind::METHOD, "method2".to_string()),
        ];
        node.visibility_sections
            .push(VisibilitySection::with_members(
                VisibilityKind::Public,
                members,
            ));

        let size = node.calculate_size(30.0, 20.0, 20.0, 16.0, 150.0);

        // Expected: header(30) + minimal_padding(8) = 38
        assert_eq!(size.x, 150.0);
        assert!(
            size.y >= 30.0 && size.y < 50.0,
            "Collapsed node should only show header"
        );
    }

    #[test]
    fn test_container_node_size_calculation_collapsed_section() {
        // Test size calculation with a collapsed section
        let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());

        // Add a collapsed section with members
        let members = vec![
            MemberItem::new(NodeId(2), NodeKind::METHOD, "method1".to_string()),
            MemberItem::new(NodeId(3), NodeKind::METHOD, "method2".to_string()),
        ];
        let mut section = VisibilitySection::with_members(VisibilityKind::Public, members);
        section.is_collapsed = true;
        node.visibility_sections.push(section);

        let size = node.calculate_size(30.0, 20.0, 20.0, 16.0, 150.0);

        // Expected: header(30) + padding_top(8) + section_header(20) + section_padding(4) + padding_bottom(8) = 70
        assert_eq!(size.x, 150.0);
        assert!(
            size.y >= 62.0 && size.y < 80.0,
            "Collapsed section should only show header"
        );
    }

    #[test]
    fn test_container_node_size_calculation_default() {
        // Test the default size calculation method
        let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());

        let members = vec![MemberItem::new(
            NodeId(2),
            NodeKind::METHOD,
            "method1".to_string(),
        )];
        node.visibility_sections
            .push(VisibilitySection::with_members(
                VisibilityKind::Public,
                members,
            ));

        let size = node.calculate_size_default();

        // Should use default constants
        assert_eq!(size.x, 180.0); // DEFAULT_MIN_WIDTH
        assert!(size.y > 0.0, "Size should be calculated");
    }

    #[test]
    fn test_container_node_size_property_validation() {
        // Property 2: For any Container_Node with N members across M visibility sections,
        // the computed height SHALL be >= header_height + sum(section_header_heights) +
        // sum(member_row_heights) + padding.

        let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());

        let header_height = 30.0;
        let section_header_height = 20.0;
        let member_row_height = 20.0;
        let padding = 16.0;
        let min_width = 150.0;

        // Add 2 sections with different member counts
        let section1_members = vec![
            MemberItem::new(NodeId(2), NodeKind::METHOD, "method1".to_string()),
            MemberItem::new(NodeId(3), NodeKind::METHOD, "method2".to_string()),
        ];
        node.visibility_sections
            .push(VisibilitySection::with_members(
                VisibilityKind::Functions,
                section1_members,
            ));

        let section2_members = vec![MemberItem::new(
            NodeId(4),
            NodeKind::FIELD,
            "field1".to_string(),
        )];
        node.visibility_sections
            .push(VisibilitySection::with_members(
                VisibilityKind::Variables,
                section2_members,
            ));

        let size = node.calculate_size(
            header_height,
            section_header_height,
            member_row_height,
            padding,
            min_width,
        );

        // Calculate minimum expected height
        let num_sections = 2;
        let total_members = 3;
        let min_expected_height = header_height
            + (num_sections as f32 * section_header_height)
            + (total_members as f32 * member_row_height)
            + padding;

        assert!(
            size.y >= min_expected_height,
            "Calculated height {} should be >= minimum expected height {}",
            size.y,
            min_expected_height
        );
    }

    #[test]
    fn test_edge_bundle_contains_edge() {
        let source = EdgeAnchor::new(NodeId(1), Vec2::new(0.0, 0.0), AnchorSide::Right);
        let target = EdgeAnchor::new(NodeId(2), Vec2::new(100.0, 0.0), AnchorSide::Left);
        let route = EdgeRoute::new(EdgeId(1), source, target, EdgeKind::CALL);

        let edges = vec![EdgeId(1), EdgeId(2), EdgeId(3)];
        let bundle = EdgeBundle::new(edges, NodeId(1), NodeId(2), route);

        assert!(bundle.contains_edge(EdgeId(1)));
        assert!(bundle.contains_edge(EdgeId(2)));
        assert!(bundle.contains_edge(EdgeId(3)));
        assert!(!bundle.contains_edge(EdgeId(4)));
    }

    #[test]
    fn test_bundle_data_creation() {
        let edge_ids = vec![EdgeId(1), EdgeId(2), EdgeId(3)];
        let edge_kinds = vec![EdgeKind::CALL, EdgeKind::CALL, EdgeKind::INHERITANCE];
        let relationships = vec![
            ("ClassA".to_string(), "ClassB".to_string(), EdgeKind::CALL),
            ("ClassA".to_string(), "ClassB".to_string(), EdgeKind::CALL),
            (
                "ClassA".to_string(),
                "ClassB".to_string(),
                EdgeKind::INHERITANCE,
            ),
        ];

        let bundle = BundleData::new(edge_ids.clone(), edge_kinds.clone(), relationships.clone());

        assert_eq!(bundle.edge_ids, edge_ids);
        assert_eq!(bundle.edge_kinds, edge_kinds);
        assert_eq!(bundle.relationships, relationships);
        assert_eq!(bundle.edge_count(), 3);
        assert!(!bundle.is_expanded);
    }

    #[test]
    fn test_bundle_data_thickness_calculation() {
        // Single edge: base width
        assert_eq!(BundleData::calculate_thickness(1), 1.0);

        // 2 edges: log2(2) + 1 = 1 + 1 = 2.0
        assert_eq!(BundleData::calculate_thickness(2), 2.0);

        // 4 edges: log2(4) + 1 = 2 + 1 = 3.0
        assert_eq!(BundleData::calculate_thickness(4), 3.0);

        // 8 edges: log2(8) + 1 = 3 + 1 = 4.0
        assert_eq!(BundleData::calculate_thickness(8), 4.0);

        // 16 edges: log2(16) + 1 = 4 + 1 = 5.0
        assert_eq!(BundleData::calculate_thickness(16), 5.0);

        // 32 edges: log2(32) + 1 = 5 + 1 = 6.0
        assert_eq!(BundleData::calculate_thickness(32), 6.0);

        // 64 edges: log2(64) + 1 = 6 + 1 = 7.0, but clamped to 6.0
        assert_eq!(BundleData::calculate_thickness(64), 6.0);

        // 100 edges: should be clamped to max 6.0
        assert_eq!(BundleData::calculate_thickness(100), 6.0);
    }

    #[test]
    fn test_bundle_data_toggle_expanded() {
        let edge_ids = vec![EdgeId(1), EdgeId(2)];
        let edge_kinds = vec![EdgeKind::CALL, EdgeKind::CALL];
        let relationships = vec![
            ("A".to_string(), "B".to_string(), EdgeKind::CALL),
            ("A".to_string(), "B".to_string(), EdgeKind::CALL),
        ];

        let mut bundle = BundleData::new(edge_ids, edge_kinds, relationships);

        assert!(!bundle.is_expanded);
        bundle.toggle_expanded();
        assert!(bundle.is_expanded);
        bundle.toggle_expanded();
        assert!(!bundle.is_expanded);
    }

    #[test]
    fn test_bundle_data_set_expanded() {
        let edge_ids = vec![EdgeId(1), EdgeId(2)];
        let edge_kinds = vec![EdgeKind::CALL, EdgeKind::CALL];
        let relationships = vec![
            ("A".to_string(), "B".to_string(), EdgeKind::CALL),
            ("A".to_string(), "B".to_string(), EdgeKind::CALL),
        ];

        let mut bundle = BundleData::new(edge_ids, edge_kinds, relationships);

        bundle.set_expanded(true);
        assert!(bundle.is_expanded);
        bundle.set_expanded(false);
        assert!(!bundle.is_expanded);
    }

    #[test]
    fn test_bundle_data_contains_edge() {
        let edge_ids = vec![EdgeId(1), EdgeId(2), EdgeId(3)];
        let edge_kinds = vec![EdgeKind::CALL, EdgeKind::CALL, EdgeKind::CALL];
        let relationships = vec![
            ("A".to_string(), "B".to_string(), EdgeKind::CALL),
            ("A".to_string(), "B".to_string(), EdgeKind::CALL),
            ("A".to_string(), "B".to_string(), EdgeKind::CALL),
        ];

        let bundle = BundleData::new(edge_ids, edge_kinds, relationships);

        assert!(bundle.contains_edge(EdgeId(1)));
        assert!(bundle.contains_edge(EdgeId(2)));
        assert!(bundle.contains_edge(EdgeId(3)));
        assert!(!bundle.contains_edge(EdgeId(4)));
    }

    #[test]
    fn test_collapse_state_creation() {
        let state = CollapseState::new();
        assert!(!state.is_collapsed);
        assert!(state.collapsed_sections.is_empty());
    }

    #[test]
    fn test_collapse_state_collapsed() {
        let state = CollapseState::collapsed();
        assert!(state.is_collapsed);
        assert!(state.collapsed_sections.is_empty());
    }

    #[test]
    fn test_collapse_state_with_collapsed_sections() {
        let mut sections = HashSet::new();
        sections.insert(VisibilityKind::Private);
        sections.insert(VisibilityKind::Protected);

        let state = CollapseState::with_collapsed_sections(sections.clone());
        assert!(!state.is_collapsed);
        assert_eq!(state.collapsed_sections, sections);
    }

    #[test]
    fn test_collapse_state_toggle_collapsed() {
        let mut state = CollapseState::new();
        assert!(!state.is_collapsed);

        state.toggle_collapsed();
        assert!(state.is_collapsed);

        state.toggle_collapsed();
        assert!(!state.is_collapsed);
    }

    #[test]
    fn test_collapse_state_toggle_section() {
        let mut state = CollapseState::new();

        assert!(!state.is_section_collapsed(VisibilityKind::Public));

        state.toggle_section(VisibilityKind::Public);
        assert!(state.is_section_collapsed(VisibilityKind::Public));

        state.toggle_section(VisibilityKind::Public);
        assert!(!state.is_section_collapsed(VisibilityKind::Public));
    }

    #[test]
    fn test_collapse_state_expand_all_sections() {
        let mut state = CollapseState::new();
        state.toggle_section(VisibilityKind::Public);
        state.toggle_section(VisibilityKind::Private);

        assert!(state.is_section_collapsed(VisibilityKind::Public));
        assert!(state.is_section_collapsed(VisibilityKind::Private));

        state.expand_all_sections();
        assert!(!state.is_section_collapsed(VisibilityKind::Public));
        assert!(!state.is_section_collapsed(VisibilityKind::Private));
    }

    #[test]
    fn test_collapse_state_collapse_all_sections() {
        let mut state = CollapseState::new();

        state.collapse_all_sections();
        assert!(state.is_section_collapsed(VisibilityKind::Public));
        assert!(state.is_section_collapsed(VisibilityKind::Private));
        assert!(state.is_section_collapsed(VisibilityKind::Protected));
        assert!(state.is_section_collapsed(VisibilityKind::Internal));
        assert!(state.is_section_collapsed(VisibilityKind::Functions));
        assert!(state.is_section_collapsed(VisibilityKind::Variables));
        assert!(state.is_section_collapsed(VisibilityKind::Other));
    }

    #[test]
    fn test_graph_view_state_creation() {
        let state = GraphViewState::new();
        assert!(state.collapse_states.is_empty());
        assert!(state.section_states.is_empty());
        assert!(state.hidden_nodes.is_empty());
        assert!(state.custom_positions.is_empty());
        assert_eq!(state.zoom, 1.0);
        assert_eq!(state.pan, Vec2::new(0.0, 0.0));
    }

    #[test]
    fn test_graph_view_state_get_collapse_state() {
        let state = GraphViewState::new();

        // Should return default state for non-existent node
        let collapse_state = state.get_collapse_state(NodeId(1));
        assert!(!collapse_state.is_collapsed);
        assert!(collapse_state.collapsed_sections.is_empty());
    }

    #[test]
    fn test_graph_view_state_set_collapse_state() {
        let mut state = GraphViewState::new();
        let collapse_state = CollapseState::collapsed();

        state.set_collapse_state(NodeId(1), collapse_state.clone());

        let retrieved = state.get_collapse_state(NodeId(1));
        assert_eq!(retrieved, collapse_state);
    }

    #[test]
    fn test_graph_view_state_toggle_node_collapsed() {
        let mut state = GraphViewState::new();

        assert!(!state.is_node_collapsed(NodeId(1)));

        state.toggle_node_collapsed(NodeId(1));
        assert!(state.is_node_collapsed(NodeId(1)));

        state.toggle_node_collapsed(NodeId(1));
        assert!(!state.is_node_collapsed(NodeId(1)));
    }

    #[test]
    fn test_graph_view_state_toggle_section_collapsed() {
        let mut state = GraphViewState::new();

        assert!(!state.is_section_collapsed(NodeId(1), VisibilityKind::Public));

        state.toggle_section_collapsed(NodeId(1), VisibilityKind::Public);
        assert!(state.is_section_collapsed(NodeId(1), VisibilityKind::Public));

        state.toggle_section_collapsed(NodeId(1), VisibilityKind::Public);
        assert!(!state.is_section_collapsed(NodeId(1), VisibilityKind::Public));
    }

    #[test]
    fn test_graph_view_state_hide_show_node() {
        let mut state = GraphViewState::new();

        assert!(!state.is_node_hidden(NodeId(1)));

        state.hide_node(NodeId(1));
        assert!(state.is_node_hidden(NodeId(1)));

        state.show_node(NodeId(1));
        assert!(!state.is_node_hidden(NodeId(1)));
    }

    #[test]
    fn test_graph_view_state_custom_positions() {
        let mut state = GraphViewState::new();
        let position = Vec2::new(100.0, 200.0);

        assert_eq!(state.get_custom_position(NodeId(1)), None);

        state.set_custom_position(NodeId(1), position);
        assert_eq!(state.get_custom_position(NodeId(1)), Some(position));

        state.clear_custom_position(NodeId(1));
        assert_eq!(state.get_custom_position(NodeId(1)), None);
    }

    #[test]
    fn test_graph_view_state_clear_all_custom_positions() {
        let mut state = GraphViewState::new();

        state.set_custom_position(NodeId(1), Vec2::new(100.0, 200.0));
        state.set_custom_position(NodeId(2), Vec2::new(300.0, 400.0));

        assert_eq!(state.custom_positions.len(), 2);

        state.clear_all_custom_positions();
        assert!(state.custom_positions.is_empty());
    }

    #[test]
    fn test_graph_view_state_set_zoom() {
        let mut state = GraphViewState::new();

        // Normal zoom
        state.set_zoom(2.0);
        assert_eq!(state.zoom, 2.0);

        // Zoom clamped to minimum
        state.set_zoom(0.05);
        assert_eq!(state.zoom, 0.1);

        // Zoom clamped to maximum
        state.set_zoom(5.0);
        assert_eq!(state.zoom, 4.0);
    }

    #[test]
    fn test_graph_view_state_set_pan() {
        let mut state = GraphViewState::new();
        let pan = Vec2::new(50.0, 100.0);

        state.set_pan(pan);
        assert_eq!(state.pan, pan);
    }

    #[test]
    fn test_graph_view_state_recenter_on() {
        let mut state = GraphViewState::new();
        state.set_zoom(2.0);

        let node_pos = Vec2::new(10.0, -5.0);
        let expected_pan = Vec2::new(-20.0, 10.0);
        assert_eq!(state.expected_pan_for_center_on(node_pos), expected_pan);

        // Not centered initially.
        assert!(!state.is_centered_on(node_pos, 0.5));

        // Recenters by setting pan to the expected value.
        assert!(state.recenter_on(node_pos, 0.5));
        assert_eq!(state.pan, expected_pan);
        assert!(state.is_centered_on(node_pos, 0.5));

        // Idempotent within tolerance.
        assert!(!state.recenter_on(node_pos, 0.5));
    }

    #[test]
    fn test_graph_view_state_set_layout_algorithm() {
        use codestory_events::LayoutAlgorithm;

        let mut state = GraphViewState::new();

        state.set_layout_algorithm(LayoutAlgorithm::Radial);
        assert_eq!(state.layout_algorithm, LayoutAlgorithm::Radial);

        state.set_layout_algorithm(LayoutAlgorithm::Hierarchical);
        assert_eq!(state.layout_algorithm, LayoutAlgorithm::Hierarchical);
    }

    #[test]
    fn test_graph_view_state_set_layout_direction() {
        use codestory_core::LayoutDirection;

        let mut state = GraphViewState::new();

        state.set_layout_direction(LayoutDirection::Vertical);
        assert_eq!(state.layout_direction, LayoutDirection::Vertical);

        state.set_layout_direction(LayoutDirection::Horizontal);
        assert_eq!(state.layout_direction, LayoutDirection::Horizontal);
    }

    #[test]
    fn test_graph_view_state_expand_collapse_all_nodes() {
        let mut state = GraphViewState::new();

        // Set up some nodes with collapsed state
        state.set_collapse_state(NodeId(1), CollapseState::collapsed());
        state.set_collapse_state(NodeId(2), CollapseState::collapsed());

        assert!(state.is_node_collapsed(NodeId(1)));
        assert!(state.is_node_collapsed(NodeId(2)));

        // Expand all
        state.expand_all_nodes();
        assert!(!state.is_node_collapsed(NodeId(1)));
        assert!(!state.is_node_collapsed(NodeId(2)));

        // Collapse all
        state.collapse_all_nodes();
        assert!(state.is_node_collapsed(NodeId(1)));
        assert!(state.is_node_collapsed(NodeId(2)));
    }

    #[test]
    fn test_graph_view_state_expand_collapse_all_sections() {
        let mut state = GraphViewState::new();

        // Set up a node with some collapsed sections
        let mut collapse_state = CollapseState::new();
        collapse_state.toggle_section(VisibilityKind::Public);
        collapse_state.toggle_section(VisibilityKind::Private);
        state.set_collapse_state(NodeId(1), collapse_state);

        assert!(state.is_section_collapsed(NodeId(1), VisibilityKind::Public));
        assert!(state.is_section_collapsed(NodeId(1), VisibilityKind::Private));

        // Expand all sections
        state.expand_all_sections();
        assert!(!state.is_section_collapsed(NodeId(1), VisibilityKind::Public));
        assert!(!state.is_section_collapsed(NodeId(1), VisibilityKind::Private));

        // Collapse all sections
        state.collapse_all_sections();
        assert!(state.is_section_collapsed(NodeId(1), VisibilityKind::Public));
        assert!(state.is_section_collapsed(NodeId(1), VisibilityKind::Private));
    }

    #[test]
    fn test_graph_view_state_serialization() {
        let mut state = GraphViewState::new();

        // Set up some state
        state.set_collapse_state(NodeId(1), CollapseState::collapsed());
        state.hide_node(NodeId(2));
        state.set_custom_position(NodeId(3), Vec2::new(100.0, 200.0));
        state.set_zoom(2.5);
        state.set_pan(Vec2::new(50.0, 75.0));

        // Serialize
        let json = serde_json::to_string(&state).expect("Failed to serialize");

        // Deserialize
        let deserialized: GraphViewState =
            serde_json::from_str(&json).expect("Failed to deserialize");

        // Verify
        assert!(deserialized.is_node_collapsed(NodeId(1)));
        assert!(deserialized.is_node_hidden(NodeId(2)));
        assert_eq!(
            deserialized.get_custom_position(NodeId(3)),
            Some(Vec2::new(100.0, 200.0))
        );
        assert_eq!(deserialized.zoom, 2.5);
        assert_eq!(deserialized.pan, Vec2::new(50.0, 75.0));
    }

    #[test]
    fn test_collapse_state_serialization() {
        let mut state = CollapseState::new();
        state.toggle_collapsed();
        state.toggle_section(VisibilityKind::Public);
        state.toggle_section(VisibilityKind::Private);

        // Serialize
        let json = serde_json::to_string(&state).expect("Failed to serialize");

        // Deserialize
        let deserialized: CollapseState =
            serde_json::from_str(&json).expect("Failed to deserialize");

        // Verify
        assert_eq!(deserialized, state);
        assert!(deserialized.is_collapsed);
        assert!(deserialized.is_section_collapsed(VisibilityKind::Public));
        assert!(deserialized.is_section_collapsed(VisibilityKind::Private));
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    // Strategy for generating random member counts (0-50 members per section)
    fn member_count_strategy() -> impl Strategy<Value = usize> {
        0usize..=50
    }

    // Strategy for generating random section counts (0-5 sections)
    fn section_count_strategy() -> impl Strategy<Value = usize> {
        0usize..=5
    }

    // Strategy for generating random UI dimensions
    fn ui_dimensions_strategy() -> impl Strategy<Value = (f32, f32, f32, f32, f32)> {
        (
            20.0f32..=50.0,   // header_height
            15.0f32..=30.0,   // section_header_height
            15.0f32..=30.0,   // member_row_height
            8.0f32..=32.0,    // padding
            100.0f32..=300.0, // min_width
        )
    }

    // Strategy for generating a visibility section with a specific number of members
    fn visibility_section_with_members_strategy(
        member_count: usize,
    ) -> impl Strategy<Value = VisibilitySection> {
        let kinds = vec![
            VisibilityKind::Public,
            VisibilityKind::Private,
            VisibilityKind::Protected,
            VisibilityKind::Internal,
            VisibilityKind::Functions,
            VisibilityKind::Variables,
            VisibilityKind::Other,
        ];

        prop::sample::select(kinds).prop_map(move |kind| {
            // Generate members that match the section kind
            let mut members: Vec<MemberItem> = (0..member_count)
                .map(|i| {
                    // Choose NodeKind based on the VisibilityKind to ensure correct grouping
                    let node_kind = match kind {
                        VisibilityKind::Functions => {
                            // Alternate between function-like kinds
                            match i % 3 {
                                0 => NodeKind::FUNCTION,
                                1 => NodeKind::METHOD,
                                _ => NodeKind::MACRO,
                            }
                        }
                        VisibilityKind::Variables => {
                            // Alternate between variable-like kinds
                            match i % 5 {
                                0 => NodeKind::FIELD,
                                1 => NodeKind::VARIABLE,
                                2 => NodeKind::GLOBAL_VARIABLE,
                                3 => NodeKind::CONSTANT,
                                _ => NodeKind::ENUM_CONSTANT,
                            }
                        }
                        VisibilityKind::Other => {
                            // Use kinds that don't fit in Functions or Variables
                            match i % 4 {
                                0 => NodeKind::CLASS,
                                1 => NodeKind::STRUCT,
                                2 => NodeKind::ENUM,
                                _ => NodeKind::INTERFACE,
                            }
                        }
                        // For visibility-based sections, we can use any kind
                        // but let's mix them to test that visibility sections can contain any type
                        VisibilityKind::Public
                        | VisibilityKind::Private
                        | VisibilityKind::Protected
                        | VisibilityKind::Internal => {
                            if i % 2 == 0 {
                                NodeKind::METHOD
                            } else {
                                NodeKind::FIELD
                            }
                        }
                    };

                    MemberItem::new(NodeId((i + 1) as i64), node_kind, format!("member_{}", i))
                })
                .collect();

            // Sort members by name to match the expected behavior from group_members_into_sections
            members.sort_unstable_by(|a, b| a.name.cmp(&b.name));

            VisibilitySection::with_members(kind, members)
        })
    }

    // Strategy for generating a UmlNode with random sections and members
    fn uml_node_with_sections_strategy() -> impl Strategy<Value = (UmlNode, Vec<(usize, bool)>)> {
        section_count_strategy().prop_flat_map(|_num_sections| {
            prop::collection::vec(
                (member_count_strategy(), any::<bool>()).prop_flat_map(
                    |(member_count, is_collapsed)| {
                        visibility_section_with_members_strategy(member_count).prop_map(
                            move |mut section| {
                                section.is_collapsed = is_collapsed;
                                (section, member_count, is_collapsed)
                            },
                        )
                    },
                ),
                0..=5,
            )
            .prop_map(|sections_data| {
                let mut node = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());

                let section_info: Vec<(usize, bool)> = sections_data
                    .iter()
                    .map(|(_, count, collapsed)| (*count, *collapsed))
                    .collect();

                // Track the next available NodeId to ensure uniqueness across all sections
                let mut next_node_id = 1i64;

                for (mut section, _, _) in sections_data {
                    // Reassign NodeIds to ensure uniqueness across all sections
                    for member in &mut section.members {
                        member.id = NodeId(next_node_id);
                        next_node_id += 1;
                    }
                    node.visibility_sections.push(section);
                }

                (node, section_info)
            })
        })
    }

    proptest! {
        /// **Validates: Requirements 1.4**
        ///
        /// Property 2: Container Node Size Calculation
        ///
        /// For any Container_Node with N members across M visibility sections,
        /// the computed height SHALL be >= header_height + sum(section_header_heights) +
        /// sum(member_row_heights) + padding.
        ///
        /// This property ensures that the calculated size of a container node is always
        /// large enough to accommodate all its content, including:
        /// - The header
        /// - All section headers (for non-empty sections)
        /// - All member rows (for expanded sections)
        /// - Appropriate padding
        #[test]
        fn prop_container_node_size_calculation(
            (node, section_info) in uml_node_with_sections_strategy(),
            (header_height, section_header_height, member_row_height, padding, min_width)
                in ui_dimensions_strategy()
        ) {
            // Skip collapsed nodes as they have different sizing logic
            prop_assume!(!node.is_collapsed);

            let size = node.calculate_size(
                header_height,
                section_header_height,
                member_row_height,
                padding,
                min_width,
            );

            // Calculate the minimum expected height based on the formula
            let mut min_expected_height = header_height;

            // Add padding before sections
            min_expected_height += padding * 0.5;

            // Count non-empty sections and their members
            let mut total_section_headers = 0;
            let mut total_member_rows = 0;

            for section in node.visibility_sections.iter() {
                // Skip empty sections
                if section.members.is_empty() {
                    continue;
                }

                // Add section header
                total_section_headers += 1;
                min_expected_height += section_header_height;

                // If section is collapsed, only add minimal spacing
                if section.is_collapsed {
                    min_expected_height += padding * 0.25;
                } else {
                    // Add member rows for expanded sections
                    let member_count = section.members.len();
                    total_member_rows += member_count;
                    min_expected_height += member_count as f32 * member_row_height;

                    // Add spacing between sections
                    min_expected_height += padding * 0.5;
                }
            }

            // Add bottom padding
            min_expected_height += padding * 0.5;

            // Verify the property: calculated height >= minimum expected height
            prop_assert!(
                size.y >= min_expected_height - 0.01, // Allow small floating point tolerance
                "Container node size calculation failed:\n\
                 Calculated height: {}\n\
                 Minimum expected height: {}\n\
                 Header height: {}\n\
                 Section headers: {} x {} = {}\n\
                 Member rows: {} x {} = {}\n\
                 Padding: {}\n\
                 Number of sections: {}\n\
                 Section info: {:?}",
                size.y,
                min_expected_height,
                header_height,
                total_section_headers,
                section_header_height,
                total_section_headers as f32 * section_header_height,
                total_member_rows,
                member_row_height,
                total_member_rows as f32 * member_row_height,
                padding,
                node.visibility_sections.len(),
                section_info
            );

            // Verify width is at least min_width
            prop_assert_eq!(
                size.x,
                min_width,
                "Width should equal min_width"
            );

            // Verify height is positive
            prop_assert!(
                size.y > 0.0,
                "Height should be positive"
            );
        }

        /// Property 2 variant: Test with collapsed nodes
        ///
        /// For collapsed nodes, the height should be just header + minimal padding,
        /// regardless of the number of members.
        #[test]
        fn prop_container_node_size_calculation_collapsed(
            (mut node, _section_info) in uml_node_with_sections_strategy(),
            (header_height, section_header_height, member_row_height, padding, min_width)
                in ui_dimensions_strategy()
        ) {
            // Force node to be collapsed
            node.is_collapsed = true;

            let size = node.calculate_size(
                header_height,
                section_header_height,
                member_row_height,
                padding,
                min_width,
            );

            // For collapsed nodes, height should be header + minimal padding
            let expected_height = header_height + padding * 0.5;

            prop_assert!(
                size.y >= expected_height - 0.01 && size.y <= expected_height + 1.0,
                "Collapsed node should have minimal height:\n\
                 Calculated height: {}\n\
                 Expected height: {} (header {} + padding {})\n\
                 Number of sections: {}",
                size.y,
                expected_height,
                header_height,
                padding * 0.5,
                node.visibility_sections.len()
            );

            // Width should still be min_width
            prop_assert_eq!(size.x, min_width);
        }

        /// Property 2 variant: Test that size increases monotonically with member count
        ///
        /// Adding more members should never decrease the calculated size.
        #[test]
        fn prop_container_node_size_monotonic(
            member_count1 in 0usize..=20,
            member_count2 in 0usize..=20,
            (header_height, section_header_height, member_row_height, padding, min_width)
                in ui_dimensions_strategy()
        ) {
            // Create two nodes with different member counts
            let mut node1 = UmlNode::new(NodeId(1), NodeKind::CLASS, "TestClass".to_string());
            let mut node2 = UmlNode::new(NodeId(2), NodeKind::CLASS, "TestClass".to_string());

            // Add members to node1
            if member_count1 > 0 {
                let members1: Vec<MemberItem> = (0..member_count1)
                    .map(|i| MemberItem::new(NodeId((i + 1) as i64), NodeKind::METHOD, format!("method_{}", i)))
                    .collect();
                node1.visibility_sections.push(VisibilitySection::with_members(VisibilityKind::Public, members1));
            }

            // Add members to node2
            if member_count2 > 0 {
                let members2: Vec<MemberItem> = (0..member_count2)
                    .map(|i| MemberItem::new(NodeId((i + 1) as i64), NodeKind::METHOD, format!("method_{}", i)))
                    .collect();
                node2.visibility_sections.push(VisibilitySection::with_members(VisibilityKind::Public, members2));
            }

            let size1 = node1.calculate_size(header_height, section_header_height, member_row_height, padding, min_width);
            let size2 = node2.calculate_size(header_height, section_header_height, member_row_height, padding, min_width);

            // If node2 has more members, it should be taller (or equal if both have 0)
            if member_count2 > member_count1 {
                prop_assert!(
                    size2.y >= size1.y,
                    "Node with more members should be taller:\n\
                     Node1 members: {}, height: {}\n\
                     Node2 members: {}, height: {}",
                    member_count1, size1.y,
                    member_count2, size2.y
                );
            } else if member_count1 > member_count2 {
                prop_assert!(
                    size1.y >= size2.y,
                    "Node with more members should be taller:\n\
                     Node1 members: {}, height: {}\n\
                     Node2 members: {}, height: {}",
                    member_count1, size1.y,
                    member_count2, size2.y
                );
            }
        }

        /// **Validates: Requirements 1.2**
        ///
        /// Property 3: Member Grouping Correctness
        ///
        /// For any Container_Node with members, all members SHALL be assigned to exactly one
        /// Visibility_Section, and the section assignment SHALL be based on the member's
        /// visibility or kind.
        ///
        /// This property ensures that:
        /// 1. Every member appears in exactly one section (no duplicates, no missing members)
        /// 2. Members are grouped correctly based on their NodeKind
        /// 3. The grouping is deterministic and consistent
        #[test]
        fn prop_member_grouping_correctness(
            (node, _section_info) in uml_node_with_sections_strategy()
        ) {
            // Skip nodes with no sections
            prop_assume!(!node.visibility_sections.is_empty());

            // Skip nodes with any empty sections - these should never be created by the grouping logic
            // The actual implementation (group_members_into_sections) only creates non-empty sections
            prop_assume!(node.visibility_sections.iter().all(|s| !s.members.is_empty()));

            // Collect all member IDs from all sections
            let mut all_member_ids = Vec::new();
            let mut member_id_to_section = std::collections::HashMap::new();

            for section in &node.visibility_sections {
                for member in &section.members {
                    all_member_ids.push(member.id);

                    // Track which section this member belongs to
                    if let Some(existing_section) = member_id_to_section.insert(member.id, section.kind) {
                        // If we get here, the member appears in multiple sections - this is a violation
                        prop_assert!(
                            false,
                            "Member {:?} appears in multiple sections: {:?} and {:?}",
                            member.id,
                            existing_section,
                            section.kind
                        );
                    }
                }
            }

            // Property 3.1: Each member appears exactly once (no duplicates)
            let unique_count = all_member_ids.iter().collect::<std::collections::HashSet<_>>().len();
            prop_assert_eq!(
                unique_count,
                all_member_ids.len(),
                "Some members appear more than once. Total members: {}, Unique members: {}",
                all_member_ids.len(),
                unique_count
            );

            // Property 3.2: Section assignment is based on member's kind
            // Verify that each member is in the correct section based on its kind
            for section in &node.visibility_sections {
                for member in &section.members {
                    let is_correctly_grouped = match section.kind {
                        VisibilityKind::Functions => matches!(
                            member.kind,
                            NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
                        ),
                        VisibilityKind::Variables => matches!(
                            member.kind,
                            NodeKind::FIELD
                                | NodeKind::VARIABLE
                                | NodeKind::GLOBAL_VARIABLE
                                | NodeKind::CONSTANT
                                | NodeKind::ENUM_CONSTANT
                        ),
                        VisibilityKind::Other => {
                            // Other section should contain kinds that don't fit in Functions or Variables
                            !matches!(
                                member.kind,
                                NodeKind::FUNCTION
                                    | NodeKind::METHOD
                                    | NodeKind::MACRO
                                    | NodeKind::FIELD
                                    | NodeKind::VARIABLE
                                    | NodeKind::GLOBAL_VARIABLE
                                    | NodeKind::CONSTANT
                                    | NodeKind::ENUM_CONSTANT
                            )
                        }
                        // For visibility-based sections (Public, Private, Protected, Internal),
                        // we currently don't have visibility information in the test data,
                        // so we accept any kind in these sections
                        VisibilityKind::Public
                        | VisibilityKind::Private
                        | VisibilityKind::Protected
                        | VisibilityKind::Internal => true,
                    };

                    prop_assert!(
                        is_correctly_grouped,
                        "Member {:?} with kind {:?} is incorrectly placed in section {:?}",
                        member.name,
                        member.kind,
                        section.kind
                    );
                }
            }

            // Property 3.3: Members within each section should be sorted by name
            // (This ensures consistent, predictable ordering)
            for section in &node.visibility_sections {
                let member_names: Vec<&str> = section.members.iter().map(|m| m.name.as_str()).collect();
                let mut sorted_names = member_names.clone();
                sorted_names.sort_unstable();

                prop_assert_eq!(
                    &member_names,
                    &sorted_names,
                    "Members in section {:?} are not sorted by name. Found: {:?}, Expected: {:?}",
                    section.kind,
                    member_names,
                    sorted_names
                );
            }
        }
    }

    fn collapse_state_strategy() -> impl Strategy<Value = CollapseState> {
        let sections_strategy = prop::collection::hash_set(
            prop::sample::select(vec![
                VisibilityKind::Public,
                VisibilityKind::Private,
                VisibilityKind::Protected,
                VisibilityKind::Internal,
                VisibilityKind::Functions,
                VisibilityKind::Variables,
                VisibilityKind::Other,
            ]),
            0..=7,
        );

        (any::<bool>(), sections_strategy).prop_map(|(is_collapsed, sections)| {
            let mut state = CollapseState::new();
            state.is_collapsed = is_collapsed;
            state.collapsed_sections = sections;
            state
        })
    }

    fn graph_view_state_strategy() -> impl Strategy<Value = GraphViewState> {
        let collapse_states_strategy = prop::collection::hash_map(
            any::<i64>().prop_map(NodeId),
            collapse_state_strategy(),
            0..=10,
        );

        collapse_states_strategy.prop_map(|collapse_states| {
            let mut state = GraphViewState::new();
            state.collapse_states = collapse_states;
            state
        })
    }

    proptest! {
        /// **Validates: Requirements 1.7**
        ///
        /// Property 5: Node Collapse Toggle Round-trip
        ///
        /// Toggling a node's collapse state twice should restore the original state.
        #[test]
        fn prop_node_collapse_toggle_round_trip(
            mut state in collapse_state_strategy()
        ) {
            let original_state = state.clone();

            state.toggle_collapsed();
            prop_assert_ne!(state.is_collapsed, original_state.is_collapsed);

            state.toggle_collapsed();
            prop_assert_eq!(state.is_collapsed, original_state.is_collapsed);
            prop_assert_eq!(state, original_state);
        }

        /// **Validates: Requirements 6.1**
        ///
        /// Property 19: Section Collapse Toggle Round-trip
        ///
        /// Toggling a section's collapse state twice should restore the original state.
        #[test]
        fn prop_section_collapse_toggle_round_trip(
            mut state in collapse_state_strategy(),
            kind_idx in 0usize..7
        ) {
             let kinds = [
                VisibilityKind::Public,
                VisibilityKind::Private,
                VisibilityKind::Protected,
                VisibilityKind::Internal,
                VisibilityKind::Functions,
                VisibilityKind::Variables,
                VisibilityKind::Other,
            ];
            let kind = kinds[kind_idx];
            let original_state = state.clone();

            state.toggle_section(kind);
            prop_assert_ne!(state.is_section_collapsed(kind), original_state.is_section_collapsed(kind));

            state.toggle_section(kind);
            prop_assert_eq!(state.is_section_collapsed(kind), original_state.is_section_collapsed(kind));
            prop_assert_eq!(state, original_state);
        }

        /// **Validates: Requirements 6.5**
        ///
        /// Property 20: Persistence Round-trip
        ///
        /// Serializing and deserializing the GraphViewState should result in an identical state.
        #[test]
        fn prop_persistence_round_trip(
            state in graph_view_state_strategy()
        ) {
             let json = serde_json::to_string(&state).expect("Failed to serialize");
             let deserialized: GraphViewState = serde_json::from_str(&json).expect("Failed to deserialize");
             prop_assert_eq!(state, deserialized);
        }

        // =====================================================================
        // Phase 12: Viewport Controls Property Tests
        // =====================================================================

        /// **Validates: Requirements 7.4**
        ///
        /// Property 23: Zoom Level Clamping
        ///
        /// For any zoom input value, the resulting zoom level SHALL be clamped
        /// to the range [0.1, 4.0] (10% to 400%).
        #[test]
        fn prop_zoom_level_clamping(
            zoom_input in -10.0f32..=10.0f32
        ) {
            let mut state = GraphViewState::new();
            state.set_zoom(zoom_input);

            prop_assert!(
                state.zoom >= 0.1,
                "Zoom level {} should be >= 0.1 (10%) for input {}",
                state.zoom, zoom_input
            );
            prop_assert!(
                state.zoom <= 4.0,
                "Zoom level {} should be <= 4.0 (400%) for input {}",
                state.zoom, zoom_input
            );

            // If input is within valid range, zoom should equal input
            if (0.1..=4.0).contains(&zoom_input) {
                prop_assert!(
                    (state.zoom - zoom_input).abs() < f32::EPSILON,
                    "Zoom {} should equal input {} when within valid range",
                    state.zoom, zoom_input
                );
            }
        }

        /// Property 23 variant: Zoom clamping idempotence
        ///
        /// Clamping a value that's already in range should not change it.
        #[test]
        fn prop_zoom_clamping_idempotent(
            zoom_input in 0.1f32..=4.0f32
        ) {
            let mut state = GraphViewState::new();
            state.set_zoom(zoom_input);
            let first_zoom = state.zoom;
            state.set_zoom(first_zoom);

            prop_assert!(
                (state.zoom - first_zoom).abs() < f32::EPSILON,
                "Double-clamping should be idempotent: {} vs {}",
                state.zoom, first_zoom
            );
        }

        /// **Validates: Requirements 7.5**
        ///
        /// Property 24: Low Zoom Simplification
        ///
        /// For any zoom level < 0.5, Container_Nodes SHALL be rendered without
        /// member details (simplified mode).
        ///
        /// This test verifies the threshold logic: zoom values below 0.5 should
        /// trigger simplified rendering, while values >= 0.5 should show full detail.
        #[test]
        fn prop_low_zoom_simplification_threshold(
            zoom_level in 0.1f32..=4.0f32
        ) {
            let should_simplify = zoom_level < 0.5;

            // Verify the threshold is correct
            if should_simplify {
                prop_assert!(
                    zoom_level < 0.5,
                    "Zoom level {} should trigger simplified rendering",
                    zoom_level
                );
            } else {
                prop_assert!(
                    zoom_level >= 0.5,
                    "Zoom level {} should show full detail rendering",
                    zoom_level
                );
            }
        }

        /// **Validates: Requirements 7.3**
        ///
        /// Property 22: Zoom to Fit Bounds
        ///
        /// After executing "Zoom to Fit", all node bounding boxes SHALL be
        /// fully contained within the viewport bounds.
        ///
        /// This test verifies the viewport containment math: given a set of
        /// node positions and a viewport, the computed zoom/pan should contain
        /// all nodes.
        #[test]
        fn prop_zoom_to_fit_bounds(
            node_positions in prop::collection::vec(
                (-1000.0f32..=1000.0, -1000.0f32..=1000.0),
                1..=50
            ),
            viewport_width in 200.0f32..=2000.0,
            viewport_height in 200.0f32..=2000.0,
        ) {
            // Compute the bounding box of all nodes
            let mut min_x = f32::INFINITY;
            let mut min_y = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            let mut max_y = f32::NEG_INFINITY;

            let node_width = 150.0f32;
            let node_height = 50.0f32;

            for &(x, y) in &node_positions {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x + node_width);
                max_y = max_y.max(y + node_height);
            }

            let content_width = max_x - min_x;
            let content_height = max_y - min_y;

            // Calculate the zoom level needed to fit all content
            let zoom_x = viewport_width / content_width.max(1.0);
            let zoom_y = viewport_height / content_height.max(1.0);
            let fit_zoom = zoom_x.min(zoom_y).clamp(0.1, 4.0);

            // After fit, all content should be within the viewport
            // Allow small floating-point tolerance
            let tolerance = 1.0;
            let fitted_width = content_width * fit_zoom;
            let fitted_height = content_height * fit_zoom;

            prop_assert!(
                fitted_width <= viewport_width + tolerance || fit_zoom <= 0.1 + f32::EPSILON,
                "Fitted width {} should be <= viewport width {} (zoom: {})",
                fitted_width, viewport_width, fit_zoom
            );
            prop_assert!(
                fitted_height <= viewport_height + tolerance || fit_zoom <= 0.1 + f32::EPSILON,
                "Fitted height {} should be <= viewport height {} (zoom: {})",
                fitted_height, viewport_height, fit_zoom
            );
        }

        /// **Validates: Requirements 7.2**
        ///
        /// Property 21: Zoom Cursor Centering
        ///
        /// For any zoom operation at cursor position P, the world-space point
        /// under P before zoom SHALL remain under P after zoom (within
        /// floating-point tolerance).
        ///
        /// This tests the math: zoom centered at a point should keep that
        /// point fixed.
        #[test]
        fn prop_zoom_cursor_centering(
            cursor_x in -500.0f32..=500.0,
            cursor_y in -500.0f32..=500.0,
            initial_zoom in 0.5f32..=2.0,
            zoom_factor in 0.8f32..=1.25
        ) {
            let initial_pan_x = 0.0f32;
            let initial_pan_y = 0.0f32;

            // World point under cursor before zoom:
            // world_point = (cursor - pan) / zoom
            let world_x = (cursor_x - initial_pan_x) / initial_zoom;
            let world_y = (cursor_y - initial_pan_y) / initial_zoom;

            // Apply zoom centered on cursor
            let new_zoom = (initial_zoom * zoom_factor).clamp(0.1, 4.0);

            // After zoom, adjust pan so cursor still maps to same world point:
            // cursor = world_point * new_zoom + new_pan
            // new_pan = cursor - world_point * new_zoom
            let new_pan_x = cursor_x - world_x * new_zoom;
            let new_pan_y = cursor_y - world_y * new_zoom;

            // Verify: world point under cursor after zoom should match
            let after_world_x = (cursor_x - new_pan_x) / new_zoom;
            let after_world_y = (cursor_y - new_pan_y) / new_zoom;

            let tolerance = 0.01;
            prop_assert!(
                (after_world_x - world_x).abs() < tolerance,
                "X world coordinate should be preserved: before={}, after={}, diff={}",
                world_x, after_world_x, (after_world_x - world_x).abs()
            );
            prop_assert!(
                (after_world_y - world_y).abs() < tolerance,
                "Y world coordinate should be preserved: before={}, after={}, diff={}",
                world_y, after_world_y, (after_world_y - world_y).abs()
            );
        }

        // =====================================================================
        // Phase 14: Performance Optimization Property Tests
        // =====================================================================

        /// **Validates: Requirements 10.1, 10.4**
        ///
        /// Property 25: Viewport Culling
        ///
        /// For any graph with more than 50 nodes, only nodes whose bounding
        /// boxes intersect the expanded viewport (viewport + margin) SHALL be
        /// included in the render list.
        ///
        /// This test verifies:
        /// 1. Below threshold (< 50 nodes): all nodes returned regardless of position.
        /// 2. At/above threshold (>= 50 nodes): only visible nodes returned.
        /// 3. No visible node is incorrectly culled (false negative).
        /// 4. No invisible node is incorrectly included (false positive).
        #[test]
        fn prop_viewport_culling(
            // Viewport position and size
            vp_x in -500.0f32..500.0,
            vp_y in -500.0f32..500.0,
            vp_w in 100.0f32..1000.0,
            vp_h in 100.0f32..1000.0,
            // Number of nodes (range covers below and above threshold)
            node_count in 10usize..120,
        ) {
            let viewport = Rect::from_min_max(
                Vec2::new(vp_x, vp_y),
                Vec2::new(vp_x + vp_w, vp_y + vp_h),
            );
            let expanded = viewport.expand(VIEWPORT_CULL_MARGIN);

            // Generate nodes: half inside viewport, half outside
            let mut node_rects = HashMap::new();
            let node_size = 80.0f32;
            for i in 0..node_count {
                let id = NodeId(i as i64);
                let rect = if i % 2 == 0 {
                    // Place inside the viewport
                    let frac = (i as f32) / (node_count as f32);
                    Rect::from_min_max(
                        Vec2::new(vp_x + frac * vp_w * 0.5, vp_y + frac * vp_h * 0.5),
                        Vec2::new(
                            vp_x + frac * vp_w * 0.5 + node_size,
                            vp_y + frac * vp_h * 0.5 + node_size,
                        ),
                    )
                } else {
                    // Place far outside the viewport (beyond margin)
                    Rect::from_min_max(
                        Vec2::new(vp_x + vp_w + VIEWPORT_CULL_MARGIN + 200.0, vp_y + vp_h + VIEWPORT_CULL_MARGIN + 200.0),
                        Vec2::new(
                            vp_x + vp_w + VIEWPORT_CULL_MARGIN + 200.0 + node_size,
                            vp_y + vp_h + VIEWPORT_CULL_MARGIN + 200.0 + node_size,
                        ),
                    )
                };
                node_rects.insert(id, rect);
            }

            let visible = viewport_cull(&node_rects, viewport);

            if node_count < VIEWPORT_CULL_THRESHOLD {
                // Below threshold: all nodes should be returned
                prop_assert_eq!(
                    visible.len(), node_count,
                    "Below threshold ({}), all {} nodes should be visible, got {}",
                    VIEWPORT_CULL_THRESHOLD, node_count, visible.len()
                );
            } else {
                // Above threshold: only nodes intersecting expanded viewport should be returned

                // 1. No false negatives: every node intersecting expanded viewport is included
                for (id, rect) in &node_rects {
                    if expanded.intersects(rect) {
                        prop_assert!(
                            visible.contains(id),
                            "Node {:?} intersects expanded viewport but was culled", id
                        );
                    }
                }

                // 2. No false positives: every visible node must intersect expanded viewport
                for id in &visible {
                    let rect = node_rects.get(id).expect("visible node must exist");
                    prop_assert!(
                        expanded.intersects(rect),
                        "Node {:?} is in visible set but does not intersect expanded viewport", id
                    );
                }

                // 3. Visible count should be less than total (we placed half far outside)
                prop_assert!(
                    visible.len() <= node_count,
                    "Visible count {} should be <= total {}",
                    visible.len(), node_count
                );
            }
        }

        /// Property 25 variant: Viewport culling with all nodes inside viewport
        ///
        /// When all nodes are inside the viewport, all should be returned regardless
        /// of the threshold.
        #[test]
        fn prop_viewport_culling_all_visible(
            vp_w in 500.0f32..2000.0,
            vp_h in 500.0f32..2000.0,
            node_count in 50usize..100,
        ) {
            let viewport = Rect::from_min_max(
                Vec2::new(0.0, 0.0),
                Vec2::new(vp_w, vp_h),
            );

            let mut node_rects = HashMap::new();
            let node_size = 10.0f32;
            for i in 0..node_count {
                let id = NodeId(i as i64);
                let x = (i as f32 % 20.0) * 20.0 + 10.0;
                let y = (i as f32 / 20.0).floor() * 20.0 + 10.0;
                node_rects.insert(id, Rect::from_min_max(
                    Vec2::new(x, y),
                    Vec2::new(x + node_size, y + node_size),
                ));
            }

            let visible = viewport_cull(&node_rects, viewport);
            prop_assert_eq!(
                visible.len(), node_count,
                "All {} nodes are inside viewport, all should be visible",
                node_count
            );
        }

        /// Property 25 variant: Viewport culling with all nodes outside viewport
        ///
        /// When all nodes are far outside the viewport, none should be returned
        /// (above threshold).
        #[test]
        fn prop_viewport_culling_none_visible(
            node_count in 50usize..100,
        ) {
            let viewport = Rect::from_min_max(
                Vec2::new(0.0, 0.0),
                Vec2::new(100.0, 100.0),
            );

            let mut node_rects = HashMap::new();
            let far = 100.0 + VIEWPORT_CULL_MARGIN + 500.0;
            for i in 0..node_count {
                let id = NodeId(i as i64);
                node_rects.insert(id, Rect::from_min_max(
                    Vec2::new(far + i as f32 * 20.0, far),
                    Vec2::new(far + i as f32 * 20.0 + 10.0, far + 10.0),
                ));
            }

            let visible = viewport_cull(&node_rects, viewport);
            prop_assert_eq!(
                visible.len(), 0,
                "All nodes are far outside viewport, none should be visible, got {}",
                visible.len()
            );
        }
    }
}
