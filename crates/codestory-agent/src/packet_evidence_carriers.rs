//! Structural checks that decide whether one *cited anchor* proves one specific flow
//! requirement.
//!
//! Requirement coverage used to be decided by the requirement's `FlowRole`: any claim whose
//! wording produced that role closed every requirement wearing it. Two requirements in the same
//! flow routinely share a role — a client's request finalization and its transport send are both
//! steps of one dispatch — so a single piece of evidence closed both, and prose alone could close
//! either. Every carrier here reads only the citation, never the claim's wording.
//!
//! What each carrier asks for is two independent factors: which subsystem the anchor belongs to,
//! and which step of it the anchor is. One word may not answer both — a carrier whose subsystem
//! list and step list share a word has one factor, which is how a symbol named `renderChart`
//! proved a static site's renderer. The subsystem factor is read from the anchor's own *name*
//! wherever a name can carry it; a directory says where a symbol was filed, not what it does, and
//! a path-sourced subsystem re-opens the moment an off-subject symbol is filed beside the evidence
//! it is impersonating.
//!
//! One word may not answer both questions even when it appears twice. `Layout.render` in
//! `src/components/layout.tsx` reads as two factors — a subsystem word and a step word — until you
//! notice that the subsystem word and the folder are the same noun, and that the noun is one every
//! front end uses. So a subsystem factor has to be answered by a word that is *specific to that
//! subsystem*, and a compound noun whose head is the flow's subject (`FrameBuffer`, `sourceMap`,
//! `PaymentHandler`) has to say with its other word that it belongs here.
//!
//! Two generic words are not a substitute for one specific one, and the static-site carriers are
//! where that was tried. They accepted any two *different* web nouns from `page`, `layout`,
//! `template`, `document`, `collection`, `asset`, `theme`, `renderer` and `generator` — on the
//! theory that one such noun is a component framework's and two are a site generator's. A name
//! carries two as easily as one: `AssetCollection.process` and `PageTemplate.render`, filed under
//! `src/ui/`, closed that whole flow between them out of a repository with no static site in it.
//!
//! One surface is the exception, stated so the limit is visible rather than assumed. A stylesheet,
//! an HTML document and a schema file are proved *by the file*: what the indexer emits from them
//! are selectors, attributes and statements — `:root`, `required`, `CREATE TABLE` — with no
//! identifier to scope by, so there the path is the subsystem. That is the whole of the exception:
//! `is_form_constraint_markup` reads the path for a `.html`, `.htm` or `.xhtml` anchor and for
//! nothing else.
//!
//! A single-file component looks like that surface and is not it. The indexer blanks an SFC's
//! template before parsing it (`codestory_indexer::template_pipeline::prepare_template_source`), so
//! a `.vue` or `.svelte` citation names a `<script>` export — `greet`, `bump`, `clampMin` — an
//! ordinary identifier that can carry a subsystem word of its own. While those two extensions
//! counted as markup the path supplied the form factor for them, and `clampMin`, `submitJob` and
//! `validityWindow` in `src/forms/Widget.vue` closed all three steps of a form validation flow
//! between them without one of the three naming a form.
//!
//! The static-site carriers used to be a second exception, on the grounds that a build phase is
//! named for its phase and not for the site. They are not any more. What they carried instead of a
//! name-side subsystem was the generic-noun list above, so a subject word in a *directory* plus one
//! such noun plus a step verb closed the flow — `AssetPipeline.run` and `Layout.render` under
//! `lib/site/` proved a static-site build between them, and so did `Pipeline.run` and `Page.render`.
//! No static site is obliged to live in a directory called `site/`, and a site generator's lifecycle
//! and output methods hang off the site object itself, so the subject is read from the name where
//! every other carrier here reads it.

use crate::packet_scoring::{normalize_identifier, packet_display_path};
use crate::packet_terms::{
    packet_terms_indicate_server_request_dispatch_flow,
    packet_terms_indicate_server_route_dispatch_flow,
};
use codestory_contracts::api::{AgentCitationDto, NodeKind};

