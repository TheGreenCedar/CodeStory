pub(crate) mod catalog;
mod diagnostics;
mod discovery;
mod profile;
mod transport;

pub(crate) use catalog::tools_for_revision_v3;
pub(crate) use diagnostics::{
    DiagnosticsBindingV3, DiagnosticsReadErrorV3, DiagnosticsRegistryV3, attach_capability_uri_v3,
};
pub(crate) use discovery::NativeSessionV3;
#[cfg(feature = "proof-qualification-support")]
pub(crate) use discovery::discovery_contract_v3;
pub(crate) use profile::McpRevisionV3;
pub(crate) use transport::{
    FrameResponseV3, build_proof_tool_result_v3, build_tool_result_v3, jsonrpc_internal_error_v3,
    jsonrpc_invalid_params_v3, process_jsonrpc_frame_v3, semantic_tool_error_v3,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum V3SurfaceSet {
    EvidenceOnly,
    WithProof,
}

impl V3SurfaceSet {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::EvidenceOnly => "evidence_only",
            Self::WithProof => "with_proof",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevisionNativeToolResultMeasurementV3 {
    pub(crate) revision: McpRevisionV3,
    pub(crate) call_tool_result_bytes: Vec<u8>,
    pub(crate) byte_length: usize,
    pub(crate) elapsed_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StdioV3InternalError {
    Serialization(String),
    InvalidProjection(String),
    OutputSchemaViolation,
    ResultExceedsBudget {
        maximum_bytes: usize,
        actual_bytes: usize,
    },
}

#[cfg(feature = "proof-qualification-support")]
pub(crate) fn measure_revision_native_proof_result_v3(
    root: &serde_json::Value,
) -> Result<Vec<RevisionNativeToolResultMeasurementV3>, StdioV3InternalError> {
    McpRevisionV3::all()
        .iter()
        .map(|revision| {
            let started = std::time::Instant::now();
            let result = transport::build_proof_tool_result_v3(*revision, root)?;
            let call_tool_result_bytes =
                crate::stdio_transport::v3_serialize_call_tool_result(&result)
                    .map_err(|error| StdioV3InternalError::Serialization(error.to_string()))?;
            let elapsed_ns = u64::try_from(started.elapsed().as_nanos())
                .expect("revision-native measurement elapsed time exceeds u64 nanoseconds");
            let byte_length = call_tool_result_bytes.len();
            Ok(RevisionNativeToolResultMeasurementV3 {
                revision: *revision,
                call_tool_result_bytes,
                byte_length,
                elapsed_ns,
            })
        })
        .collect()
}

#[cfg(feature = "v3-evidence-separation-support")]
pub(crate) fn validate_evidence_only_surface_v3() -> Result<(), String> {
    let projections =
        codestory_runtime::v3_evidence_qualification_support::real_projection_fixtures(|packet| {
            let root = serde_json::to_value(packet).map_err(|_| ())?;
            McpRevisionV3::all()
                .iter()
                .map(|revision| {
                    let result = transport::build_tool_result_v3(*revision, "packet", &root)
                        .map_err(|_| ())?;
                    crate::stdio_transport::v3_serialize_call_tool_result(&result)
                        .map(|bytes| bytes.len())
                        .map_err(|_| ())
                })
                .max()
                .transpose()
                .map(|maximum| maximum.unwrap_or_default())
        })?;
    let projections = [
        (
            "packet",
            serde_json::to_value(projections.packet).map_err(|error| error.to_string())?,
        ),
        (
            "context",
            serde_json::to_value(projections.context).map_err(|error| error.to_string())?,
        ),
        (
            "search",
            serde_json::to_value(projections.search).map_err(|error| error.to_string())?,
        ),
    ];
    for revision in McpRevisionV3::all() {
        let tools = catalog::tools_for_surface_v3(*revision, V3SurfaceSet::EvidenceOnly);
        if tools.iter().any(|tool| tool["name"] == "prove_call_path") {
            return Err(format!(
                "evidence-only {} advertised prove_call_path",
                revision.as_str()
            ));
        }
        for (name, projection) in &projections {
            if !tools.iter().any(|tool| tool["name"] == *name) {
                return Err(format!(
                    "evidence-only {} omitted {name}",
                    revision.as_str()
                ));
            }
            transport::build_tool_result_v3(*revision, name, projection).map_err(|error| {
                format!("evidence-only {} {name}: {error:?}", revision.as_str())
            })?;
        }
        let evidence_identity =
            discovery::discovery_contract_for_surface_v3(*revision, V3SurfaceSet::EvidenceOnly);
        let proof_identity =
            discovery::discovery_contract_for_surface_v3(*revision, V3SurfaceSet::WithProof);
        if evidence_identity.sha256 == proof_identity.sha256 {
            return Err(format!(
                "evidence-only {} discovery identity was not surface-bound",
                revision.as_str()
            ));
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "v3-evidence-separation-support"))]
mod evidence_separation_tests {
    #[test]
    fn sealed_evidence_only_conformance_covers_all_revisions() {
        super::validate_evidence_only_surface_v3().expect("evidence-only v3 surface conformance");
    }
}
