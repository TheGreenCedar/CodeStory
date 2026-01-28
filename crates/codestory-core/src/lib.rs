use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

pub mod access;
pub mod definition;
pub mod error;
pub mod location_type;
pub mod node_type;
pub mod protocol;
pub mod token_component;

pub use access::AccessKind;
pub use definition::DefinitionKind;
pub use error::{ErrorFilter, ErrorInfo, IndexStep};
pub use location_type::LocationType;
pub use node_type::{BundleInfo, NodeType};
pub use token_component::{Token, TokenComponent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId(pub i64);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdgeId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[allow(non_camel_case_types)]
#[repr(i32)]
pub enum NodeKind {
    // Structural
    MODULE,
    NAMESPACE,
    PACKAGE,
    FILE,

    // Types
    STRUCT,
    CLASS,
    INTERFACE,
    ANNOTATION,
    UNION,
    ENUM,
    TYPEDEF,
    TYPE_PARAMETER,
    BUILTIN_TYPE,

    // Callable/Executable
    FUNCTION,
    METHOD,
    MACRO,

    // Variables/Constants
    GLOBAL_VARIABLE,
    FIELD,
    VARIABLE, // Generic variable
    CONSTANT,
    ENUM_CONSTANT,

    // Other
    UNKNOWN,
}

/// Error type for enum conversion failures
#[derive(Error, Debug, Clone)]
pub enum EnumConversionError {
    #[error("Invalid NodeKind value: {0}")]
    InvalidNodeKind(i32),
    #[error("Invalid EdgeKind value: {0}")]
    InvalidEdgeKind(i32),
    #[error("Invalid OccurrenceKind value: {0}")]
    InvalidOccurrenceKind(i32),
}

impl TryFrom<i32> for NodeKind {
    type Error = EnumConversionError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(NodeKind::MODULE),
            1 => Ok(NodeKind::NAMESPACE),
            2 => Ok(NodeKind::PACKAGE),
            3 => Ok(NodeKind::FILE),
            4 => Ok(NodeKind::STRUCT),
            5 => Ok(NodeKind::CLASS),
            6 => Ok(NodeKind::INTERFACE),
            7 => Ok(NodeKind::ANNOTATION),
            8 => Ok(NodeKind::UNION),
            9 => Ok(NodeKind::ENUM),
            10 => Ok(NodeKind::TYPEDEF),
            11 => Ok(NodeKind::TYPE_PARAMETER),
            12 => Ok(NodeKind::BUILTIN_TYPE),
            13 => Ok(NodeKind::FUNCTION),
            14 => Ok(NodeKind::METHOD),
            15 => Ok(NodeKind::MACRO),
            16 => Ok(NodeKind::GLOBAL_VARIABLE),
            17 => Ok(NodeKind::FIELD),
            18 => Ok(NodeKind::VARIABLE),
            19 => Ok(NodeKind::CONSTANT),
            20 => Ok(NodeKind::ENUM_CONSTANT),
            21 => Ok(NodeKind::UNKNOWN),
            _ => Err(EnumConversionError::InvalidNodeKind(value)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[allow(non_camel_case_types)]
#[repr(i32)]
pub enum EdgeKind {
    // Definition/Hierarchy
    MEMBER, // parent defines child

    // Usage
    TYPE_USAGE,
    USAGE,
    CALL,

    // OOP
    INHERITANCE,
    OVERRIDE,

    // Generics
    TYPE_ARGUMENT,
    TEMPLATE_SPECIALIZATION,

    // Imports
    INCLUDE,
    IMPORT,

    // Metaprogramming
    MACRO_USAGE,
    ANNOTATION_USAGE,

    UNKNOWN,
}

impl TryFrom<i32> for EdgeKind {
    type Error = EnumConversionError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(EdgeKind::MEMBER),
            1 => Ok(EdgeKind::TYPE_USAGE),
            2 => Ok(EdgeKind::USAGE),
            3 => Ok(EdgeKind::CALL),
            4 => Ok(EdgeKind::INHERITANCE),
            5 => Ok(EdgeKind::OVERRIDE),
            6 => Ok(EdgeKind::TYPE_ARGUMENT),
            7 => Ok(EdgeKind::TEMPLATE_SPECIALIZATION),
            8 => Ok(EdgeKind::INCLUDE),
            9 => Ok(EdgeKind::IMPORT),
            10 => Ok(EdgeKind::MACRO_USAGE),
            11 => Ok(EdgeKind::ANNOTATION_USAGE),
            12 => Ok(EdgeKind::UNKNOWN),
            _ => Err(EnumConversionError::InvalidEdgeKind(value)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub serialized_name: String, // e.g., "n:my_namespace.c:MyClass"
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Edge {
    pub id: EdgeId,
    pub source: NodeId,
    pub target: NodeId,
    pub kind: EdgeKind,
}

/// Represents a location in a source file.
///
/// All line and column numbers are **1-based** for consistency with the original
/// Sourcetrail database format. Line 1 is the first line of the file, column 1 is
/// the first character of a line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceLocation {
    pub file_node_id: NodeId,
    /// 1-based line number where the location starts
    pub start_line: u32,
    /// 1-based column number where the location starts
    pub start_col: u32,
    /// 1-based line number where the location ends
    pub end_line: u32,
    /// 1-based column number where the location ends
    pub end_col: u32,
}

// Helper types for hierarchical names
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct NameHierarchy {
    pub name_delimiter: String,
    pub name_elements: Vec<String>,
}

impl NameHierarchy {
    pub fn new(delimiter: &str) -> Self {
        Self {
            name_delimiter: delimiter.to_string(),
            name_elements: Vec::new(),
        }
    }

    pub fn push(&mut self, element: &str) {
        self.name_elements.push(element.to_string());
    }

    pub fn serialize_to_string(&self) -> String {
        self.name_elements.join(&self.name_delimiter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[allow(non_camel_case_types)]
#[repr(i32)]
pub enum OccurrenceKind {
    DEFINITION,
    REFERENCE,
    DECLARATION,
    MACRO_DEFINITION,
    MACRO_REFERENCE,
    UNKNOWN,
}

impl TryFrom<i32> for OccurrenceKind {
    type Error = EnumConversionError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(OccurrenceKind::DEFINITION),
            1 => Ok(OccurrenceKind::REFERENCE),
            2 => Ok(OccurrenceKind::DECLARATION),
            3 => Ok(OccurrenceKind::MACRO_DEFINITION),
            4 => Ok(OccurrenceKind::MACRO_REFERENCE),
            5 => Ok(OccurrenceKind::UNKNOWN),
            _ => Err(EnumConversionError::InvalidOccurrenceKind(value)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct QualifiedName {
    /// The local name of the symbol (e.g., "MyClass")
    pub name: String,
    /// The fully qualified scope (e.g., "com.example")
    pub scope: String,
    /// The full unique identifier (e.g., "com.example.MyClass")
    pub full_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Occurrence {
    pub element_id: i64, // Can be a NodeId or EdgeId
    pub kind: OccurrenceKind,
    pub location: SourceLocation,
}

// ============================================================================
// Bookmark Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkCategory {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: i64,
    pub category_id: i64,
    pub node_id: NodeId,
    pub comment: Option<String>,
}

// ============================================================================
// Trail Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrailDirection {
    Incoming,
    Outgoing,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailConfig {
    pub root_id: NodeId,
    pub depth: u32,
    pub direction: TrailDirection,
    pub edge_filter: Vec<EdgeKind>,
    pub max_nodes: usize,
}

impl Default for TrailConfig {
    fn default() -> Self {
        Self {
            root_id: NodeId(0),
            depth: 2,
            direction: TrailDirection::Both,
            edge_filter: vec![],
            max_nodes: 500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TrailResult {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub depth_map: std::collections::HashMap<NodeId, u32>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LayoutDirection {
    #[default]
    Horizontal,
    Vertical,
}
