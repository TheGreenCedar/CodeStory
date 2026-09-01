//! Packet planning contracts and policy.
//!
//! This crate decides *what to ask for*: generic retrieval queries from the
//! prompt, structural evidence roles and carriers a citation can play, and the
//! deduplicated query plan. Task-class taxonomies do not steer planning. It
//! owns none of the machinery that answers those questions.
//!
//! Specifically, nothing here may activate a publication, open or write
//! storage, execute retrieval, retry a publication, or move readiness. The only
//! runtime state planning may see is what the host already pinned, and it sees
//! that exclusively through [`pinned_reader::PinnedReader`]. The crate DAG
//! enforces the rule from below — `codestory-agent` depends on
//! `codestory-contracts` and nothing else in this workspace — and
//! `codestory-cli`'s architecture contracts enforce it from above.

pub mod citation;
#[cfg(any(test, feature = "test-support"))]
pub mod eval_probes;
pub mod packet_citations;
pub mod packet_claim_profile_registry;
pub mod packet_claim_profiles;
pub mod packet_claims;
pub mod packet_command;
pub mod packet_coverage;
pub mod packet_degradation;
pub mod packet_evidence;
pub mod packet_evidence_carriers;
pub mod packet_evidence_roles;
pub mod packet_execution_graphs;
#[doc(hidden)]
pub mod packet_execution_plan_v3;
pub mod packet_flow_requirements;
pub mod packet_freshness;
pub mod packet_obligations;
pub mod packet_plan;
pub mod packet_probes;
pub mod packet_profile_telemetry;
pub mod packet_required_probes;
pub mod packet_scoring;
pub mod packet_terms;
pub mod pinned_reader;
pub mod planning;
pub mod profiles;
pub mod repository_evidence_plan;
pub mod text;
pub mod trail;
pub use pinned_reader::{ContinuationRefusal, PinnedReader, admit_continuation_probe};
