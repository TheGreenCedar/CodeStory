use crate::agent::packet_batch::packet_file_stem_matches_query;
use crate::agent::packet_evidence_roles::{
    PacketEvidenceRole, packet_claim_key_for_citation, packet_evidence_role,
};
use crate::agent::packet_required_probes::{
    packet_citation_probe_match_rank, packet_citation_probe_token_coverage,
    packet_citation_satisfies_required_probe, packet_required_probe_needs_exact_match,
};
use crate::agent::packet_scoring::{
    normalize_identifier, packet_citation_key, packet_display_name_is_import_literal,
    packet_display_name_is_test_like, packet_display_path, packet_low_signal_display_name,
};
use crate::{query_mentions_non_primary_source, retrieval_file_role_from_path};
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, NodeKind, PacketBudgetLimitsDto, SearchHitOrigin,
};
use std::collections::{HashMap, HashSet};

pub(crate) fn cap_citations(answer: &mut AgentAnswerDto, limits: &PacketBudgetLimitsDto) -> bool {
    cap_citations_with_protected(answer, limits, &HashSet::new())
}

pub(crate) fn cap_citations_with_protected(
    answer: &mut AgentAnswerDto,
    limits: &PacketBudgetLimitsDto,
    protected_citation_keys: &HashSet<String>,
) -> bool {
    let original_len = answer.citations.len();
    let mut files = HashSet::new();
    let mut roles = HashSet::new();
    let mut claim_keys: HashSet<String> = HashSet::new();
    let mut secondary_claim_keys: HashSet<String> = HashSet::new();
    let mut kept = Vec::new();
    let mut deferred = Vec::new();

    let mut candidates = answer.citations.drain(..).collect::<Vec<_>>();
    prioritize_protected_citations(&mut candidates, protected_citation_keys);
    for citation in candidates {
        let file = citation.file_path.as_deref().map(packet_display_path);
        let role = packet_evidence_role(&citation);
        let claim_key = role.map(|role| packet_claim_key_for_citation(role, &citation));
        let low_priority_role = packet_low_priority_cap_role(role);
        let protected = packet_citation_is_protected(&citation, protected_citation_keys);
        if protected
            && kept.len() < limits.max_anchors as usize
            && packet_file_fits_limit(file.as_deref(), &files, limits.max_files)
        {
            if let Some(path) = file {
                files.insert(path);
            }
            if let Some(role) = role {
                roles.insert(role);
            }
            if let Some(ref claim_key) = claim_key {
                claim_keys.insert(claim_key.clone());
            }
            kept.push(citation);
            continue;
        }
        if let Some(ref claim_key) = claim_key
            && claim_keys.contains(claim_key)
            && replace_weaker_duplicate_claim_citation(
                &mut kept,
                claim_key,
                citation.clone(),
                protected_citation_keys,
            )
        {
            rebuild_packet_cap_tracking(&kept, &mut files, &mut roles, &mut claim_keys);
            continue;
        }
        let file_is_new = file.as_ref().is_some_and(|path| !files.contains(path));
        let role_is_new = role.is_some_and(|role| !roles.contains(&role));
        let claim_key_is_new = claim_key
            .as_ref()
            .is_some_and(|key| !claim_keys.contains(key));
        let secondary_claim_definition = claim_key.as_ref().is_some_and(|key| {
            claim_keys.contains(key)
                && !secondary_claim_keys.contains(key)
                && packet_keep_secondary_claim_definition(key, &citation)
        });
        let claim_key_expands_primary_packet_coverage =
            !low_priority_role && claim_key_is_new && (role_is_new || file_is_new);
        let expands_primary_packet_coverage = !low_priority_role
            && (claim_key_expands_primary_packet_coverage
                || role_is_new
                || kept.is_empty()
                || (claim_key.is_none() && file_is_new)
                || secondary_claim_definition);
        if kept.len() >= limits.max_anchors as usize
            && packet_primary_definition_file_citation(&citation)
            && replace_weaker_same_role_or_low_priority_citation(
                &mut kept,
                citation.clone(),
                protected_citation_keys,
                limits,
            )
        {
            rebuild_packet_cap_tracking(&kept, &mut files, &mut roles, &mut claim_keys);
            continue;
        }
        if kept.len() >= limits.max_anchors as usize
            && !low_priority_role
            && role_is_new
            && replace_overrepresented_role_citation(
                &mut kept,
                citation.clone(),
                protected_citation_keys,
                limits,
            )
        {
            rebuild_packet_cap_tracking(&kept, &mut files, &mut roles, &mut claim_keys);
            continue;
        }
        if kept.len() < limits.max_anchors as usize
            && expands_primary_packet_coverage
            && packet_file_fits_limit(file.as_deref(), &files, limits.max_files)
        {
            if let Some(path) = file {
                files.insert(path);
            }
            if let Some(role) = role {
                roles.insert(role);
            }
            if let Some(ref claim_key) = claim_key {
                claim_keys.insert(claim_key.clone());
                if secondary_claim_definition {
                    secondary_claim_keys.insert(claim_key.clone());
                }
            }
            kept.push(citation);
        } else {
            deferred.push(citation);
        }
    }

    let mut primary_new_files = Vec::new();
    let mut primary_duplicate_files = Vec::new();
    let mut low_priority_new_files = Vec::new();
    let mut low_priority_duplicate_files = Vec::new();
    for citation in deferred {
        let file = citation.file_path.as_deref().map(packet_display_path);
        let low_priority = packet_low_priority_cap_role(packet_evidence_role(&citation));
        if file.as_ref().is_some_and(|path| files.contains(path)) {
            if low_priority {
                low_priority_duplicate_files.push(citation);
            } else {
                primary_duplicate_files.push(citation);
            }
        } else if low_priority {
            low_priority_new_files.push(citation);
        } else {
            primary_new_files.push(citation);
        }
    }
    for citation in primary_new_files
        .into_iter()
        .chain(primary_duplicate_files)
        .chain(low_priority_new_files)
        .chain(low_priority_duplicate_files)
    {
        if kept.len() >= limits.max_anchors as usize {
            continue;
        }
        let file = citation.file_path.as_deref().map(packet_display_path);
        if !packet_file_fits_limit(file.as_deref(), &files, limits.max_files) {
            continue;
        }
        if let Some(path) = file {
            files.insert(path);
        }
        kept.push(citation);
    }

    let truncated = kept.len() < original_len;
    answer.citations = kept;
    truncated
}

pub(crate) fn packet_low_priority_cap_role(role: Option<PacketEvidenceRole>) -> bool {
    role.is_some_and(PacketEvidenceRole::is_low_priority_cap_role)
}

fn packet_citation_is_protected(
    citation: &AgentCitationDto,
    protected_citation_keys: &HashSet<String>,
) -> bool {
    packet_citation_protection_rank(citation, protected_citation_keys) < 2
}

