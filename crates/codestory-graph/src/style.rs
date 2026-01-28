//! Graph View Style System
//!
//! Provides color mapping for nodes and edges based on their types,
//! mirroring the Sourcetrail color scheme.

use codestory_core::{EdgeKind, NodeKind};

/// RGB color representation
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn to_tuple(&self) -> (u8, u8, u8, u8) {
        (self.r, self.g, self.b, self.a)
    }

    pub fn darken(&self, factor: f32) -> Self {
        Self {
            r: ((self.r as f32) * (1.0 - factor)) as u8,
            g: ((self.g as f32) * (1.0 - factor)) as u8,
            b: ((self.b as f32) * (1.0 - factor)) as u8,
            a: self.a,
        }
    }

    pub fn lighten(&self, factor: f32) -> Self {
        Self {
            r: ((self.r as f32) + (255.0 - self.r as f32) * factor) as u8,
            g: ((self.g as f32) + (255.0 - self.g as f32) * factor) as u8,
            b: ((self.b as f32) + (255.0 - self.b as f32) * factor) as u8,
            a: self.a,
        }
    }
}

/// Node color palette based on Sourcetrail's color scheme
#[derive(Debug, Clone, Copy)]
pub struct NodeColors {
    pub fill: Color,
    pub border: Color,
    pub text: Color,
    pub hatching: Color, // For non-indexed nodes
}

/// Edge color and style
#[derive(Debug, Clone, Copy)]
pub struct EdgeStyle {
    pub color: Color,
    pub width: f32,
    pub dashed: bool,
    pub arrow_head: bool,
}

/// Hatching pattern for non-indexed nodes
/// Defines diagonal striped pattern overlay
#[derive(Debug, Clone, Copy)]
pub struct HatchingPattern {
    /// Color of the hatching lines
    pub color: Color,

    /// Angle of the hatching lines in degrees (typically 45 for diagonal)
    pub angle: f32,

    /// Spacing between hatching lines in pixels
    pub spacing: f32,

    /// Width of each hatching line in pixels
    pub line_width: f32,
}

/// Complete style for a graph node
#[derive(Debug, Clone)]
pub struct NodeStyle {
    pub colors: NodeColors,
    pub corner_radius: f32,
    pub font_size: f32,
    pub font_bold: bool,
    pub min_width: f32,
    pub min_height: f32,
    pub icon: Option<&'static str>,
}

// ============================================================================
// Color Constants - Based on Sourcetrail's color scheme
// ============================================================================

// Types and Classes (gray tones)
pub const COLOR_TYPE_FILL: Color = Color::rgb(85, 85, 85);
pub const COLOR_TYPE_BORDER: Color = Color::rgb(70, 70, 70);
pub const COLOR_TYPE_TEXT: Color = Color::rgb(255, 255, 255);

// Functions and Methods (yellow/gold tones)
pub const COLOR_FUNCTION_FILL: Color = Color::rgb(200, 160, 80);
pub const COLOR_FUNCTION_BORDER: Color = Color::rgb(170, 130, 60);
pub const COLOR_FUNCTION_TEXT: Color = Color::rgb(30, 30, 30);

// Variables and Fields (blue tones)
pub const COLOR_VARIABLE_FILL: Color = Color::rgb(80, 130, 180);
pub const COLOR_VARIABLE_BORDER: Color = Color::rgb(60, 110, 160);
pub const COLOR_VARIABLE_TEXT: Color = Color::rgb(255, 255, 255);

// Files and Modules (green tones)
pub const COLOR_FILE_FILL: Color = Color::rgb(80, 140, 100);
pub const COLOR_FILE_BORDER: Color = Color::rgb(60, 120, 80);
pub const COLOR_FILE_TEXT: Color = Color::rgb(255, 255, 255);

// Namespaces and Packages (purple tones)
pub const COLOR_NAMESPACE_FILL: Color = Color::rgb(130, 100, 160);
pub const COLOR_NAMESPACE_BORDER: Color = Color::rgb(110, 80, 140);
pub const COLOR_NAMESPACE_TEXT: Color = Color::rgb(255, 255, 255);

// Macros (orange tones)
pub const COLOR_MACRO_FILL: Color = Color::rgb(200, 120, 80);
pub const COLOR_MACRO_BORDER: Color = Color::rgb(180, 100, 60);
pub const COLOR_MACRO_TEXT: Color = Color::rgb(255, 255, 255);

// Enums (teal tones)
pub const COLOR_ENUM_FILL: Color = Color::rgb(80, 150, 150);
pub const COLOR_ENUM_BORDER: Color = Color::rgb(60, 130, 130);
pub const COLOR_ENUM_TEXT: Color = Color::rgb(255, 255, 255);

// Unknown/Default
pub const COLOR_UNKNOWN_FILL: Color = Color::rgb(100, 100, 100);
pub const COLOR_UNKNOWN_BORDER: Color = Color::rgb(80, 80, 80);
pub const COLOR_UNKNOWN_TEXT: Color = Color::rgb(255, 255, 255);

// Bundle nodes
pub const COLOR_BUNDLE_FILL: Color = Color::rgb(60, 60, 70);
pub const COLOR_BUNDLE_BORDER: Color = Color::rgb(50, 50, 60);
pub const COLOR_BUNDLE_TEXT: Color = Color::rgb(200, 200, 200);

// Focus/Active colors
pub const COLOR_FOCUS_BORDER: Color = Color::rgb(255, 200, 100);
pub const COLOR_ACTIVE_FILL: Color = Color::rgb(60, 80, 100);
pub const COLOR_HOVER_OVERLAY: Color = Color::rgba(255, 255, 255, 30);

// Hatching color for non-indexed nodes
pub const COLOR_HATCHING: Color = Color::rgba(50, 50, 50, 150);

