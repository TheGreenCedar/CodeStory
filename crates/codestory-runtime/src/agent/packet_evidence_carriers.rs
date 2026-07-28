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

fn terminal(citation: &AgentCitationDto) -> String {
    normalize_identifier(&crate::terminal_symbol_segment(&citation.display_name))
}

/// The last `::`/`.`/`/` segment of a symbol name with its original casing intact. Case carries
/// meaning for naming conventions — `useData` is a hook and `userProfile` is not — so the
/// lowercased forms used for matching cannot answer that question.
fn terminal_segment_raw(display_name: &str) -> &str {
    display_name
        .rsplit([':', '.', '/', '\\'])
        .next()
        .unwrap_or(display_name)
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

fn path_has_any_extension(citation: &AgentCitationDto, extensions: &[&str]) -> bool {
    let path = path(citation);
    extensions.iter().any(|extension| path.ends_with(extension))
}

// ---------------------------------------------------------------------------
// Token matching
//
// A carrier used to ask whether the flattened, lowercased symbol name *contained* a needle. That
// accepts any symbol in the repository whose letters happen to line up: `adminPanel`,
// `terminalWidth` and `determineFieldOrder` all contain "min", `invalidateRecordCache` contains
// "validate", and `userProfile` starts with "use". Matching whole tokens instead means a needle has
// to name a word the author actually wrote.
// ---------------------------------------------------------------------------

/// Split an identifier into lowercase word tokens: separators, `camelCase` humps, acronym runs and
/// letter/digit transitions are all boundaries. `basic_format_args` becomes
/// `["basic", "format", "args"]` and `determineFieldOrder` becomes
/// `["determine", "field", "order"]` — which no longer contains "min".
fn identifier_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for run in value.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        push_case_split_tokens(run, &mut tokens);
    }
    tokens
}

fn push_case_split_tokens(run: &str, tokens: &mut Vec<String>) {
    let chars: Vec<char> = run.chars().collect();
    if chars.is_empty() {
        return;
    }
    let mut start = 0;
    for index in 1..chars.len() {
        let previous = chars[index - 1];
        let current = chars[index];
        // `fooBar` and `format2Args` break before the hump; `HTTPClient` breaks before the last
        // capital of an acronym run so it yields "http" and "client".
        let leaves_lower_run = !previous.is_ascii_uppercase() && current.is_ascii_uppercase();
        let enters_word_after_acronym = previous.is_ascii_uppercase()
            && current.is_ascii_uppercase()
            && chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
        let crosses_digit_boundary = previous.is_ascii_digit() != current.is_ascii_digit();
        if leaves_lower_run || enters_word_after_acronym || crosses_digit_boundary {
            tokens.push(
                chars[start..index]
                    .iter()
                    .collect::<String>()
                    .to_lowercase(),
            );
            start = index;
        }
    }
    tokens.push(chars[start..].iter().collect::<String>().to_lowercase());
}

fn name_tokens(citation: &AgentCitationDto) -> Vec<String> {
    identifier_tokens(&citation.display_name)
}

fn path_tokens(citation: &AgentCitationDto) -> Vec<String> {
    identifier_tokens(&path(citation))
}

/// True when the citation's own name contains one of `needles` as a whole token.
fn names_token(citation: &AgentCitationDto, needles: &[&str]) -> bool {
    has_token(&name_tokens(citation), needles)
}

/// True when either the citation's name or the file it lives in contains one of `needles` as a
/// whole token. This is how a carrier asks "does this anchor belong to my subsystem at all?".
fn names_or_path_token(citation: &AgentCitationDto, needles: &[&str]) -> bool {
    has_token(&name_tokens(citation), needles) || has_token(&path_tokens(citation), needles)
}

fn has_token(tokens: &[String], needles: &[&str]) -> bool {
    tokens
        .iter()
        .any(|token| needles.iter().any(|needle| token == needle))
}

/// Token-anchored prefix match, for stems that appear with several endings (`execute`,
/// `execution`, `executor`). Still anchored at a word boundary, unlike a bare substring.
fn any_token_starts_with(tokens: &[String], prefixes: &[&str]) -> bool {
    tokens
        .iter()
        .any(|token| prefixes.iter().any(|prefix| token.starts_with(prefix)))
}

