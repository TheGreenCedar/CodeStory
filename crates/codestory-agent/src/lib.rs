//! Pure packet seed planning and repository-derived evidence compilation.
//!
//! The original question may seed generic retrieval. Exact selectors come only
//! from typed probes.
//! Once retrieval has produced typed candidates, compilation sees only stable
//! identities, source ranges, directed relations, ambiguity, and publication
//! identity. It owns none of the machinery that answers those questions.
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
pub mod evidence_compiler;
pub mod packet_citations;
pub mod packet_command;
pub mod packet_coverage;
pub mod packet_degradation;
pub mod packet_evidence;
pub mod packet_execution_graphs;
#[doc(hidden)]
pub mod packet_freshness;
pub mod packet_plan;
pub mod packet_probes;
pub mod packet_scoring;
pub mod packet_terms;
pub mod pinned_reader;
pub mod planning;
pub mod profiles;
pub mod text;
pub mod trail;
pub use pinned_reader::{ContinuationRefusal, PinnedReader, admit_continuation_probe};
