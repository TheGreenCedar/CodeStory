//! Structural checks that decide whether one *cited anchor* proves one specific flow
//! requirement.
//!
//! Requirement coverage used to be decided by the requirement's `FlowRole`: any claim whose
//! wording produced that role closed every requirement wearing it. Two requirements in the same
//! flow routinely share a role — a client's request finalization and its transport send are both
//! steps of one dispatch — so a single piece of evidence closed both, and prose alone could close
//! either. Every carrier here reads only the citation, so a requirement is closed by evidence for
//! that requirement and by nothing else.

use crate::agent::packet_scoring::{normalize_identifier, packet_display_path};
use codestory_contracts::api::{AgentCitationDto, NodeKind};

fn display(citation: &AgentCitationDto) -> String {
    normalize_identifier(&citation.display_name)
}

fn terminal(citation: &AgentCitationDto) -> String {
    normalize_identifier(&crate::terminal_symbol_segment(&citation.display_name))
}

fn path(citation: &AgentCitationDto) -> String {
    citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn owns_behavior(citation: &AgentCitationDto) -> bool {
    matches!(
        citation.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::CLASS | NodeKind::STRUCT
    )
}

fn has_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn path_has_any_extension(citation: &AgentCitationDto, extensions: &[&str]) -> bool {
    let path = path(citation);
    extensions.iter().any(|extension| path.ends_with(extension))
}

// ---------------------------------------------------------------------------
// HTTP client lifecycle
// ---------------------------------------------------------------------------

/// The convenience request method a caller reaches first: a verb-named method on a client type.
/// Distinct from the factory that builds the client and from the adapter that finally sends.
pub(crate) fn citation_owns_client_request_method(citation: &AgentCitationDto) -> bool {
    matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        && matches!(
            terminal(citation).as_str(),
            "request" | "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
        )
}

/// The step that turns a configured request into a transport-ready one.
pub(crate) fn citation_owns_client_request_finalization(citation: &AgentCitationDto) -> bool {
    if !owns_behavior(citation) {
        return false;
    }
    let display = display(citation);
    has_any(
        &display,
        &[
            "finalize",
            "finalise",
            "prepare",
            "tohttprequest",
            "buildrequest",
            "requestbody",
        ],
    )
}

/// The boundary where a transport response becomes a value the caller can read.
pub(crate) fn citation_owns_client_response_materialization(citation: &AgentCitationDto) -> bool {
    if !owns_behavior(citation) {
        return false;
    }
    let display = display(citation);
    display.contains("response")
        && has_any(
            &display,
            &[
                "stream",
                "frombytes",
                "materiali",
                "settle",
                "transform",
                "body",
                "read",
            ],
        )
}

// ---------------------------------------------------------------------------
// Data-fetching hook + cache
// ---------------------------------------------------------------------------

pub(crate) fn citation_owns_hook_public_export(citation: &AgentCitationDto) -> bool {
    if !matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD) {
        return false;
    }
    let display = display(citation);
    display.starts_with("use") && display.len() > 3 && !display.contains("cache")
}

pub(crate) fn citation_owns_hook_key_serialization(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let display = display(citation);
        display.contains("serialize")
            || (display.contains("key") && has_any(&display, &["hash", "stable", "stringify"]))
    }
}

pub(crate) fn citation_owns_hook_cache_helper(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && display(citation).contains("cache")
}

pub(crate) fn citation_owns_hook_mutation_flow(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && has_any(&display(citation), &["mutate", "mutation"])
}

// ---------------------------------------------------------------------------
// HTML / CSS structure
// ---------------------------------------------------------------------------

fn is_markup_document(citation: &AgentCitationDto) -> bool {
    path_has_any_extension(citation, &[".html", ".htm", ".xhtml", ".vue", ".svelte"])
}

fn is_stylesheet(citation: &AgentCitationDto) -> bool {
    path_has_any_extension(citation, &[".css", ".scss", ".sass", ".less"])
}

pub(crate) fn citation_owns_html_app_shell(citation: &AgentCitationDto) -> bool {
    is_markup_document(citation)
        && has_any(
            &display(citation),
            &[
                "app", "root", "main", "body", "shell", "module", "script", "mount",
            ],
        )
}

