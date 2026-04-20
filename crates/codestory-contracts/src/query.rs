use crate::api::{NodeKind, TrailDirection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphQueryAst {
    pub operations: Vec<GraphQueryOperation>,
}

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