// Edge colors
pub const COLOR_EDGE_MEMBER: Color = Color::rgb(100, 100, 100);
pub const COLOR_EDGE_TYPE_USE: Color = Color::rgb(140, 140, 140);
pub const COLOR_EDGE_CALL: Color = Color::rgb(200, 160, 80);
pub const COLOR_EDGE_INHERITANCE: Color = Color::rgb(80, 130, 180);
pub const COLOR_EDGE_OVERRIDE: Color = Color::rgb(100, 150, 200);
pub const COLOR_EDGE_USAGE: Color = Color::rgb(80, 130, 180);
pub const COLOR_EDGE_IMPORT: Color = Color::rgb(80, 140, 100);
pub const COLOR_EDGE_INCLUDE: Color = Color::rgb(80, 140, 100);
pub const COLOR_EDGE_MACRO_USAGE: Color = Color::rgb(200, 120, 80);
pub const COLOR_EDGE_ANNOTATION: Color = Color::rgb(180, 100, 140);
pub const COLOR_EDGE_UNKNOWN: Color = Color::rgb(120, 120, 120);

// ============================================================================
// Style Functions
// ============================================================================

/// State tracking for node rendering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NodeState {
    /// Whether the node is currently active (selected/clicked)
    pub is_active: bool,

    /// Whether the node is focused (e.g., via keyboard navigation)
    pub is_focused: bool,

    /// Whether the node is currently being hovered over
    pub is_hovered: bool,

    /// Whether the node is indexed (affects hatching pattern)
    pub is_indexed: bool,
}

impl NodeState {
    /// Create a new node state with all flags set to false except is_indexed
    pub fn new() -> Self {
        Self {
            is_active: false,
            is_focused: false,
            is_hovered: false,
            is_indexed: true,
        }
    }

    /// Create a node state for an indexed node
    pub fn indexed() -> Self {
        Self {
            is_active: false,
            is_focused: false,
            is_hovered: false,
            is_indexed: true,
        }
    }

    /// Create a node state for a non-indexed node
    pub fn not_indexed() -> Self {
        Self {
            is_active: false,
            is_focused: false,
            is_hovered: false,
            is_indexed: false,
        }
    }

    /// Set the active state
    pub fn with_active(mut self, active: bool) -> Self {
        self.is_active = active;
        self
    }

    /// Set the focused state
    pub fn with_focused(mut self, focused: bool) -> Self {
        self.is_focused = focused;
        self
    }

    /// Set the hovered state
    pub fn with_hovered(mut self, hovered: bool) -> Self {
        self.is_hovered = hovered;
        self
    }

    /// Set the indexed state
    pub fn with_indexed(mut self, indexed: bool) -> Self {
        self.is_indexed = indexed;
        self
    }
}

/// State tracking for edge rendering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EdgeState {
    /// Whether the edge is currently active (selected/clicked)
    pub is_active: bool,

    /// Whether the edge is currently being hovered over
    pub is_hovered: bool,

    /// Whether the edge is part of a bundle
    pub is_bundled: bool,

    /// Whether the edge is currently focused
    pub is_focused: bool,
}

impl EdgeState {
    /// Create a new edge state with all flags set to false
    pub fn new() -> Self {
        Self {
            is_active: false,
            is_hovered: false,
            is_bundled: false,
            is_focused: false,
        }
    }

    /// Create an edge state for a bundled edge
    pub fn bundled() -> Self {
        Self {
            is_active: false,
            is_hovered: false,
            is_bundled: true,
            is_focused: false,
        }
    }

    /// Set the active state
    pub fn with_active(mut self, active: bool) -> Self {
        self.is_active = active;
        self
    }

    /// Set the hovered state
    pub fn with_hovered(mut self, hovered: bool) -> Self {
        self.is_hovered = hovered;
        self
    }

    /// Set the bundled state
    pub fn with_bundled(mut self, bundled: bool) -> Self {
        self.is_bundled = bundled;
        self
    }

    /// Set the focused state
    pub fn with_focused(mut self, focused: bool) -> Self {
        self.is_focused = focused;
        self
    }
}

/// Get the style for a node based on its kind
pub fn get_node_style(
    kind: NodeKind,
    is_active: bool,
    is_focused: bool,
    is_indexed: bool,
) -> NodeStyle {
    let base_colors = get_node_colors(kind);

    let colors = if is_active {
        NodeColors {
            fill: COLOR_ACTIVE_FILL,
            border: COLOR_FOCUS_BORDER,
            text: base_colors.text,
            hatching: base_colors.hatching,
        }
    } else if is_focused {
        NodeColors {
            fill: base_colors.fill.lighten(0.1),
            border: COLOR_FOCUS_BORDER,
            text: base_colors.text,
            hatching: base_colors.hatching,
        }
    } else if !is_indexed {
        NodeColors {
            fill: base_colors.fill.darken(0.2),
            border: base_colors.border,
            text: base_colors.text.darken(0.2),
            hatching: COLOR_HATCHING,
        }
    } else {
        base_colors
    };

    NodeStyle {
        colors,
        corner_radius: 5.0,
        font_size: get_font_size_for_kind(kind),
        font_bold: is_type_kind(kind),
        min_width: 80.0,
        min_height: 30.0,
        icon: get_icon_for_kind(kind),
    }
}

