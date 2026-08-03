//! The read-only seam between packet planning and the runtime that pins state.
//!
//! `codestory-agent` plans; it never activates, stores, executes retrieval,
//! retries a publication, or moves readiness. Everything it needs to know about
//! what the *current* public operation pinned arrives through [`PinnedReader`],
//! whose every method is an owned read of an identity the runtime already
//! selected. There is deliberately no method that hands back a store, a
//! session, a controller, or anything else that could be written through.
//!
//! A reader that answers `None` means "the runtime is not holding that pin
//! right now", and every decision below treats that as a refusal. Losing a pin
//! can never widen what a continuation is allowed to reuse.

use codestory_contracts::api::{PACKET_PROBE_CONTRACT_VERSION, PacketProbeRejectionCodeDto};

/// Read-only view of the identities pinned by the public operation in flight.
pub trait PinnedReader {
    /// Identity of the project the current operation pinned, or `None` when no
    /// project is open.
    fn pinned_project_id(&self) -> Option<String>;

    /// Core generation the current operation pinned, or `None` when no core
    /// publication is selected.
    fn pinned_core_generation_id(&self) -> Option<String>;

    /// Retrieval generation the current operation pinned, or `None` when no
    /// retrieval publication is selected.
    fn pinned_retrieval_generation(&self) -> Option<String>;
}

/// Why a continuation probe may not be reused against the pinned state.
///
/// Each variant carries exactly one rejection code and one message, so the
/// wire-visible refusal is a property of the decision rather than of the caller
/// that renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationRefusal {
    /// The continuation was minted against a different probe contract.
    IncompatibleContract { requested: u32 },
    /// No project is open, so nothing can be continued.
    NoOpenProject,
    /// The continuation belongs to a project other than the pinned one.
    ProjectChanged,
    /// The pinned core generation is not the one the continuation was minted
    /// against (including the case where no core publication is selected).
    CoreGenerationChanged,
    /// The continuation named a retrieval generation that is not the pinned one
    /// (including the case where no retrieval publication is selected).
    RetrievalGenerationChanged,
    /// The continuation carries no query to continue.
    EmptyQuery,
}

impl ContinuationRefusal {
    /// The wire rejection code for this refusal.
    pub fn code(&self) -> PacketProbeRejectionCodeDto {
        match self {
            Self::IncompatibleContract { .. } => {
                PacketProbeRejectionCodeDto::IncompatibleContinuation
            }
            Self::NoOpenProject
            | Self::ProjectChanged
            | Self::CoreGenerationChanged
            | Self::RetrievalGenerationChanged => PacketProbeRejectionCodeDto::StaleContinuation,
            Self::EmptyQuery => PacketProbeRejectionCodeDto::MalformedProbe,
        }
    }

    /// The wire rejection message for this refusal.
    pub fn message(&self) -> String {
        match self {
            Self::IncompatibleContract { requested } => format!(
                "continuation contract {requested} is incompatible with {PACKET_PROBE_CONTRACT_VERSION}"
            ),
            Self::NoOpenProject => "continuation requires an open project".to_string(),
            Self::ProjectChanged => "continuation belongs to a different project".to_string(),
            Self::CoreGenerationChanged => {
                "continuation core generation is no longer selected".to_string()
            }
            Self::RetrievalGenerationChanged => {
                "continuation retrieval generation is no longer selected".to_string()
            }
            Self::EmptyQuery => "continuation query must not be empty".to_string(),
        }
    }
}

