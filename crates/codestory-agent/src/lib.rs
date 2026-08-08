//! Packet planning, extracted from `codestory-runtime`.
//!
//! This crate decides *what to ask for*: the terms a prompt carries, the flow
//! requirements a task class implies, the evidence roles and carriers a
//! citation can play, and the deduplicated query plan that comes out the other
//! side. It owns none of the machinery that answers those questions.
//!
//! Specifically, nothing here may activate a publication, open or write
//! storage, execute retrieval, retry a publication, or move readiness. The only
//! runtime state planning may see is what the host already pinned, and it sees
//! that exclusively through [`pinned_reader::PinnedReader`]. The crate DAG
//! enforces the rule from below — `codestory-agent` depends on
//! `codestory-contracts` and nothing else in this workspace — and
//! `codestory-cli`'s architecture contracts enforce it from above.

#[cfg(any(test, feature = "test-support"))]
pub mod eval_probes;
pub mod packet_citations;
pub mod packet_command;
pub mod packet_evidence_carriers;
pub mod packet_evidence_roles;
pub mod packet_execution_graphs;
pub mod packet_flow_requirements;
pub mod packet_probes;
pub mod packet_scoring;
pub mod packet_terms;
pub mod pinned_reader;
pub mod planning;
pub mod text;
pub mod trail;

pub use pinned_reader::{ContinuationRefusal, PinnedReader, admit_continuation_probe};