fn terminal(citation: &AgentCitationDto) -> String {
    normalize_identifier(&crate::text::terminal_symbol_segment(
        &citation.display_name,
    ))
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

fn owns_callable_behavior(citation: &AgentCitationDto) -> bool {
    matches!(
        citation.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
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

/// Match a callable whose terminal segment names an action and whose owner, or the remainder of
/// that same compound terminal, independently names the subsystem. An owner-qualified send
/// method and `sendRequest` have two factors; a bare `send` or `dispatch_hook` does not.
fn callable_owns_terminal_action_for_subject(
    citation: &AgentCitationDto,
    actions: &[&str],
    subjects: &[&str],
) -> bool {
    if !owns_callable_behavior(citation) {
        return false;
    }
    let terminal_tokens = identifier_tokens(terminal_segment_raw(&citation.display_name));
    if !has_token(&terminal_tokens, actions) {
        return false;
    }
    let tokens = name_tokens(citation);
    let owner_token_count = tokens.len().saturating_sub(terminal_tokens.len());
    has_token(&tokens[..owner_token_count], subjects)
        || terminal_tokens.iter().any(|token| {
            subjects.iter().any(|subject| token == subject)
                && !actions.iter().any(|action| token == action)
        })
}

fn callable_has_exact_owner_and_terminal_action(
    citation: &AgentCitationDto,
    owners: &[&str],
    actions: &[&str],
) -> bool {
    if !owns_callable_behavior(citation) {
        return false;
    }
    let mut segments = citation
        .display_name
        .rsplit(['.', ':', '#', '/', '\\'])
        .filter(|segment| !segment.is_empty());
    let Some(terminal) = segments.next() else {
        return false;
    };
    let Some(owner) = segments.next() else {
        return false;
    };
    owners
        .iter()
        .any(|expected| normalize_identifier(owner) == *expected)
        && has_token(&identifier_tokens(terminal), actions)
}

fn exact_terminal_identity(citation: &AgentCitationDto) -> String {
    normalize_identifier(terminal_segment_raw(&citation.display_name))
}

fn callable_has_exact_terminal(citation: &AgentCitationDto, terminals: &[&str]) -> bool {
    owns_callable_behavior(citation)
        && terminals
            .iter()
            .any(|expected| exact_terminal_identity(citation) == *expected)
}

/// Subjects that put HTTP on a method name without being an inbound server engine.
const HTTP_HANDLE_EXCLUDED_SUBJECTS: &[&str] = &[
    "telemetry",
    "metrics",
    "monitoring",
    "observability",
    "cache",
    "caches",
    "client",
    "clients",
    "session",
    "sessions",
];

/// A callable whose terminal names both the handle step and HTTP. Owner `app` and subject
/// `router` remain the primary dispatch carriers; this admits engines that put HTTP on the
/// method rather than the type. The owner segment is required so a bare handle-plus-HTTP
/// identifier cannot close the step without naming a receiver.
pub fn citation_owns_http_request_handle(citation: &AgentCitationDto) -> bool {
    if !owns_callable_behavior(citation) {
        return false;
    }
    let mut segments = citation
        .display_name
        .rsplit(['.', ':', '#', '/', '\\'])
        .filter(|segment| !segment.is_empty());
    let Some(terminal) = segments.next() else {
        return false;
    };
    if segments.next().is_none() {
        return false;
    }
    !has_token(&name_tokens(citation), HTTP_HANDLE_EXCLUDED_SUBJECTS)
        && terminal_names_http_request_handle(terminal)
}

fn terminal_names_http_request_handle(terminal: &str) -> bool {
    let terminal_tokens = identifier_tokens(terminal);
    has_token(&terminal_tokens, &["handle"]) && has_token(&terminal_tokens, &["http"])
}

/// A client type's own `send` method. Session send is the preceding dispatch stage, and a
/// cache `send` is not a transport.
fn citation_owns_named_client_send(citation: &AgentCitationDto) -> bool {
    if !callable_has_exact_terminal(citation, &["send"]) {
        return false;
    }
    let tokens = name_tokens(citation);
    has_token(&tokens, &["client", "clients"])
        && !has_token(
            &tokens,
            &[
                "session",
                "sessions",
                "cache",
                "caches",
                "database",
                "db",
                "hook",
                "hooks",
                "metrics",
                "telemetry",
                "monitoring",
                "observability",
            ],
        )
}

fn callable_owns_terminal_action_for_two_subjects(
    citation: &AgentCitationDto,
    actions: &[&str],
    primary_subjects: &[&str],
    boundary_subjects: &[&str],
) -> bool {
    if !owns_callable_behavior(citation)
        || !has_token(
            &identifier_tokens(terminal_segment_raw(&citation.display_name)),
            actions,
        )
    {
        return false;
    }
    let tokens = name_tokens(citation);
    has_token(&tokens, primary_subjects) && has_token(&tokens, boundary_subjects)
}

// ---------------------------------------------------------------------------
// HTTP client lifecycle
// ---------------------------------------------------------------------------

/// The convenience request method a caller reaches first: a verb-named method on a client type.
/// Distinct from the factory that builds the client and from the adapter that finally sends.
///
/// The verb list is the whole HTTP method set, so the terminal-segment test alone accepts every
/// `.get`, `.post`, `.delete` and `.options` in a repository — `Store.get`, `Queue.head`,
/// `FeatureFlags.options`. The receiver has to be a client before its verb means anything.
///
/// "request" is the one verb in the method set that is also a word in the client list, so a symbol
/// named `X.request` satisfies both factors with one word. That is not closable here: a real
/// client's own request method is routinely spelled exactly that way, with the receiver naming the
/// library rather than the word "client", and nothing in a name separates it from a
/// `FrameKind.request` somewhere else in the repository. It is recorded as a family in
/// `COMPOUND_EVIDENCE_SURFACE` rather than left for the next reviewer to find again.
pub fn citation_owns_client_request_method(citation: &AgentCitationDto) -> bool {
    matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        && belongs_to_http_client(citation)
        && matches!(
            terminal(citation).as_str(),
            "request" | "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
        )
}

/// The package-level HTTP helper that opens the outbound-request lifecycle. A bare `request`
/// citation is lawful here only on the public API module; the exact outgoing CALL to the following
/// client/session request stage remains mandatory and is checked by the flow requirement.
pub fn citation_owns_client_public_facade_helper(citation: &AgentCitationDto) -> bool {
    if citation.kind != NodeKind::FUNCTION
        || !matches!(
            terminal(citation).as_str(),
            "request" | "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
        )
    {
        return false;
    }
    let path_tokens = path_tokens(citation);
    let public_api_surface = has_token(&path_tokens, &["api"])
        || path(citation)
            .rsplit_once("/lib/")
            .is_some_and(|(_, relative)| {
                !relative.contains('/')
                    && has_token(
                        &path_tokens,
                        &["http", "https", "request", "requests", "client"],
                    )
            });
    public_api_surface
        && !names_or_path_token(
            citation,
            &[
                "hook",
                "hooks",
                "cache",
                "caches",
                "database",
                "db",
                "telemetry",
                "monitoring",
                "observability",
                "metrics",
            ],
        )
}

/// A public outbound-request entrypoint. Unlike the broader request-method ranking predicate, a
/// bare `request` is insufficient here: the symbol must independently name a client/session
/// subject so `FrameKind.request` cannot become a material packet carrier.
pub fn citation_owns_client_request_entrypoint(citation: &AgentCitationDto) -> bool {
    (callable_has_exact_owner_and_terminal_action(citation, &["session"], &["request"])
        && has_token(
            &path_tokens(citation),
            &["http", "https", "request", "requests", "client"],
        ))
        || callable_owns_terminal_action_for_two_subjects(
            citation,
            &[
                "request", "get", "post", "put", "patch", "delete", "head", "options",
            ],
            &["client", "session"],
            &["http", "https", "requests", "fetch"],
        )
}

/// The callable that hands an assembled outbound request to the selected transport. Adapters and
/// transports are deliberately excluded: they are the terminal boundary, while this carrier is
/// the session/client dispatch immediately before it.
pub fn citation_owns_client_request_dispatch(citation: &AgentCitationDto) -> bool {
    ((callable_has_exact_owner_and_terminal_action(citation, &["session"], &["send"])
        && has_token(
            &path_tokens(citation),
            &["http", "https", "request", "requests", "client"],
        ))
        || callable_owns_terminal_action_for_two_subjects(
            citation,
            &["send"],
            &["client", "session"],
            &["http", "https", "requests", "fetch"],
        ))
        && !names_token(
            citation,
            &["adapter", "adapters", "transport", "transports"],
        )
}

/// The session/client step that chooses the transport interface used for an outbound request.
/// Both the client/session owner and the adapter/transport subject must be present in the symbol;
/// a generic hook or a directory name cannot satisfy this boundary.
pub fn citation_owns_client_adapter_selection(citation: &AgentCitationDto) -> bool {
    callable_owns_terminal_action_for_two_subjects(
        citation,
        &["get", "select", "choose", "resolve"],
        &["client", "session"],
        &["adapter", "adapters", "transport", "transports"],
    )
}

/// The declared adapter/transport send boundary. This excludes the session's own `send` method,
/// which is the preceding dispatch stage. A client type whose terminal is exactly `send` is the
/// same boundary when the library names the client instead of an adapter.
pub fn citation_owns_client_transport_send(citation: &AgentCitationDto) -> bool {
    callable_owns_terminal_action_for_subject(
        citation,
        &["send"],
        &["adapter", "adapters", "transport", "transports"],
    ) || citation_owns_named_client_send(citation)
}

/// Registration of an inbound server request surface.
pub fn citation_owns_server_request_entrypoint(citation: &AgentCitationDto) -> bool {
    callable_has_exact_owner_and_terminal_action(citation, &["app"], &["route", "use", "listen"])
        || callable_owns_terminal_action_for_subject(
            citation,
            &["route", "use", "listen"],
            &["router", "middleware", "wsgi", "asgi", "servlet"],
        )
}

/// A callable that registers a route or method on an application/router surface. The route
/// subject must be present in the callable's own name; a generic handler or group in a routing
/// directory is not registration evidence.
pub fn citation_owns_server_route_registration(citation: &AgentCitationDto) -> bool {
    callable_has_exact_owner_and_terminal_action(
        citation,
        &["app"],
        &[
            "route", "use", "handle", "register", "get", "post", "put", "patch", "delete", "head",
            "options",
        ],
    ) || callable_owns_terminal_action_for_subject(
        citation,
        &[
            "route", "use", "handle", "register", "get", "post", "put", "patch", "delete", "head",
            "options",
        ],
        &["route", "router"],
    )
}

/// The callable that selects or invokes an inbound request handler.
pub fn citation_owns_server_request_dispatch(citation: &AgentCitationDto) -> bool {
    callable_has_exact_owner_and_terminal_action(citation, &["app"], &["handle"])
        || callable_owns_terminal_action_for_subject(
            citation,
            &["dispatch", "handle", "invoke"],
            &[
                "router",
                "controller",
                "middleware",
                "wsgi",
                "asgi",
                "servlet",
            ],
        )
        || citation_owns_http_request_handle(citation)
}

/// The concrete request dispatcher from which route lookup and handler-chain calls originate.
/// Route-group `handle` methods are registration helpers and intentionally stay outside this
/// narrower carrier.
pub fn citation_owns_server_route_match_dispatch(citation: &AgentCitationDto) -> bool {
    citation_owns_http_request_handle(citation)
        || callable_owns_terminal_action_for_subject(
            citation,
            &["dispatch", "handle", "process"],
            &["http", "request"],
        )
        || callable_has_exact_owner_and_terminal_action(
            citation,
            &["app", "server"],
            &["dispatch", "handle"],
        )
        || callable_has_exact_owner_and_terminal_action(citation, &["router"], &["dispatch"])
}

/// The server/application callable immediately before inbound handler dispatch.
pub fn citation_owns_server_request_handler_entrypoint(citation: &AgentCitationDto) -> bool {
    owns_callable_behavior(citation) && server_request_handler_call_source(&citation.display_name)
}

/// Prefer a cited inbound dispatch callable when the question is about routing
/// a request through handlers. Route-group helpers often outscore that
/// callable on raw lexical overlap.
pub fn packet_server_dispatch_callable_rank_bonus(
    citation: &AgentCitationDto,
    terms: &[String],
) -> f32 {
    if !(packet_terms_indicate_server_route_dispatch_flow(terms)
        || packet_terms_indicate_server_request_dispatch_flow(terms))
    {
        return 0.0;
    }
    if citation_owns_server_request_dispatch(citation) {
        6.0
    } else {
        0.0
    }
}

/// The response-side callable that leaves a server handler for its writer or transport.
pub fn citation_owns_server_response_terminal(citation: &AgentCitationDto) -> bool {
    callable_has_exact_owner_and_terminal_action(
        citation,
        &["res", "response", "reply"],
        &["send", "end", "write", "flush", "finish"],
    ) || callable_owns_terminal_action_for_two_subjects(
        citation,
        &["send", "end", "write", "flush", "finish"],
        &["response", "responses", "reply"],
        &[
            "writer",
            "sender",
            "transport",
            "socket",
            "stream",
            "output",
        ],
    )
}

/// Typed CALL targets that advance an outbound public request into preparation or dispatch.
pub fn client_request_entrypoint_call_target(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    if has_token(
        &tokens,
        &[
            "cache",
            "database",
            "db",
            "hook",
            "metrics",
            "telemetry",
            "monitoring",
            "observability",
        ],
    ) {
        return false;
    }
    let terminal = identifier_tokens(terminal_segment_raw(display_name));
    let owns_client_lifecycle = has_token(&tokens, &["client", "session"]);
    let owns_prepared_artifact =
        has_token(&tokens, &["prepared"]) && has_token(&tokens, &["request", "requests"]);
    let prepares_request =
        has_token(&terminal, &["prepare"]) && has_token(&terminal, &["request", "requests"]);
    let dispatches_request = has_token(&terminal, &["request", "send"]);
    let builds_prepared_artifact = has_token(&terminal, &["build"]);
    (owns_client_lifecycle && (prepares_request || dispatches_request))
        || (owns_prepared_artifact && builds_prepared_artifact)
}

/// Typed CALL targets that advance a top-level package helper into the session/client request
/// entrypoint. The final adjacent owner/action pair keeps another package helper, a request-shaped
/// metrics symbol, or a similarly named hook from satisfying the boundary.
pub fn client_public_facade_successor_call_target(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    !has_token(
        &tokens,
        &[
            "cache",
            "database",
            "db",
            "hook",
            "metrics",
            "telemetry",
            "monitoring",
            "observability",
        ],
    ) && (tokens.windows(2).last().is_some_and(|tail| {
        tail[1] == "request" && matches!(tail[0].as_str(), "client" | "session")
    }) || tokens
        .windows(2)
        .last()
        .is_some_and(|tail| tail == ["with", "client"]))
}

/// Typed CALL sources that precede the client/session dispatch stage.
pub fn client_request_dispatch_predecessor_call_source(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    tokens.windows(2).last().is_some_and(|tail| {
        tail[1] == "request" && matches!(tail[0].as_str(), "client" | "session" | "requests")
    })
}

/// Typed CALL targets that advance client/session dispatch into adapter selection.
pub fn client_request_dispatch_successor_call_target(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    tokens.windows(2).last().is_some_and(|tail| {
        matches!(tail[0].as_str(), "get" | "select" | "choose" | "resolve")
            && matches!(
                tail[1].as_str(),
                "adapter" | "adapters" | "transport" | "transports"
            )
    })
}

/// Typed CALL targets that implement route or middleware registration.
pub fn server_request_entrypoint_call_target(display_name: &str) -> bool {
    display_terminal_owner_action_matches(
        display_name,
        &["router"],
        &["route", "routes", "use", "listen"],
    ) || display_terminal_owner_action_matches(display_name, &["server"], &["listen"])
}

/// The stage that inserts a route into a router-owned or route-named store/tree.
pub fn server_route_insertion_call_target(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    let terminal = identifier_tokens(terminal_segment_raw(display_name));
    ((has_token(&terminal, &["add", "insert", "register", "store"])
        && has_token(&tokens, &["route", "routes", "router", "routing"]))
        || display_terminal_owner_action_matches(display_name, &["router"], &["route"]))
        && !has_token(
            &tokens,
            &["metrics", "telemetry", "monitoring", "observability"],
        )
}

/// The inbound server callable that owns the edge into request dispatch.
pub fn server_request_handler_call_source(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    let terminal = identifier_tokens(terminal_segment_raw(display_name));
    let excluded = has_token(
        &tokens,
        &[
            "cache",
            "client",
            "metrics",
            "monitoring",
            "observability",
            "search",
            "session",
            "telemetry",
        ],
    );
    let server_owner = has_token(
        &tokens[..tokens.len().saturating_sub(terminal.len())],
        &["app", "asgi", "server", "servlet", "wsgi"],
    );
    !excluded
        && ((has_token(&terminal, &["serve"]) && has_token(&terminal, &["http"]))
            || (server_owner && has_token(&terminal, &["dispatch", "handle", "invoke", "process"])))
}

/// The exact route-lookup target reached from an inbound request dispatcher.
pub fn server_route_lookup_call_target(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    let terminal = identifier_tokens(terminal_segment_raw(display_name));
    let get_value = terminal.as_slice() == ["get", "value"];
    let route_lookup = has_token(&terminal, &["find", "lookup", "match", "resolve", "select"])
        && has_token(&tokens, &["route", "router", "tree"]);
    (get_value && has_token(&tokens, &["node", "route", "router", "tree"])) || route_lookup
}

/// The context/handler-chain continuation reached after a request route matches.
pub fn server_handler_chain_call_target(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    let terminal = identifier_tokens(terminal_segment_raw(display_name));
    !has_token(&tokens, &["render", "template", "ui", "view", "widget"])
        && ((has_token(&tokens, &["context"]) && has_token(&terminal, &["next", "proceed", "run"]))
            || (has_token(&tokens, &["handler", "handlers", "middleware"])
                && has_token(&terminal, &["dispatch", "execute", "invoke", "next", "run"])))
}

/// Typed CALL targets that select or invoke the inbound handler.
pub fn server_request_dispatch_call_target(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    tokens.as_slice() == ["finalhandler"]
        || display_terminal_owner_action_matches(
            display_name,
            &[
                "app",
                "router",
                "controller",
                "middleware",
                "wsgi",
                "asgi",
                "servlet",
            ],
            &["dispatch", "handle", "invoke"],
        )
        || (terminal_names_http_request_handle(terminal_segment_raw(display_name))
            && !has_token(&tokens, HTTP_HANDLE_EXCLUDED_SUBJECTS))
}

/// Typed CALL targets that perform the response write or flush.
pub fn server_response_terminal_call_target(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    display_terminal_owner_action_matches(
        display_name,
        &[
            "res",
            "response",
            "reply",
            "writer",
            "transport",
            "socket",
            "stream",
        ],
        &["end", "write", "flush", "finish"],
    ) || tokens.windows(3).last().is_some_and(|tail| {
        matches!(tail[0].as_str(), "response" | "reply")
            && tail[1] == "sender"
            && matches!(tail[2].as_str(), "end" | "write" | "flush" | "finish")
    }) || (tokens.len() == 2
        && tokens[0] == "output"
        && matches!(tokens[1].as_str(), "end" | "write" | "flush" | "finish"))
}

fn display_terminal_owner_action_matches(
    display_name: &str,
    owners: &[&str],
    actions: &[&str],
) -> bool {
    let tokens = identifier_tokens(display_name);
    let Some(tail) = tokens.windows(2).last() else {
        return false;
    };
    owners.contains(&tail[0].as_str())
        && actions.contains(&tail[1].as_str())
        && !has_token(
            &tokens[..tokens.len().saturating_sub(2)],
            &["metrics", "telemetry", "monitoring", "observability"],
        )
}

/// Anchors that belong to an HTTP client at all. `Uri.prepare` is a URL utility that happens to be
/// named "prepare"; without this scoping it closed the finalization step of any client flow.
///
/// Read from the symbol's own name and not its path. A directory named `client/` or `http/` holds
/// plenty of symbols that are not the client — moving `Store.get` into a file named for the client
/// must not turn it into the client's request method, and a path-sourced subsystem is exactly what
/// would.
const HTTP_CLIENT_WORDS: &[&str] = &[
    "request",
    "requests",
    "http",
    "https",
    "client",
    "clients",
    "adapter",
    "adapters",
    "transport",
    "transports",
    "send",
    "sends",
    "fetch",
];

fn belongs_to_http_client(citation: &AgentCitationDto) -> bool {
    names_token(citation, HTTP_CLIENT_WORDS)
}

/// The step that turns a configured request into a transport-ready one.
pub fn citation_owns_client_request_finalization(citation: &AgentCitationDto) -> bool {
    if !owns_behavior(citation) || !belongs_to_http_client(citation) {
        return false;
    }
    names_token_prefix(citation, &["finaliz", "finalis", "prepar"])
        || (names_token(citation, &["request", "requests"])
            && names_token(citation, &["to", "build", "body"]))
}

/// The boundary where a transport response becomes a value the caller can read.
pub fn citation_owns_client_response_materialization(citation: &AgentCitationDto) -> bool {
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

/// A surface whose citations are script identifiers. `Cache.write` in `lib/cache.rb` is a
/// server-side cache and must not stand in for the hook's cache helper.
///
/// `.vue` and `.svelte` are here because that is what the indexer produces from them: their
/// templates are blanked and only the `<script>` block is parsed, so a citation from a single-file
/// component is a function or method with a name to read, not a markup attribute.
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
///
/// Only a capital counts. Admitting `use_` and `use-` as well handed the convention to every
/// snake-cased and kebab-cased `use_temp_dir`, `use_default_locale` and `use-legacy-mode` in any
/// repository, none of which is a data-fetching hook — the convention this recognises exists in
/// `camelCase` front-end code and nowhere else, which is also why the export has to sit on a
/// script surface.
fn names_a_hook(citation: &AgentCitationDto) -> bool {
    let segment = terminal_segment_raw(&citation.display_name);
    let Some(rest) = segment.strip_prefix("use") else {
        return false;
    };
    rest.chars()
        .next()
        .is_some_and(|next| next.is_ascii_uppercase())
}

pub fn citation_owns_hook_public_export(citation: &AgentCitationDto) -> bool {
    matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        && is_script_surface(citation)
        && names_a_hook(citation)
        && !names_token(citation, &["cache", "caches"])
}

/// Serializing *the cache key*. Being a `serialize*` function on a script surface is the shape of
/// every `serializeSettings`, `serializeForm` and `serializeQueryString` ever written; the
/// requirement is about the key, so the key has to be named.
pub fn citation_owns_hook_key_serialization(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && is_script_surface(citation)
        && names_token(citation, &["key", "keys"])
        && (names_token_prefix(citation, &["serializ", "serialis"])
            || names_token(citation, &["hash", "stable", "stringify"]))
}

/// The helper that holds cache state, not any method that touches a cache. `Cache.put` and
/// `Cache.get` are the cache's own API on every cache in every repository; this requirement is the
/// hook library's helper *around* one, so the anchor has to name the helper.
pub fn citation_owns_hook_cache_helper(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && is_script_surface(citation)
        && names_token(citation, &["cache", "caches"])
        && (names_token(
            citation,
            &["helper", "helpers", "provider", "context", "state", "store"],
        ) || names_token_prefix(citation, &["make", "creat", "init"]))
}

pub fn citation_owns_hook_mutation_flow(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && is_script_surface(citation)
        && names_token_prefix(citation, &["mutat"])
}

// ---------------------------------------------------------------------------
// HTML / CSS structure
// ---------------------------------------------------------------------------

/// A document whose indexed anchors are *not* identifiers: an HTML file yields ids, classes and
/// attributes — a mount-point id, `required`, `pattern` — and there is no name in one of those to
/// read a
/// subsystem out of. This is the module header's declared exception, and the only place a path is
/// allowed to answer "which subsystem is this".
///
/// `.vue` and `.svelte` used to be here and are gone. They are single-file components, and the
/// indexer blanks their templates before parsing, so a citation from one names a `<script>` export
/// like any other script symbol. While they counted as markup the exception applied to them, and
/// every symbol filed in a component library's `forms/` directory inherited the form factor from
/// the folder.
fn is_markup_document(citation: &AgentCitationDto) -> bool {
    path_has_any_extension(citation, &[".html", ".htm", ".xhtml"])
}

fn is_stylesheet(citation: &AgentCitationDto) -> bool {
    path_has_any_extension(citation, &[".css", ".scss", ".sass", ".less"])
}

pub fn citation_owns_html_app_shell(citation: &AgentCitationDto) -> bool {
    is_markup_document(citation)
        && names_token(
            citation,
            &[
                "app", "root", "main", "body", "shell", "module", "script", "mount",
            ],
        )
}

pub fn citation_owns_css_structure(citation: &AgentCitationDto) -> bool {
    is_stylesheet(citation)
}

pub fn citation_owns_css_animation_entrypoint(citation: &AgentCitationDto) -> bool {
    is_stylesheet(citation) && names_token(citation, &["import", "use", "forward"])
}

pub fn citation_owns_css_animation_structure(citation: &AgentCitationDto) -> bool {
    is_stylesheet(citation)
        // Sibling of `css_animation_entrypoint`. `@import "animations/base"` names the animation
        // directory, so without this an import closed the structure requirement too.
        && !citation_owns_css_animation_entrypoint(citation)
        && names_token(
            citation,
            &[
                "keyframes",
                "animation",
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
///
/// On a **script** surface the form factor is read from the anchor's own name. Reading it from the
/// path too meant a directory supplied the subsystem, and any off-subject symbol filed beside the
/// real evidence inherited it: `clampMin` in `src/forms/layout.ts` closed the native-constraint
/// requirement while the identical `clampMin` in `src/render/layout.ts` closed nothing, so the
/// folder — not the symbol — decided whether the packet was sufficient.
///
/// A **markup document** is the exception the module header states, and it is narrower than the
/// surface: it belongs to `citation_owns_form_native_constraint` alone. Those anchors really are
/// markup — `required`, `pattern`, `minlength` are attributes written in the document, with no
/// identifier to scope by — so there the file is the subsystem and the path may carry the form
/// factor. The other two steps are script behaviour, and their own witnesses are `.js` files; while
/// they took the exception too, one HTML document under a `forms/` path closed all three steps of
/// the flow out of any three lexical hits in it, which is a sufficient verdict over a packet that
/// proved one step at most.
///
/// The exception is `.html`, `.htm` and `.xhtml` and nothing else. A `.vue` or `.svelte` citation is
/// a `<script>` export with a name of its own, so it reads its form factor from the name like any
/// other script.
/// Words that say the anchor is about a *form*.
///
/// "validate", "validates", "validation", "validations", "invalid" and "preventdefault" used to be
/// here and are gone. Each is a word the carriers below use as their *step*, and a subsystem list
/// and a step list that share a word give the carrier one factor rather than two. Validation is
/// also universal — schema validation, licence validation, password-strength validation — so
/// `validationMin`, `validationCheck` and `validationSubmit` closed all three requirements of this
/// flow between them, out of a repository with no form in it.
///
/// "validity" stays, because it is the one of them that is a *form control's* own noun rather than
/// a generic activity: `ValidityState` and `element.validity` are the constraint-validation API,
/// which is where the real anchors `setCustomValidity` and `renderValidityMessage` get it from.
/// That it is also `form_custom_validation`'s step word is recorded in the evidence surfaces.
const FORM_SUBSYSTEM_WORDS: &[&str] = &[
    "form",
    "forms",
    "fieldset",
    "validity",
    "constraint",
    "constraints",
    "guard",
    "guards",
];

/// The anchor names a form, on a surface a form is written on. A browser document counts as such a
/// surface because a `<script>` block inside one is indexed as script, so `setCustomValidity` in an
/// HTML document still reads as what it is — but the form factor comes from the name there, as it
/// does everywhere else.
fn is_form_validation_surface(citation: &AgentCitationDto) -> bool {
    (is_script_surface(citation) || is_markup_document(citation))
        && names_token(citation, FORM_SUBSYSTEM_WORDS)
}

/// The one place the path may answer the form question: a constraint attribute written into a
/// markup document. `required` and `pattern` are the whole anchor, and neither can say which
/// document it was written in.
fn is_form_constraint_markup(citation: &AgentCitationDto) -> bool {
    is_markup_document(citation) && names_or_path_token(citation, FORM_SUBSYSTEM_WORDS)
}

pub fn citation_owns_form_native_constraint(citation: &AgentCitationDto) -> bool {
    (is_form_validation_surface(citation) || is_form_constraint_markup(citation))
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

pub fn citation_owns_form_custom_validation(citation: &AgentCitationDto) -> bool {
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

pub fn citation_owns_form_submit_guard(citation: &AgentCitationDto) -> bool {
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

pub fn citation_owns_shell_installer_bootstrap(citation: &AgentCitationDto) -> bool {
    is_shell_script(citation)
        && names_token_prefix(
            citation,
            &["install", "bootstrap", "download", "setup", "source"],
        )
}

const SHELL_FUNCTION_SUBJECT_TOKENS: &[&str] = &["shell", "function", "command"];
const SHELL_FUNCTION_ACTION_TOKENS: &[&str] =
    &["use", "run", "exec", "execute", "case", "dispatch"];

pub fn citation_owns_shell_function_dispatch(citation: &AgentCitationDto) -> bool {
    let tokens = name_tokens(citation);
    matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        && is_shell_script(citation)
        && taxonomy_has_token(&tokens, SHELL_FUNCTION_SUBJECT_TOKENS)
        && taxonomy_has_token(&tokens, SHELL_FUNCTION_ACTION_TOKENS)
}

pub fn citation_owns_shell_completion(citation: &AgentCitationDto) -> bool {
    is_shell_script(citation)
        && names_token_prefix(citation, &["completion", "compgen", "complete", "alias"])
}

// ---------------------------------------------------------------------------
// Buffered IO
// ---------------------------------------------------------------------------

/// "segment" is gone. It is the head of `SegmentTree`, `SegmentDescriptor` and every other
/// segmented structure in software, and it was the whole of this factor: `SegmentTree.read` closed
/// the read/write step of a byte-buffer flow it has nothing to do with. No expected symbol of the
/// buffered-IO corpus is named for a segment.
fn names_buffer(citation: &AgentCitationDto) -> bool {
    names_token_prefix(citation, &["buffer"])
}

/// The peers a byte buffer sits between. This is the second factor: it says the anchor is in an IO
/// pipeline and not merely that some word in it ends in "buffer".
fn names_io_peer(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "source", "sources", "sink", "sinks", "stream", "streams", "byte", "bytes", "io",
            "reader", "writer", "input", "output", "socket", "pipe", "channel",
        ],
    )
}

/// Whether some segment of the symbol's name is *nothing but* the buffer word — the type called
/// `Buffer`, the function called `buffer`, or a method hanging off either. Such a name is the
/// buffer, which is the reading `ONE_WORD_EVIDENCE_SURFACE` records and intends.
///
/// `FrameBuffer`, `ZBuffer` and `RingBufferStats` are not: the word beside the head noun says which
/// kind of buffer, and a pixel buffer is not the byte buffer this flow is about. Accepting them was
/// the same collapse as everywhere else in this module — one word answering both "is this the
/// buffered-IO subsystem" and "which step of it is this".
fn names_the_buffer_itself(citation: &AgentCitationDto) -> bool {
    citation
        .display_name
        .split(['.', ':', '/', '\\'])
        .filter(|segment| !segment.is_empty())
        .any(|segment| {
            let tokens = identifier_tokens(segment);
            tokens.len() == 1 && tokens[0].starts_with("buffer")
        })
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
pub fn citation_owns_buffer_storage(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && names_buffer(citation)
        && !names_io_operation(citation)
        && (names_the_buffer_itself(citation) || names_io_peer(citation))
}

/// The operations that move bytes across that buffer. Sibling of `buffer_storage`, so a citation
/// that only names the container must not close it.
///
/// The anchor must name the buffer itself as well as the operation. A generic source or sink
/// operation names IO, but not *this* buffer flow; accepting those peers let a real storage
/// citation combine with unrelated database and telemetry operations to close the whole flow.
/// The positive surface remains operations named on the storage object itself, which carry both
/// factors in one citation.
pub fn citation_owns_buffer_read_write(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && names_io_operation(citation) && names_the_buffer_itself(citation)
}

// ---------------------------------------------------------------------------
// Logger record + handler
// ---------------------------------------------------------------------------

/// Anchors that belong to a logging subsystem. "record" and "handler" are two of the most reused
/// words in any codebase — `createUserRecord` is a database row and `handleClick` is a UI callback —
/// so a carrier that reads only those words speaks for every subsystem at once.
///
/// Read from the *name*, the same way its sibling `citation_owns_log_record_creation` already reads
/// it. While this asked the path as well the two carriers disagreed about what a subsystem is: a
/// `createUserRecord` in `src/logging/` was correctly rejected as a database row, but a
/// `PaymentHandler.process` in the very same directory was accepted as the logger's handler step.
/// Closing the verb `handle*` left the noun-in-the-directory open, and any `*Handler.process` filed
/// beside a logger closed its dispatch step.
fn belongs_to_logging(citation: &AgentCitationDto) -> bool {
    names_token(citation, &["log", "logs", "logger", "loggers", "logging"])
}

/// Creating the record a logger emits. The logging factor is read from the *name*: a
/// `createUserRecord` that happens to sit in `src/logging/` is still a database row, and letting
/// the directory supply the subsystem is how an off-subject symbol inside a flow's own folder
/// closes that flow's requirement.
pub fn citation_owns_log_record_creation(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let tokens = name_tokens(citation);
        has_token(&tokens, &["log", "logs", "logger", "loggers", "logging"])
            && has_token(&tokens, &["record", "records"])
            && !any_token_starts_with(&tokens, &["handler", "handle"])
            && has_token(&tokens, &["add", "create", "make", "build", "log"])
    }
}

/// Processing a record, not registering something that might: a symbol that pushes a handler onto
/// a stack names a handler but does nothing with a record, so it must not close this requirement.
///
/// The anchor has to name the *handler* — the noun — and not merely start with the verb "handle".
/// `handleClick`, `handleScroll` and `handleResize` are callbacks in every front end ever written
/// and were each accepted here, because `handle` is a prefix of `handler` and the second factor
/// accepted the same prefix again.
///
/// It must also name the logging subsystem itself. Structural adjectives such as `Abstract`,
/// `Default`, `Processing`, `Interface`, and `Fallback` occur on handlers in every domain. Treating
/// one as a record-pipeline subject let an HTTP handler combine with real record-creation evidence
/// and close the whole logging flow. A proof-bearing handler anchor therefore says both whose
/// handler it is and which processing step it owns.
pub fn citation_owns_log_handler_processing(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && belongs_to_logging(citation) && {
        let tokens = name_tokens(citation);
        let names_a_handler = has_token(&tokens, &["handler", "handlers"]);
        let only_registers = has_token(
            &tokens,
            &["push", "pop", "add", "remove", "set", "register"],
        );
        names_a_handler
            && !only_registers
            && (has_token(&tokens, &["write", "emit", "flush", "batch", "interface"])
                || any_token_starts_with(&tokens, &["process"]))
    }
}

// ---------------------------------------------------------------------------
// Static-site build
// ---------------------------------------------------------------------------

/// The words that name a *static site*.
///
/// "view"/"views" is deliberately absent: it is the MVC directory every server framework ships, and
/// `app/views/` in a Rails app is not a static site. "render" is absent because it is this flow's
/// *step*, not its subject — a carrier whose subsystem factor and step factor can both be satisfied
/// by one word has one factor, which is how `renderChart` proved a site renderer.
///
/// "static" is gone with the directory that justified it. It earned its place as the
/// `public/static/` spelling of a site root, and a folder is no longer read here; as a word in a
/// *name* it is the storage-class keyword every C-family language ships, so `StaticInitializer.run`
/// and `StaticAnalyzer.process` would have inherited a static-site build from it.
const SITE_BUILD_SUBJECT_WORDS: &[&str] = &["site", "sites"];

/// A `site` beside a `map` is a *sitemap* — the navigation artifact every server-rendered
/// application emits — and not the static site this flow is about. This is the same reading
/// `belongs_to_object_mapper` gives the word from the other side: the compound decides which of the
/// two nouns is the head, so `SiteMapGenerator.process` and `SiteMap.write` are neither a build
/// lifecycle nor a build's output boundary.
fn names_a_sitemap(citation: &AgentCitationDto) -> bool {
    names_token(citation, &["map", "maps", "sitemap", "sitemaps"])
}

/// Anchors that belong to a static-site build. Without this, `Cache.write` in `lib/cache.rb` closed
/// the site's terminal boundary purely because its name contains "write".
///
/// Read from the **name**, like every other subsystem factor in this module, and it has to be the
/// site itself. Two things were wrong with the version this replaces, and they compounded.
///
/// The first was the directory. This flow used to be the module header's second path exception, so
/// `site` in a *folder* answered the subsystem question outright and the name was left carrying one
/// generic web noun and one step verb — the one-word-plus-a-folder shape the rest of the module
/// exists to reject. `AssetPipeline.run` and `Layout.render` under `lib/site/` closed the entire
/// flow between them, as did `Pipeline.run` and `Page.render`, and the identical symbols one
/// directory over closed nothing.
///
/// The second was the fallback that survived taking the directory away: two *different* generic web
/// nouns. `page`, `layout`, `template`, `document`, `collection`, `asset`, `theme`, `renderer` and
/// `generator` are as much a component framework's vocabulary as a site generator's, and a name
/// carries two of them as easily as one — `AssetCollection.process` and `PageTemplate.render` in
/// `src/ui/` closed this flow with no site anywhere in the packet. Two generic words are two
/// signals of the same generic thing, not one specific one.
///
/// What is left is narrower than the corpus's full anchor set, and deliberately: a site generator's
/// helper classes are named `Renderer` and `Reader`, and those no longer close a step on their own.
/// The build's phases hang off the site object, which does say it. Reader evidence has its own
/// source-backed carrier below, so the lifecycle can stay narrow without losing the content-loading
/// phase.
fn belongs_to_site_build(citation: &AgentCitationDto) -> bool {
    names_token(citation, SITE_BUILD_SUBJECT_WORDS) && !names_a_sitemap(citation)
}

/// The two carriers below used to carry a third condition, `names_site_build_object` — a list of
/// nouns a site build produces (`page`, `layout`, `asset`, `renderer`, `pipeline`, `html`, …). It
/// was there to stand in for a name-side subsystem while the subsystem came from the directory, and
/// it is gone with the directory. It cannot narrow anything now: "site" was itself the first entry
/// in that list, so every name `belongs_to_site_build` accepts satisfies it. Leaving it in would
/// read as a second factor and be none.
pub fn citation_owns_site_lifecycle(citation: &AgentCitationDto) -> bool {
    callable_has_exact_owner_and_terminal_action(citation, &["site"], &["process"])
}

/// The source that turns project files into static-site content.
///
/// `Reader.read` alone is deliberately insufficient: that spelling appears in IO, HTTP, database,
/// and document libraries. A bare helper becomes a site-build carrier only when its pinned source
/// names the site it populates and at least two independent static-site content families. The
/// source condition keeps `HttpReader.read`, `PageReader.read`, and an opaque generic reader from
/// becoming requirement-bearing merely because a site-build question asked for reading.
pub fn citation_owns_site_reader(citation: &AgentCitationDto) -> bool {
    if !callable_has_exact_owner_and_terminal_action(citation, &["reader"], &["read"]) {
        return false;
    }
    let Some(source_excerpt) = citation.source_excerpt.as_deref() else {
        return false;
    };
    let tokens = identifier_tokens(source_excerpt);
    if !has_token(&tokens, &["site", "sites"]) {
        return false;
    }
    [
        &["layout", "layouts", "template", "templates"][..],
        &[
            "collection",
            "collections",
            "document",
            "documents",
            "post",
            "posts",
        ][..],
        &["page", "pages"][..],
        &["asset", "assets", "static"][..],
        &["data"][..],
    ]
    .iter()
    .filter(|family| has_token(&tokens, family))
    .take(2)
    .count()
        == 2
}

pub fn citation_owns_site_terminal(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && belongs_to_site_build(citation)
        // Sibling of `site_lifecycle`, which excludes these same three stems, so one anchor cannot
        // close both steps.
        && (names_token(citation, &["output", "outputs", "emit", "emits", "render", "renders"])
            || names_token_prefix(citation, &["writ", "read"]))
}

// ---------------------------------------------------------------------------
// Object mapper
// ---------------------------------------------------------------------------

/// The things an object mapper maps. A bare `map` is the most overloaded noun in the language, and
/// the compound it heads is what says which kind: `sourceMap` is a build artifact, `roadMap` and
/// `siteMap` are navigation, `heatMap` and `tileMap` are graphics. A map of types is an object
/// mapper's
/// own noun, and so are the model words beside it.
const OBJECT_MAPPER_SUBJECT_WORDS: &[&str] = &[
    "type",
    "types",
    "object",
    "objects",
    "model",
    "models",
    "entity",
    "entities",
    "dto",
    "dtos",
    "member",
    "members",
    "property",
    "properties",
    "destination",
    "class",
    "classes",
];

/// Anchors that belong to an object mapper. "profile" and "plan" are ordinary words — `userProfile`
/// closed the mapper's configuration requirement until the carrier asked which subsystem it is in.
///
/// Read from the name: any symbol dropped into a `mapping/` directory would otherwise inherit the
/// subsystem it happens to be filed under.
///
/// The noun forms — `mapper`, `mapping` — name the subsystem on their own. A bare `map` does not:
/// it is the head of every `sourceMap`, `roadMap`, `siteMap`, `heatMap` and `tileMap` in software,
/// and each of those satisfied this factor while a second, genuinely unrelated word ("options",
/// "config", "plan", "planner", "executor") satisfied the step. So `sourceMapOptions` proved a
/// mapper's configuration and `RoadMapPlanner` proved its execution plan, both of them anywhere in
/// any repository. A bare `map` counts only when what it maps is named beside it — which is what
/// the real anchors do, being named for the *type* map they build a plan for.
fn belongs_to_object_mapper(citation: &AgentCitationDto) -> bool {
    if names_token(citation, &["mapper", "mappers", "mapping", "mappings"]) {
        return true;
    }
    names_token(citation, &["map", "maps"]) && names_token(citation, OBJECT_MAPPER_SUBJECT_WORDS)
}

fn names_mapper_configuration(citation: &AgentCitationDto) -> bool {
    names_token_prefix(citation, &["config", "profile", "option"])
}

pub fn citation_owns_mapper_configuration(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && belongs_to_object_mapper(citation)
        && names_mapper_configuration(citation)
        && !names_token_prefix(citation, &["plan", "execut", "pipeline"])
}

/// The plan a mapper executes. "mapper" and "mapping" are absent from the step list on purpose:
/// they are what `belongs_to_object_mapper` already asks for, so listing them here let a symbol
/// named nothing but `Mapper` satisfy both of this carrier's factors at once.
pub fn citation_owns_mapper_execution(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && belongs_to_object_mapper(citation)
        && !names_mapper_configuration(citation)
        && names_token_prefix(citation, &["plan", "execut", "pipeline"])
}

// ---------------------------------------------------------------------------
// String predicates
// ---------------------------------------------------------------------------

fn belongs_to_string_predicates(citation: &AgentCitationDto) -> bool {
    matches!(citation.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        && (names_token(citation, &["string", "strings", "text"])
            || (names_token(citation, &["char"])
                && names_token(citation, &["sequence", "sequences"])))
}

/// A string helper's blank-or-whitespace predicate. The string subject is deliberately read from
/// the symbol, not its package path, so an unrelated record predicate filed beside text helpers
/// cannot close the requirement.
pub fn citation_owns_string_blank_predicate(citation: &AgentCitationDto) -> bool {
    belongs_to_string_predicates(citation) && names_token(citation, &["blank", "whitespace"])
}

/// The empty-string predicate remains distinct from blank/trim behavior.
pub fn citation_owns_string_empty_predicate(citation: &AgentCitationDto) -> bool {
    belongs_to_string_predicates(citation) && names_token(citation, &["empty"])
}

/// The helper that hands a case-sensitive comparison to a region matcher.
pub fn citation_owns_string_region_handoff(citation: &AgentCitationDto) -> bool {
    belongs_to_string_predicates(citation)
        && names_token(citation, &["region"])
        && names_token(
            citation,
            &["match", "matches", "matching", "compare", "equal", "equals"],
        )
}

// ---------------------------------------------------------------------------
// Runtime formatting
// ---------------------------------------------------------------------------

const RUNTIME_FORMATTING_WORDS: &[&str] = &[
    "format",
    "formats",
    "formatted",
    "formatter",
    "formatters",
    "formatting",
    "fmt",
    "vformat",
    "printf",
    "sprintf",
    "fprintf",
];

const RUNTIME_FORMAT_ARGUMENT_WORDS: &[&str] = &[
    "format",
    "formats",
    "formatted",
    "formatter",
    "formatters",
    "formatting",
];

/// Anchors that belong to the runtime-formatting subsystem. The error carrier below asks only
/// whether a symbol sounds like a failure path, which every subsystem in a repository has; scoping
/// it to the formatting surface is what stops `CliParseError` in `src/cli/parse.cc` from standing
/// in for the formatter's fallback.
fn belongs_to_runtime_formatting(citation: &AgentCitationDto) -> bool {
    names_token(citation, RUNTIME_FORMATTING_WORDS)
}

/// The type-erased argument store a runtime formatter reads from.
pub fn citation_owns_format_arguments(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation) && {
        let tokens = name_tokens(citation);
        has_token(&tokens, RUNTIME_FORMAT_ARGUMENT_WORDS)
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
pub fn citation_owns_formatter_fallback(citation: &AgentCitationDto) -> bool {
    owns_behavior(citation)
        && belongs_to_runtime_formatting(citation)
        && names_token_prefix(
            citation,
            &["error", "throw", "fail", "assert", "fallback", "panic"],
        )
}

// ---------------------------------------------------------------------------
// Subsystem scopes for the role-classified requirements
//
// The other half of the requirement tables does not use a carrier at all: it asks the shared
// evidence-role classifier "what kind of thing is this citation". That classifier is deliberately
// coarse — it answers a ranking question, not a coverage one — and a great deal of it keys on the
// *path*: anything under `runtime/` is runtime orchestration, anything under `indexer/` is symbol
// extraction, anything under `app/`, `views/` or `pages/` is route handling, anything under
// `flags/` is argument planning. So every symbol in those directories closed the requirement that
// listed the role, whatever the symbol actually was.
//
// These scopes are the same second factor the carriers already carry, made available to the
// role-classified requirements. They read the citation's own *name*: a directory can tell you where
// a symbol was filed, never what it does, and letting a directory supply the subsystem is precisely
// what lets an off-subject symbol inside a flow's own folder close that flow's requirement.
// ---------------------------------------------------------------------------

/// A callable that starts or schedules indexing work.
///
/// Both factors come from identifier tokens. A path under `indexer/` cannot promote an unrelated
/// `run`, and querying an index is not an indexing entrypoint. Explicit construction and write
/// methods remain eligible even when their owner is named `SearchIndex`; generic lifecycle verbs
/// are rejected when the owner or method instead names read/query/search/lookup execution.
const INDEXING_SUBJECT_TOKENS: &[&str] = &["index", "indexer", "indexing", "reindex"];
const INDEXING_MUTATION_ACTIONS: &[&str] = &[
    "index",
    "reindex",
    "build",
    "rebuild",
    "create",
    "construct",
    "generate",
    "write",
    "persist",
    "store",
    "save",
    "update",
    "insert",
    "upsert",
    "ingest",
    "populate",
    "materialize",
];
const INDEXING_OBSERVATION_ACTIONS: &[&str] = &[
    "read", "query", "search", "lookup", "execute", "scan", "fetch", "get", "list", "inspect",
    "load", "open", "find", "check", "describe",
];
const INDEXING_OBSERVATION_OWNER_NOUNS: &[&str] = &[
    "reader",
    "querier",
    "searcher",
    "executor",
    "scanner",
    "fetcher",
    "getter",
    "lister",
    "inspector",
    "loader",
    "opener",
    "finder",
    "checker",
    "describer",
];
const INDEXING_LIFECYCLE_ACTIONS: &[&str] =
    &["run", "start", "schedule", "queue", "enqueue", "dispatch"];
const INDEXING_DIRECT_OBJECT_TOKENS: &[&str] = &[
    "document",
    "file",
    "source",
    "record",
    "repository",
    "project",
    "workspace",
];
const TAXONOMY_PLURAL_ES_BYTES: &[u8] = &[101, 115];
const TAXONOMY_PLURAL_IES_BYTES: &[u8] = &[105, 101, 115];
const TAXONOMY_SPLIT_RE_BYTES: &[u8] = &[114, 101];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IndexingActionDirection {
    Mutation,
    Observation,
    Lifecycle,
    Unknown,
}

fn taxonomy_token_matches(token: &str, family: &str) -> bool {
    if token == family
        || token
            .strip_suffix('s')
            .is_some_and(|singular| singular == family)
    {
        return true;
    }
    let token_bytes = token.as_bytes();
    let family_bytes = family.as_bytes();
    (token_bytes.ends_with(TAXONOMY_PLURAL_ES_BYTES)
        && token_bytes.get(..token_bytes.len() - 2) == Some(family_bytes))
        || (token_bytes.ends_with(TAXONOMY_PLURAL_IES_BYTES)
            && family_bytes.ends_with(b"y")
            && token_bytes.get(..token_bytes.len() - 3)
                == family_bytes.get(..family_bytes.len() - 1))
}

fn taxonomy_has_token(tokens: &[String], family: &[&str]) -> bool {
    tokens.iter().any(|token| {
        family
            .iter()
            .any(|candidate| taxonomy_token_matches(token, candidate))
    })
}

fn indexing_action_direction(terminal_tokens: &[String]) -> IndexingActionDirection {
    let Some(action) = terminal_tokens.first().map(String::as_str) else {
        return IndexingActionDirection::Unknown;
    };
    if INDEXING_MUTATION_ACTIONS
        .iter()
        .any(|candidate| taxonomy_token_matches(action, candidate))
        || (action.as_bytes() == TAXONOMY_SPLIT_RE_BYTES
            && terminal_tokens.get(1).is_some_and(|token| token == "index"))
    {
        IndexingActionDirection::Mutation
    } else if INDEXING_OBSERVATION_ACTIONS
        .iter()
        .any(|candidate| taxonomy_token_matches(action, candidate))
    {
        IndexingActionDirection::Observation
    } else if INDEXING_LIFECYCLE_ACTIONS
        .iter()
        .any(|candidate| taxonomy_token_matches(action, candidate))
    {
        IndexingActionDirection::Lifecycle
    } else {
        IndexingActionDirection::Unknown
    }
}

fn is_intrinsic_indexing_action(terminal_tokens: &[String]) -> bool {
    terminal_tokens.first().is_some_and(|action| {
        taxonomy_token_matches(action, "index") || taxonomy_token_matches(action, "reindex")
    }) || (terminal_tokens
        .first()
        .is_some_and(|action| action.as_bytes() == TAXONOMY_SPLIT_RE_BYTES)
        && terminal_tokens.get(1).is_some_and(|token| token == "index"))
}

pub fn citation_owns_indexing_entrypoint(citation: &AgentCitationDto) -> bool {
    if !matches!(
        citation.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    ) {
        return false;
    }

    let tokens = name_tokens(citation);
    let terminal_tokens = identifier_tokens(terminal_segment_raw(&citation.display_name));
    let action_width = if terminal_tokens
        .first()
        .is_some_and(|token| token.as_bytes() == TAXONOMY_SPLIT_RE_BYTES)
        && terminal_tokens.get(1).is_some_and(|token| token == "index")
    {
        2
    } else {
        1
    };
    let terminal_subject_tokens = terminal_tokens.get(action_width..).unwrap_or_default();
    let owner_token_count = tokens.len().saturating_sub(terminal_tokens.len());
    let owner_tokens = &tokens[..owner_token_count];
    let has_distinct_subject = taxonomy_has_token(owner_tokens, INDEXING_SUBJECT_TOKENS)
        || taxonomy_has_token(terminal_subject_tokens, INDEXING_SUBJECT_TOKENS);

    match indexing_action_direction(&terminal_tokens) {
        IndexingActionDirection::Mutation => {
            let indexes_explicit_object = is_intrinsic_indexing_action(&terminal_tokens)
                && taxonomy_has_token(terminal_subject_tokens, INDEXING_DIRECT_OBJECT_TOKENS);
            has_distinct_subject || indexes_explicit_object
        }
        IndexingActionDirection::Observation | IndexingActionDirection::Unknown => false,
        IndexingActionDirection::Lifecycle => {
            has_distinct_subject
                && !taxonomy_has_token(&tokens, INDEXING_OBSERVATION_ACTIONS)
                && !taxonomy_has_token(&tokens, INDEXING_OBSERVATION_OWNER_NOUNS)
        }
    }
}

/// Indexing: discovering files, extracting symbols, and persisting them.
pub fn flow_belongs_to_indexing(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "index",
            "indexes",
            "indexed",
            "indexer",
            "indexers",
            "indexing",
            "symbol",
            "symbols",
            "snapshot",
            "snapshots",
            "workspace",
            "workspaces",
            "candidate",
            "candidates",
            "catalog",
            "catalogs",
            "ingest",
            "crawl",
        ],
    )
}

/// A server receiving and routing an inbound request.
pub fn flow_belongs_to_server_request(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "request",
            "requests",
            "route",
            "routes",
            "router",
            "routers",
            "routing",
            "controller",
            "controllers",
            "handler",
            "handlers",
            "endpoint",
            "endpoints",
            "server",
            "servers",
            "middleware",
            "http",
            "https",
            "protocol",
            // "dispatch"/"dispatcher" are absent for the same reason "render" is absent from the
            // static-site subject list: they are this flow's *step*, and the role classifier grants
            // `RequestDispatch` from the same word. While both lists held it, `dispatchRider` — or
            // any other name with "dispatch" in it — satisfied the subsystem factor and the role
            // with one word and closed the dispatch step of two different flows.
            // The name each ecosystem gives the server-to-application gateway. These are protocol
            // names in the same sense as "http", not product names: a server's request entrypoint
            // is routinely named for the gateway it speaks and for nothing else, with no other
            // request word anywhere in the symbol.
            "wsgi",
            "asgi",
            "cgi",
            "fastcgi",
            "rack",
            "servlet",
            "gateway",
        ],
    )
}

/// A client assembling and issuing an outbound request.
pub fn flow_belongs_to_client_request(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "client",
            "clients",
            "http",
            "https",
            "request",
            "requests",
            "instance",
            "instances",
            "factory",
            "factories",
            "session",
            "sessions",
            "transport",
            "transports",
            "adapter",
            "adapters",
            "send",
            "sends",
            "fetch",
            "url",
            "urls",
            "connection",
            "connections",
        ],
    )
}

/// Where a request leaves the process and a response comes back.
pub fn flow_belongs_to_request_terminal(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "adapter",
            "adapters",
            "transport",
            "transports",
            "response",
            "responses",
            "socket",
            "sockets",
            "stream",
            "streams",
            "writer",
            "sink",
            "buffer",
            "send",
            "sends",
            "sender",
        ],
    )
}

/// A URL session and the delegate callbacks it drives.
pub fn flow_belongs_to_url_session(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "session",
            "sessions",
            "task",
            "tasks",
            "delegate",
            "delegates",
            "url",
            "urls",
            "request",
            "requests",
            "response",
            "responses",
            "client",
            "clients",
            "transport",
            "connection",
            "connections",
        ],
    )
}