/// Get the base colors for a node kind
pub fn get_node_colors(kind: NodeKind) -> NodeColors {
    match kind {
        // Types
        NodeKind::CLASS
        | NodeKind::STRUCT
        | NodeKind::INTERFACE
        | NodeKind::UNION
        | NodeKind::TYPEDEF
        | NodeKind::TYPE_PARAMETER => NodeColors {
            fill: COLOR_TYPE_FILL,
            border: COLOR_TYPE_BORDER,
            text: COLOR_TYPE_TEXT,
            hatching: COLOR_HATCHING,
        },

        // Functions
        NodeKind::FUNCTION | NodeKind::METHOD => NodeColors {
            fill: COLOR_FUNCTION_FILL,
            border: COLOR_FUNCTION_BORDER,
            text: COLOR_FUNCTION_TEXT,
            hatching: COLOR_HATCHING,
        },

        // Variables
        NodeKind::GLOBAL_VARIABLE | NodeKind::FIELD | NodeKind::VARIABLE | NodeKind::CONSTANT => {
            NodeColors {
                fill: COLOR_VARIABLE_FILL,
                border: COLOR_VARIABLE_BORDER,
                text: COLOR_VARIABLE_TEXT,
                hatching: COLOR_HATCHING,
            }
        }

        // Files
        NodeKind::FILE => NodeColors {
            fill: COLOR_FILE_FILL,
            border: COLOR_FILE_BORDER,
            text: COLOR_FILE_TEXT,
            hatching: COLOR_HATCHING,
        },

        // Namespaces/Modules
        NodeKind::MODULE | NodeKind::NAMESPACE | NodeKind::PACKAGE => NodeColors {
            fill: COLOR_NAMESPACE_FILL,
            border: COLOR_NAMESPACE_BORDER,
            text: COLOR_NAMESPACE_TEXT,
            hatching: COLOR_HATCHING,
        },

        // Enums
        NodeKind::ENUM | NodeKind::ENUM_CONSTANT => NodeColors {
            fill: COLOR_ENUM_FILL,
            border: COLOR_ENUM_BORDER,
            text: COLOR_ENUM_TEXT,
            hatching: COLOR_HATCHING,
        },

        // Macros
        NodeKind::MACRO => NodeColors {
            fill: COLOR_MACRO_FILL,
            border: COLOR_MACRO_BORDER,
            text: COLOR_MACRO_TEXT,
            hatching: COLOR_HATCHING,
        },

        // Annotations
        NodeKind::ANNOTATION => NodeColors {
            fill: COLOR_NAMESPACE_FILL,
            border: COLOR_NAMESPACE_BORDER,
            text: COLOR_NAMESPACE_TEXT,
            hatching: COLOR_HATCHING,
        },

        // Builtin
        NodeKind::BUILTIN_TYPE => NodeColors {
            fill: COLOR_TYPE_FILL.lighten(0.2),
            border: COLOR_TYPE_BORDER,
            text: COLOR_TYPE_TEXT,
            hatching: COLOR_HATCHING,
        },

        // Unknown
        NodeKind::UNKNOWN => NodeColors {
            fill: COLOR_UNKNOWN_FILL,
            border: COLOR_UNKNOWN_BORDER,
            text: COLOR_UNKNOWN_TEXT,
            hatching: COLOR_HATCHING,
        },
    }
}

/// Get the style for a node based on its kind and state
pub fn get_node_style_with_state(kind: NodeKind, state: NodeState) -> NodeStyle {
    get_node_style(kind, state.is_active, state.is_focused, state.is_indexed)
}

/// Get the style for an edge based on its kind
pub fn get_edge_style(kind: EdgeKind, is_active: bool, is_focused: bool) -> EdgeStyle {
    let base_color = get_edge_color(kind);
    let base_width = get_edge_width(kind);

    let (color, width) = if is_active {
        (COLOR_FOCUS_BORDER, base_width * 2.0)
    } else if is_focused {
        (base_color.lighten(0.3), base_width * 1.5)
    } else {
        (base_color, base_width)
    };

    EdgeStyle {
        color,
        width,
        dashed: is_dashed_edge(kind),
        arrow_head: has_arrow_head(kind),
    }
}

/// Get the style for an edge based on its kind and state
pub fn get_edge_style_with_state(kind: EdgeKind, state: EdgeState) -> EdgeStyle {
    get_edge_style(kind, state.is_active, state.is_focused)
}

/// Get the base color for an edge kind
pub fn get_edge_color(kind: EdgeKind) -> Color {
    match kind {
        EdgeKind::MEMBER => COLOR_EDGE_MEMBER,
        EdgeKind::TYPE_USAGE => COLOR_EDGE_TYPE_USE,
        EdgeKind::CALL => COLOR_EDGE_CALL,
        EdgeKind::USAGE => COLOR_EDGE_USAGE,
        EdgeKind::INHERITANCE => COLOR_EDGE_INHERITANCE,
        EdgeKind::OVERRIDE => COLOR_EDGE_OVERRIDE,
        EdgeKind::TYPE_ARGUMENT => COLOR_EDGE_TYPE_USE,
        EdgeKind::TEMPLATE_SPECIALIZATION => COLOR_EDGE_TYPE_USE,
        EdgeKind::IMPORT => COLOR_EDGE_IMPORT,
        EdgeKind::INCLUDE => COLOR_EDGE_INCLUDE,
        EdgeKind::MACRO_USAGE => COLOR_EDGE_MACRO_USAGE,
        EdgeKind::ANNOTATION_USAGE => COLOR_EDGE_ANNOTATION,
        EdgeKind::UNKNOWN => COLOR_EDGE_UNKNOWN,
    }
}

/// Get the width for an edge kind
fn get_edge_width(kind: EdgeKind) -> f32 {
    match kind {
        EdgeKind::MEMBER => 1.0,
        EdgeKind::INHERITANCE | EdgeKind::OVERRIDE => 2.0,
        EdgeKind::CALL => 1.5,
        _ => 1.0,
    }
}

/// Check if edge should be dashed
fn is_dashed_edge(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::OVERRIDE | EdgeKind::TEMPLATE_SPECIALIZATION)
}

/// Check if edge should have an arrow head
fn has_arrow_head(kind: EdgeKind) -> bool {
    !matches!(kind, EdgeKind::MEMBER)
}

