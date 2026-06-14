use crate::agent::eval_probes::{
    eval_probes_enabled, push_eval_required_probe_queries,
    push_prompt_concept_derived_symbol_probes,
};
use crate::agent::packet_batch::packet_file_stem_matches_query;
use crate::agent::packet_scoring::{
    normalize_identifier, packet_display_path, packet_query_stop_term,
};
use crate::agent::packet_terms::{
    packet_probe_terms, packet_terms_have, packet_terms_have_any,
    packet_terms_indicate_indexing_flow, packet_terms_indicate_prepared_session_adapter_flow,
    packet_terms_indicate_request_dispatch_flow, packet_terms_indicate_search_execution_flow,
};
use crate::exact_symbol_query_terms;
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, PacketClaimDto, PacketTaskClassDto,
};

pub(crate) fn packet_missing_sufficiency_probe_queries_with_extra(
    question: &str,
    task_class: PacketTaskClassDto,
    answer: &AgentAnswerDto,
    supported_claims: &[PacketClaimDto],
    extra_probes: &[String],
) -> Vec<String> {
    packet_sufficiency_required_probe_queries_with_extra(question, task_class, extra_probes)
        .into_iter()
        .filter(|query| !packet_probe_query_is_covered(query, answer, supported_claims))
        .collect()
}

fn packet_probe_query_is_covered(
    query: &str,
    answer: &AgentAnswerDto,
    supported_claims: &[PacketClaimDto],
) -> bool {
    packet_probe_query_is_cited(query, answer)
        || packet_probe_query_is_claimed(query, supported_claims)
}

pub(crate) fn packet_probe_query_is_claimed(
    query: &str,
    supported_claims: &[PacketClaimDto],
) -> bool {
    if let Some(parts) = packet_file_scoped_symbol_probe_parts(query) {
        return supported_claims
            .iter()
            .any(|claim| packet_claim_covers_file_scoped_probe(&parts, claim));
    }

    if !packet_probe_query_allows_claim_coverage(query) {
        return false;
    }
    let normalized_query = normalize_identifier(query);
    if normalized_query.is_empty() {
        return false;
    }
    supported_claims.iter().any(|claim| {
        let normalized_claim = normalize_identifier(&claim.claim);
        normalized_claim.contains(&normalized_query)
    })
}

fn packet_claim_covers_file_scoped_probe(
    parts: &PacketFileScopedSymbolProbe,
    claim: &PacketClaimDto,
) -> bool {
    let claim_file_matches = claim.citations.iter().any(|citation| {
        citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .map(|path| {
                path.rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(path.as_str())
                    .eq_ignore_ascii_case(&parts.file_name)
            })
            .unwrap_or(false)
    });
    if !claim_file_matches {
        return false;
    }
    let normalized_claim = normalize_identifier(&claim.claim);
    parts
        .symbols
        .iter()
        .all(|symbol| normalized_claim.contains(symbol))
}

fn packet_probe_query_allows_claim_coverage(query: &str) -> bool {
    let trimmed = query.trim();
    trimmed.contains('.')
        && !trimmed.contains('/')
        && !trimmed.contains('\\')
        && !trimmed.chars().any(char::is_whitespace)
}

#[cfg(test)]
pub(crate) fn packet_sufficiency_required_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    packet_sufficiency_required_probe_queries_with_extra(question, task_class, &[])
}

pub(crate) fn packet_sufficiency_required_probe_queries_with_extra(
    question: &str,
    task_class: PacketTaskClassDto,
    extra_probes: &[String],
) -> Vec<String> {
    let terms = packet_probe_terms(question);
    let mut queries = packet_prompt_exact_symbol_probe_queries(question, &terms, task_class);
    push_unique_owned_terms(&mut queries, extra_probes);
    push_unique_owned_terms(
        &mut queries,
        &packet_sufficiency_required_probe_queries_from_terms(&terms, task_class),
    );
    queries
}

