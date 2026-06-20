//! Trail contracts for graph-neighborhood and path views.
//!
//! This module re-exports the core trail model plus API DTOs so callers can use
//! one import path for trail requests and responses. Trail output is navigation
//! evidence: filtering, truncation, and speculative-edge hiding affect what is
//! shown, not what exists in the underlying graph.

pub use crate::api::{TrailConfigDto, TrailContextDto, TrailFilterOptionsDto};
pub use crate::graph::{TrailCallerScope, TrailConfig, TrailDirection, TrailMode, TrailResult};