fn packet_citation_protection_rank(
    citation: &AgentCitationDto,
    protected_citation_keys: &HashSet<String>,
) -> u8 {
    if citation.coverage_role.as_deref() == Some("explicit exact probe") {
        0
    } else if protected_citation_keys.contains(&packet_citation_key(citation)) {
        1
    } else {
        2
    }
}

fn prioritize_protected_citations(
    citations: &mut [AgentCitationDto],
    protected_citation_keys: &HashSet<String>,
) {
    citations
        .sort_by_key(|citation| packet_citation_protection_rank(citation, protected_citation_keys));
}

fn replace_weaker_same_role_or_low_priority_citation(
    kept: &mut [AgentCitationDto],
    candidate: AgentCitationDto,
    protected_citation_keys: &HashSet<String>,
    limits: &PacketBudgetLimitsDto,
) -> bool {
    let candidate_role = packet_evidence_role(&candidate);
    let candidate_file = candidate.file_path.as_deref().map(packet_display_path);
    let mut replacement: Option<(usize, u8, f32)> = None;

    for (index, existing) in kept.iter().enumerate() {
        if packet_citation_is_protected(existing, protected_citation_keys) {
            continue;
        }
        if !packet_file_fits_limit_after_replacement(
            candidate_file.as_deref(),
            kept,
            index,
            limits.max_files,
        ) {
            continue;
        }

        let existing_role = packet_evidence_role(existing);
        let replacement_priority = if packet_low_priority_cap_role(existing_role) {
            3
        } else if candidate_role.is_some()
            && candidate_role == existing_role
            && !packet_primary_definition_file_citation(existing)
        {
            2
        } else {
            0
        };
        if replacement_priority == 0 {
            continue;
        }

        let existing_rank = existing.score;
        let should_replace = replacement
            .map(|(_, best_priority, best_rank)| {
                replacement_priority > best_priority
                    || (replacement_priority == best_priority && existing_rank < best_rank)
            })
            .unwrap_or(true);
        if should_replace {
            replacement = Some((index, replacement_priority, existing_rank));
        }
    }

    let Some((index, _, _)) = replacement else {
        return false;
    };
    kept[index] = candidate;
    true
}

fn replace_overrepresented_role_citation(
    kept: &mut [AgentCitationDto],
    candidate: AgentCitationDto,
    protected_citation_keys: &HashSet<String>,
    limits: &PacketBudgetLimitsDto,
) -> bool {
    let Some(candidate_role) = packet_evidence_role(&candidate) else {
        return false;
    };
    if kept
        .iter()
        .any(|citation| packet_evidence_role(citation) == Some(candidate_role))
    {
        return false;
    }
    let candidate_file = candidate.file_path.as_deref().map(packet_display_path);
    let role_counts = kept.iter().filter_map(packet_evidence_role).fold(
        HashMap::<PacketEvidenceRole, usize>::new(),
        |mut counts, role| {
            *counts.entry(role).or_insert(0) += 1;
            counts
        },
    );

    let mut replacement: Option<(usize, usize, f32)> = None;
    for (index, existing) in kept.iter().enumerate() {
        if packet_citation_is_protected(existing, protected_citation_keys) {
            continue;
        }
        let Some(existing_role) = packet_evidence_role(existing) else {
            continue;
        };
        let existing_role_count = role_counts.get(&existing_role).copied().unwrap_or_default();
        if existing_role_count <= 1 {
            continue;
        }
        if !packet_file_fits_limit_after_replacement(
            candidate_file.as_deref(),
            kept,
            index,
            limits.max_files,
        ) {
            continue;
        }
        let existing_rank = existing.score;
        let should_replace = replacement
            .map(|(_, best_count, best_rank)| {
                existing_role_count > best_count
                    || (existing_role_count == best_count && existing_rank < best_rank)
            })
            .unwrap_or(true);
        if should_replace {
            replacement = Some((index, existing_role_count, existing_rank));
        }
    }

    let Some((index, _, _)) = replacement else {
        return false;
    };
    kept[index] = candidate;
    true
}

fn packet_file_fits_limit_after_replacement(
    path: Option<&str>,
    kept: &[AgentCitationDto],
    replacement_index: usize,
    max_files: u32,
) -> bool {
    let files = kept
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != replacement_index)
        .filter_map(|(_, citation)| citation.file_path.as_deref().map(packet_display_path))
        .collect::<HashSet<_>>();
    packet_file_fits_limit(path, &files, max_files)
}

fn replace_weaker_duplicate_claim_citation(
    kept: &mut [AgentCitationDto],
    claim_key: &str,
    candidate: AgentCitationDto,
    protected_citation_keys: &HashSet<String>,
) -> bool {
    let Some(index) = kept.iter().position(|citation| {
        packet_evidence_role(citation)
            .map(|role| packet_claim_key_for_citation(role, citation) == claim_key)
            .unwrap_or(false)
    }) else {
        return false;
    };
    if packet_citation_is_protected(&kept[index], protected_citation_keys) {
        return false;
    }
    if packet_prefer_duplicate_claim_citation(&candidate, &kept[index]) {
        kept[index] = candidate;
        return true;
    }
    false
}

fn packet_prefer_duplicate_claim_citation(
    candidate: &AgentCitationDto,
    existing: &AgentCitationDto,
) -> bool {
    if packet_prefer_flow_anchor_path_citation(candidate, existing) {
        return true;
    }
    normalize_identifier(&candidate.display_name) == normalize_identifier(&existing.display_name)
        && packet_exact_definition_file_citation(candidate)
        && !packet_exact_definition_file_citation(existing)
}

pub(crate) fn packet_primary_definition_file_citation(citation: &AgentCitationDto) -> bool {
    packet_exact_definition_file_citation(citation)
        || packet_near_stem_type_definition_file(citation)
}

fn packet_near_stem_type_definition_file(citation: &AgentCitationDto) -> bool {
    if citation.origin != SearchHitOrigin::IndexedSymbol
        || !citation.resolvable
        || !matches!(
            citation.kind,
            NodeKind::STRUCT
                | NodeKind::CLASS
                | NodeKind::INTERFACE
                | NodeKind::UNION
                | NodeKind::ENUM
                | NodeKind::TYPEDEF
        )
    {
        return false;
    }
    let normalized_display = normalize_identifier(&citation.display_name);
    if normalized_display.is_empty()
        || packet_low_signal_display_name(normalized_display.as_str())
        || packet_exact_definition_file_citation(citation)
    {
        return false;
    }
    let stem = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .and_then(|path| {
            let file_name = path.rsplit('/').next().unwrap_or(path.as_str());
            file_name
                .rsplit_once('.')
                .map(|(stem, _)| stem.to_string())
                .or_else(|| Some(file_name.to_string()))
        })
        .map(|stem| normalize_identifier(&stem))
        .unwrap_or_default();
    if stem.is_empty() {
        return false;
    }

    let len_delta = normalized_display.len().abs_diff(stem.len());
    if len_delta > 2 {
        return false;
    }
    let shared_prefix = normalized_display
        .chars()
        .zip(stem.chars())
        .take_while(|(left, right)| left == right)
        .count();
    shared_prefix >= 8
        && shared_prefix.saturating_mul(5)
            >= normalized_display.len().min(stem.len()).saturating_mul(4)
}

