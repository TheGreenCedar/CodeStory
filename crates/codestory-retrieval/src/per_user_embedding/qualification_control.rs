//! Nonce-gated client qualification requests and retained operation results.

use super::client::PerUserEmbeddingClient;
use super::{
    AwakeMonotonicClock, EmbeddingResult, EmbeddingServerSnapshot,
    PER_USER_EMBEDDING_MAX_DOCUMENT_COUNT, PER_USER_EMBEDDING_MAX_INPUT_BYTES,
    PerUserEmbeddingError, hex_sha256,
};
use crate::config::SidecarRuntimeConfig;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationRequest {
    pub schema_version: u32,
    pub nonce_sha256: String,
    pub scenario: String,
    pub parameters: EmbeddingQualificationParameters,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationParameters {
    pub query_count: u32,
    pub bulk_count: u32,
    pub documents_per_bulk: u32,
    pub input_bytes: u32,
    pub hold_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationOperationResult {
    pub correlation_id: String,
    pub class: String,
    pub submitted_ns: u64,
    pub completed_ns: u64,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<EmbeddingQualificationAttemptResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationAttemptResult {
    pub ordinal: u32,
    pub request_id: String,
    pub server_instance_id: String,
    pub submitted_ns: u64,
    pub completed_ns: u64,
    pub outcome: String,
}

pub(super) type EmbeddingQualificationAttemptExchange = (
    (EmbeddingResult, Vec<u8>),
    Vec<EmbeddingQualificationAttemptResult>,
);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQualificationResult {
    pub schema_version: u32,
    pub scenario: String,
    pub started_ns: u64,
    pub finished_ns: u64,
    pub operations: Vec<EmbeddingQualificationOperationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_snapshot: Option<EmbeddingServerSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_snapshot: Option<EmbeddingServerSnapshot>,
}

pub fn run_per_user_embedding_qualification(
    runtime: &SidecarRuntimeConfig,
    request: EmbeddingQualificationRequest,
) -> Result<EmbeddingQualificationResult> {
    validate_qualification_gate(&request)?;
    validate_qualification_request(&request)?;
    let client = PerUserEmbeddingClient::for_runtime(runtime)?;
    let clock = Arc::clone(&client.transport.clock());
    let started_ns = clock.now_ns();
    let initial_snapshot = client.observe()?;
    let input = "q".repeat(request.parameters.input_bytes.max(1) as usize);
    let documents = (0..request.parameters.documents_per_bulk.max(1))
        .map(|index| format!("{index}:{input}"))
        .collect::<Vec<_>>();
    let mut work = Vec::new();
    match request.scenario.as_str() {
        "query" | "replay" => {
            for _ in 0..request.parameters.query_count.max(1) {
                work.push(("query", input.clone(), Vec::new()));
            }
        }
        "bulk" => {
            for _ in 0..request.parameters.bulk_count.max(1) {
                work.push(("bulk", String::new(), documents.clone()));
            }
        }
        "mixed" => {
            for _ in 0..request.parameters.bulk_count {
                work.push(("bulk", String::new(), documents.clone()));
            }
            for _ in 0..request.parameters.query_count {
                work.push(("query", input.clone(), Vec::new()));
            }
        }
        "lease" => work.push(("lease", String::new(), Vec::new())),
        "observe" => work.push(("observe", String::new(), Vec::new())),
        "incompatible" => work.push(("incompatible", String::new(), Vec::new())),
        _ => bail!("embedding_qualification_scenario_unknown"),
    }
    let mut workers = Vec::with_capacity(work.len());
    for (class, query, bulk) in work {
        let client = client.clone();
        let clock = Arc::clone(&clock);
        let hold = Duration::from_millis(request.parameters.hold_ms);
        workers.push(
            thread::Builder::new()
                .name(format!("codestory-embedding-qualification-{class}"))
                .spawn(move || qualification_operation(client, clock, class, query, bulk, hold))
                .context("spawn embedding qualification operation")?,
        );
    }
    let mut operations = Vec::with_capacity(workers.len());
    for worker in workers {
        operations.push(
            worker
                .join()
                .map_err(|_| anyhow!("embedding_qualification_operation_panicked"))?,
        );
    }
    let final_snapshot = client.observe()?;
    Ok(EmbeddingQualificationResult {
        schema_version: 1,
        scenario: request.scenario,
        started_ns,
        finished_ns: clock.now_ns(),
        operations,
        initial_snapshot,
        final_snapshot,
    })
}

fn qualification_operation(
    mut client: PerUserEmbeddingClient,
    clock: Arc<dyn AwakeMonotonicClock>,
    class: &str,
    query: String,
    bulk: Vec<String>,
    hold: Duration,
) -> EmbeddingQualificationOperationResult {
    let correlation_id = Uuid::new_v4().to_string();
    let submitted_ns = clock.now_ns();
    let result = match class {
        "query" => client
            .embed_query_with_qualification_attempts(&query)
            .map(|(_, attempts)| (None, attempts)),
        "bulk" => client
            .embed_documents_with_qualification_attempts(&bulk)
            .map(|(_, attempts)| (None, attempts)),
        "lease" => client.acquire_residency_lease().and_then(|mut lease| {
            if !hold.is_zero() {
                clock.sleep(hold);
            }
            let identity = lease.revalidate()?;
            lease.release()?;
            Ok((Some(identity), Vec::new()))
        }),
        "observe" => client.observe().map(|_| (None, Vec::new())),
        "incompatible" => {
            client.compatibility.config_sha256 = "qualification-incompatible".into();
            client
                .ensure_resident()
                .map(|identity| (Some(identity), Vec::new()))
        }
        _ => unreachable!("qualification scenarios are validated before dispatch"),
    };
    let completed_ns = clock.now_ns();
    match result {
        Ok((identity, attempts)) => EmbeddingQualificationOperationResult {
            correlation_id,
            class: class.into(),
            submitted_ns,
            completed_ns,
            status: "ok".into(),
            error_code: None,
            server_instance_id: identity
                .as_ref()
                .map(|identity| identity.server_instance_id.clone()),
            load_generation: identity.as_ref().map(|identity| identity.load_generation),
            attempts,
        },
        Err(error) => EmbeddingQualificationOperationResult {
            correlation_id,
            class: class.into(),
            submitted_ns,
            completed_ns,
            status: "failed".into(),
            error_code: Some(qualification_error_code(&error)),
            server_instance_id: None,
            load_generation: None,
            attempts: Vec::new(),
        },
    }
}

fn validate_qualification_gate(request: &EmbeddingQualificationRequest) -> Result<()> {
    let directory = std::env::var_os("CODESTORY_EMBED_QUALIFICATION_DIR")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("embedding_qualification_gate_closed"))?;
    let nonce = std::env::var("CODESTORY_EMBED_QUALIFICATION_NONCE")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("embedding_qualification_gate_closed"))?;
    if !PathBuf::from(directory).is_dir() || request.nonce_sha256 != hex_sha256(nonce.as_bytes()) {
        bail!("embedding_qualification_gate_closed");
    }
    Ok(())
}

fn validate_qualification_request(request: &EmbeddingQualificationRequest) -> Result<()> {
    if request.schema_version != 1
        || request.parameters.query_count > 128
        || request.parameters.bulk_count > 128
        || request.parameters.documents_per_bulk > PER_USER_EMBEDDING_MAX_DOCUMENT_COUNT as u32
        || request.parameters.input_bytes == 0
        || request.parameters.input_bytes as usize > PER_USER_EMBEDDING_MAX_INPUT_BYTES
        || request.parameters.hold_ms > 600_000
    {
        bail!("embedding_qualification_request_invalid");
    }
    Ok(())
}

fn qualification_error_code(error: &anyhow::Error) -> String {
    error
        .chain()
        .find_map(|cause| {
            cause
                .downcast_ref::<PerUserEmbeddingError>()
                .map(|error| error.code.clone())
        })
        .unwrap_or_else(|| {
            error
                .to_string()
                .split(':')
                .next()
                .unwrap_or("embedding_qualification_failed")
                .into()
        })
}
