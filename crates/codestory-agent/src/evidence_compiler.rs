//! Pure repository-derived evidence compilation.
//!
//! The compiler receives only admitted, hydrated repository evidence. It has
//! no access to the original question and therefore cannot encode prompt
//! taxonomies or expected answer stages.

use codestory_contracts::api::{SupportUnitDto, SupportUnitKindDto};
use codestory_contracts::compilation::{
    INTERIM_MAX_ADMITTED_CANDIDATES, PACKET_COMPILATION_CONTRACT_VERSION_V1,
    PUBLIC_PACKET_SERIALIZED_MAX_BYTES, PacketAdmissionGapKindV1, PacketAdmissionOriginV1,
    PacketCompilationInputV1, PacketContinuationSelectorV1, PacketDirectedRelationV1,
    PacketHydratedSourceRangeV1, PacketRelationCertaintyV1, PacketStructuralGapReasonV1,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryDerivedCompilationV1 {
    pub support: Vec<SupportUnitDto>,
    pub continuation: Vec<PacketContinuationSelectorV1>,
}

pub fn compile_repository_evidence(
    input: &PacketCompilationInputV1,
) -> RepositoryDerivedCompilationV1 {
    if input.contract_version != PACKET_COMPILATION_CONTRACT_VERSION_V1 {
        return RepositoryDerivedCompilationV1 {
            support: Vec::new(),
            continuation: Vec::new(),
        };
    }

    let admissions = ordered_admissions(input);
    let admission_order = admissions
        .iter()
        .enumerate()
        .map(|(index, admission)| (admission.stable_identity.as_str(), index))
        .collect::<HashMap<_, _>>();
    let sources = selected_source_ranges(input, &admission_order);
    let relations = selected_relations(input, &admission_order);
    let mut support = Vec::new();
    let mut source_budget_omissions = BTreeSet::new();
    let mut relation_budget_omissions = BTreeSet::new();

    // Weave the path-diverse source order with the directed forest. This keeps
    // exact selectors and first-path witnesses ahead of repeated ranges while
    // preventing sixteen source rows from starving every structural edge.
    let mut source_index = 0;
    let mut relation_index = 0;
    while source_index < sources.len() || relation_index < relations.len() {
        if let Some(source) = sources.get(source_index) {
            if !push_if_within_public_budget(&mut support, source_support_unit(source.witness)) {
                source_budget_omissions.extend(source.represented_identities.iter().cloned());
            }
            source_index += 1;
        }

        if let Some(relation) = relations.get(relation_index) {
            if !push_if_within_public_budget(&mut support, relation_support_unit(relation)) {
                relation_budget_omissions.insert(relation.from_identity.clone());
                relation_budget_omissions.insert(relation.to_identity.clone());
            }
            relation_index += 1;
        }
    }

    let source_identities = support
        .iter()
        .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
        .filter_map(|unit| unit.symbol_id.clone())
        .collect::<BTreeSet<_>>();
    let represented_source_paths = sources
        .iter()
        .flat_map(|source| {
            source
                .represented_identities
                .iter()
                .map(|identity| (identity.as_str(), source.witness.path.as_str()))
        })
        .collect::<HashMap<_, _>>();

    for admission in &admissions {
        if source_identities.contains(admission.stable_identity.as_str()) {
            continue;
        }
        let _ = push_if_within_public_budget(
            &mut support,
            SupportUnitDto {
                id: format!("symbol:{}", admission.stable_identity),
                kind: SupportUnitKindDto::SymbolLocation,
                summary: admission.stable_identity.clone(),
                path: represented_source_paths
                    .get(admission.stable_identity.as_str())
                    .map(|path| (*path).to_string()),
                symbol_id: Some(admission.stable_identity.clone()),
                start_line: None,
                end_line: None,
                snippet: None,
                edge_kind: None,
                from_symbol: None,
                to_symbol: None,
                query: None,
            },
        );
    }

    support.truncate(INTERIM_MAX_ADMITTED_CANDIDATES);
    let continuation = continuation_selectors(
        input,
        &sources,
        &relations,
        &source_budget_omissions,
        &relation_budget_omissions,
        &support,
    );
    RepositoryDerivedCompilationV1 {
        support,
        continuation,
    }
}

fn ordered_admissions(
    input: &PacketCompilationInputV1,
) -> Vec<&codestory_contracts::compilation::PacketAdmissionReceiptV1> {
    let mut admissions = input.admissions.iter().collect::<Vec<_>>();
    admissions.sort_by(|left, right| {
        admission_origin_rank(left.origin)
            .cmp(&admission_origin_rank(right.origin))
            .then_with(|| left.packet_ordinal.cmp(&right.packet_ordinal))
            .then_with(|| left.stable_identity.cmp(&right.stable_identity))
    });
    admissions.dedup_by(|left, right| left.stable_identity == right.stable_identity);
    admissions.truncate(INTERIM_MAX_ADMITTED_CANDIDATES);
    admissions
}

fn admission_origin_rank(origin: PacketAdmissionOriginV1) -> u8 {
    match origin {
        PacketAdmissionOriginV1::ExactTypedSelector => 0,
        PacketAdmissionOriginV1::Retrieval => 1,
    }
}

struct SelectedSourceRange<'a> {
    witness: &'a PacketHydratedSourceRangeV1,
    represented_identities: BTreeSet<String>,
}