pub(crate) fn packet_sufficiency_required_probe_queries_from_terms(
    terms: &[String],
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    if !matches!(
        task_class,
        PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::EditPlanning
    ) {
        return Vec::new();
    }

    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let mut queries = Vec::new();

    if eval_probes_enabled() {
        push_eval_required_probe_queries(terms, &mut queries);
        return queries;
    }

    if has("exec") && has_any(&["runtime", "session"]) {
        push_unique_terms(&mut queries, &["exec runtime", "exec session"]);
    }
    if has("exec") && has_any(&["cli", "command", "subcommand"]) {
        push_unique_terms(&mut queries, &["exec cli", "exec command"]);
    }
    if has_any(&["json", "jsonl"]) && has_any(&["event", "events", "output"]) {
        push_unique_terms(&mut queries, &["json event output", "jsonl event output"]);
    }
    if has("thread") && has_any(&["start", "starts", "started"]) {
        push_unique_term(&mut queries, "thread start");
    }
    if has("turn") && has_any(&["start", "starts", "started"]) {
        push_unique_term(&mut queries, "turn start");
    }
    if has_any(&["storage", "persistent"]) || (has("data") && has_any(&["access", "accessed"])) {
        push_unique_terms(&mut queries, &["storage access", "persistent storage"]);
    }
    if packet_terms_indicate_indexing_flow(terms) {
        push_indexing_flow_required_probe_queries(&mut queries);
    }
    if packet_terms_indicate_request_dispatch_flow(terms) {
        push_unique_terms(
            &mut queries,
            &[
                "request interceptor",
                "request dispatch",
                "transport adapter",
            ],
        );
    }
    if packet_terms_indicate_prepared_session_adapter_flow(terms) {
        push_unique_terms(
            &mut queries,
            &[
                "request preparation",
                "session request",
                "session send",
                "adapter send",
                "adapter selection",
            ],
        );
    }
    if has("event") && has("loop") {
        push_unique_terms(
            &mut queries,
            &[
                "event loop",
                "event dispatch",
                "network input",
                "command dispatch",
            ],
        );
    }
    if has("call") && has_any(&["command", "commands", "dispatch", "dispatches"]) {
        push_unique_terms(&mut queries, &["command dispatch", "command handler"]);
    }
    if packet_terms_indicate_search_execution_flow(terms) {
        push_search_flow_probe_queries(&mut queries);
    }
    if has_any(&["indexing", "indexed", "indexer"])
        && (has_any(&["storage", "persistent", "project", "configuration", "group"])
            || has_any(&["command", "commands"]))
    {
        push_unique_terms(
            &mut queries,
            &["build index", "source group indexing", "indexer command"],
        );
    }

    queries
}

pub(crate) fn packet_prompt_exact_symbol_probe_queries(
    question: &str,
    terms: &[String],
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    if !matches!(
        task_class,
        PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::EditPlanning
            | PacketTaskClassDto::SymbolOwnership
            | PacketTaskClassDto::BugLocalization
    ) {
        return Vec::new();
    }

    let mut queries = Vec::new();
    for term in exact_symbol_query_terms(question) {
        if packet_prompt_exact_symbol_term_is_probe(&term) {
            push_unique_term(&mut queries, &term);
        }
    }
    if eval_probes_enabled() {
        push_prompt_concept_derived_symbol_probes(terms, &mut queries);
    }
    queries
}

fn packet_prompt_exact_symbol_term_is_probe(term: &str) -> bool {
    let trimmed = term.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let letters = trimmed
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .collect::<Vec<_>>();
    !letters.is_empty() && !letters.iter().all(|ch| ch.is_ascii_uppercase())
}

pub(crate) fn packet_concrete_file_probe_queries_from_required(
    required_queries: &[String],
) -> Vec<String> {
    let mut queries = Vec::new();
    for query in required_queries {
        if let Some(file_query) = packet_required_probe_file_query(query) {
            push_unique_term(&mut queries, &file_query);
        }
    }
    queries
}

fn packet_required_probe_file_query(query: &str) -> Option<String> {
    if !packet_required_probe_needs_concrete_file(query) {
        return None;
    }
    let normalized_query = normalize_identifier(query);
    if normalized_query == "eventprocessor" {
        return Some("event_processor.rs".to_string());
    }
    query
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        .then(|| format!("{query}.rs"))
}

