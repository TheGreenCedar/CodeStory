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

pub(crate) use app::artifacts::ensure_dot_only_for_trail;
pub(crate) use app::diagnostics::{
    build_readiness_lanes_for_runtime, build_summary_readiness, doctor_sidecar_status,
};
pub(crate) use app::rendering::{build_query_resolution_output, build_search_hit_output};
pub(crate) use app::resolution::{
    build_ambiguous_target_error_output, resolve_target_or_emit_ambiguity,
};
/// Parse arguments and run the CodeStory CLI.
pub use app::run;
pub(crate) use app::{
    attach_complete_publication, local_refresh_output_from_summary, packet_sufficiency_label,
    preflight_output_file,
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

/// Capture the platform clock shared by native embedding client/server proof.
#[doc(hidden)]
pub fn native_embedding_qualification_clock()
-> Result<std::sync::Arc<dyn codestory_retrieval::AwakeMonotonicClock>> {
    embedding_server_transport::qualification_clock()
}

/// Read the suspend-inclusive monotonic clock used by qualification evidence.
#[doc(hidden)]
pub fn native_embedding_qualification_inclusive_now_ns() -> Result<u64> {
    embedding_server_transport::inclusive_now_ns()
}

/// Name the suspend-inclusive clock API used by qualification evidence.
#[doc(hidden)]
pub fn native_embedding_qualification_inclusive_clock_api() -> &'static str {
    embedding_server_transport::inclusive_clock_api()
}

/// Read the platform boot identity used to correlate qualification clocks.
#[doc(hidden)]
pub fn native_embedding_qualification_boot_id() -> Result<String> {
    embedding_server_transport::boot_id()
}
