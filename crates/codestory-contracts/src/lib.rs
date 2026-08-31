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
pub mod bounded_locks;
pub mod config_registry;
pub mod core_publication;
pub mod events;
pub mod graph;
pub mod grounding;
pub mod installed_agent_timing;
pub mod language_support;
pub mod owned_artifacts;
pub mod packet_projection_v3;
pub mod proof_resolution;
pub mod query;
pub mod trail;
pub mod validation_receipts;

pub mod wire;
pub mod workspace;
