//! Pure repository-derived evidence compilation.
//!
//! The compiler receives only admitted, hydrated repository evidence. It has
//! no access to the original question and therefore cannot encode prompt
//! taxonomies or expected answer stages.

use codestory_contracts::api::{SupportUnitDto, SupportUnitKindDto};
use codestory_contracts::compilation::{
    INTERIM_MAX_ADMITTED_CANDIDATES, INTERIM_SOURCE_ROW_UPPER_BOUND,
    PACKET_COMPILATION_CONTRACT_VERSION_V1, PUBLIC_PACKET_SERIALIZED_MAX_BYTES,
    PacketAdmissionGapKindV1, PacketAdmissionOriginV1, PacketCompilationInputV1,
    PacketContinuationSelectorV1, PacketDirectedRelationV1, PacketHydratedSourceRangeV1,
    PacketRelationCertaintyV1, PacketStructuralGapReasonV1,
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
    compile_repository_evidence_with_row_limit(input, INTERIM_MAX_ADMITTED_CANDIDATES)
}

/// A proposed compilation is accepted only when its complete public
/// representation fits. `BudgetExceeded` preserves the existing full product
/// for the ordinary finalizer's typed refusal; it never asserts availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepresentationBudgetSelectionV1 {
    Complete {
        product: RepositoryDerivedCompilationV1,
        representation_bytes: usize,
        measured_candidates: usize,
    },
    BudgetExceeded {
        product: RepositoryDerivedCompilationV1,
        measured_candidates: usize,
    },
}

/// Select whole authenticated rows before freezing delivery. The caller's
/// pure measurement seam must project BOTH support and continuation, reserve
/// the full negotiated envelope, and return None when compaction yields a
/// budget-refusal variant rather than an admissible Complete candidate.
/// It must not perform retrieval, hydration, capability minting, or retries.
///
/// There are at most seventeen proposals over already hydrated input. Do not
/// binary-search: omitting a page changes gaps and continuation, so final byte
/// cost need not be monotone in the number of retained rows.
pub fn compile_repository_evidence_for_representation<E>(
    input: &PacketCompilationInputV1,
    mut measure_complete: impl FnMut(&RepositoryDerivedCompilationV1) -> Result<Option<usize>, E>,
) -> Result<RepresentationBudgetSelectionV1, E> {
    let full = compile_repository_evidence(input);
    let mut measured_candidates = 0;
    for row_limit in (0..=full.support.len()).rev() {
        let candidate = if row_limit == full.support.len() {
            full.clone()
        } else {
            compile_repository_evidence_with_row_limit(input, row_limit)
        };
        measured_candidates += 1;
        if let Some(representation_bytes) = measure_complete(&candidate)?
            && representation_bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES
        {
            return Ok(RepresentationBudgetSelectionV1::Complete {
                product: candidate,
                representation_bytes,
                measured_candidates,
            });
        }
    }
    Ok(RepresentationBudgetSelectionV1::BudgetExceeded {
        product: full,
        measured_candidates,
    })
}

