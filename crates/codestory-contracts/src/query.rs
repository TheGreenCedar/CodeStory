//! Serializable graph-query AST.
//!
//! Query values describe caller intent, not an execution plan. Runtime crates
//! remain responsible for resolving symbols, applying defaults, enforcing
//! limits, and reporting partial evidence when the query cannot be satisfied.

use crate::api::{NodeKind, TrailDirection};
use serde::{Deserialize, Serialize};

/// Ordered pipeline of graph-query operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphQueryAst {
    pub operations: Vec<GraphQueryOperation>,
}

/// One operation in a graph-query pipeline.
///
/// The serde tag is part of the JSON contract; callers should add operations
/// append-only and keep existing tag spellings stable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GraphQueryOperation {
    Trail(TrailQuery),
    Symbol(SymbolQuery),
    Search(SearchQuery),
    Filter(FilterQuery),
    Limit(LimitQuery),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrailQuery {
    pub symbol: String,
    pub depth: Option<u32>,
    pub direction: Option<TrailDirection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolQuery {
    pub query: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchQuery {
    pub query: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilterQuery {
    pub kind: Option<NodeKind>,
    pub file: Option<String>,
    pub depth: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LimitQuery {
    pub count: u32,
}