pub(crate) fn packet_prefer_flow_anchor_path_citation(
    candidate: &AgentCitationDto,
    existing: &AgentCitationDto,
) -> bool {
    let candidate_path = candidate
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let existing_path = existing
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if candidate_path == existing_path {
        return false;
    }
    let candidate_role = retrieval_file_role_from_path(&candidate_path);
    let existing_role = retrieval_file_role_from_path(&existing_path);
    candidate_role == crate::RetrievalFileRole::Source && existing_role.is_non_primary()
}

pub(crate) fn packet_exact_definition_file_citation(citation: &AgentCitationDto) -> bool {
    citation.origin == SearchHitOrigin::IndexedSymbol
        && citation.resolvable
        && matches!(
            citation.kind,
            NodeKind::STRUCT
                | NodeKind::CLASS
                | NodeKind::INTERFACE
                | NodeKind::UNION
                | NodeKind::ENUM
                | NodeKind::TYPEDEF
        )
        && !packet_low_signal_display_name(normalize_identifier(&citation.display_name).as_str())
        && packet_file_stem_matches_query(&citation.display_name, citation.file_path.as_deref())
}

fn packet_keep_secondary_claim_definition(_claim_key: &str, citation: &AgentCitationDto) -> bool {
    if !packet_primary_definition_file_citation(citation) {
        return false;
    }
    packet_mandatory_secondary_path_citation(citation)
}

fn packet_mandatory_secondary_path_citation(citation: &AgentCitationDto) -> bool {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    path.contains("event_processor")
        || path.contains("_events")
        || path.contains("-events")
        || path.contains("/cli/")
        || path.ends_with("/main.rs")
}

fn rebuild_packet_cap_tracking(
    kept: &[AgentCitationDto],
    files: &mut HashSet<String>,
    roles: &mut HashSet<PacketEvidenceRole>,
    claim_keys: &mut HashSet<String>,
) {
    files.clear();
    roles.clear();
    claim_keys.clear();
    for citation in kept {
        if let Some(path) = citation.file_path.as_deref().map(packet_display_path) {
            files.insert(path);
        }
        if let Some(role) = packet_evidence_role(citation) {
            roles.insert(role);
            claim_keys.insert(packet_claim_key_for_citation(role, citation));
        }
    }
}

fn packet_file_fits_limit(path: Option<&str>, files: &HashSet<String>, max_files: u32) -> bool {
    path.is_none_or(|path| files.contains(path) || files.len() < max_files as usize)
}

const PACKET_FOCUS_NEIGHBORHOOD_CARRY_LIMIT: usize = 4;

pub(crate) fn cap_packet_citations(
    answer: &mut AgentAnswerDto,
    limits: &PacketBudgetLimitsDto,
    required_probe_queries: &[String],
) -> bool {
    let mut protected_citation_keys =
        promote_required_probe_citations(answer, required_probe_queries);
    let focus_neighborhood_keys =
        promote_focus_neighborhood_citations(answer, &protected_citation_keys);
    protected_citation_keys.extend(focus_neighborhood_keys);
    prioritize_protected_citations(&mut answer.citations, &protected_citation_keys);
    if protected_citation_keys.is_empty() {
        cap_citations(answer, limits)
    } else {
        cap_citations_with_protected(answer, limits, &protected_citation_keys)
    }
}

pub(crate) fn promote_required_probe_citations(
    answer: &mut AgentAnswerDto,
    required_probe_queries: &[String],
) -> HashSet<String> {
    if required_probe_queries.is_empty() || answer.citations.is_empty() {
        return HashSet::new();
    }

    let mut seen_probe_queries = HashSet::new();
    let required_probe_queries = required_probe_queries
        .iter()
        .filter(|query| seen_probe_queries.insert(query.as_str()))
        .collect::<Vec<_>>();
    let focus_roots = packet_command_focus_roots(&answer.citations);
    let mut promoted_indices = Vec::new();
    let mut promoted_index_set = HashSet::new();
    for query in &required_probe_queries {
        let query = query.as_str();
        if let Some(limit) = packet_required_probe_multi_match_limit(query) {
            promote_distinct_required_probe_matches(
                answer,
                query,
                limit,
                &mut promoted_indices,
                &mut promoted_index_set,
                &focus_roots,
            );
            continue;
        }
        if promoted_indices
            .iter()
            .any(|index| packet_citation_satisfies_required_probe(query, &answer.citations[*index]))
        {
            continue;
        }
        let mut best_match = None;
        for (index, citation) in answer.citations.iter().enumerate() {
            if promoted_index_set.contains(&index) {
                continue;
            }
            let Some(match_rank) = packet_citation_probe_match_rank(query, citation) else {
                continue;
            };
            if packet_display_name_is_import_literal(&citation.display_name.to_ascii_lowercase())
                && !packet_citation_satisfies_required_probe(query, citation)
            {
                continue;
            }
            if best_match
                .map(|(best_index, best_rank)| {
                    packet_prefer_required_probe_match(
                        query,
                        citation,
                        match_rank,
                        &answer.citations[best_index],
                        best_rank,
                        &focus_roots,
                    )
                })
                .unwrap_or(true)
            {
                best_match = Some((index, match_rank));
            }
        }
        if let Some((index, _)) = best_match
            && promoted_index_set.insert(index)
        {
            promoted_indices.push(index);
        }
    }
    if promoted_indices.is_empty() {
        return HashSet::new();
    }

    let protected_citation_keys = promoted_indices
        .iter()
        .map(|index| packet_citation_key(&answer.citations[*index]))
        .collect::<HashSet<_>>();
    let mut reordered = Vec::with_capacity(answer.citations.len());
    for index in &promoted_indices {
        reordered.push(answer.citations[*index].clone());
    }
    for (index, citation) in answer.citations.drain(..).enumerate() {
        if !promoted_index_set.contains(&index) {
            reordered.push(citation);
        }
    }
    prioritize_protected_citations(&mut reordered, &protected_citation_keys);
    answer.citations = reordered;
    answer.retrieval_trace.annotations.push(format!(
        "packet_required_probe_citations promoted={} required={}",
        promoted_index_set.len(),
        required_probe_queries
            .iter()
            .map(|query| query.as_str())
            .collect::<Vec<_>>()
            .join("|")
            .replace('`', "'")
    ));
    protected_citation_keys
}

