//! Shared contracts for CodeStory crates and API consumers.
//!
//! This crate owns the serializable graph model, API DTOs, event payloads,
//! query/trail shapes, and language support contracts that downstream crates
//! exchange across process, cache, and UI boundaries. Public types here are
//! compatibility surfaces: changing a serialized field name, enum spelling, or
//! readiness meaning can break callers even when Rust still compiles.
//!
//! Keep behavior in producer crates. Keep this crate focused on stable shape,
//! explicit evidence semantics, and small helpers that prevent callers from
//! reinterpreting the same contract differently.

pub mod api;
pub mod events;
pub mod graph;
pub mod grounding;
pub mod language_support;
pub mod query;
pub mod trail;
pub mod workspace;