/// Bringing a command server up.
pub fn flow_belongs_to_command_server(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "server",
            "servers",
            "serve",
            "daemon",
            "bootstrap",
            "startup",
            "init",
            "main",
            "listen",
            "listener",
        ],
    )
}

/// The loop that waits for readiness and fires callbacks.
pub fn flow_belongs_to_event_loop(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "event", "events", "loop", "loops", "poll", "polling", "select", "epoll", "kqueue",
            "reactor", "tick",
        ],
    )
}

/// Reading a command off the wire.
pub fn flow_belongs_to_network_input(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "network",
            "networking",
            "socket",
            "sockets",
            "connection",
            "connections",
            "client",
            "clients",
            "query",
            "queries",
            "protocol",
            "wire",
        ],
    )
}

/// Choosing and running the command a request named.
///
/// "dispatch"/"dispatcher" are absent: the role classifier grants `RequestDispatch` and
/// `CommandDispatch` from that same word, so listing it here let one word answer both "is this the
/// command subsystem" and "is this its dispatch step". A command dispatcher says "command".
pub fn flow_belongs_to_command_dispatch(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "command", "commands", "table", "handler", "handlers", "exec", "execute",
        ],
    )
}

/// The callable that repeatedly drives an event loop, rather than a helper that creates,
/// rebinds, destroys, or observes one.
///
/// Event-loop names are unusually prone to false coverage: `rebindEventLoop` and
/// `deleteEventLoop` both earn the broad `EventLoop` evidence role while proving nothing about
/// how registered work is driven. This carrier admits only a loop/reactor owner paired with a
/// driver action, or a compound `processEvents`-style callable. The exact outgoing CALL is checked
/// separately by `command_event_loop_driver_call_target`.
pub fn citation_owns_command_event_loop_driver(citation: &AgentCitationDto) -> bool {
    if !owns_callable_behavior(citation) {
        return false;
    }
    let tokens = name_tokens(citation);
    if has_token(
        &tokens,
        &[
            "client",
            "connection",
            "delete",
            "destroy",
            "metric",
            "metrics",
            "monitoring",
            "observability",
            "rebind",
            "telemetry",
        ],
    ) {
        return false;
    }
    let terminal = identifier_tokens(terminal_segment_raw(&citation.display_name));
    let driver_action = has_token(
        &terminal,
        &[
            "dispatch", "main", "poll", "process", "run", "serve", "wait",
        ],
    );
    let loop_owner = has_token(&tokens, &["loop", "reactor"]);
    let processes_events =
        has_token(&terminal, &["process"]) && has_token(&terminal, &["event", "events"]);
    let event_runtime_main = has_token(&terminal, &["main"])
        && has_token(&tokens, &["event", "events"])
        && has_token(&tokens, &["runtime"]);
    let source_backed_main = has_token(&terminal, &["main"])
        && citation.source_excerpt.as_deref().is_some_and(|source| {
            let source_tokens = identifier_tokens(source);
            let names_loop = (has_token(&source_tokens, &["event", "events"])
                && has_token(&source_tokens, &["loop", "loops"]))
                || has_token(&source_tokens, &["reactor"]);
            let drives_loop = (has_token(&source_tokens, &["process"])
                && has_token(&source_tokens, &["event", "events"]))
                || has_token(
                    &source_tokens,
                    &["dispatch", "poll", "select", "tick", "wait"],
                );
            let repeats = has_token(&source_tokens, &["for", "loop", "loops", "until", "while"]);
            names_loop && drives_loop && repeats
        });
    driver_action && (loop_owner || processes_events || event_runtime_main || source_backed_main)
}

