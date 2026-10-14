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

/// Three prefix families plus one fixed-base neighborhood: at most sixteen
/// additions and k*(16-k)<=64 exchanges of already hydrated physical units.
pub const REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1: usize = 3
    * (INTERIM_MAX_ADMITTED_CANDIDATES + 1)
    + INTERIM_MAX_ADMITTED_CANDIDATES
    + INTERIM_MAX_ADMITTED_CANDIDATES * INTERIM_MAX_ADMITTED_CANDIDATES / 4;

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
/// Three bounded prefix families compete with a fixed-base physical-source
/// neighborhood; the bound is REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1. Do not
/// binary-search: omitting a page changes gaps and continuation, so final byte
/// cost need not be monotone in the number of retained rows.
pub fn compile_repository_evidence_for_representation<E>(
    input: &PacketCompilationInputV1,
    measure_complete: impl FnMut(&RepositoryDerivedCompilationV1) -> Result<Option<usize>, E>,
) -> Result<RepresentationBudgetSelectionV1, E> {
    compile_repository_evidence_for_representation_with_navigation(input, &[], measure_complete)
}

/// Optional publication-bound navigation affects only source ordering, never
/// emitted graph proof, admission, hydration, certainty or answer sufficiency.
pub fn compile_repository_evidence_for_representation_with_navigation<E>(
    input: &PacketCompilationInputV1,
    navigation: &[PacketDirectedRelationV1],
    mut measure_complete: impl FnMut(&RepositoryDerivedCompilationV1) -> Result<Option<usize>, E>,
) -> Result<RepresentationBudgetSelectionV1, E> {
    let full = compile_repository_evidence(input);
    let source_first = compile_repository_evidence_with_row_allocation(
        input,
        INTERIM_MAX_ADMITTED_CANDIDATES,
        false,
    );
    let complete_first = compile_repository_evidence_with_source_order_and_navigation(
        input,
        INTERIM_MAX_ADMITTED_CANDIDATES,
        false,
        true,
        navigation,
    );
    let mut measured_candidates = 0;
    let mut best = None;
    for row_limit in (0..=full
        .support
        .len()
        .max(source_first.support.len())
        .max(complete_first.support.len()))
        .rev()
    {
        let graph_candidate = if row_limit == full.support.len() {
            full.clone()
        } else {
            compile_repository_evidence_with_row_limit(input, row_limit)
        };
        let source_candidate = if row_limit == source_first.support.len() {
            source_first.clone()
        } else {
            compile_repository_evidence_with_row_allocation(input, row_limit, false)
        };
        let mut candidates = vec![graph_candidate];
        if !candidates.contains(&source_candidate) {
            candidates.push(source_candidate);
        }
        let complete_candidate = if row_limit == complete_first.support.len() {
            complete_first.clone()
        } else {
            compile_repository_evidence_with_source_order_and_navigation(
                input, row_limit, false, true, navigation,
            )
        };
        if !candidates.contains(&complete_candidate) {
            candidates.push(complete_candidate);
        }
        for candidate in candidates {
            measured_candidates += 1;
            if let Some(representation_bytes) = measure_complete(&candidate)?
                && representation_bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES
            {
                let rank = repository_evidence_selection_rank(input, navigation, &candidate);
                // Keep the graph-preserving family on a source-rank tie.
                // Omitted relations retain their ordinary structural selectors.
                if best
                    .as_ref()
                    .is_none_or(|(previous, _, _)| rank > *previous)
                {
                    best = Some((rank, candidate, representation_bytes));
                }
            }
        }
    }
    if let Some((_, baseline, _)) = best.clone() {
        let admissions = ordered_admissions(input);
        let admission_order = admissions
            .iter()
            .enumerate()
            .map(|(i, a)| (a.stable_identity.as_str(), i))
            .collect::<HashMap<_, _>>();
        let sources = selected_source_ranges(input, &admission_order);
        let exact = admissions
            .iter()
            .filter(|a| a.origin == PacketAdmissionOriginV1::ExactTypedSelector)
            .map(|a| a.stable_identity.as_str())
            .collect::<BTreeSet<_>>();
        let base_sources = baseline
            .support
            .iter()
            .filter(|r| r.kind == SupportUnitKindDto::SourceRange)
            .cloned()
            .collect::<Vec<_>>();
        let graph = baseline
            .support
            .iter()
            .filter(|r| r.kind == SupportUnitKindDto::TypedGraphEdge)
            .cloned()
            .collect::<Vec<_>>();
        // Include every baseline unit first. Add at most 16-k distinct physical
        // witnesses, in sealed admission/path-diverse order. Aliases are one
        // physical unit, and complete DTO equality prevents ID-only collisions.
        let mut pool = base_sources.clone();
        for source in &sources {
            let row = source_support_unit(source.witness);
            if pool.len() < INTERIM_MAX_ADMITTED_CANDIDATES && !pool.contains(&row) {
                pool.push(row);
            }
        }
        let available = pool
            .into_iter()
            .filter(|r| !base_sources.contains(r))
            .collect::<Vec<_>>();
        let protected = |row: &SupportUnitDto| {
            sources.iter().any(|s| {
                source_support_unit(s.witness) == *row
                    && s.represented_identities
                        .iter()
                        .any(|id| exact.contains(id.as_str()))
            })
        };
        // Exactly one pass around this fixed baseline; accepting an improvement
        // never restarts the neighborhood or invents a new hydration request.
        for incoming in &available {
            for removed in
                std::iter::once(None).chain(base_sources.iter().filter(|r| !protected(r)).map(Some))
            {
                let mut chosen = base_sources
                    .iter()
                    .filter(|r| removed != Some(*r))
                    .cloned()
                    .collect::<Vec<_>>();
                chosen.push(incoming.clone());
                chosen.extend(graph.iter().cloned());
                if chosen.len() > INTERIM_MAX_ADMITTED_CANDIDATES {
                    continue;
                }
                let candidate = compile_repository_evidence_with_options(
                    input,
                    chosen.len(),
                    false,
                    false,
                    &[],
                    Some(&chosen),
                );
                // The internal row-byte gate can still reject a chosen unit.
                // Preserve witnesses, rather than merely permitting their IDs.
                if graph.iter().any(|row| !candidate.support.contains(row))
                    || base_sources
                        .iter()
                        .filter(|row| protected(row))
                        .any(|row| !candidate.support.contains(row))
                {
                    continue;
                }
                measured_candidates += 1;
                if let Some(bytes) = measure_complete(&candidate)?
                    && bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES
                {
                    let rank = repository_evidence_selection_rank(input, navigation, &candidate);
                    if best.as_ref().is_none_or(|(old, _, _)| rank > *old) {
                        best = Some((rank, candidate, bytes));
                    }
                }
            }
        }
    }
    if let Some((_, product, representation_bytes)) = best {
        return Ok(RepresentationBudgetSelectionV1::Complete {
            product,
            representation_bytes,
            measured_candidates,
        });
    }
    Ok(RepresentationBudgetSelectionV1::BudgetExceeded {
        product: full,
        measured_candidates,
    })
}

/// Accepted navigation-prefix selection, evaluated on the current input.
/// Callable metadata was not part of that policy. Clearing it in this local
/// view reproduces the accepted policy without changing authenticated sources,
/// delivery receipts, publication, graph or caller-owned hydration.
pub fn compile_repository_evidence_baseline_for_representation<E>(
    current: &PacketCompilationInputV1,
    navigation: &[PacketDirectedRelationV1],
    mut measure_complete: impl FnMut(&RepositoryDerivedCompilationV1) -> Result<Option<usize>, E>,
) -> Result<RepresentationBudgetSelectionV1, E> {
    let mut view = current.clone();
    for source in &mut view.sources {
        source.callable_body = None;
    }
    let input = &view;
    let full = compile_repository_evidence(input);
    let source_first = compile_repository_evidence_with_row_allocation(
        input,
        INTERIM_MAX_ADMITTED_CANDIDATES,
        false,
    );
    let complete_first = compile_repository_evidence_with_source_order_and_navigation(
        input,
        INTERIM_MAX_ADMITTED_CANDIDATES,
        false,
        true,
        navigation,
    );
    let mut measured_candidates = 0;
    let mut best = None;
    for row_limit in (0..=full
        .support
        .len()
        .max(source_first.support.len())
        .max(complete_first.support.len()))
        .rev()
    {
        let graph_candidate = if row_limit == full.support.len() {
            full.clone()
        } else {
            compile_repository_evidence_with_row_limit(input, row_limit)
        };
        let source_candidate = if row_limit == source_first.support.len() {
            source_first.clone()
        } else {
            compile_repository_evidence_with_row_allocation(input, row_limit, false)
        };
        let mut candidates = vec![graph_candidate];
        if !candidates.contains(&source_candidate) {
            candidates.push(source_candidate);
        }
        let complete_candidate = if row_limit == complete_first.support.len() {
            complete_first.clone()
        } else {
            compile_repository_evidence_with_source_order_and_navigation(
                input, row_limit, false, true, navigation,
            )
        };
        if !candidates.contains(&complete_candidate) {
            candidates.push(complete_candidate);
        }
        for candidate in candidates {
            measured_candidates += 1;
            if let Some(representation_bytes) = measure_complete(&candidate)?
                && representation_bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES
            {
                let rank = repository_evidence_selection_rank(input, navigation, &candidate);
                // Keep the graph-preserving family on a source-rank tie.
                // Omitted relations retain their ordinary structural selectors.
                if best
                    .as_ref()
                    .is_none_or(|(previous, _, _)| rank > *previous)
                {
                    best = Some((rank, candidate, representation_bytes));
                }
            }
        }
    }

    Ok(match best {
        Some((_, product, representation_bytes)) => RepresentationBudgetSelectionV1::Complete {
            product,
            representation_bytes,
            measured_candidates,
        },
        None => RepresentationBudgetSelectionV1::BudgetExceeded {
            product: full,
            measured_candidates,
        },
    })
}

pub fn repository_evidence_baseline_rank(
    input: &PacketCompilationInputV1,
    navigation: &[PacketDirectedRelationV1],
    product: &RepositoryDerivedCompilationV1,
) -> RepositoryEvidenceSelectionRankV1 {
    let mut rank = repository_evidence_selection_rank(input, navigation, product);
    rank.direct_body_reach = 0;
    rank.complete_callable_units = 0;
    rank
}

/// Conservative joint body-progress search around a baseline already accepted for this same
/// publication, request and hydrated input. This is not the legacy selector.
/// The caller must check served source, unresolved labels and selector authority
/// inside `measure_complete`; bytes alone do not establish those invariants.
/// Partial pages qualify only with authenticated executable-body intersection;
/// old source is immutable and unread tails remain ordinary continuation.
/// One baseline, all single/pair additions, then winner/fallback revalidation:
/// <=137 candidate callbacks plus <=2 validation callbacks. Caller-internal
/// rendering probes and baseline construction have separate bounds.
pub const REPOSITORY_EVIDENCE_JOINT_CALLBACKS_MAX: usize = 139;

/// Refusal variants carry no rejected source or graph payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JointRepresentationSelectionV1 {
    Complete {
        product: RepositoryDerivedCompilationV1,
        representation_bytes: usize,
        measured_candidates: usize,
    },
    InvalidBaseline,
    NoAdmissibleFrame {
        measured_candidates: usize,
    },
}

