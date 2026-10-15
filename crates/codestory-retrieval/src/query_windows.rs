//! Retrieval-owned query windows. Native acceptance decides fit; raw bytes,
//! one elapsed deadline, and one authenticated encode producer remain pinned.

use crate::per_user_embedding::{
    EmbeddingEngineIdentity, PER_USER_EMBEDDING_MAX_INPUT_BYTES,
    PER_USER_EMBEDDING_QUERY_BATCH_MAX, PerUserEmbeddingError,
};
use anyhow::{Result, anyhow, bail};
use std::ops::Range;
use std::time::{Duration, Instant};

pub(crate) const QUERY_WINDOW_POLICY: &str = "native-query-windows-max-cosine-v1";
const MAX_WINDOWS: usize = 64;
// The first legacy-compatible client call can replay one lost server. Reserve
// that extra submission; later calls cannot replay. Thus at most128 embedding
// operation submissions, including rejected and accepted native requests.
const MAX_ATTEMPTS: usize = 127;

#[derive(Debug)]
pub(crate) struct QueryWindowVector {
    pub range: Range<usize>,
    pub vector: Vec<f32>,
}

#[derive(Debug)]
pub(crate) struct QueryWindowBatch {
    pub groups: Vec<Vec<QueryWindowVector>>,
    pub identity: EmbeddingEngineIdentity,
    pub deadline: Instant,
    pub expanded: bool,
}

#[derive(Clone)]
struct Window {
    query: usize,
    range: Range<usize>,
}