fn promote_distinct_required_probe_matches(
    answer: &AgentAnswerDto,
    query: &str,
    limit: usize,
    promoted_indices: &mut Vec<usize>,
    promoted_index_set: &mut HashSet<usize>,
    focus_roots: &[PacketCommandFocusRoot],
) {
    let mut promoted_paths = promoted_indices
        .iter()
        .filter(|index| packet_citation_satisfies_required_probe(query, &answer.citations[**index]))
        .filter_map(|index| packet_citation_file_path_key(&answer.citations[*index]))
        .collect::<HashSet<_>>();
    let prefer_shared_source_set = !packet_query_mentions_platform_source_set(query);

    while promoted_paths.len() < limit {
        let promoted_source_set_score = promoted_indices
            .iter()
            .filter(|index| {
                packet_citation_satisfies_required_probe(query, &answer.citations[**index])
            })
            .map(|index| packet_source_set_path_score(&answer.citations[*index]))
            .max()
            .unwrap_or_default();
        let mut best_match = None;
        for (index, citation) in answer.citations.iter().enumerate() {
            if promoted_index_set.contains(&index) {
                continue;
            }
            let Some(path) = packet_citation_file_path_key(citation) else {
                continue;
            };
            if promoted_paths.contains(&path) {
                continue;
            }
            if !packet_required_probe_multi_match_candidate(query, citation) {
                continue;
            }
            if prefer_shared_source_set
                && promoted_source_set_score >= 2
                && packet_source_set_path_score(citation) < promoted_source_set_score
            {
                continue;
            }
            let Some(match_rank) = packet_citation_probe_match_rank(query, citation) else {
                continue;
            };
            if packet_display_name_is_import_literal(&citation.display_name.to_ascii_lowercase())
                && !packet_citation_satisfies_required_probe(query, citation)
            {
                continue;
            }
            if best_match
                .map(|(best_index, best_rank)| {
                    packet_prefer_required_probe_match(
                        query,
                        citation,
                        match_rank,
                        &answer.citations[best_index],
                        best_rank,
                        focus_roots,
                    )
                })
                .unwrap_or(true)
            {
                best_match = Some((index, match_rank));
            }
        }
        let Some((index, _)) = best_match else {
            break;
        };
        if let Some(path) = packet_citation_file_path_key(&answer.citations[index]) {
            promoted_paths.insert(path);
        }
        if promoted_index_set.insert(index) {
            promoted_indices.push(index);
        }
    }
}

fn packet_required_probe_multi_match_limit(query: &str) -> Option<usize> {
    match normalize_identifier(query).as_str() {
        "mapperpublicapi" | "mapperruntimeapi" | "mappingruntimeentrypoint" => Some(3),
        "sqlschemascripts" | "schemadialectscripts" => Some(3),
        "bufferedsource" | "bufferedsink" | "bufferedwrapper" | "sourcebuffer" | "sinkbuffer"
        | "sourcereadbuffer" | "sinkwritebuffer" => Some(2),
        "httptoplevelhelper"
        | "publicclientfacade"
        | "clientconveniencemethod"
        | "clientinterfacemethod"
        | "clientinterfacehelper"
        | "requestfinalization"
        | "transportreadyrequestobject"
        | "clientsendimplementation"
        | "transportsend"
        | "requestresponse"
        | "responsestreamboundary" => Some(2),
        "htmlformrequiredconstraint"
        | "htmlformpatternconstraint"
        | "htmlformminmaxconstraints"
        | "customformvalidationinput"
        | "customvalidationvaliditystate"
        | "customvalidationerrorrendering"
        | "submitpreventdefault" => Some(2),
        "sessionrequestcreation"
        | "requestobjectcreation"
        | "requestresumedispatch"
        | "requestvalidationpipeline"
        | "delegatecallbackhandling"
        | "urlsessioncallbackboundary" => Some(2),
        normalized if normalized.ends_with("requestvalidation") => Some(2),
        "serverbootstrap"
        | "commandserverentrypoint"
        | "eventloopsource"
        | "networkcommandinput"
        | "commandtabledispatch"
        | "commanddispatch" => Some(2),
        _ => None,
    }
}

fn packet_required_probe_multi_match_candidate(query: &str, citation: &AgentCitationDto) -> bool {
    if query_mentions_non_primary_source(query) {
        return true;
    }
    if packet_display_name_is_test_like(&citation.display_name) {
        return false;
    }
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if path.is_empty() {
        return true;
    }
    if retrieval_file_role_from_path(&path).is_non_primary() {
        return false;
    }
    !path.contains("/test/")
        && !path.contains("/tests/")
        && !path.contains("/docs/")
        && !path.contains("/doc/")
        && !path.contains("/tools/")
        && !path.contains("/tool/")
        && !path.contains("/examples/")
        && !path.contains("/example/")
        && !path.contains("/third_party/")
        && !path.contains("/vendor/")
        && !path.contains("/node_modules/")
}

pub(crate) fn promote_focus_neighborhood_citations(
    answer: &mut AgentAnswerDto,
    protected_citation_keys: &HashSet<String>,
) -> HashSet<String> {
    if answer.citations.is_empty() {
        return HashSet::new();
    }
    let focus_roots = packet_command_focus_roots(&answer.citations);
    if focus_roots.is_empty() {
        return HashSet::new();
    }
    let protected_file_paths = answer
        .citations
        .iter()
        .filter(|citation| packet_citation_is_protected(citation, protected_citation_keys))
        .filter_map(packet_citation_file_path_key)
        .collect::<HashSet<_>>();

    let mut ranked_candidates = answer
        .citations
        .iter()
        .enumerate()
        .filter(|(_, citation)| {
            packet_focus_neighborhood_candidate(
                citation,
                &focus_roots,
                protected_citation_keys,
                &protected_file_paths,
            )
        })
        .map(|(index, citation)| {
            (
                index,
                packet_focus_neighborhood_rank(citation, &focus_roots),
            )
        })
        .collect::<Vec<_>>();
    ranked_candidates.sort_by(|(left_index, left_rank), (right_index, right_rank)| {
        right_rank
            .cmp(left_rank)
            .then_with(|| left_index.cmp(right_index))
    });

    let mut promoted_indices = Vec::new();
    let mut promoted_file_paths = HashSet::new();
    for (index, _) in ranked_candidates {
        let Some(path) = packet_citation_file_path_key(&answer.citations[index]) else {
            continue;
        };
        if !promoted_file_paths.insert(path) {
            continue;
        }
        promoted_indices.push(index);
        if promoted_indices.len() >= PACKET_FOCUS_NEIGHBORHOOD_CARRY_LIMIT {
            break;
        }
    }
    if promoted_indices.is_empty() {
        return HashSet::new();
    }

    let promoted_index_set = promoted_indices.iter().copied().collect::<HashSet<_>>();
    let promoted_keys = promoted_indices
        .iter()
        .map(|index| packet_citation_key(&answer.citations[*index]))
        .collect::<HashSet<_>>();
    let mut all_protected_citation_keys = protected_citation_keys.clone();
    all_protected_citation_keys.extend(promoted_keys.iter().cloned());
    let mut reordered = Vec::with_capacity(answer.citations.len());
    for citation in &answer.citations {
        if packet_citation_is_protected(citation, protected_citation_keys) {
            reordered.push(citation.clone());
        }
    }
    for index in promoted_indices {
        reordered.push(answer.citations[index].clone());
    }
    for (index, citation) in answer.citations.drain(..).enumerate() {
        if !packet_citation_is_protected(&citation, protected_citation_keys)
            && !promoted_index_set.contains(&index)
        {
            reordered.push(citation);
        }
    }
    prioritize_protected_citations(&mut reordered, &all_protected_citation_keys);
    answer.citations = reordered;
    answer.retrieval_trace.annotations.push(format!(
        "packet_focus_neighborhood_citations promoted={} roots={}",
        promoted_keys.len(),
        focus_roots
            .iter()
            .map(|root| root.root.as_str())
            .collect::<Vec<_>>()
            .join("|")
            .replace('`', "'")
    ));
    promoted_keys
}