pub fn compile_repository_evidence_joint_additions<E>(
    input: &PacketCompilationInputV1,
    baseline: &RepositoryDerivedCompilationV1,
    mut measure_complete: impl FnMut(&RepositoryDerivedCompilationV1) -> Result<Option<usize>, E>,
) -> Result<JointRepresentationSelectionV1, E> {
    let refusal = |measured_candidates| JointRepresentationSelectionV1::NoAdmissibleFrame {
        measured_candidates,
    };
    // Reconstruct every source/graph DTO from the current authenticated input.
    // Foreign, drifted or internally over-budget baseline rows fail closed.
    let canonical = compile_repository_evidence_with_options(
        input,
        baseline.support.len(),
        false,
        false,
        &[],
        Some(&baseline.support),
    );
    if input.contract_version != PACKET_COMPILATION_CONTRACT_VERSION_V1
        || baseline.support.len() > INTERIM_MAX_ADMITTED_CANDIDATES
        || canonical.support.len() != baseline.support.len()
        || baseline
            .support
            .iter()
            .any(|row| !canonical.support.contains(row))
    {
        return Ok(JointRepresentationSelectionV1::InvalidBaseline);
    }
    if canonical.continuation != baseline.continuation {
        return Ok(JointRepresentationSelectionV1::InvalidBaseline);
    }
    let mut measured_candidates = 1;
    let Some(base_bytes) =
        measure_complete(baseline)?.filter(|bytes| *bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES)
    else {
        return Ok(refusal(measured_candidates));
    };
    let admissions = ordered_admissions(input);
    let order = admissions
        .iter()
        .enumerate()
        .map(|(i, a)| (a.stable_identity.as_str(), i))
        .collect::<HashMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut pool = Vec::new();
    for source in selected_source_ranges(input, &order) {
        let witness = source.witness;
        let Some(key) = authenticated_callable_progress_key(input, witness) else {
            continue;
        };
        let row = source_support_unit(witness);
        let singleton = RepositoryDerivedCompilationV1 {
            support: vec![row.clone()],
            continuation: Vec::new(),
        };
        if newly_uncovered_callable_body_lines(input, baseline, &singleton) > 0
            && seen.insert(key)
            && pool.len() < INTERIM_MAX_ADMITTED_CANDIDATES
        {
            pool.push(row);
        }
    }
    let mut best = baseline.clone();
    let mut best_gain = (0_u64, 0_usize);
    let mut best_bytes = base_bytes;
    // Rejected singles never prune pairs: full-envelope cost is nonmonotone.
    for first in 0..pool.len() {
        for second in std::iter::once(None).chain((first + 1..pool.len()).map(Some)) {
            let added_rows = 1 + usize::from(second.is_some());
            if baseline.support.len() + added_rows > INTERIM_MAX_ADMITTED_CANDIDATES {
                continue;
            }
            let mut rows = baseline.support.clone();
            rows.push(pool[first].clone());
            if let Some(second) = second {
                rows.push(pool[second].clone());
            }
            let candidate = compile_repository_evidence_with_options(
                input,
                rows.len(),
                false,
                false,
                &[],
                Some(&rows),
            );
            // The internal budget must not silently drop a proposed witness.
            if candidate.support.len() != rows.len()
                || rows.iter().any(|row| !candidate.support.contains(row))
            {
                continue;
            }
            let new_lines = newly_uncovered_callable_body_lines(input, baseline, &candidate);
            if new_lines == 0 {
                continue;
            }
            let gain = repository_evidence_joint_progress_rank(input, baseline, &candidate);
            measured_candidates += 1;
            if let Some(bytes) = measure_complete(&candidate)?
                && bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES
                && (gain > best_gain || (gain == best_gain && bytes < best_bytes))
            {
                best = candidate;
                best_gain = gain;
                best_bytes = bytes;
            }
        }
    }
    // Validation includes the caller's final served-frame constraints, not
    // just a remembered cost. A stale baseline never becomes Complete.
    measured_candidates += 1;
    if let Some(bytes) =
        measure_complete(&best)?.filter(|bytes| *bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES)
    {
        return Ok(JointRepresentationSelectionV1::Complete {
            product: best,
            representation_bytes: bytes,
            measured_candidates,
        });
    }
    if best != *baseline {
        measured_candidates += 1;
        if let Some(bytes) =
            measure_complete(baseline)?.filter(|bytes| *bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES)
        {
            return Ok(JointRepresentationSelectionV1::Complete {
                product: baseline.clone(),
                representation_bytes: bytes,
                measured_candidates,
            });
        }
    }
    Ok(refusal(measured_candidates))
}

/// Compare additions across verified page extents against one fixed baseline.
/// Progress uses the same physical union and metadata consensus as joint search.
pub fn repository_evidence_joint_progress_rank(
    input: &PacketCompilationInputV1,
    baseline: &RepositoryDerivedCompilationV1,
    candidate: &RepositoryDerivedCompilationV1,
) -> (u64, usize) {
    (
        newly_uncovered_callable_body_lines(input, baseline, candidate),
        repository_evidence_selection_rank(input, &[], candidate)
            .complete_callable_units
            .saturating_sub(
                repository_evidence_selection_rank(input, &[], baseline).complete_callable_units,
            ),
    )
}