/// A callable whose name permits inspecting an exact outgoing event-loop boundary without making
/// the citation a carrier by itself. The caller remains inert unless the raw edge and declared
/// event-loop target both validate at the exact-boundary seam.
pub fn citation_may_start_command_event_loop_exact_boundary(citation: &AgentCitationDto) -> bool {
    if !owns_callable_behavior(citation) {
        return false;
    }
    let tokens = name_tokens(citation);
    let terminal = identifier_tokens(terminal_segment_raw(&citation.display_name));
    terminal.last().is_some_and(|token| token == "main")
        && !has_token(
            &tokens,
            &[
                "client",
                "config",
                "connection",
                "delete",
                "destroy",
                "metric",
                "metrics",
                "monitoring",
                "observability",
                "rebind",
                "telemetry",
            ],
        )
}

/// Typed CALL targets that perform one iteration of an event loop.
pub fn command_event_loop_driver_call_target(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    if has_token(
        &tokens,
        &[
            "client",
            "connection",
            "delete",
            "destroy",
            "handler",
            "metric",
            "metrics",
            "monitoring",
            "observability",
            "rebind",
            "telemetry",
        ],
    ) {
        return false;
    }
    let terminal = identifier_tokens(terminal_segment_raw(display_name));
    let processes_events =
        has_token(&terminal, &["process"]) && has_token(&terminal, &["event", "events"]);
    let dispatches_events = has_token(&terminal, &["dispatch"])
        && has_token(&terminal, &["event", "events", "callback", "callbacks"]);
    let owner_count = tokens.len().saturating_sub(terminal.len());
    let loop_owner = has_token(&tokens[..owner_count], &["loop", "poller", "reactor"]);
    let loop_action = has_token(
        &terminal,
        &["dispatch", "poll", "process", "select", "tick", "wait"],
    );
    processes_events || dispatches_events || (loop_owner && loop_action)
}