fn compile_repository_evidence_with_row_limit(
    input: &PacketCompilationInputV1,
    row_limit: usize,
) -> RepositoryDerivedCompilationV1 {
    let row_limit = row_limit.min(INTERIM_MAX_ADMITTED_CANDIDATES);
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
    let mut represented_identities = BTreeSet::new();
    let mut retained_source_identities = BTreeSet::new();
    let mut retained_source_pages = Vec::new();

    // Weave the path-diverse source order with the directed forest. This keeps
    // exact selectors and first-path witnesses ahead of repeated ranges while
    // preventing sixteen source rows from starving every structural edge.
    let mut source_index = 0;
    let mut relation_index = 0;
    while source_index < sources.len() || relation_index < relations.len() {
        if let Some(source) = sources.get(source_index) {
            if push_if_within_public_budget(
                &mut support,
                source_support_unit(source.witness),
                row_limit,
            ) {
                retained_source_pages.push(source.witness);
                represented_identities.insert(source.witness.stable_identity.clone());
                retained_source_identities.extend(source.represented_identities.iter().cloned());
            } else {
                source_budget_omissions.extend(source.represented_identities.iter().cloned());
            }
            source_index += 1;
        }

        if let Some(relation) = relations.get(relation_index) {
            if push_if_within_public_budget(
                &mut support,
                relation_support_unit(relation),
                row_limit,
            ) {
                represented_identities.insert(relation.from_identity.clone());
                represented_identities.insert(relation.to_identity.clone());
            } else {
                relation_budget_omissions.insert(relation.from_identity.clone());
                relation_budget_omissions.insert(relation.to_identity.clone());
            }
            relation_index += 1;
        }
    }

    let source_identities = sources
        .iter()
        .filter(|source| represented_identities.contains(&source.witness.stable_identity))
        .map(|source| source.witness.stable_identity.as_str())
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
        let retained = push_if_within_public_budget(
            &mut support,
            SupportUnitDto {
                source_delivery: None,
                id: format!("symbol:{}", admission.stable_identity),
                kind: SupportUnitKindDto::SymbolLocation,
                summary: format!(
                    "Navigation only: no bounded source range for {}",
                    admission.stable_identity
                ),
                path: admission
                    .stable_identity
                    .strip_prefix("path:")
                    .map(ToOwned::to_owned)
                    .or_else(|| {
                        represented_source_paths
                            .get(admission.stable_identity.as_str())
                            .map(|path| (*path).to_string())
                    }),
                symbol_id: public_symbol_id(&admission.stable_identity),
                start_line: None,
                end_line: None,
                snippet: None,
                edge_kind: None,
                from_symbol: None,
                to_symbol: None,
                query: None,
            },
            row_limit,
        );
        if retained {
            represented_identities.insert(admission.stable_identity.clone());
        }
    }

    support.truncate(row_limit);
    let continuation = continuation_selectors(
        input,
        &sources,
        &relations,
        &source_budget_omissions,
        &relation_budget_omissions,
        &represented_identities,
        &retained_source_identities,
        &retained_source_pages,
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
                && source.delivery.as_ref().is_none_or(|delivery| {
                    delivery.matches_delivery(source.start_line, source.end_line)
                })
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
                && (if kept.witness.delivery.is_some() || candidate.delivery.is_some() {
                    kept.witness.delivery == candidate.delivery
                        && kept.witness.start_line == candidate.start_line
                        && kept.witness.end_line == candidate.end_line
                        && kept.witness.source == candidate.source
                } else {
                    true
                })
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
        .filter(|relation| relation.from_identity != relation.to_identity)
        .collect::<Vec<_>>();
    certain.sort_by(|left, right| {
        relation_priority(left, admission_order)
            .cmp(&relation_priority(right, admission_order))
            .then_with(|| left.relation_kind.cmp(&right.relation_kind))
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
) -> (u8, usize, usize, usize, usize) {
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
        from,
        to,
    )
}

fn component_root(components: &BTreeMap<String, String>, identity: &str) -> String {
    components
        .get(identity)
        .cloned()
        .unwrap_or_else(|| identity.to_string())
}

fn source_support_unit(source: &PacketHydratedSourceRangeV1) -> SupportUnitDto {
    let mut summary = source
        .symbol
        .clone()
        .unwrap_or_else(|| source.stable_identity.clone());
    if let Some(delivery) = &source.delivery
        && source.end_line < delivery.requested_range.end_line
    {
        summary.push_str(&format!(
            " (requested lines {}-{}; complete lines {}-{} delivered; unread lines {}-{})",
            delivery.requested_range.start_line,
            delivery.requested_range.end_line,
            source.start_line,
            source.end_line,
            source.end_line + 1,
            delivery.requested_range.end_line,
        ));
    }
    SupportUnitDto {
        id: format!(
            "source:{}:{}:{}",
            source.stable_identity, source.start_line, source.end_line
        ),
        kind: SupportUnitKindDto::SourceRange,
        source_delivery: source.delivery.clone(),
        summary,
        path: Some(source.path.clone()),
        symbol_id: public_symbol_id(&source.stable_identity),
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
        source_delivery: None,
        id: format!("edge:{}", relation.relation_id),
        kind: SupportUnitKindDto::TypedGraphEdge,
        summary: format!(
            "`{}` {} `{}`",
            relation.from_identity,
            relation.relation_kind.as_str(),
            relation.to_identity
        ),
        path: None,
        symbol_id: public_symbol_id(&relation.from_identity),
        start_line: None,
        end_line: None,
        snippet: None,
        edge_kind: Some(relation.relation_kind.as_str().to_string()),
        from_symbol: public_symbol_id(&relation.from_identity),
        to_symbol: public_symbol_id(&relation.to_identity),
        query: None,
    }
}

fn push_if_within_public_budget(
    support: &mut Vec<SupportUnitDto>,
    candidate: SupportUnitDto,
    row_limit: usize,
) -> bool {
    if support.len() >= row_limit {
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

fn public_symbol_id(stable_identity: &str) -> Option<String> {
    stable_identity.strip_prefix("node:").map(ToOwned::to_owned)
}

fn source_page_was_retained(
    source: &PacketHydratedSourceRangeV1,
    pages: &[&PacketHydratedSourceRangeV1],
) -> bool {
    pages.iter().any(|page| {
        page.path == source.path
            && page.start_line == source.start_line
            && page.end_line == source.end_line
            && page.source == source.source
            && page.delivery == source.delivery
    })
}

fn continuation_selectors(
    input: &PacketCompilationInputV1,
    sources: &[SelectedSourceRange<'_>],
    relations: &[&PacketDirectedRelationV1],
    source_budget_omissions: &BTreeSet<String>,
    relation_budget_omissions: &BTreeSet<String>,
    represented_identities: &BTreeSet<String>,
    retained_source_identities: &BTreeSet<String>,
    retained_source_pages: &[&PacketHydratedSourceRangeV1],
) -> Vec<PacketContinuationSelectorV1> {
    let mut selectors = Vec::new();
    let mut identities_with_explicit_gaps = BTreeSet::new();
    let mut omitted_page_cursors = BTreeMap::<String, Vec<_>>::new();
    for selected in sources {
        let source = selected.witness;
        if source_page_was_retained(source, retained_source_pages) {
            continue;
        }
        let Some(delivery) = &source.delivery else {
            continue;
        };
        for identity in &selected.represented_identities {
            if source_budget_omissions.contains(identity) {
                omitted_page_cursors
                    .entry(identity.clone())
                    .or_default()
                    .push(delivery.requested_range);
            }
        }
    }
    let terminal_source_identities = input
        .admission_gaps
        .iter()
        .filter_map(|gap| {
            let identity = gap.stable_identity.as_ref()?;
            let terminal = match gap.kind {
                PacketAdmissionGapKindV1::SourceUnavailable => true,
                PacketAdmissionGapKindV1::SourceBudgetExceeded => {
                    input.admissions.iter().any(|admission| {
                        &admission.stable_identity == identity
                            && admission.reserved_source_bytes as usize
                                >= INTERIM_SOURCE_ROW_UPPER_BOUND
                    })
                }
                _ => false,
            };
            (terminal
                && !sources
                    .iter()
                    .any(|source| source.represented_identities.contains(identity)))
            .then(|| identity.clone())
        })
        .chain(
            input
                .sources
                .iter()
                .filter(|source| {
                    source.delivery.as_ref().is_some_and(|delivery| {
                        delivery.matches_delivery(source.start_line, source.end_line)
                            && delivery.blocked_line_rendered_bytes.is_some()
                            && source_page_was_retained(source, retained_source_pages)
                    })
                })
                .map(|source| source.stable_identity.clone()),
        )
        .collect::<BTreeSet<_>>();

    let mut ordered_gaps = input.admission_gaps.iter().enumerate().collect::<Vec<_>>();
    ordered_gaps.sort_by(|(left_index, left), (right_index, right)| {
        left.exact_selector_ordinal
            .unwrap_or(u32::MAX)
            .cmp(&right.exact_selector_ordinal.unwrap_or(u32::MAX))
            .then_with(|| left_index.cmp(right_index))
    });
    for (_, gap) in ordered_gaps {
        let Some(stable_identity) = gap.stable_identity.clone() else {
            continue;
        };
        identities_with_explicit_gaps.insert(stable_identity.clone());
        // Failed pinned hydration and exhausted rows cannot progress by
        // offering the same read. Keep recoverable pre-admission omissions
        // and authenticated page cursors for other identities.
        if terminal_source_identities.contains(&stable_identity) {
            continue;
        }
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
        if matches!(
            gap.kind,
            PacketAdmissionGapKindV1::SourceUnavailable
                | PacketAdmissionGapKindV1::SourceBudgetExceeded
        ) && let Some(ranges) = omitted_page_cursors.get(&stable_identity)
        {
            // A page that never reached support retries its original bounds.
            // Preserve this gap's reason without restarting at node defaults.
            for range in ranges {
                if let Some(mut selector) = continuation_selector(stable_identity.clone(), reason) {
                    selector.source_range = Some(*range);
                    selectors.push(selector);
                }
            }
        } else if let Some(selector) = continuation_selector(stable_identity, reason) {
            selectors.push(selector);
        }
    }
    for ambiguity in &input.ambiguities {
        for stable_identity in &ambiguity.candidate_identities {
            if terminal_source_identities.contains(stable_identity) {
                continue;
            }
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
    for admission in ordered_admissions(input) {
        let identity = &admission.stable_identity;
        // A retained complete-line page leaves an explicit unread tail.
        // Omitted source rows retry the requested start rather than skipping
        // bytes that never reached the public packet.
        for source in input
            .sources
            .iter()
            .filter(|source| &source.stable_identity == identity)
        {
            if let Some(delivery) = &source.delivery
                && delivery.matches_delivery(source.start_line, source.end_line)
                && let Some(next) = delivery.next_start_line
                && source_page_was_retained(source, retained_source_pages)
                && let Some(mut selector) = continuation_selector(
                    identity.clone(),
                    PacketStructuralGapReasonV1::SourceBudgetExceeded,
                )
            {
                selector.source_range =
                    Some(codestory_contracts::compilation::PacketSourceRangeV1 {
                        start_line: next,
                        end_line: delivery.requested_range.end_line,
                    });
                selectors.push(selector);
            }
        }
        let mut omitted_page_cursor = false;
        if let Some(ranges) = omitted_page_cursors.get(identity) {
            for range in ranges {
                if let Some(mut selector) = continuation_selector(
                    identity.clone(),
                    PacketStructuralGapReasonV1::SourceBudgetExceeded,
                ) {
                    selector.source_range = Some(*range);
                    selectors.push(selector);
                    omitted_page_cursor = true;
                }
            }
        }
        if identities_with_explicit_gaps.contains(identity) {
            continue;
        }
        if terminal_source_identities.contains(identity) {
            continue;
        }
        let reason = if source_budget_omissions.contains(identity) {
            (!omitted_page_cursor).then_some(PacketStructuralGapReasonV1::SourceBudgetExceeded)
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

    let mut seen = Vec::new();
    selectors.retain(|selector| {
        let key = (
            selector.stable_identity.clone(),
            selector.reason,
            selector.source_range,
        );
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    // Prefer identities with no delivered source while preserving every
    // structural reason and the existing order within each source group.
    // A delivered source does not resolve an omitted incident relation.
    selectors
        .sort_by_key(|selector| retained_source_identities.contains(&selector.stable_identity));
    selectors
}

fn continuation_selector(
    stable_identity: String,
    reason: PacketStructuralGapReasonV1,
) -> Option<PacketContinuationSelectorV1> {
    if let Some(path) = stable_identity.strip_prefix("path:") {
        return Some(PacketContinuationSelectorV1 {
            source_range: None,
            stable_identity: stable_identity.clone(),
            path: Some(path.to_string()),
            symbol_id: None,
            reason,
        });
    }
    let symbol_id = stable_identity.strip_prefix("node:")?;
    Some(PacketContinuationSelectorV1 {
        source_range: None,
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
                    stable_identity: "node:retrieved".into(),
                    score_version: "v1".into(),
                    reserved_source_bytes: 128,
                    origin: PacketAdmissionOriginV1::Retrieval,
                },
                PacketAdmissionReceiptV1 {
                    packet_ordinal: 0,
                    stable_identity: "node:exact".into(),
                    score_version: "v1".into(),
                    reserved_source_bytes: 128,
                    origin: PacketAdmissionOriginV1::ExactTypedSelector,
                },
            ],
            sources: vec![
                PacketHydratedSourceRangeV1 {
                    stable_identity: "node:retrieved".into(),
                    path: "src/shared.rs".into(),
                    symbol: Some("retrieved".into()),
                    start_line: 5,
                    end_line: 8,
                    source: "fn retrieved() {}".into(),
                    delivery: None,
                    parser_completeness: PacketParserCompletenessV1::Complete,
                },
                PacketHydratedSourceRangeV1 {
                    stable_identity: "node:exact".into(),
                    path: "src/exact.rs".into(),
                    symbol: Some("exact".into()),
                    start_line: 1,
                    end_line: 10,
                    source: "fn exact() {}".into(),
                    delivery: None,
                    parser_completeness: PacketParserCompletenessV1::Complete,
                },
                PacketHydratedSourceRangeV1 {
                    stable_identity: "node:exact".into(),
                    path: "src/exact.rs".into(),
                    symbol: Some("exact".into()),
                    start_line: 3,
                    end_line: 4,
                    source: "nested".into(),
                    delivery: None,
                    parser_completeness: PacketParserCompletenessV1::Complete,
                },
            ],
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        }
    }

    fn measured_page_input() -> PacketCompilationInputV1 {
        use codestory_contracts::compilation::{PacketSourceDeliveryV1, PacketSourceRangeV1};
        let mut value = input();
        value.sources.truncate(2);
        for source in &mut value.sources {
            source.start_line = 1;
            source.end_line = 1;
            source.source = "```text\n>    1 | fn body() {}\n```".into();
            source.delivery = Some(PacketSourceDeliveryV1 {
                requested_range: PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: 3,
                },
                source_sha256: "a".repeat(64),
                remaining_source_bytes: 36,
                next_start_line: Some(2),
                blocked_line_rendered_bytes: None,
            });
        }
        value
    }

    #[test]
    fn measured_selection_keeps_exact_page_and_restarts_omitted_page() {
        let input = measured_page_input();
        let mut proposals = Vec::new();
        let selected = compile_repository_evidence_for_representation(&input, |product| {
            proposals.push(product.support.len());
            Ok::<_, ()>((product.support.len() == 1).then_some(16_384))
        })
        .unwrap();
        let RepresentationBudgetSelectionV1::Complete {
            product,
            measured_candidates,
            ..
        } = selected
        else {
            panic!("one whole exact page fits");
        };
        assert_eq!(proposals, vec![2, 1]);
        assert_eq!(measured_candidates, 2);
        assert_eq!(product.support[0].symbol_id.as_deref(), Some("exact"));
        let exact = input
            .sources
            .iter()
            .find(|source| source.stable_identity == "node:exact")
            .unwrap();
        assert_eq!(
            product.support[0].snippet.as_deref(),
            Some(exact.source.as_str())
        );
        assert_eq!(
            product.support[0].source_delivery.as_ref(),
            exact.delivery.as_ref()
        );
        let cursor = |identity, start, end| {
            product.continuation.iter().any(|selector| {
                selector.stable_identity == identity
                    && selector.source_range
                        == Some(codestory_contracts::compilation::PacketSourceRangeV1 {
                            start_line: start,
                            end_line: end,
                        })
            })
        };
        assert!(cursor("node:exact", 2, 3));
        assert!(cursor("node:retrieved", 1, 3));
        assert!(!cursor("node:retrieved", 2, 3));
    }

    #[test]
    fn measured_selection_inspects_nonmonotone_candidates_without_binary_search() {
        let input = measured_page_input();
        let mut proposals = Vec::new();
        let selected = compile_repository_evidence_for_representation(&input, |product| {
            proposals.push(product.support.len());
            // Omissions can grow the mandatory continuation. The seam is
            // deliberately nonmonotone; support-array byte size is no oracle.
            Ok::<_, ()>(match product.support.len() {
                2 => Some(16_385),
                1 => None,
                _ => Some(4_096),
            })
        })
        .unwrap();
        assert_eq!(proposals, vec![2, 1, 0]);
        let RepresentationBudgetSelectionV1::Complete { product, .. } = selected else {
            panic!("empty evidence with honest continuation fits")
        };
        assert!(product.support.is_empty());
        assert!(
            product
                .continuation
                .iter()
                .any(|selector| selector.stable_identity == "node:exact")
        );
    }

    #[test]
    fn measured_selection_preserves_refusal_and_propagates_measurement_error() {
        let input = measured_page_input();
        let full = compile_repository_evidence(&input);
        let refused =
            compile_repository_evidence_for_representation(&input, |_| Ok::<_, ()>(None)).unwrap();
        assert_eq!(
            refused,
            RepresentationBudgetSelectionV1::BudgetExceeded {
                product: full,
                measured_candidates: 3
            }
        );
        let mut calls = 0;
        let error = compile_repository_evidence_for_representation(&input, |_| {
            calls += 1;
            Err::<Option<usize>, _>("invalid projection")
        })
        .unwrap_err();
        assert_eq!(error, "invalid projection");
        assert_eq!(calls, 1);
    }

    #[test]
    fn measured_selection_has_at_most_seventeen_pure_proposals() {
        let mut input = input();
        input.admissions.clear();
        input.sources.clear();
        for index in 0..INTERIM_MAX_ADMITTED_CANDIDATES {
            input.admissions.push(PacketAdmissionReceiptV1 {
                packet_ordinal: index as u32,
                stable_identity: format!("path:src/item_{index}.rs"),
                score_version: "v1".into(),
                reserved_source_bytes: 512,
                origin: PacketAdmissionOriginV1::Retrieval,
            });
        }
        let mut calls = 0;
        let selection = compile_repository_evidence_for_representation(&input, |_| {
            calls += 1;
            Ok::<_, ()>(Some(16_385))
        })
        .unwrap();
        assert_eq!(calls, INTERIM_MAX_ADMITTED_CANDIDATES + 1);
        let RepresentationBudgetSelectionV1::BudgetExceeded {
            measured_candidates,
            ..
        } = selection
        else {
            panic!("honest refusal")
        };
        assert_eq!(measured_candidates, calls);
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
    fn admitted_file_without_source_is_explicit_navigation() {
        let mut input = input();
        input.admissions = vec![PacketAdmissionReceiptV1 {
            packet_ordinal: 0,
            stable_identity: "path:src/large.rs".into(),
            score_version: "v1".into(),
            reserved_source_bytes: 512,
            origin: PacketAdmissionOriginV1::Retrieval,
        }];
        input.sources.clear();
        let product = compile_repository_evidence(&input);
        assert_eq!(product.support.len(), 1);
        let location = &product.support[0];
        assert_eq!(location.kind, SupportUnitKindDto::SymbolLocation);
        assert_eq!(location.path.as_deref(), Some("src/large.rs"));
        assert!(location.summary.starts_with("Navigation only:"));
        assert!(location.snippet.is_none());
    }

    #[test]
    fn source_row_limit_retries_preserve_recoverable_budget_and_source_cases() {
        use codestory_contracts::compilation::PacketAdmissionGapV1;

        let cases = [
            (
                "exhausted row",
                512,
                true,
                false,
                PacketAdmissionGapKindV1::SourceBudgetExceeded,
                false,
            ),
            (
                "over-reserved row",
                1024,
                true,
                false,
                PacketAdmissionGapKindV1::SourceBudgetExceeded,
                false,
            ),
            (
                "smaller reservation",
                511,
                true,
                false,
                PacketAdmissionGapKindV1::SourceBudgetExceeded,
                true,
            ),
            (
                "unadmitted loss",
                512,
                false,
                false,
                PacketAdmissionGapKindV1::SourceBudgetExceeded,
                true,
            ),
            (
                "hydrated allocation loss",
                512,
                true,
                true,
                PacketAdmissionGapKindV1::SourceBudgetExceeded,
                true,
            ),
            (
                "candidate count loss",
                512,
                true,
                false,
                PacketAdmissionGapKindV1::CandidateCountExceeded,
                true,
            ),
            (
                "unavailable source",
                512,
                true,
                false,
                PacketAdmissionGapKindV1::SourceUnavailable,
                false,
            ),
        ];
        for (case, reservation, admitted, hydrated, kind, retry) in cases {
            let mut input = input();
            input
                .admissions
                .retain(|admission| admitted && admission.stable_identity == "node:exact");
            for admission in &mut input.admissions {
                admission.reserved_source_bytes = reservation;
            }
            input
                .sources
                .retain(|source| hydrated && source.stable_identity == "node:exact");
            input.admission_gaps = vec![PacketAdmissionGapV1 {
                kind,
                stable_identity: Some("node:exact".into()),
                exact_selector_ordinal: Some(0),
            }];
            let product = compile_repository_evidence(&input);
            assert_eq!(!product.continuation.is_empty(), retry, "{case}");
            if admitted && !hydrated {
                assert_eq!(product.support.len(), 1, "{case}");
                assert_eq!(
                    product.support[0].kind,
                    SupportUnitKindDto::SymbolLocation,
                    "{case}"
                );
                assert!(product.support[0].snippet.is_none(), "{case}");
            }
        }
    }

    #[test]
    fn unavailable_source_retry_cannot_displace_a_verified_page_cursor() {
        use codestory_contracts::compilation::{
            PacketAdmissionGapV1, PacketIdentityAmbiguityV1, PacketSourceDeliveryV1,
            PacketSourceRangeV1,
        };

        for identity in ["path:src/unavailable.h", "node:exact"] {
            for admitted in [false, true] {
                let mut input = input();
                input.admissions[0].reserved_source_bytes = 512;
                input.admissions[1].stable_identity = identity.into();
                input
                    .admissions
                    .retain(|receipt| admitted || receipt.stable_identity != identity);
                input
                    .sources
                    .retain(|source| source.stable_identity == "node:retrieved");
                let page = &mut input.sources[0];
                page.end_line = 6;
                page.source = ">    5 | first\n     6 | second".into();
                page.delivery = Some(PacketSourceDeliveryV1 {
                    requested_range: PacketSourceRangeV1 {
                        start_line: 5,
                        end_line: 8,
                    },
                    source_sha256: "a".repeat(64),
                    remaining_source_bytes: 20,
                    next_start_line: Some(7),
                    blocked_line_rendered_bytes: None,
                });
                input.admission_gaps = vec![PacketAdmissionGapV1 {
                    kind: PacketAdmissionGapKindV1::SourceUnavailable,
                    stable_identity: Some(identity.into()),
                    exact_selector_ordinal: Some(0),
                }];
                input.ambiguities = vec![PacketIdentityAmbiguityV1 {
                    selector: "shared".into(),
                    candidate_identities: vec![identity.into()],
                }];

                let product = compile_repository_evidence(&input);
                assert_eq!(
                    product.continuation.len(),
                    1,
                    "{identity}, admitted={admitted}"
                );
                let cursor = &product.continuation[0];
                assert_eq!(cursor.stable_identity, "node:retrieved");
                assert_eq!(
                    cursor.source_range,
                    Some(PacketSourceRangeV1 {
                        start_line: 7,
                        end_line: 8
                    })
                );
                assert!(
                    product.support.iter().all(|unit| unit.snippet.is_none()
                        || unit.symbol_id.as_deref() == Some("retrieved"))
                );

                // Missing descriptor allocation bounds precede hydration and
                // must still offer an exact read that can be admitted later.
                input.admission_gaps[0].kind = PacketAdmissionGapKindV1::SourceBoundMissing;
                input
                    .admissions
                    .retain(|receipt| receipt.stable_identity != identity);
                let recoverable = compile_repository_evidence(&input);
                assert!(
                    recoverable
                        .continuation
                        .iter()
                        .any(|selector| selector.stable_identity == identity
                            && selector.reason == PacketStructuralGapReasonV1::SourceUnavailable)
                );

                // A failed observation must not erase an independently
                // delivered page for that same identity.
                input.admission_gaps = vec![PacketAdmissionGapV1 {
                    kind: PacketAdmissionGapKindV1::SourceUnavailable,
                    stable_identity: Some("node:retrieved".into()),
                    exact_selector_ordinal: Some(0),
                }];
                let delivered = compile_repository_evidence(&input);
                assert!(
                    delivered
                        .continuation
                        .iter()
                        .any(|selector| selector.stable_identity == "node:retrieved"
                            && selector.source_range
                                == Some(PacketSourceRangeV1 {
                                    start_line: 7,
                                    end_line: 8
                                }))
                );
            }
        }
    }

    #[test]
    fn continuation_progress_prioritizes_unshown_sources_without_closing_omitted_relations() {
        let mut input = input();
        input.admissions = (0..16)
            .map(|ordinal| PacketAdmissionReceiptV1 {
                packet_ordinal: ordinal,
                stable_identity: format!("node:{ordinal}"),
                score_version: "v1".into(),
                reserved_source_bytes: 128,
                origin: PacketAdmissionOriginV1::Retrieval,
            })
            .collect();
        input.sources = (0..16)
            .map(|ordinal| PacketHydratedSourceRangeV1 {
                stable_identity: format!("node:{ordinal}"),
                path: format!("src/{ordinal}.rs"),
                symbol: Some(format!("stage_{ordinal}")),
                start_line: 1,
                end_line: 1,
                source: format!("fn stage_{ordinal}() {{}}"),
                delivery: None,
                parser_completeness: PacketParserCompletenessV1::Complete,
            })
            .collect();
        input.relations = (1..16)
            .map(|ordinal| PacketDirectedRelationV1 {
                relation_id: format!("incident-{ordinal:02}"),
                from_identity: "node:0".into(),
                to_identity: format!("node:{ordinal}"),
                relation_kind: PacketRelationKindV1::Call,
                certainty: PacketRelationCertaintyV1::Certain,
            })
            .collect();

        let product = compile_repository_evidence(&input);
        assert_eq!(product.support.len(), INTERIM_MAX_ADMITTED_CANDIDATES);
        assert!(
            product
                .support
                .iter()
                .any(|unit| unit.kind == SupportUnitKindDto::SourceRange
                    && unit.symbol_id.as_deref() == Some("0"))
        );
        assert!(
            !product
                .support
                .iter()
                .any(|unit| unit.id == "edge:incident-09")
        );
        assert!(
            !product
                .support
                .iter()
                .any(|unit| unit.kind == SupportUnitKindDto::SourceRange
                    && unit.symbol_id.as_deref() == Some("8"))
        );
        assert_eq!(
            product
                .continuation
                .iter()
                .map(|selector| (selector.stable_identity.as_str(), selector.reason))
                .collect::<Vec<_>>(),
            vec![
                ("node:8", PacketStructuralGapReasonV1::SourceBudgetExceeded),
                ("node:9", PacketStructuralGapReasonV1::SourceBudgetExceeded),
                ("node:10", PacketStructuralGapReasonV1::SourceBudgetExceeded),
                ("node:11", PacketStructuralGapReasonV1::SourceBudgetExceeded),
                ("node:12", PacketStructuralGapReasonV1::SourceBudgetExceeded),
                ("node:13", PacketStructuralGapReasonV1::SourceBudgetExceeded),
                ("node:14", PacketStructuralGapReasonV1::SourceBudgetExceeded),
                ("node:15", PacketStructuralGapReasonV1::SourceBudgetExceeded),
                ("node:0", PacketStructuralGapReasonV1::DisconnectedSeed),
            ],
            "unshown source identities lead, while the shown endpoint retains its omitted-relation gap"
        );
        assert!(
            serde_json::to_vec(&product.support).unwrap().len()
                <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES
        );
    }

    #[test]
    fn continuation_progress_preserves_missing_source_and_ambiguity_reasons_in_each_priority_group()
    {
        use codestory_contracts::compilation::{PacketAdmissionGapV1, PacketIdentityAmbiguityV1};
        let mut input = input();
        input.admission_gaps = vec![
            PacketAdmissionGapV1 {
                kind: PacketAdmissionGapKindV1::SourceUnavailable,
                stable_identity: Some("node:exact".into()),
                exact_selector_ordinal: Some(0),
            },
            PacketAdmissionGapV1 {
                kind: PacketAdmissionGapKindV1::SourceUnavailable,
                stable_identity: Some("node:missing".into()),
                exact_selector_ordinal: Some(1),
            },
        ];
        input.ambiguities = vec![PacketIdentityAmbiguityV1 {
            selector: "shared".into(),
            candidate_identities: vec![
                "node:retrieved".into(),
                "node:missing".into(),
                "node:other".into(),
                "node:missing".into(),
            ],
        }];
        let product = compile_repository_evidence(&input);
        assert_eq!(
            product
                .continuation
                .iter()
                .map(|selector| (selector.stable_identity.as_str(), selector.reason))
                .collect::<Vec<_>>(),
            vec![
                ("node:other", PacketStructuralGapReasonV1::AmbiguousSelector),
                ("node:exact", PacketStructuralGapReasonV1::SourceUnavailable),
                (
                    "node:retrieved",
                    PacketStructuralGapReasonV1::AmbiguousSelector
                ),
            ]
        );

        input.admission_gaps.truncate(1);
        input.ambiguities.clear();
        let source_present_only = compile_repository_evidence(&input);
        assert_eq!(source_present_only.continuation.len(), 1);
        assert_eq!(
            source_present_only.continuation[0].stable_identity,
            "node:exact"
        );
        assert_eq!(
            source_present_only.continuation[0].reason,
            PacketStructuralGapReasonV1::SourceUnavailable
        );
    }

    #[test]
    fn only_certain_relations_enter_the_product() {
        let mut input = input();
        input.relations = vec![
            PacketDirectedRelationV1 {
                relation_id: "certain".into(),
                from_identity: "node:exact".into(),
                to_identity: "node:retrieved".into(),
                relation_kind: PacketRelationKindV1::Call,
                certainty: PacketRelationCertaintyV1::Certain,
            },
            PacketDirectedRelationV1 {
                relation_id: "uncertain".into(),
                from_identity: "node:exact".into(),
                to_identity: "node:retrieved".into(),
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
            .retain(|source| source.stable_identity == "node:exact");
        input.relations = vec![PacketDirectedRelationV1 {
            relation_id: "connects-seeds".into(),
            from_identity: "node:exact".into(),
            to_identity: "node:retrieved".into(),
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
            .position(|unit| unit.id == "symbol:node:retrieved")
            .expect("symbol fallback");
        assert!(relation_index < fallback_index);
    }

    #[test]
    fn parser_complete_source_wins_identical_range_ties() {
        let mut input = input();
        input.sources.insert(
            0,
            PacketHydratedSourceRangeV1 {
                stable_identity: "node:retrieved".into(),
                path: "src/shared.rs".into(),
                symbol: Some("retrieved".into()),
                start_line: 5,
                end_line: 8,
                source: "partial witness".into(),
                delivery: None,
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
                from_identity: "node:retrieved".into(),
                to_identity: "node:external-a".into(),
                relation_kind: PacketRelationKindV1::Usage,
                certainty: PacketRelationCertaintyV1::Certain,
            },
            PacketDirectedRelationV1 {
                relation_id: "second-incident".into(),
                from_identity: "node:retrieved".into(),
                to_identity: "node:external-b".into(),
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
    fn complete_source_delivery_preserves_distinct_unread_tails_and_terminal_lines() {
        use codestory_contracts::compilation::{PacketSourceDeliveryV1, PacketSourceRangeV1};
        let mut frozen = input();
        frozen.relations.clear();
        frozen.sources.truncate(2);
        for (index, source) in frozen.sources.iter_mut().enumerate() {
            source.path = "shared.rs".into();
            source.start_line = 1;
            source.end_line = 2;
            source.source = "same complete physical lines".into();
            source.delivery = Some(PacketSourceDeliveryV1 {
                requested_range: PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: 10 + index as u32,
                },
                source_sha256: "a".repeat(64),
                remaining_source_bytes: 100,
                next_start_line: Some(3),
                blocked_line_rendered_bytes: None,
            });
        }
        let product = compile_repository_evidence(&frozen);
        assert_eq!(
            product
                .support
                .iter()
                .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
                .count(),
            2
        );
        assert_eq!(
            product
                .continuation
                .iter()
                .filter(|selector| selector.source_range.is_some())
                .count(),
            2
        );
        for source in &frozen.sources {
            assert!(
                product
                    .continuation
                    .iter()
                    .any(
                        |selector| selector.stable_identity == source.stable_identity
                            && selector.source_range
                                == Some(PacketSourceRangeV1 {
                                    start_line: 3,
                                    end_line: source
                                        .delivery
                                        .as_ref()
                                        .unwrap()
                                        .requested_range
                                        .end_line
                                })
                    )
            );
        }
        let blocked = frozen.sources[0].delivery.as_mut().unwrap();
        blocked.next_start_line = None;
        blocked.blocked_line_rendered_bytes = Some(600);
        let product = compile_repository_evidence(&frozen);
        assert!(product.support.iter().any(|unit| {
            unit.source_delivery
                .as_ref()
                .is_some_and(|delivery| delivery.blocked_line_rendered_bytes == Some(600))
        }));
        assert!(
            !product
                .continuation
                .iter()
                .any(
                    |selector| selector.stable_identity == frozen.sources[0].stable_identity
                        && selector.source_range.is_some()
                )
        );
        frozen.sources[0].delivery.as_mut().unwrap().next_start_line = Some(1);
        let invalid = compile_repository_evidence(&frozen);
        assert!(
            !invalid
                .support
                .iter()
                .any(|unit| unit.kind == SupportUnitKindDto::SourceRange
                    && unit.symbol_id.as_deref()
                        == public_symbol_id(&frozen.sources[0].stable_identity).as_deref())
        );
    }

    #[test]
    fn terminal_source_refusal_does_not_reenter_through_ambiguity() {
        use codestory_contracts::compilation::{PacketAdmissionGapV1, PacketIdentityAmbiguityV1};
        let mut frozen = input();
        let identity = frozen.admissions[0].stable_identity.clone();
        frozen.admissions[0].reserved_source_bytes = INTERIM_SOURCE_ROW_UPPER_BOUND as u32;
        frozen
            .sources
            .retain(|source| source.stable_identity != identity);
        frozen.admission_gaps.push(PacketAdmissionGapV1 {
            kind: PacketAdmissionGapKindV1::SourceBudgetExceeded,
            stable_identity: Some(identity.clone()),
            exact_selector_ordinal: None,
        });
        frozen.ambiguities.push(PacketIdentityAmbiguityV1 {
            selector: "shared".into(),
            candidate_identities: vec![identity.clone(), "node:other".into()],
        });
        let product = compile_repository_evidence(&frozen);
        assert!(
            !product
                .continuation
                .iter()
                .any(|selector| selector.stable_identity == identity)
        );
        assert!(
            product
                .continuation
                .iter()
                .any(|selector| selector.stable_identity == "node:other"
                    && selector.reason == PacketStructuralGapReasonV1::AmbiguousSelector)
        );
    }

    #[test]
    fn omitted_blocked_page_retries_its_requested_start_without_skipping_unseen_lines() {
        use codestory_contracts::compilation::{PacketSourceDeliveryV1, PacketSourceRangeV1};
        for explicit_gap in [false, true] {
            let mut frozen = input();
            frozen.admissions = (0..16)
                .map(|ordinal| PacketAdmissionReceiptV1 {
                    packet_ordinal: ordinal,
                    stable_identity: format!("node:{ordinal}"),
                    score_version: "v1".into(),
                    reserved_source_bytes: 512,
                    origin: PacketAdmissionOriginV1::Retrieval,
                })
                .collect();
            frozen.sources = (0..16)
                .map(|ordinal| PacketHydratedSourceRangeV1 {
                    stable_identity: format!("node:{ordinal}"),
                    path: format!("src/{ordinal}.rs"),
                    symbol: None,
                    start_line: 5,
                    end_line: 6,
                    source: "two complete lines".into(),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                    delivery: Some(PacketSourceDeliveryV1 {
                        requested_range: PacketSourceRangeV1 {
                            start_line: 5,
                            end_line: 20,
                        },
                        source_sha256: "a".repeat(64),
                        remaining_source_bytes: 1000,
                        next_start_line: if ordinal == 15 { None } else { Some(7) },
                        blocked_line_rendered_bytes: if ordinal == 15 { Some(600) } else { None },
                    }),
                })
                .collect();
            frozen.relations = (1..16)
                .map(|ordinal| PacketDirectedRelationV1 {
                    relation_id: format!("edge-{ordinal:02}"),
                    from_identity: "node:0".into(),
                    to_identity: format!("node:{ordinal}"),
                    relation_kind: PacketRelationKindV1::Call,
                    certainty: PacketRelationCertaintyV1::Certain,
                })
                .collect();
            if explicit_gap {
                frozen.admission_gaps =
                    vec![codestory_contracts::compilation::PacketAdmissionGapV1 {
                        kind: PacketAdmissionGapKindV1::SourceUnavailable,
                        stable_identity: Some("node:15".into()),
                        exact_selector_ordinal: Some(0),
                    }];
            }
            let product = compile_repository_evidence(&frozen);
            assert!(
                !product
                    .support
                    .iter()
                    .any(|unit| unit.kind == SupportUnitKindDto::SourceRange
                        && unit.symbol_id.as_deref() == Some("15"))
            );
            assert!(
                product
                    .continuation
                    .iter()
                    .any(|selector| selector.stable_identity == "node:15"
                        && selector.source_range
                            == Some(PacketSourceRangeV1 {
                                start_line: 5,
                                end_line: 20
                            })),
                "an unseen requested page must retain its original range"
            );
            assert!(
                !product
                    .continuation
                    .iter()
                    .any(|selector| selector.stable_identity == "node:15"
                        && selector.source_range.is_none())
            );
            // A retained blocked page cannot erase a different unseen page for the same identity.
            frozen.sources[0].delivery.as_mut().unwrap().next_start_line = None;
            frozen.sources[0]
                .delivery
                .as_mut()
                .unwrap()
                .blocked_line_rendered_bytes = Some(600);
            frozen.sources[15].stable_identity = "node:0".into();
            frozen.sources[15].path = frozen.sources[0].path.clone();
            frozen.sources[15].start_line = 50;
            frozen.sources[15].end_line = 51;
            frozen.sources[15].source = "different later complete lines".into();
            frozen.sources[15]
                .delivery
                .as_mut()
                .unwrap()
                .requested_range = PacketSourceRangeV1 {
                start_line: 50,
                end_line: 80,
            };
            frozen.sources[15]
                .delivery
                .as_mut()
                .unwrap()
                .next_start_line = Some(52);
            frozen.sources[15]
                .delivery
                .as_mut()
                .unwrap()
                .blocked_line_rendered_bytes = None;
            if explicit_gap {
                frozen.admission_gaps[0].stable_identity = Some("node:0".into());
            }
            let duplicate_identity = compile_repository_evidence(&frozen);
            assert!(
                !duplicate_identity
                    .support
                    .iter()
                    .any(|unit| unit.start_line == Some(50))
            );
            assert!(duplicate_identity.continuation.iter().any(
                |selector| selector.stable_identity == "node:0"
                    && selector.source_range
                        == Some(PacketSourceRangeV1 {
                            start_line: 50,
                            end_line: 80
                        })
            ));
        }
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
