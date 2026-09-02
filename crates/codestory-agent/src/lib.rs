//! Prompt-blind packet seed planning and evidence-policy helpers.
//!
//! Horizon A forwards the unchanged question to generic retrieval and records
//! caller-supplied free-query seeds. It does not infer answer shapes, material
//! roles, lifecycle stages, or structural traversal from prompt wording. The
//! repository-derived evidence compiler belongs to Horizon B (`#2106`).
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
