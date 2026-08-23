//! Dark core-snapshot adapter for indexed source call-path proof facts.
//!
//! This module is deliberately not a product facade. Its caller must already
//! be inside `PublicOperationService::run_with_cancel`, which installs the
//! complete core snapshot used for every Store read below.

// The complete adapter stays dark until the atomic v3 surface cut. Building
// `codestory-runtime` with either sealed support feature must not turn that
// deliberate lack of production callers into warning noise.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[cfg(all(
    not(any(test, feature = "test-support")),
    feature = "proof-qualification-support"
))]
use codestory_agent::proof_qualification_support::{
    AdmittedRawCallEdge, BuiltCallPathFacts, CallableContainmentEvidence,
    CheckedBuiltCallPathIntegration, ExactScopeSelector, ExactSymbolSelector, FactBuildGap,
    IndexedCallEdgeReceipt, IndexedLineWindow, InternalCorePublicationIdentity, InternalProjection,
    PROOF_DOMAIN, PinnedNodeIdentity, ProofHashes, RawAdmissionFailure, ReceiptRef,
    ResolvedNodeIdentity, UnavailableReason, ValidatedCallPathContract, ValidatedContractRendering,
    VerifiedDirectCallFact, VerifiedProofFact, check_built_call_path_integration,
    diagnose_raw_call_edge, project_internal_call_path_result,
};
#[cfg(any(test, feature = "test-support"))]
use codestory_agent::proof_qualification_test_support::{
    AdmittedRawCallEdge, BuiltCallPathFacts, CallableContainmentEvidence,
    CheckedBuiltCallPathIntegration, ExactScopeSelector, ExactSymbolSelector, FactBuildGap,
    IndexedCallEdgeReceipt, IndexedLineWindow, InternalCorePublicationIdentity, InternalProjection,
    PROOF_DOMAIN, PinnedNodeIdentity, ProofHashes, RawAdmissionFailure, RawCallEdgeAdmission,
    ReceiptRef, ResolvedNodeIdentity, UnavailableReason, ValidatedCallPathContract,
    ValidatedContractRendering, VerifiedDirectCallFact, VerifiedProofFact, admit_raw_call_edge,
    check_built_call_path_integration, diagnose_raw_call_edge, project_internal_call_path_result,
};
use codestory_contracts::api::ApiError;
use codestory_contracts::graph::{Node, NodeId, NodeKind};
use codestory_contracts::proof_resolution::{CallResolutionFact, ProofResolutionStatus};
use codestory_store::{FileInfo, IndexPublicationRecord, Store};
use codestory_workspace::{
    ProjectRelativePathResolution, WorkspacePathIdentity, project_identity_v3,
    resolve_project_relative_path, workspace_relative_path,
};
use sha2::{Digest, Sha256};

use crate::AppController;
use crate::path_identity::OperationPathIdentityResolver;
use crate::services::{PublicOperation, PublicOperationService};

const INDEXED_LINE_KIND: &str = "indexed_line_v1";
const MAX_LINE_WINDOW_BYTES: usize = 8_192;
pub const MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP: u32 = 128;
pub const MAX_QUALIFICATION_OBSERVED_RECEIPTS_PER_CASE: usize =
    MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP as usize * 6;
