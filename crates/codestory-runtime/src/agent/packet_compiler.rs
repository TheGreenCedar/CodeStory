//! Runtime adapter for the pure repository-derived packet compiler.
//!
//! Runtime owns publication checks and converts pinned repository records into
//! [`PacketCompilationInputV1`]. Selection itself lives in
//! `codestory-agent` and cannot see the question.

use crate::AppController;
use crate::agent::packet_candidate::PacketProofSession;
use crate::agent::packet_coverage::PacketCoverageInput;
use crate::agent::packet_freshness::PacketFreshnessInput;
use codestory_agent::evidence_compiler::{
    JointRepresentationSelectionV1, RepositoryDerivedCompilationV1,
    RepresentationBudgetSelectionV1, compile_repository_evidence,
    compile_repository_evidence_baseline_for_representation,
    compile_repository_evidence_for_representation, compile_repository_evidence_joint_additions,
    repository_evidence_baseline_rank, repository_evidence_joint_progress_rank,
};
use codestory_contracts::api::{
    AgentPacketDto, AgentPacketRequestDto, BoundedDrillPlanDto, DrillGapKindDto, DrillOptionDto,
    EmbeddingVectorPublicationIdentityDto, PACKET_DRILL_MAX_BYTES, PACKET_DRILL_MAX_DEPTH,
    PACKET_DRILL_MAX_HITS, PACKET_DRILL_MAX_OPTIONS, PacketDispositionDto,
    PacketProbeResolutionDto, PacketProbeResolutionStatusDto, SourceCoverageNotEstablishedCauseDto,
    SourceCoverageObservationDto, SourceCoverageStatusDto, SupportUnitDto, SupportUnitKindDto,
    decode_drill_option_id,
};
use codestory_contracts::compilation::{
    INTERIM_SOURCE_CONTINUATION_UPPER_BOUND, INTERIM_SOURCE_ROW_UPPER_BOUND,
    PACKET_COMPILATION_CONTRACT_VERSION_V1, PUBLIC_PACKET_SERIALIZED_MAX_BYTES,
    PacketAdmissionGapKindV1, PacketAdmissionGapV1, PacketAdmissionOriginV1,
    PacketAdmissionReceiptV1, PacketCompilationInputV1, PacketCompilationPublicationV1,
    PacketContinuationSelectorV1, PacketDirectedRelationV1, PacketHydratedSourceRangeV1,
    PacketIdentityAmbiguityV1, PacketParserCompletenessV1, PacketRelationCertaintyV1,
    PacketRelationKindV1, PacketSourceDeliveryV1, PacketSourceRangeV1, PacketStructuralGapReasonV1,
};
use codestory_contracts::graph::{
    EdgeKind as CoreEdgeKind, FileCoverageReason, Node as CoreNode, NodeId as CoreNodeId,
    NodeKind as CoreNodeKind, ResolutionCertainty,
};
use codestory_store::{FileInfo, Store};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Only the exhausted requested-page fallback may signal a measured capacity refusal.
pub(crate) const REQUESTED_PAGE_CAPACITY_REFUSAL: &str = "packet_output_budget_exceeded";

const FILE_NAVIGATION_VERIFICATION_MAX_BYTES: usize = 1024 * 1024;
const SOURCE_PAGE_FENCE_BYTES: usize = "```text\n".len() + "```".len();

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PreparedPacketCompilationV1 {
    input: PacketCompilationInputV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    call_navigation: Vec<PacketDirectedRelationV1>,
    pub(crate) source_coverage: Vec<SourceCoverageObservationDto>,
    publication: PacketCompilationPublicationV1,
    source_refusals: Vec<String>,
    // Live proof minted only by successful pinned PublishedFile hydration.
    // Replay source DTOs cannot certify deliberate absence from Core.
    #[serde(skip)]
    published_file_sources: Vec<PacketHydratedSourceRangeV1>,
    file_navigation_paths: HashMap<String, String>,
    // Complete-line alternatives are made during the same authenticated read.
    // They remain private runtime witnesses, never compiler question inputs.
    source_page_alternatives: BTreeMap<usize, Vec<PacketHydratedSourceRangeV1>>,
    requested_source_identities: BTreeSet<String>,
    #[serde(default)]
    requested_source_minimum_ends: BTreeMap<String, u32>,
    #[serde(default)]
    issued_context_sources: Option<Vec<PacketHydratedSourceRangeV1>>,
}

impl PreparedPacketCompilationV1 {
    pub(crate) fn replay_snapshot(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    pub(crate) fn replay_input(&self) -> &PacketCompilationInputV1 {
        &self.input
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn from_fixture_input(input: PacketCompilationInputV1) -> Self {
        Self {
            publication: input.publication.clone(),
            call_navigation: Vec::new(),
            input,
            source_coverage: Vec::new(),
            source_refusals: Vec::new(),
            published_file_sources: Vec::new(),
            file_navigation_paths: HashMap::new(),
            source_page_alternatives: BTreeMap::new(),
            requested_source_identities: BTreeSet::new(),
            requested_source_minimum_ends: BTreeMap::new(),
            issued_context_sources: None,
        }
    }

    pub(crate) fn supports_served_source_record(&self) -> bool {
        self.input.sources.iter().all(|s| {
            s.delivery.as_ref().is_some_and(|d| {
                d.matches_delivery(s.start_line, s.end_line)
                    && (s
                        .stable_identity
                        .strip_prefix("node:")
                        .is_some_and(|id| id.parse::<i64>().is_ok())
                        || (s.stable_identity == format!("path:{}", s.path)
                            && s.symbol.is_none()
                            && s.start_line == 1
                            && d.requested_range.end_line == s.end_line
                            && d.remaining_source_bytes == 0
                            && d.next_start_line.is_none()
                            && d.blocked_line_rendered_bytes.is_none()))
            })
        })
    }
    pub(crate) fn default_product(&self) -> RepositoryDerivedCompilationV1 {
        let mut product = compile_repository_evidence(&self.input);
        attach_file_navigation_paths(&mut product.support, &self.file_navigation_paths);
        product
    }

    pub(crate) fn into_frozen(
        self,
        mut product: RepositoryDerivedCompilationV1,
    ) -> FrozenPacketCompilationV1 {
        attach_file_navigation_paths(&mut product.support, &self.file_navigation_paths);
        FrozenPacketCompilationV1 {
            product,
            source_coverage: self.source_coverage,
            publication: self.publication,
            source_refusals: self.source_refusals,
            hydrated_sources: self.input.sources,
            published_file_sources: self.published_file_sources,
            context_floor: Vec::new(),
        }
    }

    pub(crate) fn apply_proposal(
        &self,
        packet: &mut AgentPacketDto,
        request: Option<&AgentPacketRequestDto>,
        product: &RepositoryDerivedCompilationV1,
    ) {
        apply_compilation_state(
            packet,
            request,
            product,
            &self.publication,
            &self.source_refusals,
            &self.input.sources,
            &self.published_file_sources,
        );
    }

    #[cfg(test)]
    pub(crate) fn select_for_representation(
        self,
        mut measure: impl FnMut(
            &Self,
            &RepositoryDerivedCompilationV1,
        ) -> Result<Option<usize>, codestory_contracts::api::ApiError>,
    ) -> Result<FrozenPacketCompilationV1, codestory_contracts::api::ApiError> {
        self.select_for_representation_with_baseline(|prepared, product, _baseline| {
            measure(prepared, product)
        })
    }

    pub(crate) fn select_for_representation_with_baseline(
        mut self,
        mut measure: impl FnMut(
            &Self,
            &RepositoryDerivedCompilationV1,
            Option<&RepositoryDerivedCompilationV1>,
        ) -> Result<Option<usize>, codestory_contracts::api::ApiError>,
    ) -> Result<FrozenPacketCompilationV1, codestory_contracts::api::ApiError> {
        // Preserve the existing whole-request callback bound. Every accepted
        // or rejected callback consumes one slot, including final validation.
        let mut calls = 0usize;
        let mut bounded_measure =
            |prepared: &Self,
             product: &RepositoryDerivedCompilationV1,
             baseline: Option<&RepositoryDerivedCompilationV1>| {
                let limit = 8 * codestory_agent::evidence_compiler::REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1 + 1;
                if calls >= limit {
                    return Err(codestory_contracts::api::ApiError::internal(
                        "Packet representation proposal bound exhausted.",
                    ));
                }
                calls += 1;
                measure(prepared, product, baseline)
            };
        let original_sources = self.input.sources.clone();
        let alternatives = std::mem::take(&mut self.source_page_alternatives);
        // Protect independently selected current context before requested progress.
        // Only current sealed, hydrated witnesses enter; no parent inventory is inferred.
        let context_sources = self.issued_context_sources.clone().unwrap_or_else(|| {
            original_sources
                .iter()
                .filter(|source| {
                    !self
                        .requested_source_identities
                        .contains(&source.stable_identity)
                })
                .cloned()
                .collect::<Vec<_>>()
        });
        let context_floor = if (self.issued_context_sources.is_none()
            && self.requested_source_identities.is_empty())
            || context_sources.is_empty()
        {
            None
        } else {
            self.input.sources = context_sources.clone();
            let original_context = self.default_product();
            if self.issued_context_sources.is_some()
                && context_sources.iter().any(|source| {
                    !original_context.support.iter().any(|row| {
                        row.kind == SupportUnitKindDto::SourceRange
                            && final_support_stable_identity(row).as_deref()
                                == Some(source.stable_identity.as_str())
                            && row.start_line == Some(source.start_line)
                            && row.end_line == Some(source.end_line)
                            && row.snippet.as_deref() == Some(source.source.as_str())
                            && row.source_delivery == source.delivery
                    })
                })
            {
                return Err(codestory_contracts::api::ApiError::internal(
                    "Issued source floor cannot omit any previously served row.",
                ));
            }
            // Issued history is immutable. A valid, never-served default is
            // protected only after the complete caller frame proves it fits;
            // otherwise select its current baseline through that same renderer.
            // Invalid source witnesses retain the fail-closed predicate without
            // reaching the representation callback.
            let preserve_original_context = self.issued_context_sources.is_some()
                || !source_floor_retained(&original_context.support, &original_context.support)
                || bounded_measure(&self, &original_context, None)?
                    .is_some_and(|bytes| bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES);
            let mut candidates_examined = 0usize;
            let mut floor_predicate_rejected = 0usize;
            let mut floor_preserving_measurements = 0usize;
            let mut measure_none = 0usize;
            let mut minimum_measured_bytes: Option<usize> = None;
            let selected = compile_repository_evidence_baseline_for_representation(
                &self.input,
                &self.call_navigation,
                |product| {
                    candidates_examined += 1;
                    let mut displayed = product.clone();
                    attach_file_navigation_paths(
                        &mut displayed.support,
                        &self.file_navigation_paths,
                    );
                    if (preserve_original_context
                        && !source_floor_retained(&displayed.support, &original_context.support))
                        || (!preserve_original_context
                            && (!displayed
                                .support
                                .iter()
                                .any(|row| row.kind == SupportUnitKindDto::SourceRange)
                                || !source_floor_retained(&displayed.support, &displayed.support)))
                    {
                        floor_predicate_rejected += 1;
                        return Ok(None);
                    }
                    floor_preserving_measurements += 1;
                    let measured = bounded_measure(&self, &displayed, None)?;
                    if let Some(bytes) = measured {
                        minimum_measured_bytes = Some(
                            minimum_measured_bytes.map_or(bytes, |minimum| minimum.min(bytes)),
                        );
                    } else {
                        measure_none += 1;
                    }
                    Ok(measured)
                },
            )?;
            match selected {
                RepresentationBudgetSelectionV1::Complete { mut product, .. } => {
                    attach_file_navigation_paths(&mut product.support, &self.file_navigation_paths);
                    Some(product)
                }
                RepresentationBudgetSelectionV1::BudgetExceeded { .. } => {
                    return Err(codestory_contracts::api::ApiError::internal(format!(
                        "Authenticated current context cannot fit the public representation. candidates_examined={candidates_examined}; floor_predicate_rejected={floor_predicate_rejected}; floor_preserving_measurements={floor_preserving_measurements}; measure_none={measure_none}; minimum_measured_bytes={minimum_measured_bytes:?}; wire_cap={PUBLIC_PACKET_SERIALIZED_MAX_BYTES}; context_sources={}; context_source_bytes={}; incomplete_context_pages={}",
                        context_sources.len(),
                        context_sources
                            .iter()
                            .map(|source| source.source.len())
                            .sum::<usize>(),
                        context_sources
                            .iter()
                            .filter(|source| source
                                .delivery
                                .as_ref()
                                .is_some_and(|delivery| delivery.next_start_line.is_some()))
                            .count(),
                    )));
                }
            }
        };
        let protected_context_sources = self.issued_context_sources.clone().unwrap_or_else(|| {
            context_sources
                .iter()
                .filter(|source| {
                    context_floor.as_ref().is_some_and(|floor| {
                        floor.support.iter().any(|row| {
                            row.kind == SupportUnitKindDto::SourceRange
                                && final_support_stable_identity(row).as_deref()
                                    == Some(source.stable_identity.as_str())
                                && row.path.as_deref() == Some(source.path.as_str())
                                && row.start_line == Some(source.start_line)
                                && row.end_line == Some(source.end_line)
                                && row.snippet.as_deref() == Some(source.source.as_str())
                                && row.source_delivery == source.delivery
                        })
                    })
                })
                .cloned()
                .collect()
        });
        let mut extents = vec![original_sources.clone()];
        let mut sources = original_sources.clone();
        for (_, pages) in alternatives.into_iter().rev() {
            for source in &mut sources {
                if let Some(page) = pages
                    .iter()
                    .find(|page| page.stable_identity == source.stable_identity)
                {
                    *source = page.clone();
                }
            }
            // A selected source range may fit with shorter context even when
            // shrinking every row together would unnecessarily split it.
            let mut requested_extent = sources.clone();
            for source in &mut requested_extent {
                if self
                    .requested_source_identities
                    .contains(&source.stable_identity)
                    && let Some(original) = original_sources
                        .iter()
                        .find(|original| original.stable_identity == source.stable_identity)
                {
                    *source = original.clone();
                }
            }
            // A shorter new page must compete beside the exact selected floor,
            // including genuine issued prefixes. Restoring
            // unrequested rows here retains the existing extent/callback bound.
            // Requested old rows keep their hydrated candidates: the floor
            // check below must still reject any candidate losing their prefix.
            let mut mixed_extent = sources.clone();
            for source in &mut mixed_extent {
                if !self
                    .requested_source_identities
                    .contains(&source.stable_identity)
                    && let Some(original) = protected_context_sources
                        .iter()
                        .find(|old| old.stable_identity == source.stable_identity)
                {
                    *source = original.clone();
                }
            }
            for extent in [requested_extent, mixed_extent] {
                if !extents.contains(&extent) {
                    extents.push(extent);
                }
            }
        }
        // Compare authenticated extents under the actual caller renderer.
        // Apply the same requested-progress/admitted-reach rank as the inner
        // family comparison. No question or new hydration is consulted.
        let verified_extents = extents.clone();
        let mut best = None;
        for sources in extents {
            self.input.sources = sources;
            let selection = compile_repository_evidence_baseline_for_representation(
                &self.input,
                &self.call_navigation,
                |product| {
                    let mut product = product.clone();
                    attach_file_navigation_paths(&mut product.support, &self.file_navigation_paths);
                    let retained = self.requested_source_identities.iter().all(|identity| {
                        product.support.iter().any(|unit| {
                            unit.kind == SupportUnitKindDto::SourceRange
                                && final_support_stable_identity(unit).as_ref() == Some(identity)
                                && self.requested_source_minimum_ends.get(identity).is_none_or(
                                    |minimum| unit.end_line.is_some_and(|end| end >= *minimum),
                                )
                        })
                    });
                    if !retained
                        || context_floor.as_ref().is_some_and(|floor| {
                            !source_floor_retained(&product.support, &floor.support)
                        })
                    {
                        return Ok(None);
                    }
                    bounded_measure(&self, &product, None)
                },
            )?;
            if let RepresentationBudgetSelectionV1::Complete { product, .. } = selection {
                let rank =
                    repository_evidence_baseline_rank(&self.input, &self.call_navigation, &product);
                if best
                    .as_ref()
                    .is_none_or(|(previous, _, _)| rank > *previous)
                {
                    best = Some((rank, self.input.sources.clone(), product));
                }
            }
        }
        if let Some((_, sources, baseline)) = best {
            self.input.sources = sources.clone();
            // Preserve the exact legacy baseline; shorter verified pages may
            // compete only beside its original source witnesses. Restoring
            // requested/issued rows collapses both variants of each cap.
            let baseline_sources = sources
                .iter()
                .filter(|source| {
                    baseline.support.iter().any(|row| {
                        row.kind == SupportUnitKindDto::SourceRange
                            && final_support_stable_identity(row).as_deref()
                                == Some(source.stable_identity.as_str())
                            && row.path.as_deref() == Some(source.path.as_str())
                            && row.start_line == Some(source.start_line)
                            && row.end_line == Some(source.end_line)
                            && row.snippet.as_deref() == Some(source.source.as_str())
                            && row.source_delivery == source.delivery
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            if baseline
                .support
                .iter()
                .filter(|row| row.kind == SupportUnitKindDto::SourceRange)
                .any(|row| {
                    !baseline_sources.iter().any(|source| {
                        final_support_stable_identity(row).as_deref()
                            == Some(source.stable_identity.as_str())
                            && row.path.as_deref() == Some(source.path.as_str())
                            && row.start_line == Some(source.start_line)
                            && row.end_line == Some(source.end_line)
                            && row.snippet.as_deref() == Some(source.source.as_str())
                            && row.source_delivery == source.delivery
                    })
                })
            {
                return Err(codestory_contracts::api::ApiError::internal(
                    "Current baseline source witness is missing.",
                ));
            }
            let mut joint_extents = vec![sources];
            for mut extent in verified_extents {
                for source in &mut extent {
                    if let Some(original) = baseline_sources
                        .iter()
                        .find(|old| old.stable_identity == source.stable_identity)
                    {
                        *source = original.clone();
                    }
                }
                if !joint_extents.contains(&extent) {
                    joint_extents.push(extent);
                }
            }
            // Original plus three verified caps. <=8*51 baseline callbacks,
            // <=4*139 joint callbacks, one context-default probe and one final replay fit the unchanged
            // whole-request guard (1049). Unsupported inventories fail closed.
            if joint_extents.len() > 4 {
                return Err(codestory_contracts::api::ApiError::internal(
                    "Verified joint extent bound exhausted.",
                ));
            }
            let mut winner = None;
            for sources in joint_extents {
                self.input.sources = sources;
                let joint = compile_repository_evidence_joint_additions(
                    &self.input,
                    &baseline,
                    |product| {
                        let mut displayed = product.clone();
                        let mut displayed_baseline = baseline.clone();
                        attach_file_navigation_paths(
                            &mut displayed.support,
                            &self.file_navigation_paths,
                        );
                        attach_file_navigation_paths(
                            &mut displayed_baseline.support,
                            &self.file_navigation_paths,
                        );
                        if context_floor.as_ref().is_some_and(|floor| {
                            !source_floor_retained(&displayed.support, &floor.support)
                        }) {
                            return Ok(None);
                        }
                        bounded_measure(&self, &displayed, Some(&displayed_baseline))
                    },
                )?;
                if let JointRepresentationSelectionV1::Complete {
                    product,
                    representation_bytes,
                    ..
                } = joint
                {
                    let gain =
                        repository_evidence_joint_progress_rank(&self.input, &baseline, &product);
                    if winner
                        .as_ref()
                        .is_none_or(|(previous_gain, previous_bytes, _, _)| {
                            gain > *previous_gain
                                || (gain == *previous_gain
                                    && representation_bytes < *previous_bytes)
                        })
                    {
                        winner = Some((
                            gain,
                            representation_bytes,
                            self.input.sources.clone(),
                            product,
                        ));
                    }
                }
            }
            let Some((_, _, sources, mut product)) = winner else {
                return Err(codestory_contracts::api::ApiError::internal(
                    "Current baseline or joint served frame failed final validation.",
                ));
            };
            self.input.sources = sources;
            let mut displayed_baseline = baseline;
            attach_file_navigation_paths(
                &mut displayed_baseline.support,
                &self.file_navigation_paths,
            );
            attach_file_navigation_paths(&mut product.support, &self.file_navigation_paths);
            if context_floor
                .as_ref()
                .is_some_and(|floor| !source_floor_retained(&product.support, &floor.support))
                || bounded_measure(&self, &product, Some(&displayed_baseline))?
                    .is_none_or(|bytes| bytes > PUBLIC_PACKET_SERIALIZED_MAX_BYTES)
            {
                return Err(codestory_contracts::api::ApiError::internal(
                    "Selected joint source extent failed final served-frame validation.",
                ));
            }
            let mut frozen = self.into_frozen(product);
            frozen.context_floor = context_floor.map(|floor| floor.support).unwrap_or_default();
            return Ok(frozen);
        }
        if let Some(floor) = context_floor {
            self.input.sources = context_sources;
            self.source_refusals.extend(self.requested_source_identities.iter().map(|identity|
                format!("{identity}: no requested complete-line page fits without losing authenticated context.")));
            let fits = bounded_measure(&self, &floor, Some(&floor))?
                .is_some_and(|bytes| bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES);
            if !fits {
                return Err(codestory_contracts::api::ApiError::new(
                    REQUESTED_PAGE_CAPACITY_REFUSAL,
                    "No requested page fits without losing authenticated context.",
                ));
            }
            let protected = floor.support.clone();
            let mut frozen = self.into_frozen(floor);
            frozen.context_floor = protected;
            return Ok(frozen);
        }
        // All complete extents failed. Preserve a measured, explicit refusal;
        // the ordinary finalizer still owns an oversized-envelope refusal.
        self.input.sources = original_sources;
        self.source_refusals.extend(self.requested_source_identities.iter().map(|identity|
            format!("{identity}: no authenticated requested complete-line page fit the serialized public representation.")));
        let selection = compile_repository_evidence_for_representation(&self.input, |product| {
            let mut product = product.clone();
            attach_file_navigation_paths(&mut product.support, &self.file_navigation_paths);
            bounded_measure(&self, &product, None)
        })?;
        let product = match selection {
            RepresentationBudgetSelectionV1::Complete { product, .. }
            | RepresentationBudgetSelectionV1::BudgetExceeded { product, .. } => product,
        };
        Ok(self.into_frozen(product))
    }
    /// Freeze the original offered frontier while joint evidence is measured.
    /// Internal coverage only nominates removals: the final served-frame guard
    /// must still authenticate every resolving witness before acceptance.
    pub(crate) fn retain_baseline_continuation_actions(
        &self,
        baseline: &AgentPacketDto,
        candidate: &mut AgentPacketDto,
    ) -> bool {
        let resolved = self.resolved_baseline_option_witnesses(baseline, candidate);
        let partial = self.authenticated_partial_baseline_actions(baseline, candidate);
        retain_current_baseline_actions_with_partial(baseline, candidate, &resolved, &partial)
    }

    /// A partial prefix does not resolve an old full-range action. It may
    /// replace that action in the compiler's canonical frontier with its tail.
    /// Authenticate that transition before restoring the old action verbatim;
    /// the source row independently exports its honest next-page action.
    fn authenticated_partial_baseline_actions(
        &self,
        baseline: &AgentPacketDto,
        candidate: &AgentPacketDto,
    ) -> BTreeSet<String> {
        let mut partial = BTreeSet::new();
        let (Some(old), Some(current)) = (
            baseline.disposition.drill.as_ref(),
            candidate.disposition.drill.as_ref(),
        ) else {
            return partial;
        };
        for option in &old.options {
            let Some(range) = option.source_range else {
                continue;
            };
            if option.structural_reason != Some(PacketStructuralGapReasonV1::SourceBudgetExceeded) {
                continue;
            }
            for source in &self.input.sources {
                let target = option.symbol_id.as_ref().map_or_else(
                    || option.path.as_deref() == Some(source.path.as_str()),
                    |symbol| source.stable_identity == format!("node:{symbol}"),
                );
                let Some(delivery) = source.delivery.as_ref() else {
                    continue;
                };
                if !target
                    || source.start_line != range.start_line
                    || source.end_line >= range.end_line
                    || delivery.requested_range != range
                    || !delivery.matches_delivery(source.start_line, source.end_line)
                    || option
                        .path
                        .as_ref()
                        .is_some_and(|path| path != &source.path)
                {
                    continue;
                }
                let Some(next) = delivery.next_start_line else {
                    continue;
                };
                if source.end_line.checked_add(1) != Some(next) {
                    continue;
                }
                let selector = PacketContinuationSelectorV1 {
                    stable_identity: source.stable_identity.clone(),
                    path: option.path.clone(),
                    symbol_id: option.symbol_id.clone(),
                    reason: PacketStructuralGapReasonV1::SourceBudgetExceeded,
                    source_range: Some(PacketSourceRangeV1 {
                        start_line: next,
                        end_line: range.end_line,
                    }),
                };
                let Some(mut tail) = drill_option_from_selector(&selector) else {
                    continue;
                };
                tail.source_label = continuation_source_label(&selector, &self.input.sources);
                if !current.options.contains(&tail) {
                    continue;
                }
                if candidate.support.iter().any(|row| {
                    row.kind == SupportUnitKindDto::SourceRange
                        && row.path.as_deref() == Some(source.path.as_str())
                        && row.start_line == Some(source.start_line)
                        && row.end_line == Some(source.end_line)
                        && row.snippet.as_deref() == Some(source.source.as_str())
                        && row.source_delivery.as_ref() == Some(delivery)
                        && option
                            .symbol_id
                            .as_ref()
                            .is_none_or(|id| row.symbol_id.as_ref() == Some(id))
                }) {
                    partial.insert(option.id.clone());
                    break;
                }
            }
        }
        partial
    }

    pub(crate) fn resolved_baseline_option_witnesses(
        &self,
        baseline: &AgentPacketDto,
        candidate: &AgentPacketDto,
    ) -> BTreeMap<String, SupportUnitDto> {
        let mut resolved = BTreeMap::new();
        let Some(drill) = baseline.disposition.drill.as_ref() else {
            return resolved;
        };
        for option in &drill.options {
            let Some(range) = option.source_range else {
                continue;
            };
            for source in &self.input.sources {
                let target = option.symbol_id.as_ref().map_or_else(
                    || option.path.as_deref() == Some(source.path.as_str()),
                    |symbol| source.stable_identity == format!("node:{symbol}"),
                );
                let Some(delivery) = source.delivery.as_ref() else {
                    continue;
                };
                if !target
                    || !delivery.matches_delivery(source.start_line, source.end_line)
                    || source.start_line > range.start_line
                    || source.end_line < range.end_line
                    || option
                        .path
                        .as_ref()
                        .is_some_and(|path| path != &source.path)
                {
                    continue;
                }
                if let Some(witness) = candidate.support.iter().find(|row| {
                    row.kind == SupportUnitKindDto::SourceRange
                        && row.path.as_deref() == Some(source.path.as_str())
                        && row.start_line == Some(source.start_line)
                        && row.end_line == Some(source.end_line)
                        && row.snippet.as_deref() == Some(source.source.as_str())
                        && row.source_delivery.as_ref() == Some(delivery)
                        && option
                            .symbol_id
                            .as_ref()
                            .is_none_or(|id| row.symbol_id.as_ref() == Some(id))
                }) {
                    resolved.insert(option.id.clone(), witness.clone());
                    break;
                }
            }
        }
        resolved
    }
}

#[cfg(test)]
fn retain_current_baseline_actions(
    baseline: &AgentPacketDto,
    candidate: &mut AgentPacketDto,
    resolved: &BTreeMap<String, SupportUnitDto>,
) -> bool {
    retain_current_baseline_actions_with_partial(baseline, candidate, resolved, &BTreeSet::new())
}

fn retain_current_baseline_actions_with_partial(
    baseline: &AgentPacketDto,
    candidate: &mut AgentPacketDto,
    resolved: &BTreeMap<String, SupportUnitDto>,
    partial: &BTreeSet<String>,
) -> bool {
    let Some(old) = baseline.disposition.drill.as_ref() else {
        return true;
    };
    let retained = old
        .options
        .iter()
        .filter(|option| !resolved.contains_key(&option.id))
        .cloned()
        .collect::<Vec<_>>();
    // With no original action outstanding, retain the ordinary canonical plan.
    if retained.is_empty() {
        return true;
    }
    let Some(current) = candidate.disposition.drill.as_mut() else {
        return false;
    };
    if old.parent_packet_id != current.parent_packet_id
        || old.core_generation_id != current.core_generation_id
        || old.retrieval_generation != current.retrieval_generation
        || old.max_bytes != current.max_bytes
        || old.max_hits != current.max_hits
        || old.max_depth != current.max_depth
        || old.remaining_rounds != current.remaining_rounds
        || retained
            .iter()
            .any(|option| !current.options.contains(option) && !partial.contains(&option.id))
    {
        return false;
    }
    // Full compiler continuation/omission inventories are not changed here.
    current.gap_ids = retained
        .iter()
        .map(|option| option.gap_id.clone())
        .collect();
    current.options = retained;
    true
}

pub(crate) struct FrozenPacketCompilationV1 {
    pub(crate) product: RepositoryDerivedCompilationV1,
    pub(crate) source_coverage: Vec<SourceCoverageObservationDto>,
    publication: PacketCompilationPublicationV1,
    source_refusals: Vec<String>,
    hydrated_sources: Vec<PacketHydratedSourceRangeV1>,
    published_file_sources: Vec<PacketHydratedSourceRangeV1>,
    context_floor: Vec<SupportUnitDto>,
}

#[derive(Debug, Clone)]
struct AuthenticatedPacketAdmissionV1 {
    receipt: PacketAdmissionReceiptV1,
    origin: AuthenticatedPacketOriginV1,
}

#[derive(Debug, Clone, Copy)]
enum AuthenticatedPacketOriginV1 {
    CoreNode(CoreNodeId),
    PublishedFile,
}

/// Hydrate exactly the packet-wide admitted identities and compile their
/// repository evidence while the core/retrieval publication is pinned. This
/// runs before any presentation or output-budget mutation.
#[cfg(test)]
pub(crate) fn freeze_packet_compilation(
    controller: &AppController,
    project_id: &str,
    probe_resolutions: &[PacketProbeResolutionDto],
    publication: Option<&EmbeddingVectorPublicationIdentityDto>,
    session: &PacketProofSession,
    request: Option<&AgentPacketRequestDto>,
) -> Result<FrozenPacketCompilationV1, codestory_contracts::api::ApiError> {
    let prepared = prepare_packet_compilation(
        controller,
        project_id,
        probe_resolutions,
        publication,
        session,
        request,
    )?;
    let product = prepared.default_product();
    Ok(prepared.into_frozen(product))
}

pub(crate) fn prepare_packet_compilation(
    controller: &AppController,
    project_id: &str,
    probe_resolutions: &[PacketProbeResolutionDto],
    publication: Option<&EmbeddingVectorPublicationIdentityDto>,
    session: &PacketProofSession,
    request: Option<&AgentPacketRequestDto>,
) -> Result<PreparedPacketCompilationV1, codestory_contracts::api::ApiError> {
    if let Some(request) = request {
        validate_requested_publication(controller, request, publication)?;
    }
    let mut admission_gaps = session.gaps();
    let storage = controller.open_storage_read_only()?;
    let mut source_refusals = Vec::new();
    let mut ranges =
        authenticated_continuation_ranges(controller, project_id, probe_resolutions, publication)?;
    let requested_identities = ranges.keys().cloned().collect::<BTreeSet<_>>();
    // Save the actual authenticated cursor before restoring any served prefix.
    let requested_source_minimum_ends = ranges
        .iter()
        .map(|(identity, range)| (identity.clone(), range.start_line))
        .collect::<BTreeMap<_, _>>();
    // Revalidate original served prefixes at their exact reserved rendered cost.
    // Never promote the unread requested tail into the protected floor.
    let issued = session.issued_source_record.borrow().clone();
    let issued_context_sources = if let Some(record) = issued.as_ref() {
        let mut old_admissions = Vec::new();
        let mut old_ranges = HashMap::new();
        for witness in &record.witnesses {
            let identity = witness.stable_identity();
            let mut admission = session
                .receipts()
                .into_iter()
                .find(|a| a.stable_identity == identity)
                .ok_or_else(|| {
                    codestory_contracts::api::ApiError::internal(
                        "Protected source admission missing.",
                    )
                })?;
            if admission.reserved_source_bytes < witness.rendered_bytes as u32 {
                return Err(codestory_contracts::api::ApiError::internal(
                    "Protected source reservation reduced.",
                ));
            }
            admission.reserved_source_bytes = witness.rendered_bytes as u32;
            old_admissions.push(admission);
            old_ranges.insert(identity.clone(), witness.delivery.requested_range);
            // A request for the next page of an old row extends the same
            // original prefix, retaining its receipt and source identity.
            ranges.insert(identity, witness.delivery.requested_range);
        }
        let mut old_gaps = Vec::new();
        let mut old_refusals = Vec::new();
        let mut old_alternatives = BTreeMap::new();
        let (_, old_sources, _) = hydrate_admitted_sources_with_ranges(
            controller,
            &storage,
            &old_admissions,
            &mut old_gaps,
            &old_ranges,
            &mut old_refusals,
            &mut old_alternatives,
        )?;
        if !old_gaps.is_empty()
            || !old_refusals.is_empty()
            || old_sources.len() != record.witnesses.len()
            || record
                .witnesses
                .iter()
                .any(|w| !old_sources.iter().any(|s| w.matches(s)))
        {
            return Err(codestory_contracts::api::ApiError::invalid_argument(
                "Previously issued source witnesses changed or cannot be revalidated.",
            ));
        }
        Some(old_sources)
    } else {
        None
    };
    // Only publication/node-authenticated ranges receive reservation priority.
    // Only verified rendered costs participate; no question or expected answer does.
    let (demands, minimum_pages) = authenticated_source_page_demands(
        controller,
        &storage,
        &session.receipts(),
        &ranges,
        &requested_source_minimum_ends,
    )?;
    session.reserve_source_pages_with_demands(&ranges.keys().cloned().collect(), &demands);
    let admissions = session.receipts();
    let mut source_page_alternatives = BTreeMap::new();
    let (authenticated_admissions, mut sources, file_navigation_paths) =
        hydrate_admitted_sources_with_ranges(
            controller,
            &storage,
            &admissions,
            &mut admission_gaps,
            &ranges,
            &mut source_refusals,
            &mut source_page_alternatives,
        )?;
    let source_paths = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    let source_coverage = observe_admitted_source_coverage(controller, &storage, &source_paths);
    for source in &mut sources {
        source.parser_completeness = parser_completeness_for_path(&source.path, &source_coverage);
    }
    for pages in source_page_alternatives.values_mut() {
        for page in pages {
            page.parser_completeness = parser_completeness_for_path(&page.path, &source_coverage);
            if let Some(minimum) = minimum_pages.get(&page.stable_identity)
                && page.end_line < minimum.end_line
                && let Some(fresh) = sources
                    .iter()
                    .find(|fresh| fresh.stable_identity == page.stable_identity)
                && let Some(admission) = admissions
                    .iter()
                    .find(|a| a.stable_identity == page.stable_identity)
                && let Some(replacement) =
                    revalidated_minimum_page(fresh, minimum, source_byte_cap(admission))
            {
                // Replace a nonadvancing shrink page; do not add extents or reads.
                *page = replacement;
            }
        }
    }
    let requested_source_identities = sources
        .iter()
        .filter(|source| requested_identities.contains(&source.stable_identity))
        .map(|source| source.stable_identity.clone())
        .collect();
    let published_file_sources = authenticated_admissions
        .iter()
        .filter(|admission| matches!(admission.origin, AuthenticatedPacketOriginV1::PublishedFile))
        .filter_map(|admission| {
            sources
                .iter()
                .find(|source| source.stable_identity == admission.receipt.stable_identity)
        })
        .cloned()
        .collect();
    let relations = hydrate_induced_relations(&storage, &authenticated_admissions)?;
    let admissions = authenticated_admissions
        .into_iter()
        .map(|admission| admission.receipt)
        .collect();
    let publication = PacketCompilationPublicationV1 {
        project_id: project_id.to_string(),
        core_generation_id: publication
            .map(|publication| publication.core_generation_id.clone())
            .unwrap_or_else(|| {
                controller
                    .active_core_publication()
                    .map(|pin| pin.generation_id)
                    .unwrap_or_default()
            }),
        retrieval_generation: publication
            .map(|publication| publication.retrieval_generation.clone()),
    };
    let input = PacketCompilationInputV1 {
        contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
        publication: publication.clone(),
        admissions,
        sources,
        relations,
        ambiguities: probe_ambiguities(probe_resolutions),
        admission_gaps,
    };
    Ok(PreparedPacketCompilationV1 {
        input,
        call_navigation: session.admitted_call_navigation(),
        source_coverage,
        publication,
        source_refusals,
        published_file_sources,
        file_navigation_paths,
        source_page_alternatives,
        requested_source_identities,
        requested_source_minimum_ends,
        issued_context_sources,
    })
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn replay_prepare_from_sealed_admissions(
    root: &Path,
    core: &Path,
    owned_cache: &Path,
    publication: PacketCompilationPublicationV1,
    admissions: Vec<PacketAdmissionReceiptV1>,
    call_navigation: Vec<PacketDirectedRelationV1>,
) -> Result<PreparedPacketCompilationV1, codestory_contracts::api::ApiError> {
    // CPU fixture state is isolated in an owned temporary cache. No live host
    // controller, activation, publication pointer, model or engine is opened.
    let controller = AppController::new_with_owned_cache_root(owned_cache);
    controller.state.lock().project_root = Some(root.to_owned());
    let storage = Store::open_immutable_generation(core)
        .map_err(|e| codestory_contracts::api::ApiError::internal(e.to_string()))?;
    let mut gaps = Vec::new();
    let mut refusals = Vec::new();
    let mut alternatives = BTreeMap::new();
    let (authenticated, mut sources, navigation) = hydrate_admitted_sources_with_ranges(
        &controller,
        &storage,
        &admissions,
        &mut gaps,
        &HashMap::new(),
        &mut refusals,
        &mut alternatives,
    )?;
    let paths = sources.iter().map(|s| s.path.clone()).collect::<Vec<_>>();
    let coverage = observe_admitted_source_coverage(&controller, &storage, &paths);
    for source in &mut sources {
        source.parser_completeness = parser_completeness_for_path(&source.path, &coverage)
    }
    for pages in alternatives.values_mut() {
        for source in pages {
            source.parser_completeness = parser_completeness_for_path(&source.path, &coverage)
        }
    }
    let relations = hydrate_induced_relations(&storage, &authenticated)?;
    let input = PacketCompilationInputV1 {
        contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
        publication: publication.clone(),
        admissions: authenticated.into_iter().map(|a| a.receipt).collect(),
        sources,
        relations,
        ambiguities: Vec::new(),
        admission_gaps: gaps,
    };
    Ok(PreparedPacketCompilationV1 {
        input,
        call_navigation,
        publication,
        source_coverage: coverage,
        source_refusals: refusals,
        published_file_sources: Vec::new(),
        file_navigation_paths: navigation,
        source_page_alternatives: alternatives,
        requested_source_identities: std::collections::BTreeSet::new(),
        requested_source_minimum_ends: BTreeMap::new(),
        issued_context_sources: None,
    })
}

pub(crate) fn validate_requested_publication(
    controller: &AppController,
    request: &AgentPacketRequestDto,
    publication: Option<&EmbeddingVectorPublicationIdentityDto>,
) -> Result<(), codestory_contracts::api::ApiError> {
    let actual_core = controller
        .active_core_publication()
        .map(|pin| pin.generation_id);
    if request
        .core_generation_id
        .as_deref()
        .is_some_and(|expected| Some(expected) != actual_core.as_deref())
        || request
            .retrieval_generation
            .as_deref()
            .is_some_and(|expected| {
                publication.is_none_or(|pin| pin.retrieval_generation != expected)
            })
    {
        return Err(codestory_contracts::api::ApiError::invalid_argument(
            "pinned continuation publication changed before source hydration",
        ));
    }
    Ok(())
}

fn authenticated_continuation_ranges(
    controller: &AppController,
    project_id: &str,
    resolutions: &[PacketProbeResolutionDto],
    publication: Option<&EmbeddingVectorPublicationIdentityDto>,
) -> Result<HashMap<String, PacketSourceRangeV1>, codestory_contracts::api::ApiError> {
    let mut ranges = HashMap::new();
    for resolution in resolutions {
        let codestory_contracts::api::PacketProbeDto::Continuation {
            contract_version,
            project_id: expected_project,
            core_generation_id,
            retrieval_generation,
            selector,
        } = &resolution.probe
        else {
            continue;
        };
        let Some(range) = selector.source_range else {
            continue;
        };
        let core = controller
            .active_core_publication()
            .map(|pin| pin.generation_id);
        if *contract_version != codestory_contracts::api::PACKET_PROBE_CONTRACT_VERSION
            || expected_project != project_id
            || core.as_deref() != Some(core_generation_id.as_str())
            || retrieval_generation.as_deref()
                != publication.map(|pin| pin.retrieval_generation.as_str())
            || resolution.status != PacketProbeResolutionStatusDto::Continuation
            || !range.is_valid()
            || resolution
                .symbol_id
                .as_ref()
                .is_none_or(|id| selector.stable_identity != format!("node:{id}"))
        {
            return Err(codestory_contracts::api::ApiError::invalid_argument(
                "source continuation is not authenticated against the pinned publication and node",
            ));
        }
        if ranges
            .insert(selector.stable_identity.clone(), range)
            .is_some_and(|previous| previous != range)
        {
            return Err(codestory_contracts::api::ApiError::invalid_argument(
                "conflicting source ranges for one admitted identity",
            ));
        }
    }
    Ok(ranges)
}

fn attach_file_navigation_paths(support: &mut [SupportUnitDto], paths: &HashMap<String, String>) {
    for unit in support {
        if unit.kind == SupportUnitKindDto::SymbolLocation
            && let Some(identity) = unit.id.strip_prefix("symbol:")
            && let Some(path) = paths.get(identity)
        {
            unit.path = Some(path.clone());
        }
    }
}

pub(crate) fn apply_frozen_packet_compilation(
    packet: &mut AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
    frozen: FrozenPacketCompilationV1,
) {
    apply_compilation_state(
        packet,
        request,
        &frozen.product,
        &frozen.publication,
        &frozen.source_refusals,
        &frozen.hydrated_sources,
        &frozen.published_file_sources,
    );
    // Rejected previews never enter final-support telemetry.
    crate::agent::packet_batch::observe_packet_raf_final_support(
        &final_support_identities_for_observation(&packet.support),
    );
}

/// Commit a proposal whose shared compilation state has already been applied,
/// presentation-bounded and validated by the public record builder. Do not
/// reclassify or mutate the exact measured packet at the freeze boundary.
pub(crate) fn commit_measured_packet_compilation(
    packet: &AgentPacketDto,
    frozen: FrozenPacketCompilationV1,
) -> Result<(), codestory_contracts::api::ApiError> {
    if !source_floor_retained(&packet.support, &frozen.context_floor)
        || packet.support != frozen.product.support
        || packet
            .answer
            .source_coverage
            .iter()
            .any(|observation| !frozen.source_coverage.contains(observation))
    {
        return Err(codestory_contracts::api::ApiError::internal(
            "Frozen compilation differs from measured source delivery.",
        ));
    }
    crate::agent::packet_batch::observe_packet_raf_final_support(
        &final_support_identities_for_observation(&packet.support),
    );
    Ok(())
}

fn apply_compilation_state(
    packet: &mut AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
    product: &RepositoryDerivedCompilationV1,
    publication: &PacketCompilationPublicationV1,
    source_refusals: &[String],
    hydrated_sources: &[PacketHydratedSourceRangeV1],
    published_file_sources: &[PacketHydratedSourceRangeV1],
) {
    packet.support = product.support.clone();
    packet.disposition = classify_packet_disposition(
        packet,
        request,
        &product.continuation,
        hydrated_sources,
        &publication.project_id,
        publication.core_generation_id.clone(),
        publication.retrieval_generation.clone(),
        published_file_sources,
    );
    if !source_refusals.is_empty()
        && packet.disposition.kind == codestory_contracts::api::PacketDispositionKindDto::Supported
    {
        packet.disposition =
            PacketDispositionDto::not_established("an admitted source could not be delivered");
    }
    packet
        .disposition
        .omission_receipts
        .extend(source_refusals.iter().cloned());
    // A verified page can permit positive evidence without proving index
    // coverage. Keep those observed gaps in the actual measured response.
    for gap in PacketCoverageInput::from_observations(&packet.answer.source_coverage).gaps() {
        if !packet.disposition.omission_receipts.contains(&gap) {
            packet.disposition.omission_receipts.push(gap);
        }
    }
}

fn final_support_identities_for_observation(support: &[SupportUnitDto]) -> Vec<String> {
    let mut identities = Vec::new();
    let mut seen = BTreeSet::new();
    for unit in support {
        let Some(identity) = final_support_stable_identity(unit) else {
            continue;
        };
        if seen.insert(identity.clone()) {
            identities.push(identity);
        }
    }
    identities
}

// Expansion may add complete physical lines, but cannot erase or rewrite a floor.
// Compare canonical hydrated characters, not numeric display alignment.
fn source_floor_retained(candidate: &[SupportUnitDto], floor: &[SupportUnitDto]) -> bool {
    floor
        .iter()
        .filter(|old| old.kind == SupportUnitKindDto::SourceRange)
        .all(|old| {
            let Some(identity) = final_support_stable_identity(old) else {
                return false;
            };
            let Some(old_delivery) = old.source_delivery.as_ref() else {
                // Legacy FILE/path rows carry no paging receipt. Keep that
                // authenticated row byte-for-byte; never synthesize a hash.
                return candidate.iter().any(|new| new == old);
            };
            let Some(prefix) = old
                .snippet
                .as_deref()
                .and_then(|text| text.strip_suffix("\u{0060}\u{0060}\u{0060}"))
            else {
                return false;
            };
            candidate.iter().any(|new| {
                new.kind == SupportUnitKindDto::SourceRange
                    && final_support_stable_identity(new).as_ref() == Some(&identity)
                    && new.path == old.path
                    && new.start_line == old.start_line
                    && new
                        .end_line
                        .zip(old.end_line)
                        .is_some_and(|(end, previous)| end >= previous)
                    && new.source_delivery.as_ref().is_some_and(|delivery| {
                        delivery.source_sha256 == old_delivery.source_sha256
                            && delivery.requested_range == old_delivery.requested_range
                            && new
                                .start_line
                                .zip(new.end_line)
                                .is_some_and(|(start, end)| delivery.matches_delivery(start, end))
                    })
                    && new
                        .snippet
                        .as_deref()
                        .is_some_and(|text| text.starts_with(prefix))
            })
        })
}

fn final_support_stable_identity(unit: &SupportUnitDto) -> Option<String> {
    match unit.kind {
        SupportUnitKindDto::SymbolLocation => unit
            .id
            .strip_prefix("symbol:")
            .map(str::to_string)
            .or_else(|| {
                unit.symbol_id
                    .as_ref()
                    .map(|symbol_id| format!("node:{symbol_id}"))
            }),
        SupportUnitKindDto::SourceRange => unit
            .symbol_id
            .as_ref()
            .map(|symbol_id| format!("node:{symbol_id}"))
            .or_else(|| {
                unit.path
                    .as_ref()
                    .filter(|path| !path.trim().is_empty())
                    .map(|path| format!("path:{path}"))
            }),
        SupportUnitKindDto::TypedGraphEdge | SupportUnitKindDto::CompleteQueryNegative => None,
    }
}

type HydratedAdmittedSources = (
    Vec<AuthenticatedPacketAdmissionV1>,
    Vec<PacketHydratedSourceRangeV1>,
    HashMap<String, String>,
);

#[cfg(test)]
fn hydrate_admitted_sources(
    controller: &AppController,
    storage: &Store,
    admissions: &[PacketAdmissionReceiptV1],
    admission_gaps: &mut Vec<PacketAdmissionGapV1>,
) -> Result<HydratedAdmittedSources, codestory_contracts::api::ApiError> {
    hydrate_admitted_sources_with_ranges(
        controller,
        storage,
        admissions,
        admission_gaps,
        &HashMap::new(),
        &mut Vec::new(),
        &mut BTreeMap::new(),
    )
}

fn hydrate_admitted_sources_with_ranges(
    controller: &AppController,
    storage: &Store,
    admissions: &[PacketAdmissionReceiptV1],
    admission_gaps: &mut Vec<PacketAdmissionGapV1>,
    ranges: &HashMap<String, PacketSourceRangeV1>,
    source_refusals: &mut Vec<String>,
    source_page_alternatives: &mut BTreeMap<usize, Vec<PacketHydratedSourceRangeV1>>,
) -> Result<HydratedAdmittedSources, codestory_contracts::api::ApiError> {
    let project_root = controller.require_project_root()?;
    let mut authenticated = Vec::new();
    let mut sources = Vec::new();
    let mut file_navigation_paths = HashMap::new();
    let mut callable_body_cache = HashMap::new();
    for admission in admissions {
        let previous_refusal_count = source_refusals.len();
        let (origin, is_file_source, result) =
            if let Some(raw_id) = admission.stable_identity.strip_prefix("node:") {
                let Some(node) = authenticated_node(storage, raw_id)? else {
                    push_admission_gap(
                        admission_gaps,
                        admission,
                        PacketAdmissionGapKindV1::StableIdentityMissing,
                        false,
                    );
                    continue;
                };
                if let Some(file_id) = (node.kind == CoreNodeKind::FILE)
                    .then_some(node.id)
                    .or(node.file_node_id)
                    && let Some(file) = storage.get_file_by_id(file_id.0).map_err(|error| {
                        codestory_contracts::api::ApiError::internal(format!(
                            "Failed to authenticate admitted packet file: {error}"
                        ))
                    })?
                    && let Some(path) = authenticated_file_navigation_path(controller, &file)
                {
                    file_navigation_paths.insert(admission.stable_identity.clone(), path);
                }
                (
                    AuthenticatedPacketOriginV1::CoreNode(node.id),
                    node.kind == CoreNodeKind::FILE,
                    hydrate_admitted_node_source_with_range_cached(
                        controller,
                        storage,
                        admission,
                        &node,
                        ranges.get(&admission.stable_identity).copied(),
                        source_refusals,
                        source_page_alternatives,
                        &mut callable_body_cache,
                    ),
                )
            } else if let Some(path) = admission.stable_identity.strip_prefix("path:") {
                if let Some(file) = find_admitted_file(storage, &project_root, path)? {
                    if let Some(path) = authenticated_file_navigation_path(controller, &file) {
                        file_navigation_paths.insert(admission.stable_identity.clone(), path);
                    }
                    (
                        AuthenticatedPacketOriginV1::CoreNode(CoreNodeId(file.id)),
                        true,
                        hydrate_admitted_file_source(controller, storage, admission, &file),
                    )
                } else {
                    let witness = match super::retrieval_primary::read_admitted_published_file(
                        controller, admission,
                    ) {
                        Ok(Some(witness)) => witness,
                        _ => {
                            push_admission_gap(
                                admission_gaps,
                                admission,
                                PacketAdmissionGapKindV1::SourceUnavailable,
                                true,
                            );
                            continue;
                        }
                    };
                    file_navigation_paths
                        .insert(admission.stable_identity.clone(), witness.path().to_owned());
                    (
                        AuthenticatedPacketOriginV1::PublishedFile,
                        true,
                        hydrate_published_file_source(admission, &witness),
                    )
                }
            } else {
                push_admission_gap(
                    admission_gaps,
                    admission,
                    PacketAdmissionGapKindV1::StableIdentityMissing,
                    false,
                );
                continue;
            };
        match result {
            Ok(source) => {
                authenticated.push(AuthenticatedPacketAdmissionV1 {
                    receipt: admission.clone(),
                    origin,
                });
                sources.push(source);
            }
            Err(PacketAdmissionGapKindV1::SourceBudgetExceeded) => {
                if admission.reserved_source_bytes as usize >= INTERIM_SOURCE_ROW_UPPER_BOUND
                    && source_refusals.len() == previous_refusal_count
                {
                    source_refusals.push(if is_file_source {
                        file_source_budget_refusal(
                            admission,
                            file_navigation_paths
                                .get(&admission.stable_identity)
                                .map(String::as_str),
                        )
                    } else {
                        format!(
                            "Admitted source {} did not fit the {}-byte source-row cap; no source range was delivered under the pinned publication.",
                            admission.packet_ordinal, source_byte_cap(admission),
                        )
                    });
                }
                authenticated.push(AuthenticatedPacketAdmissionV1 {
                    receipt: admission.clone(),
                    origin,
                });
                push_admission_gap(
                    admission_gaps,
                    admission,
                    PacketAdmissionGapKindV1::SourceBudgetExceeded,
                    true,
                );
            }
            Err(kind) => {
                file_navigation_paths.remove(&admission.stable_identity);
                let missing_bounds = kind == PacketAdmissionGapKindV1::SourceBoundMissing;
                source_refusals.push(format!(
                    "Admitted source {}: {}; no source range was delivered under the pinned publication.",
                    admission.packet_ordinal,
                    if missing_bounds { "source bounds are unavailable" } else { "source bytes are unavailable" },
                ));
                // Descriptor allocation bounds can be missing before admission.
                // Only a failed authenticated hydration becomes terminal here.
                let kind = if missing_bounds {
                    PacketAdmissionGapKindV1::SourceUnavailable
                } else {
                    kind
                };
                push_admission_gap(admission_gaps, admission, kind, true);
            }
        }
    }
    Ok((authenticated, sources, file_navigation_paths))
}

fn file_source_budget_refusal(
    admission: &PacketAdmissionReceiptV1,
    authenticated_path: Option<&str>,
) -> String {
    // This reason follows verified whole-file hydration. Keep it ahead of
    // identity text so a long path cannot hide the evidence boundary.
    let mut message = format!(
        "Admitted file {}: verified whole-file source or numbered rendering exceeds the {}-byte source-row cap; navigation only; no source range was delivered under the pinned publication. Identity: {}",
        admission.packet_ordinal,
        source_byte_cap(admission),
        admission.stable_identity,
    );
    if let Some(path) = authenticated_path {
        let _ = write!(message, "; file: {path}");
    }
    let cap = codestory_contracts::packet_projection_v3::MESSAGE_MAX_BYTES_V3;
    if message.len() > cap {
        let mut end = cap - "...".len();
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str("...");
    }
    message
}

fn authenticated_file_navigation_path(
    controller: &AppController,
    file: &FileInfo,
) -> Option<String> {
    let path = file.path.to_string_lossy();
    let resolved = controller.resolve_project_file_path(&path, false).ok()?;
    let project_root = controller.require_project_root().ok()?;
    let indexed_path = if file.path.is_absolute() {
        file.path.clone()
    } else {
        project_root.join(&file.path)
    };
    if !codestory_workspace::same_workspace_path(&resolved, &indexed_path) {
        return None;
    }
    codestory_workspace::workspace_relative_path(&project_root, &resolved)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

fn push_admission_gap(
    admission_gaps: &mut Vec<PacketAdmissionGapV1>,
    admission: &PacketAdmissionReceiptV1,
    kind: PacketAdmissionGapKindV1,
    expose_authenticated_identity: bool,
) {
    admission_gaps.push(PacketAdmissionGapV1 {
        kind,
        stable_identity: expose_authenticated_identity.then(|| admission.stable_identity.clone()),
        exact_selector_ordinal: (admission.origin == PacketAdmissionOriginV1::ExactTypedSelector)
            .then_some(admission.packet_ordinal),
    });
}

fn authenticated_node(
    storage: &Store,
    raw_id: &str,
) -> Result<Option<CoreNode>, codestory_contracts::api::ApiError> {
    let Ok(node_id) = raw_id.parse::<i64>() else {
        return Ok(None);
    };
    storage.get_node(CoreNodeId(node_id)).map_err(|error| {
        codestory_contracts::api::ApiError::internal(format!(
            "Failed to authenticate admitted packet node: {error}"
        ))
    })
}

type CallableBodyCacheV1 =
    HashMap<(i64, String), Option<Vec<codestory_indexer::ParsedCallableBodySpan>>>;

#[cfg(test)]
fn hydrate_admitted_node_source_with_range(
    controller: &AppController,
    storage: &Store,
    admission: &PacketAdmissionReceiptV1,
    node: &CoreNode,
    requested: Option<PacketSourceRangeV1>,
    source_refusals: &mut Vec<String>,
    source_page_alternatives: &mut BTreeMap<usize, Vec<PacketHydratedSourceRangeV1>>,
) -> Result<PacketHydratedSourceRangeV1, PacketAdmissionGapKindV1> {
    hydrate_admitted_node_source_with_range_cached(
        controller,
        storage,
        admission,
        node,
        requested,
        source_refusals,
        source_page_alternatives,
        &mut HashMap::new(),
    )
}

// Keep admission, source bounds, and hydration caches explicit at this boundary.
#[allow(clippy::too_many_arguments)]
fn hydrate_admitted_node_source_with_range_cached(
    controller: &AppController,
    storage: &Store,
    admission: &PacketAdmissionReceiptV1,
    node: &CoreNode,
    requested: Option<PacketSourceRangeV1>,
    source_refusals: &mut Vec<String>,
    source_page_alternatives: &mut BTreeMap<usize, Vec<PacketHydratedSourceRangeV1>>,
    callable_body_cache: &mut CallableBodyCacheV1,
) -> Result<PacketHydratedSourceRangeV1, PacketAdmissionGapKindV1> {
    let file_id = if node.kind == CoreNodeKind::FILE {
        node.id
    } else {
        node.file_node_id
            .ok_or(PacketAdmissionGapKindV1::SourceBoundMissing)?
    };
    let file = storage
        .get_file_by_id(file_id.0)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?
        .ok_or(PacketAdmissionGapKindV1::SourceUnavailable)?;
    if node.kind == CoreNodeKind::FILE {
        if requested.is_some() {
            return Err(PacketAdmissionGapKindV1::SourceBoundMissing);
        }
        return hydrate_admitted_file_source(controller, storage, admission, &file);
    }
    let (start_line, end_line) = valid_source_bounds(node.start_line, node.end_line)
        .ok_or(PacketAdmissionGapKindV1::SourceBoundMissing)?;
    let requested = requested.unwrap_or(PacketSourceRangeV1 {
        start_line,
        end_line,
    });
    if !requested.is_valid() || requested.start_line < start_line || requested.end_line > end_line {
        return Err(PacketAdmissionGapKindV1::SourceBoundMissing);
    }
    let project_root = controller
        .require_project_root()
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    let source = crate::search_evidence::verified_file_checked(storage, Some(&project_root), &file)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    if source.content.lines().count() >= requested.end_line as usize {
        let raw = source
            .content
            .split_inclusive('\n')
            .nth(requested.start_line as usize - 1)
            .ok_or(PacketAdmissionGapKindV1::SourceBoundMissing)?;
        let line = raw
            .strip_suffix("\r\n")
            .or_else(|| raw.strip_suffix('\n'))
            .unwrap_or(raw);
        let required = format!(">{:>5} | ", requested.start_line)
            .len()
            .saturating_add(line.len())
            .saturating_add(1)
            .saturating_add(SOURCE_PAGE_FENCE_BYTES);
        if required > source_byte_cap(admission) {
            let remaining = source
                .content
                .split_inclusive('\n')
                .skip(requested.start_line as usize - 1)
                .take((requested.end_line - requested.start_line + 1) as usize)
                .map(str::len)
                .sum::<usize>();
            source_refusals.push(format!(
                "{}: requested lines {}-{}; zero complete lines delivered; {} source bytes remain; first complete line requires {} rendered bytes, exceeding the {}-byte row cap; indexed SHA-256 {}.",
                admission.stable_identity, requested.start_line, requested.end_line,
                remaining, required, source_byte_cap(admission), source.content_sha256,
            ));
            return Err(PacketAdmissionGapKindV1::SourceBudgetExceeded);
        }
    }
    let (markdown, delivered_end, delivery) = complete_node_page(
        &source.content,
        requested,
        source_byte_cap(admission),
        &source.content_sha256,
    )?;
    let callable_body = if matches!(
        node.kind,
        CoreNodeKind::FUNCTION | CoreNodeKind::METHOD | CoreNodeKind::MACRO
    ) {
        let parsed = callable_body_cache
            .entry((file.id, source.content_sha256.clone()))
            .or_insert_with(|| {
                codestory_indexer::parsed_callable_body_spans(&file.path, &source.content)
            });
        parsed.as_ref().and_then(|spans| {
            let mut matches = spans.iter().filter(|span| {
                Some(span.start_line) == node.start_line
                    && Some(span.start_column) == node.start_col
                    && Some(span.end_line) == node.end_line
                    && Some(span.end_column) == node.end_col
            });
            let span = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            Some(codestory_contracts::compilation::PacketCallableBodyV1 {
                unit_range: PacketSourceRangeV1 {
                    start_line: span.start_line,
                    end_line: span.end_line,
                },
                body_range: PacketSourceRangeV1 {
                    start_line: span.body_start_line,
                    end_line: span.body_end_line,
                },
                source_sha256: source.content_sha256.clone(),
            })
        })
    } else {
        None
    };
    let page = PacketHydratedSourceRangeV1 {
        callable_body,
        stable_identity: admission.stable_identity.clone(),
        path: authenticated_file_navigation_path(controller, &file)
            .ok_or(PacketAdmissionGapKindV1::SourceUnavailable)?,
        symbol: Some(
            node.qualified_name
                .clone()
                .unwrap_or_else(|| node.serialized_name.clone()),
        ),
        start_line: requested.start_line,
        end_line: delivered_end,
        source: markdown,
        parser_completeness: PacketParserCompletenessV1::Unknown,
        delivery: Some(delivery),
    };
    // Normalize shrink levels so heterogeneous fair reservations still have
    // at most four extents, each measured by the unchanged row compiler.
    for cap in [2048, 1024, INTERIM_SOURCE_ROW_UPPER_BOUND] {
        if cap >= source_byte_cap(admission) {
            continue;
        }
        if let Ok((body, end, delivery)) =
            complete_node_page(&source.content, requested, cap, &source.content_sha256)
        {
            let mut alternative = page.clone();
            alternative.source = body;
            alternative.end_line = end;
            alternative.delivery = Some(delivery);
            source_page_alternatives
                .entry(cap)
                .or_default()
                .push(alternative);
        }
    }
    Ok(page)
}

/// Packet pages never clip a numbered physical line or infer bounds from prose.
fn complete_node_page(
    text: &str,
    requested: PacketSourceRangeV1,
    cap: usize,
    source_sha256: &str,
) -> Result<(String, u32, PacketSourceDeliveryV1), PacketAdmissionGapKindV1> {
    if !requested.is_valid() || text.lines().count() < requested.end_line as usize {
        return Err(PacketAdmissionGapKindV1::SourceBoundMissing);
    }
    let mut markdown = String::from("```text\n");
    let mut end = None;
    let mut remaining = 0_u64;
    let mut next_line_bytes = None;
    for (index, raw) in text
        .split_inclusive('\n')
        .enumerate()
        .skip(requested.start_line as usize - 1)
        .take((requested.end_line - requested.start_line + 1) as usize)
    {
        let line_number = index as u32 + 1;
        let line = raw
            .strip_suffix("\r\n")
            .or_else(|| raw.strip_suffix('\n'))
            .unwrap_or(raw);
        let prefix = format!(
            "{}{:>5} | ",
            if end.is_none() { ">" } else { " " },
            line_number
        );
        let rendered_bytes = prefix.len().saturating_add(line.len()).saturating_add(1);
        if next_line_bytes.is_none()
            && markdown
                .len()
                .saturating_add(rendered_bytes)
                .saturating_add(3)
                <= cap
        {
            markdown.push_str(&prefix);
            markdown.push_str(line);
            markdown.push('\n');
            end = Some(line_number);
        } else {
            remaining = remaining.saturating_add(raw.len() as u64);
            next_line_bytes
                .get_or_insert(rendered_bytes.saturating_add(SOURCE_PAGE_FENCE_BYTES) as u64);
        }
    }
    let end = end.ok_or(PacketAdmissionGapKindV1::SourceBudgetExceeded)?;
    markdown.push_str("```");
    let blocked =
        next_line_bytes.filter(|bytes| *bytes > cap.max(INTERIM_SOURCE_ROW_UPPER_BOUND) as u64);
    let next_start_line = (remaining > 0 && blocked.is_none()).then(|| end + 1);
    let delivery = PacketSourceDeliveryV1 {
        requested_range: requested,
        source_sha256: source_sha256.to_owned(),
        remaining_source_bytes: remaining,
        next_start_line,
        blocked_line_rendered_bytes: blocked,
    };
    if !delivery.matches_delivery(requested.start_line, end) {
        return Err(PacketAdmissionGapKindV1::SourceBoundMissing);
    }
    Ok((markdown, end, delivery))
}

fn hydrate_admitted_file_source(
    controller: &AppController,
    storage: &Store,
    admission: &PacketAdmissionReceiptV1,
    file: &FileInfo,
) -> Result<PacketHydratedSourceRangeV1, PacketAdmissionGapKindV1> {
    // A file identity has no source focus. Preserve a complete short file when
    // its pinned bytes fit the admission; otherwise leave it as navigation.
    // Reading its first few lines would falsely promote an arbitrary header
    // to a source witness for the retrieval question.
    if !file.indexed || !file.complete || file.line_count == 0 {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    }
    let expected_hash = storage
        .get_file_content_hash(file.id)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?
        .ok_or(PacketAdmissionGapKindV1::SourceUnavailable)?;
    let path = file.path.to_string_lossy();
    let resolved = controller
        .resolve_project_file_path(&path, false)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    let project_root = controller
        .require_project_root()
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    let indexed_path = if file.path.is_absolute() {
        file.path.clone()
    } else {
        project_root.join(&file.path)
    };
    if !codestory_workspace::same_workspace_path(&resolved, &indexed_path) {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    }
    let cap = source_byte_cap(admission);
    let mut bytes = Vec::new();
    std::fs::File::open(&resolved)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?
        .take((FILE_NAVIGATION_VERIFICATION_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    if bytes.len() > FILE_NAVIGATION_VERIFICATION_MAX_BYTES {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    }
    if format!("{:x}", Sha256::digest(&bytes)) != expected_hash {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    }
    let Ok(text) = codestory_contracts::source_text::decode_source_text(&file.path, &bytes) else {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    };
    if text.lines().count() != file.line_count as usize {
        return Err(PacketAdmissionGapKindV1::SourceBoundMissing);
    }
    if text.len() > cap {
        return Err(PacketAdmissionGapKindV1::SourceBudgetExceeded);
    }
    let markdown = complete_file_markdown(&text, file.line_count, cap)?;
    let canonical_path = codestory_workspace::workspace_relative_path(&project_root, &resolved)
        .ok_or(PacketAdmissionGapKindV1::SourceUnavailable)?
        .to_string_lossy()
        .replace('\\', "/");
    hydrated_source(admission, &canonical_path, None, &markdown)
}

fn hydrate_published_file_source(
    admission: &PacketAdmissionReceiptV1,
    witness: &codestory_retrieval::PublishedFileSourceWitness,
) -> Result<PacketHydratedSourceRangeV1, PacketAdmissionGapKindV1> {
    let text = witness.content();
    let line_count = u32::try_from(text.lines().count())
        .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
    if line_count == 0 {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    }
    let cap = source_byte_cap(admission);
    if text.len() > cap {
        return Err(PacketAdmissionGapKindV1::SourceBudgetExceeded);
    }
    let markdown = complete_file_markdown(text, line_count, cap)?;
    let mut source = hydrated_source(admission, witness.path(), None, &markdown)?;
    source.delivery = Some(PacketSourceDeliveryV1 {
        requested_range: PacketSourceRangeV1 {
            start_line: 1,
            end_line: line_count,
        },
        source_sha256: witness.source_sha256().to_owned(),
        remaining_source_bytes: 0,
        next_start_line: None,
        blocked_line_rendered_bytes: None,
    });
    Ok(source)
}

fn complete_file_markdown(
    text: &str,
    line_count: u32,
    cap: usize,
) -> Result<String, PacketAdmissionGapKindV1> {
    // The focused-snippet helper caps context at 50 lines. File admission is
    // different: it may claim source only when every line fits the row cap.
    let mut markdown = String::from("```text\n");
    for (index, line) in text.lines().enumerate() {
        let marker = if index == 0 { ">" } else { " " };
        writeln!(markdown, "{marker}{:>5} | {line}", index + 1)
            .map_err(|_| PacketAdmissionGapKindV1::SourceUnavailable)?;
        if markdown.len().saturating_add(3) > cap {
            return Err(PacketAdmissionGapKindV1::SourceBudgetExceeded);
        }
    }
    markdown.push_str("```");
    if source_receipt_line_range(&markdown) != Some((1, line_count)) {
        return Err(PacketAdmissionGapKindV1::SourceBudgetExceeded);
    }
    Ok(markdown)
}

fn hydrated_source(
    admission: &codestory_contracts::compilation::PacketAdmissionReceiptV1,
    path: &str,
    symbol: Option<String>,
    source: &str,
) -> Result<PacketHydratedSourceRangeV1, PacketAdmissionGapKindV1> {
    if source.trim().is_empty() {
        return Err(PacketAdmissionGapKindV1::SourceUnavailable);
    }
    let (start_line, end_line) =
        source_receipt_line_range(source).ok_or(PacketAdmissionGapKindV1::SourceBoundMissing)?;
    Ok(PacketHydratedSourceRangeV1 {
        callable_body: None,
        stable_identity: admission.stable_identity.clone(),
        path: path.to_string(),
        symbol,
        start_line,
        end_line,
        source: source.to_string(),
        delivery: None,
        parser_completeness: PacketParserCompletenessV1::Unknown,
    })
}

/// Plan only already-admitted, hash-verified node pages. This is a bounded
/// cost preflight, not source delivery: hydration rereads and verifies normally.
/// It avoids reserving 4 KiB for a small complete body while starving context.
struct AuthenticatedMinimumSourcePage {
    source: String,
    end_line: u32,
    delivery: PacketSourceDeliveryV1,
}

fn minimum_source_page(
    text: &str,
    requested: PacketSourceRangeV1,
    minimum: u32,
    hash: &str,
) -> Option<AuthenticatedMinimumSourcePage> {
    if minimum < requested.start_line || minimum > requested.end_line {
        return None;
    }
    let (prefix, end, _) = complete_node_page(
        text,
        PacketSourceRangeV1 {
            start_line: requested.start_line,
            end_line: minimum,
        },
        INTERIM_SOURCE_CONTINUATION_UPPER_BOUND,
        hash,
    )
    .ok()?;
    if end != minimum {
        return None;
    }
    // Use the full original requested range for truthful raw remainder/cursor.
    let (source, end_line, delivery) =
        complete_node_page(text, requested, prefix.len(), hash).ok()?;
    (end_line == minimum).then_some(AuthenticatedMinimumSourcePage {
        source,
        end_line,
        delivery,
    })
}

fn revalidated_minimum_page(
    fresh: &PacketHydratedSourceRangeV1,
    minimum: &AuthenticatedMinimumSourcePage,
    cap: usize,
) -> Option<PacketHydratedSourceRangeV1> {
    let delivery = fresh.delivery.as_ref()?;
    let prefix = minimum.source.strip_suffix("```")?;
    if minimum.source.len() > cap
        || minimum.source.len() > INTERIM_SOURCE_CONTINUATION_UPPER_BOUND
        || fresh.end_line < minimum.end_line
        || delivery.source_sha256 != minimum.delivery.source_sha256
        || delivery.requested_range != minimum.delivery.requested_range
        || !minimum
            .delivery
            .matches_delivery(fresh.start_line, minimum.end_line)
        || !fresh.source.starts_with(prefix)
    {
        return None;
    }
    let mut result = fresh.clone();
    result.source = minimum.source.clone();
    result.end_line = minimum.end_line;
    result.delivery = Some(minimum.delivery.clone());
    Some(result)
}

type AuthenticatedSourcePageDemands = (
    HashMap<String, usize>,
    HashMap<String, AuthenticatedMinimumSourcePage>,
);

fn authenticated_source_page_demands(
    controller: &AppController,
    storage: &Store,
    admissions: &[PacketAdmissionReceiptV1],
    ranges: &HashMap<String, PacketSourceRangeV1>,
    minimum_ends: &BTreeMap<String, u32>,
) -> Result<AuthenticatedSourcePageDemands, codestory_contracts::api::ApiError> {
    let root = controller.require_project_root()?;
    let mut groups: HashMap<i64, (FileInfo, Vec<(String, PacketSourceRangeV1)>)> = HashMap::new();
    for admission in admissions {
        let Some(id) = admission.stable_identity.strip_prefix("node:") else {
            continue;
        };
        let Some(node) = authenticated_node(storage, id)? else {
            continue;
        };
        if node.kind == CoreNodeKind::FILE {
            continue;
        }
        let Some(file_id) = node.file_node_id else {
            continue;
        };
        let Some(file) = storage
            .get_file_by_id(file_id.0)
            .map_err(|error| codestory_contracts::api::ApiError::internal(error.to_string()))?
        else {
            continue;
        };
        if authenticated_file_navigation_path(controller, &file).is_none() {
            continue;
        }
        let Some((start_line, end_line)) = valid_source_bounds(node.start_line, node.end_line)
        else {
            continue;
        };
        let requested =
            ranges
                .get(&admission.stable_identity)
                .copied()
                .unwrap_or(PacketSourceRangeV1 {
                    start_line,
                    end_line,
                });
        if !requested.is_valid()
            || requested.start_line < start_line
            || requested.end_line > end_line
        {
            continue;
        }
        groups
            .entry(file.id)
            .or_insert_with(|| (file, Vec::new()))
            .1
            .push((admission.stable_identity.clone(), requested));
    }
    let mut demands = HashMap::new();
    let mut minimum_pages = HashMap::new();
    for (_, (file, pages)) in groups {
        let Ok(source) = crate::search_evidence::verified_file_checked(storage, Some(&root), &file)
        else {
            continue;
        };
        // Only one verified file is held at a time. Hydration rechecks its hash.
        let line_count = source.content.lines().count();
        for (identity, requested) in pages {
            if line_count < requested.end_line as usize {
                continue;
            }
            let mut bytes = SOURCE_PAGE_FENCE_BYTES;
            for (offset, raw) in source
                .content
                .split_inclusive('\n')
                .skip(requested.start_line as usize - 1)
                .take((requested.end_line - requested.start_line + 1) as usize)
                .enumerate()
            {
                let text = raw
                    .strip_suffix("\r\n")
                    .or_else(|| raw.strip_suffix('\n'))
                    .unwrap_or(raw);
                let number = requested.start_line + offset as u32;
                bytes = bytes
                    .saturating_add(
                        format!("{}{:>5} | ", if offset == 0 { ">" } else { " " }, number).len(),
                    )
                    .saturating_add(text.len())
                    .saturating_add(1);
                if bytes >= INTERIM_SOURCE_CONTINUATION_UPPER_BOUND {
                    bytes = INTERIM_SOURCE_CONTINUATION_UPPER_BOUND;
                    break;
                }
            }
            if let Some(minimum) = minimum_ends.get(&identity)
                && let Some(page) = minimum_source_page(
                    &source.content,
                    requested,
                    *minimum,
                    &source.content_sha256,
                )
            {
                minimum_pages.insert(identity.clone(), page);
            }
            demands.insert(identity, bytes);
        }
    }
    Ok((demands, minimum_pages))
}

fn source_byte_cap(admission: &PacketAdmissionReceiptV1) -> usize {
    (admission.reserved_source_bytes as usize).clamp(1, INTERIM_SOURCE_CONTINUATION_UPPER_BOUND)
}

pub(super) fn find_admitted_file(
    storage: &Store,
    project_root: &Path,
    path: &str,
) -> Result<Option<FileInfo>, codestory_contracts::api::ApiError> {
    for candidate in admitted_file_lookup_paths(project_root, path) {
        let file = storage.get_file_by_path(&candidate).map_err(|error| {
            codestory_contracts::api::ApiError::internal(format!(
                "Failed to authenticate admitted packet path: {error}"
            ))
        })?;
        if file.is_some() {
            return Ok(file);
        }
    }
    Ok(None)
}

fn admitted_file_lookup_paths(project_root: &Path, path: &str) -> Vec<PathBuf> {
    let candidate = PathBuf::from(path);
    let mut paths = vec![candidate.clone()];
    if !candidate.is_absolute() {
        let joined = project_root.join(candidate);
        if !paths.contains(&joined) {
            paths.push(joined);
        }
    }
    paths
}

fn valid_source_bounds(start_line: Option<u32>, end_line: Option<u32>) -> Option<(u32, u32)> {
    let start_line = start_line.filter(|line| *line > 0)?;
    let end_line = end_line.filter(|line| *line >= start_line)?;
    Some((start_line, end_line))
}

fn source_receipt_line_range(markdown: &str) -> Option<(u32, u32)> {
    let mut start = None;
    let mut end = None;
    for line in markdown.lines() {
        let line = line
            .trim_start()
            .strip_prefix("> ")
            .unwrap_or(line.trim_start());
        let Some((line_number, _)) = line.split_once(" | ") else {
            continue;
        };
        let Ok(line_number) = line_number.trim().parse::<u32>() else {
            continue;
        };
        start = Some(start.map_or(line_number, |current: u32| current.min(line_number)));
        end = Some(end.map_or(line_number, |current: u32| current.max(line_number)));
    }
    start.zip(end)
}

pub(crate) fn observe_admitted_source_coverage(
    controller: &AppController,
    storage: &Store,
    paths: &[String],
) -> Vec<SourceCoverageObservationDto> {
    let Ok(project_root) = controller.require_project_root() else {
        return paths
            .iter()
            .map(|path| source_coverage_not_established(path))
            .collect();
    };
    let mut seen = BTreeSet::new();
    let mut observations = Vec::new();
    for path in paths {
        let canonical = match codestory_workspace::resolve_project_relative_path(
            &project_root,
            Path::new(path),
        ) {
            Ok(
                codestory_workspace::ProjectRelativePathResolution::Existing { relative, .. }
                | codestory_workspace::ProjectRelativePathResolution::Missing { relative, .. },
            ) => Some(relative.to_string_lossy().replace('\\', "/")),
            _ => None,
        };
        let identity = canonical.as_deref().unwrap_or(path);
        if !seen.insert(identity.to_string()) {
            continue;
        }
        observations.push(if canonical.is_some() {
            observe_one_admitted_source_coverage(storage, &project_root, identity)
                .unwrap_or_else(|_| source_coverage_not_established(identity))
        } else {
            source_coverage_not_established(path)
        });
    }
    observations
}

fn observe_one_admitted_source_coverage(
    storage: &Store,
    project_root: &Path,
    path: &str,
) -> Result<SourceCoverageObservationDto, codestory_store::StorageError> {
    // Policy exclusions carry no `file` row (excluded sources are never
    // registered as parser-backed coverage), so the exclusion table is
    // consulted on the normalized relative spelling before the file lookup.
    for candidate in admitted_file_lookup_paths(project_root, path) {
        let relative_path = codestory_workspace::workspace_relative_path(project_root, &candidate)
            .unwrap_or_else(|| candidate.clone())
            .to_string_lossy()
            .replace('\\', "/");
        if storage.has_source_policy_exclusion_path(&relative_path)? {
            return Ok(SourceCoverageObservationDto {
                path: path.to_string(),
                status: SourceCoverageStatusDto::PolicyExcluded,
                reason: None,
                not_established_cause: None,
                observed_size: None,
                byte_cap: None,
            });
        }
    }

    let mut file = None;
    for candidate in admitted_file_lookup_paths(project_root, path) {
        if let Some(found) = storage.get_file_by_path(&candidate)? {
            file = Some(found);
            break;
        }
    }
    let Some(file) = file else {
        return Ok(source_coverage_not_established(path));
    };

    let verified_source = storage.get_file_content_hash(file.id)?.is_some();
    let structural_projection = if file.language == "openapi" {
        storage.has_file_owned_openapi_endpoint_projection(file.id)?
    } else if codestory_indexer::structural::is_structural_candidate_path(&file.path) {
        storage.has_structural_text_projection_for_file(file.id)?
    } else {
        true
    };
    let errors = storage.get_file_coverage_reasons(file.id)?;
    let reason = if !file.complete || !file.indexed || !verified_source || !structural_projection {
        errors
            .first()
            .copied()
            .or_else(|| {
                (file.indexed && verified_source && !file.complete)
                    .then_some(FileCoverageReason::ParserPartial)
            })
            .or(Some(FileCoverageReason::CollectorFailure))
    } else {
        None
    };
    Ok(SourceCoverageObservationDto {
        path: path.to_string(),
        status: if reason.is_some() {
            SourceCoverageStatusDto::Incomplete
        } else {
            SourceCoverageStatusDto::Indexed
        },
        reason,
        not_established_cause: None,
        observed_size: None,
        byte_cap: None,
    })
}

fn source_coverage_not_established(path: &str) -> SourceCoverageObservationDto {
    SourceCoverageObservationDto {
        path: path.to_string(),
        status: SourceCoverageStatusDto::NotEstablished,
        reason: None,
        not_established_cause: Some(SourceCoverageNotEstablishedCauseDto::LookupUnavailable),
        observed_size: None,
        byte_cap: None,
    }
}

fn parser_completeness_for_path(
    path: &str,
    source_coverage: &[SourceCoverageObservationDto],
) -> PacketParserCompletenessV1 {
    source_coverage
        .iter()
        .find(|observation| observation.path == path)
        .map(|observation| match observation.status {
            SourceCoverageStatusDto::Indexed => PacketParserCompletenessV1::Complete,
            SourceCoverageStatusDto::Incomplete => PacketParserCompletenessV1::Partial,
            SourceCoverageStatusDto::PolicyExcluded | SourceCoverageStatusDto::NotEstablished => {
                PacketParserCompletenessV1::Unknown
            }
        })
        .unwrap_or(PacketParserCompletenessV1::Unknown)
}

fn hydrate_induced_relations(
    storage: &Store,
    admissions: &[AuthenticatedPacketAdmissionV1],
) -> Result<Vec<PacketDirectedRelationV1>, codestory_contracts::api::ApiError> {
    let mut stable_identity_by_node = HashMap::new();
    for admission in admissions {
        if let AuthenticatedPacketOriginV1::CoreNode(node_id) = admission.origin {
            stable_identity_by_node
                .entry(node_id)
                .or_insert_with(|| admission.receipt.stable_identity.clone());
        }
    }
    let node_ids = admissions
        .iter()
        .filter_map(|admission| match admission.origin {
            AuthenticatedPacketOriginV1::CoreNode(node_id) => Some(node_id),
            AuthenticatedPacketOriginV1::PublishedFile => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    storage
        .get_certain_edge_representatives_between_node_ids(&node_ids)
        .map_err(|error| {
            codestory_contracts::api::ApiError::internal(format!(
                "Failed to load admitted packet relations: {error}"
            ))
        })
        .map(|edges| {
            edges
                .into_iter()
                .filter_map(|edge| {
                    let (from, to) = edge.effective_endpoints();
                    Some(PacketDirectedRelationV1 {
                        relation_id: edge.id.0.to_string(),
                        from_identity: stable_identity_by_node.get(&from)?.clone(),
                        to_identity: stable_identity_by_node.get(&to)?.clone(),
                        relation_kind: packet_relation_kind(edge.kind),
                        certainty: relation_certainty(edge.certainty),
                    })
                })
                .collect()
        })
}

fn packet_relation_kind(kind: CoreEdgeKind) -> PacketRelationKindV1 {
    use CoreEdgeKind as EdgeKind;
    match kind {
        EdgeKind::MEMBER => PacketRelationKindV1::Member,
        EdgeKind::TYPE_USAGE => PacketRelationKindV1::TypeUsage,
        EdgeKind::USAGE => PacketRelationKindV1::Usage,
        EdgeKind::CALL => PacketRelationKindV1::Call,
        EdgeKind::INHERITANCE => PacketRelationKindV1::Inheritance,
        EdgeKind::OVERRIDE => PacketRelationKindV1::Override,
        EdgeKind::TYPE_ARGUMENT => PacketRelationKindV1::TypeArgument,
        EdgeKind::TEMPLATE_SPECIALIZATION => PacketRelationKindV1::TemplateSpecialization,
        EdgeKind::INCLUDE => PacketRelationKindV1::Include,
        EdgeKind::IMPORT => PacketRelationKindV1::Import,
        EdgeKind::MACRO_USAGE => PacketRelationKindV1::MacroUsage,
        EdgeKind::ANNOTATION_USAGE => PacketRelationKindV1::AnnotationUsage,
        EdgeKind::UNKNOWN => PacketRelationKindV1::Unknown,
    }
}

fn relation_certainty(certainty: Option<ResolutionCertainty>) -> PacketRelationCertaintyV1 {
    match certainty {
        Some(ResolutionCertainty::Certain) => PacketRelationCertaintyV1::Certain,
        Some(ResolutionCertainty::Probable) => PacketRelationCertaintyV1::Probable,
        Some(ResolutionCertainty::Uncertain) => PacketRelationCertaintyV1::Uncertain,
        _ => PacketRelationCertaintyV1::Unknown,
    }
}

fn probe_ambiguities(
    probe_resolutions: &[PacketProbeResolutionDto],
) -> Vec<PacketIdentityAmbiguityV1> {
    probe_resolutions
        .iter()
        .filter(|resolution| resolution.status == PacketProbeResolutionStatusDto::Ambiguous)
        .map(|resolution| PacketIdentityAmbiguityV1 {
            selector: format!("probe:{}", resolution.input_index),
            candidate_identities: resolution
                .candidates
                .iter()
                .map(|candidate| format!("node:{}", candidate.symbol_id))
                .collect(),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn classify_packet_disposition(
    packet: &AgentPacketDto,
    request: Option<&AgentPacketRequestDto>,
    continuation: &[PacketContinuationSelectorV1],
    hydrated_sources: &[PacketHydratedSourceRangeV1],
    project_id: &str,
    core_generation_id: String,
    retrieval_generation: Option<String>,
    published_file_sources: &[PacketHydratedSourceRangeV1],
) -> PacketDispositionDto {
    if let Some(request) = request {
        if let Some(expected) = request.core_generation_id.as_deref()
            && expected != core_generation_id
        {
            return PacketDispositionDto::unavailable("pinned core publication changed");
        }
        if let Some(expected) = request.retrieval_generation.as_deref()
            && Some(expected) != retrieval_generation.as_deref()
        {
            return PacketDispositionDto::unavailable("pinned retrieval publication changed");
        }
    }

    let freshness = PacketFreshnessInput::from_observation(packet.answer.freshness.as_ref());
    if freshness.blocks_packet_availability() {
        return PacketDispositionDto::unavailable(
            freshness
                .gap()
                .unwrap_or_else(|| "publication freshness is not established".to_string()),
        );
    }
    if packet.answer.retrieval_trace.steps.iter().any(|step| {
        matches!(
            step.status,
            codestory_contracts::api::AgentRetrievalStepStatusDto::Error
        )
    }) {
        return PacketDispositionDto::unavailable("retrieval recorded a hard error");
    }

    let already_drilled = request.is_some_and(|request| {
        request.parent_packet_id.is_some()
            || !request.option_ids.is_empty()
            || request.probes.iter().any(|probe| {
                matches!(
                    probe,
                    codestory_contracts::api::PacketProbeDto::Continuation { .. }
                )
            })
    });

    let coverage = packet_coverage_for_disposition(
        &packet.answer.source_coverage,
        &packet.support,
        published_file_sources,
    );
    let coverage_blocks = coverage.blocks_packet_availability();
    if coverage_blocks {
        let blocked_paths = coverage.unprovable_paths();
        let only_parser_partial = packet
            .answer
            .source_coverage
            .iter()
            .filter(|observation| blocked_paths.contains(&observation.path.as_str()))
            .all(|observation| {
                observation.status == SourceCoverageStatusDto::Incomplete
                    && observation.reason == Some(FileCoverageReason::ParserPartial)
            });
        // A used continuation round can retain the exact page it delivered,
        // even when representation pruning restores an unrelated parser gap.
        // Its resolved publication/node/range proof is required; parent IDs or
        // an unrelated retained page cannot establish this terminal evidence.
        if only_parser_partial
            && already_drilled
            && has_retained_requested_source_page(
                packet,
                hydrated_sources,
                project_id,
                &core_generation_id,
                retrieval_generation.as_deref(),
            )
        {
            let mut disposition = PacketDispositionDto::not_established(
                "the bounded continuation left a structural gap",
            );
            disposition.omission_receipts.extend(coverage.gaps());
            return disposition;
        }
        // A parser gap elsewhere remains a refusal, but cannot erase an
        // authenticated retained page's safe next-line action. No omitted
        // source target or other coverage failure enters this exception.
        if already_drilled
            || !only_parser_partial
            || !continuation
                .iter()
                .any(|selector| is_retained_source_continuation(selector, &packet.support))
        {
            return PacketDispositionDto::unavailable(
                coverage
                    .gaps()
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| "source coverage is not established".to_string()),
            );
        }
    }
    if !already_drilled {
        let mut seen_options = BTreeSet::new();
        let options = continuation
            .iter()
            .filter(|selector| {
                !coverage_blocks || is_retained_source_continuation(selector, &packet.support)
            })
            .filter_map(|selector| {
                let mut option = drill_option_from_selector(selector)?;
                option.source_label = continuation_source_label(selector, hydrated_sources);
                Some(option)
            })
            .filter(|option| seen_options.insert(option.id.clone()))
            .take(PACKET_DRILL_MAX_OPTIONS)
            .collect::<Vec<_>>();
        if !options.is_empty() {
            let mut disposition = PacketDispositionDto::drill_once(
                "bounded structural continuation available",
                BoundedDrillPlanDto {
                    parent_packet_id: packet.packet_id.clone(),
                    core_generation_id,
                    retrieval_generation,
                    gap_ids: options.iter().map(|option| option.gap_id.clone()).collect(),
                    options,
                    max_bytes: PACKET_DRILL_MAX_BYTES,
                    max_hits: PACKET_DRILL_MAX_HITS,
                    max_depth: PACKET_DRILL_MAX_DEPTH,
                    remaining_rounds: 1,
                },
            );
            if coverage_blocks {
                // Preserve each refusal in the candidate before its public
                // record and complete representation are measured.
                disposition.omission_receipts.extend(coverage.gaps());
            }
            return disposition;
        }
    }

    if coverage_blocks {
        return PacketDispositionDto::unavailable(
            coverage
                .gaps()
                .into_iter()
                .next()
                .unwrap_or_else(|| "source coverage is not established".to_string()),
        );
    }
    if packet.support.is_empty() {
        PacketDispositionDto::not_established("no bounded repository evidence was retained")
    } else if (already_drilled && !continuation.is_empty())
        || packet.support.iter().any(|unit| {
            unit.source_delivery
                .as_ref()
                .is_some_and(|delivery| delivery.remaining_source_bytes > 0)
        })
    {
        PacketDispositionDto::not_established("the bounded continuation left a structural gap")
    } else {
        // This legacy internal state means only that positive evidence exists.
        // Public v3 never projects it as answer sufficiency.
        PacketDispositionDto::supported()
    }
}

fn has_retained_requested_source_page(
    packet: &AgentPacketDto,
    hydrated_sources: &[PacketHydratedSourceRangeV1],
    project_id: &str,
    core_generation_id: &str,
    retrieval_generation: Option<&str>,
) -> bool {
    packet.plan.probe_resolutions.iter().any(|resolution| {
        let codestory_contracts::api::PacketProbeDto::Continuation {
            contract_version,
            project_id: expected_project,
            core_generation_id: expected_core,
            retrieval_generation: expected_retrieval,
            selector,
        } = &resolution.probe
        else {
            return false;
        };
        let (Some(range), Some(symbol_id)) = (selector.source_range, &resolution.symbol_id) else {
            return false;
        };
        if *contract_version != codestory_contracts::api::PACKET_PROBE_CONTRACT_VERSION
            || expected_project != project_id
            || expected_core != core_generation_id
            || expected_retrieval.as_deref() != retrieval_generation
            || resolution.status != PacketProbeResolutionStatusDto::Continuation
            || resolution.rejection.is_some()
            || selector.reason != PacketStructuralGapReasonV1::SourceBudgetExceeded
            || !range.is_valid()
            || selector.stable_identity != format!("node:{symbol_id}")
            || selector.symbol_id.as_deref() != Some(symbol_id.as_str())
        {
            return false;
        }
        packet.support.iter().any(|unit| {
            let (Some(delivery), Some(start), Some(end)) =
                (&unit.source_delivery, unit.start_line, unit.end_line)
            else {
                return false;
            };
            unit.kind == SupportUnitKindDto::SourceRange
                && unit.symbol_id.as_deref() == Some(symbol_id.as_str())
                && hydrated_sources.iter().any(|source| {
                    source.stable_identity == selector.stable_identity
                        && unit.path.as_deref() == Some(source.path.as_str())
                        && source.start_line == start
                        && source.end_line == end
                        && source.delivery.as_ref() == Some(delivery)
                        && unit.snippet.as_deref() == Some(source.source.as_str())
                        && resolution
                            .path
                            .as_deref()
                            .is_none_or(|path| path == source.path)
                        && selector
                            .path
                            .as_deref()
                            .is_none_or(|path| path == source.path)
                })
                && unit
                    .snippet
                    .as_deref()
                    .is_some_and(|body| !body.trim().is_empty())
                && delivery.requested_range == range
                && delivery.matches_delivery(start, end)
        })
    })
}

fn is_retained_source_continuation(
    selector: &PacketContinuationSelectorV1,
    support: &[SupportUnitDto],
) -> bool {
    if selector.reason != PacketStructuralGapReasonV1::SourceBudgetExceeded {
        return false;
    }
    let Some(range) = selector.source_range else {
        return false;
    };
    support.iter().any(|unit| {
        if unit.kind != SupportUnitKindDto::SourceRange
            || final_support_stable_identity(unit).as_deref()
                != Some(selector.stable_identity.as_str())
            || selector
                .path
                .as_deref()
                .is_some_and(|path| unit.path.as_deref() != Some(path))
            || selector
                .symbol_id
                .as_deref()
                .is_some_and(|id| unit.symbol_id.as_deref() != Some(id))
            || unit.snippet.as_deref().is_none_or(str::is_empty)
        {
            return false;
        }
        let (Some(delivery), Some(start), Some(end)) =
            (&unit.source_delivery, unit.start_line, unit.end_line)
        else {
            return false;
        };
        delivery.matches_delivery(start, end)
            && delivery.next_start_line == Some(range.start_line)
            && delivery.requested_range.end_line == range.end_line
    })
}

fn packet_coverage_for_disposition(
    observations: &[codestory_contracts::api::SourceCoverageObservationDto],
    support: &[SupportUnitDto],
    published_file_sources: &[PacketHydratedSourceRangeV1],
) -> PacketCoverageInput {
    let verified_source_paths = support
        .iter()
        .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
        .filter(|unit| {
            unit.snippet
                .as_deref()
                .is_some_and(|snippet| !snippet.is_empty())
        })
        .filter_map(|unit| unit.path.as_deref())
        .collect::<BTreeSet<_>>();
    let blocking = observations
        .iter()
        .filter(|observation| {
            // This live same-pin proof also survives output omission. It proves
            // the missing Core row is deliberate, not that the row was served.
            if observation.status == SourceCoverageStatusDto::NotEstablished
                && observation.not_established_cause
                    == Some(SourceCoverageNotEstablishedCauseDto::LookupUnavailable)
                && published_file_sources
                    .iter()
                    .any(|source| source.path == observation.path)
            {
                return false;
            }
            observation.status != SourceCoverageStatusDto::Incomplete
                || observation.reason != Some(FileCoverageReason::ParserPartial)
                || !verified_source_paths.contains(observation.path.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    PacketCoverageInput::from_observations(&blocking)
}

/// Labels borrow only already hydrated, publication-bound source metadata.
/// Executable selector fields and identities remain untouched.
fn continuation_source_label(
    selector: &PacketContinuationSelectorV1,
    sources: &[PacketHydratedSourceRangeV1],
) -> Option<codestory_contracts::api::DrillSourceLabelDto> {
    let mut matching = sources.iter().filter(|source| {
        source.stable_identity == selector.stable_identity
            && source.start_line > 0
            && source.end_line >= source.start_line
            && !source.source.trim().is_empty()
            && source.delivery.as_ref().is_none_or(|delivery| {
                delivery.matches_delivery(source.start_line, source.end_line)
            })
            && selector
                .path
                .as_ref()
                .is_none_or(|path| path == &source.path)
    });
    let first = matching.next()?;
    let nonempty = |value: &str| (!value.trim().is_empty()).then(|| value.to_owned());
    let mut label = codestory_contracts::api::DrillSourceLabelDto {
        path: nonempty(&first.path),
        symbol: first.symbol.as_deref().and_then(nonempty),
    };
    for source in matching {
        if label.path != nonempty(&source.path) {
            label.path = None;
        }
        if label.symbol != source.symbol.as_deref().and_then(nonempty) {
            label.symbol = None;
        }
    }
    (label.path.is_some() || label.symbol.is_some()).then_some(label)
}

fn drill_option_from_selector(selector: &PacketContinuationSelectorV1) -> Option<DrillOptionDto> {
    let mut gap_id = format!(
        "{}:{}",
        structural_reason_label(selector.reason),
        selector.stable_identity
    );
    if let Some(range) = selector.source_range {
        gap_id.push_str(&format!(":lines:{}:{}", range.start_line, range.end_line));
    }
    let mut option = if let Some(path) = selector
        .path
        .as_deref()
        .or_else(|| selector.stable_identity.strip_prefix("path:"))
    {
        DrillOptionDto::bounded_source_read(gap_id, path)
    } else {
        let symbol_id = selector
            .symbol_id
            .as_deref()
            .or_else(|| selector.stable_identity.strip_prefix("node:"))?;
        DrillOptionDto::omitted_symbol(gap_id, symbol_id)
    };
    option.structural_reason = Some(selector.reason);
    if let Some(range) = selector.source_range {
        if !range.is_valid() {
            return None;
        }
        option = option.with_source_range(range);
    }
    if option.id.len() > codestory_contracts::packet_projection_v3::IDENTITY_MAX_BYTES_V3 {
        return None;
    }
    Some(option)
}

fn structural_reason_label(reason: PacketStructuralGapReasonV1) -> &'static str {
    match reason {
        PacketStructuralGapReasonV1::CandidateCountExceeded => "candidate_count_exceeded",
        PacketStructuralGapReasonV1::SourceBudgetExceeded => "source_budget_exceeded",
        PacketStructuralGapReasonV1::SourceUnavailable => "source_unavailable",
        PacketStructuralGapReasonV1::AmbiguousSelector => "ambiguous_selector",
        PacketStructuralGapReasonV1::DisconnectedSeed => "disconnected_seed",
    }
}

pub(crate) fn continuation_probes_from_options(
    req: &AgentPacketRequestDto,
    project_id: &str,
) -> Result<Vec<codestory_contracts::api::PacketProbeDto>, codestory_contracts::api::ApiError> {
    use codestory_contracts::api::{ApiError, PacketProbeDto};
    let decoded_options = drill_options_from_ids(&req.option_ids);
    if decoded_options.len() != req.option_ids.len() {
        return Err(ApiError::invalid_argument(
            "packet contains a malformed continuation option ID",
        ));
    }
    let mut probes = Vec::new();
    for option in decoded_options {
        if let Some(range) = option.source_range {
            let symbol_id = option.symbol_id.ok_or_else(|| {
                ApiError::invalid_argument("source range continuation requires a node identity")
            })?;
            let core_generation_id = req.core_generation_id.clone().ok_or_else(|| {
                ApiError::invalid_argument(
                    "source range continuation requires a pinned core generation",
                )
            })?;
            probes.push(PacketProbeDto::Continuation {
                contract_version: codestory_contracts::api::PACKET_PROBE_CONTRACT_VERSION,
                project_id: project_id.to_owned(),
                core_generation_id,
                retrieval_generation: req.retrieval_generation.clone(),
                selector: codestory_contracts::compilation::PacketContinuationSelectorV1 {
                    stable_identity: format!("node:{symbol_id}"),
                    path: None,
                    symbol_id: Some(symbol_id),
                    reason: codestory_contracts::compilation::PacketStructuralGapReasonV1::SourceBudgetExceeded,
                    source_range: Some(range),
                },
            });
        } else if let Some(path) = option.path {
            probes.push(PacketProbeDto::ExactPath { path });
        } else if let Some(symbol_id) = option.symbol_id {
            probes.push(PacketProbeDto::SymbolId { id: symbol_id });
        }
    }
    Ok(probes)
}

/// Decode only stable path or symbol continuations. Historical query-text
/// options are deliberately not reintroduced as retrieval policy.
pub fn drill_options_from_ids(option_ids: &[String]) -> Vec<DrillOptionDto> {
    option_ids
        .iter()
        .filter_map(|id| {
            let (kind, target) = decode_drill_option_id(id)?;
            let canonical_id = codestory_contracts::api::encode_drill_option_id(kind, &target);
            let (target, range) = if kind == DrillGapKindDto::OmittedMandatorySupport {
                if let Some(encoded) = target.strip_prefix("range:") {
                    let mut parts = encoded.splitn(3, ':');
                    let start = parts.next()?;
                    let end = parts.next()?;
                    let range = PacketSourceRangeV1 {
                        start_line: start.parse().ok()?,
                        end_line: end.parse().ok()?,
                    };
                    if !range.is_valid()
                        || range.start_line.to_string() != start
                        || range.end_line.to_string() != end
                    {
                        return None;
                    }
                    (parts.next()?.to_owned(), Some(range))
                } else {
                    (target, None)
                }
            } else {
                (target, None)
            };
            let option = match kind {
                DrillGapKindDto::BoundedSourceRead => Some(DrillOptionDto::bounded_source_read(
                    format!("source_unavailable:{target}"),
                    target,
                )),
                DrillGapKindDto::OmittedMandatorySupport => {
                    if let Some(path) = target.strip_prefix("path:") {
                        Some(DrillOptionDto::omitted_source_path(
                            format!("disconnected_seed:{path}"),
                            path,
                        ))
                    } else {
                        let symbol = target.strip_prefix("symbol:")?;
                        Some(DrillOptionDto::omitted_symbol(
                            format!("disconnected_seed:{symbol}"),
                            symbol,
                        ))
                    }
                }
            }?;
            let option = if let Some(range) = range {
                option.with_source_range(range)
            } else {
                option
            };
            // Reject noncanonical escapes and malformed range IDs rather than
            // quietly turning them into a different executable action.
            if option.id != canonical_id {
                return None;
            }
            // Preserve the caller's exact legacy or compact reference for
            // audit/fingerprints; decoding does not grant source authority.
            let mut option = option;
            option.id = id.clone();
            Some(option)
        })
        .take(PACKET_DRILL_MAX_OPTIONS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{Edge as CoreEdge, EdgeId as CoreEdgeId};
    use codestory_store::FileRole;

    // Opt-in control against a retained publication. No retrieval opener is called:
    // absence of its real pin must refuse the actual returned assertion selector.
    #[test]
    #[ignore = "requires the retained continuation fixture and existing complete Core"]
    fn returned_assertion_continuation_refuses_without_retained_retrieval_pin() {
        use codestory_contracts::api::PacketProbeRejectionCodeDto;
        let fixture_path = std::env::var("CODESTORY_CPU_LABEL_REPRESENTATION").unwrap();
        let fixture: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture_path).unwrap()).unwrap();
        assert_eq!(
            fixture["schema"],
            "CodeStory-returned-continuation-refusal/v428"
        );
        let request: AgentPacketRequestDto =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        assert_eq!(request.option_ids.len(), 1);
        assert_eq!(request.option_ids[0], fixture["returned_option_ids"][0]);
        assert!(request.retrieval_generation.is_some());
        let project_id = fixture["project_id"].as_str().unwrap();
        let project_root = Path::new(fixture["project_root"].as_str().unwrap());
        assert_eq!(
            codestory_workspace::project_identity_v3(project_root).project_id,
            project_id
        );
        let cache = tempfile::tempdir().unwrap();
        let controller = AppController::new_with_owned_cache_root(cache.path());
        {
            let mut state = controller.state.lock();
            state.project_root = Some(project_root.to_owned());
            state.storage_path = Some(PathBuf::from(
                fixture["logical_storage_path"].as_str().unwrap(),
            ));
        }
        controller.with_complete_core_snapshot(|core| {
            assert_eq!(Some(core.generation_id.as_str()), request.core_generation_id.as_deref());
            assert!(crate::agent::retrieval_primary::active_pinned_retrieval_publication(&controller).is_none());
            for (case, requested_project, expected_message) in [
                ("missing-retrieval", project_id, "continuation retrieval generation is no longer selected"),
                ("changed-core", project_id, "continuation core generation is no longer selected"),
                ("changed-project", "different-project", "continuation belongs to a different project"),
            ] {
                let mut control = request.clone();
                if case == "changed-core" { control.core_generation_id = Some("different-core".into()); }
                let probes = continuation_probes_from_options(&control, requested_project).unwrap();
                let resolutions = crate::agent::packet_probe::resolve_packet_probes(&controller, probes);
                assert_eq!(resolutions.len(), 1);
                let resolution = &resolutions[0];
                assert_eq!(resolution.status, PacketProbeResolutionStatusDto::Rejected, "{case}");
                let rejection = resolution.rejection.as_ref().unwrap();
                assert_eq!(rejection.code, PacketProbeRejectionCodeDto::StaleContinuation, "{case}");
                assert_eq!(rejection.message, expected_message, "{case}");
                assert!(resolution.symbol_id.is_none() && resolution.path.is_none());
                let session = PacketProofSession::new();
                let refusal = match freeze_packet_compilation(&controller, project_id, &resolutions, None, &session, Some(&control)) {
                    Err(refusal) => refusal,
                    Ok(_) => panic!("{case}: unpinned continuation reached frozen source"),
                };
                assert_eq!(refusal.code, "invalid_argument");
                assert_eq!(refusal.message, "pinned continuation publication changed before source hydration");
                assert!(session.receipts().is_empty());
                println!("retained-continuation-control {case}: resolver={}; freeze={}; source/render unreached", rejection.message, refusal.message);
            }
            Ok(())
        }).unwrap();
    }

    #[test]
    fn continuation_source_labels_preserve_identity_execution_and_pins() {
        let selector = PacketContinuationSelectorV1 {
            stable_identity: "node:7".into(),
            path: None,
            symbol_id: Some("7".into()),
            reason: PacketStructuralGapReasonV1::SourceBudgetExceeded,
            source_range: Some(PacketSourceRangeV1 {
                start_line: 12,
                end_line: 20,
            }),
        };
        let source = PacketHydratedSourceRangeV1 {
            callable_body: None,
            stable_identity: "node:7".into(),
            path: "src/worker.rs".into(),
            symbol: Some("Worker.run".into()),
            start_line: 10,
            end_line: 20,
            source: "fn run() {}".into(),
            delivery: None,
            parser_completeness: PacketParserCompletenessV1::Complete,
        };
        let packet = source_page_test_packet();
        let classify = |sources: &[PacketHydratedSourceRangeV1], request| {
            classify_packet_disposition(
                &packet,
                request,
                std::slice::from_ref(&selector),
                sources,
                "project",
                "core".into(),
                Some("retrieval".into()),
                &[],
            )
        };
        let baseline = classify(&[], None).drill.unwrap();
        let labeled = classify(std::slice::from_ref(&source), None).drill.unwrap();
        let option = &labeled.options[0];
        assert_eq!(
            option.source_label.as_ref().unwrap().path.as_deref(),
            Some("src/worker.rs")
        );
        assert_eq!(
            option.source_label.as_ref().unwrap().symbol.as_deref(),
            Some("Worker.run")
        );
        assert!(
            option.path.is_none(),
            "display path cannot change symbol routing"
        );
        let mut without_label = labeled.clone();
        without_label.options[0].source_label = None;
        assert_eq!(
            serde_json::to_value(&without_label).unwrap(),
            serde_json::to_value(&baseline).unwrap()
        );
        let request: AgentPacketRequestDto = serde_json::from_value(serde_json::json!({
            "question":"inspect", "option_ids":[option.id],
            "core_generation_id":"core", "retrieval_generation":"retrieval"
        }))
        .unwrap();
        let probes = continuation_probes_from_options(&request, "project").unwrap();
        let codestory_contracts::api::PacketProbeDto::Continuation {
            selector: executed, ..
        } = &probes[0]
        else {
            panic!("same typed continuation");
        };
        assert_eq!(executed.stable_identity, selector.stable_identity);
        assert_eq!(executed.source_range, selector.source_range);
        assert!(executed.path.is_none());
        let mut stale = request;
        stale.core_generation_id = Some("different".into());
        assert_eq!(
            classify(&[source], Some(&stale)).kind,
            codestory_contracts::api::PacketDispositionKindDto::Unavailable
        );
    }

    #[test]
    fn continuation_source_labels_keep_missing_conflicting_and_invalid_metadata_unknown() {
        let selector = PacketContinuationSelectorV1 {
            stable_identity: "node:7".into(),
            path: None,
            symbol_id: Some("7".into()),
            reason: PacketStructuralGapReasonV1::DisconnectedSeed,
            source_range: None,
        };
        let mut source = PacketHydratedSourceRangeV1 {
            callable_body: None,
            stable_identity: "node:7".into(),
            path: "src/a.rs".into(),
            symbol: None,
            start_line: 1,
            end_line: 2,
            source: "fn run() {}".into(),
            delivery: None,
            parser_completeness: PacketParserCompletenessV1::Complete,
        };
        assert!(continuation_source_label(&selector, &[]).is_none());
        let label = continuation_source_label(&selector, &[source.clone()]).unwrap();
        assert!(label.symbol.is_none());
        assert_eq!(
            serde_json::to_value(&label).unwrap()["symbol"],
            serde_json::Value::Null
        );
        let mut other = source.clone();
        other.path = "src/b.rs".into();
        other.symbol = Some("other".into());
        assert!(continuation_source_label(&selector, &[source.clone(), other.clone()]).is_none());
        other.path = source.path.clone();
        let label = continuation_source_label(&selector, &[source.clone(), other]).unwrap();
        assert_eq!(label.path.as_deref(), Some("src/a.rs"));
        assert!(label.symbol.is_none());
        source.stable_identity = "node:8".into();
        assert!(continuation_source_label(&selector, &[source.clone()]).is_none());
        source.stable_identity = "node:7".into();
        source.source.clear();
        assert!(continuation_source_label(&selector, &[source.clone()]).is_none());
        source.source = "fn run() {}".into();
        source.start_line = 0;
        assert!(continuation_source_label(&selector, &[source.clone()]).is_none());
        source.start_line = 1;
        source.delivery = Some(PacketSourceDeliveryV1 {
            requested_range: PacketSourceRangeV1 {
                start_line: 1,
                end_line: 2,
            },
            source_sha256: "a".repeat(64),
            remaining_source_bytes: 0,
            next_start_line: Some(2),
            blocked_line_rendered_bytes: None,
        });
        assert!(continuation_source_label(&selector, &[source]).is_none());
    }

    #[test]
    fn requested_source_reservation_survives_sixteen_admissions() {
        let session = PacketProofSession::new();
        session.admit_exact_selector("node:0", 512, 0);
        for id in 1..16 {
            session.admit(&format!("node:{id}"), 512);
        }
        session.seal_retrieval_admission();
        let requested_identities = std::collections::HashSet::from(["node:0".to_owned()]);
        session.reserve_source_pages(&requested_identities);
        let before = session.receipts();
        let requested = PacketSourceRangeV1 {
            start_line: 1,
            end_line: 41,
        };
        let text = "    process(value); // complete line\n".repeat(41);
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        let (_, end, delivery) =
            complete_node_page(&text, requested, source_byte_cap(&before[0]), &hash).unwrap();
        assert_eq!(
            end, requested.end_line,
            "unrelated admissions must not starve an authenticated requested extent that fits its row cap"
        );
        assert_eq!(delivery.remaining_source_bytes, 0);
        session.reserve_source_pages(&requested_identities);
        assert_eq!(
            session.receipts(),
            before,
            "reservations are never refunded or replenished"
        );
        assert_eq!(before.len(), 16);
        assert!(
            before
                .iter()
                .map(|r| r.reserved_source_bytes as usize)
                .sum::<usize>()
                <= 16384
        );
        assert!(before.iter().all(|r| r.reserved_source_bytes <= 4096));
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn measured_row_mix_replays_source_first_delivery_with_honest_relation_gaps() {
        use crate::evidence_projection_v3::representation_budget_test_support as fixture;
        use codestory_contracts::packet_projection_v3::PacketProjectionV3Dto;
        for (requested, per_row_frame) in [(false, 0), (true, 0), (false, 3500)] {
            let texts = (0..3)
                .map(|id| format!("fn operation_{id}() {{\n    process(value);\n}}\n"))
                .collect::<Vec<_>>();
            let input = PacketCompilationInputV1 {
                contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
                publication: PacketCompilationPublicationV1 {
                    project_id: "project-1".into(),
                    core_generation_id: "core-generation-1".into(),
                    retrieval_generation: Some("retrieval-generation-1".into()),
                },
                admissions: (0..3)
                    .map(|id| PacketAdmissionReceiptV1 {
                        packet_ordinal: id,
                        stable_identity: format!("node:{id}"),
                        score_version: "fixture".into(),
                        reserved_source_bytes: 512,
                        origin: PacketAdmissionOriginV1::Retrieval,
                    })
                    .collect(),
                sources: texts
                    .iter()
                    .enumerate()
                    .map(|(id, body)| {
                        let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
                        let (source, end, delivery) = complete_node_page(
                            body,
                            PacketSourceRangeV1 {
                                start_line: 1,
                                end_line: 3,
                            },
                            512,
                            &hash,
                        )
                        .unwrap();
                        PacketHydratedSourceRangeV1 {
                            callable_body: None,
                            stable_identity: format!("node:{id}"),
                            path: format!("source_{id}.rs"),
                            symbol: Some(format!("operation_{id}")),
                            start_line: 1,
                            end_line: end,
                            source,
                            delivery: Some(delivery),
                            parser_completeness: PacketParserCompletenessV1::Complete,
                        }
                    })
                    .collect(),
                relations: vec![PacketDirectedRelationV1 {
                    relation_id: "edge-0".into(),
                    from_identity: "node:0".into(),
                    to_identity: "node:2".into(),
                    relation_kind: codestory_contracts::compilation::PacketRelationKindV1::Call,
                    certainty: PacketRelationCertaintyV1::Certain,
                }],
                ambiguities: Vec::new(),
                admission_gaps: Vec::new(),
            };
            let request: AgentPacketRequestDto =
                serde_json::from_value(serde_json::json!({"question":"Read repository evidence"}))
                    .unwrap();
            let (base, _) = fixture::default_packet(input.clone(), &request);
            let mut prepared = PreparedPacketCompilationV1::from_fixture_input(input);
            if requested {
                prepared.requested_source_identities.insert("node:0".into());
            }
            let mut accepted = None;
            let mut proposals = 0;
            let frozen = prepared
                .select_for_representation(|prepared, product| {
                    proposals += 1;
                    let mut candidate = base.clone();
                    prepared.apply_proposal(&mut candidate, Some(&request), product);
                    crate::agent::packet_budget::enforce_packet_output_budget(
                        Path::new("/fixture"),
                        &mut candidate,
                    );
                    assert_eq!(candidate.support, product.support);
                    // Exercise real JSON/mirroring with a constrained caller's opaque
                    // per-row frame. This is not the installed stdio URI/meta proof.
                    let wire_size = |projection: &PacketProjectionV3Dto| {
                        let root = serde_json::to_value(projection).unwrap();
                        let rows = match projection {
                            PacketProjectionV3Dto::Complete { evidence, .. } => {
                                evidence.as_slice().len()
                            }
                            _ => 0,
                        };
                        serde_json::to_vec(&serde_json::json!({
                        "content":[{"type":"text", "text":serde_json::to_string(&root).unwrap()}],
                        "structuredContent":root, "isError":false,
                        "_meta":{"opaque_caller_frame":"x".repeat(if per_row_frame == 0 {
                            // A caller viewport decorates its fourth row with a
                            // large frame. Lower-row candidates remain real JSON.
                            usize::from(rows > 3) * 16384
                        } else { rows * per_row_frame })}
                    })).unwrap().len()
                    };
                    let projected = fixture::project_packet(&request, &candidate, |projection| {
                        Ok(wire_size(projection))
                    })?;
                    let PacketProjectionV3Dto::Complete { .. } = projected.projection else {
                        return Ok(None);
                    };
                    let bytes = wire_size(&projected.projection);
                    if bytes <= 16384 {
                        accepted = Some((candidate, bytes));
                    }
                    Ok(Some(bytes))
                })
                .unwrap();
            let (packet, bytes) = accepted.unwrap();
            let source_rows = packet
                .support
                .iter()
                .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
                .count();
            if per_row_frame == 0 {
                assert_eq!(source_rows, 3);
            } else {
                assert!(
                    source_rows > 0 && source_rows < 3,
                    "expensive omission/frame metadata must not force an oversized alternative"
                );
            }

            if per_row_frame == 0 {
                assert!(
                    packet
                        .support
                        .iter()
                        .all(|unit| unit.kind != SupportUnitKindDto::TypedGraphEdge)
                );
            }
            assert!(
                packet
                    .support
                    .iter()
                    .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
                    .all(|unit| {
                        unit.source_delivery
                            .as_ref()
                            .unwrap()
                            .remaining_source_bytes
                            == 0
                    })
            );
            assert!(bytes <= 16384 && proposals <= 35);
            if per_row_frame == 0 {
                for endpoint in ["node:0", "node:2"] {
                    assert!(
                        frozen
                            .product
                            .continuation
                            .iter()
                            .any(|selector| selector.stable_identity == endpoint
                                && selector.reason
                                    == PacketStructuralGapReasonV1::DisconnectedSeed)
                    );
                }
            } else {
                assert!(
                    frozen
                        .product
                        .continuation
                        .iter()
                        .any(|selector| selector.source_range
                            == Some(PacketSourceRangeV1 {
                                start_line: 1,
                                end_line: 3
                            })),
                    "omitted whole pages retain their original bounds"
                );
            }
            println!(
                "row-mix fixture requested={requested} per_row_frame={per_row_frame} complete_sources={source_rows} wire_bytes={bytes} proposals={proposals}"
            );
            commit_measured_packet_compilation(&packet, frozen).unwrap();
        }
    }

    #[test]
    #[ignore = "requires retained source-coverage fixture paths; CPU only"]
    fn retained_current_context_survives_delay_and_shutdown_selection_and_commit() {
        use crate::evidence_projection_v3::representation_budget_test_support as fixture;
        use codestory_contracts::packet_projection_v3::PacketProjectionV3Dto;
        let config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var("CODESTORY_CPU_LABEL_REPRESENTATION").unwrap()).unwrap(),
        )
        .unwrap();
        let initial: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config["initial"].as_str().unwrap()).unwrap())
                .unwrap();
        let root = Path::new(config["source"].as_str().unwrap());
        let captured_meta = initial["_meta"].clone();
        let continued: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config["continued"].as_str().unwrap()).unwrap())
                .unwrap();
        for (selected, first, last, keep_delay) in [
            ("7339483592651562902", 101, 153, false),
            ("6355496689727442353", 558, 672, false),
            ("6355496689727442353", 558, 672, true),
        ] {
            let mut contexts = Vec::new();
            for row in initial["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["kind"] == "exact_source")
                .chain(
                    continued["evidence"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|row| keep_delay && row["symbol_id"] == "7339483592651562902"),
                )
            {
                let identity = format!("node:{}", row["symbol_id"].as_str().unwrap());
                let path = row["path"].as_str().unwrap();
                let content = std::fs::read_to_string(root.join(path)).unwrap();
                let hash = format!("{:x}", Sha256::digest(content.as_bytes()));
                assert_eq!(row["source_delivery"]["source_sha256"], hash);
                let delivery: PacketSourceDeliveryV1 =
                    serde_json::from_value(row["source_delivery"].clone()).unwrap();
                let (source, end, receipt) = complete_node_page(
                    &content,
                    delivery.requested_range,
                    row["summary"].as_str().unwrap().len(),
                    &hash,
                )
                .unwrap();
                assert_eq!(end, row["end_line"].as_u64().unwrap() as u32);
                assert_eq!(receipt, delivery);
                contexts.push(PacketHydratedSourceRangeV1 {
                    callable_body: None,
                    stable_identity: identity,
                    path: path.into(),
                    symbol: None,
                    start_line: row["start_line"].as_u64().unwrap() as u32,
                    end_line: end,
                    source,
                    parser_completeness: PacketParserCompletenessV1::Complete,
                    delivery: Some(receipt),
                });
            }
            assert_eq!(contexts.len(), if keep_delay { 13 } else { 12 });
            let path = if first == 101 {
                "chain_test.go"
            } else {
                "cron_test.go"
            };
            let content = std::fs::read_to_string(root.join(path)).unwrap();
            let hash = format!("{:x}", Sha256::digest(content.as_bytes()));
            let range = PacketSourceRangeV1 {
                start_line: first,
                end_line: last,
            };
            let make = |cap| {
                let (source, end, delivery) =
                    complete_node_page(&content, range, cap, &hash).unwrap();
                PacketHydratedSourceRangeV1 {
                    callable_body: None,
                    stable_identity: format!("node:{selected}"),
                    path: path.into(),
                    symbol: None,
                    start_line: first,
                    end_line: end,
                    source,
                    parser_completeness: PacketParserCompletenessV1::Complete,
                    delivery: Some(delivery),
                }
            };
            let mut sources = vec![make(4096)];
            sources.extend(contexts.clone());
            let mut alternatives = BTreeMap::new();
            for cap in [2048, 1024, 512] {
                let mut pages = vec![make(cap)];
                for context in &contexts {
                    if context.source.len() <= cap {
                        continue;
                    }
                    let content = std::fs::read_to_string(root.join(&context.path)).unwrap();
                    let (source, end, delivery) = complete_node_page(
                        &content,
                        context.delivery.as_ref().unwrap().requested_range,
                        cap,
                        &context.delivery.as_ref().unwrap().source_sha256,
                    )
                    .unwrap();
                    let mut page = context.clone();
                    page.source = source;
                    page.end_line = end;
                    page.delivery = Some(delivery);
                    pages.push(page);
                }
                alternatives.insert(cap, pages);
            }
            let input = PacketCompilationInputV1 {
                contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
                publication: PacketCompilationPublicationV1 {
                    project_id: "project-1".into(),
                    core_generation_id: "core-generation-1".into(),
                    retrieval_generation: Some("retrieval-generation-1".into()),
                },
                admissions: sources
                    .iter()
                    .enumerate()
                    .map(|(index, source)| PacketAdmissionReceiptV1 {
                        packet_ordinal: index as u32,
                        stable_identity: source.stable_identity.clone(),
                        score_version: "retained-fixture".into(),
                        reserved_source_bytes: if index == 0 { 4096 } else { 1024 },
                        origin: if index == 0 {
                            PacketAdmissionOriginV1::ExactTypedSelector
                        } else {
                            PacketAdmissionOriginV1::Retrieval
                        },
                    })
                    .collect(),
                sources,
                relations: Vec::new(),
                ambiguities: Vec::new(),
                admission_gaps: Vec::new(),
            };
            let request: AgentPacketRequestDto = serde_json::from_value(serde_json::json!({
                "question":"Inspect a received repository source range", "parent_packet_id":"parent",
                "core_generation_id":"core-generation-1", "retrieval_generation":"retrieval-generation-1"
            })).unwrap();
            let (base, _) = fixture::default_packet(input.clone(), &request);
            let mut prepared = PreparedPacketCompilationV1::from_fixture_input(input);
            prepared.source_page_alternatives = alternatives;
            prepared
                .requested_source_identities
                .insert(format!("node:{selected}"));
            let mut accepted = None;
            let mut calls = 0;
            let frozen = prepared
                .select_for_representation_with_baseline(|prepared, product, _baseline| {
                    calls += 1;
                    let mut candidate = base.clone();
                    prepared.apply_proposal(&mut candidate, Some(&request), product);
                    crate::agent::packet_budget::enforce_packet_output_budget(
                        Path::new("/fixture"),
                        &mut candidate,
                    );
                    assert_eq!(candidate.support, product.support);
                    let wire = |projection: &PacketProjectionV3Dto| {
                        let mut value = serde_json::to_value(projection).unwrap();
                        value["_meta"] = captured_meta.clone();
                        serde_json::to_vec(&value).unwrap().len() + 1
                    };
                    let projected = fixture::project_packet(&request, &candidate, |projection| {
                        Ok(wire(projection))
                    })?;
                    let PacketProjectionV3Dto::Complete { .. } = projected.projection else {
                        return Ok(None);
                    };
                    let bytes = wire(&projected.projection);
                    if bytes <= 16384 {
                        accepted = Some((candidate, bytes));
                    }
                    Ok(Some(bytes))
                })
                .unwrap();
            let (packet, bytes) = accepted.unwrap();
            for context in &contexts {
                let row = packet
                    .support
                    .iter()
                    .find(|row| {
                        row.kind == SupportUnitKindDto::SourceRange
                            && final_support_stable_identity(row).as_deref()
                                == Some(&context.stable_identity)
                    })
                    .unwrap_or_else(|| {
                        panic!("retained context erased: {}", context.stable_identity)
                    });
                assert_eq!(row.start_line, Some(context.start_line));
                assert!(
                    row.end_line.unwrap() >= context.end_line,
                    "retained context shortened: {}",
                    context.stable_identity
                );
                let prefix = context
                    .source
                    .strip_suffix("\u{0060}\u{0060}\u{0060}")
                    .unwrap();
                assert!(row.snippet.as_deref().unwrap().starts_with(prefix));
            }
            let requested = packet.support.iter().find(|row| {
                row.kind == SupportUnitKindDto::SourceRange
                    && final_support_stable_identity(row).as_deref()
                        == Some(&format!("node:{selected}"))
            });
            if let Some(requested) = requested {
                if first == 101 {
                    assert_eq!(requested.end_line, Some(153));
                    assert_eq!(
                        requested
                            .source_delivery
                            .as_ref()
                            .unwrap()
                            .remaining_source_bytes,
                        0
                    );
                } else {
                    let delivered = requested.end_line.unwrap();
                    assert!(delivered < 665, "shutdown must page to retain old context");
                    let receipt = requested.source_delivery.as_ref().unwrap();
                    assert_eq!(receipt.next_start_line, Some(delivered + 1));
                    assert!(receipt.remaining_source_bytes > 129);
                    assert!(receipt.matches_delivery(first, delivered));
                }
            } else {
                assert!(keep_delay, "a fitting requested page must be delivered");
                assert!(
                    !frozen.source_refusals.is_empty(),
                    "no-fit progress is explicitly refused"
                );
            }
            assert!(bytes <= 16384 && calls <= 8 * 51 + 1);
            commit_measured_packet_compilation(&packet, frozen).unwrap();
            println!(
                "retained selection selected={selected} contexts=12 requested_end={:?} bytes={bytes} calls={calls}",
                requested.and_then(|row| row.end_line)
            );
        }
    }

    #[test]
    fn issued_prefix_is_floor_but_not_requested_line_progress() {
        let body = "ordinary_call(argument);\n".repeat(100);
        let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
        let range = PacketSourceRangeV1 {
            start_line: 1,
            end_line: 100,
        };
        let make = |cap| {
            let (source, end_line, delivery) =
                complete_node_page(&body, range, cap, &hash).unwrap();
            PacketHydratedSourceRangeV1 {
                callable_body: None,
                stable_identity: "node:7".into(),
                path: "ordinary.rs".into(),
                symbol: None,
                start_line: 1,
                end_line,
                source,
                delivery: Some(delivery),
                parser_completeness: PacketParserCompletenessV1::Complete,
            }
        };
        let old = make(1024);
        let full = make(4096);
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project-1".into(),
                core_generation_id: "core-1".into(),
                retrieval_generation: None,
            },
            admissions: vec![PacketAdmissionReceiptV1 {
                packet_ordinal: 0,
                stable_identity: "node:7".into(),
                score_version: "ordinary".into(),
                reserved_source_bytes: 4096,
                origin: PacketAdmissionOriginV1::ExactTypedSelector,
            }],
            sources: vec![full],
            relations: vec![],
            ambiguities: vec![],
            admission_gaps: vec![],
        };
        let mut prepared = PreparedPacketCompilationV1::from_fixture_input(input);
        prepared.requested_source_identities.insert("node:7".into());
        prepared
            .requested_source_minimum_ends
            .insert("node:7".into(), old.end_line + 1);
        prepared.issued_context_sources = Some(vec![old.clone()]);
        prepared
            .source_page_alternatives
            .insert(1024, vec![old.clone()]);
        prepared
            .source_page_alternatives
            .insert(512, vec![make(512)]);
        let mut advancing = prepared.clone();
        let minimum = minimum_source_page(&body, range, old.end_line + 1, &hash).unwrap();
        let page = revalidated_minimum_page(&advancing.input.sources[0], &minimum, 4096).unwrap();
        for pages in advancing.source_page_alternatives.values_mut() {
            pages[0] = page.clone();
        }
        let mut floor_measured = false;
        let frozen = prepared
            .select_for_representation(|_, product| {
                let row = product
                    .support
                    .iter()
                    .find(|r| r.kind == SupportUnitKindDto::SourceRange);
                if let Some(row) = row
                    && row.end_line == Some(old.end_line)
                {
                    floor_measured = true;
                    return Ok(Some(1024));
                }
                Ok(None)
            })
            .unwrap();
        assert!(
            floor_measured,
            "old floor must remain independently measurable"
        );
        assert!(
            !frozen.source_refusals.is_empty(),
            "an unchanged prefix must explicitly refuse requested progress"
        );
        let retained = frozen
            .product
            .support
            .iter()
            .find(|r| r.kind == SupportUnitKindDto::SourceRange)
            .unwrap();
        assert_eq!(retained.end_line, Some(old.end_line));
        assert_eq!(retained.snippet.as_deref(), Some(old.source.as_str()));
        assert_eq!(retained.source_delivery, old.delivery);
        let advanced = advancing
            .select_for_representation(|_, product| {
                let row = product
                    .support
                    .iter()
                    .find(|r| r.kind == SupportUnitKindDto::SourceRange);
                Ok(row
                    .filter(|r| r.end_line.is_some_and(|end| end <= old.end_line + 1))
                    .map(|_| 1100))
            })
            .unwrap();
        assert!(advanced.source_refusals.is_empty());
        let row = advanced
            .product
            .support
            .iter()
            .find(|r| r.kind == SupportUnitKindDto::SourceRange)
            .unwrap();
        assert_eq!(row.end_line, Some(old.end_line + 1));
        assert!(
            row.snippet
                .as_deref()
                .unwrap()
                .starts_with(old.source.strip_suffix("```").unwrap())
        );
        let delivery = row.source_delivery.as_ref().unwrap();
        assert_eq!(delivery.requested_range, range);
        assert_eq!(delivery.next_start_line, Some(old.end_line + 2));
        assert!(delivery.remaining_source_bytes > 0);
    }

    #[test]
    fn minimum_page_requires_fresh_coverage_hash_range_and_actual_reservation() {
        let body = "ordinary_call(argument);\n".repeat(100);
        let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
        let range = PacketSourceRangeV1 {
            start_line: 1,
            end_line: 100,
        };
        let minimum = minimum_source_page(&body, range, 30, &hash).unwrap();
        let (source, end_line, delivery) = complete_node_page(&body, range, 4096, &hash).unwrap();
        let fresh = PacketHydratedSourceRangeV1 {
            callable_body: None,
            stable_identity: "node:7".into(),
            path: "ordinary.rs".into(),
            symbol: None,
            start_line: 1,
            end_line,
            source,
            delivery: Some(delivery),
            parser_completeness: PacketParserCompletenessV1::Complete,
        };
        let page = revalidated_minimum_page(&fresh, &minimum, minimum.source.len()).unwrap();
        assert_eq!(page.end_line, 30);
        assert_eq!(
            page.delivery.as_ref().unwrap().remaining_source_bytes,
            (body.len() * 70 / 100) as u64
        );
        assert_eq!(page.delivery.as_ref().unwrap().requested_range, range);
        assert!(revalidated_minimum_page(&fresh, &minimum, minimum.source.len() - 1).is_none());
        let mut changed = fresh.clone();
        changed.delivery.as_mut().unwrap().source_sha256 = "different".into();
        assert!(revalidated_minimum_page(&changed, &minimum, 4096).is_none());
        changed = fresh.clone();
        changed.delivery.as_mut().unwrap().requested_range.end_line -= 1;
        assert!(revalidated_minimum_page(&changed, &minimum, 4096).is_none());
        changed = fresh.clone();
        changed.end_line = 29;
        assert!(revalidated_minimum_page(&changed, &minimum, 4096).is_none());
        changed = fresh.clone();
        changed.source = changed.source.replacen("ordinary_call", "modified_call", 1);
        assert!(revalidated_minimum_page(&changed, &minimum, 4096).is_none());
        assert!(minimum_source_page(&body, range, 101, &hash).is_none());
        let huge = "x".repeat(4096) + "\n";
        assert!(
            minimum_source_page(
                &huge,
                PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: 1
                },
                1,
                &hash
            )
            .is_none()
        );
    }

    #[test]
    fn issued_source_floor_allows_short_new_page_without_shortening_old_body() {
        use crate::evidence_projection_v3::representation_budget_test_support as fixture;
        use codestory_contracts::packet_projection_v3::PacketProjectionV3Dto;
        let make = |id: i64, content: &str, cap: usize| {
            let hash = format!("{:x}", Sha256::digest(content.as_bytes()));
            let (source, end_line, delivery) = complete_node_page(
                content,
                PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: content.lines().count() as u32,
                },
                cap,
                &hash,
            )
            .unwrap();
            PacketHydratedSourceRangeV1 {
                callable_body: None,
                stable_identity: format!("node:{id}"),
                path: format!("source_{id}.rs"),
                symbol: None,
                start_line: 1,
                end_line,
                source,
                delivery: Some(delivery),
                parser_completeness: PacketParserCompletenessV1::Complete,
            }
        };
        let old_body = "    original_operation(argument, retained_context);\n".repeat(46);
        let new_body = "    requested_operation(argument, additional_context);\n".repeat(62);
        let old = make(7, &old_body, 4096);
        let new = make(9, &new_body, 4096);
        assert!(old.source.len() > 2048);
        assert_eq!(old.delivery.as_ref().unwrap().remaining_source_bytes, 0);
        assert_eq!(new.delivery.as_ref().unwrap().remaining_source_bytes, 0);
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project-1".into(),
                core_generation_id: "core-generation-1".into(),
                retrieval_generation: Some("retrieval-generation-1".into()),
            },
            admissions: vec![
                PacketAdmissionReceiptV1 {
                    packet_ordinal: 0,
                    stable_identity: "node:9".into(),
                    score_version: "ordinary".into(),
                    reserved_source_bytes: 4096,
                    origin: PacketAdmissionOriginV1::ExactTypedSelector,
                },
                PacketAdmissionReceiptV1 {
                    packet_ordinal: 1,
                    stable_identity: "node:7".into(),
                    score_version: "ordinary".into(),
                    reserved_source_bytes: 4096,
                    origin: PacketAdmissionOriginV1::Retrieval,
                },
            ],
            sources: vec![new.clone(), old.clone()],
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        let request: AgentPacketRequestDto = serde_json::from_value(serde_json::json!({
            "question":"Read another repository source page", "parent_packet_id":"parent",
            "core_generation_id":"core-generation-1", "retrieval_generation":"retrieval-generation-1"
        })).unwrap();
        let (base, _) = fixture::default_packet(input.clone(), &request);
        let short = make(9, &new_body, 512);
        // A real complete public frame with an opaque caller field is known
        // to fit the old body plus a short requested page. No query semantics
        // or candidate-order assumptions enter this capacity control.
        let mut known_input = input.clone();
        known_input.sources = vec![short.clone(), old.clone()];
        let known = PreparedPacketCompilationV1::from_fixture_input(known_input);
        let mut known_packet = base.clone();
        known.apply_proposal(&mut known_packet, Some(&request), &known.default_product());
        let known_projection = fixture::project_packet(&request, &known_packet, |p| {
            Ok(serde_json::to_vec(p).unwrap().len() + 1)
        })
        .unwrap();
        assert!(matches!(
            known_projection.projection,
            PacketProjectionV3Dto::Complete { .. }
        ));
        let known_bytes = serde_json::to_vec(&known_projection.projection)
            .unwrap()
            .len()
            + 1;
        let opaque_bytes = PUBLIC_PACKET_SERIALIZED_MAX_BYTES - known_bytes - 64;
        let wire = |p: &PacketProjectionV3Dto| {
            serde_json::to_vec(&serde_json::json!({
                "packet":p, "opaque_caller_frame":"x".repeat(opaque_bytes)
            }))
            .unwrap()
            .len()
                + 1
        };
        assert!(wire(&known_projection.projection) <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES);
        let mut prepared = PreparedPacketCompilationV1::from_fixture_input(input);
        prepared.requested_source_identities.insert("node:9".into());
        prepared.issued_context_sources = Some(vec![old.clone()]);
        for cap in [2048, 1024, 512] {
            prepared
                .source_page_alternatives
                .insert(cap, vec![make(9, &new_body, cap), make(7, &old_body, cap)]);
        }
        let mut measured = 0;
        let mut accepted = None;
        let frozen = prepared
            .select_for_representation(|prepared, product| {
                measured += 1;
                let mut candidate = base.clone();
                prepared.apply_proposal(&mut candidate, Some(&request), product);
                let projected = fixture::project_packet(&request, &candidate, |p| Ok(wire(p)))?;
                if !matches!(projected.projection, PacketProjectionV3Dto::Complete { .. }) {
                    return Ok(None);
                }
                let bytes = wire(&projected.projection);
                if bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES {
                    accepted = Some(candidate);
                }
                Ok(Some(bytes))
            })
            .unwrap();
        let packet = accepted.unwrap();
        let old_row = packet
            .support
            .iter()
            .find(|r| r.symbol_id.as_deref() == Some("7"))
            .unwrap();
        assert_eq!(old_row.snippet.as_deref(), Some(old.source.as_str()));
        assert_eq!(old_row.source_delivery, old.delivery);
        assert_eq!(old_row.start_line, Some(1));
        assert_eq!(old_row.end_line, Some(old.end_line));
        let requested = packet
            .support
            .iter()
            .find(|r| {
                r.kind == SupportUnitKindDto::SourceRange && r.symbol_id.as_deref() == Some("9")
            })
            .expect("the known-fitting mixed extent must deliver requested source progress");
        assert!(requested.end_line.unwrap() < new.end_line);
        assert!(
            requested
                .source_delivery
                .as_ref()
                .unwrap()
                .next_start_line
                .is_some()
        );
        assert!(frozen.source_refusals.is_empty());
        assert!(measured <= 8 * codestory_agent::evidence_compiler::REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1 + 1);
        commit_measured_packet_compilation(&packet, frozen).unwrap();
    }

    #[test]
    fn issued_source_record_floor_protects_all_five_original_bodies_and_allows_optional_context() {
        let sources = (0..16)
            .map(|id| {
                let body = format!(
                    "fn ordinary_{id}() {{
    operation(value);
}}
"
                );
                let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
                let (source, end_line, delivery) = complete_node_page(
                    &body,
                    PacketSourceRangeV1 {
                        start_line: 1,
                        end_line: 3,
                    },
                    512,
                    &hash,
                )
                .unwrap();
                PacketHydratedSourceRangeV1 {
                    callable_body: None,
                    stable_identity: format!("node:{id}"),
                    path: format!("source_{id}.rs"),
                    symbol: Some(format!("ordinary_{id}")),
                    start_line: 1,
                    end_line,
                    source,
                    delivery: Some(delivery),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                }
            })
            .collect::<Vec<_>>();
        let originals = sources[..5].to_vec();
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project".into(),
                core_generation_id: "core".into(),
                retrieval_generation: Some("retrieval".into()),
            },
            admissions: sources
                .iter()
                .enumerate()
                .map(|(id, source)| PacketAdmissionReceiptV1 {
                    packet_ordinal: id as u32,
                    stable_identity: source.stable_identity.clone(),
                    score_version: "ordinary".into(),
                    reserved_source_bytes: 512,
                    origin: if id == 5 {
                        PacketAdmissionOriginV1::ExactTypedSelector
                    } else {
                        PacketAdmissionOriginV1::Retrieval
                    },
                })
                .collect(),
            sources,
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        let measured = |_: &PreparedPacketCompilationV1, p: &RepositoryDerivedCompilationV1| {
            Ok(Some(
                p.support
                    .iter()
                    .filter(|r| r.kind == SupportUnitKindDto::SourceRange)
                    .count()
                    * 2100,
            ))
        };
        let mut issued = PreparedPacketCompilationV1::from_fixture_input(input);
        issued.requested_source_identities.insert("node:5".into());
        issued.issued_context_sources = Some(originals.clone());
        let frozen = issued.select_for_representation(measured).unwrap();
        assert_eq!(
            frozen
                .context_floor
                .iter()
                .filter(|r| r.kind == SupportUnitKindDto::SourceRange)
                .count(),
            5
        );
        for old in &originals {
            assert!(
                frozen.product.support.iter().any(|r| r.symbol_id.as_deref()
                    == old.stable_identity.strip_prefix("node:")
                    && r.snippet.as_deref() == Some(old.source.as_str())
                    && r.source_delivery == old.delivery
                    && r.start_line == Some(old.start_line)
                    && r.end_line == Some(old.end_line)),
                "previous body and receipt retained byte-for-byte"
            );
        }
        assert!(
            frozen
                .product
                .support
                .iter()
                .any(|r| r.kind == SupportUnitKindDto::SourceRange
                    && r.symbol_id.as_deref() == Some("5")),
            "requested new page makes progress"
        );
        assert!(
            frozen
                .product
                .support
                .iter()
                .filter(|r| r.kind == SupportUnitKindDto::SourceRange)
                .count()
                <= 7
        );
    }

    #[test]
    fn exhausted_requested_page_preserves_the_measured_capacity_refusal() {
        use crate::evidence_projection_v3::representation_budget_test_support as fixture;
        use codestory_contracts::packet_projection_v3::PacketProjectionV3Dto;
        let sources = (0..2)
            .map(|id| {
                let body = format!("fn operation_{id}() {{\n    process(value);\n}}\n");
                let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
                let (source, end_line, delivery) = complete_node_page(
                    &body,
                    PacketSourceRangeV1 {
                        start_line: 1,
                        end_line: 3,
                    },
                    512,
                    &hash,
                )
                .unwrap();
                PacketHydratedSourceRangeV1 {
                    callable_body: None,
                    stable_identity: format!("node:{id}"),
                    path: format!("source_{id}.rs"),
                    symbol: Some(format!("operation_{id}")),
                    start_line: 1,
                    end_line,
                    source,
                    delivery: Some(delivery),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                }
            })
            .collect::<Vec<_>>();
        let old = sources[0].clone();
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project-1".into(),
                core_generation_id: "core-generation-1".into(),
                retrieval_generation: Some("retrieval-generation-1".into()),
            },
            admissions: sources
                .iter()
                .enumerate()
                .map(|(id, source)| PacketAdmissionReceiptV1 {
                    packet_ordinal: id as u32,
                    stable_identity: source.stable_identity.clone(),
                    score_version: "capacity-control".into(),
                    reserved_source_bytes: 512,
                    origin: if id == 1 {
                        PacketAdmissionOriginV1::ExactTypedSelector
                    } else {
                        PacketAdmissionOriginV1::Retrieval
                    },
                })
                .collect(),
            sources,
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        let request: AgentPacketRequestDto =
            serde_json::from_value(serde_json::json!({"question":"Read repository evidence"}))
                .unwrap();
        let (base, _) = fixture::default_packet(input.clone(), &request);
        let mut prepared = PreparedPacketCompilationV1::from_fixture_input(input);
        prepared.requested_source_identities.insert("node:1".into());
        prepared
            .requested_source_minimum_ends
            .insert("node:1".into(), 1);
        prepared.issued_context_sources = Some(vec![old.clone()]);
        let mut floor_fit = false;
        let mut progress_refused = false;
        let mut final_refusal = None;
        let error = prepared.select_for_representation_with_baseline(|prepared, product, baseline| {
            let mut packet = base.clone();
            prepared.apply_proposal(&mut packet, Some(&request), product);
            let progress = product.support.iter().any(|row| row.symbol_id.as_deref() == Some("1")
                && row.kind == SupportUnitKindDto::SourceRange);
            let terminal = !prepared.source_refusals.is_empty();
            let unbounded = fixture::project_packet(&request, &packet, |_| Ok(0))?.projection;
            let PacketProjectionV3Dto::Complete {
                schema_version, identity, publication, retrieval, diagnostics, ..
            } = unbounded else { panic!("unbounded fixture must retain complete evidence") };
            let fallback = crate::evidence_projection_v3::packet_budget_exceeded_projection_v3_from_envelope(
                schema_version, identity, publication, retrieval, diagnostics,
                PUBLIC_PACKET_SERIALIZED_MAX_BYTES + 1,
            );
            let padding = if progress || terminal {
                PUBLIC_PACKET_SERIALIZED_MAX_BYTES - serde_json::to_vec(&fallback).unwrap().len() - 1
            } else { 0 };
            let projected = fixture::project_packet(&request, &packet, |projection| {
                Ok(serde_json::to_vec(projection).unwrap().len() + padding)
            })?;
            match projected.projection {
                PacketProjectionV3Dto::Complete { .. } => {
                    assert!(!progress && !terminal, "only the initial protected floor fits");
                    floor_fit = true;
                    Ok(Some(serde_json::to_vec(&projected.projection).unwrap().len()))
                }
                PacketProjectionV3Dto::BudgetExceeded {
                    maximum_bytes, required_complete_bytes, ..
                } => {
                    assert!(required_complete_bytes > maximum_bytes);
                    assert!(serde_json::to_vec(&projected.projection).unwrap().len() + padding
                        <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES);
                    if terminal {
                        assert!(baseline.is_some());
                        assert_eq!(product.support, baseline.unwrap().support);
                        assert!(product.support.iter().any(|row| row.snippet.as_deref() == Some(old.source.as_str())
                            && row.source_delivery == old.delivery));
                        final_refusal = Some(projected.projection);
                    } else { progress_refused = true; }
                    Ok(None)
                }
            }
        }).err().expect("requested page must exhaust measured capacity");
        assert!(
            floor_fit && progress_refused && final_refusal.is_some(),
            "exercise all three causal stages"
        );
        assert_eq!(error.code, REQUESTED_PAGE_CAPACITY_REFUSAL);
    }

    #[test]
    fn current_context_capacity_diagnostic_distinguishes_floor_and_real_projection_refusal() {
        use crate::evidence_projection_v3::representation_budget_test_support as fixture;
        use codestory_contracts::packet_projection_v3::PacketProjectionV3Dto;
        let sources = (0..2)
            .map(|id| {
                let body = format!("fn operation_{id}() {{\n    process(value);\n}}\n");
                let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
                let (source, end_line, delivery) = complete_node_page(
                    &body,
                    PacketSourceRangeV1 {
                        start_line: 1,
                        end_line: 3,
                    },
                    512,
                    &hash,
                )
                .unwrap();
                PacketHydratedSourceRangeV1 {
                    callable_body: None,
                    stable_identity: format!("node:{id}"),
                    path: format!("source_{id}.rs"),
                    symbol: Some(format!("operation_{id}")),
                    start_line: 1,
                    end_line,
                    source,
                    delivery: Some(delivery),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                }
            })
            .collect::<Vec<_>>();
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project-1".into(),
                core_generation_id: "core-generation-1".into(),
                retrieval_generation: Some("retrieval-generation-1".into()),
            },
            admissions: sources
                .iter()
                .enumerate()
                .map(|(id, source)| PacketAdmissionReceiptV1 {
                    packet_ordinal: id as u32,
                    stable_identity: source.stable_identity.clone(),
                    score_version: "capacity-control".into(),
                    reserved_source_bytes: 512,
                    origin: if id == 0 {
                        PacketAdmissionOriginV1::ExactTypedSelector
                    } else {
                        PacketAdmissionOriginV1::Retrieval
                    },
                })
                .collect(),
            sources,
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        let request: AgentPacketRequestDto =
            serde_json::from_value(serde_json::json!({"question":"Read repository evidence"}))
                .unwrap();
        for mode in 0..4 {
            let mut current = input.clone();
            if mode == 0 {
                current.sources[1].source.push_str("invalid fence suffix");
            }
            let (base, _) = fixture::default_packet(current.clone(), &request);
            let mut prepared = PreparedPacketCompilationV1::from_fixture_input(current.clone());
            prepared.requested_source_identities.insert("node:0".into());
            let padding = if mode == 2 {
                let mut context_input = current.clone();
                context_input
                    .sources
                    .retain(|source| source.stable_identity == "node:1");
                let context = fixture::default_packet(context_input, &request).0;
                let full = fixture::project_packet(&request, &context, |_| Ok(0))
                    .unwrap()
                    .projection;
                let PacketProjectionV3Dto::Complete {
                    schema_version,
                    identity,
                    publication,
                    retrieval,
                    diagnostics,
                    ..
                } = full
                else {
                    panic!("unbounded generic context must be complete");
                };
                let fallback = crate::evidence_projection_v3::packet_budget_exceeded_projection_v3_from_envelope(
                    schema_version, identity, publication, retrieval, diagnostics,
                    PUBLIC_PACKET_SERIALIZED_MAX_BYTES + 1,
                );
                let fallback_bytes = serde_json::to_vec(&fallback).unwrap().len();
                PUBLIC_PACKET_SERIALIZED_MAX_BYTES
                    .checked_sub(fallback_bytes + 32)
                    .unwrap()
            } else {
                0
            };
            let mut measurements = 0;
            let mut complete_sizes = Vec::new();
            let result =
                prepared.select_for_representation_with_baseline(|prepared, product, _| {
                    measurements += 1;
                    if mode == 1 {
                        return Ok(Some(PUBLIC_PACKET_SERIALIZED_MAX_BYTES + 23));
                    }
                    let mut candidate = base.clone();
                    prepared.apply_proposal(&mut candidate, Some(&request), product);
                    let first_probe = complete_sizes.len();
                    let projection = fixture::project_packet(&request, &candidate, |projection| {
                        let bytes = serde_json::to_vec(projection).unwrap().len() + padding;
                        if matches!(projection, PacketProjectionV3Dto::Complete { .. }) {
                            complete_sizes.push(bytes);
                        }
                        Ok(bytes)
                    })?
                    .projection;
                    match projection {
                        PacketProjectionV3Dto::Complete { .. } => Ok(Some(
                            serde_json::to_vec(&projection).unwrap().len() + padding,
                        )),
                        PacketProjectionV3Dto::BudgetExceeded {
                            maximum_bytes,
                            required_complete_bytes,
                            ..
                        } => {
                            assert_eq!(maximum_bytes as usize, PUBLIC_PACKET_SERIALIZED_MAX_BYTES);
                            assert!(
                                required_complete_bytes as usize
                                    > PUBLIC_PACKET_SERIALIZED_MAX_BYTES
                            );
                            assert_eq!(
                                required_complete_bytes as usize,
                                complete_sizes[first_probe]
                            );
                            assert!(
                                serde_json::to_vec(&projection).unwrap().len() + padding
                                    <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES
                            );
                            Ok(None)
                        }
                    }
                });
            if mode == 3 {
                let frozen = result.unwrap();
                let expected = fixture::default_packet(input.clone(), &request).0.support;
                assert!(source_floor_retained(&frozen.product.support, &expected));
            } else {
                let error = result.err().expect("unfitting context must fail closed");
                let rendered = serde_json::to_string(&error).unwrap();
                assert!(
                    rendered.contains("Authenticated current context cannot fit"),
                    "mode={mode}: {rendered}"
                );
                if mode == 0 {
                    assert_eq!(measurements, 0);
                    assert!(rendered.contains("floor_preserving_measurements=0"));
                } else {
                    assert!(measurements > 0);
                    assert!(!rendered.contains("floor_preserving_measurements=0;"));
                    if mode == 1 {
                        assert!(rendered.contains("minimum_measured_bytes=Some(16407)"));
                        assert!(rendered.contains("measure_none=0;"));
                    } else {
                        assert!(!complete_sizes.is_empty());
                        assert!(
                            complete_sizes
                                .iter()
                                .all(|bytes| *bytes > PUBLIC_PACKET_SERIALIZED_MAX_BYTES)
                        );
                        assert!(rendered.contains("minimum_measured_bytes=None"));
                        assert!(!rendered.contains("measure_none=0;"));
                    }
                }
                println!(
                    "capacity control mode={mode} measurements={measurements} complete_min={:?}",
                    complete_sizes.iter().min()
                );
            }
        }
    }

    #[test]
    fn context_floor_rejects_last_line_hash_path_alias_and_legacy_rewrites() {
        let text = "fn operation() {\n    keep();\n}\n";
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        let range = PacketSourceRangeV1 {
            start_line: 1,
            end_line: 3,
        };
        let (snippet, end, receipt) = complete_node_page(text, range, 4096, &hash).unwrap();
        let old: SupportUnitDto = serde_json::from_value(serde_json::json!({
            "id":"source:1", "kind":"source_range", "summary":"original",
            "path":"source.rs", "symbol_id":"1", "start_line":1,"end_line":end,
            "snippet":snippet,"source_delivery":receipt
        }))
        .unwrap();
        assert!(source_floor_retained(
            std::slice::from_ref(&old),
            std::slice::from_ref(&old)
        ));
        for mutation in 0..5 {
            let mut changed = old.clone();
            match mutation {
                0 => {
                    changed.snippet =
                        Some(changed.snippet.unwrap().replace("}\n```", "} altered\n```"))
                }
                1 => changed.source_delivery.as_mut().unwrap().source_sha256 = "0".repeat(64),
                2 => changed.path = Some("another.rs".into()),
                3 => changed.symbol_id = Some("alias-covering-same-file".into()),
                _ => changed.end_line = Some(2),
            }
            assert!(
                !source_floor_retained(std::slice::from_ref(&changed), std::slice::from_ref(&old)),
                "mutation {mutation}"
            );
        }
        let mut legacy = old.clone();
        legacy.symbol_id = None;
        legacy.source_delivery = None;
        assert!(source_floor_retained(
            std::slice::from_ref(&legacy),
            std::slice::from_ref(&legacy)
        ));
        let mut changed = legacy.clone();
        changed.snippet = Some("rewritten".into());
        assert!(!source_floor_retained(
            std::slice::from_ref(&changed),
            std::slice::from_ref(&legacy)
        ));
        let input: PacketCompilationInputV1 = serde_json::from_value(serde_json::json!({
            "contract_version":PACKET_COMPILATION_CONTRACT_VERSION_V1,
            "publication":{"project_id":"project-1","core_generation_id":"core-generation-1","retrieval_generation":null},
            "admissions":[],"sources":[],"relations":[],"ambiguities":[],"admission_gaps":[]
        })).unwrap();
        let prepared = PreparedPacketCompilationV1::from_fixture_input(input);
        let mut frozen = prepared.clone().into_frozen(prepared.default_product());
        frozen.context_floor = vec![legacy];
        assert!(commit_measured_packet_compilation(&source_page_test_packet(), frozen).is_err());
    }

    #[test]
    fn requested_source_selection_preserves_context_or_refuses_progress() {
        use crate::evidence_projection_v3::representation_budget_test_support as fixture;
        use codestory_contracts::packet_projection_v3::PacketProjectionV3Dto;

        fn mirrored_size(projection: &PacketProjectionV3Dto) -> Result<usize, ()> {
            let root = serde_json::to_value(projection).map_err(|_| ())?;
            let text = serde_json::to_string(&root).map_err(|_| ())?;
            serde_json::to_vec(&serde_json::json!({
                "content": [{"type": "text", "text": text}],
                "structuredContent": root,
                "isError": false
            }))
            .map(|bytes| bytes.len())
            .map_err(|_| ())
        }

        for (context_count, requested_lines, context_lines) in [(2, 40, 100), (4, 150, 5)] {
            let texts = std::iter::once(
                "    selected_operation(value); // retained source line\n".repeat(requested_lines),
            )
            .chain((0..context_count).map(|_| {
                "    context_operation(value); // unrelated source line\n".repeat(context_lines)
            }))
            .collect::<Vec<_>>();
            let mut sources = Vec::new();
            let mut alternatives = BTreeMap::<usize, Vec<PacketHydratedSourceRangeV1>>::new();
            for (id, text) in texts.iter().enumerate() {
                let range = PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: text.lines().count() as u32,
                };
                let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
                let make = |cap| {
                    let (body, end, delivery) =
                        complete_node_page(text, range, cap, &hash).unwrap();
                    PacketHydratedSourceRangeV1 {
                        callable_body: None,
                        stable_identity: format!("node:{id}"),
                        path: format!("source_{id}.rs"),
                        symbol: Some(format!("operation_{id}")),
                        start_line: 1,
                        end_line: end,
                        source: body,
                        parser_completeness: PacketParserCompletenessV1::Complete,
                        delivery: Some(delivery),
                    }
                };
                sources.push(make(4096));
                for cap in [2048, 1024, 512] {
                    alternatives.entry(cap).or_default().push(make(cap));
                }
            }
            let input = PacketCompilationInputV1 {
                contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
                publication: PacketCompilationPublicationV1 {
                    project_id: "project-1".into(),
                    core_generation_id: "core-generation-1".into(),
                    retrieval_generation: Some("retrieval-generation-1".into()),
                },
                admissions: (0..texts.len())
                    .map(|id| PacketAdmissionReceiptV1 {
                        packet_ordinal: id as u32,
                        stable_identity: format!("node:{id}"),
                        score_version: "fixture".into(),
                        reserved_source_bytes: if id == 0 || context_lines > 5 {
                            4096
                        } else {
                            512
                        },
                        origin: if id == 0 {
                            PacketAdmissionOriginV1::ExactTypedSelector
                        } else {
                            PacketAdmissionOriginV1::Retrieval
                        },
                    })
                    .collect(),
                sources,
                relations: Vec::new(),
                ambiguities: Vec::new(),
                admission_gaps: Vec::new(),
            };
            let request: AgentPacketRequestDto = serde_json::from_value(serde_json::json!({"question":"Read selected repository source", "parent_packet_id":"parent", "core_generation_id":"core-generation-1", "retrieval_generation":"retrieval-generation-1"})).unwrap();
            let (base, _) = fixture::default_packet(input.clone(), &request);
            let mut prepared = PreparedPacketCompilationV1::from_fixture_input(input);
            prepared.source_page_alternatives = alternatives;
            prepared.requested_source_identities.insert("node:0".into());
            assert!(prepared.issued_context_sources.is_none());
            let expected_end = prepared.input.sources[0].end_line;
            let project = |prepared: &PreparedPacketCompilationV1,
                           product: &RepositoryDerivedCompilationV1|
             -> Result<_, codestory_contracts::api::ApiError> {
                let mut candidate = base.clone();
                prepared.apply_proposal(&mut candidate, Some(&request), product);
                crate::agent::packet_budget::enforce_packet_output_budget(
                    Path::new("/fixture"),
                    &mut candidate,
                );
                assert_eq!(
                    candidate.support, product.support,
                    "the internal presentation gate must not change compiled source delivery"
                );
                let projected = fixture::project_packet(&request, &candidate, mirrored_size)?;
                Ok((candidate, projected))
            };
            // Establish the independent current baseline with the same actual
            // renderer before asking runtime to protect it beside progress.
            let mut context = prepared.clone();
            context
                .input
                .sources
                .retain(|source| source.stable_identity != "node:0");
            let default_context = context.default_product();
            let default_projection = project(&context, &default_context).unwrap().1.projection;
            let selected_context = compile_repository_evidence_baseline_for_representation(
                &context.input,
                &context.call_navigation,
                |product| -> Result<Option<usize>, codestory_contracts::api::ApiError> {
                    let projection = project(&context, product)?.1.projection;
                    if matches!(&projection, PacketProjectionV3Dto::Complete { .. }) {
                        Ok(Some(mirrored_size(&projection).unwrap()))
                    } else {
                        Ok(None)
                    }
                },
            )
            .unwrap();
            let RepresentationBudgetSelectionV1::Complete {
                product: selected_context,
                representation_bytes,
                ..
            } = selected_context
            else {
                panic!("an independently measured current context must fit this renderer")
            };
            assert!(representation_bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES);
            assert!(
                selected_context
                    .support
                    .iter()
                    .any(|row| row.kind == SupportUnitKindDto::SourceRange)
            );
            if context_lines == 100 {
                assert!(
                    matches!(
                        default_projection,
                        PacketProjectionV3Dto::BudgetExceeded { .. }
                    ),
                    "the hostile default must really exceed the mirrored public envelope"
                );
                assert_ne!(selected_context.support, default_context.support);
            } else {
                assert!(matches!(
                    default_projection,
                    PacketProjectionV3Dto::Complete { .. }
                ));
                assert_eq!(
                    selected_context, default_context,
                    "an already fitting default context must remain unchanged"
                );
            }
            // Independently exhibit requested progress beside the exact floor
            // using only already hydrated, authenticated page alternatives.
            let mut combined = prepared.clone();
            for source in &mut combined.input.sources {
                let protected = selected_context.support.iter().any(|row| {
                    row.kind == SupportUnitKindDto::SourceRange
                        && final_support_stable_identity(row).as_deref()
                            == Some(source.stable_identity.as_str())
                        && row.path.as_deref() == Some(source.path.as_str())
                        && row.start_line == Some(source.start_line)
                        && row.end_line == Some(source.end_line)
                        && row.snippet.as_deref() == Some(source.source.as_str())
                        && row.source_delivery == source.delivery
                });
                if !protected {
                    *source = prepared.source_page_alternatives[&512]
                        .iter()
                        .find(|page| page.stable_identity == source.stable_identity)
                        .expect("existing smallest authenticated page")
                        .clone();
                }
            }
            let combined_selection = compile_repository_evidence_baseline_for_representation(
                &combined.input,
                &combined.call_navigation,
                |product| -> Result<Option<usize>, codestory_contracts::api::ApiError> {
                    if !source_floor_retained(&product.support, &selected_context.support)
                        || !product.support.iter().any(|row| {
                            row.kind == SupportUnitKindDto::SourceRange
                                && row.symbol_id.as_deref() == Some("0")
                                && row.source_delivery.is_some()
                        })
                    {
                        return Ok(None);
                    }
                    let projection = project(&combined, product)?.1.projection;
                    let bytes = mirrored_size(&projection).unwrap();
                    println!(
                        "combined-page diagnostic context_lines={context_lines} baseline_bytes={representation_bytes} requested_end={:?} requested_cursor={:?} projected_bytes={bytes} projection={:?}",
                        product.support.iter().find(|row| row.symbol_id.as_deref() == Some("0")).and_then(|row| row.end_line),
                        product.support.iter().find(|row| row.symbol_id.as_deref() == Some("0")).and_then(|row| row.source_delivery.as_ref()).and_then(|delivery| delivery.next_start_line),
                        match &projection {
                            PacketProjectionV3Dto::Complete { .. } => "complete".to_string(),
                            PacketProjectionV3Dto::BudgetExceeded { required_complete_bytes, .. } => format!("budget_exceeded:required={required_complete_bytes}"),
                        }
                    );
                    if matches!(projection, PacketProjectionV3Dto::Complete { .. }) {
                        Ok(Some(bytes))
                    } else {
                        Ok(None)
                    }
                },
            )
            .unwrap();
            assert!(
                matches!(combined_selection, RepresentationBudgetSelectionV1::Complete { representation_bytes, .. }
                    if representation_bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES),
                "an independently authenticated combined candidate must fit before requiring runtime progress"
            );
            let mut accepted = None;
            let mut choices = Vec::new();
            let mut measurements = 0;
            let mut fitting_context_depths = Vec::new();
            let frozen = prepared.select_for_representation(|prepared, product| {
                measurements += 1;
                let (candidate, projected) = project(prepared, product)?;
                let PacketProjectionV3Dto::Complete { .. } = projected.projection else {
                    return Ok(None);
                };
                let bytes = mirrored_size(&projected.projection).unwrap();
                choices.push((
                    candidate
                        .support
                        .iter()
                        .find(|row| row.symbol_id.as_deref() == Some("0"))
                        .and_then(|row| row.end_line),
                    candidate
                        .support
                        .iter()
                        .filter(|row| row.source_delivery.is_some())
                        .count(),
                    serde_json::to_vec(&candidate).unwrap().len(),
                    bytes,
                ));
                if bytes <= 16384 {
                    let contexts = candidate
                        .support
                        .iter()
                        .filter(|row| {
                            row.source_delivery.is_some() && row.symbol_id.as_deref() != Some("0")
                        })
                        .collect::<Vec<_>>();
                    assert!(contexts.iter().all(|row| {
                        row.start_line == Some(1)
                            && row.source_delivery.as_ref().is_some_and(|delivery| {
                                delivery.requested_range.end_line == context_lines as u32
                                    && delivery.matches_delivery(1, row.end_line.unwrap())
                            })
                    }));
                    if candidate.support.iter().any(|row| {
                        row.symbol_id.as_deref() == Some("0") && row.end_line == Some(expected_end)
                    }) {
                        fitting_context_depths.push((
                            contexts.len(),
                            contexts
                                .iter()
                                .filter_map(|row| row.end_line)
                                .max()
                                .unwrap_or(0),
                        ));
                    }
                    accepted = Some((candidate, bytes));
                }
                Ok(Some(bytes))
            });
            let frozen = frozen.expect(
                "protect the fitting renderer-selected current baseline beside requested progress",
            );
            let (packet, bytes) = accepted.unwrap();
            assert_eq!(frozen.context_floor, selected_context.support);
            for old in selected_context
                .support
                .iter()
                .filter(|row| row.kind == SupportUnitKindDto::SourceRange)
            {
                let retained = packet
                    .support
                    .iter()
                    .find(|row| {
                        row.kind == SupportUnitKindDto::SourceRange
                            && row.symbol_id == old.symbol_id
                    })
                    .expect("the selected context source remains present");
                assert_eq!(
                    (
                        &retained.path,
                        retained.start_line,
                        retained.end_line,
                        &retained.snippet,
                        &retained.source_delivery
                    ),
                    (
                        &old.path,
                        old.start_line,
                        old.end_line,
                        &old.snippet,
                        &old.source_delivery
                    ),
                    "requested progress must not change a selected current source witness"
                );
            }
            let selected = packet
                .support
                .iter()
                .find(|row| row.symbol_id.as_deref() == Some("0") && row.source_delivery.is_some())
                .expect(
                    "the independently fitting combined candidate must retain requested progress",
                );
            assert!(
                selected
                    .end_line
                    .is_some_and(|end| end > 0 && end <= expected_end),
                "requested progress advances only with protected current context"
            );
            let delivery = selected.source_delivery.as_ref().unwrap();
            assert!(delivery.matches_delivery(1, selected.end_line.unwrap()));
            assert_eq!(
                delivery.next_start_line,
                (selected.end_line.unwrap() < requested_lines as u32)
                    .then_some(selected.end_line.unwrap() + 1)
            );
            assert_eq!(
                delivery.remaining_source_bytes > 0,
                selected.end_line.unwrap() < requested_lines as u32,
                "a complete requested range has no invented tail"
            );
            for original in frozen
                .context_floor
                .iter()
                .filter(|row| row.kind == SupportUnitKindDto::SourceRange)
            {
                assert!(source_floor_retained(
                    &packet.support,
                    std::slice::from_ref(original)
                ));
            }
            assert!(bytes <= 16384);
            assert!(
                measurements
                    <= 8 * codestory_agent::evidence_compiler::REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1 + 1,
                "all complete, refused and final replay callbacks use the unchanged whole-request bound"
            );
            assert!(
                choices.len() <= 358,
                "seven extents, at most fifty-one row proposals and one replay"
            );
            println!(
                "selected-source fixture requested_lines={requested_lines} context_rows={context_count} winner_end={:?} winner_rows={} wire_bytes={bytes} measured_choices={choices:?}",
                selected.end_line,
                packet
                    .support
                    .iter()
                    .filter(|row| row.source_delivery.is_some())
                    .count()
            );
            commit_measured_packet_compilation(&packet, frozen).unwrap();
        }
    }

    #[test]
    fn complete_callable_and_test_bodies_use_expanded_pages_and_honest_tails() {
        for (declaration, ending) in [
            ("fn transform() {", "}"),
            ("def test_roundtrip():", "    assert result"),
            ("void visit() {", "}"),
        ] {
            let body = format!(
                "{declaration}\r\n{}\r\n{ending}\r\n",
                ("    process(\"é\");\r\n").repeat(25)
            );
            let range = PacketSourceRangeV1 {
                start_line: 1,
                end_line: body.lines().count() as u32,
            };
            let hash = format!("{:x}", Sha256::digest(body.as_bytes()));
            let (_, prefix_end, prefix) = complete_node_page(&body, range, 512, &hash).unwrap();
            assert!(prefix.remaining_source_bytes > 0);
            assert!(prefix_end < range.end_line);
            let (complete, end, delivery) = complete_node_page(&body, range, 1024, &hash).unwrap();
            assert_eq!(end, range.end_line);
            assert_eq!(delivery.remaining_source_bytes, 0);
            assert!(complete.contains(ending));

            let long = format!(
                "{declaration}\n{}{ending}\n",
                "    process(\"é\");\n".repeat(300)
            );
            let last = long.lines().count() as u32;
            let mut next = 1;
            let mut pages = 0;
            loop {
                let (_, end, delivery) = complete_node_page(
                    &long,
                    PacketSourceRangeV1 {
                        start_line: next,
                        end_line: last,
                    },
                    1024,
                    &hash,
                )
                .unwrap();
                pages += 1;
                if let Some(cursor) = delivery.next_start_line {
                    assert_eq!(cursor, end + 1);
                    assert!(cursor > next);
                    next = cursor;
                } else {
                    assert_eq!(end, last);
                    assert_eq!(delivery.remaining_source_bytes, 0);
                    break;
                }
            }
            assert!(pages > 4);
        }
    }

    #[test]
    fn winning_source_extent_is_replayed_with_navigation_and_refuses_changed_measurement() {
        let text = "    process(42);\n".repeat(30);
        let requested = PacketSourceRangeV1 {
            start_line: 1,
            end_line: 30,
        };
        let page = |cap| {
            let (source, end_line, delivery) =
                complete_node_page(&text, requested, cap, &"a".repeat(64)).unwrap();
            PacketHydratedSourceRangeV1 {
                callable_body: None,
                stable_identity: "node:1".into(),
                path: "src/transform.rs".into(),
                symbol: None,
                start_line: 1,
                end_line,
                source,
                delivery: Some(delivery),
                parser_completeness: PacketParserCompletenessV1::Complete,
            }
        };
        let full = page(1024);
        let smaller = page(512);
        assert_eq!(full.delivery.as_ref().unwrap().remaining_source_bytes, 0);
        assert!(smaller.delivery.as_ref().unwrap().remaining_source_bytes > 0);
        let make = || {
            let input = PacketCompilationInputV1 {
                contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
                publication: PacketCompilationPublicationV1 {
                    project_id: "project".into(),
                    core_generation_id: "core".into(),
                    retrieval_generation: None,
                },
                admissions: (1..=2)
                    .map(|id| PacketAdmissionReceiptV1 {
                        packet_ordinal: id - 1,
                        stable_identity: format!("node:{id}"),
                        score_version: "test".into(),
                        reserved_source_bytes: 1024,
                        origin:
                            codestory_contracts::compilation::PacketAdmissionOriginV1::Retrieval,
                    })
                    .collect(),
                sources: vec![full.clone()],
                relations: vec![],
                ambiguities: vec![],
                admission_gaps: vec![],
            };
            let mut prepared = PreparedPacketCompilationV1::from_fixture_input(input);
            prepared
                .source_page_alternatives
                .insert(512, vec![smaller.clone()]);
            prepared
                .file_navigation_paths
                .insert("node:2".into(), "src/navigation.rs".into());
            prepared
        };
        let mut observed = Vec::new();
        let frozen = make()
            .select_for_representation(|prepared, product| {
                assert!(
                    product
                        .support
                        .iter()
                        .filter(|unit| unit.symbol_id.as_deref() == Some("2"))
                        .all(|unit| unit.path.as_deref() == Some("src/navigation.rs"))
                );
                observed.push(prepared.input.sources[0].source.len());
                Ok(Some(100))
            })
            .unwrap();
        assert!(observed.contains(&full.source.len()) && observed.contains(&smaller.source.len()));
        assert_eq!(observed.last(), Some(&full.source.len()));
        assert!(observed.len() <= 2 * 51 + 1);
        let final_measurement = observed.len();
        assert!(
            frozen
                .product
                .support
                .iter()
                .any(|unit| unit.symbol_id.as_deref() == Some("2")
                    && unit.path.as_deref() == Some("src/navigation.rs"))
        );

        assert_eq!(frozen.hydrated_sources[0], full);
        for refusal in ["none", "oversized", "error"] {
            let mut calls = 0;
            let result = make().select_for_representation(|_, _| {
                calls += 1;
                if calls < final_measurement {
                    return Ok(Some(100));
                }
                match refusal {
                    "none" => Ok(None),
                    "oversized" => Ok(Some(PUBLIC_PACKET_SERIALIZED_MAX_BYTES + 1)),
                    _ => Err(codestory_contracts::api::ApiError::internal(
                        "cancelled final measurement",
                    )),
                }
            });
            assert!(result.is_err(), "{refusal}");
            assert_eq!(calls, final_measurement);
        }
    }

    #[test]
    fn complete_source_pages_preserve_physical_lines_and_terminal_bounds() {
        let hash = "a".repeat(64);
        for text in ["éé\n\nlast", "éé\r\n\r\nlast", "a\n", "a", "a\r"] {
            let requested = PacketSourceRangeV1 {
                start_line: 1,
                end_line: text.lines().count() as u32,
            };
            let (full, end, delivery) = complete_node_page(text, requested, 512, &hash).unwrap();
            assert_eq!(end, requested.end_line);
            assert_eq!(delivery.remaining_source_bytes, 0);
            assert_eq!(delivery.next_start_line, None);
            assert_eq!(delivery.blocked_line_rendered_bytes, None);
            assert!(full.ends_with("\n```"));
            let (exact, exact_end, _) =
                complete_node_page(text, requested, full.len(), &hash).unwrap();
            assert_eq!((exact, exact_end), (full.clone(), end));
            if requested.end_line == 1 {
                assert_eq!(
                    complete_node_page(text, requested, full.len() - 1, &hash),
                    Err(PacketAdmissionGapKindV1::SourceBudgetExceeded)
                );
            } else {
                let (short, delivered, receipt) =
                    complete_node_page(text, requested, full.len() - 1, &hash).unwrap();
                assert!(short.ends_with("\n```"));
                assert_eq!(delivered, end - 1);
                assert_eq!(receipt.next_start_line, Some(end));
                assert_eq!(
                    receipt.remaining_source_bytes,
                    text.split_inclusive('\n').next_back().unwrap().len() as u64
                );
            }
        }
        let text = format!("first\r\n{}\r\nlast", "é".repeat(300));
        let (page, end, receipt) = complete_node_page(
            &text,
            PacketSourceRangeV1 {
                start_line: 1,
                end_line: 3,
            },
            512,
            &hash,
        )
        .unwrap();
        assert_eq!(end, 1);
        assert!(page.contains("first") && !page.contains('é'));
        assert_eq!(receipt.remaining_source_bytes, 600 + 2 + 4);
        assert_eq!(receipt.next_start_line, None);
        assert!(receipt.blocked_line_rendered_bytes.unwrap() > 512);
        assert_eq!(
            complete_node_page(
                &text,
                PacketSourceRangeV1 {
                    start_line: 2,
                    end_line: 3
                },
                512,
                &hash
            ),
            Err(PacketAdmissionGapKindV1::SourceBudgetExceeded)
        );
        assert_eq!(
            complete_node_page(
                "one\n",
                PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: 2
                },
                512,
                &hash
            ),
            Err(PacketAdmissionGapKindV1::SourceBoundMissing)
        );
        assert_eq!(
            complete_node_page(
                "one",
                PacketSourceRangeV1 {
                    start_line: 0,
                    end_line: 1
                },
                512,
                &hash
            ),
            Err(PacketAdmissionGapKindV1::SourceBoundMissing)
        );
    }

    #[test]
    fn source_page_blockage_counts_the_complete_standalone_page() {
        let hash = "a".repeat(64);
        for cap in [512, 1536, 4096] {
            for over_by in [0, 1] {
                // Nine prefix bytes, one newline and eleven fence bytes.
                let text = format!("first\n{}\n", "x".repeat(cap - 21 + over_by));
                let (_, end, delivery) = complete_node_page(
                    &text,
                    PacketSourceRangeV1 {
                        start_line: 1,
                        end_line: 2,
                    },
                    cap,
                    &hash,
                )
                .unwrap();
                assert_eq!(end, 1);
                if over_by == 0 {
                    assert_eq!(delivery.next_start_line, Some(2));
                    assert_eq!(delivery.blocked_line_rendered_bytes, None);
                    let (page, end, complete) = complete_node_page(
                        &text,
                        PacketSourceRangeV1 {
                            start_line: 2,
                            end_line: 2,
                        },
                        cap,
                        &hash,
                    )
                    .unwrap();
                    assert_eq!(page.len(), cap);
                    assert_eq!(end, 2);
                    assert_eq!(complete.remaining_source_bytes, 0);
                } else {
                    assert_eq!(delivery.next_start_line, None);
                    assert_eq!(delivery.blocked_line_rendered_bytes, Some(cap as u64 + 1));
                    assert_eq!(
                        complete_node_page(
                            &text,
                            PacketSourceRangeV1 {
                                start_line: 2,
                                end_line: 2
                            },
                            cap,
                            &hash
                        ),
                        Err(PacketAdmissionGapKindV1::SourceBudgetExceeded)
                    );
                }
            }
        }
    }

    #[test]
    fn source_range_option_ids_round_trip_and_reject_malformed_cursors() {
        let range = PacketSourceRangeV1 {
            start_line: 12,
            end_line: 80,
        };
        let original = DrillOptionDto::omitted_symbol("gap", "-42").with_source_range(range);
        let decoded = drill_options_from_ids(std::slice::from_ref(&original.id));
        assert_eq!(decoded[0].id, original.id);
        assert_eq!(decoded[0].symbol_id.as_deref(), Some("-42"));
        assert_eq!(decoded[0].source_range, Some(range));
        for target in [
            "range:0:9:symbol:1",
            "range:10:9:symbol:1",
            "range:01:9:symbol:1",
            "range:1:999999999999:symbol:1",
            "range:1:9:query:guess",
        ] {
            let id = codestory_contracts::api::encode_drill_option_id(
                DrillGapKindDto::OmittedMandatorySupport,
                target,
            );
            assert!(drill_options_from_ids(&[id]).is_empty(), "{target}");
        }
        assert!(
            drill_options_from_ids(
                &["omitted_mandatory_support:range%3a1%3A9%3Asymbol%3A1".into()]
            )
            .is_empty()
        );
    }

    #[test]
    fn compact_and_legacy_range_references_dispatch_identically_with_pins() {
        let legacy = "omitted_mandatory_support:range%3A12%3A80%3Asymbol%3A-42";
        let compact = "ms1:12:80:-42";
        let make_request = |id: &str| -> AgentPacketRequestDto {
            serde_json::from_value(serde_json::json!({
                "question":"inspect source", "parent_packet_id":"parent",
                "option_ids":[id], "core_generation_id":"core", "retrieval_generation":"retrieval"
            }))
            .unwrap()
        };
        let old = make_request(legacy);
        let new = make_request(compact);
        assert_eq!(drill_options_from_ids(&old.option_ids)[0].id, legacy);
        assert_eq!(drill_options_from_ids(&new.option_ids)[0].id, compact);
        let old_probes = continuation_probes_from_options(&old, "project").unwrap();
        let new_probes = continuation_probes_from_options(&new, "project").unwrap();
        assert_eq!(
            serde_json::to_value(&old_probes).unwrap(),
            serde_json::to_value(&new_probes).unwrap()
        );
        assert_eq!(old.option_ids, [legacy]);
        assert_eq!(new.option_ids, [compact]);
        let mut unpinned = new.clone();
        unpinned.core_generation_id = None;
        assert!(continuation_probes_from_options(&unpinned, "project").is_err());
        for id in [
            "ms2:12:80:-42",
            "ms1:12:80:-042",
            "ms1:12:80:-0",
            "ms1:12:80:-42:foreign",
            "ms1:0:80:-42",
        ] {
            assert!(
                continuation_probes_from_options(&make_request(id), "project").is_err(),
                "{id}"
            );
        }
    }

    #[test]
    fn canonical_coverage_keeps_nested_files_and_parser_status_distinct() {
        let process_cache = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        // The host path contains a source marker before the actual project root.
        let root = parent.path().join("src/checkout");
        std::fs::create_dir_all(&root).unwrap();
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
        controller.state.lock().project_root = Some(root.clone());
        let storage = Store::new_in_memory().unwrap();
        for (id, relative, complete) in [
            (1, "src/main.rs", true),
            (2, "tools/repos/worker/src/main.rs", false),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let text = format!("fn item_{id}() {{}}\n");
            std::fs::write(&path, &text).unwrap();
            let file = FileInfo {
                id,
                path,
                language: "rust".into(),
                modification_time: 0,
                indexed: true,
                complete,
                line_count: 1,
                file_role: FileRole::Source,
            };
            storage.insert_file(&file).unwrap();
            storage
                .update_file_metadata(
                    &file,
                    Some(&format!("{:x}", Sha256::digest(text.as_bytes()))),
                )
                .unwrap();
        }
        let observations = observe_admitted_source_coverage(
            &controller,
            &storage,
            &[
                root.join("src/main.rs").to_string_lossy().into_owned(),
                "./src/main.rs".into(),
                root.join("tools/repos/worker/src/main.rs")
                    .to_string_lossy()
                    .into_owned(),
                "tools/repos/worker/src/main.rs".into(),
            ],
        );
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].path, "src/main.rs");
        assert_eq!(observations[1].path, "tools/repos/worker/src/main.rs");
        assert_eq!(
            parser_completeness_for_path("src/main.rs", &observations),
            PacketParserCompletenessV1::Complete
        );
        assert_eq!(
            parser_completeness_for_path("tools/repos/worker/src/main.rs", &observations),
            PacketParserCompletenessV1::Partial
        );
        assert_eq!(
            parser_completeness_for_path("repos/other/src/main.rs", &observations),
            PacketParserCompletenessV1::Unknown
        );
        let outside = parent.path().join("outside.rs");
        std::fs::write(&outside, "fn outside() {}\n").unwrap();
        let unavailable = observe_admitted_source_coverage(
            &controller,
            &storage,
            &[outside.to_string_lossy().into_owned()],
        );
        assert_eq!(
            unavailable[0].status,
            SourceCoverageStatusDto::NotEstablished
        );
    }

    #[test]
    fn parser_partial_source_exception_never_crosses_nested_file_identity() {
        let observation = SourceCoverageObservationDto {
            path: "tools/repos/worker/src/main.rs".into(),
            status: SourceCoverageStatusDto::Incomplete,
            reason: Some(FileCoverageReason::ParserPartial),
            not_established_cause: None,
            observed_size: None,
            byte_cap: None,
        };
        let mut source = SupportUnitDto {
            source_delivery: None,
            id: "source:1".into(),
            kind: SupportUnitKindDto::SourceRange,
            summary: "bounded source".into(),
            path: Some("src/main.rs".into()),
            symbol_id: Some("1".into()),
            start_line: Some(1),
            end_line: Some(1),
            snippet: Some("fn root() {}".into()),
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        };
        assert!(
            packet_coverage_for_disposition(
                std::slice::from_ref(&observation),
                std::slice::from_ref(&source),
                &[],
            )
            .blocks_packet_availability()
        );
        source.path = Some(observation.path.clone());
        assert!(
            !packet_coverage_for_disposition(
                std::slice::from_ref(&observation),
                std::slice::from_ref(&source),
                &[],
            )
            .blocks_packet_availability()
        );
        source.snippet = None;
        assert!(
            packet_coverage_for_disposition(
                std::slice::from_ref(&observation),
                std::slice::from_ref(&source),
                &[],
            )
            .blocks_packet_availability()
        );
    }

    #[test]
    fn published_file_coverage_proof_is_live_and_never_replay_authority() {
        let source = PacketHydratedSourceRangeV1 {
            callable_body: None,
            stable_identity: "path:settings.ini".into(),
            path: "settings.ini".into(),
            symbol: None,
            start_line: 1,
            end_line: 1,
            source: "```text\n>    1 | value = 1\n```".into(),
            parser_completeness: PacketParserCompletenessV1::Unknown,
            delivery: Some(PacketSourceDeliveryV1 {
                requested_range: PacketSourceRangeV1 {
                    start_line: 1,
                    end_line: 1,
                },
                source_sha256: "a".repeat(64),
                remaining_source_bytes: 0,
                next_start_line: None,
                blocked_line_rendered_bytes: None,
            }),
        };
        let observation = source_coverage_not_established(&source.path);
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project".into(),
                core_generation_id: "core".into(),
                retrieval_generation: None,
            },
            admissions: Vec::new(),
            sources: vec![source.clone()],
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        let mut prepared = PreparedPacketCompilationV1::from_fixture_input(input);
        assert!(
            packet_coverage_for_disposition(
                std::slice::from_ref(&observation),
                &[],
                &prepared.published_file_sources
            )
            .blocks_packet_availability(),
            "source DTO shape alone has no live publication authority"
        );
        prepared.published_file_sources = vec![source];
        assert!(
            !packet_coverage_for_disposition(
                std::slice::from_ref(&observation),
                &[],
                &prepared.published_file_sources
            )
            .blocks_packet_availability(),
            "successful same-pin whole file proves deliberate Core absence even after output omission"
        );
        let mut encoded = serde_json::to_value(&prepared).unwrap();
        assert!(encoded.get("published_file_sources").is_none());
        encoded["published_file_sources"] =
            serde_json::to_value(&prepared.published_file_sources).unwrap();
        let replayed: PreparedPacketCompilationV1 = serde_json::from_value(encoded).unwrap();
        assert!(replayed.published_file_sources.is_empty());
        assert!(
            packet_coverage_for_disposition(
                std::slice::from_ref(&observation),
                &[],
                &replayed.published_file_sources
            )
            .blocks_packet_availability()
        );
        for mode in 0..4 {
            let mut blocked = observation.clone();
            match mode {
                0 => blocked.path = "other/settings.ini".into(),
                1 => {
                    blocked.not_established_cause =
                        Some(SourceCoverageNotEstablishedCauseDto::PublicationIncomplete)
                }
                2 => blocked.not_established_cause = None,
                _ => {
                    blocked.status = SourceCoverageStatusDto::Incomplete;
                    blocked.reason = Some(FileCoverageReason::ParserPartial);
                    blocked.not_established_cause = None;
                }
            }
            assert!(
                packet_coverage_for_disposition(&[blocked], &[], &prepared.published_file_sources)
                    .blocks_packet_availability(),
                "unrelated coverage failure remains closed: {mode}"
            );
        }
        assert_eq!(observation.status, SourceCoverageStatusDto::NotEstablished);
        assert_eq!(
            observation.not_established_cause,
            Some(SourceCoverageNotEstablishedCauseDto::LookupUnavailable)
        );
        assert_eq!(
            prepared.published_file_sources[0].parser_completeness,
            PacketParserCompletenessV1::Unknown
        );
    }

    #[test]
    fn registered_nested_source_refuses_missing_stale_and_directory_replacements() {
        let cache = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let controller = AppController::new_with_owned_cache_root(cache.path());
        controller.state.lock().project_root = Some(project.path().to_owned());
        let mut storage = Store::new_in_memory().unwrap();
        let path = project.path().join("tools/repos/worker/src/main.rs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = "fn action() {}\n";
        std::fs::write(&path, text).unwrap();
        let file = FileInfo {
            id: 1,
            path: path.clone(),
            language: "rust".into(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: FileRole::Source,
        };
        storage.insert_file(&file).unwrap();
        storage
            .update_file_metadata(
                &file,
                Some(&format!("{:x}", Sha256::digest(text.as_bytes()))),
            )
            .unwrap();
        storage
            .insert_nodes_batch(&[
                CoreNode {
                    id: CoreNodeId(1),
                    kind: CoreNodeKind::FILE,
                    serialized_name: "main.rs".into(),
                    ..Default::default()
                },
                CoreNode {
                    id: CoreNodeId(2),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "action".into(),
                    file_node_id: Some(CoreNodeId(1)),
                    start_line: Some(1),
                    end_line: Some(1),
                    ..Default::default()
                },
            ])
            .unwrap();
        let admissions = ["node:2", "path:tools/repos/worker/src/main.rs"]
            .into_iter()
            .enumerate()
            .map(|(ordinal, identity)| PacketAdmissionReceiptV1 {
                packet_ordinal: ordinal as u32,
                stable_identity: identity.into(),
                score_version: "test".into(),
                reserved_source_bytes: 512,
                origin: PacketAdmissionOriginV1::ExactTypedSelector,
            })
            .collect::<Vec<_>>();
        let mut gaps = Vec::new();
        let (authenticated, sources, _) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps).unwrap();
        assert_eq!(authenticated.len(), 2);
        assert_eq!(sources.len(), 2);
        assert!(gaps.is_empty());
        assert!(
            sources
                .iter()
                .all(|source| source.path == "tools/repos/worker/src/main.rs")
        );
        let refuse = |label: &str| {
            let mut gaps = Vec::new();
            let (authenticated, sources, navigation) =
                hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps).unwrap();
            assert!(
                authenticated.is_empty() && sources.is_empty() && navigation.is_empty(),
                "{label}"
            );
            assert_eq!(gaps.len(), 2, "{label}");
            assert!(
                gaps.iter()
                    .all(|gap| gap.kind == PacketAdmissionGapKindV1::SourceUnavailable),
                "{label}"
            );
            let input = PacketCompilationInputV1 {
                contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
                publication: PacketCompilationPublicationV1 {
                    project_id: "project".into(),
                    core_generation_id: "generation".into(),
                    retrieval_generation: None,
                },
                admissions: vec![],
                sources,
                relations: vec![],
                ambiguities: vec![],
                admission_gaps: gaps,
            };
            assert!(
                compile_repository_evidence(&input).support.is_empty(),
                "{label}"
            );
        };
        std::fs::remove_file(&path).unwrap();
        refuse("deleted registered file");
        std::fs::write(&path, "fn changed() {}\n").unwrap();
        refuse("stale registered hash");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        refuse("registered file replaced by directory");
        std::fs::remove_dir(&path).unwrap();
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            let target = outside.path().join("main.rs");
            std::fs::write(&target, text).unwrap();
            // Identical pinned bytes do not authorize an outside target.
            std::os::unix::fs::symlink(&target, &path).unwrap();
            refuse("escaping symlink with unchanged source bytes");
        }
    }

    #[test]
    fn authenticated_node_pages_advance_and_refuse_changed_or_out_of_bound_source() {
        let process_cache = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
        controller.state.lock().project_root = Some(project.path().to_path_buf());
        let storage = Store::new_in_memory().unwrap();
        let text = (1..=30)
            .map(|line| format!("let stage_{line:02} = \"{}\";\r\n", "x".repeat(40)))
            .collect::<String>();
        let path = project.path().join("stages.rs");
        std::fs::write(&path, &text).unwrap();
        let file = FileInfo {
            id: 1,
            path,
            language: "rust".into(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 30,
            file_role: FileRole::Source,
        };
        storage.insert_file(&file).unwrap();
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        storage.update_file_metadata(&file, Some(&hash)).unwrap();
        let node = CoreNode {
            id: CoreNodeId(2),
            kind: CoreNodeKind::FUNCTION,
            serialized_name: "stages".into(),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(1),
            end_line: Some(30),
            ..Default::default()
        };
        let admission = PacketAdmissionReceiptV1 {
            packet_ordinal: 0,
            stable_identity: "node:2".into(),
            score_version: "test".into(),
            reserved_source_bytes: 512,
            origin: PacketAdmissionOriginV1::ExactTypedSelector,
        };
        let first = hydrate_admitted_node_source_with_range(
            &controller,
            &storage,
            &admission,
            &node,
            None,
            &mut Vec::new(),
            &mut BTreeMap::new(),
        )
        .unwrap();
        let input = PacketCompilationInputV1 {
            contract_version: 1,
            publication: PacketCompilationPublicationV1 {
                project_id: "p".into(),
                core_generation_id: "g".into(),
                retrieval_generation: None,
            },
            admissions: vec![admission.clone()],
            sources: vec![first.clone()],
            relations: vec![],
            ambiguities: vec![],
            admission_gaps: vec![],
        };
        let product = compile_repository_evidence(&input);
        let selector = product
            .continuation
            .iter()
            .find(|selector| selector.source_range.is_some())
            .unwrap();
        let option = drill_option_from_selector(selector).unwrap();
        let decoded = drill_options_from_ids(&[option.id]);
        let second = hydrate_admitted_node_source_with_range(
            &controller,
            &storage,
            &admission,
            &node,
            decoded[0].source_range,
            &mut Vec::new(),
            &mut BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(second.start_line, first.end_line + 1);
        assert!(second.end_line > first.end_line);
        assert_eq!(second.delivery.as_ref().unwrap().source_sha256, hash);
        assert!(
            second.delivery.as_ref().unwrap().remaining_source_bytes
                < first.delivery.as_ref().unwrap().remaining_source_bytes
        );
        assert!(!second.source.contains("stage_01"));
        for range in [
            PacketSourceRangeV1 {
                start_line: 0,
                end_line: 30,
            },
            PacketSourceRangeV1 {
                start_line: 10,
                end_line: 31,
            },
        ] {
            assert_eq!(
                hydrate_admitted_node_source_with_range(
                    &controller,
                    &storage,
                    &admission,
                    &node,
                    Some(range),
                    &mut Vec::new(),
                    &mut BTreeMap::new(),
                ),
                Err(PacketAdmissionGapKindV1::SourceBoundMissing)
            );
        }
        std::fs::write(&file.path, text.replace("stage_01", "changed!")).unwrap();
        assert_eq!(
            hydrate_admitted_node_source_with_range(
                &controller,
                &storage,
                &admission,
                &node,
                decoded[0].source_range,
                &mut Vec::new(),
                &mut BTreeMap::new(),
            ),
            Err(PacketAdmissionGapKindV1::SourceUnavailable)
        );
    }

    #[test]
    fn emitted_range_reaches_pinned_freeze_and_public_projection() {
        for (first_overlong, enlarged_continuation, legacy_id) in [
            (false, false, false),
            (false, true, false),
            (true, false, false),
            (false, false, true),
            (false, true, true),
            (true, false, true),
        ] {
            use codestory_contracts::api::{PacketDispositionKindDto, PacketProbeDto};
            use codestory_contracts::packet_projection_v3::{GapKindV3Dto, PacketProjectionV3Dto};
            use codestory_store::{IndexPublicationMode, IndexPublicationRecord};
            use std::sync::{Arc, atomic::AtomicBool};
            let project = tempfile::tempdir().unwrap();
            let process_cache = tempfile::tempdir().unwrap();
            let database = process_cache.path().join("core.sqlite3");
            let mut storage = Store::open(&database).unwrap();
            let path = project.path().join("stages.rs");
            let text = (1..=30)
                .map(|line| format!("let stage_{line:02} = \"{}\";\r\n", "x".repeat(40)))
                .collect::<String>();
            let text = if first_overlong {
                format!("{}\n{}", "é".repeat(300), "\n".repeat(29))
            } else {
                text
            };
            std::fs::write(&path, &text).unwrap();
            let file = FileInfo {
                id: 1,
                path,
                language: "rust".into(),
                modification_time: 0,
                indexed: true,
                complete: true,
                line_count: 30,
                file_role: FileRole::Source,
            };
            storage.insert_file(&file).unwrap();
            let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
            storage.update_file_metadata(&file, Some(&hash)).unwrap();
            let unavailable_path = project.path().join("unavailable.h");
            let unavailable_text = "#define FLAG 1\n";
            std::fs::write(&unavailable_path, unavailable_text).unwrap();
            let unavailable_file = FileInfo {
                id: 3,
                path: unavailable_path,
                language: "c".into(),
                modification_time: 0,
                indexed: true,
                complete: false,
                line_count: 1,
                file_role: FileRole::Source,
            };
            storage.insert_file(&unavailable_file).unwrap();
            storage
                .update_file_metadata(
                    &unavailable_file,
                    Some(&format!(
                        "{:x}",
                        Sha256::digest(unavailable_text.as_bytes())
                    )),
                )
                .unwrap();
            storage
                .insert_nodes_batch(&[
                    CoreNode {
                        id: CoreNodeId(1),
                        kind: CoreNodeKind::FILE,
                        serialized_name: "stages.rs".into(),
                        ..Default::default()
                    },
                    CoreNode {
                        id: CoreNodeId(2),
                        kind: CoreNodeKind::FUNCTION,
                        serialized_name: "stages".into(),
                        file_node_id: Some(CoreNodeId(1)),
                        start_line: Some(1),
                        end_line: Some(30),
                        ..Default::default()
                    },
                    CoreNode {
                        id: CoreNodeId(3),
                        kind: CoreNodeKind::FILE,
                        serialized_name: "unavailable.h".into(),
                        ..Default::default()
                    },
                    CoreNode {
                        id: CoreNodeId(5),
                        kind: CoreNodeKind::FUNCTION,
                        serialized_name: "partial".into(),
                        file_node_id: Some(CoreNodeId(3)),
                        start_line: Some(1),
                        end_line: Some(1),
                        ..Default::default()
                    },
                    CoreNode {
                        id: CoreNodeId(4),
                        kind: CoreNodeKind::FUNCTION,
                        serialized_name: "boundless".into(),
                        file_node_id: Some(CoreNodeId(1)),
                        ..Default::default()
                    },
                ])
                .unwrap();
            let publication = IndexPublicationRecord {
                generation: 1,
                generation_id: "11111111-1111-4111-8111-111111111111".into(),
                run_id: "source-page-run".into(),
                mode: IndexPublicationMode::Full,
                published_at_epoch_ms: 1,
            };
            // Publish a genuine tiny lexical identity generation inside this test's tempdir.
            let search_path = crate::search_publication::search_index_path_for_publication(
                &database,
                Some(&publication),
            )
            .unwrap();
            let mut engine = crate::search::engine::SearchEngine::new(Some(&search_path)).unwrap();
            let mut symbol_session = engine.begin_symbol_index().unwrap();
            let (_, stats) = crate::search_publication::load_canonical_search_symbols(
                &storage,
                16,
                None,
                |batch| {
                    symbol_session
                        .add_nodes(
                            batch
                                .into_iter()
                                .map(|entry| (entry.node_id, entry.display_name)),
                        )
                        .map(|_| ())
                        .map_err(|error| {
                            codestory_contracts::api::ApiError::internal(error.to_string())
                        })
                },
            )
            .unwrap();
            symbol_session.finish().unwrap();
            crate::search_publication::write_search_generation_completion(
                &search_path,
                &publication,
                stats.search_symbol_stream_rows as usize,
                engine.tantivy_doc_count(),
            )
            .unwrap();
            drop(engine);
            storage.put_index_publication(&publication).unwrap();
            drop(storage);
            let controller = AppController::new_with_owned_cache_root(process_cache.path());
            {
                let mut state = controller.state.lock();
                state.project_root = Some(project.path().to_owned());
                state.storage_path = Some(database);
            }
            let service = crate::services::PublicOperationService::new(controller.clone());
            let project_id = codestory_workspace::project_identity_v3(project.path()).project_id;
            service.run_observational_with_cancel("source-pages", Arc::new(AtomicBool::new(false)), || {
            let initial_request: AgentPacketRequestDto = serde_json::from_value(serde_json::json!({"question":"show repository source"})).unwrap();
            let session = PacketProofSession::new();
            session.admit("node:2", 512);
            session.admit("path:unavailable.h", 512);
            session.admit("node:3", 512);
            session.admit("node:4", 512);
            session.admit("path:stages.rs", 512);
            let first = freeze_packet_compilation(&controller, &project_id, &[], None, &session, Some(&initial_request))?;
            if first_overlong {
                assert!(first.product.support.iter().all(|unit| unit.kind != SupportUnitKindDto::SourceRange));
                assert!(first.product.continuation.is_empty());
                assert_eq!(first.source_refusals.len(), 5);
                assert!(first.source_refusals[0].contains("zero complete lines delivered"));
                assert!(first.source_refusals[0].contains("630 source bytes remain"));
                assert!(first.source_refusals[0].contains(&hash));
                let mut packet = source_page_test_packet();
                apply_frozen_packet_compilation(&mut packet, Some(&initial_request), first);
                assert_eq!(packet.disposition.kind, PacketDispositionKindDto::NotEstablished);
                let public = crate::evidence_projection_v3::project_packet_v3(&service, "refusal-test-caller", &initial_request, &packet, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
                let PacketProjectionV3Dto::Complete { evidence, gaps, continuation, .. } = public.projection else { panic!("small refusal fits") };
                assert!(continuation.is_none());
                assert!(evidence.as_slice().iter().all(|row| row.kind != codestory_contracts::packet_projection_v3::EvidenceKindV3Dto::ExactSource));
                assert!(gaps.as_slice().iter().any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable && gap.message.as_ref().is_some_and(|message| message.as_str().contains("630 source bytes remain"))));
                return Ok(());
            }
            let selector = first.product.continuation.iter().find(|selector| selector.source_range.is_some()).unwrap();
            let first_end = first.product.support.iter().find(|unit| unit.source_delivery.is_some()).unwrap().end_line.unwrap();
            let first_unread = first.product.support.iter().find(|unit| unit.source_delivery.is_some()).unwrap().source_delivery.as_ref().unwrap().remaining_source_bytes;
            let option = drill_option_from_selector(selector).unwrap();
            assert_eq!(first.product.continuation.len(), 1, "only the verified cursor can progress");
            assert_eq!(first.source_refusals.len(), 4);
            assert!(first.source_refusals.iter().any(|receipt| receipt.contains("source bounds")));
            assert!(first.product.support.iter().all(|unit| unit.symbol_id.as_deref() == Some("2") || (unit.kind == SupportUnitKindDto::SymbolLocation && unit.path.as_deref() == Some("stages.rs") && unit.snippet.is_none())), "only verified file navigation and the bounded function survive");
            let mut initial_packet = source_page_test_packet();
            initial_packet.answer.source_coverage = first.source_coverage.clone();
            apply_frozen_packet_compilation(&mut initial_packet, Some(&initial_request), first);
            assert_eq!(initial_packet.disposition.kind, PacketDispositionKindDto::DrillOnce);
            let public = crate::evidence_projection_v3::project_packet_v3(&service, "mixed-refusal-caller", &initial_request, &initial_packet, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
            let PacketProjectionV3Dto::Complete { gaps, continuation, .. } = public.projection else { panic!("bounded mixed refusal fits") };
            assert_eq!(gaps.as_slice().iter().filter(|gap| gap.kind == GapKindV3Dto::SourceUnavailable).count(), 4);
            assert!(gaps.as_slice().iter().any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable && gap.message.as_ref().is_some_and(|message| message.as_str().contains("512-byte source-row cap"))));
            assert!(continuation.is_some());

            let navigation_session = PacketProofSession::new();
            navigation_session.admit("path:stages.rs", 512);
            let navigation = freeze_packet_compilation(&controller, &project_id, &[], None, &navigation_session, Some(&initial_request))?;
            assert_eq!(navigation.product.support.len(), 1);
            assert!(navigation.product.support.iter().all(|unit| unit.kind == SupportUnitKindDto::SymbolLocation && unit.path.as_deref() == Some("stages.rs") && unit.snippet.is_none()));
            assert!(navigation.product.continuation.is_empty());
            let mut navigation_packet = source_page_test_packet();
            apply_frozen_packet_compilation(&mut navigation_packet, Some(&initial_request), navigation);
            assert_eq!(navigation_packet.disposition.kind, PacketDispositionKindDto::NotEstablished);
            let public = crate::evidence_projection_v3::project_packet_v3(&service, "navigation-refusal-caller", &initial_request, &navigation_packet, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
            assert!(serde_json::to_vec(&public.projection).unwrap().len() <= codestory_contracts::compilation::PUBLIC_PACKET_SERIALIZED_MAX_BYTES);
            let PacketProjectionV3Dto::Complete { evidence, gaps, continuation, .. } = public.projection else { panic!("bounded file refusal fits") };
            assert!(continuation.is_none());
            assert!(evidence.as_slice().iter().all(|row| row.kind != codestory_contracts::packet_projection_v3::EvidenceKindV3Dto::ExactSource));
            assert!(gaps.as_slice().iter().any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable && gap.message.as_ref().is_some_and(|message| message.as_str().contains("512-byte source-row cap"))));
            assert!(gaps.as_slice().iter().any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable && gap.message.as_ref().is_some_and(|message| {
                let text = message.as_str();
                text.contains("verified whole-file") && text.contains("navigation only")
                    && text.contains("Identity: path:stages.rs") && text.contains("file: stages.rs")
            })));

            let refused_session = PacketProofSession::new();
            refused_session.admit("path:unavailable.h", 512);
            refused_session.admit("node:3", 512);
            refused_session.admit("node:4", 512);
            let refused = freeze_packet_compilation(&controller, &project_id, &[], None, &refused_session, Some(&initial_request))?;
            assert!(refused.product.support.is_empty() && refused.product.continuation.is_empty());
            let mut refused_packet = source_page_test_packet();
            apply_frozen_packet_compilation(&mut refused_packet, Some(&initial_request), refused);
            assert_eq!(refused_packet.disposition.kind, PacketDispositionKindDto::NotEstablished);
            let public = crate::evidence_projection_v3::project_packet_v3(&service, "empty-refusal-caller", &initial_request, &refused_packet, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
            let PacketProjectionV3Dto::Complete { evidence, gaps, continuation, .. } = public.projection else { panic!("bounded empty refusal fits") };
            assert!(evidence.as_slice().is_empty() && continuation.is_none());
            assert_eq!(gaps.as_slice().iter().filter(|gap| gap.kind == GapKindV3Dto::SourceUnavailable).count(), 3);
            let range = option.source_range.expect("verified range reference");
            assert_eq!(option.id, format!("ms1:{}:{}:2", range.start_line, range.end_line));
            let incoming_id = if legacy_id {
                format!("omitted_mandatory_support:range%3A{}%3A{}%3Asymbol%3A2", range.start_line, range.end_line)
            } else { option.id.clone() };
            let request: AgentPacketRequestDto = serde_json::from_value(serde_json::json!({
                "question":"show repository source", "parent_packet_id":"first-source-page",
                "core_generation_id":"11111111-1111-4111-8111-111111111111", "option_ids":[incoming_id.clone()],
            })).unwrap();
            let probes = continuation_probes_from_options(&request, &project_id)?;
            assert!(matches!(&probes[0], PacketProbeDto::Continuation { selector, .. } if selector.source_range.unwrap().start_line == first_end + 1));
            let resolutions = crate::agent::packet_probe::resolve_packet_probes(&controller, probes.clone());
            assert_eq!(resolutions[0].status, PacketProbeResolutionStatusDto::Continuation, "{:?}", resolutions[0]);
            assert!(resolutions[0].path.is_none(), "the production symbol resolver is identity-only");
            // Mixed spellings resolve to the same identity and one source
            // reservation, rather than granting a second page or admission.
            let mut mixed_request = request.clone();
            mixed_request.option_ids = vec![incoming_id, option.id.clone()];
            let mixed_probes = continuation_probes_from_options(&mixed_request, &project_id)?;
            assert_eq!(serde_json::to_value(&mixed_probes[0]).unwrap(), serde_json::to_value(&mixed_probes[1]).unwrap());
            let mixed_session = std::rc::Rc::new(PacketProofSession::new());
            let mixed_resolutions = {
                let _guard = crate::agent::packet_candidate::install_packet_proof_session(std::rc::Rc::clone(&mixed_session));
                crate::agent::packet_probe::resolve_packet_probes(&controller, mixed_probes)
            };
            if enlarged_continuation { mixed_session.seal_retrieval_admission(); }
            let mixed = freeze_packet_compilation(&controller, &project_id, &mixed_resolutions, None, &mixed_session, Some(&mixed_request))?;
            assert_eq!(mixed_session.receipts().len(), 1);
            assert_eq!(mixed.product.support.iter().filter(|row| row.source_delivery.is_some()).count(), 1);
            let second_session = PacketProofSession::new(); second_session.admit("node:2", 512);
            second_session.admit("node:5", 512);
            if enlarged_continuation { second_session.seal_retrieval_admission(); }
            let second = freeze_packet_compilation(&controller, &project_id, &resolutions, None, &second_session, Some(&request))?;
            let source = second.product.support.iter().find(|unit| unit.source_delivery.is_some()).unwrap();
            let mixed_page = mixed.product.support.iter().find(|row| row.source_delivery.is_some()).unwrap();
            assert_eq!(mixed_page, source, "mixed spellings cannot grant extra source progress");
            assert_eq!(source.start_line, Some(first_end + 1));
            assert!(source.end_line.unwrap() > first_end);
            assert!(source.source_delivery.as_ref().unwrap().remaining_source_bytes < first_unread);
            if enlarged_continuation {
                let reserved = second_session.receipts()[0].reserved_source_bytes as usize;
                assert!(reserved >= source.snippet.as_ref().unwrap().len() && reserved <= 4096,
                    "reserve the verified rendered demand within the unchanged maximum");
                assert_eq!(source.end_line, Some(30));
                assert_eq!(source.source_delivery.as_ref().unwrap().remaining_source_bytes, 0);
                assert!(source.snippet.as_ref().unwrap().len() > 512);
                assert!(source.snippet.as_ref().unwrap().len() <= 4096);
            }
            let expected_context = second.product.support.iter()
                .find(|row| row.symbol_id.as_deref() == Some("5")).unwrap().clone();
            let expected_receipt = source.source_delivery.clone();
            let expected_start = source.start_line.unwrap();
            let expected_end = source.end_line.unwrap();
            let mut packet = source_page_test_packet();
            packet.answer.source_coverage = second.source_coverage.clone();
            packet.plan.probe_resolutions = resolutions.clone();
            apply_frozen_packet_compilation(&mut packet, Some(&request), second);
            assert_eq!(packet.disposition.omission_receipts.iter().filter(|gap| gap.contains("unavailable.h") && gap.contains("incomplete_index")).count(), 1, "a retained partial-file page does not prove index coverage");
            assert_eq!(packet.disposition.kind, if enlarged_continuation { PacketDispositionKindDto::Supported } else { PacketDispositionKindDto::NotEstablished });
            assert!(packet.disposition.drill.is_none(), "one permitted round cannot emit another drill");
            let public = crate::evidence_projection_v3::project_packet_v3(&service, "page-test-caller", &request, &packet, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
            assert!(serde_json::to_vec(&public.projection).unwrap().len() <= codestory_contracts::compilation::PUBLIC_PACKET_SERIALIZED_MAX_BYTES);
            let PacketProjectionV3Dto::Complete { evidence, gaps, continuation, .. } = public.projection else { panic!("small page fits") };
            assert!(continuation.is_none());
            let public_page = evidence.as_slice().iter().find(|row| row.source_delivery.is_some()).unwrap();
            assert_eq!(public_page.source_delivery, expected_receipt);
            // Public line labels may compact transport padding, while source
            // text and physical line numbers must remain exact.
            let observed = public_page.summary.as_ref().unwrap().as_str()
                .strip_prefix("```text\n").unwrap().strip_suffix("\n```").unwrap()
                .lines().enumerate().map(|(index, line)| {
                    assert!(line.starts_with(if index == 0 { ">" } else { " " }));
                    let (number, body) = line.split_once(" | ").unwrap();
                    (number.trim().trim_start_matches('>').trim().parse::<u32>().unwrap(), body)
                }).collect::<Vec<_>>();
            let expected = text.lines().enumerate()
                .skip(expected_start as usize - 1)
                .take((expected_end - expected_start + 1) as usize)
                .map(|(index, body)| (index as u32 + 1, body)).collect::<Vec<_>>();
            assert_eq!(observed, expected);
            assert_eq!(public_page.start_line, Some(expected_start));
            assert_eq!(public_page.end_line, Some(expected_end));
            assert!(public_page.summary.as_ref().unwrap().as_str().ends_with("\n```"));
            assert_eq!(gaps.as_slice().iter().any(|gap| gap.kind == GapKindV3Dto::EvidenceMissing && gap.message.as_ref().is_some_and(|message| message.as_str().contains("source bytes remain"))), !enlarged_continuation);
            let prepared = prepare_packet_compilation(&controller, &project_id, &resolutions, None, &second_session, Some(&request))?;
            let mut saw_fitting_candidate = false; let mut accepted = None;
            let frozen = prepared.select_for_representation(|prepared, product| {
                let mut candidate = packet.clone(); prepared.apply_proposal(&mut candidate, Some(&request), product);
                if product.support.len() > 2 { return Ok(Some(16385)); }
                saw_fitting_candidate = true;
                assert!(candidate.disposition.drill.is_none());
                assert_eq!(candidate.disposition.omission_receipts.iter().filter(|gap| gap.contains("unavailable.h") && gap.contains("incomplete_index")).count(), 1, "restored coverage refusal is not duplicated");
                accepted = Some(candidate.clone());
                Ok(Some(serde_json::to_vec(&candidate).unwrap().len()))
            })?;
            assert!(saw_fitting_candidate);
            let accepted = accepted.unwrap();
            assert!(accepted.support.iter().any(|row| row.symbol_id.as_deref() == Some("2")
                && row.source_delivery == expected_receipt));
            assert!(accepted.support.contains(&expected_context),
                "authenticated current context remains byte-for-byte alongside requested progress");
            commit_measured_packet_compilation(&accepted, frozen)?;
            let public = crate::evidence_projection_v3::project_packet_v3(&service, "real-resolver-retained-context", &request, &accepted, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
            let PacketProjectionV3Dto::Complete { evidence, gaps, continuation, .. } = public.projection else { panic!("continued page with retained context fits") };
            assert!(continuation.is_none());
            assert!(evidence.as_slice().iter().any(|row| row.source_delivery == expected_receipt));
            assert!(gaps.as_slice().iter().any(|gap| gap.kind == GapKindV3Dto::SourceUnavailable && gap.message.as_ref().is_some_and(|message| message.as_str().contains("unavailable.h"))));
            if enlarged_continuation {
                let PacketProbeDto::Continuation { selector, .. } = &resolutions[0].probe else { unreachable!() };
                let requested = selector.source_range.unwrap();
                let reads = crate::search_evidence::verified_source_read_count();
                let charged = *second_session.admitted_source_bytes.borrow();
                let prepared = prepare_packet_compilation(&controller, &project_id, &resolutions, None, &second_session, Some(&request))?;
                assert!(!prepared.source_page_alternatives.is_empty());
                let reads_after_hydration = crate::search_evidence::verified_source_read_count();
                assert_eq!(reads_after_hydration, reads + 4,
                    "two file demand-preflight reads and two fresh hydration reads; selection reuses their pages");
                let envelope_bytes = |prepared: &PreparedPacketCompilationV1, product: &RepositoryDerivedCompilationV1, padding: usize| {
                    let mut candidate = packet.clone(); prepared.apply_proposal(&mut candidate, Some(&request), product);
                    let projection = crate::evidence_projection_v3::project_packet_v3(&service, "mirrored-complete-page", &request, &candidate, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
                    let PacketProjectionV3Dto::Complete { evidence, .. } = &projection.projection
                        else { panic!("the unpadded bounded fixture fits"); };
                    if product.support.iter().any(|row|
                        row.kind == SupportUnitKindDto::SourceRange
                            && row.symbol_id.as_deref() == Some("2")
                            && row.source_delivery.is_some()) {
                        assert!(evidence.as_slice().iter().any(|row|
                            row.symbol_id.as_ref().is_some_and(|id| id.as_str() == "2")
                            && row.path.as_ref().is_some_and(|path| path.as_str() == "stages.rs")
                            && row.source_delivery.as_ref().is_some_and(|delivery|
                                delivery.requested_range == requested && delivery.source_sha256 == hash)),
                            "an included requested page survives public projection");
                    }
                    let root = serde_json::to_value(&projection.projection).unwrap();
                    let envelope = serde_json::json!({"jsonrpc":"2.0", "id":"continued", "result":{
                        "structuredContent":root, "content":[{"type":"text","text":serde_json::to_string(&root).unwrap()}],
                        "_meta":{"padding":"m".repeat(padding)} }});
                    // Charge the complete JSON-RPC frame, including its newline.
                    Ok::<_, codestory_contracts::api::ApiError>((candidate, serde_json::to_vec(&envelope).unwrap().len() + 1))
                };
                let original_page = prepared.input.sources.iter().find(|page| page.stable_identity == "node:2").unwrap();
                let page_2048 = prepared.source_page_alternatives.get(&2048)
                    .and_then(|pages| pages.iter().find(|page| page.stable_identity == "node:2"))
                    .unwrap_or(original_page);
                assert_eq!(page_2048, original_page, "2048 bytes still deliver the original page, so the first smaller extent is at most1024");
                // Derive one fixed pressure from every original-page row cap.
                // This keeps the smallest original proposal over the real limit
                // despite unrelated fixture metadata or temporary path lengths.
                let mut smallest_original = usize::MAX;
                compile_repository_evidence_for_representation(&prepared.input, |product| {
                    if product.support.contains(&expected_context)
                        && product.support.iter().any(|unit| unit.symbol_id.as_deref() == Some("2") && unit.source_delivery.is_some()) {
                        let mut product = product.clone(); attach_file_navigation_paths(&mut product.support, &prepared.file_navigation_paths);
                        smallest_original = smallest_original.min(envelope_bytes(&prepared, &product, 0)?.1);
                    }
                    Ok(Some(16385))
                })?;
                assert_ne!(smallest_original, usize::MAX, "an original requested-page proposal exists");
                let padding = 16385usize.checked_sub(smallest_original).expect("the unpadded fixture fits before adding pressure");
                assert_eq!(smallest_original + padding, 16385);
                let mut accepted = None; let mut accepted_bytes = None; let mut tried_extents = BTreeSet::new();
                let frozen = prepared.select_for_representation(|prepared, product| {
                    let (candidate, bytes) = envelope_bytes(prepared, product, padding)?;
                    if let Some(page) = candidate.support.iter().find(|unit|
                        unit.symbol_id.as_deref() == Some("2") && unit.source_delivery.is_some()) {
                        tried_extents.insert(page.end_line.unwrap());
                    }
                    if bytes <= 16384 { accepted = Some(candidate); accepted_bytes = Some(bytes); }
                    Ok(Some(bytes))
                })?;
                assert!(accepted_bytes.is_some_and(|bytes| bytes <= 16384));
                let accepted = accepted.expect("a smaller complete-line page fits the mirrored envelope");
                assert!(accepted.support.contains(&expected_context));
                let page = accepted.support.iter().find(|unit|
                    unit.symbol_id.as_deref() == Some("2") && unit.source_delivery.is_some()).unwrap();
                assert!(tried_extents.len() > 1, "the original page alone cannot fit");
                assert!(page.end_line.unwrap() < 30);
                assert!(page.end_line.unwrap() > first_end);
                let (_, exact_end, exact_delivery) = complete_node_page(&text, requested, 1024, &hash).unwrap();
                assert!(page.end_line.unwrap() <= exact_end);
                assert_eq!(page.source_delivery.as_ref().unwrap().source_sha256, hash);
                let delivery = page.source_delivery.as_ref().unwrap();
                assert_eq!(delivery.next_start_line, Some(page.end_line.unwrap() + 1));
                assert!(delivery.remaining_source_bytes >= exact_delivery.remaining_source_bytes);
                assert_eq!(delivery.remaining_source_bytes, text.split_inclusive('\n').skip(page.end_line.unwrap() as usize).map(str::len).sum::<usize>() as u64);
                assert_eq!(delivery.requested_range, requested);
                assert_eq!(accepted.disposition.kind, PacketDispositionKindDto::NotEstablished);
                assert!(accepted.disposition.drill.is_none());
                assert_eq!(accepted.disposition.omission_receipts.iter().filter(|gap| gap.contains("unavailable.h") && gap.contains("incomplete_index")).count(), 1);
                let public = crate::evidence_projection_v3::project_packet_v3(&service, "mirrored-gap-retention", &request, &accepted, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
                let PacketProjectionV3Dto::Complete { gaps, .. } = public.projection else { panic!("accepted continued page fits") };
                assert_eq!(gaps.as_slice().iter().filter(|gap| gap.kind == GapKindV3Dto::SourceUnavailable && gap.message.as_ref().is_some_and(|message| message.as_str().contains("unavailable.h"))).count(), 1);
                assert_eq!(crate::search_evidence::verified_source_read_count(), reads_after_hydration);
                assert_eq!(*second_session.admitted_source_bytes.borrow(), charged);
                commit_measured_packet_compilation(&accepted, frozen)?;
            }
            for mutation in ["core", "project", "status", "node", "retrieval"] {
                let mut hostile = resolutions.clone();
                match mutation {
                    "status" => hostile[0].status = PacketProbeResolutionStatusDto::Rejected,
                    "node" => hostile[0].symbol_id = Some("3".into()),
                    _ => if let PacketProbeDto::Continuation { project_id, core_generation_id, retrieval_generation, .. } = &mut hostile[0].probe { match mutation { "core" => *core_generation_id = "different".into(), "project" => *project_id = "different".into(), _ => *retrieval_generation = Some("different".into()) } },
                }
                let reads = crate::search_evidence::verified_source_read_count();
                let charged = *second_session.admitted_source_bytes.borrow();
                let receipts = second_session.receipts();
                assert!(freeze_packet_compilation(&controller, &project_id, &hostile, None, &second_session, Some(&request)).is_err(), "{mutation}");
                assert_eq!(crate::search_evidence::verified_source_read_count(), reads, "refuse {mutation} before source hydration");
                assert_eq!(*second_session.admitted_source_bytes.borrow(), charged, "refuse {mutation} before reservation growth");
                assert_eq!(second_session.receipts(), receipts, "refuse {mutation} without changing delivery reservations");
            }
            let mut conflict = resolutions.clone(); let mut different = resolutions[0].clone();
            if let PacketProbeDto::Continuation { selector, .. } = &mut different.probe { selector.source_range.as_mut().unwrap().start_line += 1; }
            conflict.push(different);
            assert!(freeze_packet_compilation(&controller, &project_id, &conflict, None, &second_session, Some(&request)).is_err());
            let mut stale_request = request.clone(); stale_request.core_generation_id = Some("different".into());
            let reads = crate::search_evidence::verified_source_read_count();
            assert!(freeze_packet_compilation(&controller, &project_id, &resolutions, None, &second_session, Some(&stale_request)).is_err());
            assert_eq!(crate::search_evidence::verified_source_read_count(), reads);
            Ok(())
        }).unwrap();
        }
    }

    fn source_page_test_packet() -> AgentPacketDto {
        let trace = serde_json::json!({"request_id":"source-page-request", "resolved_profile":"investigate",
            "policy_mode":"latency_first", "total_latency_ms":0, "sla_missed":false,
            "semantic_fallback_count":0, "semantic_fallbacks":[], "semantic_stage_timeout_zero_hits":0,
            "semantic_abstained_count":0, "annotations":[], "steps":[], "packet_sidecar_diagnostics":[]});
        serde_json::from_value(serde_json::json!({
            "packet_id":"source-page-request", "question":"show repository source", "plan":{"queries":[],"trace":[]},
            "answer":{"answer_id":"source-page-answer", "prompt":"show repository source", "summary":"fixture",
                "sections":[], "citations":[], "subgraph_ids":[], "retrieval_version":"fixture", "graphs":[], "retrieval_trace":trace,
                "freshness":{"status":"fresh", "changed_file_count":0, "new_file_count":0, "removed_file_count":0,
                    "checked_file_count":0, "indexed_file_count":0, "duration_ms":0}},
            "support":[], "disposition":{"kind":"not_established"},
            "budget":{"requested":"standard", "limits":{"max_anchors":16,"max_files":16,"max_snippets":16,"max_trail_edges":16,"max_output_bytes":65536},
                "used":{"anchors":0,"files":0,"snippets":0,"trail_edges":0,"output_bytes":0}, "truncated":false, "next_deeper_command":null},
            "retrieval_trace_summary":{"retrieval_trace":trace,"source_read_steps":0,"search_steps":0,"trail_steps":0}
        })).unwrap()
    }

    fn partial_coverage_continuation_fixture() -> (AgentPacketDto, Vec<PacketContinuationSelectorV1>)
    {
        let mut packet = source_page_test_packet();
        packet.support = vec![
            serde_json::from_value(serde_json::json!({
                "id":"source:node:1", "kind":"source_range", "summary":"source page",
                "path":"src/retained.rs", "symbol_id":"1", "start_line":1, "end_line":1,
                "snippet":"```text\n>    1 | fn retained() {}\n```",
                "source_delivery":{"requested_range":{"start_line":1,"end_line":3},
                    "source_sha256":"a".repeat(64), "remaining_source_bytes":40,
                    "next_start_line":2, "blocked_line_rendered_bytes":null}
            }))
            .unwrap(),
        ];
        packet.answer.source_coverage = vec![SourceCoverageObservationDto {
            path: "src/omitted.rs".into(),
            status: SourceCoverageStatusDto::Incomplete,
            reason: Some(FileCoverageReason::ParserPartial),
            not_established_cause: None,
            observed_size: None,
            byte_cap: None,
        }];
        let retained = PacketContinuationSelectorV1 {
            stable_identity: "node:1".into(),
            path: None,
            symbol_id: Some("1".into()),
            reason: PacketStructuralGapReasonV1::SourceBudgetExceeded,
            source_range: Some(PacketSourceRangeV1 {
                start_line: 2,
                end_line: 3,
            }),
        };
        let omitted = PacketContinuationSelectorV1 {
            stable_identity: "node:2".into(),
            symbol_id: Some("2".into()),
            ..retained.clone()
        };
        (packet, vec![omitted, retained])
    }

    #[test]
    fn partial_prefix_keeps_original_action_and_authenticated_tail() {
        let (mut baseline, _) = partial_coverage_continuation_fixture();
        let source_row = baseline.support[0].clone();
        baseline.support.clear();
        baseline.disposition.kind = codestory_contracts::api::PacketDispositionKindDto::DrillOnce;
        let selector = PacketContinuationSelectorV1 {
            stable_identity: "node:1".into(),
            path: None,
            symbol_id: Some("1".into()),
            reason: PacketStructuralGapReasonV1::SourceBudgetExceeded,
            source_range: Some(PacketSourceRangeV1 {
                start_line: 1,
                end_line: 3,
            }),
        };
        let source = PacketHydratedSourceRangeV1 {
            callable_body: None,
            stable_identity: "node:1".into(),
            path: "src/retained.rs".into(),
            symbol: Some("retained".into()),
            start_line: 1,
            end_line: 1,
            source: source_row.snippet.clone().unwrap(),
            delivery: source_row.source_delivery.clone(),
            parser_completeness: PacketParserCompletenessV1::Complete,
        };
        let mut option = drill_option_from_selector(&selector).unwrap();
        option.source_label = continuation_source_label(&selector, std::slice::from_ref(&source));
        baseline.disposition.drill = Some(BoundedDrillPlanDto {
            parent_packet_id: baseline.packet_id.clone(),
            core_generation_id: "core".into(),
            retrieval_generation: Some("retrieval".into()),
            gap_ids: vec![option.gap_id.clone()],
            options: vec![option.clone()],
            max_bytes: 32768,
            max_hits: 8,
            max_depth: 2,
            remaining_rounds: 1,
        });
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project".into(),
                core_generation_id: "core".into(),
                retrieval_generation: Some("retrieval".into()),
            },
            admissions: vec![],
            sources: vec![source],
            relations: vec![],
            ambiguities: vec![],
            admission_gaps: vec![],
        };
        let prepared = PreparedPacketCompilationV1::from_fixture_input(input.clone());
        let tail_selector = PacketContinuationSelectorV1 {
            source_range: Some(PacketSourceRangeV1 {
                start_line: 2,
                end_line: 3,
            }),
            ..selector
        };
        let mut tail = drill_option_from_selector(&tail_selector).unwrap();
        tail.source_label = continuation_source_label(&tail_selector, &input.sources);
        let mut canonical = baseline.clone();
        canonical.support = vec![source_row];
        canonical.disposition.drill.as_mut().unwrap().gap_ids = vec![tail.gap_id.clone()];
        canonical.disposition.drill.as_mut().unwrap().options = vec![tail];
        assert!(
            prepared
                .resolved_baseline_option_witnesses(&baseline, &canonical)
                .is_empty()
        );
        let mut candidate = canonical.clone();
        assert!(prepared.retain_baseline_continuation_actions(&baseline, &mut candidate));
        assert_eq!(candidate.disposition.drill, baseline.disposition.drill);
        assert_eq!(candidate.support, canonical.support);
        assert_eq!(
            candidate.support[0]
                .source_delivery
                .as_ref()
                .unwrap()
                .next_start_line,
            Some(2)
        );
        // The final public frame keeps the old action and the independent row tail.
        use crate::evidence_projection_v3::representation_budget_test_support as fixture;
        use codestory_contracts::packet_projection_v3::PacketProjectionV3Dto;
        let request: AgentPacketRequestDto =
            serde_json::from_value(serde_json::json!({"question":"Read repository evidence"}))
                .unwrap();
        let (shell, _) = fixture::default_packet(input.clone(), &request);
        let mut old_frame = baseline.clone();
        let mut new_frame = candidate.clone();
        for packet in [&mut old_frame, &mut new_frame] {
            packet.answer.retrieval_trace.retrieval_publication =
                shell.answer.retrieval_trace.retrieval_publication.clone();
            packet.retrieval_trace_summary.retrieval_trace = packet.answer.retrieval_trace.clone();
            let drill = packet.disposition.drill.as_mut().unwrap();
            drill.core_generation_id = "core-generation-1".into();
            drill.retrieval_generation = Some("retrieval-generation-1".into());
        }
        let project = |packet| {
            fixture::project_packet(&request, packet, |frame| {
                Ok(serde_json::to_vec(frame).unwrap().len())
            })
            .unwrap()
            .projection
        };
        let old_projection = project(&old_frame);
        let new_projection = project(&new_frame);
        assert!(
            crate::evidence_projection_v3::packet_projection_preserves_baseline_v3(
                &old_projection,
                &new_projection,
                &BTreeMap::new(),
                &new_frame.support
            )
        );
        let PacketProjectionV3Dto::Complete {
            evidence,
            continuation,
            ..
        } = new_projection
        else {
            panic!("bounded frame fits");
        };
        let tail_patch = evidence.as_slice()[0]
            .packet_request_patch
            .as_ref()
            .unwrap();
        assert_eq!(tail_patch.option_ids.as_slice()[0].as_str(), "ms1:2:3:1");
        assert!(
            continuation
                .unwrap()
                .packet_request_patch
                .unwrap()
                .option_ids
                .as_slice()
                .iter()
                .any(|id| id.as_str() == option.id)
        );
        // Refuse mismatched identity, range, delivery, exact row, tail, and authority.
        for mutation in 0..12 {
            let mut bad_input = input.clone();
            let mut bad = canonical.clone();
            match mutation {
                0 => bad_input.sources[0].stable_identity = "node:other".into(),
                1 => bad_input.sources[0].path = "other.rs".into(),
                2 => {
                    bad_input.sources[0]
                        .delivery
                        .as_mut()
                        .unwrap()
                        .requested_range
                        .start_line = 2
                }
                3 => {
                    bad_input.sources[0]
                        .delivery
                        .as_mut()
                        .unwrap()
                        .requested_range
                        .end_line = 4
                }
                4 => {
                    bad_input.sources[0]
                        .delivery
                        .as_mut()
                        .unwrap()
                        .source_sha256 = "b".repeat(64)
                }
                5 => {
                    bad_input.sources[0]
                        .delivery
                        .as_mut()
                        .unwrap()
                        .next_start_line = Some(3)
                }
                6 => bad.support[0].snippet = Some("altered".into()),
                7 => bad.support.clear(),
                8 => {
                    bad.disposition.drill.as_mut().unwrap().options[0]
                        .source_range
                        .as_mut()
                        .unwrap()
                        .start_line = 3
                }
                9 => bad.disposition.drill.as_mut().unwrap().options[0].source_label = None,
                10 => bad.disposition.drill.as_mut().unwrap().core_generation_id = "other".into(),
                _ => bad.disposition.drill.as_mut().unwrap().max_bytes = 1,
            }
            let before = bad.clone();
            assert!(
                !PreparedPacketCompilationV1::from_fixture_input(bad_input)
                    .retain_baseline_continuation_actions(&baseline, &mut bad),
                "mutation {mutation}"
            );
            assert_eq!(
                serde_json::to_value(bad).unwrap(),
                serde_json::to_value(before).unwrap()
            );
        }
    }

    #[test]
    fn baseline_action_resolution_requires_authenticated_whole_range() {
        use codestory_contracts::api::{BoundedDrillPlanDto, DrillOptionDto};
        let (mut baseline, _) = partial_coverage_continuation_fixture();
        let mut candidate = baseline.clone();
        let mut source = candidate.support[0].clone();
        source.end_line = Some(3);
        source.snippet =
            Some("```text\n>    1 | fn retained() {\n>    2 | work();\n>    3 | }\n```".into());
        source
            .source_delivery
            .as_mut()
            .unwrap()
            .remaining_source_bytes = 0;
        source.source_delivery.as_mut().unwrap().next_start_line = None;
        candidate.support = vec![source.clone()];
        let range = PacketSourceRangeV1 {
            start_line: 1,
            end_line: 3,
        };
        let option = DrillOptionDto::omitted_symbol("source_budget_exceeded:node:1", "1")
            .with_source_range(range);
        baseline.disposition.drill = Some(BoundedDrillPlanDto {
            parent_packet_id: baseline.packet_id.clone(),
            core_generation_id: "core".into(),
            retrieval_generation: Some("retrieval".into()),
            gap_ids: vec![option.gap_id.clone()],
            options: vec![option.clone()],
            max_bytes: 32768,
            max_hits: 8,
            max_depth: 2,
            remaining_rounds: 1,
        });
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project".into(),
                core_generation_id: "core".into(),
                retrieval_generation: Some("retrieval".into()),
            },
            admissions: vec![],
            sources: vec![PacketHydratedSourceRangeV1 {
                callable_body: None,
                stable_identity: "node:1".into(),
                path: source.path.clone().unwrap(),
                symbol: None,
                start_line: 1,
                end_line: 3,
                source: source.snippet.clone().unwrap(),
                delivery: source.source_delivery.clone(),
                parser_completeness: PacketParserCompletenessV1::Complete,
            }],
            relations: vec![],
            ambiguities: vec![],
            admission_gaps: vec![],
        };
        let prepared = PreparedPacketCompilationV1::from_fixture_input(input.clone());
        assert!(
            prepared
                .resolved_baseline_option_witnesses(&baseline, &candidate)
                .contains_key(&option.id)
        );
        for mutation in 0..7 {
            let mut bad_input = input.clone();
            let mut old = baseline.clone();
            match mutation {
                0 => old.disposition.drill.as_mut().unwrap().options[0].source_range = None,
                1 => bad_input.sources[0].delivery = None,
                2 => {
                    bad_input.sources[0]
                        .delivery
                        .as_mut()
                        .unwrap()
                        .source_sha256 = "invalid".into()
                }
                3 => bad_input.sources[0].end_line = 2,
                4 => {
                    old.disposition.drill.as_mut().unwrap().options[0].path =
                        Some("wrong.rs".into())
                }
                5 => {
                    old.disposition.drill.as_mut().unwrap().options[0].symbol_id =
                        Some("other".into())
                }
                _ => bad_input.sources[0].start_line = 2,
            }
            assert!(
                PreparedPacketCompilationV1::from_fixture_input(bad_input)
                    .resolved_baseline_option_witnesses(&old, &candidate)
                    .is_empty(),
                "mutation {mutation}"
            );
        }
    }

    #[test]
    fn baseline_action_frontier_retains_authority_without_refilling() {
        use codestory_contracts::api::{BoundedDrillPlanDto, DrillOptionDto};
        let (mut baseline, _) = partial_coverage_continuation_fixture();
        let option = |id: &str| {
            DrillOptionDto::omitted_symbol(format!("source_budget_exceeded:node:{id}"), id)
        };
        let a = option("1");
        let b = option("2");
        let c = option("3");
        baseline.disposition.drill = Some(BoundedDrillPlanDto {
            parent_packet_id: baseline.packet_id.clone(),
            core_generation_id: "core".into(),
            retrieval_generation: Some("retrieval".into()),
            gap_ids: vec![a.gap_id.clone(), b.gap_id.clone()],
            options: vec![a.clone(), b.clone()],
            max_bytes: 32768,
            max_hits: 8,
            max_depth: 2,
            remaining_rounds: 1,
        });
        let empty = BTreeMap::new();
        let mut unchanged = baseline.clone();
        assert!(retain_current_baseline_actions(
            &baseline,
            &mut unchanged,
            &empty
        ));
        assert_eq!(
            serde_json::to_value(&unchanged).unwrap(),
            serde_json::to_value(&baseline).unwrap()
        );
        let mut candidate = baseline.clone();
        candidate.disposition.drill.as_mut().unwrap().options = vec![b.clone(), c.clone()];
        let omissions = candidate.disposition.omission_receipts.clone();
        let coverage = candidate.answer.source_coverage.clone();
        let resolved = BTreeMap::from([(a.id.clone(), baseline.support[0].clone())]);
        assert!(retain_current_baseline_actions(
            &baseline,
            &mut candidate,
            &resolved
        ));
        assert_eq!(
            candidate.disposition.drill.as_ref().unwrap().options,
            vec![b.clone()]
        );
        assert_eq!(candidate.disposition.omission_receipts, omissions);
        assert_eq!(candidate.answer.source_coverage, coverage);
        for mutation in 0..4 {
            let mut stale = baseline.clone();
            let drill = stale.disposition.drill.as_mut().unwrap();
            match mutation {
                0 => drill.core_generation_id = "other".into(),
                1 => drill.parent_packet_id = "other".into(),
                2 => drill.remaining_rounds = 0,
                _ => drill.options[1].path = Some("changed.rs".into()),
            }
            let before = stale.clone();
            assert!(!retain_current_baseline_actions(
                &baseline, &mut stale, &empty
            ));
            assert_eq!(
                serde_json::to_value(&stale).unwrap(),
                serde_json::to_value(&before).unwrap()
            );
        }
        let both = BTreeMap::from([
            (a.id.clone(), baseline.support[0].clone()),
            (b.id.clone(), baseline.support[0].clone()),
        ]);
        let mut all_resolved = candidate.clone();
        all_resolved.disposition.drill.as_mut().unwrap().options = vec![c];
        let before = all_resolved.clone();
        assert!(retain_current_baseline_actions(
            &baseline,
            &mut all_resolved,
            &both
        ));
        assert_eq!(
            serde_json::to_value(&all_resolved).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
    }

    #[test]
    fn retained_source_cursor_preserves_unrelated_parser_gap_before_projection() {
        let (packet, selectors) = partial_coverage_continuation_fixture();
        let disposition = classify_packet_disposition(
            &packet,
            None,
            &selectors,
            &[],
            "project",
            "core".into(),
            Some("retrieval".into()),
            &[],
        );
        assert_eq!(
            disposition.kind,
            codestory_contracts::api::PacketDispositionKindDto::DrillOnce
        );
        let drill = disposition.drill.as_ref().unwrap();
        assert_eq!(drill.options.len(), 1);
        assert_eq!(drill.options[0].symbol_id.as_deref(), Some("1"));
        assert_eq!(drill.options[0].source_range, selectors[1].source_range);
        assert_eq!(drill.remaining_rounds, 1);
        assert!(
            disposition
                .omission_receipts
                .iter()
                .any(|gap| gap.contains("incomplete_index") && gap.contains("src/omitted.rs"))
        );
        assert!(
            packet
                .answer
                .source_coverage
                .iter()
                .any(|observation| observation.reason == Some(FileCoverageReason::ParserPartial))
        );
    }

    #[test]
    fn parser_gap_cursor_exception_never_accepts_other_failures_or_unserved_ranges() {
        use codestory_contracts::api::PacketDispositionKindDto;
        let (packet, selectors) = partial_coverage_continuation_fixture();
        let classify = |packet: &AgentPacketDto,
                        selectors: &[PacketContinuationSelectorV1],
                        request: Option<&AgentPacketRequestDto>| {
            classify_packet_disposition(
                packet,
                request,
                selectors,
                &[],
                "project",
                "core".into(),
                Some("retrieval".into()),
                &[],
            )
        };
        for status in [
            SourceCoverageStatusDto::PolicyExcluded,
            SourceCoverageStatusDto::NotEstablished,
            SourceCoverageStatusDto::Incomplete,
        ] {
            let mut bad = packet.clone();
            let mut observation = bad.answer.source_coverage[0].clone();
            observation.path = "src/another.rs".into();
            observation.status = status;
            observation.reason = None;
            bad.answer.source_coverage.push(observation);
            assert_eq!(
                classify(&bad, &selectors, None).kind,
                PacketDispositionKindDto::Unavailable
            );
        }
        for mutation in 0..6 {
            let mut bad = selectors[1].clone();
            match mutation {
                0 => bad.path = Some("src/conflicting.rs".into()),
                1 => bad.symbol_id = Some("99".into()),
                2 => bad.stable_identity = "node:99".into(),
                3 => bad.source_range.as_mut().unwrap().start_line = 1,
                4 => bad.source_range.as_mut().unwrap().end_line = 4,
                _ => bad.reason = PacketStructuralGapReasonV1::DisconnectedSeed,
            }
            assert_eq!(
                classify(&packet, &[bad], None).kind,
                PacketDispositionKindDto::Unavailable
            );
        }
        let mut oversized_identity = packet.clone();
        let id = "9".repeat(300);
        oversized_identity.support[0].symbol_id = Some(id.clone());
        let mut oversized_selector = selectors[1].clone();
        oversized_selector.stable_identity = format!("node:{id}");
        oversized_selector.symbol_id = Some(id);
        let refused = classify(&oversized_identity, &[oversized_selector], None);
        assert_eq!(refused.kind, PacketDispositionKindDto::Unavailable);
        assert!(
            refused
                .reason
                .as_deref()
                .unwrap()
                .contains("incomplete_index")
        );
        let mut no_receipt = packet.clone();
        no_receipt.support[0].source_delivery = None;
        assert_eq!(
            classify(&no_receipt, &selectors, None).kind,
            PacketDispositionKindDto::Unavailable
        );
        let mut blocked = packet.clone();
        let receipt = blocked.support[0].source_delivery.as_mut().unwrap();
        receipt.next_start_line = None;
        receipt.blocked_line_rendered_bytes = Some(513);
        assert_eq!(
            classify(&blocked, &selectors, None).kind,
            PacketDispositionKindDto::Unavailable
        );
        let mut malformed = packet.clone();
        malformed.support[0]
            .source_delivery
            .as_mut()
            .unwrap()
            .source_sha256 = "bad".into();
        assert_eq!(
            classify(&malformed, &selectors, None).kind,
            PacketDispositionKindDto::Unavailable
        );
        let mut hard_error = packet.clone();
        hard_error.answer.retrieval_trace.steps.push(
            codestory_contracts::api::AgentRetrievalStepDto {
                kind: codestory_contracts::api::AgentRetrievalStepKindDto::Search,
                status: codestory_contracts::api::AgentRetrievalStepStatusDto::Error,
                duration_ms: 0,
                input: vec![],
                output: vec![],
                message: None,
            },
        );
        assert_eq!(
            classify(&hard_error, &selectors, None).kind,
            PacketDispositionKindDto::Unavailable
        );
        let mut stale = packet.clone();
        stale.answer.freshness = None;
        assert_eq!(
            classify(&stale, &selectors, None).kind,
            PacketDispositionKindDto::Unavailable
        );
        for request in [
            serde_json::from_value::<AgentPacketRequestDto>(serde_json::json!(
                {"question":"Inspect","core_generation_id":"other"}))
            .unwrap(),
            serde_json::from_value::<AgentPacketRequestDto>(serde_json::json!(
                {"question":"Inspect","parent_packet_id":"parent"}))
            .unwrap(),
        ] {
            assert_eq!(
                classify(&packet, &selectors, Some(&request)).kind,
                PacketDispositionKindDto::Unavailable
            );
        }
    }

    fn delivered_continuation_fixture() -> (AgentPacketDto, AgentPacketRequestDto) {
        let (mut packet, selectors) = partial_coverage_continuation_fixture();
        let selected = selectors[1].clone();
        let unit = &mut packet.support[0];
        unit.start_line = Some(2);
        unit.end_line = Some(2);
        unit.snippet = Some("```text\n>    2 | fn retained() {}\n```".into());
        let delivery = unit.source_delivery.as_mut().unwrap();
        delivery.requested_range = selected.source_range.unwrap();
        delivery.next_start_line = Some(3);
        packet.plan.probe_resolutions = vec![PacketProbeResolutionDto {
            input_index: 0,
            probe: codestory_contracts::api::PacketProbeDto::Continuation {
                contract_version: codestory_contracts::api::PACKET_PROBE_CONTRACT_VERSION,
                project_id: "project".into(),
                core_generation_id: "core".into(),
                retrieval_generation: Some("retrieval".into()),
                selector: selected,
            },
            status: PacketProbeResolutionStatusDto::Continuation,
            normalized_query: None,
            path: None,
            symbol_id: Some("1".into()),
            candidates: Vec::new(),
            rejection: None,
        }];
        let request = serde_json::from_value(serde_json::json!({
            "question":"Inspect", "parent_packet_id":"parent",
            "core_generation_id":"core", "retrieval_generation":"retrieval"
        }))
        .unwrap();
        (packet, request)
    }

    #[test]
    fn delivered_continuation_survives_unrelated_parser_gap_without_another_round() {
        use codestory_contracts::api::PacketDispositionKindDto;
        let (packet, request) = delivered_continuation_fixture();
        let source = &packet.support[0];
        let witnesses = vec![PacketHydratedSourceRangeV1 {
            callable_body: None,
            stable_identity: "node:1".into(),
            path: source.path.clone().unwrap(),
            symbol: Some("retained".into()),
            start_line: 2,
            end_line: 2,
            source: source.snippet.clone().unwrap(),
            delivery: source.source_delivery.clone(),
            parser_completeness: PacketParserCompletenessV1::Complete,
        }];
        let classify = |p: &AgentPacketDto| {
            classify_packet_disposition(
                p,
                Some(&request),
                &[],
                &witnesses,
                "project",
                "core".into(),
                Some("retrieval".into()),
                &[],
            )
        };
        let accepted = classify(&packet);
        assert_eq!(accepted.kind, PacketDispositionKindDto::NotEstablished);
        assert!(accepted.drill.is_none());
        assert!(
            accepted
                .omission_receipts
                .iter()
                .any(|gap| gap.contains("incomplete_index") && gap.contains("src/omitted.rs"))
        );
        for mutation in 0..29 {
            let mut bad = packet.clone();
            match mutation {
                0 => bad.support.clear(),
                1 => bad.support[0].source_delivery = None,
                2 => bad.support[0].symbol_id = Some("2".into()),
                3 => bad.support[0].path = Some("src/other.rs".into()),
                4 => bad.support[0].snippet = Some(String::new()),
                5 => {
                    bad.support[0]
                        .source_delivery
                        .as_mut()
                        .unwrap()
                        .source_sha256 = "bad".into()
                }
                6 => {
                    bad.support[0]
                        .source_delivery
                        .as_mut()
                        .unwrap()
                        .requested_range
                        .start_line = 1
                }
                7 => bad.plan.probe_resolutions.clear(),
                8 => {
                    bad.plan.probe_resolutions[0].status = PacketProbeResolutionStatusDto::Rejected
                }
                9 => bad.plan.probe_resolutions[0].symbol_id = Some("2".into()),
                10 => bad.plan.probe_resolutions[0].path = Some("src/other.rs".into()),
                11 => bad.answer.source_coverage[0].reason = None,
                12 => bad.answer.freshness = None,
                22 => {
                    bad.support[0]
                        .source_delivery
                        .as_mut()
                        .unwrap()
                        .source_sha256 = "b".repeat(64)
                }
                23 => {
                    bad.support[0]
                        .source_delivery
                        .as_mut()
                        .unwrap()
                        .next_start_line = Some(4)
                }
                24 => {
                    bad.support[0]
                        .source_delivery
                        .as_mut()
                        .unwrap()
                        .remaining_source_bytes += 1
                }
                25 => {
                    bad.support[0]
                        .source_delivery
                        .as_mut()
                        .unwrap()
                        .blocked_line_rendered_bytes = Some(513)
                }
                26 => bad.support[0].start_line = Some(1),
                27 => bad.support[0].end_line = Some(3),
                28 => bad.support[0].snippet = Some("altered nonempty source".into()),
                index => {
                    let codestory_contracts::api::PacketProbeDto::Continuation {
                        contract_version,
                        project_id,
                        core_generation_id,
                        retrieval_generation,
                        selector,
                    } = &mut bad.plan.probe_resolutions[0].probe
                    else {
                        unreachable!()
                    };
                    match index {
                        13 => *contract_version += 1,
                        14 => *project_id = "other".into(),
                        15 => *core_generation_id = "other".into(),
                        16 => *retrieval_generation = None,
                        17 => selector.stable_identity = "node:2".into(),
                        18 => selector.symbol_id = Some("2".into()),
                        19 => selector.source_range.as_mut().unwrap().end_line = 4,
                        20 => selector.reason = PacketStructuralGapReasonV1::DisconnectedSeed,
                        _ => selector.path = Some("src/other.rs".into()),
                    }
                }
            }
            assert_eq!(
                classify(&bad).kind,
                PacketDispositionKindDto::Unavailable,
                "mutation {mutation}"
            );
        }
        for status in [
            SourceCoverageStatusDto::NotEstablished,
            SourceCoverageStatusDto::PolicyExcluded,
        ] {
            let mut bad = packet.clone();
            bad.answer.source_coverage[0].status = status;
            assert_eq!(classify(&bad).kind, PacketDispositionKindDto::Unavailable);
        }
    }

    #[test]
    fn representation_pruning_keeps_delivered_page_and_restores_unrelated_parser_refusal() {
        use codestory_contracts::api::PacketDispositionKindDto;
        let (packet, request) = delivered_continuation_fixture();
        let source = &packet.support[0];
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project".into(),
                core_generation_id: "core".into(),
                retrieval_generation: Some("retrieval".into()),
            },
            admissions: [1, 2]
                .into_iter()
                .enumerate()
                .map(|(ordinal, id)| PacketAdmissionReceiptV1 {
                    packet_ordinal: ordinal as u32,
                    stable_identity: format!("node:{id}"),
                    score_version:
                        codestory_contracts::compilation::PACKET_RETRIEVAL_SCORE_VERSION_V1.into(),
                    reserved_source_bytes: 512,
                    origin: PacketAdmissionOriginV1::ExactTypedSelector,
                })
                .collect(),
            sources: vec![
                PacketHydratedSourceRangeV1 {
                    callable_body: None,
                    stable_identity: "node:1".into(),
                    path: source.path.clone().unwrap(),
                    symbol: Some("retained".into()),
                    start_line: 2,
                    end_line: 2,
                    source: source.snippet.clone().unwrap(),
                    delivery: source.source_delivery.clone(),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                },
                PacketHydratedSourceRangeV1 {
                    callable_body: None,
                    stable_identity: "node:2".into(),
                    path: "src/omitted.rs".into(),
                    symbol: Some("other".into()),
                    start_line: 1,
                    end_line: 1,
                    source: "fn other() {}".into(),
                    delivery: None,
                    parser_completeness: PacketParserCompletenessV1::Partial,
                },
            ],
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        let prepared = PreparedPacketCompilationV1 {
            publication: input.publication.clone(),
            call_navigation: Vec::new(),
            input,
            source_coverage: packet.answer.source_coverage.clone(),
            source_refusals: Vec::new(),
            published_file_sources: Vec::new(),
            file_navigation_paths: HashMap::new(),
            source_page_alternatives: BTreeMap::new(),
            requested_source_identities: BTreeSet::new(),
            requested_source_minimum_ends: BTreeMap::new(),
            issued_context_sources: None,
        };
        let mut measured = Vec::new();
        let mut accepted = None;
        let frozen = prepared
            .select_for_representation(|prepared, product| {
                let mut candidate = packet.clone();
                prepared.apply_proposal(&mut candidate, Some(&request), product);
                measured.push((product.support.len(), candidate.disposition.clone()));
                let bytes = if product.support.len() > 1 {
                    16385
                } else {
                    accepted = Some(candidate.clone());
                    serde_json::to_vec(&candidate).unwrap().len()
                };
                Ok(Some(bytes))
            })
            .unwrap();
        let accepted = accepted.unwrap();
        assert!(measured.len() > 1);
        assert_eq!(frozen.product.support.len(), 1);
        assert_eq!(accepted.support[0].path.as_deref(), Some("src/retained.rs"));
        assert_eq!(
            accepted.disposition.kind,
            PacketDispositionKindDto::NotEstablished
        );
        assert!(accepted.disposition.drill.is_none());
        assert!(
            accepted
                .disposition
                .omission_receipts
                .iter()
                .any(|gap| gap.contains("src/omitted.rs"))
        );
        for disposition in [&measured[0].1, &accepted.disposition] {
            assert_eq!(
                disposition.omission_receipts.iter().filter(|gap|
                    gap.contains("src/omitted.rs") && gap.contains("incomplete_index")
                ).count(),
                1,
                "observed parser coverage remains explicit with its page retained or pruned"
            );
        }
        commit_measured_packet_compilation(&accepted, frozen).unwrap();
    }

    #[test]
    fn joint_verified_extents_restore_baseline_before_partial_body_addition() {
        use crate::evidence_projection_v3::representation_budget_test_support as fixture;
        use codestory_contracts::compilation::PacketCallableBodyV1;
        use codestory_contracts::packet_projection_v3::PacketProjectionV3Dto;
        let mut input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project-1".into(),
                core_generation_id: "core-generation-1".into(),
                retrieval_generation: Some("retrieval-generation-1".into()),
            },
            admissions: Vec::new(),
            sources: Vec::new(),
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        let mut short = Vec::new();
        for id in 0..3 {
            let text = if id == 0 {
                format!(
                    "fn anonymous() {{\n{}\n}}\n",
                    format!("    execute({});\n", "x".repeat(95)).repeat(60)
                )
            } else {
                format!("{}\n", "x".repeat(150)).repeat(8)
            };
            let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
            let end = text.lines().count() as u32;
            let range = PacketSourceRangeV1 {
                start_line: 1,
                end_line: end,
            };
            input.admissions.push(PacketAdmissionReceiptV1 {
                packet_ordinal: id,
                stable_identity: format!("node:{id}"),
                score_version: "anonymous".into(),
                reserved_source_bytes: 4096,
                origin: PacketAdmissionOriginV1::Retrieval,
            });
            for (cap, pages) in [(4096, &mut input.sources), (512, &mut short)] {
                let (source, end_line, delivery) =
                    complete_node_page(&text, range, cap, &hash).unwrap();
                pages.push(PacketHydratedSourceRangeV1 {
                    stable_identity: format!("node:{id}"),
                    path: format!("source_{id}.rs"),
                    symbol: None,
                    start_line: 1,
                    end_line,
                    source,
                    delivery: Some(delivery),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                    callable_body: (id == 0).then(|| PacketCallableBodyV1 {
                        unit_range: range,
                        body_range: PacketSourceRangeV1 {
                            start_line: 1,
                            end_line: end,
                        },
                        source_sha256: hash.clone(),
                    }),
                });
            }
        }
        input.relations.push(PacketDirectedRelationV1 {
            relation_id: "anonymous-edge".into(),
            from_identity: "node:0".into(),
            to_identity: "node:1".into(),
            relation_kind: PacketRelationKindV1::Call,
            certainty: PacketRelationCertaintyV1::Certain,
        });
        let request: AgentPacketRequestDto =
            serde_json::from_value(serde_json::json!({"question":"Read repository evidence"}))
                .unwrap();
        let (base, _) = fixture::default_packet(input.clone(), &request);
        let wire = |prepared: &PreparedPacketCompilationV1,
                    product: &RepositoryDerivedCompilationV1| {
            let mut candidate = base.clone();
            prepared.apply_proposal(&mut candidate, Some(&request), product);
            let projection = fixture::project_packet(&request, &candidate, |_| Ok(0))
                .unwrap()
                .projection;
            assert!(matches!(projection, PacketProjectionV3Dto::Complete { .. }));
            serde_json::to_vec(&projection).unwrap().len()
        };
        let mut baseline_input = input.clone();
        baseline_input.sources.remove(0);
        let baseline = compile_repository_evidence(&baseline_input);
        assert_eq!(
            baseline
                .support
                .iter()
                .filter(|row| row.kind == SupportUnitKindDto::SourceRange)
                .count(),
            2
        );
        assert!(
            baseline
                .support
                .iter()
                .any(|row| row.kind == SupportUnitKindDto::TypedGraphEdge)
        );
        let baseline_rows = baseline
            .support
            .iter()
            .filter(|r| r.kind == SupportUnitKindDto::SourceRange)
            .cloned()
            .collect::<Vec<_>>();
        let mut mixed = input.clone();
        mixed.sources[0] = short[0].clone();
        let mixed_prepared = PreparedPacketCompilationV1::from_fixture_input(mixed);
        let selection = compile_repository_evidence_baseline_for_representation(
            &mixed_prepared.input,
            &[],
            |product| {
                let only_sources = product.support.len() == 3
                    && product
                        .support
                        .iter()
                        .all(|row| row.kind == SupportUnitKindDto::SourceRange);
                Ok::<_, ()>(only_sources.then(|| wire(&mixed_prepared, product)))
            },
        )
        .unwrap();
        let RepresentationBudgetSelectionV1::Complete {
            product: mixed_product,
            ..
        } = selection
        else {
            panic!("actual three-source joint frame must be available");
        };
        let mixed_bytes = wire(&mixed_prepared, &mixed_product);
        let full_prepared = PreparedPacketCompilationV1::from_fixture_input(input.clone());
        assert!(wire(&full_prepared, &full_prepared.default_product()) > mixed_bytes + 32);
        let padding = PUBLIC_PACKET_SERIALIZED_MAX_BYTES
            .checked_sub(mixed_bytes + 32)
            .unwrap();
        for mode in 0..4 {
            let mut prepared = full_prepared.clone();
            let mut pages = short.clone();
            if mode == 3 {
                pages[0].delivery.as_mut().unwrap().source_sha256 = "b".repeat(64);
            }
            prepared.source_page_alternatives.insert(512, pages);
            let mut callbacks = 0;
            let mut fitting_mixed_callbacks = 0;
            let result = prepared.select_for_representation(|prepared, product| {
                callbacks += 1;
                let bytes = wire(prepared, product) + padding + if mode == 1 { 512 } else { 0 };
                let mixed = product.support.iter().any(|row| {
                    row.kind == SupportUnitKindDto::SourceRange
                        && row.symbol_id.as_deref() == Some("0")
                        && row.end_line == Some(short[0].end_line)
                }) && baseline_rows
                    .iter()
                    .all(|row| product.support.contains(row));
                if bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES && mixed {
                    fitting_mixed_callbacks += 1;
                }
                if mode == 2 && fitting_mixed_callbacks == 3 {
                    return Ok(None);
                }
                Ok((bytes <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES).then_some(bytes))
            });
            if mode == 2 {
                assert_eq!(fitting_mixed_callbacks, 3);
                assert!(
                    result
                        .err()
                        .unwrap()
                        .message
                        .contains("final served-frame validation")
                );
                continue;
            }
            let frozen = result.unwrap();
            for old in &baseline_rows {
                assert!(
                    frozen.product.support.contains(old),
                    "complete earlier sources cannot be traded for short alternatives"
                );
            }
            if mode != 0 {
                assert!(
                    !frozen
                        .product
                        .support
                        .iter()
                        .any(|row| row.kind == SupportUnitKindDto::SourceRange
                            && row.symbol_id.as_deref() == Some("0")),
                    "unfitting or hash-conflicted alternative earns no body addition"
                );
                continue;
            }
            let added = frozen
                .product
                .support
                .iter()
                .find(|r| {
                    r.kind == SupportUnitKindDto::SourceRange && r.symbol_id.as_deref() == Some("0")
                })
                .expect("verified short body must compete beside the unchanged baseline");
            assert_eq!(added.end_line, Some(short[0].end_line));
            assert_eq!(added.snippet.as_deref(), Some(short[0].source.as_str()));
            assert_eq!(added.source_delivery, short[0].delivery);
            assert!(
                added
                    .source_delivery
                    .as_ref()
                    .unwrap()
                    .next_start_line
                    .is_some()
            );
            assert!(frozen.product.continuation.iter().any(|c| {
                c.stable_identity == "node:0"
                    && c.source_range
                        .is_some_and(|r| r.start_line == short[0].end_line + 1)
            }));
            assert!(callbacks <= 8 * codestory_agent::evidence_compiler::REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1 + 7 * codestory_agent::evidence_compiler::REPOSITORY_EVIDENCE_JOINT_CALLBACKS_MAX + 1);
        }
    }

    #[test]
    fn representation_extents_keep_shared_rank_and_original_progress_denominator() {
        let mut input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "anonymous".into(),
                core_generation_id: "core".into(),
                retrieval_generation: None,
            },
            admissions: Vec::new(),
            sources: Vec::new(),
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        for i in 0..6 {
            input.admissions.push(PacketAdmissionReceiptV1 {
                packet_ordinal: i,
                stable_identity: format!("node:{i}"),
                score_version: "anonymous".into(),
                reserved_source_bytes: 512,
                origin: PacketAdmissionOriginV1::Retrieval,
            });
            let end_line = if i >= 4 { 20 } else { 1 };
            input.sources.push(PacketHydratedSourceRangeV1 {
                callable_body: None,
                stable_identity: format!("node:{i}"),
                path: format!("src/{i}"),
                symbol: None,
                start_line: 1,
                end_line,
                source: if i >= 4 {
                    "xxxxxxxxxxxxxx\n".repeat(20)
                } else {
                    "x".into()
                },
                delivery: Some(PacketSourceDeliveryV1 {
                    requested_range: PacketSourceRangeV1 {
                        start_line: 1,
                        end_line,
                    },
                    source_sha256: "a".repeat(64),
                    remaining_source_bytes: 0,
                    next_start_line: None,
                    blocked_line_rendered_bytes: None,
                }),
                parser_completeness: PacketParserCompletenessV1::Complete,
            });
        }
        let mut short = input.sources.clone();
        for source in short.iter_mut().skip(4) {
            source.end_line = 1;
            source.source = "xxxxxxxxxxxxxx\n".into();
            let delivery = source.delivery.as_mut().unwrap();
            delivery.remaining_source_bytes = 285;
            delivery.next_start_line = Some(2);
            assert_eq!(delivery.requested_range.end_line, 20);
        }
        let mut prepared = PreparedPacketCompilationV1::from_fixture_input(input);
        prepared.source_page_alternatives.insert(512, short);
        prepared.call_navigation = [(0, 4), (1, 4), (2, 5)]
            .map(|(from, to)| PacketDirectedRelationV1 {
                relation_id: format!("{from}-{to}"),
                from_identity: format!("node:{from}"),
                to_identity: format!("node:{to}"),
                relation_kind: PacketRelationKindV1::Call,
                certainty: PacketRelationCertaintyV1::Uncertain,
            })
            .into();
        let frozen = prepared
            .select_for_representation(|_, product| {
                let bytes = 50
                    + 200 * product.support.len()
                    + product
                        .support
                        .iter()
                        .map(|row| row.snippet.as_ref().map_or(0, String::len))
                        .sum::<usize>();
                Ok((bytes <= 1100).then_some(bytes))
            })
            .unwrap();
        assert_eq!(
            frozen
                .product
                .support
                .iter()
                .map(|row| row.symbol_id.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("4"), Some("5")]
        );
        assert!(
            frozen
                .product
                .support
                .iter()
                .all(|row| row.end_line == Some(20)
                    && row.source_delivery.as_ref().unwrap().remaining_source_bytes == 0)
        );
    }

    #[test]
    fn representation_extents_preserve_each_requested_identity_smallest_valid_page() {
        let text_a = format!("{}\r\n{}", "a".repeat(1300), "é".repeat(100) + "\r\n").repeat(1)
            + &("x".repeat(200) + "\r\n").repeat(20);
        let text_b = ("b".repeat(100) + "\r\n").repeat(40);
        let texts = [text_a, text_b];
        let mut originals = Vec::new();
        let mut alternatives = BTreeMap::<usize, Vec<PacketHydratedSourceRangeV1>>::new();
        for (index, text) in texts.iter().enumerate() {
            let requested = PacketSourceRangeV1 {
                start_line: 1,
                end_line: text.lines().count() as u32,
            };
            let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
            let make = |cap| {
                complete_node_page(text, requested, cap, &hash).map(|(body, end, delivery)| {
                    PacketHydratedSourceRangeV1 {
                        callable_body: None,
                        stable_identity: format!("node:{}", index + 1),
                        path: format!("src/page_{index}.rs"),
                        symbol: None,
                        start_line: 1,
                        end_line: end,
                        source: body,
                        delivery: Some(delivery),
                        parser_completeness: PacketParserCompletenessV1::Complete,
                    }
                })
            };
            originals.push(make(4096).unwrap());
            for cap in [2048, 1024, 512] {
                if let Ok(page) = make(cap) {
                    alternatives.entry(cap).or_default().push(page);
                }
            }
        }
        assert_eq!(
            alternatives[&1024].len(),
            1,
            "the long first complete line cannot fit smaller caps"
        );
        let expected_a = alternatives[&2048][0].clone();
        let expected_b = alternatives[&512][0].clone();
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project".into(),
                core_generation_id: "core".into(),
                retrieval_generation: None,
            },
            admissions: originals
                .iter()
                .enumerate()
                .map(|(index, source)| PacketAdmissionReceiptV1 {
                    packet_ordinal: index as u32,
                    stable_identity: source.stable_identity.clone(),
                    score_version: "test".into(),
                    reserved_source_bytes: 4096,
                    origin: PacketAdmissionOriginV1::ExactTypedSelector,
                })
                .collect(),
            sources: originals,
            relations: Vec::new(),
            admission_gaps: Vec::new(),
            ambiguities: Vec::new(),
        };
        let mut prepared = PreparedPacketCompilationV1::from_fixture_input(input);
        prepared.source_page_alternatives = alternatives;
        prepared.requested_source_identities = BTreeSet::from(["node:1".into(), "node:2".into()]);
        let no_fit = prepared.clone();
        let mut previous_ends = (u32::MAX, u32::MAX);
        let mut accepted = None;
        let frozen = prepared
            .select_for_representation(|prepared, product| {
                let a = product
                    .support
                    .iter()
                    .find(|unit| {
                        unit.kind == SupportUnitKindDto::SourceRange
                            && unit.symbol_id.as_deref() == Some("1")
                    })
                    .unwrap();
                let b = product
                    .support
                    .iter()
                    .find(|unit| {
                        unit.kind == SupportUnitKindDto::SourceRange
                            && unit.symbol_id.as_deref() == Some("2")
                    })
                    .unwrap();
                assert!(
                    a.end_line.unwrap() <= previous_ends.0
                        && b.end_line.unwrap() <= previous_ends.1,
                    "an identity never regrows when a smaller cap lacks a valid first line"
                );
                previous_ends = (a.end_line.unwrap(), b.end_line.unwrap());
                let fits = a.snippet.as_ref().unwrap().len() <= 2048
                    && b.snippet.as_ref().unwrap().len() <= 512;
                if fits {
                    accepted = Some((product.clone(), prepared.input.sources.clone()));
                }
                Ok(Some(if fits { 16000 } else { 16385 }))
            })
            .unwrap();
        let (product, measured_sources) = accepted.unwrap();
        assert_eq!(frozen.product, product);
        assert_eq!(frozen.hydrated_sources, measured_sources);
        assert_eq!(frozen.hydrated_sources, vec![expected_a, expected_b]);
        let frozen_refusal = no_fit
            .select_for_representation(|prepared, product| {
                if product.support.is_empty() {
                    assert_eq!(prepared.source_refusals.len(), 2);
                    assert!(prepared.source_refusals.iter().all(|message| {
                        message.contains("no authenticated requested complete-line page fit")
                    }));
                }
                Ok(Some(if product.support.is_empty() {
                    100
                } else {
                    16385
                }))
            })
            .unwrap();
        assert!(frozen_refusal.product.support.is_empty());
        assert_eq!(frozen_refusal.source_refusals.len(), 2);
    }

    #[test]
    fn continuation_budget_counts_distinct_actions_in_compiler_order() {
        use codestory_contracts::api::PacketDispositionKindDto;
        use serde_json::json;

        let trace = json!({"request_id":"continuation-request", "resolved_profile":"investigate",
            "policy_mode":"latency_first", "total_latency_ms":0, "sla_missed":false,
            "semantic_fallback_count":0, "semantic_fallbacks":[], "semantic_stage_timeout_zero_hits":0,
            "semantic_abstained_count":0, "annotations":[], "steps":[], "packet_sidecar_diagnostics":[]});
        let packet: AgentPacketDto = serde_json::from_value(json!({
            "packet_id":"continuation-request", "question":"repository investigation",
            "plan":{"queries":[],"trace":[]},
            "answer":{"answer_id":"continuation-answer", "prompt":"repository investigation",
                "summary":"fixture", "sections":[], "citations":[], "subgraph_ids":[],
                "retrieval_version":"fixture", "graphs":[], "retrieval_trace":trace,
                "freshness":{"status":"fresh", "changed_file_count":0, "new_file_count":0,
                    "removed_file_count":0, "checked_file_count":0, "indexed_file_count":0,
                    "duration_ms":0}},
            "support":[], "disposition":{"kind":"not_established"},
            "budget":{"requested":"standard", "limits":{"max_anchors":16,"max_files":16,
                "max_snippets":16,"max_trail_edges":16,"max_output_bytes":65536},
                "used":{"anchors":0,"files":0,"snippets":0,"trail_edges":0,"output_bytes":0},
                "truncated":false,"next_deeper_command":null},
            "retrieval_trace_summary":{"retrieval_trace":trace,"source_read_steps":0,
                "search_steps":0,"trail_steps":0}
        }))
        .expect("fresh internal packet envelope");

        // An ambiguous exact selector can lack descriptor allocation bounds
        // before admission. Both recoverable structural reasons map to the
        // same executable symbol-read action.
        let candidate_ids = [9, 2, 10, 8, 3, 7, 4, 6, 5];
        for duplicate_reasons in [false, true] {
            let hydrated_ids = candidate_ids.iter().skip(usize::from(duplicate_reasons));
            let input = PacketCompilationInputV1 {
                contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
                publication: PacketCompilationPublicationV1 {
                    project_id: "project".into(),
                    core_generation_id: "core".into(),
                    retrieval_generation: Some("retrieval".into()),
                },
                admissions: hydrated_ids
                    .clone()
                    .enumerate()
                    .map(|(ordinal, id)| PacketAdmissionReceiptV1 {
                        packet_ordinal: ordinal as u32,
                        stable_identity: format!("node:{id}"),
                        score_version:
                            codestory_contracts::compilation::PACKET_RETRIEVAL_SCORE_VERSION_V1
                                .into(),
                        reserved_source_bytes: INTERIM_SOURCE_ROW_UPPER_BOUND as u32,
                        origin: PacketAdmissionOriginV1::ExactTypedSelector,
                    })
                    .collect(),
                sources: hydrated_ids
                    .map(|id| PacketHydratedSourceRangeV1 {
                        callable_body: None,
                        stable_identity: format!("node:{id}"),
                        path: format!("src/symbol_{id}.rs"),
                        symbol: Some("shared_symbol".into()),
                        start_line: 1,
                        end_line: 1,
                        source: "fn shared_symbol() {}".into(),
                        delivery: None,
                        parser_completeness: PacketParserCompletenessV1::Complete,
                    })
                    .collect(),
                relations: Vec::new(),
                admission_gaps: if duplicate_reasons {
                    vec![PacketAdmissionGapV1 {
                        kind: PacketAdmissionGapKindV1::SourceBoundMissing,
                        stable_identity: Some("node:9".into()),
                        exact_selector_ordinal: Some(0),
                    }]
                } else {
                    Vec::new()
                },
                ambiguities: vec![PacketIdentityAmbiguityV1 {
                    selector: "shared_symbol".into(),
                    candidate_identities: candidate_ids
                        .iter()
                        .map(|id| format!("node:{id}"))
                        .collect(),
                }],
            };
            let compiled = compile_repository_evidence(&input);
            assert_eq!(
                compiled.continuation.len(),
                if duplicate_reasons { 10 } else { 9 }
            );
            let mut packet = packet.clone();
            packet.support = compiled.support.clone();
            let disposition = classify_packet_disposition(
                &packet,
                None,
                &compiled.continuation,
                &[],
                "project",
                "core".into(),
                Some("retrieval".into()),
                &[],
            );
            let drill = disposition
                .drill
                .expect("first packet offers one continuation");
            assert_eq!(drill.options.len(), PACKET_DRILL_MAX_OPTIONS);
            assert_eq!(
                drill
                    .options
                    .iter()
                    .map(|option| option.symbol_id.clone())
                    .collect::<Vec<_>>(),
                candidate_ids[..8]
                    .iter()
                    .map(|id| Some(id.to_string()))
                    .collect::<Vec<_>>(),
                "duplicate reasons must not displace the eighth distinct action",
            );
            assert_eq!(
                drill
                    .options
                    .iter()
                    .map(|option| &option.id)
                    .collect::<BTreeSet<_>>()
                    .len(),
                8
            );
            assert_eq!(
                drill.options[0].structural_reason,
                Some(if duplicate_reasons {
                    PacketStructuralGapReasonV1::SourceUnavailable
                } else {
                    PacketStructuralGapReasonV1::AmbiguousSelector
                })
            );
            assert_eq!(
                drill.gap_ids,
                drill
                    .options
                    .iter()
                    .map(|option| option.gap_id.clone())
                    .collect::<Vec<_>>()
            );
            assert_eq!(drill.core_generation_id, "core");
            assert_eq!(drill.retrieval_generation.as_deref(), Some("retrieval"));
            assert_eq!(drill.remaining_rounds, 1);

            for request_json in [
                json!({"question":"repository investigation", "parent_packet_id":packet.packet_id}),
                json!({"question":"repository investigation", "option_ids":[drill.options[0].id]}),
                json!({"question":"repository investigation", "core_generation_id":"other-core"}),
                json!({"question":"repository investigation", "retrieval_generation":"other-retrieval"}),
            ] {
                let request: AgentPacketRequestDto = serde_json::from_value(request_json).unwrap();
                let terminal = classify_packet_disposition(
                    &packet,
                    Some(&request),
                    &compiled.continuation,
                    &[],
                    "project",
                    "core".into(),
                    Some("retrieval".into()),
                    &[],
                );
                assert!(terminal.drill.is_none());
                if request.parent_packet_id.is_some()
                    || !request.option_ids.is_empty()
                    || request.probes.iter().any(|probe| {
                        matches!(
                            probe,
                            codestory_contracts::api::PacketProbeDto::Continuation { .. }
                        )
                    })
                {
                    assert_eq!(
                        terminal.reason.as_deref(),
                        Some("the bounded continuation left a structural gap")
                    );
                }
                assert_eq!(
                    terminal.kind,
                    if request.parent_packet_id.is_some()
                        || !request.option_ids.is_empty()
                        || request.probes.iter().any(|probe| matches!(
                            probe,
                            codestory_contracts::api::PacketProbeDto::Continuation { .. }
                        ))
                    {
                        PacketDispositionKindDto::NotEstablished
                    } else {
                        PacketDispositionKindDto::Unavailable
                    }
                );
            }
        }
    }

    #[test]
    fn frozen_public_packet_keeps_only_admitted_sources_and_induced_relations() {
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        use crate::agent::packet_candidate::PacketAdmissionDecision;
        use codestory_contracts::packet_projection_v3::{EvidenceKindV3Dto, PacketProjectionV3Dto};
        use codestory_store::{IndexPublicationMode, IndexPublicationRecord};
        use serde_json::json;
        use std::sync::{Arc, atomic::AtomicBool};

        let project = tempfile::tempdir().expect("isolated source project");
        let cache = tempfile::tempdir().expect("isolated Store");
        let database = cache.path().join("core.sqlite3");
        let mut storage = Store::open(&database).expect("real file-backed Store");
        // Source bodies and bounds are stored independently of admission. The
        // unrelated rows must remain present throughout the test.
        let files = [
            (10, "src/alpha.rs", "fn alpha() {}\n"),
            (20, "tools/repos/worker/src/beta.rs", "fn beta() {}\n"),
            (30, "tools/runner/src/alpha.rs", "fn gamma() {}\n"),
            (
                40,
                "tools/runner/src/reference.rs",
                "const REFERENCE: usize = 1;\n",
            ),
            (90, "noise/outside.rs", "fn NOISE_OUTSIDE() {}\n"),
            (190, "noise/unrelated.rs", "fn NOISE_UNRELATED() {}\n"),
        ];
        for (id, relative, source) in files {
            let path = project.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).expect("source directory");
            std::fs::write(&path, source).expect("real source bytes");
            let file = FileInfo {
                id,
                path,
                language: "rust".into(),
                modification_time: 0,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            };
            storage.insert_file(&file).expect("file row");
            storage
                .update_file_metadata(
                    &file,
                    Some(&format!("{:x}", Sha256::digest(source.as_bytes()))),
                )
                .expect("pinned source digest");
            storage
                .insert_nodes_batch(&[CoreNode {
                    id: CoreNodeId(id),
                    kind: CoreNodeKind::FILE,
                    serialized_name: relative.into(),
                    file_node_id: Some(CoreNodeId(id)),
                    start_line: Some(1),
                    end_line: Some(1),
                    ..Default::default()
                }])
                .expect("file node");
        }
        for (id, file_id, name) in [
            (11, 10, "alpha"),
            (21, 20, "beta"),
            (31, 30, "gamma"),
            (99, 90, "NOISE_OUTSIDE"),
            (199, 190, "NOISE_UNRELATED"),
        ] {
            storage
                .insert_nodes_batch(&[CoreNode {
                    id: CoreNodeId(id),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: name.into(),
                    file_node_id: Some(CoreNodeId(file_id)),
                    start_line: Some(1),
                    end_line: Some(1),
                    ..Default::default()
                }])
                .expect("stored symbol");
        }
        let kinds = [
            (CoreEdgeKind::MEMBER, PacketRelationKindV1::Member),
            (CoreEdgeKind::TYPE_USAGE, PacketRelationKindV1::TypeUsage),
            (CoreEdgeKind::USAGE, PacketRelationKindV1::Usage),
            (CoreEdgeKind::CALL, PacketRelationKindV1::Call),
            (CoreEdgeKind::INHERITANCE, PacketRelationKindV1::Inheritance),
            (CoreEdgeKind::OVERRIDE, PacketRelationKindV1::Override),
            (
                CoreEdgeKind::TYPE_ARGUMENT,
                PacketRelationKindV1::TypeArgument,
            ),
            (
                CoreEdgeKind::TEMPLATE_SPECIALIZATION,
                PacketRelationKindV1::TemplateSpecialization,
            ),
            (CoreEdgeKind::INCLUDE, PacketRelationKindV1::Include),
            (CoreEdgeKind::IMPORT, PacketRelationKindV1::Import),
            (CoreEdgeKind::MACRO_USAGE, PacketRelationKindV1::MacroUsage),
            (
                CoreEdgeKind::ANNOTATION_USAGE,
                PacketRelationKindV1::AnnotationUsage,
            ),
            (CoreEdgeKind::UNKNOWN, PacketRelationKindV1::Unknown),
        ];
        let mut expected_relations = Vec::new();
        for (base, from, to) in [(100, 11, 21), (200, 21, 31)] {
            for (offset, (kind, expected_kind)) in kinds.into_iter().enumerate() {
                let id = base + offset as i64;
                for representative in [id, id + 1_000] {
                    storage
                        .insert_edge(&CoreEdge {
                            id: CoreEdgeId(representative),
                            source: CoreNodeId(from),
                            target: CoreNodeId(to),
                            kind,
                            certainty: Some(ResolutionCertainty::Certain),
                            ..Default::default()
                        })
                        .expect("certain kind/pair and duplicate");
                }
                expected_relations.push((
                    id.to_string(),
                    format!("node:{from}"),
                    format!("node:{to}"),
                    expected_kind,
                ));
            }
        }
        // Both directions of a one-admitted-endpoint edge, a wholly unrelated
        // pair, and an earlier probable duplicate cannot become induced evidence.
        for (id, from, to, certainty) in [
            (1, 11, 99, ResolutionCertainty::Certain),
            (2, 199, 21, ResolutionCertainty::Certain),
            (3, 99, 199, ResolutionCertainty::Certain),
            (4, 11, 21, ResolutionCertainty::Probable),
        ] {
            storage
                .insert_edge(&CoreEdge {
                    id: CoreEdgeId(id),
                    source: CoreNodeId(from),
                    target: CoreNodeId(to),
                    kind: CoreEdgeKind::MEMBER,
                    certainty: Some(certainty),
                    ..Default::default()
                })
                .expect("hostile stored edge");
        }
        let publication = IndexPublicationRecord {
            generation: 1,
            generation_id: "h7-core-generation".into(),
            run_id: "h7-core-run".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        storage
            .put_index_publication(&publication)
            .expect("complete fixture publication");
        assert_eq!(storage.get_nodes().unwrap().len(), 11);
        assert_eq!(storage.get_edges().unwrap().len(), 56);
        drop(storage);

        let controller = AppController::new_with_owned_cache_root(process_cache.path());
        {
            let mut state = controller.state.lock();
            state.project_root = Some(project.path().to_path_buf());
            state.storage_path = Some(database);
        }
        let service = crate::services::PublicOperationService::new(controller.clone());
        let session = PacketProofSession::new();
        let admitted = [
            "node:11",
            "node:21",
            "node:31",
            "path:tools/runner/src/reference.rs",
        ];
        for identity in admitted {
            assert_eq!(
                session.admit(identity, INTERIM_SOURCE_ROW_UPPER_BOUND),
                PacketAdmissionDecision::Admitted
            );
        }
        let request: AgentPacketRequestDto =
            serde_json::from_value(json!({"question":"stored evidence fixture"})).unwrap();
        let trace = json!({"request_id":"h7-request", "resolved_profile":"investigate",
            "policy_mode":"latency_first", "total_latency_ms":0, "sla_missed":false,
            "semantic_fallback_count":0, "semantic_fallbacks":[], "semantic_stage_timeout_zero_hits":0,
            "semantic_abstained_count":0, "annotations":[], "steps":[], "packet_sidecar_diagnostics":[]});
        // Only the input packet envelope is a fixture. Evidence comes from the
        // real runtime hydration/compiler and real v3 projection below.
        let mut packet: AgentPacketDto = serde_json::from_value(json!({
            "packet_id":"h7-request", "question":request.question, "plan":{"queries":[],"trace":[]},
            "answer":{"answer_id":"h7-answer", "prompt":request.question, "summary":"fixture",
                "sections":[], "citations":[], "subgraph_ids":[], "retrieval_version":"fixture",
                "graphs":[], "retrieval_trace":trace}, "support":[],
            "disposition":{"kind":"not_established"},
            "budget":{"requested":"standard", "limits":{"max_anchors":16,"max_files":16,
                "max_snippets":16,"max_trail_edges":16,"max_output_bytes":65536},
                "used":{"anchors":0,"files":0,"snippets":0,"trail_edges":0,"output_bytes":0},
                "truncated":false, "next_deeper_command":null},
            "retrieval_trace_summary":{"retrieval_trace":trace,"source_read_steps":0,"search_steps":0,"trail_steps":0}
        })).expect("typed internal packet envelope");
        // Missing freshness remains unavailable. The hydration fixture then
        // opts into the existing synthetic freshness control so that its
        // source refusal assertions exercise hydration rather than that cap.
        let unknown = classify_packet_disposition(
            &packet,
            Some(&request),
            &[],
            &[],
            "project",
            publication.generation_id.clone(),
            None,
            &[],
        );
        assert_eq!(
            unknown.kind,
            codestory_contracts::api::PacketDispositionKindDto::Unavailable
        );
        assert!(
            unknown
                .reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("freshness unknown"))
        );
        packet.answer.freshness =
            Some(codestory_agent::packet_freshness::fresh_index_observation());
        let projected = service
            .run_observational_with_cancel(
                "h7-packet-compilation",
                Arc::new(AtomicBool::new(false)),
                || {
                    assert_eq!(controller.active_core_publication().unwrap(), publication);
                    let pinned = controller.open_storage_read_only()?;
                    let mut gaps = Vec::new();
                    let (authenticated, sources, _) = hydrate_admitted_sources(
                        &controller,
                        &pinned,
                        &session.receipts(),
                        &mut gaps,
                    )?;
                    assert!(gaps.is_empty());
                    assert_eq!(
                        authenticated
                            .iter()
                            .map(|item| item.receipt.stable_identity.as_str())
                            .collect::<BTreeSet<_>>(),
                        BTreeSet::from(admitted)
                    );
                    assert_eq!(
                        sources
                            .iter()
                            .map(|source| source.stable_identity.as_str())
                            .collect::<BTreeSet<_>>(),
                        BTreeSet::from(admitted)
                    );
                    let induced = hydrate_induced_relations(&pinned, &authenticated)?;
                    assert!(
                        induced
                            .iter()
                            .all(|edge| edge.certainty == PacketRelationCertaintyV1::Certain)
                    );
                    let mut observed = induced
                        .iter()
                        .map(|edge| {
                            (
                                edge.relation_id.clone(),
                                edge.from_identity.clone(),
                                edge.to_identity.clone(),
                                edge.relation_kind,
                            )
                        })
                        .collect::<Vec<_>>();
                    observed.sort();
                    expected_relations.sort();
                    assert_eq!(observed, expected_relations);
                    assert_eq!(
                        induced.len(),
                        26,
                        "all kinds for both admitted directed pairs before forest selection"
                    );
                    let frozen = freeze_packet_compilation(
                        &controller,
                        "h7-project",
                        &[],
                        None,
                        &session,
                        None,
                    )?;
                    assert_eq!(
                        frozen
                            .source_coverage
                            .iter()
                            .map(|row| row.path.as_str())
                            .collect::<BTreeSet<_>>(),
                        BTreeSet::from([
                            "src/alpha.rs",
                            "tools/repos/worker/src/beta.rs",
                            "tools/runner/src/alpha.rs",
                            "tools/runner/src/reference.rs"
                        ])
                    );
                    packet.answer.source_coverage = frozen.source_coverage.clone();
                    apply_frozen_packet_compilation(&mut packet, Some(&request), frozen);
                    assert_eq!(
                        packet.support.len(),
                        6,
                        "four real source rows and two forest relations fit caps"
                    );
                    assert_eq!(
                        final_support_identities_for_observation(&packet.support)
                            .into_iter()
                            .collect::<BTreeSet<_>>(),
                        admitted.into_iter().map(str::to_owned).collect()
                    );
                    let source_units = packet
                        .support
                        .iter()
                        .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
                        .collect::<Vec<_>>();
                    assert_eq!(
                        source_units
                            .iter()
                            .map(|unit| (unit.symbol_id.as_deref(), unit.path.as_deref()))
                            .collect::<BTreeSet<_>>(),
                        BTreeSet::from([
                            (Some("11"), Some("src/alpha.rs")),
                            (Some("21"), Some("tools/repos/worker/src/beta.rs")),
                            (Some("31"), Some("tools/runner/src/alpha.rs")),
                            (None, Some("tools/runner/src/reference.rs"))
                        ])
                    );
                    assert!(
                        source_units
                            .iter()
                            .all(|unit| unit.start_line == Some(1) && unit.end_line == Some(1))
                    );
                    for (path, body) in [
                        ("src/alpha.rs", "fn alpha() {}"),
                        ("tools/repos/worker/src/beta.rs", "fn beta() {}"),
                        ("tools/runner/src/alpha.rs", "fn gamma() {}"),
                        ("tools/runner/src/reference.rs", "const REFERENCE: usize = 1;"),
                    ] {
                        let source = source_units
                            .iter()
                            .find(|unit| unit.path.as_deref() == Some(path))
                            .expect("expected admitted source row");
                        assert!(
                            source
                                .snippet
                                .as_ref()
                                .is_some_and(|text| text.contains(body)),
                            "source body must match its admitted path: {path}"
                        );
                    }
                    assert_eq!(
                        packet
                            .support
                            .iter()
                            .filter(|unit| unit.kind == SupportUnitKindDto::TypedGraphEdge)
                            .map(|unit| (
                                unit.id.as_str(),
                                unit.from_symbol.as_deref(),
                                unit.to_symbol.as_deref(),
                                unit.edge_kind.as_deref()
                            ))
                            .collect::<BTreeSet<_>>(),
                        BTreeSet::from([
                            ("edge:100", Some("11"), Some("21"), Some("member")),
                            ("edge:200", Some("21"), Some("31"), Some("member"))
                        ])
                    );
                    let positive = crate::evidence_projection_v3::project_packet_v3(
                        &service,
                        "h7-caller",
                        &request,
                        &packet,
                        |projection| {
                            serde_json::to_vec(projection)
                                .map(|bytes| bytes.len())
                                .map_err(|_| ())
                        },
                    )?;
                    let path = project.path().join("tools/repos/worker/src/beta.rs");
                    let original = std::fs::read(&path).unwrap();
                    let check_refusal = |label: &str| -> Result<(), codestory_contracts::api::ApiError> {
                        let session = PacketProofSession::new();
                        session.admit("node:21", INTERIM_SOURCE_ROW_UPPER_BOUND);
                        session.admit("path:tools/repos/worker/src/beta.rs", INTERIM_SOURCE_ROW_UPPER_BOUND);
                        let frozen = freeze_packet_compilation(&controller, "h7-project", &[], None, &session, Some(&request))?;
                        assert!(frozen.product.support.is_empty(), "{label}");
                        assert!(frozen.product.continuation.is_empty(), "{label}");
                        assert_eq!(frozen.source_refusals.len(), 2, "{label}");
                        let mut refused = packet.clone();
                        refused.packet_id = format!("h7-refusal-{label}");
                        refused.answer.source_coverage = frozen.source_coverage.clone();
                        apply_frozen_packet_compilation(&mut refused, Some(&request), frozen);
                        assert_eq!(refused.disposition.kind, codestory_contracts::api::PacketDispositionKindDto::NotEstablished, "{label}");
                        let public = crate::evidence_projection_v3::project_packet_v3(&service, "h7-refusal-caller", &request, &refused, |projection| Ok(serde_json::to_vec(projection).unwrap().len()))?;
                        let PacketProjectionV3Dto::Complete { evidence, gaps, continuation, answer_sufficiency, .. } = public.projection else { panic!("bounded refusal fits: {label}") };
                        assert!(evidence.as_slice().is_empty(), "{label}");
                        assert!(continuation.is_none(), "{label}");
                        assert_eq!(gaps.as_slice().iter().filter(|gap| gap.kind == codestory_contracts::packet_projection_v3::GapKindV3Dto::SourceUnavailable).count(), 2, "{label}");
                        assert_eq!(serde_json::to_value(answer_sufficiency).unwrap(), json!("not_asserted"), "{label}");
                        Ok(())
                    };
                    std::fs::remove_file(&path).unwrap();
                    check_refusal("deleted")?;
                    std::fs::write(&path, "fn changed() {}\n").unwrap();
                    check_refusal("stale")?;
                    std::fs::remove_file(&path).unwrap();
                    std::fs::create_dir(&path).unwrap();
                    check_refusal("directory")?;
                    std::fs::remove_dir(&path).unwrap();
                    #[cfg(unix)]
                    {
                        let outside = tempfile::tempdir().unwrap();
                        let target = outside.path().join("beta.rs");
                        std::fs::write(&target, &original).unwrap();
                        std::os::unix::fs::symlink(&target, &path).unwrap();
                        check_refusal("escaping-symlink")?;
                        std::fs::remove_file(&path).unwrap();
                    }
                    std::fs::write(&path, &original).unwrap();
                    Ok(positive)
                },
            )
            .expect("normal pinned freeze-to-public projection")
            .value;
        let PacketProjectionV3Dto::Complete { evidence, gaps, .. } = &projected.projection else {
            panic!("small admitted fixture must not exceed public output budget")
        };
        assert_eq!(evidence.as_slice().len(), 6);
        assert!(!gaps.as_slice().iter().any(|gap| gap.kind
            == codestory_contracts::packet_projection_v3::GapKindV3Dto::OutputBudgetExceeded));
        assert_eq!(
            evidence
                .as_slice()
                .iter()
                .filter(|row| row.kind == EvidenceKindV3Dto::ExactSource)
                .map(|row| (
                    row.symbol_id.as_ref().map(|id| id.as_str()),
                    row.path.as_ref().map(|path| path.as_str())
                ))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                (Some("11"), Some("src/alpha.rs")),
                (Some("21"), Some("tools/repos/worker/src/beta.rs")),
                (Some("31"), Some("tools/runner/src/alpha.rs")),
                (None, Some("tools/runner/src/reference.rs"))
            ])
        );
        assert_eq!(
            evidence
                .as_slice()
                .iter()
                .filter(|row| row.kind == EvidenceKindV3Dto::GraphRelation)
                .map(|row| row.summary.as_ref().unwrap().as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["11 -[member]-> 21", "21 -[member]-> 31"])
        );
        let public_json =
            serde_json::to_string(&projected.projection).expect("actual public serialization");
        for noise in ["NOISE_", "noise/"] {
            assert!(
                !public_json.contains(noise),
                "unadmitted stored evidence leaked: {noise}"
            );
        }
        let retained = controller.open_storage_read_only().unwrap();
        assert!(retained.get_node(CoreNodeId(99)).unwrap().is_some());
        assert_eq!(
            retained.get_edges().unwrap().len(),
            56,
            "exclusion is not deletion of stored noise"
        );
    }

    #[test]
    fn whole_file_renderer_has_no_fifty_line_focus_limit() {
        let source = "\n".repeat(80);
        let rendered = complete_file_markdown(&source, 80, 1024)
            .expect("all eighty numbered lines fit the larger bounded row");
        assert_eq!(source_receipt_line_range(&rendered), Some((1, 80)));
        assert!(rendered.len() <= 1024);
        assert!(matches!(
            complete_file_markdown("x\n", 1, 8),
            Err(PacketAdmissionGapKindV1::SourceBudgetExceeded)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn encoded_contained_source_alias_uses_admitted_logical_route() {
        use std::os::unix::fs::symlink;
        for (logical_name, target_name) in [
            ("commands.ps1", "data.bin"),
            ("zcommands.ps1", "aaa-data.bin"),
        ] {
            let cache = tempfile::tempdir().unwrap();
            let project = tempfile::tempdir().unwrap();
            let logical = project.path().join(logical_name);
            let target = project.path().join(target_name);
            let text = "function Invoke-Thing {\r\n  Write-Output 'é🦀'\r\n}\r\n";
            let mut bytes = vec![0xff, 0xfe];
            for unit in text.encode_utf16() {
                bytes.extend(unit.to_le_bytes());
            }
            std::fs::write(&target, &bytes).unwrap();
            symlink(target_name, &logical).unwrap();
            let storage_path = project.path().join(".cache/codestory.db");
            let controller = AppController::new_with_owned_cache_root(cache.path());
            controller
                .open_project_summary_with_storage_path(
                    project.path().to_path_buf(),
                    storage_path.clone(),
                )
                .unwrap();
            controller
                .run_indexing_blocking_without_runtime_refresh(
                    codestory_contracts::api::IndexMode::Full,
                )
                .unwrap();
            let storage = Store::open_read_only(&storage_path).unwrap();
            let file = storage
                .get_file_by_path(&logical)
                .unwrap()
                .expect("full indexing admits the selected contained structural alias");
            assert!(file.indexed && file.complete);
            let verified = crate::search_evidence::verified_file_checked(
                &storage,
                Some(project.path()),
                &file,
            )
            .expect("logical source route has complete pinned decoded text");
            assert_eq!(verified.content, text);
            let admission = PacketAdmissionReceiptV1 {
                packet_ordinal: 0,
                stable_identity: format!("path:{logical_name}"),
                score_version: "test".into(),
                reserved_source_bytes: INTERIM_SOURCE_ROW_UPPER_BOUND as u32,
                origin: PacketAdmissionOriginV1::Retrieval,
            };
            let packet = hydrate_admitted_file_source(&controller, &storage, &admission, &file);
            let windows = controller.path_source_windows(logical_name, &[(2, 2)], 1024);
            assert!(
                packet.is_ok() && windows.is_ok(),
                "admitted alias must retain decoder route; packet={packet:?}; windows_error={:?}",
                windows.as_ref().err()
            );
            assert!(packet.unwrap().source.contains("é🦀"));
            assert!(
                windows.unwrap().1[0]
                    .render(logical_name.into(), 2, 2, 1024)
                    .snippet
                    .contains("é🦀")
            );
        }
    }

    #[test]
    fn encoded_file_admission_hydrates_decoded_text_only_for_pinned_raw_bytes() {
        let cache = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let controller = AppController::new_with_owned_cache_root(cache.path());
        controller.state.lock().project_root = Some(project.path().to_path_buf());
        let storage = Store::new_in_memory().unwrap();
        let path = project.path().join("commands.ps1");
        let text = "function Invoke-Thing {\r\n  Write-Output 'é🦀'\r\n}\r\n";
        let file = FileInfo {
            id: 1,
            path: path.clone(),
            language: "powershell".into(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 3,
            file_role: FileRole::Source,
        };
        storage.insert_file(&file).unwrap();
        let admission = PacketAdmissionReceiptV1 {
            packet_ordinal: 0,
            stable_identity: "path:commands.ps1".into(),
            score_version: "test".into(),
            reserved_source_bytes: INTERIM_SOURCE_ROW_UPPER_BOUND as u32,
            origin: PacketAdmissionOriginV1::Retrieval,
        };
        for little in [true, false] {
            let mut bytes = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in text.encode_utf16() {
                bytes.extend(if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            std::fs::write(&path, &bytes).unwrap();
            storage
                .update_file_metadata(&file, Some(&format!("{:x}", Sha256::digest(&bytes))))
                .unwrap();
            let hydrated =
                hydrate_admitted_file_source(&controller, &storage, &admission, &file).unwrap();
            assert_eq!((hydrated.start_line, hydrated.end_line), (1, 3));
            assert!(hydrated.source.contains("é🦀"));
            std::fs::write(&path, text).unwrap();
            assert!(matches!(
                hydrate_admitted_file_source(&controller, &storage, &admission, &file),
                Err(PacketAdmissionGapKindV1::SourceUnavailable)
            ));
        }
    }

    #[test]
    fn file_admission_retains_only_complete_pinned_source_or_navigation() {
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        let project = tempfile::tempdir().expect("project");
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
        controller.state.lock().project_root = Some(project.path().to_path_buf());
        let storage = Store::new_in_memory().expect("store");
        let path = project.path().join("tools/repos/worker/src/settings.rs");
        let admission = PacketAdmissionReceiptV1 {
            packet_ordinal: 0,
            stable_identity: "path:tools/repos/worker/src/settings.rs".into(),
            score_version: "test".into(),
            reserved_source_bytes: INTERIM_SOURCE_ROW_UPPER_BOUND as u32,
            origin: PacketAdmissionOriginV1::Retrieval,
        };
        let mut file = FileInfo {
            id: 1,
            path: path.clone(),
            language: "rust".into(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 2,
            file_role: FileRole::Source,
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let short = "const ENABLED: bool = true;\nconst LIMIT: usize = 2;\n";
        std::fs::write(&path, short).expect("short source");
        storage.insert_file(&file).expect("file");
        let short_hash = format!("{:x}", Sha256::digest(short.as_bytes()));
        storage
            .update_file_metadata(&file, Some(&short_hash))
            .expect("pinned source hash");
        let whole = hydrate_admitted_file_source(&controller, &storage, &admission, &file)
            .expect("complete short file source");
        assert_eq!(whole.path, "tools/repos/worker/src/settings.rs");
        assert_eq!((whole.start_line, whole.end_line), (1, 2));
        assert!(whole.source.contains("const LIMIT: usize = 2;"));

        let many_short_lines = "\n".repeat(45);
        file.line_count = 45;
        std::fs::write(&path, &many_short_lines).expect("short multiline source");
        storage
            .update_file_metadata(
                &file,
                Some(&format!(
                    "{:x}",
                    Sha256::digest(many_short_lines.as_bytes())
                )),
            )
            .expect("multiline source hash");
        let whole = hydrate_admitted_file_source(&controller, &storage, &admission, &file)
            .expect("every line fits the bounded source row");
        assert_eq!((whole.start_line, whole.end_line), (1, 45));
        assert!(whole.source.len() <= INTERIM_SOURCE_ROW_UPPER_BOUND);

        let tiny_but_unrenderable = "\n".repeat(51);
        file.line_count = 51;
        std::fs::write(&path, &tiny_but_unrenderable).expect("raw bytes fit");
        storage
            .update_file_metadata(
                &file,
                Some(&format!(
                    "{:x}",
                    Sha256::digest(tiny_but_unrenderable.as_bytes())
                )),
            )
            .expect("raw source hash");
        assert!(matches!(
            hydrate_admitted_file_source(&controller, &storage, &admission, &file),
            Err(PacketAdmissionGapKindV1::SourceBudgetExceeded)
        ));

        let long = (1..=120)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        file.line_count = 120;
        std::fs::write(&path, &long).expect("long source");
        let long_hash = format!("{:x}", Sha256::digest(long.as_bytes()));
        storage
            .update_file_metadata(&file, Some(&long_hash))
            .expect("long source hash");
        assert!(matches!(
            hydrate_admitted_file_source(&controller, &storage, &admission, &file),
            Err(PacketAdmissionGapKindV1::SourceBudgetExceeded)
        ));

        std::fs::write(&path, "changed source\n").expect("drifted source");
        assert!(matches!(
            hydrate_admitted_file_source(&controller, &storage, &admission, &file),
            Err(PacketAdmissionGapKindV1::SourceUnavailable)
        ));

        let invalid_utf8 = b"\xff\n";
        file.line_count = 1;
        std::fs::write(&path, invalid_utf8).expect("invalid UTF-8 source");
        storage
            .update_file_metadata(&file, Some(&format!("{:x}", Sha256::digest(invalid_utf8))))
            .expect("invalid UTF-8 source hash");
        assert!(matches!(
            hydrate_admitted_file_source(&controller, &storage, &admission, &file),
            Err(PacketAdmissionGapKindV1::SourceUnavailable)
        ));
        storage
            .update_file_metadata(&file, None)
            .expect("missing source hash");
        assert!(matches!(
            hydrate_admitted_file_source(&controller, &storage, &admission, &file),
            Err(PacketAdmissionGapKindV1::SourceUnavailable)
        ));
    }

    #[test]
    fn file_node_navigation_keeps_authenticated_path_without_source_text() {
        let mut support = vec![SupportUnitDto {
            source_delivery: None,
            id: "symbol:node:17".into(),
            kind: SupportUnitKindDto::SymbolLocation,
            summary: "Navigation only: no bounded source range for node:17".into(),
            path: None,
            symbol_id: Some("17".into()),
            start_line: None,
            end_line: None,
            snippet: None,
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: None,
        }];
        let paths = HashMap::from([("node:17".into(), "src/large.rs".into())]);
        attach_file_navigation_paths(&mut support, &paths);
        assert_eq!(support[0].path.as_deref(), Some("src/large.rs"));
        assert!(support[0].summary.starts_with("Navigation only:"));
        assert!(support[0].snippet.is_none());
    }

    #[test]
    fn packet_file_budget_diagnostics_cover_raw_and_rendered_limits() {
        let process_cache = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
        controller.state.lock().project_root = Some(project.path().to_owned());
        let mut storage = Store::new_in_memory().unwrap();
        let path = project.path().join("large.rs");
        let mut file = FileInfo {
            id: 17,
            path: path.clone(),
            language: "rust".into(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: FileRole::Source,
        };
        storage.insert_file(&file).unwrap();
        storage
            .insert_nodes_batch(&[CoreNode {
                id: CoreNodeId(17),
                kind: CoreNodeKind::FILE,
                serialized_name: "large.rs".into(),
                file_node_id: Some(CoreNodeId(17)),
                ..Default::default()
            }])
            .unwrap();
        let admissions = ["path:large.rs", "node:17"]
            .into_iter()
            .enumerate()
            .map(|(ordinal, identity)| PacketAdmissionReceiptV1 {
                packet_ordinal: ordinal as u32,
                stable_identity: identity.into(),
                score_version: "test".into(),
                reserved_source_bytes: INTERIM_SOURCE_ROW_UPPER_BOUND as u32,
                origin: PacketAdmissionOriginV1::Retrieval,
            })
            .collect::<Vec<_>>();
        let raw_over_cap = "a complete physical source line\n".repeat(40);
        let rendered_over_cap = "\n".repeat(51);
        assert!(raw_over_cap.len() > INTERIM_SOURCE_ROW_UPPER_BOUND);
        assert!(rendered_over_cap.len() < INTERIM_SOURCE_ROW_UPPER_BOUND);
        for text in [&raw_over_cap, &rendered_over_cap] {
            file.line_count = text.lines().count() as u32;
            std::fs::write(&path, text).unwrap();
            storage
                .update_file_metadata(
                    &file,
                    Some(&format!("{:x}", Sha256::digest(text.as_bytes()))),
                )
                .unwrap();
            let mut gaps = Vec::new();
            let mut refusals = Vec::new();
            let (authenticated, sources, paths) = hydrate_admitted_sources_with_ranges(
                &controller,
                &storage,
                &admissions,
                &mut gaps,
                &HashMap::new(),
                &mut refusals,
                &mut BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(authenticated.len(), 2);
            assert!(sources.is_empty());
            assert_eq!(gaps.len(), 2);
            assert!(
                gaps.iter()
                    .all(|gap| gap.kind == PacketAdmissionGapKindV1::SourceBudgetExceeded)
            );
            assert_eq!(refusals.len(), 2);
            for (refusal, admission) in refusals.iter().zip(&admissions) {
                assert!(refusal.contains("verified whole-file"));
                assert!(refusal.contains("512-byte source-row cap"));
                assert!(refusal.contains("navigation only"));
                assert!(refusal.contains(&format!("Identity: {}", admission.stable_identity)));
                assert!(refusal.contains("file: large.rs"));
            }
            let input = PacketCompilationInputV1 {
                contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
                publication: PacketCompilationPublicationV1 {
                    project_id: "test".into(),
                    core_generation_id: "pinned".into(),
                    retrieval_generation: None,
                },
                admissions: authenticated.into_iter().map(|item| item.receipt).collect(),
                sources,
                relations: Vec::new(),
                ambiguities: Vec::new(),
                admission_gaps: gaps,
            };
            let mut product = compile_repository_evidence(&input);
            attach_file_navigation_paths(&mut product.support, &paths);
            assert_eq!(product.support.len(), 2);
            assert!(product.support.iter().all(|row| row.kind
                == SupportUnitKindDto::SymbolLocation
                && row.path.as_deref() == Some("large.rs")
                && row.snippet.is_none()
                && row.start_line.is_none()
                && row.end_line.is_none()
                && row.source_delivery.is_none()));
            assert!(product.continuation.is_empty());
        }
    }

    #[test]
    fn file_budget_diagnostic_keeps_reason_before_bounded_utf8_identity() {
        let admission = PacketAdmissionReceiptV1 {
            packet_ordinal: 0,
            stable_identity: format!(
                "path:{}",
                "雪".repeat(codestory_contracts::packet_projection_v3::MESSAGE_MAX_BYTES_V3)
            ),
            score_version: "test".into(),
            reserved_source_bytes: 512,
            origin: PacketAdmissionOriginV1::Retrieval,
        };
        let message = file_source_budget_refusal(&admission, Some("large.rs"));
        assert!(message.contains("verified whole-file"));
        assert!(message.contains("navigation only"));
        assert!(message.contains("Identity: path:"));
        assert!(message.ends_with("..."));
        assert!(message.len() <= codestory_contracts::packet_projection_v3::MESSAGE_MAX_BYTES_V3);
        assert!(codestory_contracts::packet_projection_v3::MessageTextV3::new(message).is_ok());
    }

    #[test]
    fn packet_file_admissions_distinguish_verified_budget_from_source_drift() {
        let process_cache = tempfile::tempdir().expect("owned runtime cache root");
        let project = tempfile::tempdir().expect("project");
        let controller = AppController::new_with_owned_cache_root(process_cache.path());
        controller.state.lock().project_root = Some(project.path().to_path_buf());
        let mut storage = Store::new_in_memory().expect("store");
        let path = project.path().join("large.rs");
        let source = (1..=120)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        std::fs::write(&path, &source).expect("source");
        let mut file = FileInfo {
            id: 17,
            path: path.clone(),
            language: "rust".into(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 120,
            file_role: FileRole::Source,
        };
        storage.insert_file(&file).expect("file");
        storage
            .update_file_metadata(
                &file,
                Some(&format!("{:x}", Sha256::digest(source.as_bytes()))),
            )
            .expect("pinned hash");
        storage
            .insert_nodes_batch(&[CoreNode {
                id: CoreNodeId(17),
                kind: CoreNodeKind::FILE,
                serialized_name: "large.rs".into(),
                file_node_id: Some(CoreNodeId(17)),
                start_line: Some(1),
                ..Default::default()
            }])
            .expect("file node");
        let admissions = ["path:large.rs", "node:17"]
            .into_iter()
            .enumerate()
            .map(|(ordinal, stable_identity)| PacketAdmissionReceiptV1 {
                packet_ordinal: ordinal as u32,
                stable_identity: stable_identity.into(),
                score_version: "test".into(),
                reserved_source_bytes: INTERIM_SOURCE_ROW_UPPER_BOUND as u32,
                origin: PacketAdmissionOriginV1::Retrieval,
            })
            .collect::<Vec<_>>();
        let mut gaps = Vec::new();
        let (authenticated, sources, paths) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps)
                .expect("packet hydration");
        assert_eq!(authenticated.len(), 2);
        assert!(sources.is_empty());
        assert_eq!(paths.get("node:17").map(String::as_str), Some("large.rs"));
        assert_eq!(gaps.len(), 2);
        assert!(
            gaps.iter()
                .all(|gap| matches!(gap.kind, PacketAdmissionGapKindV1::SourceBudgetExceeded))
        );
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "test".into(),
                core_generation_id: "pinned".into(),
                retrieval_generation: None,
            },
            admissions: authenticated.into_iter().map(|item| item.receipt).collect(),
            sources,
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: gaps,
        };
        let mut product = compile_repository_evidence(&input);
        attach_file_navigation_paths(&mut product.support, &paths);
        assert_eq!(product.support.len(), 2);
        assert!(product.support.iter().all(|unit| {
            unit.kind == SupportUnitKindDto::SymbolLocation
                && unit.path.as_deref() == Some("large.rs")
                && unit.snippet.is_none()
                && unit.summary.starts_with("Navigation only:")
        }));
        assert!(
            product.continuation.is_empty(),
            "the same pinned oversized file cannot fit a retry at the same row limit"
        );

        std::fs::write(&path, source.replace("line 1", "xxxx 1")).expect("drift");
        let mut gaps = Vec::new();
        let (authenticated, sources, paths) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps)
                .expect("packet drift check");
        assert!(authenticated.is_empty());
        assert!(sources.is_empty());
        assert!(paths.is_empty());
        assert_eq!(gaps.len(), 2);
        assert!(
            gaps.iter()
                .all(|gap| matches!(gap.kind, PacketAdmissionGapKindV1::SourceUnavailable))
        );

        std::fs::write(&path, &source).expect("restore source");
        storage
            .update_file_metadata(&file, None)
            .expect("remove pinned hash");
        let mut gaps = Vec::new();
        let (authenticated, sources, paths) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps)
                .expect("missing hash check");
        assert!(authenticated.is_empty() && sources.is_empty() && paths.is_empty());
        assert_eq!(gaps.len(), 2);

        file.complete = false;
        storage
            .update_file_metadata(
                &file,
                Some(&format!("{:x}", Sha256::digest(source.as_bytes()))),
            )
            .expect("incomplete file metadata");
        let mut gaps = Vec::new();
        let (authenticated, sources, paths) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps)
                .expect("incomplete metadata check");
        assert!(authenticated.is_empty() && sources.is_empty() && paths.is_empty());
        assert_eq!(gaps.len(), 2);

        let outside = tempfile::tempdir().expect("outside project");
        file.complete = true;
        file.path = outside.path().join("outside.rs");
        std::fs::write(&file.path, &source).expect("outside source");
        storage
            .update_file_metadata(
                &file,
                Some(&format!("{:x}", Sha256::digest(source.as_bytes()))),
            )
            .expect("mismatched file path");
        let mut gaps = Vec::new();
        let (authenticated, sources, paths) =
            hydrate_admitted_sources(&controller, &storage, &admissions, &mut gaps)
                .expect("path containment check");
        assert!(authenticated.is_empty() && sources.is_empty() && paths.is_empty());
        assert_eq!(gaps.len(), 2);
    }

    #[test]
    fn unknown_query_continuations_are_not_decoded() {
        assert!(drill_options_from_ids(&["deadline_lost_candidate:diagnostic".into()]).is_empty());
    }

    #[test]
    fn stable_symbol_continuation_round_trips_without_query_text() {
        let original = DrillOptionDto::omitted_symbol("gap", "node-1");
        let decoded = drill_options_from_ids(&[original.id]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].symbol_id.as_deref(), Some("node-1"));
        assert!(decoded[0].structural_reason.is_some());
    }

    fn test_admission(
        packet_ordinal: u32,
        stable_identity: &str,
        node_id: i64,
    ) -> AuthenticatedPacketAdmissionV1 {
        AuthenticatedPacketAdmissionV1 {
            receipt: PacketAdmissionReceiptV1 {
                packet_ordinal,
                stable_identity: stable_identity.into(),
                score_version: "test".into(),
                reserved_source_bytes: 1,
                origin: PacketAdmissionOriginV1::Retrieval,
            },
            origin: AuthenticatedPacketOriginV1::CoreNode(CoreNodeId(node_id)),
        }
    }

    #[test]
    fn uncertain_relation_is_not_compiler_evidence() {
        assert_eq!(
            relation_certainty(Some(ResolutionCertainty::Uncertain)),
            PacketRelationCertaintyV1::Uncertain
        );

        let mut storage = Store::new_in_memory().expect("store");
        storage
            .insert_nodes_batch(&[
                CoreNode {
                    id: CoreNodeId(1),
                    kind: CoreNodeKind::FILE,
                    serialized_name: "src/a.rs".into(),
                    ..Default::default()
                },
                CoreNode {
                    id: CoreNodeId(2),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "crate::run".into(),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_edges_batch(&[
                CoreEdge {
                    id: CoreEdgeId(10),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: CoreEdgeKind::MEMBER,
                    certainty: Some(ResolutionCertainty::Certain),
                    ..Default::default()
                },
                CoreEdge {
                    id: CoreEdgeId(11),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: CoreEdgeKind::CALL,
                    certainty: Some(ResolutionCertainty::Uncertain),
                    ..Default::default()
                },
            ])
            .expect("insert edges");
        let admissions = [
            test_admission(0, "path:src/a.rs", 1),
            test_admission(1, "node:2", 2),
        ];

        let relations = hydrate_induced_relations(&storage, &admissions).expect("relations");

        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].relation_id, "10");
        assert_eq!(relations[0].certainty, PacketRelationCertaintyV1::Certain);
    }

    #[test]
    fn numeric_confidence_cannot_upgrade_missing_certainty() {
        assert_eq!(relation_certainty(None), PacketRelationCertaintyV1::Unknown);

        let mut storage = Store::new_in_memory().expect("store");
        storage
            .insert_nodes_batch(&[
                CoreNode {
                    id: CoreNodeId(1),
                    kind: CoreNodeKind::FILE,
                    serialized_name: "src/a.rs".into(),
                    ..Default::default()
                },
                CoreNode {
                    id: CoreNodeId(2),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "crate::run".into(),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
        storage
            .insert_edges_batch(&[
                CoreEdge {
                    id: CoreEdgeId(10),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: CoreEdgeKind::MEMBER,
                    certainty: Some(ResolutionCertainty::Certain),
                    ..Default::default()
                },
                CoreEdge {
                    id: CoreEdgeId(11),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: CoreEdgeKind::CALL,
                    confidence: Some(0.99),
                    certainty: None,
                    ..Default::default()
                },
            ])
            .expect("insert edges");
        let admissions = [
            test_admission(0, "path:src/a.rs", 1),
            test_admission(1, "node:2", 2),
        ];

        let relations = hydrate_induced_relations(&storage, &admissions).expect("relations");

        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].relation_id, "10");
    }

    #[test]
    fn missing_or_invalid_source_bounds_never_become_line_one_source() {
        for (start_line, end_line) in [
            (None, Some(3)),
            (Some(3), None),
            (Some(0), Some(3)),
            (Some(4), Some(3)),
        ] {
            assert_eq!(valid_source_bounds(start_line, end_line), None);
        }
        assert_eq!(valid_source_bounds(Some(3), Some(4)), Some((3, 4)));
    }

    #[test]
    fn public_symbol_identity_requires_a_node_in_the_pinned_core() {
        let mut storage = Store::new_in_memory().expect("store");
        assert!(
            authenticated_node(&storage, "not-an-id")
                .expect("invalid identities are rejected")
                .is_none()
        );
        assert!(
            authenticated_node(&storage, "-42")
                .expect("missing identities are rejected")
                .is_none()
        );
        storage
            .insert_nodes_batch(&[CoreNode {
                id: CoreNodeId(-42),
                kind: CoreNodeKind::FUNCTION,
                serialized_name: "crate::real".into(),
                ..Default::default()
            }])
            .expect("insert authenticated node");
        assert_eq!(
            authenticated_node(&storage, "-42")
                .expect("lookup")
                .expect("authenticated node")
                .id,
            CoreNodeId(-42)
        );
    }

    #[test]
    fn path_admissions_participate_in_the_induced_relation_graph() {
        let mut storage = Store::new_in_memory().expect("store");
        storage
            .insert_nodes_batch(&[
                CoreNode {
                    id: CoreNodeId(1),
                    kind: CoreNodeKind::FILE,
                    serialized_name: "src/a.rs".into(),
                    ..Default::default()
                },
                CoreNode {
                    id: CoreNodeId(2),
                    kind: CoreNodeKind::FILE,
                    serialized_name: "src/b.rs".into(),
                    ..Default::default()
                },
                CoreNode {
                    id: CoreNodeId(3),
                    kind: CoreNodeKind::FUNCTION,
                    serialized_name: "crate::run".into(),
                    ..Default::default()
                },
            ])
            .expect("insert file nodes");
        storage
            .insert_edges_batch(&[
                CoreEdge {
                    id: CoreEdgeId(10),
                    source: CoreNodeId(1),
                    target: CoreNodeId(2),
                    kind: CoreEdgeKind::IMPORT,
                    certainty: Some(ResolutionCertainty::Certain),
                    ..Default::default()
                },
                CoreEdge {
                    id: CoreEdgeId(11),
                    source: CoreNodeId(1),
                    target: CoreNodeId(3),
                    kind: CoreEdgeKind::MEMBER,
                    certainty: Some(ResolutionCertainty::Certain),
                    ..Default::default()
                },
            ])
            .expect("insert import edge");
        let admissions = [
            AuthenticatedPacketAdmissionV1 {
                receipt: PacketAdmissionReceiptV1 {
                    packet_ordinal: 0,
                    stable_identity: "path:src/a.rs".into(),
                    score_version: "test".into(),
                    reserved_source_bytes: 1,
                    origin: PacketAdmissionOriginV1::Retrieval,
                },
                origin: AuthenticatedPacketOriginV1::CoreNode(CoreNodeId(1)),
            },
            AuthenticatedPacketAdmissionV1 {
                receipt: PacketAdmissionReceiptV1 {
                    packet_ordinal: 1,
                    stable_identity: "path:src/b.rs".into(),
                    score_version: "test".into(),
                    reserved_source_bytes: 1,
                    origin: PacketAdmissionOriginV1::Retrieval,
                },
                origin: AuthenticatedPacketOriginV1::CoreNode(CoreNodeId(2)),
            },
            AuthenticatedPacketAdmissionV1 {
                receipt: PacketAdmissionReceiptV1 {
                    packet_ordinal: 2,
                    stable_identity: "node:3".into(),
                    score_version: "test".into(),
                    reserved_source_bytes: 1,
                    origin: PacketAdmissionOriginV1::Retrieval,
                },
                origin: AuthenticatedPacketOriginV1::CoreNode(CoreNodeId(3)),
            },
        ];

        let relations = hydrate_induced_relations(&storage, &admissions).expect("relations");

        assert_eq!(relations.len(), 2);
        assert_eq!(relations[0].from_identity, "path:src/a.rs");
        assert_eq!(relations[0].to_identity, "path:src/b.rs");
        assert_eq!(relations[0].relation_kind, PacketRelationKindV1::Import);
        assert_eq!(relations[1].from_identity, "path:src/a.rs");
        assert_eq!(relations[1].to_identity, "node:3");
        assert_eq!(relations[1].relation_kind, PacketRelationKindV1::Member);
    }

    #[test]
    fn source_receipts_require_observed_numbered_lines() {
        assert_eq!(source_receipt_line_range("source without a receipt"), None);
        assert_eq!(
            source_receipt_line_range("```text\n>    7 | fn run() {}\n     8 | }\n```"),
            Some((7, 8))
        );
    }
}