/// The central command router. Wrapper cleanup, module/plugin callbacks, documentation, and
/// observability helpers are deliberately excluded even when their names contain both "command"
/// and a dispatch verb. The exact outgoing routing/check/handler CALL is validated separately.
pub fn citation_owns_command_router(citation: &AgentCitationDto) -> bool {
    if !owns_callable_behavior(citation) {
        return false;
    }
    let tokens = name_tokens(citation);
    if !has_token(&tokens, &["command", "commands"])
        || has_token(
            &tokens,
            &[
                "benchmark",
                "cli",
                "doc",
                "docs",
                "help",
                "log",
                "logger",
                "metric",
                "metrics",
                "module",
                "monitoring",
                "observability",
                "plugin",
                "reset",
                "telemetry",
            ],
        )
    {
        return false;
    }
    let terminal = identifier_tokens(terminal_segment_raw(&citation.display_name));
    has_token(
        &terminal,
        &["dispatch", "execute", "handle", "process", "route"],
    )
}

/// Typed CALL targets that perform command lookup, policy checks, rejection/queueing, or handler
/// dispatch. Post-processing and command-adjacent module/help/metrics calls do not close the
/// routing boundary.
pub fn command_router_call_target(display_name: &str) -> bool {
    let tokens = identifier_tokens(display_name);
    if !has_token(&tokens, &["command", "commands"])
        || has_token(
            &tokens,
            &[
                "doc",
                "docs",
                "help",
                "log",
                "logger",
                "metric",
                "metrics",
                "module",
                "monitoring",
                "observability",
                "plugin",
                "processed",
                "telemetry",
            ],
        )
    {
        return false;
    }
    let terminal = identifier_tokens(terminal_segment_raw(display_name));
    has_token(
        &terminal,
        &[
            "check", "dispatch", "execute", "find", "handle", "invoke", "lookup", "queue",
            "reject", "resolve", "route", "validate",
        ],
    )
}

/// Planning and running a search.
pub fn flow_belongs_to_search(citation: &AgentCitationDto) -> bool {
    names_token(
        citation,
        &[
            "search", "searches", "searcher", "query", "queries", "grep", "match", "matcher",
            "matchers", "args", "argv", "arg", "main", "worker", "printer",
        ],
    )
}

const SEARCH_FLOW_OWNER_TOKENS: &[&str] = &[
    "arg", "args", "argv", "flag", "flags", "grep", "query", "queries", "search", "searcher",
];
const SEARCH_ARGUMENT_OWNER_TOKENS: &[&str] =
    &["arg", "args", "argv", "flag", "flags", "option", "options"];
const SEARCH_ARGUMENT_ACTION_TOKENS: &[&str] = &["parse", "plan"];
const SEARCH_CANDIDATE_COMPONENT_TOKENS: &[&str] = &["candidate", "walk", "walker"];
const SEARCH_HAYSTACK_COMPONENT_TOKENS: &[&str] = &["haystack"];
const SEARCH_MATCHER_COMPONENT_TOKENS: &[&str] = &["matcher"];
const SEARCH_SEARCHER_COMPONENT_TOKENS: &[&str] = &["searcher"];
const SEARCH_PRINTER_COMPONENT_TOKENS: &[&str] = &["printer"];
const WORKER_COMPONENT_TOKENS: &[&str] = &["worker"];

