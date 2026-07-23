//! Command-line integration and reusable executable-boundary services.
//!
//! Product orchestration remains in `codestory-runtime`. This library owns one
//! CLI module graph and exposes the native per-user embedding entrypoints so
//! auxiliary CodeStory executables use the same verified executable, peer, and
//! lifetime authority contract as `codestory-cli`.

mod app;
mod args;
mod config;
mod display;
mod drill_targeting;
mod embedding_config;
mod embedding_qualification;
mod embedding_server_transport;
mod explore;
mod file_state;
mod http_transport;
mod local_refresh_status;
mod output;
mod readiness;
mod report;
mod retrieval;
mod runtime;
mod sidecar_runtime;
mod stdio_catalog;
mod stdio_transport;

use anyhow::Result;

/// Parse arguments and run the CodeStory CLI.
pub use app::run;
pub(crate) use app::{
    attach_complete_publication, build_ambiguous_target_error_output,
    build_query_resolution_output, build_readiness_lanes_for_runtime, build_search_hit_output,
    build_summary_readiness, doctor_sidecar_status, ensure_dot_only_for_trail,
    local_refresh_output_from_summary, packet_sufficiency_label, preflight_output_file,
    resolve_target_or_emit_ambiguity,
};

/// Install the native same-user embedding client transport for this executable.
pub fn install_native_embedding_client_transport() -> Result<()> {
    embedding_server_transport::install_client_transport(
        embedding_server_transport::ClientTransportMode::SpawnCapable,
    )
}

/// Run the native embedding server entrypoint for this exact executable.
pub fn run_native_embedding_server() -> Result<()> {
    embedding_server_transport::run_internal_embedding_server()
}