fn names_token_prefix(citation: &AgentCitationDto, prefixes: &[&str]) -> bool {
    any_token_starts_with(&name_tokens(citation), prefixes)
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

/// Anchors that belong to an HTTP client at all. `Uri.prepare` is a URL utility that happens to be
/// named "prepare"; without this scoping it closed the finalization step of any client flow.
fn belongs_to_http_client(citation: &AgentCitationDto) -> bool {
    names_or_path_token(
        citation,
        &[
            "request",
            "requests",
            "http",
            "https",
            "client",
            "clients",
            "adapter",
            "adapters",
            "transport",
            "send",
            "fetch",
        ],
    )
}

/// The step that turns a configured request into a transport-ready one.
pub(crate) fn citation_owns_client_request_finalization(citation: &AgentCitationDto) -> bool {
    if !owns_behavior(citation) || !belongs_to_http_client(citation) {
        return false;
    }
    names_token_prefix(citation, &["finaliz", "finalis", "prepar"])
        || (names_token(citation, &["request", "requests"])
            && names_token(citation, &["to", "build", "body"]))
}

/// The boundary where a transport response becomes a value the caller can read.
pub(crate) fn citation_owns_client_response_materialization(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && names_token(citation, &["response", "responses"])
        && (names_token(
            citation,
            &[
                "stream",
                "bytes",
                "settle",
                "settled",
                "transform",
                "body",
                "read",
            ],
        ) || names_token_prefix(citation, &["materiali"]))
}

// ---------------------------------------------------------------------------
// Data-fetching hook + cache
// ---------------------------------------------------------------------------

/// The hook flow lives in front-end scripts. `Cache.write` in `lib/cache.rb` is a server-side cache
/// and must not stand in for the hook's cache helper.
fn is_script_surface(citation: &AgentCitationDto) -> bool {
    path_has_any_extension(
        citation,
        &[
            ".js", ".mjs", ".cjs", ".ts", ".mts", ".cts", ".jsx", ".tsx", ".vue", ".svelte",
        ],
    )
}

/// `use` followed by a capital is the hook naming convention — `useData`, `useQuery`.
/// `userProfile` and `useragentString` merely start with the same three letters, and a
/// `starts_with("use")` test could not tell them apart.
fn names_a_hook(citation: &AgentCitationDto) -> bool {
    let segment = terminal_segment_raw(&citation.display_name);
    let Some(rest) = segment.strip_prefix("use") else {
        return false;
    };
    match rest.chars().next() {
        Some(next) => next.is_ascii_uppercase() || next == '_' || next == '-',
        None => false,
    }
}

pub(crate) fn citation_owns_hook_public_export(citation: &AgentCitationDto) -> bool {
    matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        && names_a_hook(citation)
        && !names_token(citation, &["cache", "caches"])
}

pub(crate) fn citation_owns_hook_key_serialization(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && is_script_surface(citation)
        && (names_token_prefix(citation, &["serializ", "serialis"])
            || (names_token(citation, &["key", "keys"])
                && names_token(citation, &["hash", "stable", "stringify"])))
}

pub(crate) fn citation_owns_hook_cache_helper(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && is_script_surface(citation)
        && names_token(citation, &["cache", "caches"])
}

pub(crate) fn citation_owns_hook_mutation_flow(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && is_script_surface(citation)
        && names_token_prefix(citation, &["mutat"])
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
        && names_token(
            citation,
            &[
                "app", "root", "main", "body", "shell", "module", "script", "mount",
            ],
        )
}

pub(crate) fn citation_owns_css_structure(citation: &AgentCitationDto) -> bool {
    is_stylesheet(citation)
}

pub(crate) fn citation_owns_css_animation_entrypoint(citation: &AgentCitationDto) -> bool {
    is_stylesheet(citation) && names_token(citation, &["import", "use", "forward"])
}

pub(crate) fn citation_owns_css_animation_structure(citation: &AgentCitationDto) -> bool {
    is_stylesheet(citation)
        // Sibling of `css_animation_entrypoint`. `@import "animations/base"` names the animation
        // directory, so without this an import closed the structure requirement too.
        && !citation_owns_css_animation_entrypoint(citation)
        && names_token(
            citation,
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

/// A form validation anchor has to be *about a form*, not merely live in a file a browser can load.
/// Being a script or a document was the only scoping these carriers had, so `determineFieldOrder`
/// in `src/layout.js` (whose name contains "min") and `submitTelemetry` in `src/telemetry.js` closed
/// requirements about form markup they never touch.
fn is_form_validation_surface(citation: &AgentCitationDto) -> bool {
    let on_a_browser_surface = is_markup_document(citation)
        || path_has_any_extension(citation, &[".js", ".mjs", ".cjs", ".ts", ".jsx", ".tsx"]);
    on_a_browser_surface
        && names_or_path_token(
            citation,
            &[
                "form",
                "forms",
                "fieldset",
                "validation",
                "validations",
                "validate",
                "validates",
                "validity",
                "invalid",
                "constraint",
                "constraints",
                "guard",
                "guards",
                "preventdefault",
            ],
        )
}

pub(crate) fn citation_owns_form_native_constraint(citation: &AgentCitationDto) -> bool {
    is_form_validation_surface(citation)
        && names_token(
            citation,
            &[
                "required",
                "pattern",
                "minlength",
                "maxlength",
                "min",
                "max",
                "inputtype",
                "inputmode",
            ],
        )
}

pub(crate) fn citation_owns_form_custom_validation(citation: &AgentCitationDto) -> bool {
    is_form_validation_surface(citation)
        && (names_token(
            citation,
            &[
                "validity",
                "validate",
                "validates",
                "validator",
                "validation",
            ],
        ) || names_token_prefix(citation, &["customvalid", "checkvalid", "reportvalid"]))
}

pub(crate) fn citation_owns_form_submit_guard(citation: &AgentCitationDto) -> bool {
    is_form_validation_surface(citation)
        && (names_token(citation, &["submit", "submits", "preventdefault"])
            || names_token_prefix(citation, &["submitt"]))
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
        && names_token_prefix(
            citation,
            &["install", "bootstrap", "download", "setup", "source"],
        )
}

pub(crate) fn citation_owns_shell_function_dispatch(citation: &AgentCitationDto) -> bool {
    is_shell_script(citation)
        && (names_token(citation, &["use", "run", "exec", "case"])
            || names_token_prefix(citation, &["dispatch", "command", "exec"]))
}

pub(crate) fn citation_owns_shell_completion(citation: &AgentCitationDto) -> bool {
    is_shell_script(citation)
        && names_token_prefix(citation, &["completion", "compgen", "complete", "alias"])
}

// ---------------------------------------------------------------------------
// Buffered IO
// ---------------------------------------------------------------------------

fn names_buffer(citation: &AgentCitationDto) -> bool {
    names_token_prefix(citation, &["buffer", "segment"])
}

fn names_io_operation(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "read", "reads", "write", "writes", "emit", "emits", "flush", "skip", "copy", "copyto",
            "request",
        ],
    ) || names_token_prefix(citation, &["readfrom", "writeto", "copyto"])
}

/// The buffer itself — where bytes live between a source and a sink.
pub(crate) fn citation_owns_buffer_storage(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && names_buffer(citation) && !names_io_operation(citation)
}

/// The operations that move bytes across that buffer. Sibling of `buffer_storage`, so a citation
/// that only names the container must not close it.
pub(crate) fn citation_owns_buffer_read_write(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && names_io_operation(citation)
        && (names_buffer(citation) || names_token(citation, &["source", "sink", "stream"]))
}

// ---------------------------------------------------------------------------
// Logger record + handler
// ---------------------------------------------------------------------------

pub(crate) fn citation_owns_log_record_creation(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let tokens = name_tokens(citation);
        has_token(&tokens, &["record", "records"])
            && !any_token_starts_with(&tokens, &["handler", "handle"])
            && (has_token(&tokens, &["add", "create", "make", "build", "log"])
                || tokens.as_slice() == ["record"])
    }
}

