mod absence;
mod activation;
mod dead_client;
mod queue;

pub(super) use absence::wait_for_owner_absence;
pub(super) use activation::{run_activate_probe, run_cold_race_protocol_exchange};
pub(super) use dead_client::run_dead_client_load;
pub(super) use queue::run_queue_load;

const ANTI_IDLE_PROTOCOL_DEADLINE_MS: u64 = 90_000;