pub(crate) fn push_indexing_flow_required_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "indexing entrypoint",
            "file discovery",
            "symbol extraction",
            "storage persistence",
            "search projection",
            "snapshot refresh",
        ],
    );
}

pub(crate) fn push_search_flow_probe_queries(queries: &mut Vec<String>) {
    push_unique_terms(
        queries,
        &[
            "search entrypoint",
            "flag parsing",
            "argument planning",
            "candidate file walk",
            "search execution",
            "parallel search",
            "result printer",
        ],
    );
}

pub(crate) fn packet_probe_query_is_cited(query: &str, answer: &AgentAnswerDto) -> bool {
    answer
        .citations
        .iter()
        .any(|citation| packet_citation_satisfies_required_probe(query, citation))
}

pub(crate) fn packet_citation_satisfies_required_probe(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    if let Some(matches_file_scoped_symbol) =
        packet_file_scoped_symbol_probe_matches(query, citation)
    {
        return matches_file_scoped_symbol;
    }
    if packet_required_probe_needs_concrete_file(query) {
        return packet_file_stem_matches_query(query, citation.file_path.as_deref());
    }
    if packet_required_probe_needs_full_token_coverage(query) {
        if packet_citation_probe_has_exact_identifier_match(query, citation) {
            return true;
        }
        let tokens = packet_probe_match_tokens(query);
        return !tokens.is_empty()
            && packet_citation_probe_token_coverage(query, citation) >= tokens.len();
    }
    let Some(match_rank) = packet_citation_probe_match_rank(query, citation) else {
        return false;
    };
    !packet_required_probe_needs_exact_match(query) || match_rank >= 4
}

pub(crate) fn packet_required_probe_needs_exact_match(query: &str) -> bool {
    query.contains("::") || query.contains('.')
}

fn packet_required_probe_needs_concrete_file(query: &str) -> bool {
    let normalized_query = normalize_identifier(query);
    normalized_query.contains("execevents") || normalized_query == "eventprocessor"
}

fn packet_required_probe_needs_full_token_coverage(query: &str) -> bool {
    matches!(
        normalize_identifier(query).as_str(),
        "indexingentrypoint"
            | "filediscovery"
            | "symbolextraction"
            | "storagepersistence"
            | "searchprojection"
            | "snapshotrefresh"
    )
}

fn packet_citation_probe_has_exact_identifier_match(
    query: &str,
    citation: &AgentCitationDto,
) -> bool {
    let normalized_query = normalize_identifier(query);
    if normalized_query.is_empty() {
        return false;
    }
    let normalized_display = normalize_identifier(&citation.display_name);
    normalized_display == normalized_query || normalized_display.ends_with(&normalized_query)
}

pub(crate) fn packet_citation_probe_match_rank(
    query: &str,
    citation: &AgentCitationDto,
) -> Option<u8> {
    let normalized_query = normalize_identifier(query);
    if normalized_query.is_empty() {
        return Some(0);
    }
    let normalized_display = normalize_identifier(&citation.display_name);
    let normalized_path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .map(|path| normalize_identifier(&path))
        .unwrap_or_default();
    if let Some(matches_file_scoped_symbol) =
        packet_file_scoped_symbol_probe_matches(query, citation)
    {
        if matches_file_scoped_symbol {
            Some(6)
        } else {
            None
        }
    } else if packet_file_stem_matches_query(query, citation.file_path.as_deref()) {
        Some(5)
    } else if normalized_display == normalized_query
        || normalized_display.ends_with(&normalized_query)
        || (!packet_required_probe_needs_exact_match(query)
            && packet_citation_probe_token_coverage(query, citation) >= 2)
    {
        Some(4)
    } else if normalized_path.contains(&normalized_query) {
        Some(3)
    } else if normalized_display.contains(&normalized_query) {
        Some(2)
    } else if !normalized_display.is_empty() && normalized_query.contains(&normalized_display) {
        Some(1)
    } else {
        None
    }
}

