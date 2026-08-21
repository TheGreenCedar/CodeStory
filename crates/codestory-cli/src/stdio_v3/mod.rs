mod catalog;
mod diagnostics;
mod discovery;
mod profile;
mod transport;

pub(crate) use profile::McpRevisionV3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevisionNativeToolResultMeasurementV3 {
    pub(crate) revision: McpRevisionV3,
    pub(crate) call_tool_result_bytes: Vec<u8>,
    pub(crate) byte_length: usize,
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
            let result = transport::build_proof_tool_result_v3(*revision, root)?;
            let call_tool_result_bytes =
                crate::stdio_transport::v3_serialize_call_tool_result(&result)
                    .map_err(|error| StdioV3InternalError::Serialization(error.to_string()))?;
            let byte_length = call_tool_result_bytes.len();
            Ok(RevisionNativeToolResultMeasurementV3 {
                revision: *revision,
                call_tool_result_bytes,
                byte_length,
            })
        })
        .collect()
}