fn packet_focus_neighborhood_candidate(
    citation: &AgentCitationDto,
    focus_roots: &[PacketCommandFocusRoot],
    protected_citation_keys: &HashSet<String>,
    protected_file_paths: &HashSet<String>,
) -> bool {
    if packet_citation_is_protected(citation, protected_citation_keys)
        || citation.origin != SearchHitOrigin::IndexedSymbol
        || !citation.resolvable
        || packet_display_name_is_import_literal(&citation.display_name.to_ascii_lowercase())
        || packet_display_name_is_test_like(&citation.display_name)
    {
        return false;
    }
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    if path.is_empty() || packet_citation_focus_root_score(citation, focus_roots) == 0 {
        return false;
    }
    if protected_file_paths.contains(&path) {
        return false;
    }
    !retrieval_file_role_from_path(&path.to_ascii_lowercase()).is_non_primary()
}

fn packet_citation_file_path_key(citation: &AgentCitationDto) -> Option<String> {
    let path = citation.file_path.as_deref().map(packet_display_path)?;
    if path.is_empty() { None } else { Some(path) }
}

fn packet_focus_neighborhood_rank(
    citation: &AgentCitationDto,
    focus_roots: &[PacketCommandFocusRoot],
) -> (u8, u8, u8, u8, u8, u8, i32) {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    let source_file: u8 = if retrieval_file_role_from_path(&path.to_ascii_lowercase())
        == crate::RetrievalFileRole::Source
    {
        1
    } else {
        0
    };
    let direct_root_file = packet_citation_direct_focus_root_file_score(citation, focus_roots);
    let role_backed: u8 = if packet_evidence_role(citation).is_some() {
        1
    } else {
        0
    };
    let implementation_file: u8 = if packet_path_is_implementation(&path) {
        1
    } else {
        0
    };
    let definition_file: u8 = if packet_primary_definition_file_citation(citation) {
        1
    } else {
        0
    };
    (
        packet_citation_focus_root_score(citation, focus_roots),
        direct_root_file,
        packet_source_navigation_file_score(&path),
        source_file,
        role_backed,
        implementation_file.saturating_add(definition_file),
        (citation.score * 1000.0).round() as i32,
    )
}

fn packet_citation_direct_focus_root_file_score(
    citation: &AgentCitationDto,
    focus_roots: &[PacketCommandFocusRoot],
) -> u8 {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .replace('\\', "/");
    let parent = path.rsplit_once('/').map(|(parent, _)| parent);
    focus_roots
        .iter()
        .filter(|root| parent == Some(root.root.as_str()))
        .map(|root| root.weight)
        .max()
        .unwrap_or_default()
}

fn packet_source_navigation_file_score(path: &str) -> u8 {
    let normalized = packet_display_path(path).replace('\\', "/");
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name)
        .to_ascii_lowercase();
    match stem.as_str() {
        "cli" | "cmd" | "command" | "commands" => 4,
        "lib" | "mod" | "index" => 3,
        "events" | "event" => 2,
        "main" | "app" | "server" | "router" | "routes" => 2,
        "handler" | "handlers" | "entrypoint" | "entrypoints" => 1,
        _ if stem.ends_with("_events")
            || stem.ends_with("_event")
            || stem.ends_with("-events")
            || stem.ends_with("-event") =>
        {
            2
        }
        _ => 0,
    }
}

fn packet_prefer_required_probe_match(
    query: &str,
    candidate: &AgentCitationDto,
    candidate_rank: u8,
    existing: &AgentCitationDto,
    existing_rank: u8,
    focus_roots: &[PacketCommandFocusRoot],
) -> bool {
    if !query_mentions_non_primary_source(query) {
        let candidate_test_like = packet_display_name_is_test_like(&candidate.display_name);
        let existing_test_like = packet_display_name_is_test_like(&existing.display_name);
        if candidate_test_like != existing_test_like {
            return !candidate_test_like;
        }
        if let Some(prefer_candidate) =
            packet_prefer_shared_source_set_citation(query, candidate, existing)
        {
            return prefer_candidate;
        }
    }
    if candidate_rank != existing_rank {
        return candidate_rank > existing_rank;
    }
    if !packet_required_probe_needs_exact_match(query) {
        let candidate_focus = packet_citation_focus_root_score(candidate, focus_roots);
        let existing_focus = packet_citation_focus_root_score(existing, focus_roots);
        if candidate_focus != existing_focus {
            return candidate_focus > existing_focus;
        }
        let candidate_token_coverage = packet_citation_probe_token_coverage(query, candidate);
        let existing_token_coverage = packet_citation_probe_token_coverage(query, existing);
        if candidate_token_coverage != existing_token_coverage {
            return candidate_token_coverage > existing_token_coverage;
        }
    }
    if packet_prefer_flow_anchor_path_citation(candidate, existing) {
        return true;
    }
    if packet_required_probe_prefers_implementation(query)
        && packet_prefer_implementation_file(candidate, existing)
    {
        return true;
    }
    packet_exact_definition_file_citation(candidate)
        && !packet_exact_definition_file_citation(existing)
}

fn packet_prefer_shared_source_set_citation(
    query: &str,
    candidate: &AgentCitationDto,
    existing: &AgentCitationDto,
) -> Option<bool> {
    if packet_query_mentions_platform_source_set(query) {
        return None;
    }
    let candidate_score = packet_source_set_path_score(candidate);
    let existing_score = packet_source_set_path_score(existing);
    (candidate_score != existing_score).then_some(candidate_score > existing_score)
}