fn selected_source_ranges<'a>(
    input: &'a PacketCompilationInputV1,
    admission_order: &HashMap<&str, usize>,
) -> Vec<SelectedSourceRange<'a>> {
    let mut candidates = input
        .sources
        .iter()
        .filter(|source| {
            admission_order.contains_key(source.stable_identity.as_str())
                && source.start_line > 0
                && source.end_line >= source.start_line
                && !source.source.trim().is_empty()
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        admission_order[left.stable_identity.as_str()]
            .cmp(&admission_order[right.stable_identity.as_str()])
            .then_with(|| {
                parser_completeness_rank(left.parser_completeness)
                    .cmp(&parser_completeness_rank(right.parser_completeness))
            })
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.start_line.cmp(&right.start_line))
            .then_with(|| right.end_line.cmp(&left.end_line))
            .then_with(|| left.symbol.cmp(&right.symbol))
            .then_with(|| left.source.cmp(&right.source))
    });

    // One containment group emits one source range. The highest-priority
    // admitted identity owns that source row; lower-priority identities remain
    // recorded so the caller can retain them as symbol locations instead of
    // silently erasing them during deduplication.
    let mut deduped: Vec<SelectedSourceRange<'a>> = Vec::new();
    for candidate in candidates {
        let overlapping = deduped.iter().position(|kept| {
            kept.witness.path == candidate.path
                && ((kept.witness.start_line <= candidate.start_line
                    && kept.witness.end_line >= candidate.end_line)
                    || (candidate.start_line <= kept.witness.start_line
                        && candidate.end_line >= kept.witness.end_line))
        });
        if let Some(index) = overlapping {
            deduped[index]
                .represented_identities
                .insert(candidate.stable_identity.clone());
            continue;
        }
        deduped.push(SelectedSourceRange {
            witness: candidate,
            represented_identities: BTreeSet::from([candidate.stable_identity.clone()]),
        });
    }

    let exact_identities = input
        .admissions
        .iter()
        .filter(|admission| admission.origin == PacketAdmissionOriginV1::ExactTypedSelector)
        .map(|admission| admission.stable_identity.as_str())
        .collect::<BTreeSet<_>>();
    let (exact, retrieval): (Vec<_>, Vec<_>) = deduped
        .into_iter()
        .partition(|source| exact_identities.contains(source.witness.stable_identity.as_str()));
    let mut ordered = Vec::new();
    let mut seen_paths = BTreeSet::new();
    extend_path_diverse(exact, &mut seen_paths, &mut ordered);
    extend_path_diverse(retrieval, &mut seen_paths, &mut ordered);
    ordered
}

fn extend_path_diverse<'a>(
    sources: Vec<SelectedSourceRange<'a>>,
    seen_paths: &mut BTreeSet<String>,
    ordered: &mut Vec<SelectedSourceRange<'a>>,
) {
    let mut repeated = Vec::new();
    for source in sources {
        if seen_paths.insert(source.witness.path.clone()) {
            ordered.push(source);
        } else {
            repeated.push(source);
        }
    }
    ordered.extend(repeated);
}

