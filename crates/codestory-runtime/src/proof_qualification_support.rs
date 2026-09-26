//! Optional inspection surface for proof qualification and benchmarks.
//!
//! The product verifier lives in `public_call_path`; enabling this feature
//! only exposes diagnostics, trace types, and artifact helpers to the separate
//! qualification driver.

use serde::Serialize;

pub use crate::call_path_kernel::{
    AdmittedRawCallEdge, BuiltCallPathFacts, COMPACT_PROOF_MAX_BYTES, CONTRACT_INTERPRETATION,
    CallPathSpec, CallableContainmentEvidence, ClauseAnchor, ClauseClassification, FactBuildGap,
    IndexedCallEdgeReceipt, IndexedLineWindow, InternalCorePublicationIdentity, InternalProjection,
    NonMaterialKind, PinnedNodeIdentity, ProofContractField, ProofHashes, RawAdmissionFailure,
    ReceiptRef, ResolvedNodeIdentity, TranslationGap, UnavailableReason, UnresolvedMaterialReason,
    UnvalidatedCallPathContract, UnvalidatedCallPathSpec, UnvalidatedDirectCallStep,
    UnvalidatedExactScopeSelector, UnvalidatedExactSymbolSelector, ValidatedCallPathContract,
    ValidatedContractRendering, ValidationOutcome, VerifiedDirectCallFact, VerifiedProofFact,
    check_built_call_path_integration, diagnose_raw_call_edge, project_internal_call_path_result,
    project_translation_unknown_result, validate_compact_projection, validate_contract,
};
pub use crate::indexed_source_call_path_v1::{
    CandidateFailure, CandidateFailureHistogram, CandidateGate, ContainmentFailure,
    FinalizationFailure, FinalizationTrace, IntegratedProjectedCallPathResult,
    MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP, MAX_QUALIFICATION_OBSERVED_RECEIPTS_PER_CASE,
    ObservedBuiltCallPathFacts, ObservedIntegratedProjectedCallPathResult, ProofQualificationTrace,
    ResolutionFactFailure, SelectorFailure, SelectorGateOutcome, SelectorQualificationTrace,
    SourceBindingFailure, StepQualificationOutcome, StepQualificationTrace,
};
pub use crate::public_call_path::{
    PUBLIC_CALL_PATH_DOMAIN, PublicCallPathResultDto, parse_public_call_path_document,
    project_internal_projection, project_observed_public_operation,
    project_public_transport_budget_result, project_public_verification_result, proof_domain,
    public_call_path_result_schema, run_observed_call_path_public_operation,
    run_translation_unknown_public_operation, validate_public_call_path_contract,
};

/// Serialize a qualification artifact with the pinned RFC 8785 implementation.
pub fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json_canonicalizer::to_vec(value).map_err(|error| error.to_string())
}