/// Check if node kind is a type
fn is_type_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::CLASS
            | NodeKind::STRUCT
            | NodeKind::INTERFACE
            | NodeKind::ENUM
            | NodeKind::UNION
            | NodeKind::TYPEDEF
    )
}

/// Get font size for node kind
fn get_font_size_for_kind(kind: NodeKind) -> f32 {
    match kind {
        NodeKind::FILE | NodeKind::MODULE | NodeKind::NAMESPACE | NodeKind::PACKAGE => 12.0,
        NodeKind::CLASS | NodeKind::STRUCT | NodeKind::INTERFACE => 14.0,
        _ => 13.0,
    }
}

/// Get icon identifier for node kind
fn get_icon_for_kind(kind: NodeKind) -> Option<&'static str> {
    match kind {
        NodeKind::CLASS => Some("class"),
        NodeKind::STRUCT => Some("struct"),
        NodeKind::INTERFACE => Some("interface"),
        NodeKind::ENUM => Some("enum"),
        NodeKind::FUNCTION => Some("function"),
        NodeKind::METHOD => Some("method"),
        NodeKind::FIELD => Some("field"),
        NodeKind::VARIABLE => Some("variable"),
        NodeKind::CONSTANT => Some("constant"),
        NodeKind::FILE => Some("file"),
        NodeKind::MODULE => Some("module"),
        NodeKind::NAMESPACE => Some("namespace"),
        NodeKind::PACKAGE => Some("package"),
        NodeKind::MACRO => Some("macro"),
        _ => None,
    }
}

/// Get the style for bundle nodes
pub fn get_bundle_style(is_focused: bool, bundled_kind: Option<NodeKind>) -> NodeStyle {
    let base_colors = if let Some(kind) = bundled_kind {
        let mut colors = get_node_colors(kind);
        colors.fill = colors.fill.darken(0.3);
        colors
    } else {
        NodeColors {
            fill: COLOR_BUNDLE_FILL,
            border: COLOR_BUNDLE_BORDER,
            text: COLOR_BUNDLE_TEXT,
            hatching: COLOR_HATCHING,
        }
    };

    let colors = if is_focused {
        NodeColors {
            fill: base_colors.fill.lighten(0.1),
            border: COLOR_FOCUS_BORDER,
            ..base_colors
        }
    } else {
        base_colors
    };

    NodeStyle {
        colors,
        corner_radius: 5.0,
        font_size: 13.0,
        font_bold: false,
        min_width: 100.0,
        min_height: 30.0,
        icon: Some("bundle"),
    }
}

/// Get the style for group nodes (file groups, namespace groups)
pub fn get_group_style(group_type: GroupType, is_focused: bool) -> NodeStyle {
    let base_colors = match group_type {
        GroupType::File => NodeColors {
            fill: Color::rgba(80, 140, 100, 40),
            border: COLOR_FILE_BORDER,
            text: COLOR_FILE_TEXT,
            hatching: COLOR_HATCHING,
        },
        GroupType::Namespace => NodeColors {
            fill: Color::rgba(130, 100, 160, 40),
            border: COLOR_NAMESPACE_BORDER,
            text: COLOR_NAMESPACE_TEXT,
            hatching: COLOR_HATCHING,
        },
    };

    let colors = if is_focused {
        NodeColors {
            border: COLOR_FOCUS_BORDER,
            ..base_colors
        }
    } else {
        base_colors
    };

    NodeStyle {
        colors,
        corner_radius: 8.0,
        font_size: 12.0,
        font_bold: true,
        min_width: 150.0,
        min_height: 50.0,
        icon: match group_type {
            GroupType::File => Some("file"),
            GroupType::Namespace => Some("namespace"),
        },
    }
}

/// Group type for grouping nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupType {
    File,
    Namespace,
}

/// Get the human-readable label for a node kind
pub fn get_kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::MODULE => "module",
        NodeKind::NAMESPACE => "namespace",
        NodeKind::PACKAGE => "package",
        NodeKind::FILE => "file",
        NodeKind::STRUCT => "struct",
        NodeKind::CLASS => "class",
        NodeKind::INTERFACE => "interface",
        NodeKind::ANNOTATION => "annotation",
        NodeKind::UNION => "union",
        NodeKind::ENUM => "enum",
        NodeKind::TYPEDEF => "typedef",
        NodeKind::TYPE_PARAMETER => "type param",
        NodeKind::BUILTIN_TYPE => "builtin",
        NodeKind::FUNCTION => "function",
        NodeKind::METHOD => "method",
        NodeKind::MACRO => "macro",
        NodeKind::GLOBAL_VARIABLE => "global var",
        NodeKind::FIELD => "field",
        NodeKind::VARIABLE => "variable",
        NodeKind::CONSTANT => "constant",
        NodeKind::ENUM_CONSTANT => "enum const",
        NodeKind::UNKNOWN => "symbol",
    }
}

/// Get the human-readable label for an edge kind
pub fn get_edge_kind_label(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::MEMBER => "member",
        EdgeKind::TYPE_USAGE => "type use",
        EdgeKind::USAGE => "usage",
        EdgeKind::CALL => "call",
        EdgeKind::INHERITANCE => "inherits",
        EdgeKind::OVERRIDE => "overrides",
        EdgeKind::TYPE_ARGUMENT => "type arg",
        EdgeKind::TEMPLATE_SPECIALIZATION => "specialization",
        EdgeKind::INCLUDE => "include",
        EdgeKind::IMPORT => "import",
        EdgeKind::MACRO_USAGE => "macro use",
        EdgeKind::ANNOTATION_USAGE => "annotation",
        EdgeKind::UNKNOWN => "edge",
    }
}