pub(crate) fn citation_owns_css_structure(citation: &AgentCitationDto) -> bool {
    is_stylesheet(citation)
}

pub(crate) fn citation_owns_css_animation_entrypoint(citation: &AgentCitationDto) -> bool {
    is_stylesheet(citation) && has_any(&display(citation), &["import", "use", "forward"])
}

pub(crate) fn citation_owns_css_animation_structure(citation: &AgentCitationDto) -> bool {
    is_stylesheet(citation)
        && has_any(
            &display(citation),
            &[
                "keyframes",
                "animation",
                "animated",
                "transition",
                "duration",
                "delay",
                "iteration",
                "fillmode",
            ],
        )
}

// ---------------------------------------------------------------------------
// Form validation
// ---------------------------------------------------------------------------

fn is_form_surface(citation: &AgentCitationDto) -> bool {
    is_markup_document(citation)
        || path_has_any_extension(citation, &[".js", ".mjs", ".ts", ".jsx", ".tsx"])
}

pub(crate) fn citation_owns_form_native_constraint(citation: &AgentCitationDto) -> bool {
    is_form_surface(citation)
        && has_any(
            &display(citation),
            &[
                "required",
                "pattern",
                "minlength",
                "maxlength",
                "min",
                "max",
                "inputtype",
            ],
        )
}

pub(crate) fn citation_owns_form_custom_validation(citation: &AgentCitationDto) -> bool {
    is_form_surface(citation)
        && has_any(
            &display(citation),
            &[
                "setcustomvalidity",
                "checkvalidity",
                "reportvalidity",
                "validity",
                "customvalid",
                "validate",
                "validator",
            ],
        )
}

pub(crate) fn citation_owns_form_submit_guard(citation: &AgentCitationDto) -> bool {
    is_form_surface(citation) && {
        let display = display(citation);
        display.contains("submit") || display.contains("preventdefault")
    }
}

// ---------------------------------------------------------------------------
// Shell installers
// ---------------------------------------------------------------------------

fn is_shell_script(citation: &AgentCitationDto) -> bool {
    let path = path(citation);
    path.ends_with(".sh")
        || path.ends_with(".bash")
        || path.ends_with(".zsh")
        || path.ends_with("install")
}

pub(crate) fn citation_owns_shell_installer_bootstrap(citation: &AgentCitationDto) -> bool {
    is_shell_script(citation)
        && has_any(
            &display(citation),
            &["install", "bootstrap", "download", "setup", "source"],
        )
}

pub(crate) fn citation_owns_shell_function_dispatch(citation: &AgentCitationDto) -> bool {
    is_shell_script(citation)
        && has_any(
            &display(citation),
            &["dispatch", "command", "use", "run", "exec", "case"],
        )
}

pub(crate) fn citation_owns_shell_completion(citation: &AgentCitationDto) -> bool {
    is_shell_script(citation)
        && has_any(
            &display(citation),
            &["completion", "compgen", "complete", "alias"],
        )
}

// ---------------------------------------------------------------------------
// Buffered IO
// ---------------------------------------------------------------------------

fn names_buffer(citation: &AgentCitationDto) -> bool {
    let display = display(citation);
    display.contains("buffer") || display.contains("segment")
}

fn names_io_operation(display: &str) -> bool {
    has_any(
        display,
        &[
            "read", "write", "emit", "flush", "skip", "copyto", "request",
        ],
    )
}

/// The buffer itself — where bytes live between a source and a sink.
pub(crate) fn citation_owns_buffer_storage(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && names_buffer(citation) && !names_io_operation(&display(citation))
}

/// The operations that move bytes across that buffer. Sibling of `buffer_storage`, so a citation
/// that only names the container must not close it.
pub(crate) fn citation_owns_buffer_read_write(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && names_io_operation(&display(citation)) && {
        let display = display(citation);
        names_buffer(citation) || has_any(&display, &["source", "sink", "stream"])
    }
}

// ---------------------------------------------------------------------------
// Logger record + handler
// ---------------------------------------------------------------------------

pub(crate) fn citation_owns_log_record_creation(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let display = display(citation);
        display.contains("record")
            && !display.contains("handler")
            && (has_any(&display, &["add", "create", "make", "build", "log"])
                || display == "record")
    }
}