/// Processing a record, not registering something that might: a symbol that pushes a handler onto
/// a stack names a handler but does nothing with a record, so it must not close this requirement.
pub(crate) fn citation_owns_log_handler_processing(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let tokens = name_tokens(citation);
        let names_a_handler = any_token_starts_with(&tokens, &["handler", "handle"]);
        let only_registers = has_token(
            &tokens,
            &["push", "pop", "add", "remove", "set", "register"],
        );
        names_a_handler
            && !only_registers
            && (has_token(&tokens, &["write", "emit", "flush", "batch", "interface"])
                || any_token_starts_with(&tokens, &["handle", "process"]))
    }
}

// ---------------------------------------------------------------------------
// Static-site build
// ---------------------------------------------------------------------------

/// Anchors that belong to a static-site build. Without this, `Cache.write` in `lib/cache.rb` closed
/// the site's terminal boundary purely because its name contains "write".
fn belongs_to_site_build(citation: &AgentCitationDto) -> bool {
    names_or_path_token(
        citation,
        &[
            "site",
            "sites",
            "page",
            "pages",
            "post",
            "posts",
            "layout",
            "layouts",
            "template",
            "templates",
            "document",
            "documents",
            "collection",
            "collections",
            "static",
            "theme",
            "themes",
            "asset",
            "assets",
            "view",
            "views",
            "render",
            "renderer",
            "generator",
        ],
    )
}