const RECEIPT_DOMAIN: &[u8] = b"codestory.indexed-call-edge-receipt.v1\0";
const CALLABLE_KINDS: [NodeKind; 3] = [NodeKind::FUNCTION, NodeKind::METHOD, NodeKind::MACRO];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SelectorFailure {
    Missing,
    Ambiguous,
    NonCallable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorGateOutcome {
    Resolved { node_id: NodeId },
    Failed(SelectorFailure),
    Unavailable(UnavailableReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorQualificationTrace {
    pub selector_index: usize,
    pub outcome: SelectorGateOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContainmentFailure {
    EdgeSourceFileMismatch,
    Missing,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceBindingFailure {
    FileIncomplete,
    StoredHashAbsent,
    WorkingTreeReadFailed,
    WorkingTreeHashMismatch,
    InvalidUtf8,
    ExactCallsiteMismatch,
    LineMissing,
    LineOverLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CandidateFailure {
    RawAdmission(RawAdmissionFailure),
    ResolutionFact(ResolutionFactFailure),
    Containment(ContainmentFailure),
    SourceBinding(SourceBindingFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolutionFactFailure {
    Missing,
    Inconsistent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateFailureHistogram {
    pub reason: CandidateFailure,
    pub edge_ids: Vec<codestory_contracts::graph::EdgeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateGate {
    RawAdmission,
    ResolutionFact,
    Containment,
    SourceBinding,
    Line,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepQualificationOutcome {
    SelectorBlocked {
        selector_index: usize,
        outcome: SelectorGateOutcome,
    },
    Admitted {
        edge_ids: Vec<codestory_contracts::graph::EdgeId>,
    },
    FirstZeroSurvivor {
        gate: CandidateGate,
        histogram: Vec<CandidateFailureHistogram>,
    },
    CandidateLimitExceeded {
        maximum_candidate_edges: u32,
        observed_candidate_edges_at_least: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepQualificationTrace {
    pub step_index: usize,
    pub candidate_edge_ids: Vec<codestory_contracts::graph::EdgeId>,
    pub outcome: StepQualificationOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizationFailure {
    ReceiptIntegration,
    ReceiptBudget,
    ProjectionBudget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalizationTrace {
    NotRun,
    Complete { projection_bytes: usize },
    Failed(FinalizationFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofQualificationTrace {
    pub selectors: Vec<SelectorQualificationTrace>,
    pub selector_early_return: bool,
    pub steps: Vec<StepQualificationTrace>,
    pub finalization: FinalizationTrace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedBuiltCallPathFacts {
    pub built: BuiltCallPathFacts,
    pub trace: ProofQualificationTrace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedIntegratedProjectedCallPathResult {
    pub result: Result<IntegratedProjectedCallPathResult, ApiError>,
    pub trace: ProofQualificationTrace,
}

#[derive(Default)]
struct StepTraceAccumulator {
    candidate_edge_ids: Vec<codestory_contracts::graph::EdgeId>,
    raw_survivors: Vec<codestory_contracts::graph::EdgeId>,
    resolution_fact_survivors: Vec<codestory_contracts::graph::EdgeId>,
    containment_survivors: Vec<codestory_contracts::graph::EdgeId>,
    source_survivors: Vec<codestory_contracts::graph::EdgeId>,
    admitted: Vec<codestory_contracts::graph::EdgeId>,
    raw_failures: BTreeMap<CandidateFailure, Vec<codestory_contracts::graph::EdgeId>>,
    resolution_fact_failures: BTreeMap<CandidateFailure, Vec<codestory_contracts::graph::EdgeId>>,
    containment_failures: BTreeMap<CandidateFailure, Vec<codestory_contracts::graph::EdgeId>>,
    source_failures: BTreeMap<CandidateFailure, Vec<codestory_contracts::graph::EdgeId>>,
    line_failures: BTreeMap<CandidateFailure, Vec<codestory_contracts::graph::EdgeId>>,
}

impl StepTraceAccumulator {
    fn reject_raw(
        &mut self,
        edge_id: codestory_contracts::graph::EdgeId,
        reason: RawAdmissionFailure,
    ) {
        self.raw_failures
            .entry(CandidateFailure::RawAdmission(reason))
            .or_default()
            .push(edge_id);
    }

    fn reject_containment(
        &mut self,
        edge_id: codestory_contracts::graph::EdgeId,
        reason: ContainmentFailure,
    ) {
        self.containment_failures
            .entry(CandidateFailure::Containment(reason))
            .or_default()
            .push(edge_id);
    }

    fn reject_resolution_fact(
        &mut self,
        edge_id: codestory_contracts::graph::EdgeId,
        reason: ResolutionFactFailure,
    ) {
        self.resolution_fact_failures
            .entry(CandidateFailure::ResolutionFact(reason))
            .or_default()
            .push(edge_id);
    }

    fn reject_source(
        &mut self,
        edge_id: codestory_contracts::graph::EdgeId,
        reason: SourceBindingFailure,
    ) {
        self.source_failures
            .entry(CandidateFailure::SourceBinding(reason))
            .or_default()
            .push(edge_id);
    }

    fn reject_line(
        &mut self,
        edge_id: codestory_contracts::graph::EdgeId,
        reason: SourceBindingFailure,
    ) {
        self.line_failures
            .entry(CandidateFailure::SourceBinding(reason))
            .or_default()
            .push(edge_id);
    }

    fn finish(mut self, step_index: usize) -> StepQualificationTrace {
        self.candidate_edge_ids.sort();
        self.candidate_edge_ids.dedup();
        self.admitted.sort();
        self.admitted.dedup();
        let outcome = if self.raw_survivors.is_empty() {
            StepQualificationOutcome::FirstZeroSurvivor {
                gate: CandidateGate::RawAdmission,
                histogram: failure_histogram(self.raw_failures),
            }
        } else if self.resolution_fact_survivors.is_empty() {
            StepQualificationOutcome::FirstZeroSurvivor {
                gate: CandidateGate::ResolutionFact,
                histogram: failure_histogram(self.resolution_fact_failures),
            }
        } else if self.containment_survivors.is_empty() {
            StepQualificationOutcome::FirstZeroSurvivor {
                gate: CandidateGate::Containment,
                histogram: failure_histogram(self.containment_failures),
            }
        } else if self.source_survivors.is_empty() {
            StepQualificationOutcome::FirstZeroSurvivor {
                gate: CandidateGate::SourceBinding,
                histogram: failure_histogram(self.source_failures),
            }
        } else if self.admitted.is_empty() {
            StepQualificationOutcome::FirstZeroSurvivor {
                gate: CandidateGate::Line,
                histogram: failure_histogram(self.line_failures),
            }
        } else {
            StepQualificationOutcome::Admitted {
                edge_ids: self.admitted,
            }
        };
        StepQualificationTrace {
            step_index,
            candidate_edge_ids: self.candidate_edge_ids,
            outcome,
        }
    }
}

fn failure_histogram(
    failures: BTreeMap<CandidateFailure, Vec<codestory_contracts::graph::EdgeId>>,
) -> Vec<CandidateFailureHistogram> {
    failures
        .into_iter()
        .map(|(reason, mut edge_ids)| {
            edge_ids.sort();
            edge_ids.dedup();
            CandidateFailureHistogram { reason, edge_ids }
        })
        .collect()
}

pub(crate) fn build_indexed_source_call_path_facts(
    controller: &AppController,
    contract: &ValidatedCallPathContract,
) -> Result<BuiltCallPathFacts, ApiError> {
    Ok(build_observed_indexed_source_call_path_facts(controller, contract)?.built)
}

pub(crate) fn build_observed_indexed_source_call_path_facts(
    controller: &AppController,
    contract: &ValidatedCallPathContract,
) -> Result<ObservedBuiltCallPathFacts, ApiError> {
    let publication = controller.active_core_publication().ok_or_else(|| {
        ApiError::internal("indexed call-path proof requires an active core publication")
    })?;
    let project_root = controller.require_project_root()?;
    let project_id = project_identity_v3(&project_root).project_id;
    let storage = controller.open_storage_read_only()?;
    build_from_store_observed(
        &storage,
        &project_root,
        &project_id,
        &publication,
        contract,
        |path| fs::read(path),
    )
}

fn build_from_store<R>(
    store: &Store,
    project_root: &Path,
    project_id: &str,
    publication: &IndexPublicationRecord,
    contract: &ValidatedCallPathContract,
    read_source: R,
) -> Result<BuiltCallPathFacts, ApiError>
where
    R: FnMut(&Path) -> io::Result<Vec<u8>>,
{
    Ok(build_from_store_observed(
        store,
        project_root,
        project_id,
        publication,
        contract,
        read_source,
    )?
    .built)
}

fn build_from_store_observed<R>(
    store: &Store,
    project_root: &Path,
    project_id: &str,
    publication: &IndexPublicationRecord,
    contract: &ValidatedCallPathContract,
    mut read_source: R,
) -> Result<ObservedBuiltCallPathFacts, ApiError>
where
    R: FnMut(&Path) -> io::Result<Vec<u8>>,
{
    let proof_publication = InternalCorePublicationIdentity {
        project_id: project_id.to_owned(),
        generation_id: publication.generation_id.clone(),
        run_id: publication.run_id.clone(),
    };
    if store
        .validate_proof_resolution_publication(publication)
        .is_err()
    {
        return Ok(ObservedBuiltCallPathFacts {
            built: BuiltCallPathFacts {
                publication: proof_publication,
                facts: Vec::new(),
                receipts: Vec::new(),
                gaps: Vec::new(),
                unavailable: vec![UnavailableReason::ProofSemanticProjectionUnavailable],
            },
            trace: ProofQualificationTrace {
                selectors: Vec::new(),
                selector_early_return: true,
                steps: Vec::new(),
                finalization: FinalizationTrace::NotRun,
            },
        });
    }
    let files = store.files().inventory().map_err(store_error)?;
    let file_rows = store.files().get_files().map_err(store_error)?;
    let mut path_identities = OperationPathIdentityResolver::native();
    let mut resolved = Vec::with_capacity(contract.spec().steps().len() + 1);
    let mut gaps = Vec::new();
    let mut unavailable = Vec::new();
    let mut selectors = Vec::new();
    let selector_context = SelectorContext {
        store,
        project_root,
        project_id,
        publication,
        files: &file_rows,
    };

    for (selector_index, selector) in std::iter::once(contract.spec().start())
        .chain(contract.spec().steps().iter().map(|step| step.target()))
        .enumerate()
    {
        match resolve_symbol_selector(&selector_context, selector, &mut path_identities)? {
            SelectorResolution::Resolved(node) => {
                let node_id = parse_pinned_node_id(&node.pinned).ok_or_else(|| {
                    ApiError::internal(
                        "resolved call-path selector lost its numeric publication pin",
                    )
                })?;
                selectors.push(SelectorQualificationTrace {
                    selector_index,
                    outcome: SelectorGateOutcome::Resolved { node_id },
                });
                resolved.push(node);
            }
            SelectorResolution::Missing => {
                gaps.push(FactBuildGap::SelectorMissing { selector_index });
                selectors.push(SelectorQualificationTrace {
                    selector_index,
                    outcome: SelectorGateOutcome::Failed(SelectorFailure::Missing),
                });
            }
            SelectorResolution::Ambiguous => {
                gaps.push(FactBuildGap::SelectorAmbiguous { selector_index });
                selectors.push(SelectorQualificationTrace {
                    selector_index,
                    outcome: SelectorGateOutcome::Failed(SelectorFailure::Ambiguous),
                });
            }
            SelectorResolution::NonCallable => {
                gaps.push(FactBuildGap::NonCallableSelector { selector_index });
                selectors.push(SelectorQualificationTrace {
                    selector_index,
                    outcome: SelectorGateOutcome::Failed(SelectorFailure::NonCallable),
                });
            }
            SelectorResolution::Unavailable(reason) => {
                unavailable.push(reason.clone());
                selectors.push(SelectorQualificationTrace {
                    selector_index,
                    outcome: SelectorGateOutcome::Unavailable(reason),
                });
            }
        }
    }

    for (scope_index, selector) in (contract.spec().steps().len() + 1..).zip(
        contract
            .spec()
            .traversal_prohibitions()
            .iter()
            .chain(contract.spec().projection_exclusions()),
    ) {
        match resolve_scope_selector(&selector_context, selector, &mut path_identities)? {
            SelectorResolution::Resolved(node) => {
                let node_id = parse_pinned_node_id(&node.pinned).ok_or_else(|| {
                    ApiError::internal("resolved call-path scope lost its numeric publication pin")
                })?;
                selectors.push(SelectorQualificationTrace {
                    selector_index: scope_index,
                    outcome: SelectorGateOutcome::Resolved { node_id },
                });
            }
            SelectorResolution::Missing => {
                gaps.push(FactBuildGap::SelectorMissing {
                    selector_index: scope_index,
                });
                selectors.push(SelectorQualificationTrace {
                    selector_index: scope_index,
                    outcome: SelectorGateOutcome::Failed(SelectorFailure::Missing),
                });
            }
            SelectorResolution::Ambiguous => {
                gaps.push(FactBuildGap::SelectorAmbiguous {
                    selector_index: scope_index,
                });
                selectors.push(SelectorQualificationTrace {
                    selector_index: scope_index,
                    outcome: SelectorGateOutcome::Failed(SelectorFailure::Ambiguous),
                });
            }
            SelectorResolution::NonCallable => {
                gaps.push(FactBuildGap::NonCallableSelector {
                    selector_index: scope_index,
                });
                selectors.push(SelectorQualificationTrace {
                    selector_index: scope_index,
                    outcome: SelectorGateOutcome::Failed(SelectorFailure::NonCallable),
                });
            }
            SelectorResolution::Unavailable(reason) => {
                unavailable.push(reason.clone());
                selectors.push(SelectorQualificationTrace {
                    selector_index: scope_index,
                    outcome: SelectorGateOutcome::Unavailable(reason),
                });
            }
        }
    }

    unavailable.sort();
    unavailable.dedup();
    if !unavailable.is_empty()
        || !gaps.is_empty()
        || resolved.len() != contract.spec().steps().len() + 1
    {
        gaps.sort();
        gaps.dedup();
        let blocking_selector = selectors
            .iter()
            .find(|selector| !matches!(selector.outcome, SelectorGateOutcome::Resolved { .. }))
            .expect("selector early return has a blocking selector");
        let steps = (0..contract.spec().steps().len())
            .map(|step_index| StepQualificationTrace {
                step_index,
                candidate_edge_ids: Vec::new(),
                outcome: StepQualificationOutcome::SelectorBlocked {
                    selector_index: blocking_selector.selector_index,
                    outcome: blocking_selector.outcome.clone(),
                },
            })
            .collect();
        return Ok(ObservedBuiltCallPathFacts {
            built: BuiltCallPathFacts {
                publication: proof_publication,
                facts: Vec::new(),
                receipts: Vec::new(),
                gaps,
                unavailable,
            },
            trace: ProofQualificationTrace {
                selectors,
                selector_early_return: true,
                steps,
                finalization: FinalizationTrace::NotRun,
            },
        });
    }

    let files_by_id = files
        .into_iter()
        .map(|file| (file.id, file))
        .collect::<HashMap<_, _>>();
    let rows_by_id = file_rows
        .into_iter()
        .map(|file| (file.id, file))
        .collect::<HashMap<_, _>>();
    let mut source_cache = HashMap::<WorkspacePathIdentity, SourceObservation>::new();
    let mut facts = Vec::new();
    let mut receipts = Vec::new();
    let mut steps = Vec::with_capacity(contract.spec().steps().len());

    for (step_index, pair) in resolved.windows(2).enumerate() {
        let source = &pair[0];
        let target = &pair[1];
        let Some(source_id) = parse_pinned_node_id(&source.pinned) else {
            return Err(ApiError::internal(
                "resolved call-path source lost its numeric publication pin",
            ));
        };
        let Some(target_id) = parse_pinned_node_id(&target.pinned) else {
            return Err(ApiError::internal(
                "resolved call-path target lost its numeric publication pin",
            ));
        };
        let Some(source_node) = store.get_node(source_id).map_err(store_error)? else {
            return Err(ApiError::internal(
                "resolved call-path source disappeared from the pinned snapshot",
            ));
        };
        let Some(target_node) = store.get_node(target_id).map_err(store_error)? else {
            return Err(ApiError::internal(
                "resolved call-path target disappeared from the pinned snapshot",
            ));
        };
        let mut admitted_any = false;
        let mut step_unavailable = Vec::new();
        let mut step_gaps = Vec::new();
        let mut containment_failed = false;
        let bounded_edges = store
            .get_bounded_raw_call_edges_by_effective_source(
                source_id,
                MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP,
            )
            .map_err(store_error)?;
        let edges = bounded_edges.edges;
        if bounded_edges.truncated {
            steps.push(StepQualificationTrace {
                step_index,
                candidate_edge_ids: edges.iter().map(|edge| edge.id).collect(),
                outcome: StepQualificationOutcome::CandidateLimitExceeded {
                    maximum_candidate_edges: MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP,
                    observed_candidate_edges_at_least: MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP
                        + 1,
                },
            });
            unavailable.push(UnavailableReason::ProofFactsUnavailable);
            continue;
        }
        let mut step_trace = StepTraceAccumulator {
            candidate_edge_ids: edges.iter().map(|edge| edge.id).collect(),
            ..StepTraceAccumulator::default()
        };
        for edge in edges {
            let admitted = match diagnose_raw_call_edge(&edge, source_id, target_id) {
                Ok(admitted) => {
                    step_trace.raw_survivors.push(edge.id);
                    admitted
                }
                Err(reason) => {
                    step_trace.reject_raw(edge.id, reason);
                    continue;
                }
            };
            let Some(resolution_fact) = store
                .get_exact_proof_resolution_fact_by_edge(edge.id)
                .map_err(store_error)?
            else {
                step_trace.reject_resolution_fact(edge.id, ResolutionFactFailure::Missing);
                continue;
            };
            if !resolution_fact_matches_raw_edge(&resolution_fact, &admitted, source_id, target_id)
            {
                step_unavailable.push(UnavailableReason::ProofSemanticProjectionUnavailable);
                step_trace.reject_resolution_fact(edge.id, ResolutionFactFailure::Inconsistent);
                continue;
            }
            step_trace.resolution_fact_survivors.push(edge.id);
            if !is_callable(source_node.kind) || !is_callable(target_node.kind) {
                step_trace.reject_containment(edge.id, ContainmentFailure::Missing);
                continue;
            }
            let Some(source_file_id) = source_node.file_node_id else {
                step_trace.reject_containment(edge.id, ContainmentFailure::EdgeSourceFileMismatch);
                continue;
            };
            if admitted.file_node_id != source_file_id {
                step_trace.reject_containment(edge.id, ContainmentFailure::EdgeSourceFileMismatch);
                continue;
            }
            let containment = match authenticate_containment(
                store,
                &source_node,
                admitted.file_node_id,
                admitted.line,
            )? {
                Ok(containment) => {
                    step_trace.containment_survivors.push(edge.id);
                    containment
                }
                Err(reason) => {
                    containment_failed = true;
                    step_trace.reject_containment(edge.id, reason);
                    continue;
                }
            };
            let Some(file) = files_by_id.get(&admitted.file_node_id.0) else {
                step_unavailable.push(UnavailableReason::SourceNotBoundToPublication);
                step_trace.reject_source(edge.id, SourceBindingFailure::FileIncomplete);
                continue;
            };
            if !file.indexed || !file.complete {
                step_unavailable.push(UnavailableReason::SourceNotBoundToPublication);
                step_trace.reject_source(edge.id, SourceBindingFailure::FileIncomplete);
                continue;
            }
            let Some(file_row) = rows_by_id.get(&file.id) else {
                step_unavailable.push(UnavailableReason::SourceNotBoundToPublication);
                step_trace.reject_source(edge.id, SourceBindingFailure::FileIncomplete);
                continue;
            };
            let Some(indexed_hash) = file.content_hash.as_deref() else {
                step_unavailable.push(UnavailableReason::SourceNotBoundToPublication);
                step_trace.reject_source(edge.id, SourceBindingFailure::StoredHashAbsent);
                continue;
            };
            if resolution_fact.callsite.source_sha256 != indexed_hash {
                step_unavailable.push(UnavailableReason::ProofSemanticProjectionUnavailable);
                step_trace.reject_source(edge.id, SourceBindingFailure::StoredHashAbsent);
                continue;
            }
            let bound = match bind_source_once(
                project_root,
                file_row,
                indexed_hash,
                &mut path_identities,
                &mut source_cache,
                &mut read_source,
            ) {
                Ok(bound) => bound,
                Err(reason) => {
                    step_trace.reject_source(edge.id, reason);
                    if reason == SourceBindingFailure::InvalidUtf8 {
                        step_gaps.push(FactBuildGap::InvalidUtf8 { step_index });
                    } else {
                        step_unavailable.push(UnavailableReason::SourceNotBoundToPublication);
                    }
                    continue;
                }
            };
            step_trace.source_survivors.push(edge.id);
            let Some((byte_start, byte_end, text)) = complete_line(&bound.bytes, admitted.line)
            else {
                step_gaps.push(FactBuildGap::SourceLineOutOfRange { step_index });
                step_trace.reject_line(edge.id, SourceBindingFailure::LineMissing);
                continue;
            };
            let exact_start = usize::try_from(resolution_fact.callsite.start_byte).ok();
            let exact_end = usize::try_from(resolution_fact.callsite.end_byte_exclusive).ok();
            if exact_start.is_none_or(|start| start < byte_start)
                || exact_end.is_none_or(|end| end > byte_end)
                || exact_start
                    .zip(exact_end)
                    .and_then(|(start, end)| bound.bytes.get(start..end))
                    != Some(resolution_fact.callsite.raw_target.as_bytes())
            {
                step_unavailable.push(UnavailableReason::ProofSemanticProjectionUnavailable);
                step_trace.reject_line(edge.id, SourceBindingFailure::ExactCallsiteMismatch);
                continue;
            }
            if byte_end - byte_start > MAX_LINE_WINDOW_BYTES {
                step_gaps.push(FactBuildGap::SourceWindowTooLarge { step_index });
                step_trace.reject_line(edge.id, SourceBindingFailure::LineOverLimit);
                continue;
            }
            let line_window = IndexedLineWindow {
                kind: INDEXED_LINE_KIND,
                project_file_components: bound.project_file_components.clone(),
                indexed_sha256: indexed_hash.to_owned(),
                observed_sha256: bound.observed_sha256.clone(),
                anchor_line: admitted.line,
                byte_start,
                byte_end,
                text,
            };
            let receipt_id = receipt_id(
                project_id,
                publication,
                &admitted,
                source_id.0,
                target_id.0,
                indexed_hash,
                &resolution_fact,
            );
            let receipt_ref = ReceiptRef {
                receipt_id,
                edge_id: admitted.edge_id.0.to_string(),
            };
            receipts.push(IndexedCallEdgeReceipt {
                receipt: receipt_ref.clone(),
                source: source.clone(),
                target: target.clone(),
                resolution_fact_id: resolution_fact.fact_id,
                resolution_evidence_sha256: resolution_fact.provenance.evidence_sha256,
                exact_callsite_start_byte: resolution_fact.callsite.start_byte,
                callsite_identity: admitted.callsite_identity,
                column_or_ordinal: admitted.column_or_ordinal,
                containment,
                line_window,
            });
            facts.push(VerifiedProofFact::DirectCall(VerifiedDirectCallFact {
                receipt: receipt_ref,
                source: source.clone(),
                target: target.clone(),
            }));
            admitted_any = true;
            step_trace.admitted.push(edge.id);
        }
        steps.push(step_trace.finish(step_index));
        if admitted_any {
            continue;
        }
        if !step_unavailable.is_empty() || !step_gaps.is_empty() {
            unavailable.append(&mut step_unavailable);
            gaps.append(&mut step_gaps);
        } else {
            gaps.push(if source_id == target_id {
                FactBuildGap::RecursiveCallNotRepresentable { step_index }
            } else if containment_failed {
                FactBuildGap::EdgeContainmentUnproven { step_index }
            } else {
                FactBuildGap::DirectCallMissing { step_index }
            });
        }
    }
    gaps.sort();
    gaps.dedup();
    unavailable.sort();
    unavailable.dedup();
    Ok(ObservedBuiltCallPathFacts {
        built: BuiltCallPathFacts {
            publication: proof_publication,
            facts,
            receipts,
            gaps,
            unavailable,
        },
        trace: ProofQualificationTrace {
            selectors,
            selector_early_return: false,
            steps,
            finalization: FinalizationTrace::NotRun,
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegratedProjectedCallPathResult {
    pub integration: CheckedBuiltCallPathIntegration,
    pub projection: InternalProjection,
}

pub(crate) fn finalize_observed_call_path(
    contract: &ValidatedCallPathContract,
    hashes: &ProofHashes,
    rendering: &ValidatedContractRendering,
    observed: ObservedBuiltCallPathFacts,
) -> ObservedIntegratedProjectedCallPathResult {
    let ObservedBuiltCallPathFacts { built, mut trace } = observed;
    let integration = match check_built_call_path_integration(contract, hashes, rendering, built) {
        Ok(integration) => integration,
        Err(error) => {
            trace.finalization = FinalizationTrace::Failed(FinalizationFailure::ReceiptIntegration);
            return ObservedIntegratedProjectedCallPathResult {
                result: Err(ApiError::internal(format!(
                    "indexed call-path checked integration failed: {error:?}"
                ))),
                trace,
            };
        }
    };
    let projection = match project_internal_call_path_result(&integration) {
        Ok(projection) => projection,
        Err(error) => {
            trace.finalization = FinalizationTrace::Failed(FinalizationFailure::ProjectionBudget);
            return ObservedIntegratedProjectedCallPathResult {
                result: Err(ApiError::internal(format!(
                    "indexed call-path projection construction failed: {error:?}"
                ))),
                trace,
            };
        }
    };
    trace.finalization = match &projection {
        InternalProjection::Complete {
            serialized_size, ..
        } => FinalizationTrace::Complete {
            projection_bytes: *serialized_size,
        },
        InternalProjection::BudgetExceeded { .. } => {
            FinalizationTrace::Failed(FinalizationFailure::ReceiptBudget)
        }
    };
    ObservedIntegratedProjectedCallPathResult {
        result: Ok(IntegratedProjectedCallPathResult {
            integration,
            projection,
        }),
        trace,
    }
}

pub(crate) fn run_integrated_projected_public_operation(
    service: &PublicOperationService,
    controller: &AppController,
    contract: &ValidatedCallPathContract,
    hashes: &ProofHashes,
    rendering: &ValidatedContractRendering,
    cancelled: Arc<AtomicBool>,
) -> Result<PublicOperation<IntegratedProjectedCallPathResult>, ApiError> {
    service.run_with_cancel(PROOF_DOMAIN, cancelled, || {
        let built = build_indexed_source_call_path_facts(controller, contract)?;
        let integration = check_built_call_path_integration(contract, hashes, rendering, built)
            .map_err(|error| {
                ApiError::internal(format!(
                    "indexed call-path checked integration failed: {error:?}"
                ))
            })?;
        let projection = project_internal_call_path_result(&integration).map_err(|error| {
            ApiError::internal(format!(
                "indexed call-path projection construction failed: {error:?}"
            ))
        })?;
        Ok(IntegratedProjectedCallPathResult {
            integration,
            projection,
        })
    })
}

struct CheckedIntegrationInputs<'a> {
    contract: &'a ValidatedCallPathContract,
    hashes: &'a ProofHashes,
    rendering: &'a ValidatedContractRendering,
}

fn evaluate_from_store<R>(
    store: &Store,
    project_root: &Path,
    project_id: &str,
    publication: &IndexPublicationRecord,
    inputs: CheckedIntegrationInputs<'_>,
    read_source: R,
) -> Result<CheckedBuiltCallPathIntegration, ApiError>
where
    R: FnMut(&Path) -> io::Result<Vec<u8>>,
{
    let built = build_from_store(
        store,
        project_root,
        project_id,
        publication,
        inputs.contract,
        read_source,
    )?;
    check_built_call_path_integration(inputs.contract, inputs.hashes, inputs.rendering, built)
        .map_err(|error| {
            ApiError::internal(format!(
                "indexed call-path checked integration failed: {error:?}"
            ))
        })
}

#[derive(Debug)]
enum SelectorResolution {
    Resolved(ResolvedNodeIdentity),
    Missing,
    Ambiguous,
    NonCallable,
    Unavailable(UnavailableReason),
}

struct SelectorContext<'a> {
    store: &'a Store,
    project_root: &'a Path,
    project_id: &'a str,
    publication: &'a IndexPublicationRecord,
    files: &'a [FileInfo],
}

fn resolve_scope_selector(
    context: &SelectorContext<'_>,
    selector: &ExactScopeSelector,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    match selector {
        ExactScopeSelector::PinnedNode(identity) => resolve_pinned(context, identity, identities),
        ExactScopeSelector::CanonicalId(value) => resolve_canonical(context, value, identities),
        ExactScopeSelector::QualifiedName {
            qualified_name,
            project_file_components,
        } => resolve_qualified(
            context,
            qualified_name,
            project_file_components.as_deref(),
            identities,
        ),
    }
}

fn resolve_symbol_selector(
    context: &SelectorContext<'_>,
    selector: &ExactSymbolSelector,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    match selector {
        ExactSymbolSelector::PinnedNode(identity) => resolve_pinned(context, identity, identities),
        ExactSymbolSelector::CanonicalId(value) => resolve_canonical(context, value, identities),
        ExactSymbolSelector::QualifiedName {
            qualified_name,
            project_file_components,
        } => resolve_qualified(
            context,
            qualified_name,
            project_file_components.as_deref(),
            identities,
        ),
    }
}

fn resolve_pinned(
    context: &SelectorContext<'_>,
    identity: &PinnedNodeIdentity,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    if identity.project_id != context.project_id
        || identity.core_generation_id != context.publication.generation_id
        || identity.core_run_id != context.publication.run_id
    {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::PublicationPinMismatch,
        ));
    }
    let Some(node_id) = parse_pinned_node_id(identity) else {
        return Ok(SelectorResolution::Missing);
    };
    let Some(node) = context.store.get_node(node_id).map_err(store_error)? else {
        return Ok(SelectorResolution::Missing);
    };
    resolved_node(context, node, identities)
}

fn resolve_canonical(
    context: &SelectorContext<'_>,
    canonical_id: &str,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    let matches = context
        .store
        .node_ids_by_canonical_ids(&[canonical_id.to_owned()])
        .map_err(store_error)?
        .remove(canonical_id)
        .unwrap_or_default();
    resolve_unique_node(context, matches, identities)
}

fn resolve_qualified(
    context: &SelectorContext<'_>,
    qualified_name: &str,
    components: Option<&[String]>,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    let selected_files = match components {
        Some(components) => {
            let requested = components.iter().collect::<PathBuf>();
            let Ok(ProjectRelativePathResolution::Existing { absolute, .. }) =
                resolve_project_relative_path(context.project_root, &requested)
            else {
                return Ok(SelectorResolution::Unavailable(
                    UnavailableReason::SourceNotBoundToPublication,
                ));
            };
            let Ok(wanted) = identities.resolve(&absolute) else {
                return Ok(SelectorResolution::Unavailable(
                    UnavailableReason::SourceNotBoundToPublication,
                ));
            };
            context
                .files
                .iter()
                .filter(|file| {
                    stored_absolute(context.project_root, &file.path)
                        .and_then(|path| identities.resolve(&path).ok())
                        .is_some_and(|identity| identity == wanted)
                })
                .collect::<Vec<_>>()
        }
        None => {
            let mut files = context.files.iter().collect::<Vec<_>>();
            files.sort_by_key(|file| file.id);
            files
        }
    };
    if selected_files.is_empty() {
        return Ok(SelectorResolution::Missing);
    }
    let mut matches = BTreeSet::new();
    for file in selected_files {
        let Some(path) = file.path.to_str() else {
            return Ok(SelectorResolution::Unavailable(
                UnavailableReason::SourceNotBoundToPublication,
            ));
        };
        for kind in CALLABLE_KINDS {
            matches.extend(
                context
                    .store
                    .node_ids_by_file_identity_qualified_name_and_kind(path, qualified_name, kind)
                    .map_err(store_error)?,
            );
        }
    }
    resolve_unique_node(context, matches.into_iter().collect(), identities)
}

fn resolve_unique_node(
    context: &SelectorContext<'_>,
    mut matches: Vec<NodeId>,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [] => Ok(SelectorResolution::Missing),
        [node_id] => match context.store.get_node(*node_id).map_err(store_error)? {
            Some(node) => resolved_node(context, node, identities),
            None => Ok(SelectorResolution::Missing),
        },
        _ => Ok(SelectorResolution::Ambiguous),
    }
}

fn resolved_node(
    context: &SelectorContext<'_>,
    node: Node,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    if !is_callable(node.kind) {
        return Ok(SelectorResolution::NonCallable);
    }
    let Some(file_id) = node.file_node_id else {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    };
    let Some(file) = context.files.iter().find(|file| file.id == file_id.0) else {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    };
    let Some(absolute) = stored_absolute(context.project_root, &file.path) else {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    };
    if identities.resolve(&absolute).is_err() {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    }
    let Some(relative) = workspace_relative_path(context.project_root, &absolute) else {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    };
    let Some(components) = relative
        .iter()
        .map(|part| part.to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
    else {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    };
    Ok(SelectorResolution::Resolved(ResolvedNodeIdentity {
        pinned: PinnedNodeIdentity {
            project_id: context.project_id.to_owned(),
            core_generation_id: context.publication.generation_id.clone(),
            core_run_id: context.publication.run_id.clone(),
            node_id: node.id.0.to_string(),
        },
        canonical_id: node
            .canonical_id
            .unwrap_or_else(|| format!("node:{}", node.id.0)),
        qualified_name: node.qualified_name.unwrap_or(node.serialized_name),
        project_file_components: components,
    }))
}

fn authenticate_containment(
    store: &Store,
    selected_source: &Node,
    file_node_id: NodeId,
    line: u32,
) -> Result<Result<CallableContainmentEvidence, ContainmentFailure>, ApiError> {
    let projections = store
        .get_callable_projection_states_for_file(file_node_id.0)
        .map_err(store_error)?;
    let ids = projections
        .iter()
        .map(|projection| projection.node_id)
        .collect::<Vec<_>>();
    let nodes = store.get_nodes_by_ids(&ids).map_err(store_error)?;
    let mut candidates = projections
        .into_iter()
        .filter_map(|projection| {
            let node = nodes.get(&projection.node_id)?;
            (is_callable(node.kind)
                && node.file_node_id == Some(file_node_id)
                && projection.file_id == file_node_id.0
                && projection.start_line <= line
                && line <= projection.end_line)
                .then_some(projection)
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|projection| {
        (
            projection.end_line.saturating_sub(projection.start_line),
            projection.node_id,
        )
    });
    let Some(smallest) = candidates.first() else {
        return Ok(Err(ContainmentFailure::Missing));
    };
    let smallest_span = smallest.end_line.saturating_sub(smallest.start_line);
    if candidates
        .iter()
        .take_while(|candidate| {
            candidate.end_line.saturating_sub(candidate.start_line) == smallest_span
        })
        .count()
        != 1
        || smallest.node_id != selected_source.id
    {
        return Ok(Err(ContainmentFailure::Ambiguous));
    }
    Ok(Ok(CallableContainmentEvidence {
        file_node_id,
        owner_node_id: smallest.node_id,
        start_line: smallest.start_line,
        end_line: smallest.end_line,
    }))
}

struct BoundSource {
    bytes: Vec<u8>,
    observed_sha256: String,
    project_file_components: Vec<String>,
}

enum SourceObservation {
    Bound(BoundSource),
    ReadFailed,
    InvalidUtf8 { observed_sha256: String },
}

fn bind_source_once<'a, R>(
    project_root: &Path,
    file: &FileInfo,
    indexed_hash: &str,
    identities: &mut OperationPathIdentityResolver,
    cache: &'a mut HashMap<WorkspacePathIdentity, SourceObservation>,
    read_source: &mut R,
) -> Result<&'a BoundSource, SourceBindingFailure>
where
    R: FnMut(&Path) -> io::Result<Vec<u8>>,
{
    let absolute = stored_absolute(project_root, &file.path)
        .ok_or(SourceBindingFailure::WorkingTreeReadFailed)?;
    let ProjectRelativePathResolution::Existing { absolute, relative } =
        resolve_project_relative_path(project_root, &absolute)
            .map_err(|_| SourceBindingFailure::WorkingTreeReadFailed)?
    else {
        return Err(SourceBindingFailure::WorkingTreeReadFailed);
    };
    let identity = identities
        .resolve(&absolute)
        .map_err(|_| SourceBindingFailure::WorkingTreeReadFailed)?;
    if !cache.contains_key(&identity) {
        let observation = match read_source(&absolute) {
            Err(_) => SourceObservation::ReadFailed,
            Ok(bytes) => {
                let observed_sha256 = sha256_hex(&bytes);
                if std::str::from_utf8(&bytes).is_err() {
                    SourceObservation::InvalidUtf8 { observed_sha256 }
                } else {
                    let Some(project_file_components) = relative
                        .iter()
                        .map(|part| part.to_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                    else {
                        cache.insert(identity.clone(), SourceObservation::ReadFailed);
                        return Err(SourceBindingFailure::WorkingTreeReadFailed);
                    };
                    SourceObservation::Bound(BoundSource {
                        bytes,
                        observed_sha256,
                        project_file_components,
                    })
                }
            }
        };
        cache.insert(identity.clone(), observation);
    }
    let Some(observation) = cache.get(&identity) else {
        return Err(SourceBindingFailure::WorkingTreeReadFailed);
    };
    match observation {
        SourceObservation::Bound(bound) if bound.observed_sha256 == indexed_hash => Ok(bound),
        SourceObservation::Bound(_) => Err(SourceBindingFailure::WorkingTreeHashMismatch),
        SourceObservation::ReadFailed => Err(SourceBindingFailure::WorkingTreeReadFailed),
        SourceObservation::InvalidUtf8 { observed_sha256 } if observed_sha256 != indexed_hash => {
            Err(SourceBindingFailure::WorkingTreeHashMismatch)
        }
        SourceObservation::InvalidUtf8 { .. } => Err(SourceBindingFailure::InvalidUtf8),
    }
}

fn complete_line(bytes: &[u8], requested_line: u32) -> Option<(usize, usize, String)> {
    if requested_line == 0 {
        return None;
    }
    let mut line = 1_u32;
    let mut start = 0_usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            if line == requested_line {
                let end = index + 1;
                return Some((
                    start,
                    end,
                    std::str::from_utf8(&bytes[start..end]).ok()?.to_owned(),
                ));
            }
            line += 1;
            start = index + 1;
        }
    }
    if line == requested_line && start < bytes.len() {
        return Some((
            start,
            bytes.len(),
            std::str::from_utf8(&bytes[start..]).ok()?.to_owned(),
        ));
    }
    None
}

fn receipt_id(
    project_id: &str,
    publication: &IndexPublicationRecord,
    admitted: &AdmittedRawCallEdge,
    source_id: i64,
    target_id: i64,
    indexed_hash: &str,
    resolution_fact: &CallResolutionFact,
) -> String {
    let mut digest = Sha256::new();
    digest.update(RECEIPT_DOMAIN);
    for part in [
        project_id.as_bytes(),
        publication.generation_id.as_bytes(),
        publication.run_id.as_bytes(),
        &admitted.edge_id.0.to_le_bytes(),
        &source_id.to_le_bytes(),
        &target_id.to_le_bytes(),
        &admitted.file_node_id.0.to_le_bytes(),
        indexed_hash.as_bytes(),
        &admitted.line.to_le_bytes(),
        admitted.callsite_identity.as_bytes(),
        resolution_fact.fact_id.as_bytes(),
        resolution_fact.provenance.evidence_sha256.as_bytes(),
        &resolution_fact.callsite.start_byte.to_le_bytes(),
    ] {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    format!("indexed-call-edge:{:x}", digest.finalize())
}

fn resolution_fact_matches_raw_edge(
    fact: &CallResolutionFact,
    admitted: &AdmittedRawCallEdge,
    source: NodeId,
    target: NodeId,
) -> bool {
    fact.status == ProofResolutionStatus::Exact
        && fact.edge_id == Some(admitted.edge_id)
        && fact.caller == source
        && fact.target == Some(target)
        && fact.raw_edge_target == Some(admitted.raw_target)
        && fact.raw_callsite_identity.as_deref() == Some(admitted.callsite_identity.as_str())
        && fact.callsite.file_id.0 == admitted.file_node_id.0
        && fact.callsite.line == admitted.line
        && fact.lookup_domain_complete
}

fn stored_absolute(project_root: &Path, stored: &Path) -> Option<PathBuf> {
    let requested = if stored.is_absolute() {
        stored.to_path_buf()
    } else {
        project_root.join(stored)
    };
    match resolve_project_relative_path(project_root, &requested).ok()? {
        ProjectRelativePathResolution::Existing { absolute, .. } => Some(absolute),
        _ => None,
    }
}

fn parse_pinned_node_id(identity: &PinnedNodeIdentity) -> Option<NodeId> {
    let value = identity.node_id.parse::<i64>().ok()?;
    (value.to_string() == identity.node_id).then_some(NodeId(value))
}

fn is_callable(kind: NodeKind) -> bool {
    CALLABLE_KINDS.contains(&kind)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn store_error(error: codestory_store::StorageError) -> ApiError {
    ApiError::internal(format!("indexed call-path Store read failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use codestory_agent::proof_qualification_test_support::{
        ClauseAnchor, ClauseClassification, ProofContractField, ProofDisposition, ProofGap,
        Refutation, UnvalidatedCallPathContract, UnvalidatedCallPathSpec,
        UnvalidatedDirectCallStep, UnvalidatedExactScopeSelector, UnvalidatedExactSymbolSelector,
        ValidatedContractRendering, ValidationOutcome, check_call_path, validate_contract,
    };
    use codestory_contracts::api::IndexMode;
    use codestory_contracts::events::EventBus;
    use codestory_contracts::graph::{CallableProjectionState, Edge, EdgeId, EdgeKind, Node};
    use codestory_contracts::proof_resolution::{
        CallResolutionFact, CalleeForm, DependencyFileHash, EXACT_CALL_RESOLUTION_ALGORITHM,
        ExactCallsite, FileId, INTERNAL_RESOLUTION_PRODUCER, PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
        ProofResolutionAdapter, ProofResolutionProjection, ProofResolutionReason,
        ResolutionEvidence, ResolutionProvenance,
    };
    use codestory_indexer::{
        WorkspaceIndexer, build_proof_resolution_funnel, rematerialize_proof_resolution_projection,
    };
    use codestory_store::{FileInfo, FileRole, IndexPublicationMode, seal_call_resolution_fact};
    use codestory_workspace::{BuildMode, RefreshInfo};
    use serde_json::json;
    use tempfile::TempDir;

    struct Fixture {
        _root: TempDir,
        root: PathBuf,
        store: Store,
        publication: IndexPublicationRecord,
        project_id: String,
        contract: ValidatedCallPathContract,
        source_path: PathBuf,
    }

    fn canonical(value: &str) -> UnvalidatedExactSymbolSelector {
        UnvalidatedExactSymbolSelector::CanonicalId(value.to_owned())
    }

    fn validated_contract(
        start: &str,
        targets: &[&str],
    ) -> (
        ValidatedCallPathContract,
        ProofHashes,
        ValidatedContractRendering,
    ) {
        validated_contract_with_policies(start, targets, &[], &[])
    }

    fn validated_contract_with_policies(
        start: &str,
        targets: &[&str],
        traversal_prohibitions: &[&str],
        projection_exclusions: &[&str],
    ) -> (
        ValidatedCallPathContract,
        ProofHashes,
        ValidatedContractRendering,
    ) {
        let source = "exact direct ordered call path";
        let mut clauses = vec![ClauseAnchor {
            clause_id: "start".to_owned(),
            start: 0,
            end: source.len(),
            quote: source.to_owned(),
            classification: ClauseClassification::ResolvedMaterial {
                fields: vec![ProofContractField::Start],
            },
        }];
        for (index, _) in targets.iter().enumerate() {
            let step = u8::try_from(index).unwrap();
            clauses.push(ClauseAnchor {
                clause_id: format!("step-{index}"),
                start: 0,
                end: source.len(),
                quote: source.to_owned(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![
                        ProofContractField::StepTarget { step },
                        ProofContractField::Directness { step },
                        ProofContractField::Ordering { step },
                        ProofContractField::Relation { step },
                    ],
                },
            });
        }
        for (index, _) in traversal_prohibitions.iter().enumerate() {
            clauses.push(ClauseAnchor {
                clause_id: format!("traversal-{index}"),
                start: 0,
                end: source.len(),
                quote: source.to_owned(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![ProofContractField::TraversalProhibition {
                        index: u8::try_from(index).unwrap(),
                    }],
                },
            });
        }
        for (index, _) in projection_exclusions.iter().enumerate() {
            clauses.push(ClauseAnchor {
                clause_id: format!("projection-{index}"),
                start: 0,
                end: source.len(),
                quote: source.to_owned(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![ProofContractField::ProjectionExclusion {
                        index: u8::try_from(index).unwrap(),
                    }],
                },
            });
        }
        let input = UnvalidatedCallPathContract::new(
            source,
            clauses,
            UnvalidatedCallPathSpec {
                start: canonical(start),
                steps: targets
                    .iter()
                    .map(|target| UnvalidatedDirectCallStep {
                        target: canonical(target),
                    })
                    .collect(),
                prohibit_traversal_through: traversal_prohibitions
                    .iter()
                    .map(|scope| UnvalidatedExactScopeSelector::CanonicalId((*scope).to_owned()))
                    .collect(),
                exclude_from_projection: projection_exclusions
                    .iter()
                    .map(|scope| UnvalidatedExactScopeSelector::CanonicalId((*scope).to_owned()))
                    .collect(),
            },
        );
        match validate_contract(input).unwrap() {
            ValidationOutcome::Validated {
                contract,
                hashes,
                rendering,
            } => (*contract, hashes, rendering),
            other => panic!("expected validated contract, got {other:?}"),
        }
    }

    fn contract(start: &str, targets: &[&str]) -> ValidatedCallPathContract {
        validated_contract(start, targets).0
    }

    fn node(
        id: i64,
        kind: NodeKind,
        name: &str,
        canonical_id: &str,
        file_id: i64,
        start_line: u32,
        end_line: u32,
    ) -> Node {
        Node {
            id: NodeId(id),
            kind,
            serialized_name: name.to_owned(),
            qualified_name: Some(format!("crate::{name}")),
            canonical_id: Some(canonical_id.to_owned()),
            file_node_id: Some(NodeId(file_id)),
            start_line: Some(start_line),
            start_col: Some(1),
            end_line: Some(end_line),
            end_col: Some(1),
        }
    }

    fn projection(
        file_id: i64,
        node_id: i64,
        start_line: u32,
        end_line: u32,
    ) -> CallableProjectionState {
        CallableProjectionState {
            file_id,
            symbol_key: format!("symbol-{node_id}"),
            node_id: NodeId(node_id),
            signature_hash: node_id,
            normalized_signature: Some(format!("signature-{node_id}")),
            body_hash: node_id,
            start_line,
            end_line,
        }
    }

    fn fixture(bytes: &[u8]) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let source_path = root.join("src/lib.rs");
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(&source_path, bytes).unwrap();
        let mut store = Store::new_in_memory().unwrap();
        let file = FileInfo {
            id: 1,
            path: source_path.clone(),
            language: "rust".to_owned(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 2,
            file_role: FileRole::Source,
        };
        store.insert_file(&file).unwrap();
        store
            .update_file_metadata(&file, Some(&sha256_hex(bytes)))
            .unwrap();
        store
            .insert_node(&Node {
                id: NodeId(1),
                kind: NodeKind::FILE,
                serialized_name: source_path.to_string_lossy().into_owned(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: None,
                start_line: Some(1),
                start_col: Some(1),
                end_line: Some(2),
                end_col: Some(1),
            })
            .unwrap();
        store
            .insert_node(&node(2, NodeKind::FUNCTION, "source", "source-id", 1, 1, 1))
            .unwrap();
        store
            .insert_node(&node(3, NodeKind::FUNCTION, "target", "target-id", 1, 2, 2))
            .unwrap();
        store
            .insert_node(&Node {
                id: NodeId(30),
                kind: NodeKind::UNKNOWN,
                serialized_name: "target".to_owned(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: Some(NodeId(1)),
                start_line: Some(1),
                start_col: Some(1),
                end_line: Some(1),
                end_col: Some(1),
            })
            .unwrap();
        store
            .insert_edge(&Edge {
                id: EdgeId(10),
                source: NodeId(2),
                target: NodeId(30),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(3)),
                callsite_identity: Some("1:1:1:30|rust".to_owned()),
                candidate_targets: Vec::new(),
                ..Default::default()
            })
            .unwrap();
        store
            .upsert_callable_projection_states(&[
                projection(1, 1, 1, 2),
                projection(1, 2, 1, 1),
                projection(1, 3, 2, 2),
            ])
            .unwrap();
        let publication = IndexPublicationRecord {
            generation: 4,
            generation_id: "generation-4".to_owned(),
            run_id: "run-4".to_owned(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        publish_manual_resolution_facts(&mut store, &publication, &[EdgeId(10)]);
        let project_id = project_identity_v3(&root).project_id;
        Fixture {
            _root: temp,
            root,
            store,
            publication,
            project_id,
            contract: contract("source-id", &["target-id"]),
            source_path,
        }
    }

    fn publish_manual_resolution_facts(
        store: &mut Store,
        publication: &IndexPublicationRecord,
        edge_ids: &[EdgeId],
    ) {
        let nodes = store
            .get_nodes()
            .unwrap()
            .into_iter()
            .map(|node| (node.id, node))
            .collect::<HashMap<_, _>>();
        let facts: Vec<CallResolutionFact> = store
            .get_edges()
            .unwrap()
            .into_iter()
            .filter(|edge| edge_ids.contains(&edge.id))
            .map(|edge| {
                let file_id = edge.file_node_id.unwrap();
                let source_sha256 = store.get_file_content_hash(file_id.0).unwrap().unwrap();
                let caller = edge.effective_source();
                let target = edge.effective_target();
                let column_or_ordinal: u32 = edge
                    .callsite_identity
                    .as_deref()
                    .and_then(|identity| identity.split(':').nth(2))
                    .and_then(|column| column.parse().ok())
                    .unwrap();
                let column = store
                    .get_node(edge.target)
                    .unwrap()
                    .and_then(|node| node.start_col)
                    .unwrap_or(column_or_ordinal);
                let raw_target = nodes
                    .get(&edge.target)
                    .map(|node| node.serialized_name.clone())
                    .unwrap_or_else(|| "target".to_owned());
                let file_path = store
                    .get_files()
                    .unwrap()
                    .into_iter()
                    .find(|file| file.id == file_id.0)
                    .unwrap()
                    .path;
                let source = fs::read(file_path).unwrap();
                let call_line = edge.line.unwrap();
                let line_start = if call_line == 1 {
                    0
                } else {
                    source
                        .iter()
                        .enumerate()
                        .filter(|(_, byte)| **byte == b'\n')
                        .nth(call_line.saturating_sub(2) as usize)
                        .map_or(0, |(index, _)| index + 1)
                };
                let expected_start = line_start + column.saturating_sub(1) as usize;
                let line_end = source[line_start..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map(|offset| line_start + offset)
                    .unwrap_or(source.len());
                let occurrences = source[line_start..line_end]
                    .windows(raw_target.len())
                    .enumerate()
                    .filter_map(|(offset, window)| {
                        (window == raw_target.as_bytes()).then_some(line_start + offset)
                    })
                    .collect::<Vec<_>>();
                let start_byte = if occurrences.len() > 1 {
                    occurrences
                        .get(column_or_ordinal.saturating_sub(1) as usize)
                        .copied()
                        .unwrap_or(expected_start)
                } else if source.get(expected_start..expected_start + raw_target.len())
                    == Some(raw_target.as_bytes())
                {
                    expected_start
                } else {
                    occurrences.first().copied().unwrap_or(0)
                } as u64;
                seal_call_resolution_fact(CallResolutionFact {
                    fact_id: String::new(),
                    edge_id: Some(edge.id),
                    raw_edge_target: Some(edge.target),
                    raw_callsite_identity: edge.callsite_identity.clone(),
                    callsite: ExactCallsite {
                        file_id: FileId(file_id.0),
                        source_sha256: source_sha256.clone(),
                        start_byte,
                        end_byte_exclusive: start_byte + raw_target.len() as u64,
                        line: edge.line.unwrap(),
                        column,
                        callee_form: CalleeForm::Identifier,
                        raw_target,
                    },
                    caller,
                    target: Some(target),
                    status: ProofResolutionStatus::Exact,
                    reason: ProofResolutionReason::ExactResolution,
                    evidence_chain: vec![ResolutionEvidence::SameFileDeclaration {
                        declaration: target,
                    }],
                    lookup_domain_complete: true,
                    provenance: ResolutionProvenance {
                        producer: INTERNAL_RESOLUTION_PRODUCER.to_owned(),
                        fact_schema_version: PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
                        algorithm: EXACT_CALL_RESOLUTION_ALGORITHM.to_owned(),
                        language_adapter: "rust".to_owned(),
                        language_adapter_version: "test-v1".to_owned(),
                        parser_fingerprint: "2".repeat(64),
                        dependency_file_hashes: vec![DependencyFileHash {
                            file_id: FileId(file_id.0),
                            source_sha256,
                        }],
                        evidence_sha256: String::new(),
                    },
                })
                .unwrap()
            })
            .collect();
        let funnel = build_proof_resolution_funnel(&facts);
        store
            .replace_proof_resolution_projection(
                publication,
                &ProofResolutionProjection {
                    adapter_roster: vec![ProofResolutionAdapter {
                        language: "rust".to_owned(),
                        adapter_version: "test-v1".to_owned(),
                    }],
                    facts,
                    funnel,
                },
            )
            .unwrap();
    }

    fn add_duplicate_call_edges(fixture: &mut Fixture, count: u32) {
        for offset in 0..count {
            fixture
                .store
                .insert_edge(&Edge {
                    id: EdgeId(100 + i64::from(offset)),
                    source: NodeId(2),
                    target: NodeId(3),
                    kind: EdgeKind::CALL,
                    file_node_id: Some(NodeId(1)),
                    line: Some(1),
                    resolved_source: Some(NodeId(2)),
                    resolved_target: Some(NodeId(3)),
                    callsite_identity: Some(format!("1:1:{}:3|rust", offset + 1)),
                    candidate_targets: Vec::new(),
                    ..Default::default()
                })
                .unwrap();
        }
    }

    struct SourceBuiltFixture {
        _root: TempDir,
        root: PathBuf,
        source_path: PathBuf,
        store: Store,
        publication: IndexPublicationRecord,
        project_id: String,
    }

    fn source_built_fixture(source: &str) -> SourceBuiltFixture {
        source_built_fixture_with_extension("rs", source)
    }

    fn source_built_fixture_with_extension(extension: &str, source: &str) -> SourceBuiltFixture {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let source_path = root.join(format!("src/lib.{extension}"));
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(&source_path, source).unwrap();
        let mut store = Store::new_in_memory().unwrap();
        WorkspaceIndexer::new(root.clone())
            .run_incremental(
                &mut store,
                &RefreshInfo {
                    mode: BuildMode::Incremental,
                    files_to_index: vec![source_path.clone()],
                    files_to_remove: Vec::new(),
                    existing_file_ids: HashMap::new(),
                },
                &EventBus::new(),
                None,
            )
            .unwrap();
        let publication = IndexPublicationRecord {
            generation: 1,
            generation_id: "source-built-generation-1".to_owned(),
            run_id: "source-built-run-1".to_owned(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        rematerialize_proof_resolution_projection(&mut store, &publication).unwrap();
        SourceBuiltFixture {
            _root: temp,
            root: root.clone(),
            source_path,
            store,
            publication,
            project_id: project_identity_v3(&root).project_id,
        }
    }

    fn store_with_adversarial_call_edge_ids(fixture: &SourceBuiltFixture) -> Store {
        let mut edges = fixture.store.get_edges().unwrap();
        let mut call_offsets = edges
            .iter()
            .enumerate()
            .filter(|(_, edge)| edge.kind == EdgeKind::CALL)
            .map(|(offset, edge)| {
                let RawCallEdgeAdmission::Admitted(admitted) =
                    admit_raw_call_edge(edge, edge.effective_source(), edge.effective_target())
                else {
                    panic!("source-built call edge must be admitted: {edge:?}");
                };
                (offset, admitted.column_or_ordinal)
            })
            .collect::<Vec<_>>();
        call_offsets.sort_by_key(|(_, ordinal)| *ordinal);
        assert_eq!(call_offsets.len(), 2, "two real source-built calls");
        assert!(
            call_offsets[0].1 < call_offsets[1].1,
            "the real indexer must authenticate exact source occurrence order"
        );
        edges[call_offsets[0].0].id = EdgeId(200);
        edges[call_offsets[1].0].id = EdgeId(100);

        let mut store = Store::new_in_memory().unwrap();
        let files = fixture.store.files().get_files().unwrap();
        for file in &files {
            store.insert_file(file).unwrap();
            let content_hash = fixture.store.get_file_content_hash(file.id).unwrap();
            store
                .update_file_metadata(file, content_hash.as_deref())
                .unwrap();
        }
        store
            .insert_nodes_batch(&fixture.store.get_nodes().unwrap())
            .unwrap();
        store.insert_edges_batch(&edges).unwrap();
        store
            .insert_occurrences_batch(&fixture.store.get_occurrences().unwrap())
            .unwrap();
        for file in files {
            let projections = fixture
                .store
                .get_callable_projection_states_for_file(file.id)
                .unwrap();
            store
                .upsert_callable_projection_states(&projections)
                .unwrap();
        }
        let call_edge_ids = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::CALL)
            .map(|edge| edge.id)
            .collect::<Vec<_>>();
        publish_manual_resolution_facts(&mut store, &fixture.publication, &call_edge_ids);
        store
    }

    fn source_callable(store: &Store, terminal_name: &str) -> Node {
        let nodes = store.get_nodes().unwrap();
        let mut matches = nodes
            .iter()
            .filter(|node| {
                is_callable(node.kind)
                    && (node.serialized_name == terminal_name
                        || node.qualified_name.as_deref().is_some_and(|qualified| {
                            qualified.rsplit("::").next() == Some(terminal_name)
                        }))
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "source fixture must have one exact callable named {terminal_name}: matches={matches:?}, nodes={nodes:?}"
        );
        matches.remove(0)
    }

    fn canonical_id(node: &Node) -> &str {
        node.canonical_id
            .as_deref()
            .expect("source-built callable has a canonical ID")
    }

    fn source_built_census(store: &Store) -> (usize, usize) {
        let call_rows = store
            .get_edges()
            .unwrap()
            .into_iter()
            .filter(|edge| edge.kind == EdgeKind::CALL)
            .collect::<Vec<_>>();
        let admitted = call_rows
            .iter()
            .filter(|edge| {
                matches!(
                    admit_raw_call_edge(edge, edge.effective_source(), edge.effective_target()),
                    RawCallEdgeAdmission::Admitted(_)
                )
            })
            .count();
        (call_rows.len(), admitted)
    }

    #[test]
    fn observed_runtime_builder_preserves_source_built_product_results_and_prefixes() {
        let fixture = source_built_fixture(
            "pub fn step0() { step1(); }\npub fn step1() {}\npub fn step2() {}\n",
        );
        let step0 = source_callable(&fixture.store, "step0");
        let step1 = source_callable(&fixture.store, "step1");
        let step2 = source_callable(&fixture.store, "step2");
        let (contract, hashes, rendering) = validated_contract(
            canonical_id(&step0),
            &[canonical_id(&step1), canonical_id(&step2)],
        );

        let product_built = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &contract,
            |path| fs::read(path),
        )
        .unwrap();
        let observed = build_from_store_observed(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(observed.built, product_built);
        assert!(!observed.trace.selector_early_return);
        assert_eq!(observed.trace.steps.len(), 2);
        assert!(matches!(
            &observed.trace.steps[0].outcome,
            StepQualificationOutcome::Admitted { edge_ids } if edge_ids.len() == 1
        ));
        assert_eq!(
            observed.trace.steps[1].outcome,
            StepQualificationOutcome::FirstZeroSurvivor {
                gate: CandidateGate::RawAdmission,
                histogram: Vec::new(),
            }
        );

        let product_integration =
            check_built_call_path_integration(&contract, &hashes, &rendering, product_built)
                .unwrap();
        let product_projection = project_internal_call_path_result(&product_integration).unwrap();
        let finalized = finalize_observed_call_path(&contract, &hashes, &rendering, observed);
        assert_eq!(
            finalized.trace.finalization,
            FinalizationTrace::Complete {
                projection_bytes: match &product_projection {
                    InternalProjection::Complete {
                        serialized_size, ..
                    } => *serialized_size,
                    other => panic!("partial fixture must fit: {other:?}"),
                },
            }
        );
        let observed_result = finalized.result.unwrap();
        assert_eq!(observed_result.integration, product_integration);
        assert_eq!(observed_result.projection, product_projection);
        assert!(matches!(
            observed_result.integration.disposition(),
            ProofDisposition::Unknown {
                connected_receipts,
                gaps,
                ..
            } if connected_receipts.len() == 1
                && gaps == &[ProofGap::FactBuild(FactBuildGap::DirectCallMissing {
                    step_index: 1,
                })]
        ));
    }

    #[test]
    fn observed_runtime_builder_bounds_candidate_queries_without_partial_proof() {
        let mut exact = fixture(b"fn source() { target(); }\nfn target() {}\n");
        add_duplicate_call_edges(&mut exact, MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP - 1);
        let exact_observed = build_from_store_observed(
            &exact.store,
            &exact.root,
            &exact.project_id,
            &exact.publication,
            &exact.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            exact_observed.trace.steps[0].candidate_edge_ids.len(),
            MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP as usize
        );
        assert_eq!(exact_observed.built.receipts.len(), 1);
        assert!(matches!(
            &exact_observed.trace.steps[0].outcome,
            StepQualificationOutcome::Admitted { edge_ids }
                if edge_ids == &[EdgeId(10)]
        ));

        let mut over = fixture(b"fn source() { target(); }\nfn target() {}\n");
        add_duplicate_call_edges(&mut over, MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP);
        let over_observed = build_from_store_observed(
            &over.store,
            &over.root,
            &over.project_id,
            &over.publication,
            &over.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            over_observed.trace.steps[0].candidate_edge_ids.len(),
            MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP as usize
        );
        assert_eq!(
            over_observed.trace.steps[0].outcome,
            StepQualificationOutcome::CandidateLimitExceeded {
                maximum_candidate_edges: MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP,
                observed_candidate_edges_at_least: MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP + 1,
            }
        );
        assert!(over_observed.built.receipts.is_empty());
        assert!(over_observed.built.facts.is_empty());
        assert_eq!(
            over_observed.built.unavailable,
            vec![UnavailableReason::ProofFactsUnavailable]
        );
    }

    #[test]
    fn observed_runtime_trace_reports_selector_and_first_zero_survivor_gates() {
        let selector_fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        let selector_contract = contract("missing-id", &["target-id"]);
        let selector_product = build_from_store(
            &selector_fixture.store,
            &selector_fixture.root,
            &selector_fixture.project_id,
            &selector_fixture.publication,
            &selector_contract,
            |path| fs::read(path),
        )
        .unwrap();
        let selector_observed = build_from_store_observed(
            &selector_fixture.store,
            &selector_fixture.root,
            &selector_fixture.project_id,
            &selector_fixture.publication,
            &selector_contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(selector_observed.built, selector_product);
        assert!(selector_observed.trace.selector_early_return);
        assert_eq!(
            selector_observed.trace.selectors[0].outcome,
            SelectorGateOutcome::Failed(SelectorFailure::Missing)
        );
        assert_eq!(
            selector_observed.trace.steps.len(),
            selector_contract.spec().steps().len(),
            "selector early returns must classify every attempted positive step"
        );
        assert_eq!(
            selector_observed.trace.steps[0].outcome,
            StepQualificationOutcome::SelectorBlocked {
                selector_index: 0,
                outcome: SelectorGateOutcome::Failed(SelectorFailure::Missing),
            }
        );

        let mut raw_fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        raw_fixture
            .store
            .insert_node(&node(4, NodeKind::FUNCTION, "other", "other-id", 1, 2, 2))
            .unwrap();
        raw_fixture
            .store
            .upsert_callable_projection_states(&[projection(1, 4, 2, 2)])
            .unwrap();
        for edge in [
            Edge {
                id: EdgeId(30),
                source: NodeId(2),
                target: NodeId(4),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(4)),
                callsite_identity: Some("1:1:0:4|alternatives".to_owned()),
                candidate_targets: vec![NodeId(3)],
                ..Default::default()
            },
            Edge {
                id: EdgeId(20),
                source: NodeId(2),
                target: NodeId(4),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(4)),
                callsite_identity: Some("1:1:0:4|probable".to_owned()),
                candidate_targets: Vec::new(),
                ..Default::default()
            },
        ] {
            raw_fixture.store.insert_edge(&edge).unwrap();
        }
        let raw_contract = contract("source-id", &["other-id"]);
        let raw_product = build_from_store(
            &raw_fixture.store,
            &raw_fixture.root,
            &raw_fixture.project_id,
            &raw_fixture.publication,
            &raw_contract,
            |path| fs::read(path),
        )
        .unwrap();
        let raw_observed = build_from_store_observed(
            &raw_fixture.store,
            &raw_fixture.root,
            &raw_fixture.project_id,
            &raw_fixture.publication,
            &raw_contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(raw_observed.built, raw_product);
        assert_eq!(
            raw_observed.trace.steps[0],
            StepQualificationTrace {
                step_index: 0,
                candidate_edge_ids: vec![EdgeId(10), EdgeId(20), EdgeId(30)],
                outcome: StepQualificationOutcome::FirstZeroSurvivor {
                    gate: CandidateGate::ResolutionFact,
                    histogram: vec![CandidateFailureHistogram {
                        reason: CandidateFailure::ResolutionFact(ResolutionFactFailure::Missing,),
                        edge_ids: vec![EdgeId(20)],
                    }],
                },
            }
        );

        let mut containment_fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        containment_fixture
            .store
            .insert_node(&node(4, NodeKind::METHOD, "inner", "inner-id", 1, 1, 1))
            .unwrap();
        containment_fixture
            .store
            .upsert_callable_projection_states(&[projection(1, 2, 1, 1), projection(1, 4, 1, 1)])
            .unwrap();
        let containment_product = build_from_store(
            &containment_fixture.store,
            &containment_fixture.root,
            &containment_fixture.project_id,
            &containment_fixture.publication,
            &containment_fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        let containment_observed = build_from_store_observed(
            &containment_fixture.store,
            &containment_fixture.root,
            &containment_fixture.project_id,
            &containment_fixture.publication,
            &containment_fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(containment_observed.built, containment_product);
        assert_eq!(
            containment_observed.trace.steps[0].outcome,
            StepQualificationOutcome::FirstZeroSurvivor {
                gate: CandidateGate::Containment,
                histogram: vec![CandidateFailureHistogram {
                    reason: CandidateFailure::Containment(ContainmentFailure::Ambiguous),
                    edge_ids: vec![EdgeId(10)],
                }],
            }
        );

        let source_fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        let file = source_fixture.store.files().get_files().unwrap().remove(0);
        source_fixture
            .store
            .update_file_metadata(&file, None)
            .unwrap();
        let source_product = build_from_store(
            &source_fixture.store,
            &source_fixture.root,
            &source_fixture.project_id,
            &source_fixture.publication,
            &source_fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        let source_observed = build_from_store_observed(
            &source_fixture.store,
            &source_fixture.root,
            &source_fixture.project_id,
            &source_fixture.publication,
            &source_fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(source_observed.built, source_product);
        assert!(source_observed.trace.selector_early_return);
        assert!(source_observed.trace.steps.is_empty());
        assert_eq!(
            source_observed.built.unavailable,
            vec![UnavailableReason::ProofSemanticProjectionUnavailable]
        );

        let mut line_fixture = fixture(b"fn source() { target(); }\nfn target() {}");
        line_fixture
            .store
            .insert_node(&node(4, NodeKind::FUNCTION, "other", "other-id", 1, 2, 2))
            .unwrap();
        line_fixture
            .store
            .insert_edge(&Edge {
                id: EdgeId(14),
                source: NodeId(2),
                target: NodeId(4),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(3),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(4)),
                callsite_identity: Some("1:3:0:4|out-of-range".to_owned()),
                candidate_targets: Vec::new(),
                ..Default::default()
            })
            .unwrap();
        line_fixture
            .store
            .upsert_callable_projection_states(&[projection(1, 2, 1, 3), projection(1, 4, 2, 2)])
            .unwrap();
        let line_contract = contract("source-id", &["other-id"]);
        let line_product = build_from_store(
            &line_fixture.store,
            &line_fixture.root,
            &line_fixture.project_id,
            &line_fixture.publication,
            &line_contract,
            |path| fs::read(path),
        )
        .unwrap();
        let line_observed = build_from_store_observed(
            &line_fixture.store,
            &line_fixture.root,
            &line_fixture.project_id,
            &line_fixture.publication,
            &line_contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(line_observed.built, line_product);
        assert_eq!(
            line_observed.trace.steps[0].outcome,
            StepQualificationOutcome::FirstZeroSurvivor {
                gate: CandidateGate::ResolutionFact,
                histogram: vec![CandidateFailureHistogram {
                    reason: CandidateFailure::ResolutionFact(ResolutionFactFailure::Missing),
                    edge_ids: vec![EdgeId(14)],
                }],
            }
        );
    }

    #[test]
    fn observed_runtime_trace_reports_receipt_and_projection_finalization_failures() {
        let fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        let (contract, hashes, rendering) = validated_contract("source-id", &["target-id"]);
        let observed = build_from_store_observed(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &contract,
            |path| fs::read(path),
        )
        .unwrap();

        let mut receipt_integration = observed.clone();
        receipt_integration.built.receipts[0].receipt.edge_id = "hostile-edge".to_owned();
        assert!(
            check_built_call_path_integration(
                &contract,
                &hashes,
                &rendering,
                receipt_integration.built.clone(),
            )
            .is_err()
        );
        let receipt_integration =
            finalize_observed_call_path(&contract, &hashes, &rendering, receipt_integration);
        assert!(receipt_integration.result.is_err());
        assert_eq!(
            receipt_integration.trace.finalization,
            FinalizationTrace::Failed(FinalizationFailure::ReceiptIntegration)
        );

        let mut receipt_budget = observed.clone();
        receipt_budget.built.receipts[0].callsite_identity = "x".repeat(70_000);
        let receipt_budget =
            finalize_observed_call_path(&contract, &hashes, &rendering, receipt_budget);
        assert!(matches!(
            receipt_budget.result,
            Ok(IntegratedProjectedCallPathResult {
                projection: InternalProjection::BudgetExceeded { .. },
                ..
            })
        ));
        assert_eq!(
            receipt_budget.trace.finalization,
            FinalizationTrace::Failed(FinalizationFailure::ReceiptBudget)
        );

        let (missing_contract, missing_hashes, missing_rendering) =
            validated_contract("missing-id", &["target-id"]);
        let mut projection_budget = build_from_store_observed(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &missing_contract,
            |path| fs::read(path),
        )
        .unwrap();
        projection_budget.built.publication.project_id = "x".repeat(70_000);
        let projection_budget = finalize_observed_call_path(
            &missing_contract,
            &missing_hashes,
            &missing_rendering,
            projection_budget,
        );
        assert!(projection_budget.result.is_err());
        assert_eq!(
            projection_budget.trace.finalization,
            FinalizationTrace::Failed(FinalizationFailure::ProjectionBudget)
        );
    }

    #[test]
    fn source_built_one_and_six_step_paths_integrate_exact_receipts_and_stable_census() {
        let one = source_built_fixture(
            "pub fn callee() -> i32 { 1 }\npub fn caller() -> i32 { callee(); 1 }\n",
        );
        let caller = source_callable(&one.store, "caller");
        let callee = source_callable(&one.store, "callee");
        let (one_contract, one_hashes, one_rendering) =
            validated_contract(canonical_id(&caller), &[canonical_id(&callee)]);
        let one_result = evaluate_from_store(
            &one.store,
            &one.root,
            &one.project_id,
            &one.publication,
            CheckedIntegrationInputs {
                contract: &one_contract,
                hashes: &one_hashes,
                rendering: &one_rendering,
            },
            |path| fs::read(path),
        )
        .unwrap();

        assert_eq!(source_built_census(&one.store), (1, 1));
        eprintln!("source-built admission census: raw_call_rows=1 admitted_rows=1");
        let one_built = one_result.built_facts();
        assert_eq!(one_built.facts.len(), 1);
        assert_eq!(one_built.receipts.len(), 1);
        assert!(one_built.gaps.is_empty());
        assert!(one_built.unavailable.is_empty());
        let receipt = &one_built.receipts[0];
        let raw_edge = one
            .store
            .get_edges()
            .unwrap()
            .into_iter()
            .find(|edge| edge.kind == EdgeKind::CALL)
            .expect("one source-built raw call edge");
        assert_eq!(receipt.source.pinned.node_id, caller.id.0.to_string());
        assert_eq!(receipt.target.pinned.node_id, callee.id.0.to_string());
        assert_eq!(receipt.receipt.edge_id, raw_edge.id.0.to_string());
        assert_eq!(
            receipt.callsite_identity,
            raw_edge.callsite_identity.as_deref().unwrap()
        );
        assert_eq!(receipt.containment.owner_node_id, caller.id);
        assert_eq!(
            receipt.line_window.indexed_sha256,
            sha256_hex(&fs::read(&one.source_path).unwrap())
        );
        assert_eq!(
            receipt.line_window.observed_sha256,
            receipt.line_window.indexed_sha256
        );
        assert_eq!(
            receipt.line_window.text,
            "pub fn caller() -> i32 { callee(); 1 }\n"
        );
        assert_eq!(
            receipt
                .callsite_identity
                .split('|')
                .next()
                .unwrap()
                .split(':')
                .count(),
            4
        );
        assert!(receipt.receipt.receipt_id.starts_with("indexed-call-edge:"));
        assert_eq!(
            one_result.disposition(),
            &ProofDisposition::ContractProven {
                contract_digest: one_hashes.contract_digest().to_owned(),
                receipts: vec![receipt.receipt.clone()],
            }
        );

        let six = source_built_fixture(
            "pub fn step0() { step1(); }\n\
             pub fn step1() { step2(); }\n\
             pub fn step2() { step3(); }\n\
             pub fn step3() { step4(); }\n\
             pub fn step4() { step5(); }\n\
             pub fn step5() { step6(); }\n\
             pub fn step6() {}\n",
        );
        let nodes = (0..=6)
            .map(|index| source_callable(&six.store, &format!("step{index}")))
            .collect::<Vec<_>>();
        let target_ids = nodes[1..].iter().map(canonical_id).collect::<Vec<_>>();
        let (six_contract, six_hashes, six_rendering) =
            validated_contract(canonical_id(&nodes[0]), &target_ids);
        let six_result = evaluate_from_store(
            &six.store,
            &six.root,
            &six.project_id,
            &six.publication,
            CheckedIntegrationInputs {
                contract: &six_contract,
                hashes: &six_hashes,
                rendering: &six_rendering,
            },
            |path| fs::read(path),
        )
        .unwrap();

        assert_eq!(source_built_census(&six.store), (6, 6));
        eprintln!("source-built admission census: raw_call_rows=6 admitted_rows=6");
        let six_built = six_result.built_facts();
        assert_eq!(six_built.facts.len(), 6);
        assert_eq!(six_built.receipts.len(), 6);
        let receipt_ids = six_built
            .receipts
            .iter()
            .map(|receipt| receipt.receipt.receipt_id.as_str())
            .collect::<BTreeSet<_>>();
        let edge_ids = six_built
            .receipts
            .iter()
            .map(|receipt| receipt.receipt.edge_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(receipt_ids.len(), 6);
        assert_eq!(edge_ids.len(), 6);
        for (index, receipt) in six_built.receipts.iter().enumerate() {
            assert_eq!(receipt.source.pinned.node_id, nodes[index].id.0.to_string());
            assert_eq!(
                receipt.target.pinned.node_id,
                nodes[index + 1].id.0.to_string()
            );
            if let Some(next) = six_built.receipts.get(index + 1) {
                assert_eq!(receipt.target, next.source);
            }
        }
        assert!(matches!(
            six_result.disposition(),
            ProofDisposition::ContractProven { receipts, .. } if receipts.len() == 6
        ));
    }

    #[test]
    fn source_built_same_line_calls_use_source_order_before_adversarial_edge_ids() {
        let mut fixture = source_built_fixture_with_extension(
            "rs",
            "fn target(value: i32) -> i32 { value }\n\
             fn source() -> i32 { target(1) + target(2) }\n",
        );
        let source = source_callable(&fixture.store, "source");
        let target = source_callable(&fixture.store, "target");
        fixture.store = store_with_adversarial_call_edge_ids(&fixture);
        let (contract, hashes, rendering) =
            validated_contract(canonical_id(&source), &[canonical_id(&target)]);

        let result = evaluate_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            CheckedIntegrationInputs {
                contract: &contract,
                hashes: &hashes,
                rendering: &rendering,
            },
            |path| fs::read(path),
        )
        .unwrap();

        assert_eq!(result.built_facts().receipts.len(), 2);
        assert!(matches!(
            result.disposition(),
            ProofDisposition::ContractProven { receipts, .. }
                if receipts.len() == 1 && receipts[0].edge_id == "200"
        ));
    }

    #[test]
    fn source_built_hostile_mutations_never_preserve_a_proven_contract() {
        let six = source_built_fixture(
            "pub fn step0() { step1(); }\n\
             pub fn step1() { step2(); }\n\
             pub fn step2() { step3(); }\n\
             pub fn step3() { step4(); }\n\
             pub fn step4() { step5(); }\n\
             pub fn step5() { step6(); }\n\
             pub fn step6() {}\n",
        );
        let nodes = (0..=6)
            .map(|index| source_callable(&six.store, &format!("step{index}")))
            .collect::<Vec<_>>();
        let targets = nodes[1..].iter().map(canonical_id).collect::<Vec<_>>();
        let (contract, hashes, rendering) = validated_contract(canonical_id(&nodes[0]), &targets);
        let integrated = evaluate_from_store(
            &six.store,
            &six.root,
            &six.project_id,
            &six.publication,
            CheckedIntegrationInputs {
                contract: &contract,
                hashes: &hashes,
                rendering: &rendering,
            },
            |path| fs::read(path),
        )
        .unwrap();
        assert!(matches!(
            integrated.disposition(),
            ProofDisposition::ContractProven { .. }
        ));

        let built = integrated.built_facts();
        let mut reversed = built.facts.clone();
        for fact in &mut reversed {
            if let VerifiedProofFact::DirectCall(fact) = fact {
                std::mem::swap(&mut fact.source, &mut fact.target);
            }
        }
        let omitted = built.facts[..5].to_vec();
        let mut changed_target = built.facts.clone();
        let VerifiedProofFact::DirectCall(first_changed) = &mut changed_target[0] else {
            panic!("source-built fact is direct")
        };
        first_changed.target = match &built.facts[1] {
            VerifiedProofFact::DirectCall(next) => next.target.clone(),
            _ => panic!("source-built fact is direct"),
        };
        let mut reused = built.facts.clone();
        let first_receipt = match &reused[0] {
            VerifiedProofFact::DirectCall(fact) => fact.receipt.clone(),
            _ => panic!("source-built fact is direct"),
        };
        let VerifiedProofFact::DirectCall(last) = reused.last_mut().unwrap() else {
            panic!("source-built fact is direct")
        };
        last.receipt = first_receipt;

        for (mutation, facts) in [
            ("reverse", reversed),
            ("omit", omitted),
            ("target", changed_target),
            ("reuse", reused),
        ] {
            assert!(
                !matches!(
                    check_call_path(&contract, &hashes, &facts),
                    ProofDisposition::ContractProven { .. }
                ),
                "{mutation} mutation must not preserve ContractProven"
            );
        }

        let intermediate = source_built_fixture(
            "pub fn callee() {}\npub fn middle() { callee(); }\npub fn caller() { middle(); }\n",
        );
        let caller = source_callable(&intermediate.store, "caller");
        let middle = source_callable(&intermediate.store, "middle");
        let callee = source_callable(&intermediate.store, "callee");
        let (chain_contract, chain_hashes, chain_rendering) = validated_contract(
            canonical_id(&caller),
            &[canonical_id(&middle), canonical_id(&callee)],
        );
        let chain = evaluate_from_store(
            &intermediate.store,
            &intermediate.root,
            &intermediate.project_id,
            &intermediate.publication,
            CheckedIntegrationInputs {
                contract: &chain_contract,
                hashes: &chain_hashes,
                rendering: &chain_rendering,
            },
            |path| fs::read(path),
        )
        .unwrap();
        let (direct_contract, direct_hashes, _) =
            validated_contract(canonical_id(&caller), &[canonical_id(&callee)]);
        assert!(
            !matches!(
                check_call_path(&direct_contract, &direct_hashes, &chain.built_facts().facts,),
                ProofDisposition::ContractProven { .. }
            ),
            "two source-built edges through an intermediate are not one direct fact"
        );

        let raw_edge = six
            .store
            .get_edges()
            .unwrap()
            .into_iter()
            .find(|edge| edge.kind == EdgeKind::CALL)
            .unwrap();
        let mut ambiguous = raw_edge.clone();
        ambiguous.candidate_targets = vec![nodes[2].id];
        for (mutation, edge) in [("ambiguous", &ambiguous)] {
            assert_eq!(
                admit_raw_call_edge(edge, edge.effective_source(), edge.effective_target()),
                RawCallEdgeAdmission::Rejected
            );
            let rejected_edge_id = edge.id.0.to_string();
            let facts_after_admission = integrated
                .built_facts()
                .facts
                .iter()
                .filter(|fact| match fact {
                    VerifiedProofFact::DirectCall(fact) => fact.receipt.edge_id != rejected_edge_id,
                    _ => true,
                })
                .cloned()
                .collect::<Vec<_>>();
            assert!(
                !matches!(
                    check_call_path(&contract, &hashes, &facts_after_admission),
                    ProofDisposition::ContractProven { .. }
                ),
                "{mutation} source-built row is rejected before proof and cannot preserve ContractProven"
            );
        }

        fs::write(&six.source_path, "pub fn changed() {}\n").unwrap();
        let drifted = evaluate_from_store(
            &six.store,
            &six.root,
            &six.project_id,
            &six.publication,
            CheckedIntegrationInputs {
                contract: &contract,
                hashes: &hashes,
                rendering: &rendering,
            },
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            drifted.built_facts().unavailable,
            vec![UnavailableReason::SourceNotBoundToPublication]
        );
        assert!(!matches!(
            drifted.disposition(),
            ProofDisposition::ContractProven { .. }
        ));
    }

    #[test]
    fn source_built_recursion_and_policy_closure_keep_exact_fail_closed_outcomes() {
        // The alias keeps the call placeholder's source identity distinct from
        // the declaration while resolution still targets the same callable.
        // The producer then suppresses the self-edge, leaving the exact
        // recursive step unrepresentable.
        let recursive = source_built_fixture(
            "use crate::recursive as again;\npub fn recursive() { again(); }\n",
        );
        let recursive_node = source_callable(&recursive.store, "recursive");
        let (recursive_contract, recursive_hashes, recursive_rendering) = validated_contract(
            canonical_id(&recursive_node),
            &[canonical_id(&recursive_node)],
        );
        let recursive_result = evaluate_from_store(
            &recursive.store,
            &recursive.root,
            &recursive.project_id,
            &recursive.publication,
            CheckedIntegrationInputs {
                contract: &recursive_contract,
                hashes: &recursive_hashes,
                rendering: &recursive_rendering,
            },
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            recursive_result.built_facts().gaps,
            vec![FactBuildGap::RecursiveCallNotRepresentable { step_index: 0 }]
        );
        assert!(matches!(
            recursive_result.disposition(),
            ProofDisposition::Unknown { .. }
        ));

        let fixture = source_built_fixture(
            "pub fn step0() { step1(); }\n\
             pub fn step1() { step2(); }\n\
             pub fn step2() { step3(); }\n\
             pub fn step3() {}\n",
        );
        let nodes = (0..=3)
            .map(|index| source_callable(&fixture.store, &format!("step{index}")))
            .collect::<Vec<_>>();
        let targets = nodes[1..].iter().map(canonical_id).collect::<Vec<_>>();
        let (prohibited, prohibited_hashes, prohibited_rendering) =
            validated_contract_with_policies(
                canonical_id(&nodes[0]),
                &targets,
                &[canonical_id(&nodes[2])],
                &[],
            );
        let refuted = evaluate_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            CheckedIntegrationInputs {
                contract: &prohibited,
                hashes: &prohibited_hashes,
                rendering: &prohibited_rendering,
            },
            |path| fs::read(path),
        )
        .unwrap();
        let expected_chain = refuted.built_facts().receipts[..2]
            .iter()
            .map(|receipt| receipt.receipt.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            refuted.disposition(),
            &ProofDisposition::ContractRefuted {
                contract_digest: prohibited_hashes.contract_digest().to_owned(),
                refutation: Refutation::ProhibitedScopeTraversal {
                    step_index: 1,
                    prohibition_index: 0,
                    connected_receipts: expected_chain.clone(),
                },
            }
        );
        let refuted_projection = project_internal_call_path_result(&refuted).unwrap();
        let InternalProjection::Complete { root, .. } = refuted_projection else {
            panic!("small positive contradiction projection fits")
        };
        assert_eq!(
            root["disposition"]["refutation"]["connected_receipts"],
            json!(
                expected_chain
                    .iter()
                    .map(|receipt| json!({
                        "receipt_id": receipt.receipt_id,
                        "edge_id": receipt.edge_id,
                    }))
                    .collect::<Vec<_>>()
            )
        );

        let (excluded, excluded_hashes, excluded_rendering) = validated_contract_with_policies(
            canonical_id(&nodes[0]),
            &targets,
            &[],
            &[canonical_id(&nodes[2])],
        );
        let excluded_result = evaluate_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            CheckedIntegrationInputs {
                contract: &excluded,
                hashes: &excluded_hashes,
                rendering: &excluded_rendering,
            },
            |path| fs::read(path),
        )
        .unwrap();
        assert!(matches!(
            excluded_result.disposition(),
            ProofDisposition::Unknown { gaps, .. }
                if gaps == &[ProofGap::ProjectionExclusionConflictsWithRequiredReceipt { step_index: 1 }]
        ));
    }

    #[test]
    fn source_built_partial_and_recursive_results_project_exact_builder_gaps() {
        let recursive = source_built_fixture(
            "use crate::recursive as again;\npub fn recursive() { again(); }\n",
        );
        let recursive_node = source_callable(&recursive.store, "recursive");
        let (recursive_contract, recursive_hashes, recursive_rendering) = validated_contract(
            canonical_id(&recursive_node),
            &[canonical_id(&recursive_node)],
        );
        let recursive_result = evaluate_from_store(
            &recursive.store,
            &recursive.root,
            &recursive.project_id,
            &recursive.publication,
            CheckedIntegrationInputs {
                contract: &recursive_contract,
                hashes: &recursive_hashes,
                rendering: &recursive_rendering,
            },
            |path| fs::read(path),
        )
        .unwrap();
        assert!(matches!(
            recursive_result.disposition(),
            ProofDisposition::Unknown { gaps, .. }
                if gaps == &[ProofGap::FactBuild(
                    FactBuildGap::RecursiveCallNotRepresentable { step_index: 0 }
                )]
        ));
        let InternalProjection::Complete { root, .. } =
            project_internal_call_path_result(&recursive_result).unwrap()
        else {
            panic!("small recursive Unknown projection fits")
        };
        assert_eq!(
            root["disposition"]["gaps"],
            json!([{ "kind": "recursive_call_not_representable", "step_index": 0 }])
        );
        assert!(root["receipts"].as_array().unwrap().is_empty());

        let partial = source_built_fixture(
            "pub fn step0() { step1(); }\npub fn step1() {}\npub fn step2() {}\n",
        );
        let step0 = source_callable(&partial.store, "step0");
        let step1 = source_callable(&partial.store, "step1");
        let step2 = source_callable(&partial.store, "step2");
        let (partial_contract, partial_hashes, partial_rendering) = validated_contract(
            canonical_id(&step0),
            &[canonical_id(&step1), canonical_id(&step2)],
        );
        let partial_result = evaluate_from_store(
            &partial.store,
            &partial.root,
            &partial.project_id,
            &partial.publication,
            CheckedIntegrationInputs {
                contract: &partial_contract,
                hashes: &partial_hashes,
                rendering: &partial_rendering,
            },
            |path| fs::read(path),
        )
        .unwrap();
        assert!(matches!(
            partial_result.disposition(),
            ProofDisposition::Unknown {
                gaps,
                connected_receipts,
                ..
            } if gaps == &[ProofGap::FactBuild(
                FactBuildGap::DirectCallMissing { step_index: 1 }
            )] && connected_receipts.len() == 1
        ));
        let InternalProjection::Complete { root, .. } =
            project_internal_call_path_result(&partial_result).unwrap()
        else {
            panic!("small partial Unknown projection fits")
        };
        assert_eq!(
            root["steps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|step| step["status"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["proven", "unknown"]
        );
        assert_eq!(root["receipts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn integrated_execution_uses_the_existing_core_pin_without_retrieval_or_inner_retry() {
        let project = tempfile::tempdir().unwrap();
        let source_path = project.path().join("src/lib.rs");
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        let source = b"pub fn callee() -> i32 { 1 }\npub fn caller() -> i32 { callee(); 1 }\n";
        fs::write(&source_path, source).unwrap();
        let storage_path = project.path().join(".codestory-test/codestory.db");
        let controller = AppController::new_with_config(crate::test_sidecar_runtime_from_env());
        controller
            .open_project_summary_with_storage_path(
                project.path().to_path_buf(),
                storage_path.clone(),
            )
            .unwrap();
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .unwrap();
        let store = Store::open(&storage_path).unwrap();
        let caller = source_callable(&store, "caller");
        let callee = source_callable(&store, "callee");
        let (contract, hashes, rendering) =
            validated_contract(canonical_id(&caller), &[canonical_id(&callee)]);
        drop(store);

        let retrieval_pin_calls = Rc::new(Cell::new(0));
        let observed_retrieval_pin_calls = Rc::clone(&retrieval_pin_calls);
        crate::set_before_retrieval_pin_test_hook(move || {
            observed_retrieval_pin_calls.set(observed_retrieval_pin_calls.get() + 1);
        });
        let service = crate::services::PublicOperationService::new(controller.clone());
        let operation = run_integrated_projected_public_operation(
            &service,
            &controller,
            &contract,
            &hashes,
            &rendering,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(operation.attempt, 1);
        assert!(operation.core_publication.is_some());
        assert_eq!(operation.retrieval_publication, None);
        assert_eq!(retrieval_pin_calls.get(), 0);
        assert!(matches!(
            operation.value.integration.disposition(),
            ProofDisposition::ContractProven { .. }
        ));
        assert!(matches!(
            operation.value.projection,
            InternalProjection::Complete { .. }
        ));
        assert_eq!(
            Store::open(&storage_path)
                .unwrap()
                .get_retrieval_index_publication(&project_identity_v3(project.path()).project_id)
                .unwrap(),
            None
        );

        let builds = Cell::new(0_usize);
        let refusal = service
            .run_with_cancel(PROOF_DOMAIN, Arc::new(AtomicBool::new(false)), || {
                builds.set(builds.get() + 1);
                let built = build_indexed_source_call_path_facts(&controller, &contract)?;
                let mut changed = source.to_vec();
                let byte = changed.iter().position(|byte| *byte == b'1').unwrap();
                changed[byte] = b'2';
                fs::write(&source_path, changed).unwrap();
                Ok(built)
            })
            .expect_err("post-build source drift must consume the one bounded retry and refuse");
        assert_eq!(refusal.code, "project_unavailable");
        assert_eq!(builds.get(), 1);
    }

    #[test]
    fn builds_a_hash_bound_raw_edge_receipt_and_reads_source_once() {
        let fixture = fixture(b"fn source() { target(); }\r\nfn target() {}\n");
        fixture
            .store
            .insert_edge(&Edge {
                id: EdgeId(12),
                source: NodeId(2),
                target: NodeId(3),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(3)),
                callsite_identity: Some("1:1:1:3|rust".to_owned()),
                candidate_targets: Vec::new(),
                ..Default::default()
            })
            .unwrap();
        fixture
            .store
            .insert_node(&Node {
                id: NodeId(6),
                kind: NodeKind::FILE,
                serialized_name: fixture.root.join("src/other.rs").display().to_string(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: None,
                start_line: Some(1),
                start_col: Some(1),
                end_line: Some(1),
                end_col: Some(1),
            })
            .unwrap();
        fixture
            .store
            .insert_edge(&Edge {
                id: EdgeId(13),
                source: NodeId(2),
                target: NodeId(3),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(6)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(3)),
                callsite_identity: Some("6:1:0:3|wrong-file".to_owned()),
                candidate_targets: Vec::new(),
                ..Default::default()
            })
            .unwrap();
        let reads = Cell::new(0);
        let built = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &fixture.contract,
            |path| {
                reads.set(reads.get() + 1);
                fs::read(path)
            },
        )
        .unwrap();

        assert_eq!(reads.get(), 1);
        assert_eq!(built.facts.len(), 1);
        assert!(built.gaps.is_empty());
        assert!(built.unavailable.is_empty());
        assert_eq!(built.receipts.len(), 1);
        let receipt = &built.receipts[0];
        assert_eq!(receipt.receipt.edge_id, "10");
        assert_eq!(receipt.resolution_fact_id.len(), 64);
        assert_eq!(receipt.resolution_evidence_sha256.len(), 64);
        assert_eq!(receipt.callsite_identity, "1:1:1:30|rust");
        assert_eq!(receipt.containment.owner_node_id, NodeId(2));
        assert_eq!(receipt.line_window.kind, "indexed_line_v1");
        assert_eq!(receipt.line_window.text, "fn source() { target(); }\r\n");
        assert_eq!(receipt.line_window.byte_start, 0);
        assert_eq!(receipt.line_window.byte_end, 27);

        let repeated = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(repeated.receipts[0].receipt, receipt.receipt);
        assert_eq!(repeated.receipts, built.receipts);
    }

    #[test]
    fn source_binding_fails_closed_for_hash_and_utf8_drift() {
        let missing_fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        let file = missing_fixture.store.files().get_files().unwrap().remove(0);
        missing_fixture
            .store
            .update_file_metadata(&file, None)
            .unwrap();
        let reads = Cell::new(0);
        let missing_result = build_from_store(
            &missing_fixture.store,
            &missing_fixture.root,
            &missing_fixture.project_id,
            &missing_fixture.publication,
            &missing_fixture.contract,
            |path| {
                reads.set(reads.get() + 1);
                fs::read(path)
            },
        )
        .unwrap();
        assert_eq!(reads.get(), 0);
        assert_eq!(
            missing_result.unavailable,
            vec![UnavailableReason::ProofSemanticProjectionUnavailable]
        );

        let hash_fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        fs::write(
            &hash_fixture.source_path,
            b"fn source() { changed(); }\nfn target() {}\n",
        )
        .unwrap();
        let hash_result = build_from_store(
            &hash_fixture.store,
            &hash_fixture.root,
            &hash_fixture.project_id,
            &hash_fixture.publication,
            &hash_fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            hash_result.unavailable,
            vec![UnavailableReason::SourceNotBoundToPublication]
        );
        assert!(hash_result.facts.is_empty());

        let utf8_fixture = fixture(&[b'f', b'n', b' ', 0xff, b'\n']);
        let utf8_result = build_from_store(
            &utf8_fixture.store,
            &utf8_fixture.root,
            &utf8_fixture.project_id,
            &utf8_fixture.publication,
            &utf8_fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            utf8_result.gaps,
            vec![FactBuildGap::InvalidUtf8 { step_index: 0 }]
        );
        assert!(utf8_result.facts.is_empty());
    }

    #[test]
    fn hash_mismatch_precedes_invalid_utf8_and_is_cached_without_reread() {
        let fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        fixture
            .store
            .insert_edge(&Edge {
                id: EdgeId(15),
                source: NodeId(2),
                target: NodeId(3),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(3)),
                callsite_identity: Some("1:1:1:3|second-callsite".to_owned()),
                candidate_targets: Vec::new(),
                ..Default::default()
            })
            .unwrap();
        fs::write(&fixture.source_path, [0xff, b'\n']).unwrap();
        let reads = Cell::new(0);

        let built = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &fixture.contract,
            |path| {
                reads.set(reads.get() + 1);
                fs::read(path)
            },
        )
        .unwrap();

        assert_eq!(reads.get(), 1);
        assert_eq!(
            built.unavailable,
            vec![UnavailableReason::SourceNotBoundToPublication]
        );
        assert!(built.gaps.is_empty());
        assert!(built.facts.is_empty());
    }

    #[test]
    fn containment_rejects_nested_and_equal_smallest_owners() {
        for nested in [true, false] {
            let mut fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
            let intruder_id = if nested { 4 } else { 5 };
            fixture
                .store
                .insert_node(&node(
                    intruder_id,
                    NodeKind::METHOD,
                    "inner",
                    &format!("inner-{intruder_id}"),
                    1,
                    1,
                    1,
                ))
                .unwrap();
            fixture
                .store
                .upsert_callable_projection_states(&[
                    if nested {
                        projection(1, 2, 1, 2)
                    } else {
                        projection(1, 2, 1, 1)
                    },
                    projection(1, intruder_id, 1, 1),
                ])
                .unwrap();
            let built = build_from_store(
                &fixture.store,
                &fixture.root,
                &fixture.project_id,
                &fixture.publication,
                &fixture.contract,
                |path| fs::read(path),
            )
            .unwrap();
            assert_eq!(
                built.gaps,
                vec![FactBuildGap::EdgeContainmentUnproven { step_index: 0 }]
            );
            assert!(built.facts.is_empty());
        }
    }

    #[test]
    fn line_windows_preserve_lf_crlf_last_line_and_exact_cap() {
        assert_eq!(
            complete_line(b"one\ntwo", 1),
            Some((0, 4, "one\n".to_owned()))
        );
        assert_eq!(
            complete_line(b"one\ntwo", 2),
            Some((4, 7, "two".to_owned()))
        );
        assert_eq!(
            complete_line(b"one\r\ntwo\r\n", 1),
            Some((0, 5, "one\r\n".to_owned()))
        );
        assert_eq!(complete_line(b"one\n", 3), None);
        assert_eq!(
            complete_line(&vec![b'a'; MAX_LINE_WINDOW_BYTES], 1)
                .unwrap()
                .1,
            MAX_LINE_WINDOW_BYTES
        );
        assert!(
            complete_line(&vec![b'a'; MAX_LINE_WINDOW_BYTES + 1], 1)
                .unwrap()
                .1
                > MAX_LINE_WINDOW_BYTES
        );

        let mut fixture = fixture(b"fn source() { target(); }\nfn target() {}");
        fixture
            .store
            .insert_node(&node(
                4,
                NodeKind::FUNCTION,
                "other_target",
                "other-target-id",
                1,
                2,
                2,
            ))
            .unwrap();
        fixture
            .store
            .insert_edge(&Edge {
                id: EdgeId(14),
                source: NodeId(2),
                target: NodeId(4),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(3),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(4)),
                callsite_identity: Some("1:3:0:4|out-of-range".to_owned()),
                candidate_targets: Vec::new(),
                ..Default::default()
            })
            .unwrap();
        fixture
            .store
            .upsert_callable_projection_states(&[projection(1, 2, 1, 3)])
            .unwrap();
        let built = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &contract("source-id", &["other-target-id"]),
            |path| fs::read(path),
        )
        .unwrap();
        assert!(built.facts.is_empty());
        assert_eq!(
            built.gaps,
            vec![FactBuildGap::DirectCallMissing { step_index: 0 }]
        );
    }

    #[test]
    fn builder_accepts_exactly_eight_kib_and_rejects_eight_kib_plus_one() {
        for (length, accepted) in [
            (MAX_LINE_WINDOW_BYTES, true),
            (MAX_LINE_WINDOW_BYTES + 1, false),
        ] {
            let mut bytes = vec![b'a'; length];
            bytes[.."target".len()].copy_from_slice(b"target");
            let fixture = fixture(&bytes);
            let built = build_from_store(
                &fixture.store,
                &fixture.root,
                &fixture.project_id,
                &fixture.publication,
                &fixture.contract,
                |path| fs::read(path),
            )
            .unwrap();
            if accepted {
                assert_eq!(built.facts.len(), 1);
                assert!(built.gaps.is_empty());
            } else {
                assert!(built.facts.is_empty());
                assert_eq!(
                    built.gaps,
                    vec![FactBuildGap::SourceWindowTooLarge { step_index: 0 }]
                );
            }
        }
    }

    #[test]
    fn synthetic_self_edge_is_positive_but_an_ordinary_missing_edge_is_only_unknown() {
        let fixture = fixture(b"fn source() { source(); }\nfn target() {}\n");
        fixture
            .store
            .insert_edge(&Edge {
                id: EdgeId(11),
                source: NodeId(2),
                target: NodeId(2),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(2)),
                callsite_identity: Some("1:1:0:2|synthetic".to_owned()),
                candidate_targets: Vec::new(),
                ..Default::default()
            })
            .unwrap();
        let recursive = contract("source-id", &["source-id"]);
        let self_edge = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &recursive,
            |path| fs::read(path),
        )
        .unwrap();
        assert!(self_edge.facts.is_empty());
        assert_eq!(
            self_edge.gaps,
            vec![FactBuildGap::RecursiveCallNotRepresentable { step_index: 0 }]
        );

        let missing = contract("target-id", &["source-id"]);
        let absent = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &missing,
            |path| fs::read(path),
        )
        .unwrap();
        assert!(absent.facts.is_empty());
        assert_eq!(
            absent.gaps,
            vec![FactBuildGap::DirectCallMissing { step_index: 0 }]
        );
    }

    #[test]
    fn exact_selector_resolution_never_picks_first_or_prefix_matches() {
        let fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        fixture
            .store
            .insert_node(&node(
                4,
                NodeKind::METHOD,
                "duplicate",
                "source-id",
                1,
                1,
                1,
            ))
            .unwrap();
        fixture
            .store
            .insert_node(&node(
                5,
                NodeKind::STRUCT,
                "structural",
                "structural-id",
                1,
                1,
                1,
            ))
            .unwrap();
        let ambiguous = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            ambiguous.gaps,
            vec![FactBuildGap::SelectorAmbiguous { selector_index: 0 }]
        );

        let missing = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &contract("source", &["target-id"]),
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            missing.gaps,
            vec![FactBuildGap::SelectorMissing { selector_index: 0 }]
        );

        let non_callable = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &contract("structural-id", &["target-id"]),
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            non_callable.gaps,
            vec![FactBuildGap::NonCallableSelector { selector_index: 0 }]
        );
        let non_callable_target = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &contract("target-id", &["structural-id"]),
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            non_callable_target.gaps,
            vec![FactBuildGap::NonCallableSelector { selector_index: 1 }]
        );
    }

    #[test]
    fn qualified_resolution_uses_exact_names_components_and_native_file_identity() {
        let fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        let files = fixture.store.files().get_files().unwrap();
        let context = SelectorContext {
            store: &fixture.store,
            project_root: &fixture.root,
            project_id: &fixture.project_id,
            publication: &fixture.publication,
            files: &files,
        };
        let exact_path = ["src".to_owned(), "lib.rs".to_owned()];
        let exact = resolve_qualified(
            &context,
            "crate::target",
            Some(&exact_path),
            &mut OperationPathIdentityResolver::native(),
        )
        .unwrap();
        assert!(matches!(
            exact,
            SelectorResolution::Resolved(node) if node.pinned.node_id == "3"
        ));

        for wrong_path in [
            vec!["src".to_owned(), "index".to_owned()],
            vec!["src".to_owned(), "indexer".to_owned()],
        ] {
            assert!(matches!(
                resolve_qualified(
                    &context,
                    "crate::target",
                    Some(&wrong_path),
                    &mut OperationPathIdentityResolver::native(),
                )
                .unwrap(),
                SelectorResolution::Unavailable(UnavailableReason::SourceNotBoundToPublication)
            ));
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&fixture.source_path, fixture.root.join("src/alias.rs"))
                .unwrap();
            let alias_path = ["src".to_owned(), "alias.rs".to_owned()];
            assert!(matches!(
                resolve_qualified(
                    &context,
                    "crate::target",
                    Some(&alias_path),
                    &mut OperationPathIdentityResolver::native(),
                )
                .unwrap(),
                SelectorResolution::Resolved(node) if node.pinned.node_id == "3"
            ));

            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("outside.rs"), "fn outside() {}\n").unwrap();
            std::os::unix::fs::symlink(outside.path(), fixture.root.join("src/escape")).unwrap();
            let escape_path = [
                "src".to_owned(),
                "escape".to_owned(),
                "outside.rs".to_owned(),
            ];
            assert!(matches!(
                resolve_qualified(
                    &context,
                    "crate::target",
                    Some(&escape_path),
                    &mut OperationPathIdentityResolver::native(),
                )
                .unwrap(),
                SelectorResolution::Unavailable(UnavailableReason::SourceNotBoundToPublication)
            ));
        }

        fixture
            .store
            .insert_node(&node(
                4,
                NodeKind::METHOD,
                "source",
                "another-source-id",
                1,
                1,
                1,
            ))
            .unwrap();
        assert!(matches!(
            resolve_qualified(
                &context,
                "crate::source",
                None,
                &mut OperationPathIdentityResolver::native(),
            )
            .unwrap(),
            SelectorResolution::Ambiguous
        ));
    }

    #[test]
    fn pin_mismatch_and_missing_recursive_edges_are_typed_unknown_or_unavailable() {
        let fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        let mut wrong_publication = fixture.publication.clone();
        wrong_publication.run_id = "other-run".to_owned();
        let pin = PinnedNodeIdentity {
            project_id: fixture.project_id.clone(),
            core_generation_id: fixture.publication.generation_id.clone(),
            core_run_id: fixture.publication.run_id.clone(),
            node_id: "2".to_owned(),
        };
        let files = fixture.store.files().get_files().unwrap();
        let context = SelectorContext {
            store: &fixture.store,
            project_root: &fixture.root,
            project_id: &fixture.project_id,
            publication: &wrong_publication,
            files: &files,
        };
        assert!(matches!(
            resolve_pinned(&context, &pin, &mut OperationPathIdentityResolver::native(),).unwrap(),
            SelectorResolution::Unavailable(UnavailableReason::PublicationPinMismatch)
        ));

        let recursive = contract("source-id", &["source-id"]);
        let built = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &recursive,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            built.gaps,
            vec![FactBuildGap::RecursiveCallNotRepresentable { step_index: 0 }]
        );
        assert!(built.facts.is_empty());
    }
}