/// Decide whether a continuation probe may be planned against the pinned state.
///
/// Returns the trimmed continuation query on admission. The checks run in
/// pin-widening order — contract, project, core generation, retrieval
/// generation, query — so a caller learns the outermost reason first and the
/// reader is never asked for a pin the previous check already refused.
pub fn admit_continuation_probe(
    reader: &dyn PinnedReader,
    contract_version: u32,
    project_id: &str,
    core_generation_id: &str,
    retrieval_generation: Option<&str>,
    query: &str,
) -> Result<String, ContinuationRefusal> {
    if contract_version != PACKET_PROBE_CONTRACT_VERSION {
        return Err(ContinuationRefusal::IncompatibleContract {
            requested: contract_version,
        });
    }
    let Some(pinned_project_id) = reader.pinned_project_id() else {
        return Err(ContinuationRefusal::NoOpenProject);
    };
    if pinned_project_id != project_id {
        return Err(ContinuationRefusal::ProjectChanged);
    }
    if reader
        .pinned_core_generation_id()
        .is_none_or(|pinned| pinned != core_generation_id)
    {
        return Err(ContinuationRefusal::CoreGenerationChanged);
    }
    if let Some(expected) = retrieval_generation
        && reader
            .pinned_retrieval_generation()
            .is_none_or(|pinned| pinned != expected)
    {
        return Err(ContinuationRefusal::RetrievalGenerationChanged);
    }
    let query = query.trim();
    if query.is_empty() {
        return Err(ContinuationRefusal::EmptyQuery);
    }
    Ok(query.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A reader that answers exactly what the runtime's `ControllerPinnedReader`
    /// answers — three owned identities, each absent when the corresponding pin
    /// is not held — and records the order it was asked in.
    struct RecordingReader {
        project_id: Option<&'static str>,
        core_generation_id: Option<&'static str>,
        retrieval_generation: Option<&'static str>,
        reads: RefCell<Vec<&'static str>>,
    }

    impl RecordingReader {
        fn pinned() -> Self {
            Self {
                project_id: Some("project-a"),
                core_generation_id: Some("core-7"),
                retrieval_generation: Some("retrieval-7"),
                reads: RefCell::new(Vec::new()),
            }
        }

        fn reads(&self) -> Vec<&'static str> {
            self.reads.borrow().clone()
        }
    }

    impl PinnedReader for RecordingReader {
        fn pinned_project_id(&self) -> Option<String> {
            self.reads.borrow_mut().push("project");
            self.project_id.map(str::to_string)
        }

        fn pinned_core_generation_id(&self) -> Option<String> {
            self.reads.borrow_mut().push("core");
            self.core_generation_id.map(str::to_string)
        }

        fn pinned_retrieval_generation(&self) -> Option<String> {
            self.reads.borrow_mut().push("retrieval");
            self.retrieval_generation.map(str::to_string)
        }
    }

    fn admit(
        reader: &RecordingReader,
        contract_version: u32,
        project_id: &str,
        core_generation_id: &str,
        retrieval_generation: Option<&str>,
        query: &str,
    ) -> Result<String, ContinuationRefusal> {
        admit_continuation_probe(
            reader,
            contract_version,
            project_id,
            core_generation_id,
            retrieval_generation,
            query,
        )
    }

    #[test]
    fn a_continuation_matching_every_pin_is_admitted_with_a_trimmed_query() {
        let reader = RecordingReader::pinned();
        assert_eq!(
            admit(
                &reader,
                PACKET_PROBE_CONTRACT_VERSION,
                "project-a",
                "core-7",
                Some("retrieval-7"),
                "  AppController::open  ",
            ),
            Ok("AppController::open".to_string())
        );
        assert_eq!(reader.reads(), ["project", "core", "retrieval"]);
    }

    /// One row of the refusal decision table: the pinned state, the continuation
    /// as it arrived, and the exact wire pair it must produce.
    type RefusalCase = (
        RecordingReader,
        u32,
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static str,
        PacketProbeRejectionCodeDto,
        String,
    );

    #[test]
    fn every_refusal_carries_its_own_rejection_code_and_wire_message() {
        let pinned = || RecordingReader::pinned();
        let no_project = || RecordingReader {
            project_id: None,
            ..RecordingReader::pinned()
        };
        let no_core = || RecordingReader {
            core_generation_id: None,
            ..RecordingReader::pinned()
        };
        let no_retrieval = || RecordingReader {
            retrieval_generation: None,
            ..RecordingReader::pinned()
        };

        let cases: Vec<RefusalCase> = vec![
            (
                pinned(),
                PACKET_PROBE_CONTRACT_VERSION + 1,
                "project-a",
                "core-7",
                None,
                "query",
                PacketProbeRejectionCodeDto::IncompatibleContinuation,
                format!(
                    "continuation contract {} is incompatible with {PACKET_PROBE_CONTRACT_VERSION}",
                    PACKET_PROBE_CONTRACT_VERSION + 1
                ),
            ),
            (
                no_project(),
                PACKET_PROBE_CONTRACT_VERSION,
                "project-a",
                "core-7",
                None,
                "query",
                PacketProbeRejectionCodeDto::StaleContinuation,
                "continuation requires an open project".to_string(),
            ),
            (
                pinned(),
                PACKET_PROBE_CONTRACT_VERSION,
                "project-b",
                "core-7",
                None,
                "query",
                PacketProbeRejectionCodeDto::StaleContinuation,
                "continuation belongs to a different project".to_string(),
            ),
            (
                pinned(),
                PACKET_PROBE_CONTRACT_VERSION,
                "project-a",
                "core-6",
                None,
                "query",
                PacketProbeRejectionCodeDto::StaleContinuation,
                "continuation core generation is no longer selected".to_string(),
            ),
            (
                no_core(),
                PACKET_PROBE_CONTRACT_VERSION,
                "project-a",
                "core-7",
                None,
                "query",
                PacketProbeRejectionCodeDto::StaleContinuation,
                "continuation core generation is no longer selected".to_string(),
            ),
            (
                pinned(),
                PACKET_PROBE_CONTRACT_VERSION,
                "project-a",
                "core-7",
                Some("retrieval-6"),
                "query",
                PacketProbeRejectionCodeDto::StaleContinuation,
                "continuation retrieval generation is no longer selected".to_string(),
            ),
            (
                no_retrieval(),
                PACKET_PROBE_CONTRACT_VERSION,
                "project-a",
                "core-7",
                Some("retrieval-7"),
                "query",
                PacketProbeRejectionCodeDto::StaleContinuation,
                "continuation retrieval generation is no longer selected".to_string(),
            ),
            (
                pinned(),
                PACKET_PROBE_CONTRACT_VERSION,
                "project-a",
                "core-7",
                Some("retrieval-7"),
                "   ",
                PacketProbeRejectionCodeDto::MalformedProbe,
                "continuation query must not be empty".to_string(),
            ),
        ];

        for (reader, contract, project, core, retrieval, query, code, message) in cases {
            let refusal = admit(&reader, contract, project, core, retrieval, query)
                .expect_err("this continuation must be refused");
            assert_eq!(refusal.code(), code, "code for {refusal:?}");
            assert_eq!(refusal.message(), message, "message for {refusal:?}");
        }
    }

    #[test]
    fn a_refused_continuation_never_reads_a_pin_past_the_check_that_refused_it() {
        let reader = RecordingReader::pinned();
        admit(
            &reader,
            PACKET_PROBE_CONTRACT_VERSION + 1,
            "project-a",
            "core-7",
            Some("retrieval-7"),
            "query",
        )
        .expect_err("an incompatible contract is refused before any pin is read");
        assert_eq!(reader.reads(), Vec::<&str>::new());

        let reader = RecordingReader::pinned();
        admit(
            &reader,
            PACKET_PROBE_CONTRACT_VERSION,
            "project-b",
            "core-7",
            Some("retrieval-7"),
            "query",
        )
        .expect_err("a foreign project is refused before the generations are read");
        assert_eq!(reader.reads(), ["project"]);

        let reader = RecordingReader::pinned();
        admit(
            &reader,
            PACKET_PROBE_CONTRACT_VERSION,
            "project-a",
            "core-6",
            Some("retrieval-7"),
            "query",
        )
        .expect_err("a stale core generation is refused before retrieval is read");
        assert_eq!(reader.reads(), ["project", "core"]);
    }

    #[test]
    fn a_continuation_that_names_no_retrieval_generation_never_reads_that_pin() {
        let reader = RecordingReader {
            retrieval_generation: None,
            ..RecordingReader::pinned()
        };
        assert_eq!(
            admit(
                &reader,
                PACKET_PROBE_CONTRACT_VERSION,
                "project-a",
                "core-7",
                None,
                "query",
            ),
            Ok("query".to_string())
        );
        assert_eq!(reader.reads(), ["project", "core"]);
    }
}
