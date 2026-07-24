mod failure;
mod target;

pub(super) use failure::{
    StructuredCommandFailure, command_failure_envelope, command_failure_message,
    emit_command_failure, generic_command_failure, json_output_requested,
    quote_command_argument_value, quote_command_path, quote_command_value, requested_output_file,
};
pub(crate) use target::{build_ambiguous_target_error_output, resolve_target_or_emit_ambiguity};
pub(super) use target::{
    resolve_source_target_or_emit_ambiguity, structured_ambiguous_target_failure,
};