fn packet_query_mentions_platform_source_set(query: &str) -> bool {
    let normalized = normalize_identifier(query);
    [
        "jvm", "nonjvm", "android", "ios", "native", "linux", "windows", "darwin", "apple", "wasm",
        "nodejs", "browser",
    ]
    .iter()
    .any(|term| normalized.contains(term))
}

fn packet_source_set_path_score(citation: &AgentCitationDto) -> u8 {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if path.is_empty() {
        return 1;
    }
    // Path-name heuristic; replace with indexed source-set metadata if that exists.
    if path.contains("/commonmain/")
        || path.contains("/common/")
        || path.contains("/shared/")
        || path.contains("/src/main/")
    {
        return 2;
    }
    if path.contains("/jvmmain/")
        || path.contains("/nonjvmmain/")
        || path.contains("/androidmain/")
        || path.contains("/iosmain/")
        || path.contains("/nativemain/")
        || path.contains("/linuxmain/")
        || path.contains("/windowsmain/")
        || path.contains("/darwinmain/")
        || path.contains("/applemain/")
        || path.contains("/wasmmain/")
        || path.contains("/wasmwasimain/")
        || path.contains("/nodejsmain/")
        || path.contains("/jsmain/")
        || path.contains("/browsermain/")
    {
        return 0;
    }
    1
}

fn packet_required_probe_prefers_implementation(query: &str) -> bool {
    query.contains("::") || query.contains('.') || normalize_identifier(query) == "requestmethod"
}

fn packet_prefer_implementation_file(
    candidate: &AgentCitationDto,
    existing: &AgentCitationDto,
) -> bool {
    let candidate_path = candidate
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    let existing_path = existing
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default();
    packet_path_is_implementation(&candidate_path) && !packet_path_is_implementation(&existing_path)
}

