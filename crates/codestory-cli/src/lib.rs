//! Reusable native executable-boundary services owned by the CodeStory CLI.
//!
//! Product orchestration remains in `codestory-runtime`. This library exposes
//! only the native per-user embedding transport so auxiliary CodeStory
//! executables can use the same verified executable, peer, and lifetime
//! authority contract as `codestory-cli`.

// The binary also uses qualification-only transport probes. Auxiliary
// executables need only the install/server entrypoints exposed below.
#[allow(dead_code)]
mod embedding_server_transport;
#[allow(dead_code)]
mod sidecar_runtime;

use anyhow::Result;

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