pub(crate) fn check_window_control(
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<Duration> {
    let (code, message) = if cancelled() {
        (
            "embedding_cancelled",
            "the caller cancelled query window execution",
        )
    } else if Instant::now() >= deadline {
        (
            "embedding_deadline_exceeded",
            "the query window deadline elapsed",
        )
    } else {
        return Ok(deadline.saturating_duration_since(Instant::now()));
    };
    Err(PerUserEmbeddingError {
        code: code.into(),
        message: message.into(),
        retry_class: "none".into(),
        retry_after_ms: 0,
        retry_condition: "the caller starts a new request".into(),
        capacity: None,
    }
    .into())
}

pub(crate) fn same_query_engine(
    left: &EmbeddingEngineIdentity,
    right: &EmbeddingEngineIdentity,
) -> bool {
    left.server_instance_id == right.server_instance_id
        && left.load_generation == right.load_generation
        && left.model_load_count == right.model_load_count
        && left.model_digest == right.model_digest
        && left.materialized_model_sha256 == right.materialized_model_sha256
        && left.ggml_build_identity == right.ggml_build_identity
        && left.backend == right.backend
        && left.adapter_name == right.adapter_name
        && left.adapter_description == right.adapter_description
        && left.policy == right.policy
        && left.embedded_model == right.embedded_model
        && left.execution_device_names == right.execution_device_names
        && left.execution_backend_names == right.execution_backend_names
        && left.execution_observation_source == right.execution_observation_source
        && left.model_layer_count == right.model_layer_count
        && left.offloaded_layer_count == right.offloaded_layer_count
        && left.accelerator_execution_verified == right.accelerator_execution_verified
}

pub(crate) fn validate_query_inputs(texts: &[String]) -> Result<()> {
    if texts.is_empty()
        || texts.len() > PER_USER_EMBEDDING_QUERY_BATCH_MAX
        || texts.iter().any(|text| text.trim().is_empty())
    {
        bail!("embedding_server_input_shape_invalid");
    }
    let bytes = texts
        .iter()
        .try_fold(0usize, |sum, text| sum.checked_add(text.len()))
        .ok_or_else(|| anyhow!("embedding_server_input_length_overflow"))?;
    if bytes > PER_USER_EMBEDDING_MAX_INPUT_BYTES {
        bail!("embedding_server_input_too_large");
    }
    Ok(())
}

/// Only a typed native token overflow activates subdivision. Accepted fragments
/// are all-or-nothing and every original byte belongs to exactly one fragment.
pub(crate) fn embed_windows<F>(
    texts: &[String],
    started: Instant,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
    mut embed: F,
) -> Result<QueryWindowBatch>
where
    F: FnMut(&[String], Duration, bool) -> Result<(Vec<Vec<f32>>, EmbeddingEngineIdentity)>,
{
    validate_query_inputs(texts)?;
    if timeout.is_zero() {
        bail!("embedding_server_deadline_invalid");
    }
    let deadline = started
        .checked_add(timeout)
        .ok_or_else(|| anyhow!("embedding_server_deadline_invalid"))?;
    let mut pending = vec![
        texts
            .iter()
            .enumerate()
            .map(|(query, text)| Window {
                query,
                range: 0..text.len(),
            })
            .collect::<Vec<_>>(),
    ];
    let mut groups = (0..texts.len()).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut pin: Option<EmbeddingEngineIdentity> = None;
    let mut expanded = false;
    let mut leaves = texts.len();
    let mut attempts = 0;
    while let Some(windows) = pending.pop() {
        let remaining = check_window_control(deadline, cancelled)?;
        if attempts >= MAX_ATTEMPTS {
            bail!("embedding_query_window_attempt_limit");
        }
        attempts += 1;
        let raw = windows
            .iter()
            .map(|window| texts[window.query][window.range.clone()].to_string())
            .collect::<Vec<_>>();
        let result = embed(&raw, remaining, !expanded);
        check_window_control(deadline, cancelled)?;
        match result {
            Ok((vectors, identity)) => {
                if vectors.len() != windows.len() {
                    bail!(
                        "embedding_vector_row_count_mismatch: expected={} observed={}",
                        windows.len(),
                        vectors.len()
                    );
                }
                if pin
                    .as_ref()
                    .is_some_and(|pin| !same_query_engine(pin, &identity))
                {
                    bail!("embedding_query_window_engine_identity_changed");
                }
                if pin.is_none() {
                    pin = Some(identity);
                }
                for (window, vector) in windows.into_iter().zip(vectors) {
                    groups[window.query].push(QueryWindowVector {
                        range: window.range,
                        vector,
                    });
                }
            }
            Err(error) => {
                if error
                    .downcast_ref::<PerUserEmbeddingError>()
                    .is_none_or(|typed| typed.code != "native_embedding_input_too_long")
                {
                    return Err(error);
                }
                expanded = true;
                if windows.len() > 1 {
                    let midpoint = windows.len() / 2;
                    pending.push(windows[midpoint..].to_vec());
                    pending.push(windows[..midpoint].to_vec());
                } else {
                    if leaves >= MAX_WINDOWS {
                        bail!("embedding_query_window_count_limit");
                    }
                    let window = &windows[0];
                    let raw = &texts[window.query][window.range.clone()];
                    let Some(cut) = split_raw(raw) else {
                        return Err(error.context("embedding_query_window_cannot_subdivide"));
                    };
                    leaves += 1;
                    let cut = window.range.start + cut;
                    pending.push(vec![Window {
                        query: window.query,
                        range: cut..window.range.end,
                    }]);
                    pending.push(vec![Window {
                        query: window.query,
                        range: window.range.start..cut,
                    }]);
                }
            }
        }
    }
    for (group, text) in groups.iter_mut().zip(texts) {
        group.sort_unstable_by_key(|window| window.range.start);
        let mut end = 0;
        for window in group {
            if window.range.start != end || window.range.end <= end {
                bail!("embedding_query_window_coverage_invalid");
            }
            end = window.range.end;
        }
        if end != text.len() {
            bail!("embedding_query_window_coverage_invalid");
        }
    }
    check_window_control(deadline, cancelled)?;
    Ok(QueryWindowBatch {
        groups,
        identity: pin.ok_or_else(|| anyhow!("embedding_vector_missing"))?,
        deadline,
        expanded,
    })
}

/// Choose a UTF-8 boundary near the byte midpoint with visible content on both
/// sides. Whitespace is retained verbatim; a single visible character refuses.
fn split_raw(raw: &str) -> Option<usize> {
    let minimum = raw
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(offset, ch)| offset + ch.len_utf8())?;
    let maximum = raw
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_whitespace())
        .map(|(offset, _)| offset)?;
    if minimum > maximum {
        return None;
    }
    let mut cut = (raw.len() / 2).clamp(minimum, maximum);
    while !raw.is_char_boundary(cut) {
        cut -= 1;
    }
    (cut > 0 && cut < raw.len()).then_some(cut)
}

#[cfg(test)]
#[path = "query_windows_tests.rs"]
mod tests;
