//! Sealed observations used only by the proof-qualification benchmark.
//!
//! This facade is intentionally feature-gated and owns no product route. The
//! benchmark receives its domain identity and gate observations through this
//! module without exposing the dark kernel or registering a product route.

pub use crate::indexed_source_call_path_v1::{
    CandidateFailure, CandidateFailureHistogram, CandidateGate, ContainmentFailure,
    FinalizationFailure, FinalizationTrace, IntegratedProjectedCallPathResult,
    MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP, MAX_QUALIFICATION_OBSERVED_RECEIPTS_PER_CASE,
    ObservedBuiltCallPathFacts, ObservedIntegratedProjectedCallPathResult, ProofQualificationTrace,
    SelectorFailure, SelectorGateOutcome, SelectorQualificationTrace, SourceBindingFailure,
    StepQualificationOutcome, StepQualificationTrace,
};

/// Builds the same product facts plus benchmark-only observations. The caller
/// must already be inside the existing core-pinned public operation.
pub fn build_observed_indexed_source_call_path_facts(
    controller: &crate::AppController,
    contract: &codestory_agent::proof_qualification_support::ValidatedCallPathContract,
) -> Result<ObservedBuiltCallPathFacts, codestory_contracts::api::ApiError> {
    crate::indexed_source_call_path_v1::build_observed_indexed_source_call_path_facts(
        controller, contract,
    )
}

/// Runs the existing checked integration and projection over observed facts,
/// retaining a typed finalization failure for qualification reports.
pub fn finalize_observed_call_path(
    contract: &codestory_agent::proof_qualification_support::ValidatedCallPathContract,
    hashes: &codestory_agent::proof_qualification_support::ProofHashes,
    rendering: &codestory_agent::proof_qualification_support::ValidatedContractRendering,
    observed: ObservedBuiltCallPathFacts,
) -> ObservedIntegratedProjectedCallPathResult {
    crate::indexed_source_call_path_v1::finalize_observed_call_path(
        contract, hashes, rendering, observed,
    )
}

/// Identifies the request domain observed by proof qualification.
pub fn proof_domain() -> &'static str {
    codestory_agent::proof_qualification_support::proof_domain()
}