fn parser_completeness_rank(
    completeness: codestory_contracts::compilation::PacketParserCompletenessV1,
) -> u8 {
    use codestory_contracts::compilation::PacketParserCompletenessV1;
    match completeness {
        PacketParserCompletenessV1::Complete => 0,
        PacketParserCompletenessV1::Partial => 1,
        PacketParserCompletenessV1::Unknown => 2,
    }
}

fn selected_relations<'a>(
    input: &'a PacketCompilationInputV1,
    admission_order: &HashMap<&str, usize>,
) -> Vec<&'a PacketDirectedRelationV1> {
    let mut certain = input
        .relations
        .iter()
        .filter(|relation| relation.certainty == PacketRelationCertaintyV1::Certain)
        .filter(|relation| {
            relation.relation_kind
                != codestory_contracts::compilation::PacketRelationKindV1::Unknown
        })
        .filter(|relation| {
            admission_order.contains_key(relation.from_identity.as_str())
                && admission_order.contains_key(relation.to_identity.as_str())
        })
        .collect::<Vec<_>>();
    certain.sort_by(|left, right| {
        relation_priority(left, admission_order)
            .cmp(&relation_priority(right, admission_order))
            .then_with(|| left.relation_id.cmp(&right.relation_id))
    });

    let mut forest = Vec::new();
    let mut incident_candidates = Vec::new();
    let mut connected = BTreeSet::new();
    let mut components = admission_order
        .keys()
        .map(|identity| ((*identity).to_string(), (*identity).to_string()))
        .collect::<BTreeMap<_, _>>();
    for relation in certain {
        let from_admitted = admission_order.contains_key(relation.from_identity.as_str());
        let to_admitted = admission_order.contains_key(relation.to_identity.as_str());
        if from_admitted && to_admitted {
            let from_root = component_root(&components, &relation.from_identity);
            let to_root = component_root(&components, &relation.to_identity);
            if from_root != to_root {
                for root in components.values_mut() {
                    if *root == to_root {
                        *root = from_root.clone();
                    }
                }
                connected.insert(relation.from_identity.clone());
                connected.insert(relation.to_identity.clone());
                forest.push(relation);
                continue;
            }
        }
        incident_candidates.push(relation);
    }

    // A seed that the admitted-seed forest could not connect may contribute
    // one certain incident relation. This keeps graph context broad across
    // seeds instead of spending the remaining packet on one dense node.
    for relation in incident_candidates {
        let admitted_endpoints = [
            relation.from_identity.as_str(),
            relation.to_identity.as_str(),
        ]
        .into_iter()
        .filter(|identity| admission_order.contains_key(*identity))
        .collect::<Vec<_>>();
        if admitted_endpoints
            .iter()
            .all(|identity| connected.contains(*identity))
        {
            continue;
        }
        connected.extend(admitted_endpoints.into_iter().map(ToOwned::to_owned));
        forest.push(relation);
    }
    forest
}

fn relation_priority(
    relation: &PacketDirectedRelationV1,
    admission_order: &HashMap<&str, usize>,
) -> (u8, usize, usize) {
    let from = admission_order
        .get(relation.from_identity.as_str())
        .copied()
        .unwrap_or(usize::MAX);
    let to = admission_order
        .get(relation.to_identity.as_str())
        .copied()
        .unwrap_or(usize::MAX);
    (
        u8::from(from == usize::MAX || to == usize::MAX),
        from.max(to),
        from.min(to),
    )
}

fn component_root(components: &BTreeMap<String, String>, identity: &str) -> String {
    components
        .get(identity)
        .cloned()
        .unwrap_or_else(|| identity.to_string())
}

fn source_support_unit(source: &PacketHydratedSourceRangeV1) -> SupportUnitDto {
    SupportUnitDto {
        id: format!(
            "source:{}:{}:{}",
            source.stable_identity, source.start_line, source.end_line
        ),
        kind: SupportUnitKindDto::SourceRange,
        summary: source
            .symbol
            .clone()
            .unwrap_or_else(|| source.stable_identity.clone()),
        path: Some(source.path.clone()),
        symbol_id: Some(source.stable_identity.clone()),
        start_line: Some(source.start_line),
        end_line: Some(source.end_line),
        snippet: Some(source.source.clone()),
        edge_kind: None,
        from_symbol: None,
        to_symbol: None,
        query: None,
    }
}

