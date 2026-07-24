mod doctor;
mod local_freshness;
mod preflight;

pub(super) use doctor::{doctor_sidecar_status_is_live_ready, run_doctor, run_ready};
pub(crate) use local_freshness::{attach_complete_publication, local_refresh_output_from_summary};
pub(super) use preflight::run_agent;

#[cfg(test)]
pub(super) use local_freshness::{
    classify_local_refresh_failure_state, local_freshness_needs_refresh,
};
#[cfg(test)]
pub(super) use preflight::build_agent_preflight_output;
