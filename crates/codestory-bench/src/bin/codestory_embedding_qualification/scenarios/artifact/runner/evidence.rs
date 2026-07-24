use super::ScenarioEvidence;
use anyhow::{Result, bail};

pub(super) fn validate_named_evidence(scenario: &str, evidence: &ScenarioEvidence) -> Result<()> {
    let (controls, transitions): (&[&str], &[&str]) = match scenario {
        "client_death" => (
            &[
                "hold_class:bulk",
                "hold_class:query",
                "release_class:bulk",
                "release_class:query",
            ],
            &[
                "dead_client_work_observed",
                "other_client_continued",
                "client_terminated",
                "dead_client_work_reclaimed",
                "post_reclaim_other_client_query",
            ],
        ),
        "cold_race" => (
            &[],
            &[
                "no_owner_before_race",
                "two_independent_processes",
                "single_server_convergence",
            ],
        ),
        "frozen_owner" => (
            &["freeze_owner", "release_owner"],
            &["bounded_owner_unresponsive", "owner_identity_stable"],
        ),
        "incompatible_owner" => (
            &["force_incompatible", "clear_incompatible"],
            &[
                "active_owner_rejected",
                "idle_owner_draining",
                "compatible_replacement",
            ],
        ),
        "mixed_queue" => (
            &[
                "hold_class:bulk",
                "hold_class:query",
                "release_class:bulk",
                "release_class:query",
            ],
            &[
                "queues_saturated",
                "query_selected_before_bulk_backlog",
                "typed_capacity_retry_observed",
                "per_class_fifo_observed",
                "global_fifo_across_projects",
                "query_preference_observed",
                "bulk_resumed",
            ],
        ),
        "server_crash" => (
            &["hold_class:query", "crash_server"],
            &[
                "inflight_request_observed",
                "server_replaced",
                "query_replayed",
            ],
        ),
        "true_idle_respawn" => (
            &[
                "hold_class:bulk",
                "hold_class:query",
                "release_class:bulk",
                "release_class:query",
            ],
            &[
                "owner_started",
                "anti_idle_work_observed",
                "owner_preserved_across_idle_boundary",
                "anti_idle_work_reclaimed",
                "idle_surfaces_exercised",
                "owner_absent_after_true_idle",
                "server_respawned",
            ],
        ),
        "worker_stall" => (
            &["stall_native", "release_native"],
            &[
                "stalled_request_observed",
                "watchdog_fail_stop_observed",
                "unrelated_process_survived",
                "post_stall_replacement",
            ],
        ),
        _ => bail!("embedding_qualification_scenario_unknown"),
    };
    if controls
        .iter()
        .any(|control| !evidence.controls.contains(*control))
        || transitions
            .iter()
            .any(|transition| !evidence.transitions.contains(*transition))
    {
        bail!("embedding_qualification_named_evidence_incomplete:{scenario}");
    }
    Ok(())
}