fn packet_path_is_implementation(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".d.ts")
        || lower.ends_with(".d.tsx")
        || lower.ends_with(".d.cts")
        || lower.ends_with(".d.mts")
    {
        return false;
    }
    matches!(
        lower.rsplit('.').next(),
        Some(
            "c" | "cc"
                | "cpp"
                | "cxx"
                | "go"
                | "java"
                | "js"
                | "jsx"
                | "kt"
                | "php"
                | "py"
                | "rb"
                | "rs"
                | "ts"
                | "tsx"
        )
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PacketCommandFocusRoot {
    root: String,
    weight: u8,
}

fn packet_command_focus_roots(citations: &[AgentCitationDto]) -> Vec<PacketCommandFocusRoot> {
    let mut roots = Vec::<PacketCommandFocusRoot>::new();
    for citation in citations {
        let display = citation.display_name.as_str();
        let normalized_display = normalize_identifier(display);
        let path = citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .unwrap_or_default();
        let Some(root) = packet_source_root_from_path(&path) else {
            continue;
        };
        let normalized_path = path.replace('\\', "/");
        let weight =
            if normalized_display.ends_with("runmain") || normalized_display.contains("runexec") {
                3
            } else if display.contains("::Cli")
                || display.contains("::cli")
                || normalized_path.ends_with("/src/cli.rs")
                || (normalized_path.ends_with("/main.rs") && normalized_display == "main")
            {
                2
            } else if display.contains("Subcommand::") {
                1
            } else {
                continue;
            };
        packet_push_focus_root(&mut roots, root, weight);
    }
    roots.sort_by(|left, right| {
        right
            .weight
            .cmp(&left.weight)
            .then_with(|| left.root.cmp(&right.root))
    });
    roots
}

fn packet_push_focus_root(roots: &mut Vec<PacketCommandFocusRoot>, root: String, weight: u8) {
    if let Some(existing) = roots.iter_mut().find(|existing| existing.root == root) {
        existing.weight = existing.weight.max(weight);
    } else {
        roots.push(PacketCommandFocusRoot { root, weight });
    }
}

fn packet_source_root_from_path(path: &str) -> Option<String> {
    let normalized = packet_display_path(path);
    let normalized = normalized.trim_matches('/').replace('\\', "/");
    if normalized.is_empty() {
        return None;
    }
    if let Some(index) = normalized.find("/src/") {
        let root = &normalized[..index + "/src".len()];
        return (!root.is_empty()).then(|| root.to_string());
    }
    let (parent, _) = normalized.rsplit_once('/')?;
    (!parent.is_empty()).then(|| parent.to_string())
}

fn packet_citation_focus_root_score(
    citation: &AgentCitationDto,
    focus_roots: &[PacketCommandFocusRoot],
) -> u8 {
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .replace('\\', "/");
    focus_roots
        .iter()
        .filter(|root| path == root.root || path.starts_with(&format!("{}/", root.root)))
        .map(|root| root.weight)
        .max()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        AgentResponseBlockDto, AgentResponseSectionDto, AgentRetrievalPolicyModeDto,
        AgentRetrievalPresetDto, AgentRetrievalTraceDto, NodeId, PacketEvidenceResolutionDto,
        PacketEvidenceTierDto,
    };

    fn citation(display_name: &str, file_path: &str, score: f32) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(format!("test::{display_name}")),
            display_name: display_name.to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some(file_path.to_string()),
            line: Some(1),
            score,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: Some(PacketEvidenceTierDto::ResolvedGraph),
            evidence_producer: Some("test".to_string()),
            resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
        }
    }

    fn answer_fixture(citations: Vec<AgentCitationDto>) -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "packet-capping-test".to_string(),
            prompt: "Trace the generic flow.".to_string(),
            summary: "Covered by cited anchors.".to_string(),
            freshness: None,
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "Covered by cited anchors.".to_string(),
                }],
            }],
            citations,
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "packet-capping-test".to_string(),
                retrieval_publication: None,
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                annotations: Vec::new(),
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        }
    }

    #[test]
    fn explicit_exact_probe_anchor_survives_citation_capping() {
        let ordinary = citation("ordinary", "src/ordinary.rs", 500.0);
        let mut exact = citation("selected", "src/selected.rs", 1.0);
        exact.coverage_role = Some("explicit exact probe".to_string());
        exact.eligible_for_sufficiency = Some(false);
        let mut answer = answer_fixture(vec![ordinary, exact]);
        let limits = PacketBudgetLimitsDto {
            max_anchors: 1,
            max_files: 1,
            max_snippets: 1,
            max_trail_edges: 1,
            max_output_bytes: 1024,
        };

        assert!(cap_citations(&mut answer, &limits));
        assert_eq!(answer.citations.len(), 1);
        assert_eq!(answer.citations[0].display_name, "selected");
    }

    #[test]
    fn same_role_exact_probe_anchors_survive_small_cap_and_role_replacement() {
        let mut first_exact = citation("selected", "src/selected.rs", 1.0);
        first_exact.coverage_role = Some("explicit exact probe".to_string());
        first_exact.eligible_for_sufficiency = Some(false);
        let mut second_exact = citation("selected helper", "src/helper.rs", 0.5);
        second_exact.coverage_role = Some("explicit exact probe".to_string());
        second_exact.eligible_for_sufficiency = Some(false);
        let ordinary_test = citation("selected regression", "tests/selected_test.rs", 500.0);
        let ordinary_dispatch = citation("dispatch command", "src/dispatch.rs", 400.0);
        let mut answer = answer_fixture(vec![
            ordinary_test,
            first_exact,
            ordinary_dispatch,
            second_exact,
        ]);
        let limits = PacketBudgetLimitsDto {
            max_anchors: 2,
            max_files: 2,
            max_snippets: 2,
            max_trail_edges: 2,
            max_output_bytes: 1024,
        };

        assert!(cap_citations(&mut answer, &limits));
        assert_eq!(
            answer
                .citations
                .iter()
                .map(|citation| citation.display_name.as_str())
                .collect::<Vec<_>>(),
            vec!["selected", "selected helper"]
        );
        assert!(
            answer
                .citations
                .iter()
                .all(|citation| citation.coverage_role.as_deref() == Some("explicit exact probe"))
        );
    }

    #[test]
    fn packet_promotion_pipeline_keeps_exact_probes_ahead_of_required_and_focus_anchors() {
        let ordinary_required = citation("dispatch command", "crates/other/src/dispatch.rs", 500.0);
        let focus_root = citation("demo::Cli", "crates/demo/src/cli.rs", 450.0);
        let focus_neighbor = citation("runtime work", "crates/demo/src/runtime.rs", 400.0);
        let mut first_exact = citation("selected", "crates/demo/src/selected.rs", 1.0);
        first_exact.coverage_role = Some("explicit exact probe".to_string());
        first_exact.eligible_for_sufficiency = Some(false);
        let mut second_exact = citation("selected helper", "crates/demo/src/helper.rs", 0.5);
        second_exact.coverage_role = Some("explicit exact probe".to_string());
        second_exact.eligible_for_sufficiency = Some(false);
        let mut answer = answer_fixture(vec![
            ordinary_required,
            focus_root,
            focus_neighbor,
            first_exact,
            second_exact,
        ]);
        let limits = PacketBudgetLimitsDto {
            max_anchors: 2,
            max_files: 2,
            max_snippets: 2,
            max_trail_edges: 2,
            max_output_bytes: 1024,
        };

        assert!(cap_packet_citations(
            &mut answer,
            &limits,
            &["dispatch command".to_string()]
        ));
        assert_eq!(
            answer
                .citations
                .iter()
                .map(|citation| citation.display_name.as_str())
                .collect::<Vec<_>>(),
            vec!["selected", "selected helper"]
        );
        assert!(answer.retrieval_trace.annotations.iter().any(|annotation| {
            annotation.starts_with("packet_required_probe_citations promoted=1")
        }));
        assert!(answer.retrieval_trace.annotations.iter().any(|annotation| {
            annotation.starts_with("packet_focus_neighborhood_citations promoted=")
        }));
    }

    #[test]
    fn multi_match_required_probes_promote_distinct_primary_sources() {
        for (query, first_display, second_display) in [
            (
                "client send implementation",
                "Client send implementation",
                "Client send implementation adapter",
            ),
            (
                "submit prevent default",
                "Submit prevent default guard",
                "Submit prevent default handler",
            ),
            (
                "session request creation",
                "Session request creation",
                "Session request creation builder",
            ),
            (
                "network command input",
                "Network command input",
                "Network command input reader",
            ),
            (
                "source read buffer",
                "RealBufferedSource.read",
                "BufferedSource.readIntoBuffer",
            ),
        ] {
            let mut answer = answer_fixture(vec![
                citation(&format!("{query} guide"), "docs/flow-guide.md", 100.0),
                citation(&format!("{query} test"), "tests/flow_test.rs", 99.0),
                citation(first_display, "src/flow/primary.rs", 4.0),
                citation(second_display, "src/flow/secondary.rs", 3.0),
            ]);

            let protected = promote_required_probe_citations(&mut answer, &[query.to_string()]);
            let protected_paths = answer
                .citations
                .iter()
                .filter(|citation| protected.contains(&packet_citation_key(citation)))
                .filter_map(|citation| citation.file_path.as_deref())
                .collect::<Vec<_>>();

            assert_eq!(
                protected_paths,
                vec!["src/flow/primary.rs", "src/flow/secondary.rs"],
                "query `{query}` should protect two primary-source matches before docs/tests: {protected_paths:?}"
            );
            assert_eq!(
                answer.citations[0].file_path.as_deref(),
                Some("src/flow/primary.rs")
            );
            assert_eq!(
                answer.citations[1].file_path.as_deref(),
                Some("src/flow/secondary.rs")
            );
        }

        let query = "data request validation";
        let mut answer = answer_fixture(vec![
            citation(&format!("{query} guide"), "docs/flow-guide.md", 100.0),
            citation(&format!("{query} test"), "tests/flow_test.rs", 99.0),
            citation("DataRequest.validate", "src/flow/primary.rs", 4.0),
            citation(
                "DataRequest.validationPipeline",
                "src/flow/secondary.rs",
                3.0,
            ),
        ]);

        let protected = promote_required_probe_citations(&mut answer, &[query.to_string()]);
        let protected_paths = answer
            .citations
            .iter()
            .filter(|citation| protected.contains(&packet_citation_key(citation)))
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<HashSet<_>>();

        assert_eq!(
            protected_paths,
            HashSet::from(["src/flow/primary.rs", "src/flow/secondary.rs"]),
            "query `{query}` should protect both primary-source validation matches before docs/tests: {protected_paths:?}"
        );
    }

    #[test]
    fn required_probe_prefers_shared_source_set_over_platform_variant() {
        let mut answer = answer_fixture(vec![
            citation(
                "RealBufferedSource.read",
                "src/jvmMain/kotlin/io/RealBufferedSource.kt",
                100.0,
            ),
            citation(
                "RealBufferedSource.read",
                "src/commonMain/kotlin/io/RealBufferedSource.kt",
                1.0,
            ),
            citation(
                "BufferedSource",
                "src/commonMain/kotlin/io/BufferedSource.kt",
                0.5,
            ),
        ]);

        let protected = promote_required_probe_citations(
            &mut answer,
            &[
                "source read buffer".to_string(),
                "buffered source".to_string(),
            ],
        );
        let protected_paths = answer
            .citations
            .iter()
            .filter(|citation| protected.contains(&packet_citation_key(citation)))
            .filter_map(|citation| citation.file_path.as_deref())
            .collect::<Vec<_>>();

        assert!(
            protected_paths
                .iter()
                .take(2)
                .all(|path| path.contains("commonMain")),
            "generic source probes should protect shared source-set evidence before platform variants: {protected_paths:?}"
        );
    }

    #[test]
    fn request_method_probe_prefers_implementation_over_declaration() {
        let mut declaration = citation("request", "index.d.ts", 100.0);
        declaration.kind = NodeKind::METHOD;
        let mut implementation = citation("Client.request", "src/client/Client.js", 1.0);
        implementation.kind = NodeKind::METHOD;
        let mut answer = answer_fixture(vec![declaration, implementation]);

        promote_required_probe_citations(&mut answer, &["request method".to_string()]);

        assert_eq!(
            answer.citations[0].file_path.as_deref(),
            Some("src/client/Client.js")
        );
    }

    #[test]
    fn source_proven_adapter_file_survives_a_helper_in_the_same_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        let adapter_path = temp.path().join("adapters.js");
        std::fs::write(
            &adapter_path,
            "const knownAdapters = { http, xhr }; function getAdapter(name) { const adapter = knownAdapters[name]; return adapter; }",
        )
        .expect("write adapter source");
        let adapter_path = adapter_path.to_string_lossy();

        let mut helper = citation("isResolvedHandle", &adapter_path, 100.0);
        helper.kind = NodeKind::FUNCTION;
        let mut file = citation(&adapter_path, &adapter_path, 1.0);
        file.kind = NodeKind::FILE;
        let mut answer = answer_fixture(vec![helper, file]);
        let limits = PacketBudgetLimitsDto {
            max_anchors: 1,
            max_files: 1,
            max_snippets: 1,
            max_trail_edges: 1,
            max_output_bytes: 1024,
        };

        assert!(cap_packet_citations(
            &mut answer,
            &limits,
            &["adapters".to_string()]
        ));
        assert_eq!(answer.citations.len(), 1);
        assert_eq!(answer.citations[0].kind, NodeKind::FILE);
        assert_eq!(
            answer.citations[0].file_path.as_deref(),
            Some(&*adapter_path)
        );
    }

    #[test]
    fn packet_citation_capping_large_probe_set_stays_bounded() {
        const CITATION_COUNT: usize = 1_024;
        const MULTI_MATCH_CITATION_COUNT: usize = 256;
        const UNIQUE_REQUIRED_PROBE_COUNT: usize = 16;
        const REQUIRED_PROBE_COUNT: usize = 96;
        const DUPLICATE_MULTI_MATCH_PROBE_COUNT: usize =
            REQUIRED_PROBE_COUNT - UNIQUE_REQUIRED_PROBE_COUNT;

        let mut citations = Vec::with_capacity(CITATION_COUNT);
        for index in 0..CITATION_COUNT {
            let display_name = if index < MULTI_MATCH_CITATION_COUNT {
                format!("RealBufferedSource.read{index}")
            } else if index < MULTI_MATCH_CITATION_COUNT + UNIQUE_REQUIRED_PROBE_COUNT {
                format!("UniqueProbeKey{:03}", index - MULTI_MATCH_CITATION_COUNT)
            } else {
                match index % 8 {
                    0 => format!("Client send implementation {index}"),
                    1 => format!("Submit prevent default guard {index}"),
                    2 => format!("Session request creation {index}"),
                    3 => format!("Network command input {index}"),
                    4 => format!("Input helper {index}"),
                    5 => format!("DataRequest.validate {index}"),
                    6 => format!("Route registration {index}"),
                    _ => format!("Auxiliary evidence {index}"),
                }
            };
            let file_path = if index < MULTI_MATCH_CITATION_COUNT {
                format!("src/flow/source_buffer_{index}.rs")
            } else if index < MULTI_MATCH_CITATION_COUNT + UNIQUE_REQUIRED_PROBE_COUNT {
                format!(
                    "src/flow/synthetic_probe_{}.rs",
                    index - MULTI_MATCH_CITATION_COUNT
                )
            } else if index % 17 == 0 {
                format!("docs/flow-guide-{index}.md")
            } else if index % 13 == 0 {
                format!("tests/flow_{index}_test.rs")
            } else if index % 5 == 0 {
                format!("src/commonMain/kotlin/io/common_{index}.kt")
            } else {
                format!("src/flow/module_{index}.rs")
            };
            citations.push(citation(&display_name, &file_path, index as f32));
        }

        let mut required_probe_queries = (0..UNIQUE_REQUIRED_PROBE_COUNT)
            .map(|index| format!("UniqueProbeKey{index:03}"))
            .collect::<Vec<_>>();
        required_probe_queries.extend(
            (0..DUPLICATE_MULTI_MATCH_PROBE_COUNT).map(|_| "source read buffer".to_string()),
        );
        let limits = PacketBudgetLimitsDto {
            max_anchors: 18,
            max_files: 18,
            max_snippets: 80,
            max_trail_edges: 240,
            max_output_bytes: 512 * 1024,
        };
        let mut answer = answer_fixture(citations);

        let truncated = cap_packet_citations(&mut answer, &limits, &required_probe_queries);

        assert!(
            truncated,
            "large synthetic packet should hit the citation cap"
        );
        assert!(
            answer.citations.len() <= limits.max_anchors as usize,
            "citation cap should stay within max_anchors"
        );
        let leading_paths = answer
            .citations
            .iter()
            .take(UNIQUE_REQUIRED_PROBE_COUNT)
            .filter_map(|citation| citation.file_path.clone())
            .collect::<Vec<_>>();
        let expected_leading_paths = (0..UNIQUE_REQUIRED_PROBE_COUNT)
            .map(|index| format!("src/flow/synthetic_probe_{index}.rs"))
            .collect::<Vec<_>>();
        assert_eq!(
            leading_paths, expected_leading_paths,
            "unique required probes should keep their protected citation order"
        );
        let multi_match_paths = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref())
            .filter(|path| path.contains("source_buffer_"))
            .collect::<Vec<_>>();
        assert_eq!(
            multi_match_paths.len(),
            2,
            "duplicated multi-match source probes should protect exactly two distinct source-buffer citations"
        );
        let kept_files = answer
            .citations
            .iter()
            .filter_map(|citation| citation.file_path.as_deref().map(packet_display_path))
            .collect::<HashSet<_>>();
        assert!(
            kept_files.len() <= limits.max_files as usize,
            "citation cap should stay within max_files"
        );
        assert!(
            answer
                .retrieval_trace
                .annotations
                .iter()
                .any(|annotation| annotation.starts_with("packet_required_probe_citations ")),
            "required-probe promotion should still run on the large synthetic packet"
        );
        let required_probe_annotation = answer
            .retrieval_trace
            .annotations
            .iter()
            .find(|annotation| annotation.starts_with("packet_required_probe_citations "))
            .expect("large guard should record required-probe promotion");
        assert_eq!(
            required_probe_annotation
                .matches("source read buffer")
                .count(),
            1,
            "duplicate required probes should be deduped before promotion"
        );
    }
}
