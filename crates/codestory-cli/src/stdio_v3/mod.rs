mod catalog;
mod diagnostics;
mod discovery;
mod profile;
mod transport;

pub(crate) use discovery::discovery_contract_v3;
pub(crate) use profile::McpRevisionV3;

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
