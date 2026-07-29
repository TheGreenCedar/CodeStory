mod absence;
mod activation;
mod dead_client;
mod measure;
mod owner_exit;
mod queue;

pub(super) use absence::wait_for_owner_absence;
pub(super) use activation::{run_activate_probe, run_cold_race_protocol_exchange};
pub(super) use dead_client::run_dead_client_load;
pub(super) use measure::{
    run_measure_busy_retry, run_measure_constant_cold_query, run_measure_constant_spawn_hello,
    run_measure_hello, run_measure_product_query, run_measure_resident_identity,
    run_measure_spawn_hello, run_measure_true_idle, run_measure_vector_frame,
};
pub(super) use queue::run_queue_load;

const ANTI_IDLE_PROTOCOL_DEADLINE_MS: u64 = 90_000;