fn packet_file_scoped_symbol_probe_matches(
    query: &str,
    citation: &AgentCitationDto,
) -> Option<bool> {
    let parts = packet_file_scoped_symbol_probe_parts(query)?;
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    let file_name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path.as_str())
        .to_ascii_lowercase();
    if file_name != parts.file_name {
        return Some(false);
    }

    let normalized_display = normalize_identifier(&citation.display_name);
    if parts.symbols.len() >= 3 && parts.symbols[0] == "create" && parts.symbols[1] == "table" {
        let Some(table_name) = parts.symbols.last() else {
            return Some(false);
        };
        let expected = format!("createtable{table_name}");
        return Some(normalized_display == expected || normalized_display.ends_with(&expected));
    }
    if parts.symbols.len() >= 2 && parts.symbols[0] == "foreign" && parts.symbols[1] == "key" {
        return Some(
            normalized_display == "foreignkey" || normalized_display.ends_with("foreignkey"),
        );
    }
    Some(parts.symbols.iter().any(|symbol| {
        normalized_display == *symbol
            || normalized_display.ends_with(symbol)
            || packet_file_scoped_short_symbol_matches(&citation.display_name, symbol)
    }))
}

fn packet_file_scoped_short_symbol_matches(display_name: &str, symbol: &str) -> bool {
    if symbol.len() > 3 {
        return false;
    }
    display_name
        .rsplit(['.', ':', '#'])
        .next()
        .map(normalize_identifier)
        .is_some_and(|tail| tail == symbol)
}

pub(crate) struct PacketFileScopedSymbolProbe {
    pub(crate) query_path: String,
    pub(crate) file_name: String,
    pub(crate) raw_symbols: Vec<String>,
    pub(crate) symbols: Vec<String>,
}

pub(crate) fn packet_file_scoped_symbol_probe_parts(
    query: &str,
) -> Option<PacketFileScopedSymbolProbe> {
    let mut parts = query.split_whitespace();
    let file_part = parts
        .next()?
        .trim_matches(|ch: char| matches!(ch, '`' | '"' | '\''));
    let query_path = file_part.replace('\\', "/");
    let file_name = file_part.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    if !file_name.contains('.') {
        return None;
    }

    let raw_symbols = parts
        .map(|part| {
            part.trim_matches(|ch: char| matches!(ch, '`' | '"' | '\'' | ',' | ';'))
                .to_string()
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let symbols = raw_symbols
        .iter()
        .map(|part| normalize_identifier(part))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if symbols.is_empty() {
        return None;
    }

    Some(PacketFileScopedSymbolProbe {
        query_path,
        file_name,
        raw_symbols,
        symbols,
    })
}

pub(crate) fn packet_citation_probe_token_coverage(
    query: &str,
    citation: &AgentCitationDto,
) -> usize {
    let tokens = packet_probe_match_tokens(query);
    if tokens.len() < 2 {
        return 0;
    }
    let display = normalize_identifier(&citation.display_name);
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .map(|path| normalize_identifier(&path))
        .unwrap_or_default();
    tokens
        .iter()
        .filter(|token| display.contains(token.as_str()) || path.contains(token.as_str()))
        .count()
}

fn packet_probe_match_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for token in query
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() >= 3 && !packet_query_stop_term(token))
    {
        if !tokens.iter().any(|existing| existing == &token) {
            tokens.push(token);
        }
    }
    tokens
}

fn push_unique_term(terms: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.len() < 3 {
        return;
    }
    if !terms.iter().any(|term| term.eq_ignore_ascii_case(value)) {
        terms.push(value.to_string());
    }
}

fn push_unique_terms(terms: &mut Vec<String>, values: &[&str]) {
    for value in values {
        push_unique_term(terms, value);
    }
}