fn callable_owns_search_flow_component(
    citation: &AgentCitationDto,
    component_tokens: &[&str],
) -> bool {
    if !owns_callable_behavior(citation) {
        return false;
    }
    let terminal_tokens = identifier_tokens(terminal_segment_raw(&citation.display_name));
    if !has_token(&terminal_tokens, component_tokens) {
        return false;
    }
    let all_tokens = name_tokens(citation);
    let owner_token_count = all_tokens.len().saturating_sub(terminal_tokens.len());
    has_token(&all_tokens[..owner_token_count], SEARCH_FLOW_OWNER_TOKENS)
}

pub fn citation_owns_search_argument_planning(citation: &AgentCitationDto) -> bool {
    if !owns_callable_behavior(citation) {
        return false;
    }
    let terminal_tokens = identifier_tokens(terminal_segment_raw(&citation.display_name));
    if !has_token(&terminal_tokens, SEARCH_ARGUMENT_ACTION_TOKENS) {
        return false;
    }
    let all_tokens = name_tokens(citation);
    let owner_token_count = all_tokens.len().saturating_sub(terminal_tokens.len());
    has_token(
        &all_tokens[..owner_token_count],
        SEARCH_ARGUMENT_OWNER_TOKENS,
    )
}

pub fn citation_owns_search_candidate_traversal(citation: &AgentCitationDto) -> bool {
    callable_owns_search_flow_component(citation, SEARCH_CANDIDATE_COMPONENT_TOKENS)
}

pub fn citation_owns_search_haystack_construction(citation: &AgentCitationDto) -> bool {
    callable_owns_search_flow_component(citation, SEARCH_HAYSTACK_COMPONENT_TOKENS)
}

pub fn citation_owns_search_matcher_setup(citation: &AgentCitationDto) -> bool {
    callable_owns_search_flow_component(citation, SEARCH_MATCHER_COMPONENT_TOKENS)
}

pub fn citation_owns_search_searcher_setup(citation: &AgentCitationDto) -> bool {
    callable_owns_search_flow_component(citation, SEARCH_SEARCHER_COMPONENT_TOKENS)
}

pub fn citation_owns_search_printer_setup(citation: &AgentCitationDto) -> bool {
    callable_owns_search_flow_component(citation, SEARCH_PRINTER_COMPONENT_TOKENS)
}

pub fn citation_owns_search_worker_construction(citation: &AgentCitationDto) -> bool {
    if !owns_callable_behavior(citation) {
        return false;
    }
    let terminal_tokens = identifier_tokens(terminal_segment_raw(&citation.display_name));
    if !has_token(&terminal_tokens, WORKER_COMPONENT_TOKENS)
        || !has_token(&terminal_tokens, SEARCH_FLOW_OWNER_TOKENS)
    {
        return false;
    }
    let all_tokens = name_tokens(citation);
    let owner_token_count = all_tokens.len().saturating_sub(terminal_tokens.len());
    has_token(&all_tokens[..owner_token_count], SEARCH_FLOW_OWNER_TOKENS)
}

// ---------------------------------------------------------------------------
// Search evidence handoff
//
// A search implementation has two proof-bearing boundaries after execution: deciding what kind
// of evidence a result carries, and writing that evidence into the returned surface. Neither is
// interchangeable with generic search execution. Keeping the two factors in the citation's own
// callable name prevents a helper filed under `search/` from inheriting either obligation.
// ---------------------------------------------------------------------------

const SEARCH_EVIDENCE_CONTEXT_TOKENS: &[&str] = &["search", "result", "hit", "citation"];
pub(crate) const SEARCH_EVIDENCE_CLASSIFICATION_ACTIONS: &[&str] = &[
    "classify",
    "classification",
    "tier",
    "provenance",
    "resolve",
    "resolution",
    "eligibility",
];
pub(crate) const SEARCH_EVIDENCE_OUTPUT_ACTIONS: &[&str] = &[
    "append",
    "decorate",
    "emit",
    "render",
    "serialize",
    "write",
    "output",
];

fn owns_search_evidence_subject(citation: &AgentCitationDto) -> bool {
    let tokens = name_tokens(citation);
    taxonomy_has_token(&tokens, &["evidence"])
        && taxonomy_has_token(&tokens, SEARCH_EVIDENCE_CONTEXT_TOKENS)
}

pub fn citation_owns_search_evidence_classification(citation: &AgentCitationDto) -> bool {
    owns_callable_behavior(citation)
        && owns_search_evidence_subject(citation)
        && taxonomy_has_token(
            &name_tokens(citation),
            SEARCH_EVIDENCE_CLASSIFICATION_ACTIONS,
        )
}

pub fn citation_owns_search_evidence_output(citation: &AgentCitationDto) -> bool {
    owns_callable_behavior(citation)
        && owns_search_evidence_subject(citation)
        && taxonomy_has_token(&name_tokens(citation), SEARCH_EVIDENCE_OUTPUT_ACTIONS)
}

/// A schema requirement is proved by the schema file, so here the file *is* the subsystem: a `.sql`
/// anchor has no identifier of its own to scope by.
pub fn flow_belongs_to_sql_schema(citation: &AgentCitationDto) -> bool {
    path_has_any_extension(citation, &[".sql"])
}

#[cfg(test)]
fn taxonomy_plural(term: &str) -> String {
    let bytes = term.as_bytes();
    let last = bytes.last().copied();
    let penultimate = bytes.get(bytes.len().saturating_sub(2)).copied();
    if last == Some(b'y')
        && bytes
            .get(bytes.len().saturating_sub(2))
            .is_some_and(|previous| !matches!(previous, b'a' | b'e' | b'i' | b'o' | b'u'))
    {
        format!("{}ies", &term[..term.len() - 1])
    } else if matches!(last, Some(b's' | b'x' | b'z'))
        || matches!((penultimate, last), (Some(b'c' | b's'), Some(b'h')))
    {
        format!("{term}es")
    } else {
        format!("{term}s")
    }
}

#[cfg(test)]
const COMMAND_LOOP_CARRIER_TAXONOMY: &[&str] = &[
    "benchmark",
    "callback",
    "callbacks",
    "cli",
    "destroy",
    "doc",
    "docs",
    "for",
    "help",
    "metric",
    "plugin",
    "poller",
    "processed",
    "rebind",
    "reject",
    "reset",
    "runtime",
    "until",
    "wait",
    "while",
];