/// Get the hatching pattern for non-indexed nodes
/// Returns a diagonal striped pattern overlay configuration
pub fn hatching_pattern() -> HatchingPattern {
    HatchingPattern {
        color: COLOR_HATCHING,
        angle: 45.0,     // Diagonal stripes at 45 degrees
        spacing: 8.0,    // 8 pixels between lines
        line_width: 1.5, // 1.5 pixel wide lines
    }
}

/// Calculate edge width based on edge kind and bundle count
///
/// For bundled edges, uses logarithmic scaling: min(log2(bundle_count) + base_width, max_width)
/// where base_width = 1.0 and max_width = 6.0
///
/// # Arguments
/// * `kind` - The type of edge
/// * `bundle_count` - Number of edges in the bundle (use 1 for non-bundled edges)
///
/// # Returns
/// The width in pixels for rendering the edge
///
/// **Validates: Requirements 14.5, Property 31: Edge Bundle Thickness Scaling**
pub fn edge_width(kind: EdgeKind, bundle_count: usize) -> f32 {
    const BASE_WIDTH: f32 = 1.0;
    const MAX_WIDTH: f32 = 6.0;

    // Get the base width for this edge kind
    let kind_base_width = get_edge_width(kind);

    // If bundle_count is 1 or less, just return the base width for the kind
    if bundle_count <= 1 {
        return kind_base_width;
    }

    // Apply logarithmic scaling for bundled edges
    // Formula: min(log2(bundle_count) + base_width, max_width)
    let bundle_width = (bundle_count as f32).log2() + BASE_WIDTH;
    let scaled_width = bundle_width.min(MAX_WIDTH);

    // Combine with the kind's base width (use the larger of the two)
    scaled_width.max(kind_base_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_colors() {
        let colors = get_node_colors(NodeKind::CLASS);
        assert_eq!(colors.fill, COLOR_TYPE_FILL);
    }

    // ========================================================================
    // Property-Based Tests
    // ========================================================================

    #[cfg(test)]
    mod property_tests {
        use super::*;
        use proptest::prelude::*;

        /// Strategy to generate all possible NodeKind values
        fn node_kind_strategy() -> impl Strategy<Value = NodeKind> {
            prop_oneof![
                // Structural
                Just(NodeKind::MODULE),
                Just(NodeKind::NAMESPACE),
                Just(NodeKind::PACKAGE),
                Just(NodeKind::FILE),
                // Types
                Just(NodeKind::STRUCT),
                Just(NodeKind::CLASS),
                Just(NodeKind::INTERFACE),
                Just(NodeKind::ANNOTATION),
                Just(NodeKind::UNION),
                Just(NodeKind::ENUM),
                Just(NodeKind::TYPEDEF),
                Just(NodeKind::TYPE_PARAMETER),
                Just(NodeKind::BUILTIN_TYPE),
                // Callable/Executable
                Just(NodeKind::FUNCTION),
                Just(NodeKind::METHOD),
                Just(NodeKind::MACRO),
                // Variables/Constants
                Just(NodeKind::GLOBAL_VARIABLE),
                Just(NodeKind::FIELD),
                Just(NodeKind::VARIABLE),
                Just(NodeKind::CONSTANT),
                Just(NodeKind::ENUM_CONSTANT),
                // Other
                Just(NodeKind::UNKNOWN),
            ]
        }

        /// Strategy to generate all possible EdgeKind values
        fn edge_kind_strategy() -> impl Strategy<Value = EdgeKind> {
            prop_oneof![
                // Definition/Hierarchy
                Just(EdgeKind::MEMBER),
                // Usage
                Just(EdgeKind::TYPE_USAGE),
                Just(EdgeKind::USAGE),
                Just(EdgeKind::CALL),
                // OOP
                Just(EdgeKind::INHERITANCE),
                Just(EdgeKind::OVERRIDE),
                // Generics
                Just(EdgeKind::TYPE_ARGUMENT),
                Just(EdgeKind::TEMPLATE_SPECIALIZATION),
                // Imports
                Just(EdgeKind::INCLUDE),
                Just(EdgeKind::IMPORT),
                // Metaprogramming
                Just(EdgeKind::MACRO_USAGE),
                Just(EdgeKind::ANNOTATION_USAGE),
                // Other
                Just(EdgeKind::UNKNOWN),
            ]
        }

        proptest! {
            /// **Validates: Requirements 1.5, 12.3**
            ///
            /// Property 1: Node Color Mapping Consistency
            ///
            /// For any NodeKind value, the StyleResolver SHALL return a color that matches
            /// the UML semantic color scheme:
            /// - Types (CLASS, STRUCT, INTERFACE, UNION, TYPEDEF, TYPE_PARAMETER) = gray
            /// - Functions (FUNCTION, METHOD) = yellow/gold
            /// - Variables (GLOBAL_VARIABLE, FIELD, VARIABLE, CONSTANT) = blue
            /// - Files (FILE) = green
            /// - Namespaces (MODULE, NAMESPACE, PACKAGE) = purple
            /// - Enums (ENUM, ENUM_CONSTANT) = teal
            /// - Macros (MACRO) = orange
            /// - Annotations (ANNOTATION) = purple
            /// - Builtin types = lighter gray
            /// - Unknown = gray
            #[test]
            fn prop_node_color_mapping_consistency(kind in node_kind_strategy()) {
                let colors = get_node_colors(kind);

                // Verify that the returned color matches the expected UML semantic scheme
                match kind {
                    // Types should be gray
                    NodeKind::CLASS | NodeKind::STRUCT | NodeKind::INTERFACE |
                    NodeKind::UNION | NodeKind::TYPEDEF | NodeKind::TYPE_PARAMETER => {
                        prop_assert_eq!(colors.fill, COLOR_TYPE_FILL,
                            "Type nodes should use gray color scheme");
                        prop_assert_eq!(colors.border, COLOR_TYPE_BORDER);
                        prop_assert_eq!(colors.text, COLOR_TYPE_TEXT);
                    }

                    // Functions should be yellow/gold
                    NodeKind::FUNCTION | NodeKind::METHOD => {
                        prop_assert_eq!(colors.fill, COLOR_FUNCTION_FILL,
                            "Function nodes should use yellow/gold color scheme");
                        prop_assert_eq!(colors.border, COLOR_FUNCTION_BORDER);
                        prop_assert_eq!(colors.text, COLOR_FUNCTION_TEXT);
                    }

                    // Variables should be blue
                    NodeKind::GLOBAL_VARIABLE | NodeKind::FIELD |
                    NodeKind::VARIABLE | NodeKind::CONSTANT => {
                        prop_assert_eq!(colors.fill, COLOR_VARIABLE_FILL,
                            "Variable nodes should use blue color scheme");
                        prop_assert_eq!(colors.border, COLOR_VARIABLE_BORDER);
                        prop_assert_eq!(colors.text, COLOR_VARIABLE_TEXT);
                    }

                    // Files should be green
                    NodeKind::FILE => {
                        prop_assert_eq!(colors.fill, COLOR_FILE_FILL,
                            "File nodes should use green color scheme");
                        prop_assert_eq!(colors.border, COLOR_FILE_BORDER);
                        prop_assert_eq!(colors.text, COLOR_FILE_TEXT);
                    }

                    // Namespaces should be purple
                    NodeKind::MODULE | NodeKind::NAMESPACE | NodeKind::PACKAGE => {
                        prop_assert_eq!(colors.fill, COLOR_NAMESPACE_FILL,
                            "Namespace nodes should use purple color scheme");
                        prop_assert_eq!(colors.border, COLOR_NAMESPACE_BORDER);
                        prop_assert_eq!(colors.text, COLOR_NAMESPACE_TEXT);
                    }

                    // Enums should be teal
                    NodeKind::ENUM | NodeKind::ENUM_CONSTANT => {
                        prop_assert_eq!(colors.fill, COLOR_ENUM_FILL,
                            "Enum nodes should use teal color scheme");
                        prop_assert_eq!(colors.border, COLOR_ENUM_BORDER);
                        prop_assert_eq!(colors.text, COLOR_ENUM_TEXT);
                    }

                    // Macros should be orange
                    NodeKind::MACRO => {
                        prop_assert_eq!(colors.fill, COLOR_MACRO_FILL,
                            "Macro nodes should use orange color scheme");
                        prop_assert_eq!(colors.border, COLOR_MACRO_BORDER);
                        prop_assert_eq!(colors.text, COLOR_MACRO_TEXT);
                    }

                    // Annotations should be purple (same as namespaces)
                    NodeKind::ANNOTATION => {
                        prop_assert_eq!(colors.fill, COLOR_NAMESPACE_FILL,
                            "Annotation nodes should use purple color scheme");
                        prop_assert_eq!(colors.border, COLOR_NAMESPACE_BORDER);
                        prop_assert_eq!(colors.text, COLOR_NAMESPACE_TEXT);
                    }

                    // Builtin types should be lighter gray
                    NodeKind::BUILTIN_TYPE => {
                        let expected_fill = COLOR_TYPE_FILL.lighten(0.2);
                        prop_assert_eq!(colors.fill, expected_fill,
                            "Builtin type nodes should use lighter gray color scheme");
                        prop_assert_eq!(colors.border, COLOR_TYPE_BORDER);
                        prop_assert_eq!(colors.text, COLOR_TYPE_TEXT);
                    }

                    // Unknown should be gray
                    NodeKind::UNKNOWN => {
                        prop_assert_eq!(colors.fill, COLOR_UNKNOWN_FILL,
                            "Unknown nodes should use gray color scheme");
                        prop_assert_eq!(colors.border, COLOR_UNKNOWN_BORDER);
                        prop_assert_eq!(colors.text, COLOR_UNKNOWN_TEXT);
                    }
                }

                // All nodes should have hatching color set
                prop_assert_eq!(colors.hatching, COLOR_HATCHING,
                    "All nodes should have consistent hatching color");
            }

            /// **Validates: Requirements 3.4**
            ///
            /// Property 8: Edge Color Mapping
            ///
            /// For any EdgeKind value, the StyleResolver SHALL return the correct semantic color:
            /// - CALL = yellow (200, 160, 80)
            /// - INHERITANCE = blue (80, 130, 180)
            /// - MEMBER = gray (100, 100, 100)
            /// - TYPE_USAGE = gray (140, 140, 140)
            /// - USAGE = blue (80, 130, 180)
            /// - OVERRIDE = blue (100, 150, 200)
            /// - TYPE_ARGUMENT = gray (140, 140, 140)
            /// - TEMPLATE_SPECIALIZATION = gray (140, 140, 140)
            /// - IMPORT = green (80, 140, 100)
            /// - INCLUDE = green (80, 140, 100)
            /// - MACRO_USAGE = orange (200, 120, 80)
            /// - ANNOTATION_USAGE = purple/pink (180, 100, 140)
            /// - UNKNOWN = gray (120, 120, 120)
            #[test]
            fn prop_edge_color_mapping(kind in edge_kind_strategy()) {
                let color = get_edge_color(kind);

                // Verify that the returned color matches the expected semantic scheme
                match kind {
                    // CALL edges should be yellow/gold
                    EdgeKind::CALL => {
                        prop_assert_eq!(color, COLOR_EDGE_CALL,
                            "CALL edges should use yellow/gold color (200, 160, 80)");
                        prop_assert_eq!(color.r, 200);
                        prop_assert_eq!(color.g, 160);
                        prop_assert_eq!(color.b, 80);
                    }

                    // INHERITANCE edges should be blue
                    EdgeKind::INHERITANCE => {
                        prop_assert_eq!(color, COLOR_EDGE_INHERITANCE,
                            "INHERITANCE edges should use blue color (80, 130, 180)");
                        prop_assert_eq!(color.r, 80);
                        prop_assert_eq!(color.g, 130);
                        prop_assert_eq!(color.b, 180);
                    }

                    // MEMBER edges should be gray
                    EdgeKind::MEMBER => {
                        prop_assert_eq!(color, COLOR_EDGE_MEMBER,
                            "MEMBER edges should use gray color (100, 100, 100)");
                        prop_assert_eq!(color.r, 100);
                        prop_assert_eq!(color.g, 100);
                        prop_assert_eq!(color.b, 100);
                    }

                    // TYPE_USAGE edges should be gray
                    EdgeKind::TYPE_USAGE => {
                        prop_assert_eq!(color, COLOR_EDGE_TYPE_USE,
                            "TYPE_USAGE edges should use gray color (140, 140, 140)");
                        prop_assert_eq!(color.r, 140);
                        prop_assert_eq!(color.g, 140);
                        prop_assert_eq!(color.b, 140);
                    }

                    // USAGE edges should be blue
                    EdgeKind::USAGE => {
                        prop_assert_eq!(color, COLOR_EDGE_USAGE,
                            "USAGE edges should use blue color (80, 130, 180)");
                        prop_assert_eq!(color.r, 80);
                        prop_assert_eq!(color.g, 130);
                        prop_assert_eq!(color.b, 180);
                    }

                    // OVERRIDE edges should be blue (lighter shade)
                    EdgeKind::OVERRIDE => {
                        prop_assert_eq!(color, COLOR_EDGE_OVERRIDE,
                            "OVERRIDE edges should use blue color (100, 150, 200)");
                        prop_assert_eq!(color.r, 100);
                        prop_assert_eq!(color.g, 150);
                        prop_assert_eq!(color.b, 200);
                    }

                    // TYPE_ARGUMENT edges should be gray (same as TYPE_USAGE)
                    EdgeKind::TYPE_ARGUMENT => {
                        prop_assert_eq!(color, COLOR_EDGE_TYPE_USE,
                            "TYPE_ARGUMENT edges should use gray color (140, 140, 140)");
                        prop_assert_eq!(color.r, 140);
                        prop_assert_eq!(color.g, 140);
                        prop_assert_eq!(color.b, 140);
                    }

                    // TEMPLATE_SPECIALIZATION edges should be gray (same as TYPE_USAGE)
                    EdgeKind::TEMPLATE_SPECIALIZATION => {
                        prop_assert_eq!(color, COLOR_EDGE_TYPE_USE,
                            "TEMPLATE_SPECIALIZATION edges should use gray color (140, 140, 140)");
                        prop_assert_eq!(color.r, 140);
                        prop_assert_eq!(color.g, 140);
                        prop_assert_eq!(color.b, 140);
                    }

                    // IMPORT edges should be green
                    EdgeKind::IMPORT => {
                        prop_assert_eq!(color, COLOR_EDGE_IMPORT,
                            "IMPORT edges should use green color (80, 140, 100)");
                        prop_assert_eq!(color.r, 80);
                        prop_assert_eq!(color.g, 140);
                        prop_assert_eq!(color.b, 100);
                    }

                    // INCLUDE edges should be green
                    EdgeKind::INCLUDE => {
                        prop_assert_eq!(color, COLOR_EDGE_INCLUDE,
                            "INCLUDE edges should use green color (80, 140, 100)");
                        prop_assert_eq!(color.r, 80);
                        prop_assert_eq!(color.g, 140);
                        prop_assert_eq!(color.b, 100);
                    }

                    // MACRO_USAGE edges should be orange
                    EdgeKind::MACRO_USAGE => {
                        prop_assert_eq!(color, COLOR_EDGE_MACRO_USAGE,
                            "MACRO_USAGE edges should use orange color (200, 120, 80)");
                        prop_assert_eq!(color.r, 200);
                        prop_assert_eq!(color.g, 120);
                        prop_assert_eq!(color.b, 80);
                    }

                    // ANNOTATION_USAGE edges should be purple/pink
                    EdgeKind::ANNOTATION_USAGE => {
                        prop_assert_eq!(color, COLOR_EDGE_ANNOTATION,
                            "ANNOTATION_USAGE edges should use purple/pink color (180, 100, 140)");
                        prop_assert_eq!(color.r, 180);
                        prop_assert_eq!(color.g, 100);
                        prop_assert_eq!(color.b, 140);
                    }

                    // UNKNOWN edges should be gray
                    EdgeKind::UNKNOWN => {
                        prop_assert_eq!(color, COLOR_EDGE_UNKNOWN,
                            "UNKNOWN edges should use gray color (120, 120, 120)");
                        prop_assert_eq!(color.r, 120);
                        prop_assert_eq!(color.g, 120);
                        prop_assert_eq!(color.b, 120);
                    }
                }
            }
        }
    }

    #[test]
    fn test_edge_colors() {
        let color = get_edge_color(EdgeKind::CALL);
        assert_eq!(color, COLOR_EDGE_CALL);
    }

    #[test]
    fn test_color_darken() {
        let color = Color::rgb(100, 100, 100);
        let darkened = color.darken(0.5);
        assert_eq!(darkened.r, 50);
    }

    #[test]
    fn test_kind_labels() {
        assert_eq!(get_kind_label(NodeKind::CLASS), "class");
        assert_eq!(get_edge_kind_label(EdgeKind::CALL), "call");
    }

    #[test]
    fn test_node_state_creation() {
        let state = NodeState::new();
        assert!(!state.is_active);
        assert!(!state.is_focused);
        assert!(!state.is_hovered);
        assert!(state.is_indexed);
    }

    #[test]
    fn test_node_state_indexed() {
        let state = NodeState::indexed();
        assert!(state.is_indexed);
        assert!(!state.is_active);
    }

    #[test]
    fn test_node_state_not_indexed() {
        let state = NodeState::not_indexed();
        assert!(!state.is_indexed);
        assert!(!state.is_active);
    }

    #[test]
    fn test_node_state_builder() {
        let state = NodeState::new()
            .with_active(true)
            .with_focused(true)
            .with_hovered(true)
            .with_indexed(false);

        assert!(state.is_active);
        assert!(state.is_focused);
        assert!(state.is_hovered);
        assert!(!state.is_indexed);
    }

    #[test]
    fn test_edge_state_creation() {
        let state = EdgeState::new();
        assert!(!state.is_active);
        assert!(!state.is_hovered);
        assert!(!state.is_bundled);
    }

    #[test]
    fn test_edge_state_bundled() {
        let state = EdgeState::bundled();
        assert!(state.is_bundled);
        assert!(!state.is_active);
        assert!(!state.is_hovered);
    }

    #[test]
    fn test_edge_state_builder() {
        let state = EdgeState::new()
            .with_active(true)
            .with_hovered(true)
            .with_bundled(true);

        assert!(state.is_active);
        assert!(state.is_hovered);
        assert!(state.is_bundled);
    }

    #[test]
    fn test_get_node_style_with_state() {
        let state = NodeState::new().with_active(true);
        let style = get_node_style_with_state(NodeKind::CLASS, state);
        assert_eq!(style.colors.fill, COLOR_ACTIVE_FILL);
    }

    #[test]
    fn test_get_edge_style_with_state() {
        let state = EdgeState::new().with_active(true);
        let style = get_edge_style_with_state(EdgeKind::CALL, state);
        assert_eq!(style.color, COLOR_FOCUS_BORDER);
    }

    #[test]
    fn test_hatching_pattern() {
        let pattern = hatching_pattern();
        assert_eq!(pattern.color, COLOR_HATCHING);
        assert_eq!(pattern.angle, 45.0);
        assert_eq!(pattern.spacing, 8.0);
        assert_eq!(pattern.line_width, 1.5);
    }

    #[test]
    fn test_hatching_pattern_color_has_transparency() {
        let pattern = hatching_pattern();
        // Hatching color should have some transparency for overlay effect
        assert!(pattern.color.a < 255);
    }

    #[test]
    fn test_edge_width_single_edge() {
        // Single edge (bundle_count = 1) should return base width for the kind
        let width = edge_width(EdgeKind::CALL, 1);
        assert_eq!(width, 1.5); // CALL edges have base width 1.5

        let width = edge_width(EdgeKind::MEMBER, 1);
        assert_eq!(width, 1.0); // MEMBER edges have base width 1.0

        let width = edge_width(EdgeKind::INHERITANCE, 1);
        assert_eq!(width, 2.0); // INHERITANCE edges have base width 2.0
    }

    #[test]
    fn test_edge_width_bundle_scaling() {
        // Test logarithmic scaling for bundled edges
        // Formula: min(log2(bundle_count) + 1.0, 6.0)

        // bundle_count = 2: log2(2) + 1.0 = 1.0 + 1.0 = 2.0
        let width = edge_width(EdgeKind::MEMBER, 2);
        assert_eq!(width, 2.0);

        // bundle_count = 4: log2(4) + 1.0 = 2.0 + 1.0 = 3.0
        let width = edge_width(EdgeKind::MEMBER, 4);
        assert_eq!(width, 3.0);

        // bundle_count = 8: log2(8) + 1.0 = 3.0 + 1.0 = 4.0
        let width = edge_width(EdgeKind::MEMBER, 8);
        assert_eq!(width, 4.0);

        // bundle_count = 16: log2(16) + 1.0 = 4.0 + 1.0 = 5.0
        let width = edge_width(EdgeKind::MEMBER, 16);
        assert_eq!(width, 5.0);

        // bundle_count = 32: log2(32) + 1.0 = 5.0 + 1.0 = 6.0
        let width = edge_width(EdgeKind::MEMBER, 32);
        assert_eq!(width, 6.0);
    }

    #[test]
    fn test_edge_width_max_clamping() {
        // Test that width is clamped to MAX_WIDTH (6.0)
        // bundle_count = 64: log2(64) + 1.0 = 6.0 + 1.0 = 7.0, clamped to 6.0
        let width = edge_width(EdgeKind::MEMBER, 64);
        assert_eq!(width, 6.0);

        // bundle_count = 128: log2(128) + 1.0 = 7.0 + 1.0 = 8.0, clamped to 6.0
        let width = edge_width(EdgeKind::MEMBER, 128);
        assert_eq!(width, 6.0);

        // Very large bundle count should still be clamped
        let width = edge_width(EdgeKind::MEMBER, 1000);
        assert_eq!(width, 6.0);
    }

    #[test]
    fn test_edge_width_zero_bundle() {
        // bundle_count = 0 should be treated as non-bundled
        let width = edge_width(EdgeKind::CALL, 0);
        assert_eq!(width, 1.5); // Should return base width for CALL
    }

    #[test]
    fn test_edge_width_respects_kind_base() {
        // For edges with higher base width (like INHERITANCE = 2.0),
        // the result should be at least the kind's base width
        let width = edge_width(EdgeKind::INHERITANCE, 2);
        // log2(2) + 1.0 = 2.0, but INHERITANCE base is 2.0, so should be max(2.0, 2.0) = 2.0
        assert_eq!(width, 2.0);

        // With more edges, should scale up
        let width = edge_width(EdgeKind::INHERITANCE, 4);
        // log2(4) + 1.0 = 3.0, max(3.0, 2.0) = 3.0
        assert_eq!(width, 3.0);
    }
}
