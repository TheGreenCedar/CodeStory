//! Command-line integration and reusable executable-boundary services.
//!
//! Product orchestration remains in `codestory-runtime`. This library owns one
//! CLI module graph and exposes the native per-user embedding entrypoints so
//! auxiliary CodeStory executables use the same verified executable, peer, and
//! lifetime authority contract as `codestory-cli`.

mod app;
mod args;
mod cache_reset;
mod config;
mod diagnostics;
mod display;
mod drill_targeting;
mod embedding_config;
mod embedding_qualification;
mod embedding_server_transport;
mod explore;
mod file_state;
mod http_transport;
mod local_refresh_status;
#[allow(dead_code)]
mod output;
mod prove_call_path;
mod readiness;
mod report;
mod retrieval;
mod runtime;
mod sidecar_runtime;
#[cfg(test)]
mod status_wire_test_support;
mod stdio_arguments;
mod stdio_catalog;
#[allow(dead_code)]
mod stdio_transport;
#[allow(dead_code)]
mod stdio_v3;

/// Sealed Q1 conformance for shipping evidence-only v3 without proof support.
/// It registers no command, route, transport, or public tool.
#[cfg(feature = "v3-evidence-separation-support")]
#[doc(hidden)]
pub mod v3_evidence_separation_support {
    /// Validate all four revision-native evidence-only catalogs and results.
    pub fn validate() -> Result<(), String> {
        crate::stdio_v3::validate_evidence_only_surface_v3()
    }
}

/// Benchmark-only proof qualification observations. This module is not part of
/// the default CLI module graph and registers no command, transport, or tool.
#[cfg(feature = "proof-qualification-support")]
#[doc(hidden)]
pub mod proof_qualification_support {
    use std::collections::BTreeMap;

    use serde_json::Value;

    /// Exact revision-native `CallToolResult` bytes observed by qualification.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RevisionNativeToolResultMeasurement {
        pub revision: String,
        pub call_tool_result_bytes: Vec<u8>,
        pub byte_length: usize,
        pub elapsed_ns: u64,
    }

    /// Closed transport failures preserved from the revision-native builder.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ProofQualificationTransportError {
        Serialization(String),
        InvalidProjection(String),
        OutputSchemaViolation,
        ResultExceedsBudget {
            maximum_bytes: usize,
            actual_bytes: usize,
        },
    }

    impl From<crate::stdio_v3::StdioV3InternalError> for ProofQualificationTransportError {
        fn from(error: crate::stdio_v3::StdioV3InternalError) -> Self {
            match error {
                crate::stdio_v3::StdioV3InternalError::Serialization(message) => {
                    Self::Serialization(message)
                }
                crate::stdio_v3::StdioV3InternalError::InvalidProjection(message) => {
                    Self::InvalidProjection(message)
                }
                crate::stdio_v3::StdioV3InternalError::OutputSchemaViolation => {
                    Self::OutputSchemaViolation
                }
                crate::stdio_v3::StdioV3InternalError::ResultExceedsBudget {
                    maximum_bytes,
                    actual_bytes,
                } => Self::ResultExceedsBudget {
                    maximum_bytes,
                    actual_bytes,
                },
            }
        }
    }

    /// Build the exact proof result bytes for every supported dark revision.
    pub fn measure_revision_native_proof_result(
        root: &Value,
    ) -> Result<Vec<RevisionNativeToolResultMeasurement>, ProofQualificationTransportError> {
        crate::stdio_v3::measure_revision_native_proof_result_v3(root)
            .map(|measurements| {
                measurements
                    .into_iter()
                    .map(|measurement| RevisionNativeToolResultMeasurement {
                        revision: measurement.revision.as_str().to_owned(),
                        call_tool_result_bytes: measurement.call_tool_result_bytes,
                        byte_length: measurement.byte_length,
                        elapsed_ns: measurement.elapsed_ns,
                    })
                    .collect()
            })
            .map_err(ProofQualificationTransportError::from)
    }

    /// Rust-generated discovery identities for the inert launcher handshake.
    pub fn discovery_contracts() -> BTreeMap<String, String> {
        crate::stdio_v3::McpRevisionV3::all()
            .iter()
            .map(|revision| {
                let identity = crate::stdio_v3::discovery_contract_v3(*revision);
                (revision.as_str().to_owned(), identity.sha256)
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn transport_errors_preserve_every_revision_native_variant() {
            let cases = [
                (
                    crate::stdio_v3::StdioV3InternalError::Serialization("encode".into()),
                    ProofQualificationTransportError::Serialization("encode".into()),
                ),
                (
                    crate::stdio_v3::StdioV3InternalError::InvalidProjection("root".into()),
                    ProofQualificationTransportError::InvalidProjection("root".into()),
                ),
                (
                    crate::stdio_v3::StdioV3InternalError::OutputSchemaViolation,
                    ProofQualificationTransportError::OutputSchemaViolation,
                ),
                (
                    crate::stdio_v3::StdioV3InternalError::ResultExceedsBudget {
                        maximum_bytes: 64,
                        actual_bytes: 65,
                    },
                    ProofQualificationTransportError::ResultExceedsBudget {
                        maximum_bytes: 64,
                        actual_bytes: 65,
                    },
                ),
            ];
            for (internal, expected) in cases {
                assert_eq!(ProofQualificationTransportError::from(internal), expected);
            }
        }
    }
}

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
    attach_complete_publication, local_refresh_output_from_summary, preflight_output_file,
};

/// Install the native same-user embedding client transport for this executable.
pub fn install_native_embedding_client_transport() -> Result<()> {
    embedding_server_transport::install_client_transport(
        embedding_server_transport::ClientTransportMode::SpawnCapable,
    )
}

/// Run the native embedding server entrypoint for this exact executable.
pub fn run_native_embedding_server() -> Result<()> {
    diagnostics::install_process_diagnostics();
    embedding_server_transport::run_internal_embedding_server()
}