pub(crate) fn citation_owns_site_lifecycle(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && belongs_to_site_build(citation) && {
        let tokens = name_tokens(citation);
        has_token(&tokens, &["site", "build", "process", "pipeline"])
            && !has_token(&tokens, &["render", "write", "read"])
            && !any_token_starts_with(&tokens, &["render", "writ", "read"])
    }
}

pub(crate) fn citation_owns_site_terminal(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && belongs_to_site_build(citation)
        && (names_token(citation, &["output", "outputs", "emit", "emits"])
            || names_token_prefix(citation, &["render", "writ", "read"]))
}

// ---------------------------------------------------------------------------
// Object mapper
// ---------------------------------------------------------------------------

/// Anchors that belong to an object mapper. "profile" and "plan" are ordinary words — `userProfile`
/// closed the mapper's configuration requirement until the carrier asked which subsystem it is in.
fn belongs_to_object_mapper(citation: &AgentCitationDto) -> bool {
    names_or_path_token(
        citation,
        &[
            "map", "maps", "mapper", "mappers", "mapping", "mappings", "typemap",
        ],
    )
}

fn names_mapper_configuration(citation: &AgentCitationDto) -> bool {
    names_token_prefix(citation, &["config", "profile", "option"])
}

pub(crate) fn citation_owns_mapper_configuration(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && belongs_to_object_mapper(citation)
        && names_mapper_configuration(citation)
        && !names_token_prefix(citation, &["plan", "execut", "pipeline"])
}

pub(crate) fn citation_owns_mapper_execution(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && belongs_to_object_mapper(citation)
        && !names_mapper_configuration(citation)
        && names_token_prefix(
            citation,
            &["plan", "execut", "pipeline", "mapper", "mapping"],
        )
}

// ---------------------------------------------------------------------------
// Runtime formatting
// ---------------------------------------------------------------------------

/// Anchors that belong to the runtime-formatting subsystem. The error carrier below asks only
/// whether a symbol sounds like a failure path, which every subsystem in a repository has; scoping
/// it to the formatting surface is what stops `CliParseError` in `src/cli/parse.cc` from standing
/// in for the formatter's fallback.
fn belongs_to_runtime_formatting(citation: &AgentCitationDto) -> bool {
    names_or_path_token(
        citation,
        &[
            "format",
            "formats",
            "formatter",
            "formatters",
            "formatting",
            "fmt",
            "vformat",
            "printf",
            "sprintf",
            "fprintf",
        ],
    ) || names_token_prefix(citation, &["format"])
}

/// The type-erased argument store a runtime formatter reads from.
pub(crate) fn citation_owns_format_arguments(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let tokens = name_tokens(citation);
        any_token_starts_with(&tokens, &["format"])
            && has_token(
                &tokens,
                &["arg", "args", "arguments", "store", "value", "values"],
            )
            && !any_token_starts_with(&tokens, &["error", "err"])
    }
}