fn relation_support_unit(relation: &PacketDirectedRelationV1) -> SupportUnitDto {
    SupportUnitDto {
        id: format!("edge:{}", relation.relation_id),
        kind: SupportUnitKindDto::TypedGraphEdge,
        summary: format!(
            "`{}` {} `{}`",
            relation.from_identity,
            relation.relation_kind.as_str(),
            relation.to_identity
        ),
        path: None,
        symbol_id: Some(relation.from_identity.clone()),
        start_line: None,
        end_line: None,
        snippet: None,
        edge_kind: Some(relation.relation_kind.as_str().to_string()),
        from_symbol: Some(relation.from_identity.clone()),
        to_symbol: Some(relation.to_identity.clone()),
        query: None,
    }
}

fn push_if_within_public_budget(
    support: &mut Vec<SupportUnitDto>,
    candidate: SupportUnitDto,
) -> bool {
    if support.len() >= INTERIM_MAX_ADMITTED_CANDIDATES {
        return false;
    }
    support.push(candidate);
    let within_budget = serde_json::to_vec(support)
        .map(|serialized| serialized.len() <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES)
        .unwrap_or(false);
    if !within_budget {
        support.pop();
        return false;
    }
    true
}

fn continuation_selectors(
    input: &PacketCompilationInputV1,
    sources: &[SelectedSourceRange<'_>],
    relations: &[&PacketDirectedRelationV1],
    source_budget_omissions: &BTreeSet<String>,
    relation_budget_omissions: &BTreeSet<String>,
    support: &[SupportUnitDto],
) -> Vec<PacketContinuationSelectorV1> {
    let mut selectors = Vec::new();
    let mut identities_with_explicit_gaps = BTreeSet::new();
    for gap in &input.admission_gaps {
        let Some(stable_identity) = gap.stable_identity.clone() else {
            continue;
        };
        identities_with_explicit_gaps.insert(stable_identity.clone());
        let reason = match gap.kind {
            PacketAdmissionGapKindV1::CandidateCountExceeded => {
                PacketStructuralGapReasonV1::CandidateCountExceeded
            }
            PacketAdmissionGapKindV1::SourceBudgetExceeded => {
                PacketStructuralGapReasonV1::SourceBudgetExceeded
            }
            PacketAdmissionGapKindV1::StableIdentityMissing
            | PacketAdmissionGapKindV1::SourceBoundMissing
            | PacketAdmissionGapKindV1::SourceUnavailable => {
                PacketStructuralGapReasonV1::SourceUnavailable
            }
        };
        if let Some(selector) = continuation_selector(stable_identity, reason) {
            selectors.push(selector);
        }
    }
    for ambiguity in &input.ambiguities {
        for stable_identity in &ambiguity.candidate_identities {
            if let Some(selector) = continuation_selector(
                stable_identity.clone(),
                PacketStructuralGapReasonV1::AmbiguousSelector,
            ) {
                selectors.push(selector);
            }
        }
    }

    let source_identities = sources
        .iter()
        .flat_map(|source| source.represented_identities.iter().cloned())
        .collect::<BTreeSet<_>>();
    let incident_identities = relations
        .iter()
        .flat_map(|relation| [relation.from_identity.clone(), relation.to_identity.clone()])
        .collect::<BTreeSet<_>>();
    let represented_identities = support
        .iter()
        .flat_map(|unit| {
            let mut identities = Vec::new();
            if let Some(identity) = unit.symbol_id.clone() {
                identities.push(identity);
            }
            if let Some(identity) = unit.from_symbol.clone() {
                identities.push(identity);
            }
            if let Some(identity) = unit.to_symbol.clone() {
                identities.push(identity);
            }
            identities
        })
        .collect::<BTreeSet<_>>();

    for admission in ordered_admissions(input) {
        let identity = &admission.stable_identity;
        if identities_with_explicit_gaps.contains(identity) {
            continue;
        }
        let reason = if source_budget_omissions.contains(identity) {
            Some(PacketStructuralGapReasonV1::SourceBudgetExceeded)
        } else if relation_budget_omissions.contains(identity)
            || !represented_identities.contains(identity)
            || (!source_identities.contains(identity) && !incident_identities.contains(identity))
        {
            Some(PacketStructuralGapReasonV1::DisconnectedSeed)
        } else {
            None
        };
        if let Some(reason) = reason
            && let Some(selector) = continuation_selector(identity.clone(), reason)
        {
            selectors.push(selector);
        }
    }

    selectors.sort_by(|left, right| {
        left.stable_identity
            .cmp(&right.stable_identity)
            .then_with(|| structural_gap_rank(left.reason).cmp(&structural_gap_rank(right.reason)))
    });
    selectors.dedup_by(|left, right| {
        left.stable_identity == right.stable_identity && left.reason == right.reason
    });
    selectors
}

fn structural_gap_rank(reason: PacketStructuralGapReasonV1) -> u8 {
    match reason {
        PacketStructuralGapReasonV1::CandidateCountExceeded => 0,
        PacketStructuralGapReasonV1::SourceBudgetExceeded => 1,
        PacketStructuralGapReasonV1::SourceUnavailable => 2,
        PacketStructuralGapReasonV1::AmbiguousSelector => 3,
        PacketStructuralGapReasonV1::DisconnectedSeed => 4,
    }
}

fn continuation_selector(
    stable_identity: String,
    reason: PacketStructuralGapReasonV1,
) -> Option<PacketContinuationSelectorV1> {
    if let Some(path) = stable_identity.strip_prefix("path:") {
        return Some(PacketContinuationSelectorV1 {
            stable_identity: stable_identity.clone(),
            path: Some(path.to_string()),
            symbol_id: None,
            reason,
        });
    }
    let symbol_id = stable_identity.strip_prefix("node:")?;
    Some(PacketContinuationSelectorV1 {
        stable_identity: stable_identity.clone(),
        path: None,
        symbol_id: Some(symbol_id.to_string()),
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::compilation::{
        PacketAdmissionReceiptV1, PacketCompilationPublicationV1, PacketParserCompletenessV1,
        PacketRelationKindV1,
    };

    fn input() -> PacketCompilationInputV1 {
        PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project".into(),
                core_generation_id: "core".into(),
                retrieval_generation: Some("retrieval".into()),
            },
            admissions: vec![
                PacketAdmissionReceiptV1 {
                    packet_ordinal: 1,
                    stable_identity: "retrieved".into(),
                    score_version: "v1".into(),
                    reserved_source_bytes: 128,
                    origin: PacketAdmissionOriginV1::Retrieval,
                },
                PacketAdmissionReceiptV1 {
                    packet_ordinal: 0,
                    stable_identity: "exact".into(),
                    score_version: "v1".into(),
                    reserved_source_bytes: 128,
                    origin: PacketAdmissionOriginV1::ExactTypedSelector,
                },
            ],
            sources: vec![
                PacketHydratedSourceRangeV1 {
                    stable_identity: "retrieved".into(),
                    path: "src/shared.rs".into(),
                    symbol: Some("retrieved".into()),
                    start_line: 5,
                    end_line: 8,
                    source: "fn retrieved() {}".into(),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                },
                PacketHydratedSourceRangeV1 {
                    stable_identity: "exact".into(),
                    path: "src/exact.rs".into(),
                    symbol: Some("exact".into()),
                    start_line: 1,
                    end_line: 10,
                    source: "fn exact() {}".into(),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                },
                PacketHydratedSourceRangeV1 {
                    stable_identity: "exact".into(),
                    path: "src/exact.rs".into(),
                    symbol: Some("exact".into()),
                    start_line: 3,
                    end_line: 4,
                    source: "nested".into(),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                },
            ],
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        }
    }

    #[test]
    fn exact_sources_lead_and_contained_ranges_are_deduplicated() {
        let product = compile_repository_evidence(&input());
        assert_eq!(product.support[0].symbol_id.as_deref(), Some("exact"));
        assert_eq!(
            product
                .support
                .iter()
                .filter(|unit| unit.path.as_deref() == Some("src/exact.rs"))
                .count(),
            1
        );
    }

    #[test]
    fn only_certain_relations_enter_the_product() {
        let mut input = input();
        input.relations = vec![
            PacketDirectedRelationV1 {
                relation_id: "certain".into(),
                from_identity: "exact".into(),
                to_identity: "retrieved".into(),
                relation_kind: PacketRelationKindV1::Call,
                certainty: PacketRelationCertaintyV1::Certain,
            },
            PacketDirectedRelationV1 {
                relation_id: "uncertain".into(),
                from_identity: "exact".into(),
                to_identity: "retrieved".into(),
                relation_kind: PacketRelationKindV1::Call,
                certainty: PacketRelationCertaintyV1::Uncertain,
            },
        ];
        let product = compile_repository_evidence(&input);
        assert!(product.support.iter().any(|unit| unit.id == "edge:certain"));
        assert!(
            !product
                .support
                .iter()
                .any(|unit| unit.id == "edge:uncertain")
        );
    }

    #[test]
    fn connecting_forest_precedes_symbol_only_fallbacks() {
        let mut input = input();
        input
            .sources
            .retain(|source| source.stable_identity == "exact");
        input.relations = vec![PacketDirectedRelationV1 {
            relation_id: "connects-seeds".into(),
            from_identity: "exact".into(),
            to_identity: "retrieved".into(),
            relation_kind: PacketRelationKindV1::Call,
            certainty: PacketRelationCertaintyV1::Certain,
        }];

        let product = compile_repository_evidence(&input);
        let relation_index = product
            .support
            .iter()
            .position(|unit| unit.id == "edge:connects-seeds")
            .expect("connecting relation");
        let fallback_index = product
            .support
            .iter()
            .position(|unit| unit.id == "symbol:retrieved")
            .expect("symbol fallback");
        assert!(relation_index < fallback_index);
    }

    #[test]
    fn parser_complete_source_wins_identical_range_ties() {
        let mut input = input();
        input.sources.insert(
            0,
            PacketHydratedSourceRangeV1 {
                stable_identity: "retrieved".into(),
                path: "src/shared.rs".into(),
                symbol: Some("retrieved".into()),
                start_line: 5,
                end_line: 8,
                source: "partial witness".into(),
                parser_completeness: PacketParserCompletenessV1::Partial,
            },
        );

        let product = compile_repository_evidence(&input);
        let retained = product
            .support
            .iter()
            .find(|unit| unit.path.as_deref() == Some("src/shared.rs"))
            .expect("shared source witness");
        assert_eq!(retained.snippet.as_deref(), Some("fn retrieved() {}"));
    }

    #[test]
    fn relations_to_unadmitted_endpoints_never_enter_the_product() {
        let mut input = input();
        input.sources.clear();
        input.relations = vec![
            PacketDirectedRelationV1 {
                relation_id: "first-incident".into(),
                from_identity: "retrieved".into(),
                to_identity: "external-a".into(),
                relation_kind: PacketRelationKindV1::Usage,
                certainty: PacketRelationCertaintyV1::Certain,
            },
            PacketDirectedRelationV1 {
                relation_id: "second-incident".into(),
                from_identity: "retrieved".into(),
                to_identity: "external-b".into(),
                relation_kind: PacketRelationKindV1::Usage,
                certainty: PacketRelationCertaintyV1::Certain,
            },
        ];

        let product = compile_repository_evidence(&input);
        let relation_ids = product
            .support
            .iter()
            .filter(|unit| unit.kind == SupportUnitKindDto::TypedGraphEdge)
            .map(|unit| unit.id.as_str())
            .collect::<Vec<_>>();
        assert!(relation_ids.is_empty());
    }

    #[test]
    fn pure_input_makes_prompt_paraphrases_observationally_irrelevant() {
        let frozen = input();
        let first = compile_repository_evidence(&frozen);
        let second = compile_repository_evidence(&frozen);
        assert_eq!(first, second);
        assert!(
            serde_json::to_value(frozen)
                .unwrap()
                .get("question")
                .is_none()
        );
    }
}
