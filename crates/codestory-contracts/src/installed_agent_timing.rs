//! Installed-agent timing cohort contract (`InstalledAgentTimingV1`).
//!
//! Comparative wall-time claims must use one host / model / load-policy /
//! task / repeat / execution-window cohort. Whole-task warm timing is the
//! reconciled `whole_task_wall_ms` field, not a raw runner wall alias.

use serde::{Deserialize, Serialize};

pub const INSTALLED_AGENT_TIMING_CONTRACT: &str = "codestory.installed-agent-timing/v1";
pub const INSTALLED_AGENT_TIMING_COHORT_CONTRACT: &str =
    "codestory.installed-agent-timing-cohort/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledAgentTimingV1 {
    /// SHA-256 digest of the cohort dimensions (excludes arm).
    pub timing_cohort_id: String,
    pub agent_runner_ms: u32,
    pub time_to_first_packet_ms: u32,
    pub continuation_ms: u32,
    pub time_to_final_packet_ms: u32,
    pub whole_task_wall_ms: u32,
}

impl InstalledAgentTimingV1 {
    pub fn contract_id() -> &'static str {
        INSTALLED_AGENT_TIMING_CONTRACT
    }

    /// `agent_runner_ms + time_to_first_packet_ms + continuation_ms` must
    /// equal `whole_task_wall_ms` (integer milliseconds).
    pub fn phases_reconcile(&self) -> bool {
        self.agent_runner_ms
            .saturating_add(self.time_to_first_packet_ms)
            .saturating_add(self.continuation_ms)
            == self.whole_task_wall_ms
            && self.time_to_final_packet_ms
                == self
                    .time_to_first_packet_ms
                    .saturating_add(self.continuation_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconciles_phase_sum() {
        let timing = InstalledAgentTimingV1 {
            timing_cohort_id: "a".repeat(64),
            agent_runner_ms: 1201,
            time_to_first_packet_ms: 411,
            continuation_ms: 92,
            time_to_final_packet_ms: 503,
            whole_task_wall_ms: 1704,
        };
        assert!(timing.phases_reconcile());
        assert_eq!(
            InstalledAgentTimingV1::contract_id(),
            INSTALLED_AGENT_TIMING_CONTRACT
        );
    }
}