/// The error/fallback path a runtime formatter takes when an argument cannot be formatted. This is
/// the only carrier for `FlowRole::ErrorOrFallback`; without it the role would ask for evidence no
/// packet could ever cite.
pub(crate) fn citation_owns_format_errors(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && belongs_to_runtime_formatting(citation)
        && names_token_prefix(
            citation,
            &["error", "throw", "fail", "assert", "fallback", "panic"],
        )
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

    #[test]
    fn identifiers_split_into_words_so_a_needle_cannot_match_mid_word() {
        assert_eq!(
            identifier_tokens("determineFieldOrder"),
            ["determine", "field", "order"]
        );
        assert_eq!(identifier_tokens("adminPanel"), ["admin", "panel"]);
        assert_eq!(identifier_tokens("terminalWidth"), ["terminal", "width"]);
        assert_eq!(
            identifier_tokens("invalidateRecordCache"),
            ["invalidate", "record", "cache"]
        );
        assert_eq!(identifier_tokens("minLength"), ["min", "length"]);
        assert_eq!(
            identifier_tokens("basic_format_args"),
            ["basic", "format", "args"]
        );
        assert_eq!(identifier_tokens("HTTPClient"), ["http", "client"]);
        assert_eq!(
            identifier_tokens("Buffer.writeUtf8"),
            ["buffer", "write", "utf", "8"]
        );
    }

    /// Predicate-level statement of the blocking defect: each of these anchors closed a requirement
    /// it has nothing to do with, because the carrier matched an unanchored substring of the name
    /// with no file or subsystem scoping.
    #[test]
    fn carriers_reject_anchors_from_other_subsystems() {
        // "error" anywhere in the repository used to prove the formatter's fallback path.
        assert!(!citation_owns_format_errors(&citation(
            "CliParseError",
            "src/cli/parse.cc",
            NodeKind::FUNCTION
        )));
        assert!(!citation_owns_format_errors(&citation(
            "assert_valid_utf8",
            "src/text/utf8.rs",
            NodeKind::FUNCTION
        )));
        assert!(!citation_owns_format_errors(&citation(
            "panic_hook",
            "src/runtime/panic.rs",
            NodeKind::FUNCTION
        )));
        // ...while the formatter's own failure path still closes it.
        assert!(citation_owns_format_errors(&citation(
            "throw_format_error",
            "include/fmt/format.h",
            NodeKind::FUNCTION
        )));

        // A `use` prefix is not the hook naming convention.
        assert!(!citation_owns_hook_public_export(&citation(
            "userProfile",
            "src/session/user.ts",
            NodeKind::FUNCTION
        )));
        assert!(!citation_owns_hook_public_export(&citation(
            "useragentString",
            "src/http/headers.ts",
            NodeKind::FUNCTION
        )));
        assert!(citation_owns_hook_public_export(&citation(
            "useData",
            "src/index/use-data.ts",
            NodeKind::FUNCTION
        )));

        // "min"/"max" only count as whole words, and only on a form surface.
        for name in ["determineFieldOrder", "adminPanel", "terminalWidth"] {
            assert!(
                !citation_owns_form_native_constraint(&citation(
                    name,
                    "src/layout.js",
                    NodeKind::FUNCTION
                )),
                "{name} names no form constraint"
            );
        }
        assert!(citation_owns_form_native_constraint(&citation(
            "minLength",
            "examples/form.html",
            NodeKind::FUNCTION
        )));

        // A server-side cache is not a static site's terminal boundary.
        assert!(!citation_owns_site_terminal(&citation(
            "Cache.write",
            "lib/cache.rb",
            NodeKind::METHOD
        )));
        assert!(citation_owns_site_terminal(&citation(
            "Renderer.render",
            "lib/site/renderer.rb",
            NodeKind::METHOD
        )));

        // A URL utility named "prepare" is not a request finalization step.
        assert!(!citation_owns_client_request_finalization(&citation(
            "Uri.prepare",
            "lib/uri.dart",
            NodeKind::METHOD
        )));
        assert!(citation_owns_client_request_finalization(&citation(
            "BaseRequest.finalize",
            "lib/base_request.dart",
            NodeKind::METHOD
        )));
    }
}
