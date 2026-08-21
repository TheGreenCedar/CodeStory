//! Sealed observations used only by the proof-qualification benchmark.
//!
//! This facade is intentionally feature-gated and owns no product route. The
//! benchmark receives its domain identity through this module; later
//! qualification tasks add observations without exposing the dark kernel.

/// Identifies the request domain observed by proof qualification.
pub fn proof_domain() -> &'static str {
    codestory_agent::proof_qualification_support::proof_domain()
}