#[cfg(test)]
pub(crate) fn carrier_taxonomy_vocabulary() -> Vec<String> {
    let mut vocabulary = INDEXING_SUBJECT_TOKENS
        .iter()
        .chain(INDEXING_MUTATION_ACTIONS)
        .chain(INDEXING_OBSERVATION_ACTIONS)
        .chain(INDEXING_OBSERVATION_OWNER_NOUNS)
        .chain(INDEXING_LIFECYCLE_ACTIONS)
        .chain(INDEXING_DIRECT_OBJECT_TOKENS)
        .chain(SHELL_FUNCTION_SUBJECT_TOKENS)
        .chain(SHELL_FUNCTION_ACTION_TOKENS)
        .chain(SEARCH_FLOW_OWNER_TOKENS)
        .chain(SEARCH_ARGUMENT_OWNER_TOKENS)
        .chain(SEARCH_ARGUMENT_ACTION_TOKENS)
        .chain(SEARCH_CANDIDATE_COMPONENT_TOKENS)
        .chain(SEARCH_HAYSTACK_COMPONENT_TOKENS)
        .chain(SEARCH_MATCHER_COMPONENT_TOKENS)
        .chain(SEARCH_SEARCHER_COMPONENT_TOKENS)
        .chain(SEARCH_PRINTER_COMPONENT_TOKENS)
        .chain(WORKER_COMPONENT_TOKENS)
        .chain(SEARCH_EVIDENCE_CONTEXT_TOKENS)
        .chain(SEARCH_EVIDENCE_CLASSIFICATION_ACTIONS)
        .chain(SEARCH_EVIDENCE_OUTPUT_ACTIONS)
        .chain(COMMAND_LOOP_CARRIER_TAXONOMY)
        .chain(["evidence"].iter())
        .flat_map(|term| [(*term).to_string(), taxonomy_plural(term)])
        .collect::<Vec<_>>();
    vocabulary.sort();
    vocabulary.dedup();
    vocabulary
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
            target: None,
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
            source_excerpt: None,
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
    fn shell_function_dispatch_requires_distinct_subject_and_action_families() {
        for name in [
            "command_dispatch",
            "commands_dispatches",
            "shell_execute",
            "shells_executes",
            "function_run",
            "functions_run",
        ] {
            assert!(
                citation_owns_shell_function_dispatch(&citation(
                    name,
                    "src/runtime.sh",
                    NodeKind::FUNCTION,
                )),
                "{name} names both the shell-function subject and dispatch action",
            );
        }
        for name in [
            "dispatch",
            "dispatches",
            "execute",
            "executes",
            "executor",
            "executors",
            "command",
            "commands",
        ] {
            assert!(
                !citation_owns_shell_function_dispatch(&citation(
                    name,
                    "src/runtime.sh",
                    NodeKind::FUNCTION,
                )),
                "{name} supplies only one carrier factor",
            );
        }
    }

    #[test]
    fn command_loop_carriers_require_the_driver_and_central_router_boundaries() {
        for positive in ["EventLoop.run", "Reactor.processEvents", "eventRuntimeMain"] {
            assert!(
                citation_owns_command_event_loop_driver(&citation(
                    positive,
                    "src/runtime.c",
                    NodeKind::FUNCTION,
                )),
                "{positive} should own the event-loop driver boundary"
            );
        }
        for negative in [
            "Connection.rebindEventLoop",
            "Connection.rebindMain",
            "EventLoop.delete",
            "EventHandler.dispatch",
            "EventLoopMetrics.main",
            "EventLoopMetrics.poll",
            "mainConfig",
        ] {
            assert!(
                !citation_owns_command_event_loop_driver(&citation(
                    negative,
                    "src/runtime.c",
                    NodeKind::FUNCTION,
                )),
                "{negative} must not own the event-loop driver boundary"
            );
        }
        for main_boundary_candidate in ["main", "aeMain", "runtimeMain"] {
            assert!(
                citation_may_start_command_event_loop_exact_boundary(&citation(
                    main_boundary_candidate,
                    "src/runtime.c",
                    NodeKind::FUNCTION,
                )),
                "{main_boundary_candidate} may enter exact boundary discovery"
            );
            assert!(
                !citation_owns_command_event_loop_driver(&citation(
                    main_boundary_candidate,
                    "src/runtime.c",
                    NodeKind::FUNCTION,
                )),
                "{main_boundary_candidate} is not a carrier without its exact boundary"
            );
        }
        let mut source_backed_main = citation("runtimeMain", "src/runtime.c", NodeKind::FUNCTION);
        source_backed_main.source_excerpt = Some(
            "void runtimeMain(EventLoop *event_loop) { while (!event_loop->stop) { processEvents(event_loop); } }"
                .to_string(),
        );
        assert!(citation_owns_command_event_loop_driver(&source_backed_main));
        for positive in [
            "EventLoop.processEvents",
            "Poller.poll",
            "Reactor.dispatchEvents",
        ] {
            assert!(
                command_event_loop_driver_call_target(positive),
                "{positive} should be a lawful event-loop driver target"
            );
        }
        for negative in [
            "Connection.rebindEventLoop",
            "EventLoop.delete",
            "EventLoopMetrics.poll",
            "EventHandler.dispatch",
        ] {
            assert!(
                !command_event_loop_driver_call_target(negative),
                "{negative} must not close the event-loop driver boundary"
            );
        }

        for positive in [
            "processCommand",
            "CommandRouter.route",
            "CommandHandler.execute",
        ] {
            assert!(
                citation_owns_command_router(&citation(
                    positive,
                    "src/server.c",
                    NodeKind::FUNCTION,
                )),
                "{positive} should own central command routing"
            );
        }
        for negative in [
            "processCommandAndResetClient",
            "ModuleCommandDispatcher",
            "CommandHelp.process",
            "CommandMetrics.execute",
        ] {
            assert!(
                !citation_owns_command_router(&citation(
                    negative,
                    "src/server.c",
                    NodeKind::FUNCTION,
                )),
                "{negative} must not own central command routing"
            );
        }
        for positive in [
            "CommandTable.lookup",
            "commandCheckArity",
            "rejectCommand",
            "CommandHandler.execute",
            "queueCommand",
        ] {
            assert!(
                command_router_call_target(positive),
                "{positive} should be a lawful command-router target"
            );
        }
        for negative in [
            "commandProcessed",
            "ModuleCommandDispatcher",
            "CommandHelp.render",
            "CommandMetrics.record",
        ] {
            assert!(
                !command_router_call_target(negative),
                "{negative} must not close the command-router boundary"
            );
        }
    }

    #[test]
    fn shell_function_dispatch_requires_a_callable_anchor() {
        for (name, kind) in [
            ("nvm_command_dispatch", NodeKind::FUNCTION),
            ("ShellCommand::dispatch", NodeKind::METHOD),
        ] {
            assert!(
                citation_owns_shell_function_dispatch(&citation(name, "src/runtime.sh", kind)),
                "{name} is a callable shell dispatch anchor",
            );
        }

        for kind in [NodeKind::VARIABLE, NodeKind::FIELD, NodeKind::TYPEDEF] {
            assert!(
                !citation_owns_shell_function_dispatch(&citation(
                    "command_dispatch",
                    "src/runtime.sh",
                    kind,
                )),
                "a {kind:?} named like a shell dispatcher is not callable evidence",
            );
        }
    }

    #[test]
    fn search_evidence_carriers_require_subject_action_and_callable_shape() {
        for (name, kind) in [
            ("SearchHitEvidence::tier", NodeKind::METHOD),
            ("classify_result_evidence", NodeKind::FUNCTION),
        ] {
            assert!(
                citation_owns_search_evidence_classification(&citation(
                    name,
                    "src/retrieval/evidence.rs",
                    kind,
                )),
                "{name} classifies search-result evidence",
            );
        }
        for (name, kind) in [
            ("append_search_evidence_packet", NodeKind::FUNCTION),
            ("SearchEvidence::render_output", NodeKind::METHOD),
        ] {
            assert!(
                citation_owns_search_evidence_output(&citation(
                    name,
                    "src/runtime/output.rs",
                    kind,
                )),
                "{name} writes search evidence to an output surface",
            );
        }

        let wrong_action = citation(
            "SearchHitEvidence::rank",
            "src/retrieval/ranker.rs",
            NodeKind::METHOD,
        );
        assert!(!citation_owns_search_evidence_classification(&wrong_action));
        assert!(!citation_owns_search_evidence_output(&wrong_action));

        let non_callable = citation(
            "SearchHitEvidenceTier",
            "src/retrieval/evidence.rs",
            NodeKind::STRUCT,
        );
        assert!(!citation_owns_search_evidence_classification(&non_callable));
    }

    #[test]
    fn indexing_entrypoint_action_taxonomy_is_directional_and_table_driven() {
        let mutation_actions = [
            "index",
            "reindex",
            "build",
            "rebuild",
            "create",
            "construct",
            "generate",
            "write",
            "persist",
            "store",
            "save",
            "update",
            "insert",
            "upsert",
            "ingest",
            "populate",
            "materialize",
        ];
        let observation_actions = [
            "read", "query", "search", "lookup", "execute", "scan", "fetch", "get", "list",
            "inspect", "load", "open", "find", "check", "describe",
        ];

        for action in mutation_actions {
            for (name, kind) in [
                (format!("{action}_index"), NodeKind::FUNCTION),
                (format!("SearchIndex::{action}"), NodeKind::METHOD),
            ] {
                assert!(
                    citation_owns_indexing_entrypoint(&citation(&name, "src/services.rs", kind,)),
                    "{name} must claim mutation/construction ownership",
                );
            }
        }

        for action in observation_actions {
            for (name, kind) in [
                (format!("{action}_index"), NodeKind::FUNCTION),
                (format!("IndexService::{action}"), NodeKind::METHOD),
            ] {
                assert!(
                    !citation_owns_indexing_entrypoint(&citation(&name, "src/search.rs", kind,)),
                    "{name} must remain observational",
                );
            }
        }

        for owner_family in INDEXING_OBSERVATION_ACTIONS
            .iter()
            .chain(INDEXING_OBSERVATION_OWNER_NOUNS)
        {
            for owner in [(*owner_family).to_string(), taxonomy_plural(owner_family)] {
                let name = format!("Index_{owner}::run");
                assert!(
                    !citation_owns_indexing_entrypoint(&citation(
                        &name,
                        "src/search.rs",
                        NodeKind::METHOD,
                    )),
                    "singular/plural observation owner {name} must veto lifecycle ownership",
                );
            }
        }

        for action in INDEXING_LIFECYCLE_ACTIONS {
            for (name, kind) in [
                (format!("{action}_index"), NodeKind::FUNCTION),
                (format!("IndexingWorkQueue::{action}"), NodeKind::METHOD),
            ] {
                assert!(
                    citation_owns_indexing_entrypoint(&citation(&name, "src/services.rs", kind,)),
                    "{name} must claim indexing lifecycle ownership",
                );
            }
        }

        for (name, kind, expected) in [
            ("run_index", NodeKind::FUNCTION, true),
            (
                "IndexService::run_indexing_blocking_without_runtime_refresh",
                NodeKind::METHOD,
                true,
            ),
            ("BuildIndex::run", NodeKind::METHOD, true),
            ("index_file", NodeKind::FUNCTION, true),
            ("reindex_files", NodeKind::FUNCTION, true),
            ("re_index_files", NodeKind::FUNCTION, true),
            ("SearchIndex::build", NodeKind::METHOD, true),
            ("SearchIndex::execute_query", NodeKind::METHOD, false),
            ("IndexReader::read_index", NodeKind::METHOD, false),
            ("IndexReader::run", NodeKind::METHOD, false),
            ("IndexLookup::run", NodeKind::METHOD, false),
            ("run_indexed_query", NodeKind::FUNCTION, false),
            ("index", NodeKind::FUNCTION, false),
            ("cache_index", NodeKind::FUNCTION, false),
            ("build_files", NodeKind::FUNCTION, false),
            ("create_files", NodeKind::FUNCTION, false),
            ("run_index", NodeKind::VARIABLE, false),
            ("run", NodeKind::FUNCTION, false),
        ] {
            assert_eq!(
                citation_owns_indexing_entrypoint(&citation(name, "src/services.rs", kind,)),
                expected,
                "exact carrier {name}",
            );
        }
    }

    #[test]
    fn string_predicate_carriers_require_both_string_subject_and_distinct_behavior() {
        for name in ["StringUtils.isBlank", "TextPredicates::whitespaceOnly"] {
            assert!(
                citation_owns_string_blank_predicate(&citation(
                    name,
                    "src/text/predicates.rs",
                    NodeKind::METHOD,
                )),
                "{name} is a string blank predicate",
            );
        }
        for name in ["Strings.isEmpty", "CharSequencePredicates::empty"] {
            assert!(
                citation_owns_string_empty_predicate(&citation(
                    name,
                    "src/text/predicates.rs",
                    NodeKind::METHOD,
                )),
                "{name} is a string empty predicate",
            );
        }
        for name in ["Strings.regionMatches", "CharSequenceUtils::compareRegion"] {
            assert!(
                citation_owns_string_region_handoff(&citation(
                    name,
                    "src/text/compare.rs",
                    NodeKind::METHOD,
                )),
                "{name} is a string region handoff",
            );
        }

        for name in [
            "DatabaseRecord.isBlank",
            "Queue.isEmpty",
            "MemoryRegion.compare",
        ] {
            let unrelated = citation(name, "src/storage/state.rs", NodeKind::METHOD);
            assert!(!citation_owns_string_blank_predicate(&unrelated));
            assert!(!citation_owns_string_empty_predicate(&unrelated));
            assert!(!citation_owns_string_region_handoff(&unrelated));
        }
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
        assert!(!citation_owns_formatter_fallback(&arguments));
        assert!(citation_owns_formatter_fallback(&errors));
        assert!(!citation_owns_format_arguments(&errors));
    }

    #[test]
    fn runtime_formatting_rejects_words_that_only_neighbor_the_format_stem() {
        let neighbor_suffixes = ["ion", "ions", "ive", "ively", "iveness"];
        let argument_steps = ["Arg", "Args", "Arguments", "Store", "Value", "Values"];
        let failure_steps = ["Error", "Throw", "Fail", "Assert", "Fallback", "Panic"];
        let surfaces = [
            "src/geometry/one.rs",
            "src/geometry/one.vue",
            "src/geometry/one.svelte",
            "src/geometry/one.html",
        ];

        for suffix in neighbor_suffixes {
            let neighbor = format!("format{suffix}");
            for surface in surfaces {
                for step in argument_steps {
                    let arguments =
                        citation(&format!("{neighbor}{step}"), surface, NodeKind::FUNCTION);
                    assert!(
                        !citation_owns_format_arguments(&arguments),
                        "`{neighbor}` only shares a prefix with the formatting vocabulary: \
                         {arguments:?}"
                    );
                }
                for step in failure_steps {
                    let fallback =
                        citation(&format!("{neighbor}{step}"), surface, NodeKind::FUNCTION);
                    assert!(
                        !citation_owns_formatter_fallback(&fallback),
                        "`{neighbor}` only shares a prefix with the formatting vocabulary: \
                         {fallback:?}"
                    );
                }
            }
        }
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
    fn request_flow_carriers_require_independent_subject_and_action_tokens() {
        for (name, path) in [
            ("request", "src/requests/api.py"),
            ("get", "lib/transportkit/api.py"),
            ("get", "pkgs/http/lib/http.dart"),
        ] {
            assert!(citation_owns_client_public_facade_helper(&citation(
                name,
                path,
                NodeKind::FUNCTION,
            )));
        }
        for (name, path, kind) in [
            ("request", "src/client.py", NodeKind::FUNCTION),
            ("request", "src/database/api.py", NodeKind::FUNCTION),
            ("get", "src/cache/api.py", NodeKind::FUNCTION),
            ("request", "src/telemetry/api.py", NodeKind::FUNCTION),
            ("get", "src/monitoring/api.py", NodeKind::FUNCTION),
            ("request", "src/requests/api.py", NodeKind::METHOD),
            ("FrameKind.request", "src/requests/api.py", NodeKind::METHOD),
            ("Session.request", "src/requests/api.py", NodeKind::METHOD),
            ("Cache.request", "src/requests/api.py", NodeKind::METHOD),
            ("dispatch_hook", "src/requests/api.py", NodeKind::FUNCTION),
            ("get", "pkgs/http/lib/src/http.dart", NodeKind::FUNCTION),
        ] {
            assert!(!citation_owns_client_public_facade_helper(&citation(
                name, path, kind,
            )));
        }

        for positive in ["Session.request", "HttpClient.get"] {
            assert!(citation_owns_client_request_entrypoint(&citation(
                positive,
                "src/client.rs",
                NodeKind::METHOD,
            )));
        }
        for negative in [
            "FrameKind.request",
            "Session.get",
            "ClientCache.get",
            "HttpCache.get",
        ] {
            assert!(!citation_owns_client_request_entrypoint(&citation(
                negative,
                "src/client.rs",
                NodeKind::METHOD,
            )));
        }
        assert!(!citation_owns_client_request_entrypoint(&citation(
            "Session.request",
            "src/storage.rs",
            NodeKind::METHOD,
        )));

        for positive in ["Session.send", "HttpClient.send"] {
            assert!(citation_owns_client_request_dispatch(&citation(
                positive,
                "src/client.rs",
                NodeKind::METHOD,
            )));
        }
        for negative in [
            "dispatch_hook",
            "HttpTransportAdapter.send",
            "Connection.execute",
            "RequestRouter.dispatch",
            "HttpCache.send",
        ] {
            assert!(!citation_owns_client_request_dispatch(&citation(
                negative,
                "src/client.rs",
                NodeKind::FUNCTION,
            )));
        }

        for positive in ["Session.get_adapter", "HttpClient.selectTransport"] {
            assert!(citation_owns_client_adapter_selection(&citation(
                positive,
                "src/client.rs",
                NodeKind::METHOD,
            )));
        }
        for negative in [
            "dispatch_hook",
            "Session.send",
            "BaseAdapter.send",
            "AdapterRegistry.get",
        ] {
            assert!(!citation_owns_client_adapter_selection(&citation(
                negative,
                "src/client.rs",
                NodeKind::METHOD,
            )));
        }

        for positive in [
            "BaseAdapter.send",
            "HttpTransport.send",
            "BaseClient.send",
            "IOClient.send",
            "Client.send",
        ] {
            assert!(
                citation_owns_client_transport_send(&citation(
                    positive,
                    "src/client.rs",
                    NodeKind::METHOD,
                )),
                "{positive} should prove client transport send"
            );
        }
        for negative in [
            "dispatch_hook",
            "Session.send",
            "HttpCache.send",
            "DatabaseClient.send",
            "HookClient.send",
        ] {
            assert!(
                !citation_owns_client_transport_send(&citation(
                    negative,
                    "src/client.rs",
                    NodeKind::METHOD,
                )),
                "{negative} must not prove client transport send"
            );
        }

        for positive in ["app.route", "RequestRouter.route"] {
            assert!(citation_owns_server_request_entrypoint(&citation(
                positive,
                "lib/application.js",
                NodeKind::FUNCTION,
            )));
        }
        for negative in [
            "WidgetFactory.create",
            "ApplicationFactory.create",
            "HttpClient.new",
            "ServerPluginRegistry.registerPlugin",
        ] {
            assert!(!citation_owns_server_request_entrypoint(&citation(
                negative,
                "lib/application.js",
                NodeKind::FUNCTION,
            )));
        }

        for positive in ["RouterGroup.Handle", "RouteGroup.register"] {
            assert!(citation_owns_server_route_registration(&citation(
                positive,
                "lib/application.js",
                NodeKind::METHOD,
            )));
        }
        for negative in [
            "EventHandler.Handle",
            "Tree.Handle",
            "UiGroup.register",
            "RouteGroup.inspect",
        ] {
            assert!(!citation_owns_server_route_registration(&citation(
                negative,
                "lib/application.js",
                NodeKind::METHOD,
            )));
        }

        assert!(server_route_insertion_call_target("Engine.addRoute"));
        assert!(server_request_handler_call_source("Engine.ServeHTTP"));
        assert!(server_route_lookup_call_target("node.getValue"));
        assert!(server_handler_chain_call_target("Context.Next"));
        for negative in [
            "EventHandler.handle",
            "SegmentTree.add",
            "SearchEngine.handleRequest",
            "Config.getValue",
            "RenderContext.Next",
        ] {
            assert!(
                !server_route_insertion_call_target(negative)
                    && !server_request_handler_call_source(negative)
                    && !server_route_lookup_call_target(negative)
                    && !server_handler_chain_call_target(negative),
                "{negative} must not close a server-route boundary"
            );
        }

        for positive in [
            "app.handle",
            "RequestRouter.dispatch",
            "Engine.handleHTTPRequest",
        ] {
            assert!(
                citation_owns_server_request_dispatch(&citation(
                    positive,
                    "lib/application.js",
                    NodeKind::FUNCTION,
                )),
                "{positive} should prove server request dispatch"
            );
            assert!(
                server_request_dispatch_call_target(positive),
                "{positive} should be a lawful dispatch CALL target"
            );
        }
        assert!(server_request_dispatch_call_target("handleHTTPRequest"));
        assert!(!citation_owns_server_request_dispatch(&citation(
            "handleHTTPRequest",
            "lib/application.js",
            NodeKind::FUNCTION,
        )));
        let dispatch_terms = crate::packet_terms::packet_probe_terms(
            "Trace how an HTTP server routes an incoming request through route registration, request handler dispatch, and response finalization.",
        );
        assert!(
            packet_server_dispatch_callable_rank_bonus(
                &citation(
                    "ServerEngine.handleHTTPRequest",
                    "src/http/server.go",
                    NodeKind::METHOD,
                ),
                &dispatch_terms,
            ) > 0.0
        );
        assert_eq!(
            packet_server_dispatch_callable_rank_bonus(
                &citation("RouteGroup.Group", "src/http/group.go", NodeKind::METHOD),
                &dispatch_terms,
            ),
            0.0
        );
        for negative in [
            "EventProcessor.handle",
            "EventHandler.handle",
            "HttpClient.dispatch",
            "HttpClient.handleHTTPRequest",
            "Telemetry.handleHTTPRequest",
            "IRouter.Group",
        ] {
            assert!(
                !citation_owns_server_request_dispatch(&citation(
                    negative,
                    "lib/application.js",
                    NodeKind::FUNCTION,
                )),
                "{negative} must not prove server request dispatch"
            );
        }

        for positive in ["res.send", "ResponseWriter.flush"] {
            assert!(citation_owns_server_response_terminal(&citation(
                positive,
                "lib/response.js",
                NodeKind::FUNCTION,
            )));
        }
        for negative in [
            "EmailSender.send",
            "StreamWriter.write",
            "ResponseParser.finish",
        ] {
            assert!(!citation_owns_server_response_terminal(&citation(
                negative,
                "lib/response.js",
                NodeKind::FUNCTION,
            )));
        }

        for target in [
            "Session.prepare_request",
            "PreparedRequest.build",
            "Session.send",
            "Client.send",
        ] {
            assert!(client_request_entrypoint_call_target(target), "{target}");
        }
        for target in [
            "Telemetry.prepareRequest",
            "CacheBuilder.build",
            "Telemetry.send",
            "CacheClient.send",
            "CacheClient.prepareRequest",
            "DatabaseClient.send",
            "HookClient.send",
            "HookClient.prepareRequest",
        ] {
            assert!(!client_request_entrypoint_call_target(target), "{target}");
        }
        for positive in [
            "Session.request",
            "HttpClient.request",
            "requests.sessions.Session.request",
        ] {
            assert!(client_public_facade_successor_call_target(positive));
        }
        for negative in [
            "request",
            "requests.api.request",
            "FrameKind.request",
            "ClientRequestMetrics.request",
            "CacheClient.request",
            "DatabaseClient.request",
            "TelemetryClient.request",
            "MonitoringClient.request",
            "Session.prepare_request",
            "Session.send",
            "dispatch_hook",
        ] {
            assert!(!client_public_facade_successor_call_target(negative));
        }
        assert!(client_request_dispatch_predecessor_call_source(
            "Session.request"
        ));
        assert!(!client_request_dispatch_predecessor_call_source(
            "Logger.record"
        ));
        assert!(!client_request_dispatch_predecessor_call_source(
            "ClientRequestMetrics.record"
        ));
        assert!(client_request_dispatch_successor_call_target(
            "Session.get_adapter"
        ));
        assert!(!client_request_dispatch_successor_call_target(
            "Logger.record"
        ));
        assert!(!client_request_dispatch_successor_call_target(
            "AdapterResolveMetrics.record"
        ));
        assert!(server_request_entrypoint_call_target("Router.route"));
        assert!(server_request_entrypoint_call_target("Router.use"));
        assert!(server_request_entrypoint_call_target("Router.listen"));
        assert!(server_request_entrypoint_call_target("Server.listen"));
        assert!(!server_request_entrypoint_call_target("Server.route"));
        assert!(!server_request_entrypoint_call_target("Server.use"));
        assert!(server_request_dispatch_call_target("finalhandler"));
        for target in [
            "app.handle",
            "Router.dispatch",
            "Controller.handle",
            "Middleware.invoke",
            "Wsgi.handle",
            "Asgi.dispatch",
            "Servlet.invoke",
        ] {
            assert!(server_request_dispatch_call_target(target), "{target}");
        }
        assert!(server_response_terminal_call_target("ServerResponse.end"));
        for target in [
            "response.write",
            "ResponseSender.finish",
            "reply.flush",
            "writer.finish",
            "transport.write",
            "socket.end",
            "stream.flush",
            "output.finish",
        ] {
            assert!(server_response_terminal_call_target(target), "{target}");
        }
        assert!(!server_request_entrypoint_call_target(
            "PluginRegistry.registerPlugin"
        ));
        assert!(!server_request_entrypoint_call_target("Metrics.use"));
        assert!(!server_request_entrypoint_call_target("MetricsRouter.use"));
        assert!(!server_request_dispatch_call_target("HttpClient.dispatch"));
        assert!(!server_request_dispatch_call_target(
            "HttpClient.handleHTTPRequest"
        ));
        assert!(!server_request_dispatch_call_target("Telemetry.handle"));
        assert!(!server_request_dispatch_call_target(
            "Telemetry.handleHTTPRequest"
        ));
        assert!(!server_request_dispatch_call_target(
            "TelemetryRouter.handle"
        ));
        assert!(!server_response_terminal_call_target(
            "ResponseParser.parse"
        ));
        assert!(!server_response_terminal_call_target("Telemetry.end"));
        assert!(!server_response_terminal_call_target("Cache.write"));
        assert!(!server_response_terminal_call_target("EmailSender.finish"));
        assert!(!server_response_terminal_call_target(
            "TelemetryOutput.finish"
        ));
        assert!(!server_response_terminal_call_target(
            "MetricsOutput.finish"
        ));
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
        assert!(!citation_owns_formatter_fallback(&citation(
            "CliParseError",
            "src/cli/parse.cc",
            NodeKind::FUNCTION
        )));
        assert!(!citation_owns_formatter_fallback(&citation(
            "assert_valid_utf8",
            "src/text/utf8.rs",
            NodeKind::FUNCTION
        )));
        assert!(!citation_owns_formatter_fallback(&citation(
            "panic_hook",
            "src/runtime/panic.rs",
            NodeKind::FUNCTION
        )));
        // ...while the formatter's own failure path still closes it.
        assert!(citation_owns_formatter_fallback(&citation(
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
            "Site.write",
            "lib/site/site.rb",
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

    /// Each of these is a whole *family* of symbol, not one symbol: the HTTP verb set on any
    /// receiver, the `handle*` callback on any event, the `*Record` builder for any row, and the
    /// snake- or kebab-cased `use_*` in any language. Each family was accepted in full, and each
    /// is put back inside its own subsystem here rather than excluded by name.
    ///
    /// The receiver of each rejection sits in the accepting flow's *own* directory, because a
    /// carrier scoped by path rather than by name re-opens the moment a symbol is filed next to
    /// the evidence it is impersonating.
    #[test]
    fn carriers_reject_whole_families_of_off_subject_name() {
        // A verb-named accessor is not a client's convenience method, wherever it is filed.
        for name in [
            "Store.get",
            "Store.delete",
            "Cache.put",
            "FeatureFlags.options",
            "Queue.head",
            "Matrix.post",
            "Palette.patch",
        ] {
            assert!(
                !citation_owns_client_request_method(&citation(
                    name,
                    "lib/client.dart",
                    NodeKind::METHOD
                )),
                "{name} is a verb-named accessor, not an HTTP client's request method"
            );
        }
        assert!(citation_owns_client_request_method(&citation(
            "Client.get",
            "lib/client.dart",
            NodeKind::METHOD
        )));

        // A `handle*` callback is not a logging framework's record processing.
        for name in [
            "handleClick",
            "handleKeypress",
            "handleDragStart",
            "handleScroll",
            "handleResize",
        ] {
            assert!(
                !citation_owns_log_handler_processing(&citation(
                    name,
                    "src/logging/Handler.php",
                    NodeKind::FUNCTION
                )),
                "{name} names the verb `handle`, not a log handler"
            );
        }
        assert!(citation_owns_log_handler_processing(&citation(
            "LogProcessingHandler.write",
            "src/logging/Handler.php",
            NodeKind::METHOD
        )));

        // A `*Record` builder is not a logger's record creation.
        for name in [
            "createUserRecord",
            "createDnsRecord",
            "addBillingRecord",
            "makeInventoryRecord",
        ] {
            assert!(
                !citation_owns_log_record_creation(&citation(
                    name,
                    "src/logging/Logger.php",
                    NodeKind::FUNCTION
                )),
                "{name} builds a row, not a log record"
            );
        }
        assert!(citation_owns_log_record_creation(&citation(
            "Logger.addRecord",
            "src/logging/Logger.php",
            NodeKind::METHOD
        )));

        // `use_` and `use-` are not the hook naming convention, and a hook lives on a script.
        for name in ["use_temp_dir", "use-legacy-mode", "use_default_locale"] {
            assert!(
                !citation_owns_hook_public_export(&citation(
                    name,
                    "src/index/use-data.ts",
                    NodeKind::FUNCTION
                )),
                "{name} is not the `use` + capital hook convention"
            );
        }
        assert!(!citation_owns_hook_public_export(&citation(
            "useData",
            "src/index/use_data.rs",
            NodeKind::FUNCTION
        )));
        assert!(citation_owns_hook_public_export(&citation(
            "useData",
            "src/index/use-data.ts",
            NodeKind::FUNCTION
        )));

        // Serializing something on a script surface is not serializing the cache key, and calling
        // a cache is not the hook's cache helper.
        assert!(!citation_owns_hook_key_serialization(&citation(
            "serializeSettings",
            "src/_internal/utils/serialize.ts",
            NodeKind::FUNCTION
        )));
        assert!(citation_owns_hook_key_serialization(&citation(
            "serializeKey",
            "src/_internal/utils/serialize.ts",
            NodeKind::FUNCTION
        )));
        for name in ["Cache.put", "Cache.get", "Cache.write"] {
            assert!(
                !citation_owns_hook_cache_helper(&citation(
                    name,
                    "src/_internal/utils/helper.ts",
                    NodeKind::METHOD
                )),
                "{name} is a cache's own API, not the hook library's helper around one"
            );
        }
        assert!(citation_owns_hook_cache_helper(&citation(
            "makeCacheHelper",
            "src/_internal/utils/helper.ts",
            NodeKind::FUNCTION
        )));

        // A step word inside the site build's own directory is not the site build; neither is one
        // generic web noun beside a step verb, nor two of them. The receiver sits in the flow's own
        // folder, because a carrier scoped by path re-opens the moment a symbol is filed there.
        for name in [
            "Cache.write",
            "readManifest",
            "renderChart",
            "buildDnsRecord",
            // One generic web noun and a step verb.
            "Layout.render",
            "Page.render",
            "Template.render",
            "AssetPipeline.run",
            "Pipeline.run",
            "PaymentPageAllocator.process",
            "CrashReportCollection.generate",
            // Two *different* generic web nouns and a step verb — the fallback that survived
            // taking the directory away, and closed the flow on its own.
            "AssetCollection.process",
            "PageTemplate.render",
            "ThemeGenerator.run",
            "PostLayout.write",
            "DocumentTemplate.write",
            "LayoutRenderer.output",
            // A sitemap is navigation, not a static site, however the folder is spelled.
            "SiteMapGenerator.process",
            "SiteMap.write",
        ] {
            let anchor = citation(name, "lib/site/renderer.rb", NodeKind::METHOD);
            assert!(
                !citation_owns_site_terminal(&anchor),
                "{name} names no static site to write, only the folder it was filed in"
            );
            assert!(
                !citation_owns_site_lifecycle(&anchor),
                "{name} names no static site to build, only the folder it was filed in"
            );
        }
        assert!(citation_owns_site_terminal(&citation(
            "Site.write",
            "lib/site/site.rb",
            NodeKind::METHOD
        )));
        assert!(citation_owns_site_lifecycle(&citation(
            "Site.process",
            "lib/site/site.rb",
            NodeKind::METHOD
        )));

        // A single-file component is a script surface, so its own name has to say "form". While
        // `.vue` counted as markup, the `forms/` folder said it for every symbol in the file, and
        // these three closed the whole flow between them.
        for name in ["clampMin", "submitJob"] {
            let anchor = citation(name, "src/forms/Widget.vue", NodeKind::FUNCTION);
            assert!(
                !citation_owns_form_native_constraint(&anchor)
                    && !citation_owns_form_custom_validation(&anchor)
                    && !citation_owns_form_submit_guard(&anchor),
                "{name} in a single-file component names no form; the directory did"
            );
        }
        // `validityWindow` still closes one step, and is meant to: "validity" is the recorded
        // `form_custom_validation | validity` surface, a word that is both the form factor and the
        // step. It closes exactly the same step in a `.ts` file, which is the point — the extension
        // no longer decides. Its two siblings stay open, so the flow does not close.
        let validity_window =
            citation("validityWindow", "src/forms/Widget.vue", NodeKind::FUNCTION);
        assert!(citation_owns_form_custom_validation(&validity_window));
        assert!(!citation_owns_form_native_constraint(&validity_window));
        assert!(!citation_owns_form_submit_guard(&validity_window));
        assert!(citation_owns_form_custom_validation(&citation(
            "setCustomValidity",
            "src/forms/Widget.vue",
            NodeKind::FUNCTION
        )));
        assert!(citation_owns_form_native_constraint(&citation(
            "required",
            "examples/form.html",
            NodeKind::FUNCTION
        )));
    }
}