/// Processing a record, not registering something that might: a symbol that pushes a handler onto
/// a stack names a handler but does nothing with a record, so it must not close this requirement.
pub(crate) fn citation_owns_log_handler_processing(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let display = display(citation);
        let names_a_handler = display.contains("handler") || display.contains("handle");
        let only_registers = has_any(
            &display,
            &["push", "pop", "add", "remove", "set", "register"],
        );
        names_a_handler
            && !only_registers
            && has_any(
                &display,
                &[
                    "handle",
                    "process",
                    "write",
                    "emit",
                    "flush",
                    "batch",
                    "interface",
                ],
            )
    }
}

// ---------------------------------------------------------------------------
// Static-site build
// ---------------------------------------------------------------------------

pub(crate) fn citation_owns_site_lifecycle(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let display = display(citation);
        has_any(&display, &["site", "build", "process", "pipeline"])
            && !has_any(&display, &["render", "write", "read"])
    }
}

pub(crate) fn citation_owns_site_terminal(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && has_any(
            &display(citation),
            &["render", "writer", "write", "reader", "output", "emit"],
        )
}

// ---------------------------------------------------------------------------
// Object mapper
// ---------------------------------------------------------------------------

pub(crate) fn citation_owns_mapper_configuration(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let display = display(citation);
        has_any(&display, &["configuration", "config", "profile", "options"])
            && !has_any(&display, &["plan", "execut", "pipeline"])
    }
}

pub(crate) fn citation_owns_mapper_execution(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let display = display(citation);
        display.contains("typemap")
            || (has_any(
                &display,
                &["plan", "execut", "pipeline", "mapper", "mapping"],
            ) && !has_any(&display, &["configuration", "config", "profile", "options"]))
    }
}

// ---------------------------------------------------------------------------
// Runtime formatting
// ---------------------------------------------------------------------------

/// The type-erased argument store a runtime formatter reads from.
pub(crate) fn citation_owns_format_arguments(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let display = display(citation);
        display.contains("format")
            && has_any(&display, &["arg", "args", "arguments", "store", "value"])
            && !display.contains("error")
    }
}

/// The error/fallback path a runtime formatter takes when an argument cannot be formatted. This is
/// the only carrier for `FlowRole::ErrorOrFallback`; without it the role would ask for evidence no
/// packet could ever cite.
pub(crate) fn citation_owns_format_errors(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let display = display(citation);
        has_any(
            &display,
            &["error", "throw", "fail", "assert", "fallback", "panic"],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{NodeId, SearchHitOrigin};

    fn citation(display_name: &str, file_path: &str, kind: NodeKind) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(display_name.to_string()),
            display_name: display_name.to_string(),
            kind,
            file_path: Some(file_path.to_string()),
            line: Some(1),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
        }
    }

    #[test]
    fn buffer_container_and_buffer_operations_are_separate_carriers() {
        let container = citation("Buffer", "src/io/buffer.kt", NodeKind::CLASS);
        let operation = citation("Buffer.writeUtf8", "src/io/buffer.kt", NodeKind::METHOD);

        assert!(citation_owns_buffer_storage(&container));
        assert!(!citation_owns_buffer_read_write(&container));
        assert!(citation_owns_buffer_read_write(&operation));
        assert!(!citation_owns_buffer_storage(&operation));
    }

    #[test]
    fn format_error_evidence_is_reachable_and_distinct_from_argument_evidence() {
        let arguments = citation(
            "dynamic_format_arg_store",
            "include/fmt/args.h",
            NodeKind::CLASS,
        );
        let errors = citation(
            "throw_format_error",
            "include/fmt/format.h",
            NodeKind::FUNCTION,
        );

        assert!(citation_owns_format_arguments(&arguments));
        assert!(!citation_owns_format_errors(&arguments));
        assert!(citation_owns_format_errors(&errors));
        assert!(!citation_owns_format_arguments(&errors));
    }

    #[test]
    fn a_client_factory_does_not_carry_the_request_method() {
        let factory = citation("createInstance", "lib/axios.js", NodeKind::FUNCTION);
        let request = citation(
            "Axios.prototype.request",
            "lib/core/Axios.js",
            NodeKind::METHOD,
        );

        assert!(!citation_owns_client_request_method(&factory));
        assert!(citation_owns_client_request_method(&request));
    }
}