fn push_unique_owned_terms(terms: &mut Vec<String>, values: &[String]) {
    for value in values {
        push_unique_term(terms, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentCitationDto, NodeId, NodeKind, PacketClaimDto, RetrievalScoreBreakdownDto,
        SearchHitOrigin,
    };

    fn test_packet_citation(display_name: &str, file_path: &str, score: f32) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(format!(
                "test:{}:{}",
                display_name.replace(' ', "_"),
                file_path.replace(['/', '\\'], "_")
            )),
            display_name: display_name.to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some(file_path.to_string()),
            line: Some(1),
            score,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: 0.4,
                semantic: 0.2,
                graph: 0.3,
                total: score,
                provenance: Vec::new(),
            }),
        }
    }

    #[test]
    fn packet_probe_match_rank_uses_multi_token_path_coverage() {
        let mut citation = test_packet_citation(
            "std::collections::HashMap",
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
            0.6,
        );
        citation.kind = NodeKind::MODULE;

        assert_eq!(
            packet_citation_probe_match_rank("jsonl event output", &citation),
            Some(4)
        );
        assert_eq!(
            packet_citation_probe_token_coverage("jsonl event output", &citation),
            3
        );
    }

    #[test]
    fn packet_required_probe_matching_uses_file_stems_and_display_symbols() {
        let event_loop_entry = test_packet_citation("service::main", "src/event_loop.c", 0.9);
        let command_handler = test_packet_citation("CommandHandler", "src/commands.c", 0.9);
        let search_entrypoint =
            test_packet_citation("search_driver::run", "crates/search/src/main.rs", 0.9);
        let candidate_builder = test_packet_citation(
            "CandidateFiles",
            "crates/search/src/candidate_files.rs",
            0.9,
        );

        assert!(packet_citation_satisfies_required_probe(
            "event_loop.c main",
            &event_loop_entry
        ));
        assert!(packet_citation_satisfies_required_probe(
            "command handler",
            &command_handler
        ));
        assert!(packet_citation_satisfies_required_probe(
            "search driver run",
            &search_entrypoint
        ));
        assert!(packet_citation_satisfies_required_probe(
            "candidate files",
            &candidate_builder
        ));
    }

    #[test]
    fn file_scoped_required_probes_match_symbol_inside_file() {
        let gin_new = test_packet_citation("New", "gin.go", 0.9);
        let gin_with = test_packet_citation("Engine.With", "gin.go", 0.9);
        let binding_default = test_packet_citation("Default", "binding/binding.go", 0.9);
        let router_group = test_packet_citation("RouterGroup", "routergroup.go", 0.9);
        let router_group_handle = test_packet_citation("RouterGroup.Handle", "routergroup.go", 0.9);

        assert!(packet_citation_satisfies_required_probe(
            "gin.go New",
            &gin_new
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "gin.go New",
            &gin_with
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "gin.go Default",
            &binding_default
        ));
        assert!(packet_citation_satisfies_required_probe(
            "routergroup.go RouterGroup.Handle",
            &router_group_handle
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "routergroup.go RouterGroup.Handle",
            &router_group
        ));

        let create_track = test_packet_citation(
            "CREATE TABLE Track",
            "SampleDatabase/DataSources/Sample_Sqlite.sql",
            0.9,
        );
        let create_playlist_track = test_packet_citation(
            "CREATE TABLE PlaylistTrack",
            "SampleDatabase/DataSources/Sample_Sqlite.sql",
            0.9,
        );
        assert!(packet_citation_satisfies_required_probe(
            "SampleDatabase/DataSources/Sample_Sqlite.sql CREATE TABLE Track",
            &create_track
        ));
        assert!(!packet_citation_satisfies_required_probe(
            "SampleDatabase/DataSources/Sample_Sqlite.sql CREATE TABLE Track",
            &create_playlist_track
        ));
    }

    #[test]
    fn route_sufficiency_probes_can_be_covered_by_source_claims() {
        let claims = vec![
            PacketClaimDto {
                claim: "app.use registers middleware on the router.".to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "app.handle delegates request handling to the router.".to_string(),
                citations: Vec::new(),
            },
            PacketClaimDto {
                claim: "res.send prepares and sends the response body.".to_string(),
                citations: Vec::new(),
            },
        ];

        for probe in ["app.use", "app.handle", "res.send"] {
            assert!(
                packet_probe_query_is_claimed(probe, &claims),
                "expected claim-backed coverage for {probe}: {claims:?}"
            );
        }
    }
}