/// Count actual new physical body lines, never rounded fractions or aliases.
/// Baseline source coverage is subtracted as a union before candidates compete.
fn newly_uncovered_callable_body_lines(
    input: &PacketCompilationInputV1,
    baseline: &RepositoryDerivedCompilationV1,
    candidate: &RepositoryDerivedCompilationV1,
) -> u64 {
    let mut incoming = BTreeMap::<(String, String), Vec<(u32, u32)>>::new();
    for source in &input.sources {
        let Some(key) = authenticated_callable_progress_key(input, source) else {
            continue;
        };
        let row = source_support_unit(source);
        if !candidate.support.contains(&row) {
            continue;
        }
        let body = source.callable_body.as_ref().unwrap();
        incoming.entry((key.0, key.1)).or_default().push((
            source.start_line.max(body.body_range.start_line),
            source.end_line.min(body.body_range.end_line),
        ));
    }
    let mut total = 0_u64;
    for ((path, hash), mut intervals) in incoming {
        intervals.sort_unstable();
        let mut merged = Vec::<(u32, u32)>::new();
        for (start, end) in intervals {
            if let Some(last) = merged.last_mut()
                && start <= last.1.saturating_add(1)
            {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        let mut covered = baseline
            .support
            .iter()
            .filter_map(|row| {
                let delivery = row.source_delivery.as_ref()?;
                let start = row.start_line?;
                let end = row.end_line?;
                (row.kind == SupportUnitKindDto::SourceRange
                    && row.path.as_deref() == Some(path.as_str())
                    && delivery.source_sha256 == hash
                    && delivery.matches_delivery(start, end))
                .then_some((start, end))
            })
            .collect::<Vec<_>>();
        covered.sort_unstable();
        for (start, end) in merged {
            let mut cursor = u64::from(start);
            let last = u64::from(end);
            for &(old_start, old_end) in &covered {
                let left = u64::from(old_start);
                let right = u64::from(old_end);
                if right < cursor {
                    continue;
                }
                if left > last {
                    break;
                }
                if left > cursor {
                    total += left.min(last + 1) - cursor;
                }
                cursor = cursor.max(right + 1);
                if cursor > last {
                    break;
                }
            }
            if cursor <= last {
                total += last + 1 - cursor;
            }
        }
    }
    total
}

fn authenticated_callable_progress_key(
    input: &PacketCompilationInputV1,
    source: &PacketHydratedSourceRangeV1,
) -> Option<(String, String, u32, u32)> {
    let body = source.callable_body.as_ref()?;
    let delivery = source.delivery.as_ref()?;
    if !delivery.matches_delivery(source.start_line, source.end_line)
        || !body.matches_delivery(delivery)
        || source.start_line.max(body.body_range.start_line)
            > source.end_line.min(body.body_range.end_line)
    {
        return None;
    }
    // Same identity and same physical page aliases require metadata consensus.
    // A missing/conflicting alias does not become executable by choosing it last.
    for other in &input.sources {
        if (other.stable_identity == source.stable_identity
            || (other.path == source.path
                && other.start_line == source.start_line
                && other.end_line == source.end_line)
            || (other.path == source.path
                && other.callable_body.as_ref().is_some_and(|alias| {
                    alias.unit_range == body.unit_range
                        && (alias.source_sha256 == body.source_sha256
                            || other.delivery.as_ref().is_some_and(|delivery| {
                                delivery.source_sha256 == body.source_sha256
                            }))
                })))
            && (other.path != source.path
                || other.callable_body.as_ref() != Some(body)
                || !other.delivery.as_ref().is_some_and(|d| {
                    d.matches_delivery(other.start_line, other.end_line) && body.matches_delivery(d)
                }))
        {
            return None;
        }
    }
    Some((
        source.path.clone(),
        body.source_sha256.clone(),
        body.unit_range.start_line,
        body.unit_range.end_line,
    ))
}

/// A question-free rank shared by the family and physical-page comparisons.
/// Authenticated direct callable-body progress precedes speculative navigation.
/// Missing body metadata keeps the legacy range evidence unknown, not executable.
/// Navigation measures investigation reach, never source or graph proof for a
/// caller. Each admitted identity receives at most its best direct/target
/// fraction; duplicate callsites and containment aliases cannot multiply it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RepositoryEvidenceSelectionRankV1 {
    requested_retained: usize,
    requested_complete: usize,
    requested_delivered_lines: u64,
    direct_body_reach: u64,
    complete_callable_units: usize,
    admitted_reach: u64,
    direct_reach: u64,
    complete_sources: usize,
    source_rows: usize,
    source_bytes: usize,
}

pub fn repository_evidence_selection_rank(
    input: &PacketCompilationInputV1,
    navigation: &[PacketDirectedRelationV1],
    product: &RepositoryDerivedCompilationV1,
) -> RepositoryEvidenceSelectionRankV1 {
    const FRACTION_SCALE: u64 = 1_000_000;
    let admissions = ordered_admissions(input);
    let admission_order = admissions
        .iter()
        .enumerate()
        .map(|(index, admission)| (admission.stable_identity.as_str(), index))
        .collect::<HashMap<_, _>>();
    let retained = product
        .support
        .iter()
        .filter(|row| row.kind == SupportUnitKindDto::SourceRange)
        .collect::<Vec<_>>();
    // Multiple witnesses for one identity must agree on the original extent
    // and source digest. A conflicting or missing denominator stays unknown.
    let mut original_bounds = HashMap::new();
    for source in &input.sources {
        let bounds = source
            .delivery
            .as_ref()
            .filter(|delivery| delivery.matches_delivery(source.start_line, source.end_line))
            .map(|delivery| {
                (
                    source.path.as_str(),
                    delivery.source_sha256.as_str(),
                    delivery.requested_range,
                )
            });
        original_bounds
            .entry(source.stable_identity.as_str())
            .and_modify(|known| {
                if *known != bounds {
                    *known = None;
                }
            })
            .or_insert(bounds);
    }
    // Original callable extents stay independent of a clipped requested page.
    // Metadata must agree across same-identity witnesses; conflicts stay unknown.
    let mut callable_units = HashMap::new();
    for source in &input.sources {
        let unit = source
            .callable_body
            .as_ref()
            .zip(source.delivery.as_ref())
            .filter(|(body, delivery)| {
                delivery.matches_delivery(source.start_line, source.end_line)
                    && body.matches_delivery(delivery)
            })
            .map(|(body, _)| (source.path.as_str(), body));
        callable_units
            .entry(source.stable_identity.as_str())
            .and_modify(|known| {
                if *known != unit {
                    *known = None;
                }
            })
            .or_insert(unit);
    }
    let mut body_fractions = vec![0_u64; admissions.len()];
    let mut complete_callable_units = 0usize;
    let mut fractions = vec![0_u64; admissions.len()];
    let mut delivered_lines = vec![0_u64; admissions.len()];
    let mut represented = BTreeSet::new();
    for selected in selected_source_ranges(input, &admission_order) {
        let witness = selected.witness;
        let row = source_support_unit(witness);
        if !retained.iter().any(|retained| {
            retained.id == row.id
                && retained.path == row.path
                && retained.start_line == row.start_line
                && retained.end_line == row.end_line
                && retained.snippet == row.snippet
                && retained.source_delivery == row.source_delivery
        }) {
            continue;
        }
        let mut complete_physical_callable = false;
        for identity in &selected.represented_identities {
            let index = admission_order[identity.as_str()];
            if let Some(Some((path, body))) = callable_units.get(identity.as_str())
                && *path == witness.path
                && witness
                    .delivery
                    .as_ref()
                    .is_some_and(|d| d.source_sha256 == body.source_sha256)
            {
                let start = witness.start_line.max(body.body_range.start_line);
                let end = witness.end_line.min(body.body_range.end_line);
                if start <= end {
                    let lines = u64::from(end) - u64::from(start) + 1;
                    let total = u64::from(body.body_range.end_line)
                        - u64::from(body.body_range.start_line)
                        + 1;
                    body_fractions[index] =
                        body_fractions[index].max(lines * FRACTION_SCALE / total);
                }
                complete_physical_callable |= witness.start_line <= body.unit_range.start_line
                    && witness.end_line >= body.unit_range.end_line;
            }
            represented.insert(index);
            let Some(Some((_, _, requested))) = original_bounds.get(identity.as_str()) else {
                continue;
            };
            // Missing original requested bounds stays unknown. In particular,
            // a contained hydrated prefix is not the alias's complete body.
            for alias in input
                .sources
                .iter()
                .filter(|source| source.stable_identity == *identity && source.path == witness.path)
            {
                let Some(delivery) = &alias.delivery else {
                    continue;
                };
                if !delivery.matches_delivery(alias.start_line, alias.end_line)
                    || !witness
                        .delivery
                        .as_ref()
                        .is_some_and(|d| d.source_sha256 == delivery.source_sha256)
                {
                    continue;
                }
                let start = witness.start_line.max(requested.start_line);
                let end = witness.end_line.min(requested.end_line);
                if end < start {
                    continue;
                }
                let lines = u64::from(end) - u64::from(start) + 1;
                let total = u64::from(requested.end_line) - u64::from(requested.start_line) + 1;
                fractions[index] = fractions[index].max(lines * FRACTION_SCALE / total);
                delivered_lines[index] = delivered_lines[index].max(lines);
            }
        }
        complete_callable_units += usize::from(complete_physical_callable);
    }

    let mut reach = fractions.clone();
    for link in navigation.iter().take(
        INTERIM_MAX_ADMITTED_CANDIDATES
            * codestory_contracts::compilation::PACKET_CALL_DESCRIPTOR_MAX_EDGES_V1,
    ) {
        if link.relation_kind != codestory_contracts::compilation::PacketRelationKindV1::Call
            || link.from_identity == link.to_identity
        {
            continue;
        }
        if let (Some(&from), Some(&to)) = (
            admission_order.get(link.from_identity.as_str()),
            admission_order.get(link.to_identity.as_str()),
        ) {
            reach[from] = reach[from].max(fractions[to]);
        }
    }
    // The ordinal is sealed post-promotion admission priority, not the full
    // retrieval rank. Reciprocal weights permit breadth to beat a lone seed.
    let weighted = |values: &[u64]| {
        values
            .iter()
            .enumerate()
            .map(|(index, fraction)| fraction * (FRACTION_SCALE / (index as u64 + 1)))
            .sum::<u64>()
    };
    let requested = admissions
        .iter()
        .enumerate()
        .filter(|(_, a)| a.origin == PacketAdmissionOriginV1::ExactTypedSelector)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let rows = product
        .support
        .iter()
        .filter(|row| row.kind == SupportUnitKindDto::SourceRange)
        .collect::<Vec<_>>();
    RepositoryEvidenceSelectionRankV1 {
        requested_retained: requested
            .iter()
            .filter(|&&i| represented.contains(&i))
            .count(),
        requested_complete: requested
            .iter()
            .filter(|&&i| fractions[i] == FRACTION_SCALE)
            .count(),
        requested_delivered_lines: requested.iter().map(|&i| delivered_lines[i]).sum(),
        direct_body_reach: weighted(&body_fractions),
        complete_callable_units,
        admitted_reach: weighted(&reach),
        direct_reach: weighted(&fractions),
        complete_sources: rows
            .iter()
            .filter(|row| {
                row.source_delivery
                    .as_ref()
                    .is_some_and(|d| d.remaining_source_bytes == 0)
            })
            .count(),
        source_rows: rows.len(),
        source_bytes: rows
            .iter()
            .map(|row| row.snippet.as_ref().map_or(0, String::len))
            .sum(),
    }
}

fn compile_repository_evidence_with_row_limit(
    input: &PacketCompilationInputV1,
    row_limit: usize,
) -> RepositoryDerivedCompilationV1 {
    compile_repository_evidence_with_row_allocation(input, row_limit, true)
}

fn compile_repository_evidence_with_row_allocation(
    input: &PacketCompilationInputV1,
    row_limit: usize,
    reserve_graph_row: bool,
) -> RepositoryDerivedCompilationV1 {
    compile_repository_evidence_with_source_order(input, row_limit, reserve_graph_row, false)
}

fn compile_repository_evidence_with_source_order(
    input: &PacketCompilationInputV1,
    row_limit: usize,
    reserve_graph_row: bool,
    complete_first: bool,
) -> RepositoryDerivedCompilationV1 {
    compile_repository_evidence_with_source_order_and_navigation(
        input,
        row_limit,
        reserve_graph_row,
        complete_first,
        &[],
    )
}

fn compile_repository_evidence_with_source_order_and_navigation(
    input: &PacketCompilationInputV1,
    row_limit: usize,
    reserve_graph_row: bool,
    complete_first: bool,
    navigation: &[PacketDirectedRelationV1],
) -> RepositoryDerivedCompilationV1 {
    compile_repository_evidence_with_options(
        input,
        row_limit,
        reserve_graph_row,
        complete_first,
        navigation,
        None,
    )
}

fn compile_repository_evidence_with_options(
    input: &PacketCompilationInputV1,
    row_limit: usize,
    reserve_graph_row: bool,
    complete_first: bool,
    navigation: &[PacketDirectedRelationV1],
    subset: Option<&[SupportUnitDto]>,
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
    let mut sources = selected_source_ranges(input, &admission_order);
    let mut navigation_reordered = false;
    if complete_first && !navigation.is_empty() {
        (sources, navigation_reordered) =
            order_sources_by_navigation(sources, &admissions, navigation, &input.sources);
    }
    if complete_first && !navigation_reordered {
        // Only admitted physical-delivery metadata determines this alternative.
        // It competes through the same full-frame measurement as the originals.
        let exact = admissions
            .iter()
            .filter(|a| a.origin == PacketAdmissionOriginV1::ExactTypedSelector)
            .map(|a| a.stable_identity.as_str())
            .collect::<BTreeSet<_>>();
        sources.sort_by_cached_key(|source| {
            let authenticated_exact = source
                .represented_identities
                .iter()
                .any(|id| exact.contains(id.as_str()));
            let complete = source
                .witness
                .delivery
                .as_ref()
                .is_some_and(|d| d.remaining_source_bytes == 0);
            let row_bytes = serde_json::to_vec(&source_support_unit(source.witness))
                .map_or(usize::MAX, |bytes| bytes.len());
            // Preserve the ordinal among exact selections, including incomplete
            // pages; unrelated cheap context cannot displace a requested page.
            if authenticated_exact {
                (
                    0,
                    0,
                    0,
                    admission_order[source.witness.stable_identity.as_str()],
                )
            } else {
                (
                    1,
                    usize::from(!complete),
                    row_bytes,
                    admission_order[source.witness.stable_identity.as_str()],
                )
            }
        });
    }
    if complete_first {
        // Align this existing proposal family with the final evidence objective.
        // An indirect-reach prefix can otherwise omit every executable body,
        // beyond the bounded single-exchange neighborhood's ability to recover.
        // Exact selections keep their current order; unknown body metadata and
        // equal body ranks keep the preceding navigation/compact ordering.
        let exact = admissions
            .iter()
            .filter(|a| a.origin == PacketAdmissionOriginV1::ExactTypedSelector)
            .map(|a| a.stable_identity.as_str())
            .collect::<BTreeSet<_>>();
        sources.sort_by_cached_key(|source| {
            let is_exact = source
                .represented_identities
                .iter()
                .any(|id| exact.contains(id.as_str()));
            let singleton = RepositoryDerivedCompilationV1 {
                support: vec![source_support_unit(source.witness)],
                continuation: Vec::new(),
            };
            let rank = repository_evidence_selection_rank(input, navigation, &singleton);
            (
                usize::from(!is_exact),
                std::cmp::Reverse(if is_exact { 0 } else { rank.direct_body_reach }),
                std::cmp::Reverse(if is_exact {
                    0
                } else {
                    rank.complete_callable_units
                }),
            )
        });
    }
    let relations = selected_relations(input, &admission_order);
    let mut support = Vec::new();
    let mut source_budget_omissions = BTreeSet::new();
    let mut relation_budget_omissions = BTreeSet::new();
    let mut represented_identities = BTreeSet::new();
    let mut retained_source_identities = BTreeSet::new();
    let mut retained_source_pages = Vec::new();

    // The default family reserves a graph witness. The measured source-first
    // alternative fills those same bounded rows with path-diverse sources,
    // then retains graph context only where rows remain.
    let source_row_limit = row_limit.saturating_sub(usize::from(
        reserve_graph_row && !relations.is_empty() && row_limit > 1,
    ));
    let mut source_index = 0;
    let mut relation_index = 0;
    while source_index < sources.len() || relation_index < relations.len() {
        if let Some(source) = sources.get(source_index) {
            let row = source_support_unit(source.witness);
            if subset.is_none_or(|chosen| chosen.contains(&row))
                && push_if_within_public_budget(&mut support, row, source_row_limit)
            {
                retained_source_pages.push(source.witness);
                represented_identities.insert(source.witness.stable_identity.clone());
                retained_source_identities.extend(source.represented_identities.iter().cloned());
            } else {
                source_budget_omissions.extend(source.represented_identities.iter().cloned());
            }
            source_index += 1;
        }

        if source_index >= sources.len()
            && let Some(relation) = relations.get(relation_index)
        {
            let row = relation_support_unit(relation);
            if subset.is_none_or(|chosen| chosen.contains(&row))
                && push_if_within_public_budget(&mut support, row, row_limit)
            {
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

fn order_sources_by_navigation<'a>(
    sources: Vec<SelectedSourceRange<'a>>,
    admissions: &[&codestory_contracts::compilation::PacketAdmissionReceiptV1],
    navigation: &[PacketDirectedRelationV1],
    original_sources: &[PacketHydratedSourceRangeV1],
) -> (Vec<SelectedSourceRange<'a>>, bool) {
    let ranks = admissions
        .iter()
        .enumerate()
        .map(|(i, a)| (a.stable_identity.as_str(), i))
        .collect::<HashMap<_, _>>();
    let exact = admissions
        .iter()
        .filter(|a| a.origin == PacketAdmissionOriginV1::ExactTypedSelector)
        .map(|a| a.stable_identity.as_str())
        .collect::<BTreeSet<_>>();
    let mut incoming = HashMap::<&str, BTreeSet<usize>>::new();
    for link in navigation.iter().take(
        INTERIM_MAX_ADMITTED_CANDIDATES
            * codestory_contracts::compilation::PACKET_CALL_DESCRIPTOR_MAX_EDGES_V1,
    ) {
        if link.relation_kind != codestory_contracts::compilation::PacketRelationKindV1::Call
            || link.from_identity == link.to_identity
            || !ranks.contains_key(link.to_identity.as_str())
        {
            continue;
        }
        if let Some(&rank) = ranks.get(link.from_identity.as_str()) {
            incoming.entry(&link.to_identity).or_default().insert(rank);
        }
    }
    let (mut ordered, mut remaining): (Vec<_>, Vec<_>) = sources.into_iter().partition(|s| {
        s.represented_identities
            .iter()
            .any(|id| exact.contains(id.as_str()))
    });
    let mut covered = BTreeSet::<usize>::new();
    for source in &ordered {
        for id in &source.represented_identities {
            if let Some(seeds) = incoming.get(id.as_str()) {
                covered.extend(seeds);
            }
        }
    }
    let mut reordered = false;
    loop {
        let winner = remaining
            .iter()
            .enumerate()
            .map(|(i, source)| {
                let seeds = source
                    .represented_identities
                    .iter()
                    .filter_map(|id| incoming.get(id.as_str()))
                    .flatten()
                    .copied()
                    .collect::<BTreeSet<_>>();
                let gain = seeds
                    .iter()
                    .filter(|s| !covered.contains(s))
                    .map(|s| 1.0 / (*s as f64 + 1.0))
                    .sum::<f64>();
                (i, gain, seeds)
            })
            .max_by(|(a, x, _), (b, y, _)| x.total_cmp(y).then_with(|| b.cmp(a)));
        let Some((index, gain, seeds)) = winner else {
            break;
        };
        if gain <= 0.0 {
            break;
        }
        reordered = true;
        covered.extend(seeds);
        ordered.push(remaining.remove(index));
    }
    if reordered {
        // Navigation has exhausted its positive marginal gain. Restore the
        // compact-complete alternative for this unrequested tail, leaving the
        // exact and positive-gain prefixes intact. Unit cost is only a proposal
        // heuristic; the caller still measures the complete public frame.
        remaining.sort_by_cached_key(|source| {
            let complete = source_has_consistent_complete_delivery(source, original_sources);
            let row_bytes = serde_json::to_vec(&source_support_unit(source.witness))
                .map_or(usize::MAX, |bytes| bytes.len());
            let ordinal = source
                .represented_identities
                .iter()
                .filter_map(|id| ranks.get(id.as_str()))
                .min()
                .copied()
                .unwrap_or(usize::MAX);
            (usize::from(!complete), row_bytes, ordinal)
        });
    }
    ordered.extend(remaining);
    (ordered, reordered)
}

fn source_has_consistent_complete_delivery(
    source: &SelectedSourceRange<'_>,
    original_sources: &[PacketHydratedSourceRangeV1],
) -> bool {
    let witness = source.witness;
    let Some(delivery) = witness.delivery.as_ref() else {
        return false;
    };
    if delivery.remaining_source_bytes != 0
        || !delivery.matches_delivery(witness.start_line, witness.end_line)
    {
        return false;
    }
    // A duplicate identity with missing, invalid or conflicting bounds keeps
    // completion unknown. A small alias cannot complete a larger original.
    original_sources
        .iter()
        .filter(|candidate| {
            source
                .represented_identities
                .contains(&candidate.stable_identity)
        })
        .all(|candidate| {
            candidate.path == witness.path
                && candidate.delivery.as_ref().is_some_and(|other| {
                    other.matches_delivery(candidate.start_line, candidate.end_line)
                        && other.requested_range == delivery.requested_range
                        && other.source_sha256 == delivery.source_sha256
                })
        })
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

// Keep source/relation omissions and identity/page retention separate at this boundary.
#[allow(clippy::too_many_arguments)]
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

    #[test]
    fn published_call_source_navigation_is_generic_and_never_graph_proof() {
        let mut fixture = input();
        fixture.relations.clear();
        fixture.admissions = (0..6)
            .map(|i| PacketAdmissionReceiptV1 {
                packet_ordinal: i,
                stable_identity: format!("node:{i}"),
                score_version: "v1".into(),
                reserved_source_bytes: 512,
                origin: PacketAdmissionOriginV1::Retrieval,
            })
            .collect();
        fixture.sources = (0..6)
            .map(|i| PacketHydratedSourceRangeV1 {
                callable_body: None,
                stable_identity: format!("node:{i}"),
                path: "src/anonymous".into(),
                symbol: Some(format!("opaque_{i}")),
                start_line: i + 1,
                end_line: i + 1,
                source: if i >= 4 { "x".repeat(200) } else { "x".into() },
                delivery: None,
                parser_completeness: PacketParserCompletenessV1::Complete,
            })
            .collect();
        let links = [
            PacketDirectedRelationV1 {
                relation_id: "hint1".into(),
                from_identity: "node:0".into(),
                to_identity: "node:4".into(),
                relation_kind: PacketRelationKindV1::Call,
                certainty: PacketRelationCertaintyV1::Probable,
            },
            PacketDirectedRelationV1 {
                relation_id: "hint2".into(),
                from_identity: "node:1".into(),
                to_identity: "node:5".into(),
                relation_kind: PacketRelationKindV1::Call,
                certainty: PacketRelationCertaintyV1::Uncertain,
            },
        ];
        let product = compile_repository_evidence_with_source_order_and_navigation(
            &fixture, 2, false, true, &links,
        );
        assert_eq!(
            product
                .support
                .iter()
                .map(|s| s.symbol_id.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("4"), Some("5")]
        );
        assert!(
            product
                .support
                .iter()
                .all(|s| s.kind == SupportUnitKindDto::SourceRange),
            "navigation hints cannot emit certain graph proof"
        );
        // Exercise the public measured seam: a fitting related-source proposal
        // must be selectable without promoting either uncertain hint to proof.
        let measured = compile_repository_evidence_for_representation_with_navigation(
            &fixture,
            &links,
            |candidate| {
                Ok::<_, ()>(
                    (candidate.support.len() == 2
                        && candidate
                            .support
                            .iter()
                            .all(|row| matches!(row.symbol_id.as_deref(), Some("4" | "5"))))
                    .then_some(4096),
                )
            },
        )
        .unwrap();
        assert!(
            matches!(measured, RepresentationBudgetSelectionV1::Complete { product: selected, measured_candidates, .. } if selected == product && measured_candidates <= REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1)
        );
        // Structural coverage has no relevance oracle: a shared target may
        // cover both seeds and precede their independent targets. Keep this
        // counterexample visible without naming a helper role in production.
        let mut hub = links.to_vec();
        hub[1].to_identity = "node:4".into();
        let mut independent = links.to_vec();
        independent[0].to_identity = "node:2".into();
        independent[1].to_identity = "node:3".into();
        hub.extend(independent);
        let shared = compile_repository_evidence_with_source_order_and_navigation(
            &fixture, 1, false, true, &hub,
        );
        assert_eq!(shared.support[0].symbol_id.as_deref(), Some("4"));

        let mut repeated = links.to_vec();
        repeated.extend_from_slice(&links);
        assert_eq!(
            product,
            compile_repository_evidence_with_source_order_and_navigation(
                &fixture, 2, false, true, &repeated
            )
        );
        for source in &mut fixture.sources {
            source.symbol = Some("different spelling".into());
            source.path = "other/language.ext".into();
        }
        let changed = compile_repository_evidence_with_source_order_and_navigation(
            &fixture, 2, false, true, &links,
        );
        assert_eq!(
            changed
                .support
                .iter()
                .map(|s| s.symbol_id.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("4"), Some("5")]
        );
        fixture.admissions[2].origin = PacketAdmissionOriginV1::ExactTypedSelector;
        let exact = compile_repository_evidence_with_source_order_and_navigation(
            &fixture, 2, false, true, &links,
        );
        assert_eq!(
            exact.support[0].symbol_id.as_deref(),
            Some("2"),
            "typed exact source priority is preserved"
        );
    }

    #[test]
    fn published_call_inactive_navigation_preserves_physical_completion_family() {
        let mut fixture = input();
        let baseline = compile_repository_evidence_with_source_order_and_navigation(
            &fixture,
            2,
            false,
            true,
            &[],
        );
        let mut link = PacketDirectedRelationV1 {
            relation_id: "hint".into(),
            from_identity: "node:1".into(),
            to_identity: "node:999".into(),
            relation_kind: PacketRelationKindV1::Call,
            certainty: PacketRelationCertaintyV1::Probable,
        };
        assert_eq!(
            baseline,
            compile_repository_evidence_with_source_order_and_navigation(
                &fixture,
                2,
                false,
                true,
                &[link.clone()]
            )
        );
        link.to_identity = fixture.admissions[1].stable_identity.clone();
        link.from_identity = fixture.admissions[0].stable_identity.clone();
        link.relation_kind = PacketRelationKindV1::Unknown;
        assert_eq!(
            baseline,
            compile_repository_evidence_with_source_order_and_navigation(
                &fixture,
                2,
                false,
                true,
                &[link.clone()]
            )
        );
        link.relation_kind = PacketRelationKindV1::Call;
        fixture
            .sources
            .retain(|s| s.stable_identity != link.to_identity);
        let no_target = compile_repository_evidence_with_source_order_and_navigation(
            &fixture,
            2,
            false,
            true,
            &[],
        );
        assert_eq!(
            no_target,
            compile_repository_evidence_with_source_order_and_navigation(
                &fixture,
                2,
                false,
                true,
                &[link]
            )
        );
    }

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
                    callable_body: None,
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
                    callable_body: None,
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
                    callable_body: None,
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

    fn selection_fraction_fixture() -> PacketCompilationInputV1 {
        let mut value = row_mix_input();
        value.relations.clear();
        value.admissions = (0..6)
            .map(|i| PacketAdmissionReceiptV1 {
                packet_ordinal: i,
                stable_identity: format!("node:{i}"),
                score_version: "anonymous".into(),
                reserved_source_bytes: 512,
                origin: PacketAdmissionOriginV1::Retrieval,
            })
            .collect();
        value.sources = (0..6)
            .map(|i| PacketHydratedSourceRangeV1 {
                callable_body: None,
                stable_identity: format!("node:{i}"),
                path: format!("src/{i}"),
                symbol: Some(format!("opaque_{i}")),
                start_line: 1,
                end_line: 1,
                source: if i >= 4 { "x".repeat(300) } else { "x".into() },
                delivery: Some(codestory_contracts::compilation::PacketSourceDeliveryV1 {
                    requested_range: codestory_contracts::compilation::PacketSourceRangeV1 {
                        start_line: 1,
                        end_line: 1,
                    },
                    source_sha256: "a".repeat(64),
                    remaining_source_bytes: 0,
                    next_start_line: None,
                    blocked_line_rendered_bytes: None,
                }),
                parser_completeness: PacketParserCompletenessV1::Complete,
            })
            .collect();
        value
    }

    #[test]
    fn accepted_baseline_is_independent_of_callable_metadata() {
        let plain = selection_fraction_fixture();
        let mut annotated = plain.clone();
        for source in &mut annotated.sources {
            anonymous_body(source, 1, 1);
        }
        let mut plain_calls = 0;
        let mut annotated_calls = 0;
        let plain_result =
            compile_repository_evidence_baseline_for_representation(&plain, &[], |p| {
                plain_calls += 1;
                Ok::<_, ()>(Some(if p.support.len() > 2 { 16385 } else { 8000 }))
            })
            .unwrap();
        let annotated_result =
            compile_repository_evidence_baseline_for_representation(&annotated, &[], |p| {
                annotated_calls += 1;
                Ok::<_, ()>(Some(if p.support.len() > 2 { 16385 } else { 8000 }))
            })
            .unwrap();
        assert_eq!(plain_result, annotated_result);
        assert_eq!(plain_calls, annotated_calls);
        assert!(plain_calls <= 3 * (INTERIM_MAX_ADMITTED_CANDIDATES + 1));
        let refused =
            compile_repository_evidence_baseline_for_representation(&annotated, &[], |_| {
                Ok::<_, ()>(None)
            })
            .unwrap();
        assert!(matches!(
            refused,
            RepresentationBudgetSelectionV1::BudgetExceeded { .. }
        ));
    }

    fn anonymous_body(source: &mut PacketHydratedSourceRangeV1, start: u32, end: u32) {
        let delivery = source.delivery.as_ref().unwrap();
        source.callable_body = Some(codestory_contracts::compilation::PacketCallableBodyV1 {
            unit_range: delivery.requested_range,
            body_range: codestory_contracts::compilation::PacketSourceRangeV1 {
                start_line: start,
                end_line: end,
            },
            source_sha256: delivery.source_sha256.clone(),
        });
    }

    fn source_product(
        value: &PacketCompilationInputV1,
        indices: &[usize],
    ) -> RepositoryDerivedCompilationV1 {
        RepositoryDerivedCompilationV1 {
            support: indices
                .iter()
                .map(|i| source_support_unit(&value.sources[*i]))
                .collect(),
            continuation: Vec::new(),
        }
    }

    #[test]
    fn joint_partial_body_progress_preserves_baseline_and_honest_tail() {
        let mut value = selection_fraction_fixture();
        value.sources.truncate(3);
        value.admissions.truncate(3);
        value.sources[0].end_line = 4;
        value.sources[0].source = "line1\nline2\nline3\nline4".into();
        let delivery = value.sources[0].delivery.as_mut().unwrap();
        delivery.requested_range.end_line = 10;
        delivery.remaining_source_bytes = 36;
        delivery.next_start_line = Some(5);
        anonymous_body(&mut value.sources[0], 2, 9);
        let baseline_rows = source_product(&value, &[1, 2]);
        let baseline = compile_repository_evidence_with_options(
            &value,
            2,
            false,
            false,
            &[],
            Some(&baseline_rows.support),
        );
        let selected =
            compile_repository_evidence_joint_additions(&value, &baseline, |candidate| {
                assert!(
                    baseline
                        .support
                        .iter()
                        .all(|row| candidate.support.contains(row))
                );
                Ok::<_, ()>(Some(serde_json::to_vec(&candidate.support).unwrap().len()))
            })
            .unwrap();
        let JointRepresentationSelectionV1::Complete {
            product,
            measured_candidates,
            ..
        } = selected
        else {
            panic!("positive body progress fits");
        };
        assert_eq!(product.support.len(), 3);
        assert!(
            product
                .support
                .contains(&source_support_unit(&value.sources[0]))
        );
        assert!(product.continuation.iter().any(|c| {
            c.stable_identity == "node:0"
                && c.source_range
                    .is_some_and(|r| r.start_line == 5 && r.end_line == 10)
        }));
        assert!(measured_candidates <= REPOSITORY_EVIDENCE_JOINT_CALLBACKS_MAX);
        for refusal in 0..4 {
            let mut hostile = value.clone();
            match refusal {
                0 => hostile.sources[0].callable_body = None,
                1 => {
                    hostile.sources[0]
                        .callable_body
                        .as_mut()
                        .unwrap()
                        .source_sha256 = "b".repeat(64)
                }
                2 => {
                    hostile.sources[0]
                        .callable_body
                        .as_mut()
                        .unwrap()
                        .body_range
                        .start_line = 5
                }
                _ => {
                    let mut alias = hostile.sources[0].clone();
                    alias.callable_body.as_mut().unwrap().body_range.start_line = 3;
                    hostile.sources.push(alias);
                }
            }
            let result = compile_repository_evidence_joint_additions(&hostile, &baseline, |_| {
                Ok::<_, ()>(Some(4096))
            })
            .unwrap();
            assert!(
                matches!(result, JointRepresentationSelectionV1::Complete { product, .. } if product == baseline),
                "unknown, drifted, header-only or conflicted pages cannot claim body progress"
            );
        }
    }

    #[test]
    fn joint_body_progress_subtracts_coverage_union_without_alias_inflation() {
        let mut value = selection_fraction_fixture();
        value.sources.truncate(1);
        value.sources[0].end_line = 4;
        let d = value.sources[0].delivery.as_mut().unwrap();
        d.requested_range.end_line = 10;
        d.remaining_source_bytes = 36;
        d.next_start_line = Some(5);
        anonymous_body(&mut value.sources[0], 2, 9);
        value.sources.push(value.sources[0].clone());
        let mut incoming = source_product(&value, &[0]);
        incoming.support.push(incoming.support[0].clone());
        let empty = RepositoryDerivedCompilationV1 {
            support: Vec::new(),
            continuation: Vec::new(),
        };
        assert_eq!(
            newly_uncovered_callable_body_lines(&value, &empty, &incoming),
            3
        );
        let old_row = |start, end| {
            let mut source = value.sources[0].clone();
            source.start_line = start;
            source.end_line = end;
            source.delivery.as_mut().unwrap().requested_range =
                codestory_contracts::compilation::PacketSourceRangeV1 {
                    start_line: start,
                    end_line: end,
                };
            source.delivery.as_mut().unwrap().remaining_source_bytes = 0;
            source.delivery.as_mut().unwrap().next_start_line = None;
            source_support_unit(&source)
        };
        let covered = RepositoryDerivedCompilationV1 {
            support: vec![old_row(2, 3), old_row(3, 4)],
            continuation: Vec::new(),
        };
        assert_eq!(
            newly_uncovered_callable_body_lines(&value, &covered, &incoming),
            0
        );
        let partial = RepositoryDerivedCompilationV1 {
            support: vec![old_row(2, 2), old_row(4, 4)],
            continuation: Vec::new(),
        };
        assert_eq!(
            newly_uncovered_callable_body_lines(&value, &partial, &incoming),
            1
        );
        let mut tiny = value.clone();
        tiny.sources.truncate(1);
        tiny.sources[0]
            .delivery
            .as_mut()
            .unwrap()
            .requested_range
            .end_line = u32::MAX;
        tiny.sources[0]
            .callable_body
            .as_mut()
            .unwrap()
            .unit_range
            .end_line = u32::MAX;
        tiny.sources[0]
            .callable_body
            .as_mut()
            .unwrap()
            .body_range
            .end_line = u32::MAX;
        assert_eq!(
            newly_uncovered_callable_body_lines(&tiny, &partial, &source_product(&tiny, &[0])),
            1,
            "real new lines remain positive without fraction rounding"
        );
    }

    #[test]
    fn joint_partial_aliases_require_physical_unit_consensus_across_prefixes() {
        let mut value = selection_fraction_fixture();
        value.sources.truncate(1);
        value.sources[0].end_line = 2;
        let d = value.sources[0].delivery.as_mut().unwrap();
        d.requested_range.end_line = 10;
        d.remaining_source_bytes = 40;
        d.next_start_line = Some(3);
        anonymous_body(&mut value.sources[0], 2, 9);
        let mut alias = value.sources[0].clone();
        alias.stable_identity = "node:1".into();
        alias.end_line = 3;
        alias.delivery.as_mut().unwrap().remaining_source_bytes = 30;
        alias.delivery.as_mut().unwrap().next_start_line = Some(4);
        alias.callable_body.as_mut().unwrap().body_range.start_line = 3;
        value.sources.push(alias);
        assert!(authenticated_callable_progress_key(&value, &value.sources[0]).is_none());
        assert!(authenticated_callable_progress_key(&value, &value.sources[1]).is_none());
        value.sources[1].callable_body = value.sources[0].callable_body.clone();
        value.sources[1]
            .callable_body
            .as_mut()
            .unwrap()
            .source_sha256 = "b".repeat(64);
        assert!(
            authenticated_callable_progress_key(&value, &value.sources[0]).is_none(),
            "shared authenticated delivery/unit cannot hide conflicting body hash"
        );
        assert!(authenticated_callable_progress_key(&value, &value.sources[1]).is_none());
        value.sources[1].callable_body = value.sources[0].callable_body.clone();
        assert!(authenticated_callable_progress_key(&value, &value.sources[0]).is_some());
        assert!(authenticated_callable_progress_key(&value, &value.sources[1]).is_some());
        let empty = RepositoryDerivedCompilationV1 {
            support: Vec::new(),
            continuation: Vec::new(),
        };
        assert_eq!(
            newly_uncovered_callable_body_lines(&value, &empty, &source_product(&value, &[0, 1])),
            2
        );
    }

    #[test]
    fn joint_complete_additions_keep_baseline_and_find_pair_after_single_refusals() {
        let mut value = selection_fraction_fixture();
        value.sources.truncate(3);
        value.admissions.truncate(3);
        for source in &mut value.sources {
            anonymous_body(source, 1, 1);
        }
        let baseline_rows = source_product(&value, &[0]);
        let baseline = compile_repository_evidence_with_options(
            &value,
            1,
            false,
            false,
            &[],
            Some(&baseline_rows.support),
        );
        for reverse in [false, true] {
            if reverse {
                value.sources.reverse();
                value.admissions.reverse();
            }
            let mut calls = 0;
            let selected = compile_repository_evidence_joint_additions(&value, &baseline, |p| {
                calls += 1;
                assert!(baseline.support.iter().all(|row| p.support.contains(row)));
                Ok::<_, ()>((p.support.len() != 2).then_some(4096))
            })
            .unwrap();
            let JointRepresentationSelectionV1::Complete {
                product,
                measured_candidates,
                ..
            } = selected
            else {
                panic!("joint pair fits");
            };
            assert_eq!(product.support.len(), 3);
            assert_eq!(calls, measured_candidates);
            assert!(calls <= REPOSITORY_EVIDENCE_JOINT_CALLBACKS_MAX);
        }
    }

    #[test]
    fn joint_complete_additions_fail_closed_on_drift_unknown_and_stale_final_frame() {
        let mut value = selection_fraction_fixture();
        value.sources.truncate(3);
        value.admissions.truncate(3);
        for source in &mut value.sources {
            anonymous_body(source, 1, 1);
        }
        let baseline_rows = source_product(&value, &[0]);
        let baseline = compile_repository_evidence_with_options(
            &value,
            1,
            false,
            false,
            &[],
            Some(&baseline_rows.support),
        );
        for conflict in [false, true] {
            let mut altered = value.clone();
            altered.sources[1].callable_body = None;
            altered.sources[2].callable_body = None;
            if conflict {
                altered.sources.push(altered.sources[1].clone());
            }
            let selected = compile_repository_evidence_joint_additions(&altered, &baseline, |_| {
                Ok::<_, ()>(Some(4096))
            })
            .unwrap();
            assert!(
                matches!(selected, JointRepresentationSelectionV1::Complete { product, .. } if product == baseline)
            );
        }
        let mut forged = baseline.clone();
        forged.continuation.clear();
        assert_ne!(forged.continuation, baseline.continuation);
        let rejected = compile_repository_evidence_joint_additions(&value, &forged, |_| {
            panic!("unauthenticated selector must not reach the renderer");
            #[allow(unreachable_code)]
            Ok::<_, ()>(Some(4096))
        })
        .unwrap();
        assert_eq!(rejected, JointRepresentationSelectionV1::InvalidBaseline);
        let mut drifted = value.clone();
        drifted.sources[0].source.push_str("changed");
        let mut calls = 0;
        let selected = compile_repository_evidence_joint_additions(&drifted, &baseline, |_| {
            calls += 1;
            Ok::<_, ()>(Some(4096))
        })
        .unwrap();
        assert!(matches!(
            selected,
            JointRepresentationSelectionV1::InvalidBaseline
        ));
        assert_eq!(calls, 0);
        let mut calls = 0;
        let selected = compile_repository_evidence_joint_additions(&value, &baseline, |_| {
            calls += 1;
            // One baseline, two singles, one pair, then failed winner/fallback.
            Ok::<_, ()>((calls <= 4).then_some(4096))
        })
        .unwrap();
        assert!(matches!(
            selected,
            JointRepresentationSelectionV1::NoAdmissibleFrame {
                measured_candidates: 6
            }
        ));
        assert_eq!(calls, 6);
    }

    #[test]
    fn direct_bodies_beat_interface_heavy_reach_and_keep_small_worker_and_policies() {
        let mut value = selection_fraction_fixture();
        for source in &mut value.sources {
            source.source = "x".repeat(40);
        }
        for i in 3..6 {
            anonymous_body(&mut value.sources[i], 1, 1);
        }
        let links = (1..6).map(|i| anonymous_call(i, 0)).collect::<Vec<_>>();
        let policies_and_worker = source_product(&value, &[3, 4, 5]);
        let interfaces = source_product(&value, &[0, 1, 2]);
        assert!(
            repository_evidence_selection_rank(&value, &links, &policies_and_worker)
                > repository_evidence_selection_rank(&value, &links, &interfaces)
        );
        assert_eq!(
            repository_evidence_selection_rank(&value, &links, &policies_and_worker)
                .complete_callable_units,
            3
        );
        for reverse in [false, true] {
            if reverse {
                value.sources.reverse();
                value.admissions.reverse();
            }
            let selected = compile_repository_evidence_for_representation_with_navigation(
                &value,
                &links,
                |product| {
                    let bytes = 11000
                        + 1700 * product.support.len()
                        + product
                            .support
                            .iter()
                            .map(|r| r.snippet.as_ref().map_or(0, String::len))
                            .sum::<usize>();
                    Ok::<_, ()>(Some(bytes))
                },
            )
            .unwrap();
            let RepresentationBudgetSelectionV1::Complete { product, .. } = selected else {
                panic!("three bodies fit")
            };
            let bodies = product
                .support
                .iter()
                .filter_map(|r| r.symbol_id.as_deref())
                .collect::<BTreeSet<_>>();
            assert_eq!(bodies, BTreeSet::from(["3", "4", "5"]));
        }
    }

    #[test]
    fn body_progress_does_not_count_header_or_complete_requested_slice_as_whole_callable() {
        let mut value = selection_fraction_fixture();
        let source = &mut value.sources[0];
        source.end_line = 3;
        source.delivery.as_mut().unwrap().requested_range.end_line = 3;
        source.callable_body = Some(codestory_contracts::compilation::PacketCallableBodyV1 {
            unit_range: codestory_contracts::compilation::PacketSourceRangeV1 {
                start_line: 1,
                end_line: 10,
            },
            body_range: codestory_contracts::compilation::PacketSourceRangeV1 {
                start_line: 5,
                end_line: 10,
            },
            source_sha256: "a".repeat(64),
        });
        let rank = repository_evidence_selection_rank(&value, &[], &source_product(&value, &[0]));
        assert_eq!(rank.direct_body_reach, 0);
        assert_eq!(rank.complete_callable_units, 0);
        value.sources[0].end_line = 7;
        value.sources[0]
            .delivery
            .as_mut()
            .unwrap()
            .requested_range
            .end_line = 7;
        let rank = repository_evidence_selection_rank(&value, &[], &source_product(&value, &[0]));
        assert!(rank.direct_body_reach > 0);
        assert_eq!(rank.complete_callable_units, 0);
        value.sources[0].end_line = 10;
        value.sources[0]
            .delivery
            .as_mut()
            .unwrap()
            .requested_range
            .end_line = 10;
        let rank = repository_evidence_selection_rank(&value, &[], &source_product(&value, &[0]));
        assert!(rank.direct_body_reach > 0);
        assert_eq!(rank.complete_callable_units, 1);
    }

    #[test]
    fn important_partial_body_beats_cheap_low_rank_complete_units() {
        let mut value = selection_fraction_fixture();
        value.sources[0].end_line = 8;
        let d = value.sources[0].delivery.as_mut().unwrap();
        d.requested_range.end_line = 10;
        d.remaining_source_bytes = 30;
        d.next_start_line = Some(9);
        anonymous_body(&mut value.sources[0], 1, 10);
        for i in 4..6 {
            anonymous_body(&mut value.sources[i], 1, 1);
        }
        let partial =
            repository_evidence_selection_rank(&value, &[], &source_product(&value, &[0]));
        let cheap =
            repository_evidence_selection_rank(&value, &[], &source_product(&value, &[4, 5]));
        assert_eq!(partial.complete_callable_units, 0);
        assert_eq!(cheap.complete_callable_units, 2);
        assert!(
            partial > cheap,
            "weighted direct progress must precede unweighted completion count"
        );
        value.admissions[1].origin = PacketAdmissionOriginV1::ExactTypedSelector;
        assert!(
            repository_evidence_selection_rank(&value, &[], &source_product(&value, &[1]))
                > partial,
            "unknown exact requests retain authority"
        );
    }

    #[test]
    fn callable_metadata_conflicts_missing_digest_and_duplicate_witnesses_stay_honest() {
        let mut value = selection_fraction_fixture();
        anonymous_body(&mut value.sources[0], 1, 1);
        let product = source_product(&value, &[0]);
        let original = repository_evidence_selection_rank(&value, &[], &product);
        assert_eq!(original.complete_callable_units, 1);
        value.sources.push(value.sources[0].clone());
        assert_eq!(
            repository_evidence_selection_rank(&value, &[], &product),
            original
        );
        value
            .sources
            .last_mut()
            .unwrap()
            .callable_body
            .as_mut()
            .unwrap()
            .source_sha256 = "b".repeat(64);
        let conflicted = repository_evidence_selection_rank(&value, &[], &product);
        assert_eq!(conflicted.direct_body_reach, 0);
        assert_eq!(conflicted.complete_callable_units, 0);
        value.sources.pop();
        value.sources[0].callable_body = None;
        let unknown = repository_evidence_selection_rank(&value, &[], &product);
        assert_eq!(unknown.direct_body_reach, 0);
        assert_eq!(unknown.complete_callable_units, 0);
    }

    #[test]
    fn body_proposals_preserve_unknown_exact_and_legacy_navigation_order() {
        let mut value = selection_fraction_fixture();
        let links = (1..6).map(|i| anonymous_call(i, 0)).collect::<Vec<_>>();
        let legacy = compile_repository_evidence_with_source_order_and_navigation(
            &value, 1, false, true, &links,
        );
        assert_eq!(legacy.support[0].symbol_id.as_deref(), Some("0"));
        anonymous_body(&mut value.sources[5], 1, 1);
        let body = compile_repository_evidence_with_source_order_and_navigation(
            &value, 1, false, true, &links,
        );
        assert_eq!(body.support[0].symbol_id.as_deref(), Some("5"));
        let unknown_high_rank = source_product(&value, &[0]);
        assert!(
            repository_evidence_selection_rank(&value, &links, &body)
                > repository_evidence_selection_rank(&value, &links, &unknown_high_rank)
        );
        assert_eq!(
            repository_evidence_selection_rank(&value, &links, &unknown_high_rank)
                .direct_body_reach,
            0,
            "ordinary unknown has no authenticated body credit"
        );
        assert!(
            repository_evidence_selection_rank(&value, &links, &unknown_high_rank).admitted_reach
                > 0,
            "unknown retains ordinary navigation evidence"
        );
        value.admissions[2].origin = PacketAdmissionOriginV1::ExactTypedSelector;
        let exact = compile_repository_evidence_with_source_order_and_navigation(
            &value, 1, false, true, &links,
        );
        assert_eq!(exact.support[0].symbol_id.as_deref(), Some("2"));
        assert_eq!(
            repository_evidence_selection_rank(&value, &links, &exact).direct_body_reach,
            0,
            "unknown exact stays unknown"
        );
    }

    fn anonymous_call(from: usize, to: usize) -> PacketDirectedRelationV1 {
        PacketDirectedRelationV1 {
            relation_id: format!("{from}-{to}"),
            from_identity: format!("node:{from}"),
            to_identity: format!("node:{to}"),
            relation_kind: PacketRelationKindV1::Call,
            certainty: PacketRelationCertaintyV1::Uncertain,
        }
    }

    #[test]
    fn measured_selection_compares_source_reach_across_fitting_cardinalities() {
        let mut value = selection_fraction_fixture();
        let links = [
            anonymous_call(0, 4),
            anonymous_call(1, 4),
            anonymous_call(2, 5),
        ];
        for reverse in [false, true] {
            if reverse {
                value.admissions.reverse();
                value.sources.reverse();
            }
            let mut fitted_counts = BTreeSet::new();
            let selected = compile_repository_evidence_for_representation_with_navigation(
                &value,
                &links,
                |product| {
                    let bytes = 50
                        + 200 * product.support.len()
                        + product
                            .support
                            .iter()
                            .map(|row| row.snippet.as_ref().map_or(0, String::len))
                            .sum::<usize>();
                    let fits = bytes <= 1100;
                    if fits {
                        fitted_counts.insert(product.support.len());
                    }
                    Ok::<_, ()>(fits.then_some(bytes))
                },
            )
            .unwrap();
            let RepresentationBudgetSelectionV1::Complete {
                product,
                measured_candidates,
                ..
            } = selected
            else {
                panic!("bounded source alternatives fit")
            };
            assert!(fitted_counts.contains(&3) && fitted_counts.contains(&2));
            assert_eq!(
                product
                    .support
                    .iter()
                    .map(|row| row.symbol_id.as_deref())
                    .collect::<Vec<_>>(),
                vec![Some("4"), Some("5")]
            );
            assert!(measured_candidates <= REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1);
            assert!(
                product
                    .support
                    .iter()
                    .all(|row| row.kind == SupportUnitKindDto::SourceRange)
            );
            assert!(!product.continuation.is_empty());
        }
    }

    #[test]
    fn measured_navigation_tail_can_add_a_compact_complete_unit() {
        let mut value = selection_fraction_fixture();
        for (source, bytes) in value.sources.iter_mut().zip([300, 300, 300, 10, 20, 200]) {
            source.source = "x".repeat(bytes);
        }
        let delivery = value.sources[3].delivery.as_mut().unwrap();
        delivery.requested_range.end_line = 100;
        delivery.remaining_source_bytes = 1000;
        delivery.next_start_line = Some(2);
        let links = [
            anonymous_call(0, 5),
            anonymous_call(1, 5),
            anonymous_call(2, 5),
        ];
        for reverse in [false, true] {
            if reverse {
                value.admissions.reverse();
                value.sources.reverse();
            }
            let selected = compile_repository_evidence_for_representation_with_navigation(
                &value,
                &links,
                |product| {
                    // Include a fixed enclosing frame and per-row cost. The
                    // caller, rather than a source-unit estimate, decides fit.
                    let bytes = 13_000
                        + 900 * product.support.len()
                        + 2 * product
                            .support
                            .iter()
                            .map(|row| row.snippet.as_ref().map_or(0, String::len))
                            .sum::<usize>();
                    Ok::<_, ()>((bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES).then_some(bytes))
                },
            )
            .unwrap();
            let RepresentationBudgetSelectionV1::Complete {
                product,
                representation_bytes,
                measured_candidates,
            } = selected
            else {
                panic!("compact complete source pair fits")
            };
            assert_eq!(
                product
                    .support
                    .iter()
                    .map(|row| row.symbol_id.as_deref())
                    .collect::<Vec<_>>(),
                vec![Some("3"), Some("4"), Some("5")]
            );
            assert_eq!(representation_bytes, 16_160);
            assert!(measured_candidates <= REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1);
            assert!(!product.continuation.is_empty());
            let partial = value
                .sources
                .iter()
                .find(|s| s.stable_identity == "node:3")
                .unwrap();
            assert!(partial.delivery.as_ref().unwrap().remaining_source_bytes > 0);
            let retained_partial = product
                .support
                .iter()
                .find(|row| row.symbol_id.as_deref() == Some("3"))
                .unwrap();
            assert_eq!(
                retained_partial.source_delivery.as_ref().unwrap(),
                partial.delivery.as_ref().unwrap()
            );
            assert!(product.continuation.iter().any(|selector| {
                selector.stable_identity == "node:3"
                    && selector
                        .source_range
                        .as_ref()
                        .is_some_and(|r| r.start_line == 2)
            }));
        }
    }

    #[test]
    fn subset_neighborhood_never_measures_a_dropped_baseline_graph() {
        let mut value = selection_fraction_fixture();
        value.sources.truncate(5);
        value.admissions.truncate(5);
        for source in &mut value.sources {
            source.source = "x".into();
            let overhead = serde_json::to_vec(&source_support_unit(source))
                .unwrap()
                .len()
                - 1;
            source.source = "x".repeat(4000 - overhead);
            assert_eq!(
                serde_json::to_vec(&source_support_unit(source))
                    .unwrap()
                    .len(),
                4000
            );
        }
        let mut edge = anonymous_call(0, 1);
        edge.certainty = PacketRelationCertaintyV1::Certain;
        edge.relation_id = "e".repeat(600);
        value.relations = vec![edge];
        let mut invalid_late_unit_measured = false;
        let selection = compile_repository_evidence_for_representation(&value, |product| {
            let graph = product
                .support
                .iter()
                .any(|r| r.kind == SupportUnitKindDto::TypedGraphEdge);
            let late = product.support.iter().any(|r| {
                r.kind == SupportUnitKindDto::SourceRange && r.symbol_id.as_deref() == Some("4")
            });
            if late && !graph {
                invalid_late_unit_measured = true;
            }
            Ok::<_, ()>((graph || late).then_some(12_000))
        })
        .unwrap();
        assert!(!invalid_late_unit_measured);
        let RepresentationBudgetSelectionV1::Complete { product, .. } = selection else {
            panic!("baseline graph fits")
        };
        assert!(
            product
                .support
                .iter()
                .any(|r| r.kind == SupportUnitKindDto::TypedGraphEdge)
        );
    }

    #[test]
    fn measured_subsets_bypass_an_oversized_positive_navigation_unit() {
        let mut value = selection_fraction_fixture();
        for (source, bytes) in value.sources.iter_mut().zip([300, 300, 300, 20, 1000, 200]) {
            source.source = "x".repeat(bytes);
        }
        let links = [
            anonymous_call(0, 5),
            anonymous_call(1, 5),
            anonymous_call(2, 5),
            anonymous_call(3, 4),
            anonymous_call(4, 3),
        ];
        for reverse in [false, true] {
            if reverse {
                value.admissions.reverse();
                value.sources.reverse();
            }
            let selected = compile_repository_evidence_for_representation_with_navigation(
                &value,
                &links,
                |product| {
                    let bytes = 13_000
                        + 900 * product.support.len()
                        + 2 * product
                            .support
                            .iter()
                            .map(|row| row.snippet.as_ref().map_or(0, String::len))
                            .sum::<usize>();
                    Ok::<_, ()>((bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES).then_some(bytes))
                },
            )
            .unwrap();
            let RepresentationBudgetSelectionV1::Complete {
                product,
                representation_bytes,
                ..
            } = selected
            else {
                panic!("a non-prefix compact pair fits")
            };
            let ids = product
                .support
                .iter()
                .filter_map(|row| row.symbol_id.as_deref())
                .collect::<BTreeSet<_>>();
            assert_eq!(ids, BTreeSet::from(["3", "5"]));
            assert_eq!(representation_bytes, 15_240);
            assert!(!product.continuation.is_empty());
        }
    }

    #[test]
    fn navigation_tail_keeps_exact_pages_and_unknown_completion_honest() {
        let mut value = selection_fraction_fixture();
        value.admissions[0].origin = PacketAdmissionOriginV1::ExactTypedSelector;
        let exact = value.sources[0].delivery.as_mut().unwrap();
        exact.requested_range.end_line = 100;
        exact.remaining_source_bytes = 1000;
        exact.next_start_line = Some(2);
        value.sources[1].delivery = None;
        value.sources[2].source = "x".into();
        let mut conflicting = value.sources[2].clone();
        let delivery = conflicting.delivery.as_mut().unwrap();
        delivery.requested_range.end_line = 100;
        delivery.remaining_source_bytes = 1000;
        delivery.next_start_line = Some(2);
        value.sources.push(conflicting);
        let links = [anonymous_call(1, 5), anonymous_call(2, 5)];
        for reverse in [false, true] {
            if reverse {
                value.admissions.reverse();
                value.sources.reverse();
            }
            let admissions = ordered_admissions(&value);
            let ranks = admissions
                .iter()
                .enumerate()
                .map(|(i, a)| (a.stable_identity.as_str(), i))
                .collect::<HashMap<_, _>>();
            let sources = selected_source_ranges(&value, &ranks);
            let (ordered, active) =
                order_sources_by_navigation(sources, &admissions, &links, &value.sources);
            assert!(active);
            assert_eq!(ordered[0].witness.stable_identity, "node:0");
            assert_eq!(ordered[1].witness.stable_identity, "node:5");
            // Known complete tail units precede the cheaper unknown/conflict.
            assert_eq!(ordered[2].witness.stable_identity, "node:3");
            assert_eq!(ordered[3].witness.stable_identity, "node:4");
            for source in &ordered {
                if ["node:0", "node:1", "node:2"].contains(&source.witness.stable_identity.as_str())
                {
                    assert!(!source_has_consistent_complete_delivery(
                        source,
                        &value.sources
                    ));
                }
            }
        }
    }

    #[test]
    fn selection_reach_counts_unique_owners_and_preserves_prefix_uncertainty() {
        let mut value = selection_fraction_fixture();
        let links = [anonymous_call(0, 4), anonymous_call(1, 4)];
        let product = RepositoryDerivedCompilationV1 {
            support: vec![source_support_unit(&value.sources[4])],
            continuation: vec![],
        };
        let full = repository_evidence_selection_rank(&value, &links, &product);
        let mut repeated = links.to_vec();
        repeated.extend(links.iter().rev().cloned());
        assert_eq!(
            full,
            repository_evidence_selection_rank(&value, &repeated, &product)
        );
        let d = value.sources[4].delivery.as_mut().unwrap();
        d.requested_range.end_line = 100;
        d.remaining_source_bytes = 1000;
        d.next_start_line = Some(2);
        let prefix = RepositoryDerivedCompilationV1 {
            support: vec![source_support_unit(&value.sources[4])],
            continuation: vec![],
        };
        let partial = repository_evidence_selection_rank(&value, &links, &prefix);
        assert!(partial.admitted_reach > 0 && partial.admitted_reach < full.admitted_reach);
        let independent = RepositoryDerivedCompilationV1 {
            support: vec![source_support_unit(&value.sources[2])],
            continuation: vec![],
        };
        assert!(repository_evidence_selection_rank(&value, &links, &independent) > partial);
        // A complete shared target can still outrank an independent body.
        // Structural reach has no oracle for whether that target is boilerplate.
        assert!(full > repository_evidence_selection_rank(&value, &links, &independent));
        value.sources[4].delivery = None;
        assert_eq!(
            repository_evidence_selection_rank(&value, &links, &prefix).admitted_reach,
            0
        );
    }

    #[test]
    fn selection_conflicting_witness_bounds_cannot_complete_a_retained_prefix() {
        let mut value = selection_fraction_fixture();
        value.admissions.truncate(1);
        value.sources.truncate(1);
        value.admissions[0].origin = PacketAdmissionOriginV1::ExactTypedSelector;
        let full = value.sources[0].clone();
        let delivery = value.sources[0].delivery.as_mut().unwrap();
        delivery.requested_range.end_line = 100;
        delivery.remaining_source_bytes = 1000;
        delivery.next_start_line = Some(2);
        let prefix = RepositoryDerivedCompilationV1 {
            support: vec![source_support_unit(&value.sources[0])],
            continuation: vec![],
        };
        let known_prefix = repository_evidence_selection_rank(&value, &[], &prefix);
        assert!(known_prefix.admitted_reach > 0);
        assert_eq!(known_prefix.requested_complete, 0);
        value.sources.push(full);
        let conflict = repository_evidence_selection_rank(&value, &[], &prefix);
        assert_eq!(conflict.requested_complete, 0);
        assert_eq!(conflict.requested_delivered_lines, 0);
        assert_eq!(conflict.admitted_reach, 0);
        value.sources.reverse();
        assert_eq!(
            conflict,
            repository_evidence_selection_rank(&value, &[], &prefix)
        );
    }

    #[test]
    fn selection_containment_alias_is_fractional_against_original_requested_extent() {
        let mut value = selection_fraction_fixture();
        value.admissions.truncate(2);
        value.sources.truncate(2);
        value.sources[1].path = value.sources[0].path.clone();
        value.sources[1].source = value.sources[0].source.clone();
        for source in &mut value.sources {
            let delivery = source.delivery.as_mut().unwrap();
            delivery.requested_range.end_line = 40;
            delivery.remaining_source_bytes = 400;
            delivery.next_start_line = Some(2);
        }
        value.admissions[1].origin = PacketAdmissionOriginV1::ExactTypedSelector;
        let product = compile_repository_evidence(&value);
        let rank = repository_evidence_selection_rank(&value, &[], &product);
        assert_eq!(rank.requested_complete, 0);
        assert_eq!(rank.requested_delivered_lines, 1);
        // Changing a name, language or physical path spelling cannot confer proof.
        let mut renamed = value.clone();
        for source in &mut renamed.sources {
            source.path = "other.ext".into();
            source.symbol = None;
        }
        assert_eq!(
            rank,
            repository_evidence_selection_rank(
                &renamed,
                &[],
                &compile_repository_evidence(&renamed)
            )
        );
        // A cross-file alias not actually selected receives no source credit.
        value.admissions[1].origin = PacketAdmissionOriginV1::Retrieval;
        value.sources[1].path = "different.ext".into();
        let lone = RepositoryDerivedCompilationV1 {
            support: vec![source_support_unit(&value.sources[0])],
            continuation: vec![],
        };
        let cross = repository_evidence_selection_rank(&value, &[], &lone);
        assert!(cross.direct_reach < rank.direct_reach);
    }

    fn row_mix_input() -> PacketCompilationInputV1 {
        let mut value = input();
        value.admissions = (0..3)
            .map(|id| PacketAdmissionReceiptV1 {
                packet_ordinal: id,
                stable_identity: format!("node:{id}"),
                score_version: "fixture".into(),
                reserved_source_bytes: 512,
                origin: PacketAdmissionOriginV1::Retrieval,
            })
            .collect();
        value.sources = (0..3)
            .map(|id| PacketHydratedSourceRangeV1 {
                callable_body: None,
                stable_identity: format!("node:{id}"),
                path: format!("source_{id}.rs"),
                symbol: Some(format!("operation_{id}")),
                start_line: 1,
                end_line: 1,
                source: format!("fn operation_{id}() {{}}"),
                delivery: None,
                parser_completeness: PacketParserCompletenessV1::Complete,
            })
            .collect();
        value.relations = vec![PacketDirectedRelationV1 {
            relation_id: "edge-0".into(),
            from_identity: "node:0".into(),
            to_identity: "node:2".into(),
            relation_kind: PacketRelationKindV1::Call,
            certainty: PacketRelationCertaintyV1::Certain,
        }];
        value
    }

    #[test]
    fn measured_row_mix_can_keep_three_fitting_sources_without_a_graph_slot() {
        let mut value = row_mix_input();
        let expected = compile_repository_evidence(&value);
        assert_eq!(expected.support.len(), 4);
        for reverse in [false, true] {
            if reverse {
                value.sources.reverse();
                value.admissions.reverse();
            }
            let result = compile_repository_evidence_for_representation(&value, |product| {
                // A constrained caller envelope, independent of prompt or identity.
                Ok::<_, ()>(Some(
                    10_000 + product.support.len() * 1_800 + product.continuation.len() * 64,
                ))
            })
            .unwrap();
            let RepresentationBudgetSelectionV1::Complete {
                product,
                measured_candidates,
                ..
            } = result
            else {
                panic!("three exact bodies fit the caller's frame")
            };
            assert_eq!(
                product
                    .support
                    .iter()
                    .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
                    .count(),
                3,
                "a graph-reserving prefix must not prevent a fitting source-richer proposal"
            );
            assert!(
                product
                    .support
                    .iter()
                    .all(|unit| unit.kind != SupportUnitKindDto::TypedGraphEdge)
            );
            for endpoint in ["node:0", "node:2"] {
                assert!(
                    product
                        .continuation
                        .iter()
                        .any(|selector| selector.stable_identity == endpoint
                            && selector.reason == PacketStructuralGapReasonV1::DisconnectedSeed),
                    "delivered endpoint bodies do not close the omitted relation"
                );
            }
            assert!(measured_candidates <= REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1);
        }
    }

    #[test]
    fn measured_row_mix_keeps_graph_fallback_and_honest_omitted_page_bounds() {
        let mut value = row_mix_input();
        for source in &mut value.sources {
            source.delivery = Some(codestory_contracts::compilation::PacketSourceDeliveryV1 {
                requested_range: codestory_contracts::compilation::PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: 4,
                },
                source_sha256: "a".repeat(64),
                remaining_source_bytes: 50,
                next_start_line: Some(2),
                blocked_line_rendered_bytes: None,
            });
        }
        let selection = compile_repository_evidence_for_representation(&value, |product| {
            let bodies = product
                .support
                .iter()
                .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
                .count();
            Ok::<_, ()>((product.support.len() <= 2 && bodies <= 1).then_some(12_000))
        })
        .unwrap();
        let RepresentationBudgetSelectionV1::Complete { product, .. } = selection else {
            panic!("graph fallback fits")
        };
        assert!(
            product
                .support
                .iter()
                .any(|unit| unit.kind == SupportUnitKindDto::TypedGraphEdge)
        );
        assert!(
            product
                .continuation
                .iter()
                .any(|selector| selector.stable_identity == "node:0"
                    && selector.source_range
                        == Some(codestory_contracts::compilation::PacketSourceRangeV1 {
                            start_line: 2,
                            end_line: 4
                        }))
        );
        for identity in ["node:1", "node:2"] {
            assert!(
                product
                    .continuation
                    .iter()
                    .any(|selector| selector.stable_identity == identity
                        && selector.source_range
                            == Some(codestory_contracts::compilation::PacketSourceRangeV1 {
                                start_line: 1,
                                end_line: 4
                            }))
            );
        }
        value.sources.clear();
        let selected = compile_repository_evidence_for_representation(&value, |product| {
            Ok::<_, ()>(Some(serde_json::to_vec(&product.support).unwrap().len()))
        })
        .unwrap();
        let RepresentationBudgetSelectionV1::Complete { product, .. } = selected else {
            panic!("navigation fits")
        };
        assert!(
            product
                .support
                .iter()
                .any(|unit| unit.kind == SupportUnitKindDto::TypedGraphEdge)
        );
    }

    #[test]
    fn complete_first_measures_cheap_context_and_retains_original_families() {
        let mut value = row_mix_input();
        for source in &mut value.sources {
            source.delivery = Some(codestory_contracts::compilation::PacketSourceDeliveryV1 {
                requested_range: codestory_contracts::compilation::PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: 1,
                },
                source_sha256: "a".repeat(64),
                remaining_source_bytes: 0,
                next_start_line: None,
                blocked_line_rendered_bytes: None,
            });
        }
        value.sources[0].source = "large high-ranked implementation body".repeat(30);
        value.sources[1].source = "tiny unrelated helper".into();
        value.sources[2].source = "medium helper body".repeat(6);
        for same_path in [false, true] {
            if same_path {
                for source in &mut value.sources {
                    source.path = "renamed.rs".into();
                }
            }
            let cheap = compile_repository_evidence_for_representation(&value, |p| {
                Ok::<_, ()>(
                    (p.support.len() == 1)
                        .then(|| 15_500 + serde_json::to_vec(&p.support).unwrap().len()),
                )
            })
            .unwrap();
            let RepresentationBudgetSelectionV1::Complete { product, .. } = cheap else {
                panic!("one row fits")
            };
            assert_eq!(product.support[0].symbol_id.as_deref(), Some("1"));
            assert!(
                product
                    .continuation
                    .iter()
                    .any(|s| s.stable_identity == "node:0")
            );
            let original = compile_repository_evidence_for_representation(&value, |p| {
                Ok::<_, ()>(
                    (p.support.len() == 2
                        && p.support[0].symbol_id.as_deref() == Some("0")
                        && p.support[1].kind == SupportUnitKindDto::TypedGraphEdge)
                        .then_some(12_000),
                )
            })
            .unwrap();
            let RepresentationBudgetSelectionV1::Complete { product, .. } = original else {
                panic!("original graph family remains measurable")
            };
            assert_eq!(product.support[0].symbol_id.as_deref(), Some("0"));
            assert_eq!(product.support[1].kind, SupportUnitKindDto::TypedGraphEdge);
            value.sources.reverse();
            value.admissions.reverse();
        }
    }

    #[test]
    fn complete_first_authenticates_typed_priority_over_completed_context() {
        let mut value = row_mix_input();
        value.admissions[0].origin = PacketAdmissionOriginV1::ExactTypedSelector;
        for source in &mut value.sources {
            source.delivery = Some(codestory_contracts::compilation::PacketSourceDeliveryV1 {
                requested_range: codestory_contracts::compilation::PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: 4,
                },
                source_sha256: "a".repeat(64),
                remaining_source_bytes: 50,
                next_start_line: Some(2),
                blocked_line_rendered_bytes: None,
            });
        }
        value.sources[1]
            .delivery
            .as_mut()
            .unwrap()
            .requested_range
            .end_line = 1;
        value.sources[1]
            .delivery
            .as_mut()
            .unwrap()
            .remaining_source_bytes = 0;
        value.sources[1].delivery.as_mut().unwrap().next_start_line = None;
        let selected = compile_repository_evidence_for_representation(&value, |p| {
            Ok::<_, ()>((p.support.len() == 1).then_some(12_000))
        })
        .unwrap();
        let RepresentationBudgetSelectionV1::Complete { product, .. } = selected else {
            panic!("one typed page fits")
        };
        assert_eq!(product.support[0].symbol_id.as_deref(), Some("0"));
        assert_eq!(
            product.support[0]
                .source_delivery
                .as_ref()
                .unwrap()
                .remaining_source_bytes,
            50
        );
        assert!(
            product
                .continuation
                .iter()
                .any(|s| s.stable_identity == "node:0"
                    && s.source_range.as_ref().is_some_and(|r| r.start_line == 2))
        );
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
        assert_eq!(&proposals[..3], &[2, 1, 0]);
        assert!(proposals.len() <= REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1);
        assert_eq!(measured_candidates, proposals.len());
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
        assert_eq!(&proposals[..3], &[2, 1, 0]);
        assert!(proposals.len() <= REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1);
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
                callable_body: None,
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
                    && unit.symbol_id.as_deref() == Some("15"))
        );
        assert_eq!(
            product
                .support
                .iter()
                .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
                .count(),
            15
        );
        assert_eq!(
            product
                .support
                .iter()
                .filter(|unit| unit.kind == SupportUnitKindDto::TypedGraphEdge)
                .count(),
            1
        );
        assert_eq!(
            product
                .continuation
                .iter()
                .map(|selector| (selector.stable_identity.as_str(), selector.reason))
                .collect::<Vec<_>>(),
            vec![
                ("node:15", PacketStructuralGapReasonV1::SourceBudgetExceeded),
                ("node:0", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:2", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:3", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:4", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:5", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:6", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:7", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:8", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:9", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:10", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:11", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:12", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:13", PacketStructuralGapReasonV1::DisconnectedSeed),
                ("node:14", PacketStructuralGapReasonV1::DisconnectedSeed),
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
                callable_body: None,
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
                    callable_body: None,
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
