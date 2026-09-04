use super::contract::*;
use super::control::RunControl;
use anyhow::{Context, Result, ensure};
use codestory_retrieval::benchmark_support::Etr1LexicalIndex;
use codestory_retrieval::{
    EmbeddingEngineIdentity, PerUserEmbeddingClient, ProductEmbeddingClient, SidecarRuntimeConfig,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const RUN_CONTRACT: &str = "codestory.etr1-run/v1";
const ROW_CONTRACT: &str = "codestory.etr1-wording/v1";
const VECTOR_CONTRACT: &str = "codestory.embedding-diagnostic-output/v1";
const QUERY_BATCH_MAX: usize = 8;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FragmentVectorArtifact {
    contract: String,
    authority: String,
    packet_decision: String,
    input_sha256: String,
    source_commit: String,
    source_tree: String,
    build_profile: String,
    rustc: String,
    binary_sha256: String,
    initial_engine: Value,
    final_engine: Value,
    whole_encoding_wall_ms: u64,
    records: Vec<FragmentVectorRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FragmentVectorRecord {
    id: String,
    purpose: String,
    text_sha256: String,
    vector: Vec<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QualificationEvent {
    schema_version: u32,
    sequence: u64,
    action: String,
    status: String,
    server_event_sequence: u64,
    clock: Value,
    #[serde(default)]
    snapshot: Option<Value>,
    #[serde(default)]
    details: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone)]
struct QuerySpec {
    ordinal: usize,
    seed_fragment_id: String,
    original_input: String,
    encoded_input: String,
    removed_trailing_source_lines: u32,
    model_limit_rejections: u32,
}

#[derive(Debug)]
struct EncodedQuery {
    spec: QuerySpec,
    vector: Vec<f32>,
    global_batch_ordinal: u32,
}

#[derive(Debug, Default)]
struct EventCursor {
    completed_events: usize,
}

struct SourceAuthenticator<'a> {
    repository: &'a PreparedRepositoryV1,
    fragments: &'a HashMap<String, &'a FrozenFragmentV1>,
    file_cache: HashMap<String, Vec<u8>>,
    receipt: SourceAuthenticationReceiptV1,
}

impl<'a> SourceAuthenticator<'a> {
    fn new(
        repository: &'a PreparedRepositoryV1,
        fragments: &'a HashMap<String, &'a FrozenFragmentV1>,
    ) -> Self {
        Self {
            repository,
            fragments,
            file_cache: HashMap::new(),
            receipt: SourceAuthenticationReceiptV1::default(),
        }
    }

    fn authenticate(&mut self, fragment_id: &str) -> Result<()> {
        if self
            .receipt
            .authenticated_fragment_ids
            .iter()
            .any(|value| value == fragment_id)
        {
            return Ok(());
        }
        let fragment = self
            .fragments
            .get(fragment_id)
            .copied()
            .context("source_fragment_missing")?;
        ensure!(
            fragment.project_id == self.repository.project_id
                && self
                    .repository
                    .fragment_ids
                    .iter()
                    .any(|value| value == fragment_id),
            "source_fragment_repository_mismatch"
        );
        ensure!(
            fragment_id
                == super::contract::fragment_id(
                    &fragment.project_id,
                    &fragment.path,
                    &fragment.content_digest,
                    fragment.byte_range,
                ),
            "source_fragment_identity_mismatch"
        );
        if !self.file_cache.contains_key(&fragment.path) {
            let path = confined_source_path(&self.repository.local_root, &fragment.path)?;
            let bytes = fs::read(path)?;
            let digest = sha256(&bytes);
            ensure!(
                digest == fragment.content_digest,
                "source_file_digest_mismatch"
            );
            self.receipt.filesystem_bytes_read = self
                .receipt
                .filesystem_bytes_read
                .saturating_add(bytes.len() as u64);
            self.receipt
                .file_digests
                .insert(fragment.path.clone(), digest);
            self.file_cache.insert(fragment.path.clone(), bytes);
        }
        let source = &self.file_cache[&fragment.path];
        let start = usize::try_from(fragment.byte_range.start)?;
        let end = usize::try_from(fragment.byte_range.end)?;
        ensure!(
            start < end && end <= source.len(),
            "source_fragment_range_invalid"
        );
        ensure!(
            std::str::from_utf8(&source[..start]).is_ok()
                && std::str::from_utf8(&source[..end]).is_ok(),
            "source_fragment_range_splits_utf8"
        );
        ensure!(
            std::str::from_utf8(&source[start..end])? == fragment.source,
            "source_fragment_bytes_changed"
        );
        ensure!(
            observed_line(source, start) == fragment.line_range.start
                && observed_end_line(source, end) == fragment.line_range.end,
            "source_fragment_lines_changed"
        );
        self.receipt.fragment_source_bytes = self
            .receipt
            .fragment_source_bytes
            .saturating_add(fragment.source.len() as u64);
        self.receipt
            .authenticated_fragment_ids
            .push(fragment_id.to_string());
        Ok(())
    }
}

fn observed_line(bytes: &[u8], offset: usize) -> u32 {
    u32::try_from(
        bytes[..offset]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count()
            + 1,
    )
    .unwrap_or(u32::MAX)
}

fn observed_end_line(bytes: &[u8], end: usize) -> u32 {
    observed_line(bytes, end.saturating_sub(1))
}

fn private_directory(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "state_path_not_absolute");
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "state_not_directory"
    );
    ensure!(fs::canonicalize(path)? == path, "state_path_not_canonical");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
            "state_directory_not_private"
        );
    }
    #[cfg(not(unix))]
    return Err(anyhow::anyhow!(
        "etr1_requires_unix_private_directory_validation"
    ));
    Ok(())
}

fn validate_isolated_state(state_root: &Path, runtime: &SidecarRuntimeConfig) -> Result<PathBuf> {
    private_directory(state_root)?;
    let cache = state_root.join("cache");
    let ipc = state_root.join("ipc");
    private_directory(&cache)?;
    private_directory(&ipc)?;
    ensure!(runtime.cache_root == cache, "etr1_cache_not_isolated");
    ensure!(!runtime.embedding.allow_cpu, "etr1_cpu_fallback_forbidden");
    let gate = codestory_retrieval::qualification_gate_environment();
    ensure!(
        gate.directory.as_deref() == Some(ipc.as_os_str()),
        "etr1_qualification_directory_not_isolated"
    );
    let nonce = gate
        .nonce_string()
        .context("etr1_qualification_nonce_missing")?;
    ensure!(
        (16..=64).contains(&nonce.len())
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "etr1_qualification_nonce_invalid"
    );
    let events = ipc.join(format!("{nonce}.events.jsonl"));
    ensure!(!events.exists(), "etr1_qualification_events_preexisting");
    Ok(events)
}

fn validate_vector(vector: &[f32]) -> Result<()> {
    ensure!(
        vector.len() == VECTOR_DIMENSION && vector.iter().all(|value| value.is_finite()),
        "vector_shape_invalid"
    );
    let norm = vector
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>();
    ensure!((norm - 1.0).abs() < 0.001, "vector_not_normalized");
    Ok(())
}

fn engine_receipt(identity: &EmbeddingEngineIdentity) -> Result<Value> {
    ensure!(
        identity.accelerator_execution_verified
            && identity.worker_alive
            && identity.load_error.is_none()
            && identity.embedded_model
            && identity.policy == "accelerated"
            && identity.model_digest == MODEL_SHA256,
        "engine_execution_unverified"
    );
    Ok(serde_json::to_value(identity)?)
}

fn load_vector_artifact(
    path: &Path,
    expected_sha256: &str,
    preparation: &Etr1PreparationV1,
) -> Result<(
    FileBinding,
    FragmentVectorArtifact,
    HashMap<String, Vec<f32>>,
)> {
    let binding = bind_file(path, Some(expected_sha256))?;
    let artifact: FragmentVectorArtifact = read_bound_json(&binding)?;
    ensure!(
        artifact.contract == VECTOR_CONTRACT
            && artifact.authority == "post_failure_diagnostic_only"
            && artifact.packet_decision == "not_evaluated"
            && artifact.input_sha256 == preparation.embedding_input.sha256
            && artifact.source_commit == preparation.build.source_commit
            && artifact.source_tree == preparation.build.source_tree
            && artifact.build_profile == preparation.build.profile
            && artifact.rustc == preparation.build.rustc
            && artifact.whole_encoding_wall_ms > 0,
        "fragment_vector_artifact_identity_mismatch"
    );
    ensure!(
        artifact.initial_engine["model_digest"] == MODEL_SHA256
            && artifact.final_engine["model_digest"] == MODEL_SHA256
            && artifact.initial_engine["server_instance_id"]
                == artifact.final_engine["server_instance_id"]
            && artifact.initial_engine["load_generation"]
                == artifact.final_engine["load_generation"]
            && artifact.initial_engine["accelerator_execution_verified"] == true
            && artifact.final_engine["accelerator_execution_verified"] == true,
        "fragment_vector_engine_mismatch"
    );
    ensure!(
        artifact.records.len() == preparation.fragments.len(),
        "fragment_vector_count_mismatch"
    );
    let mut vectors = HashMap::with_capacity(artifact.records.len());
    for (record, fragment) in artifact.records.iter().zip(&preparation.fragments) {
        ensure!(
            record.id == fragment.fragment_id
                && record.purpose == "document"
                && record.text_sha256 == sha256(fragment.source.as_bytes()),
            "fragment_vector_record_binding_mismatch"
        );
        validate_vector(&record.vector)?;
        ensure!(
            vectors
                .insert(record.id.clone(), record.vector.clone())
                .is_none(),
            "duplicate_fragment_vector"
        );
    }
    ensure!(
        artifact.binary_sha256.len() == 64
            && artifact
                .binary_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "fragment_vector_binary_identity_invalid"
    );
    Ok((binding, artifact, vectors))
}

fn read_completed_events(path: &Path) -> Result<Vec<QualificationEvent>> {
    let bytes = fs::read(path).context("read qualification event log")?;
    ensure!(
        bytes.ends_with(b"\n"),
        "qualification_event_log_unterminated"
    );
    let mut events = Vec::new();
    let mut previous_native: Option<u64> = None;
    let mut previous_server: Option<u64> = None;
    let mut request_ids = HashSet::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let event: QualificationEvent = serde_json::from_slice(line)?;
        ensure!(
            event.schema_version == 1
                && event.sequence == 0
                && event.action == "completed_tokens"
                && event.status == "completed"
                && event.server_event_sequence > 0
                && previous_server.is_none_or(|previous| event.server_event_sequence > previous)
                && event.clock.is_object()
                && event.snapshot.is_none(),
            "qualification_token_event_invalid"
        );
        let details = event
            .details
            .as_ref()
            .context("qualification_token_details_missing")?;
        let request_id = details
            .get("request_id")
            .filter(|value| !value.is_empty())
            .context("qualification_token_request_id_missing")?;
        let native_sequence = details
            .get("native_completion_sequence")
            .context("qualification_native_completion_sequence_missing")?
            .parse::<u64>()?;
        ensure!(
            native_sequence > 0
                && previous_native
                    .is_none_or(|previous| { previous.checked_add(1) == Some(native_sequence) })
                && request_ids.insert(request_id.clone()),
            "qualification_native_completion_identity_invalid"
        );
        ensure!(
            details
                .get("completed_tokens")
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|value| value > 0),
            "qualification_completed_tokens_invalid"
        );
        previous_native = Some(native_sequence);
        previous_server = Some(event.server_event_sequence);
        events.push(event);
    }
    ensure!(
        !events.is_empty(),
        "qualification_completed_event_log_empty"
    );
    Ok(events)
}

fn completed_event_identity(event: &QualificationEvent) -> Result<(u64, u64, u64, String)> {
    let details = event
        .details
        .as_ref()
        .context("qualification_token_details_missing")?;
    let request_id = details
        .get("request_id")
        .filter(|value| !value.is_empty())
        .context("qualification_token_request_id_missing")?;
    let native_sequence = details
        .get("native_completion_sequence")
        .context("qualification_native_completion_sequence_missing")?
        .parse::<u64>()?;
    let tokens = details
        .get("completed_tokens")
        .context("qualification_completed_tokens_missing")?
        .parse::<u64>()?;
    ensure!(
        tokens > 0 && native_sequence > 0 && event.server_event_sequence > 0,
        "qualification_completed_event_identity_invalid"
    );
    Ok((
        tokens,
        native_sequence,
        event.server_event_sequence,
        sha256(request_id.as_bytes()),
    ))
}

fn consume_completed_event(
    path: &Path,
    cursor: &mut EventCursor,
) -> Result<(u64, u64, u64, String)> {
    let events = read_completed_events(path)?;
    ensure!(
        events.len() == cursor.completed_events + 1,
        "qualification_completed_event_count_invalid"
    );
    let identity = completed_event_identity(&events[cursor.completed_events])?;
    cursor.completed_events += 1;
    Ok(identity)
}

fn input_too_long(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}");
    text.contains("native_embedding_input_too_long") || text.contains("embedding input is too long")
}

fn shorten_single_query(spec: &mut QuerySpec, question: &str, source: &str) -> Result<()> {
    let lines = source.split_inclusive('\n').collect::<Vec<_>>();
    let retained = lines
        .len()
        .checked_sub(usize::try_from(spec.removed_trailing_source_lines)?)
        .context("all_seed_source_lines_removed")?;
    ensure!(retained > 1, "no_complete_seed_source_line_fits");
    let next_source = lines[..retained - 1].concat();
    ensure!(
        !next_source.trim().is_empty(),
        "no_complete_seed_source_line_fits"
    );
    spec.removed_trailing_source_lines += 1;
    spec.encoded_input = format!("{question}\n\n{next_source}");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn encode_partition(
    control: &RunControl,
    client: &ProductEmbeddingClient,
    specs: &mut [QuerySpec],
    questions: &[String],
    seed_sources: &[String],
    qualification_events: &Path,
    cursor: &mut EventCursor,
    next_batch_ordinal: &mut u32,
    batches: &mut Vec<BatchReceiptV1>,
    results: &mut Vec<EncodedQuery>,
    arm: &str,
) -> Result<()> {
    ensure!(
        !specs.is_empty() && specs.len() <= QUERY_BATCH_MAX,
        "query_batch_size_invalid"
    );
    let inputs = specs
        .iter()
        .map(|spec| spec.encoded_input.clone())
        .collect::<Vec<_>>();
    let started = Instant::now();
    match client.embed_queries_with_control(&inputs, Some(control.batch_timeout()?), &|| {
        control.cancelled()
    }) {
        Ok(vectors) => {
            let wall_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
            ensure!(vectors.len() == specs.len(), "query_vector_count_mismatch");
            for vector in &vectors {
                validate_vector(vector)?;
            }
            let (completed_tokens, native_sequence, server_sequence, request_id_sha256) =
                consume_completed_event(qualification_events, cursor)?;
            let ordinal = *next_batch_ordinal;
            *next_batch_ordinal = (*next_batch_ordinal).saturating_add(1);
            batches.push(BatchReceiptV1 {
                global_batch_ordinal: ordinal,
                arm: arm.to_string(),
                query_ordinals: specs.iter().map(|spec| spec.ordinal as u32).collect(),
                input_sha256: specs
                    .iter()
                    .map(|spec| sha256(spec.encoded_input.as_bytes()))
                    .collect(),
                wall_ns,
                completed_tokens,
                qualification_native_completion_sequence: native_sequence,
                qualification_server_event_sequence: server_sequence,
                qualification_request_id_sha256: request_id_sha256,
            });
            for (spec, vector) in specs.iter().cloned().zip(vectors) {
                results.push(EncodedQuery {
                    spec,
                    vector,
                    global_batch_ordinal: ordinal,
                });
            }
            Ok(())
        }
        Err(error) if input_too_long(&error) => {
            ensure!(arm == "candidate", "raw_question_exceeds_model_limit");
            for spec in specs.iter_mut() {
                spec.model_limit_rejections = spec.model_limit_rejections.saturating_add(1);
            }
            if specs.len() > 1 {
                let middle = specs.len() / 2;
                let (left, right) = specs.split_at_mut(middle);
                encode_partition(
                    control,
                    client,
                    left,
                    questions,
                    seed_sources,
                    qualification_events,
                    cursor,
                    next_batch_ordinal,
                    batches,
                    results,
                    arm,
                )?;
                encode_partition(
                    control,
                    client,
                    right,
                    questions,
                    seed_sources,
                    qualification_events,
                    cursor,
                    next_batch_ordinal,
                    batches,
                    results,
                    arm,
                )
            } else {
                let index = specs[0].ordinal;
                shorten_single_query(&mut specs[0], &questions[index], &seed_sources[index])?;
                encode_partition(
                    control,
                    client,
                    specs,
                    questions,
                    seed_sources,
                    qualification_events,
                    cursor,
                    next_batch_ordinal,
                    batches,
                    results,
                    arm,
                )
            }
        }
        Err(error) => Err(error).context("encode ETR-1 query batch"),
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_queries(
    control: &RunControl,
    client: &ProductEmbeddingClient,
    arm: &str,
    specs: &mut [QuerySpec],
    questions: &[String],
    seed_sources: &[String],
    qualification_events: &Path,
    cursor: &mut EventCursor,
    next_batch_ordinal: &mut u32,
) -> Result<(Vec<EncodedQuery>, Vec<BatchReceiptV1>)> {
    ensure!(
        specs.len() == questions.len() && specs.len() == seed_sources.len(),
        "query_spec_input_count_mismatch"
    );
    let mut encoded = Vec::with_capacity(specs.len());
    let mut batches = Vec::new();
    for start in (0..specs.len()).step_by(QUERY_BATCH_MAX) {
        let end = (start + QUERY_BATCH_MAX).min(specs.len());
        encode_partition(
            control,
            client,
            &mut specs[start..end],
            questions,
            seed_sources,
            qualification_events,
            cursor,
            next_batch_ordinal,
            &mut batches,
            &mut encoded,
            arm,
        )?;
    }
    encoded.sort_by_key(|query| query.spec.ordinal);
    ensure!(
        encoded
            .iter()
            .enumerate()
            .all(|(ordinal, query)| query.spec.ordinal == ordinal),
        "encoded_query_order_invalid"
    );
    Ok((encoded, batches))
}

fn score_fragments(
    query: &[f32],
    fragment_ids: &[String],
    vectors: &HashMap<String, Vec<f32>>,
) -> Result<(Vec<f32>, Vec<(String, f32)>)> {
    validate_vector(query)?;
    let mut scores = Vec::with_capacity(fragment_ids.len());
    for fragment_id in fragment_ids {
        let vector = vectors
            .get(fragment_id)
            .context("repository_fragment_vector_missing")?;
        let score = query
            .iter()
            .zip(vector)
            .fold(0.0_f32, |sum, (left, right)| sum + left * right);
        ensure!(score.is_finite(), "semantic_score_nonfinite");
        scores.push(score);
    }
    let mut ranked = fragment_ids
        .iter()
        .cloned()
        .zip(scores.iter().copied())
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    Ok((scores, ranked))
}

fn exact_legally_selectable_pool(
    descriptor_pool: &[String],
    repository: &PreparedRepositoryV1,
    fragments: &HashMap<String, &FrozenFragmentV1>,
) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for fragment_id in descriptor_pool {
        if !seen.insert(fragment_id) {
            continue;
        }
        let fragment = fragments
            .get(fragment_id)
            .copied()
            .context("legally_selectable_fragment_missing")?;
        let public_bytes = usize::try_from(repository.base_serialized_bytes)?
            .saturating_add(usize::try_from(fragment.serialized_row_bytes)?);
        if public_bytes <= PUBLIC_BYTES {
            result.push(fragment_id.clone());
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn run_arm(
    control: &RunControl,
    name: &str,
    wording: &PreparedWordingV1,
    repository: &PreparedRepositoryV1,
    fragments: &HashMap<String, &FrozenFragmentV1>,
    vectors: &HashMap<String, Vec<f32>>,
    lexical: &Etr1LexicalIndex,
    client: &ProductEmbeddingClient,
    qualification_events: &Path,
    cursor: &mut EventCursor,
    next_batch_ordinal: &mut u32,
) -> Result<ArmFrontierV1> {
    let request_started = Instant::now();
    control.check()?;
    let bm25_started = Instant::now();
    let (_, matches) = lexical.search(&wording.question)?;
    let observed_seeds = natural_seed_prefix(&matches)
        .into_iter()
        .map(|item| repository.fragment_ids[item.rowid - 1].clone())
        .collect::<Vec<_>>();
    let round_zero_bm25_ns = u64::try_from(bm25_started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    ensure!(
        observed_seeds == wording.seed_fragment_ids,
        "round_zero_seed_drift"
    );
    let seeds = observed_seeds.into_iter().collect::<BTreeSet<_>>();
    let seed_sources = wording
        .seed_fragment_ids
        .iter()
        .map(|id| {
            fragments
                .get(id)
                .map(|fragment| fragment.source.clone())
                .context("seed_fragment_missing")
        })
        .collect::<Result<Vec<_>>>()?;
    let mut authenticator = SourceAuthenticator::new(repository, fragments);
    let seed_auth_started = Instant::now();
    for fragment_id in &wording.seed_fragment_ids {
        authenticator.authenticate(fragment_id)?;
    }
    let seed_source_authentication_ns =
        u64::try_from(seed_auth_started.elapsed().as_nanos()).unwrap_or(u64::MAX);

    let query_inputs = if name == "control" {
        wording
            .seed_fragment_ids
            .iter()
            .map(|_| wording.question.clone())
            .collect::<Vec<_>>()
    } else {
        ensure!(name == "candidate", "unknown_etr1_arm");
        seed_sources
            .iter()
            .map(|source| format!("{}\n\n{source}", wording.question))
            .collect()
    };
    let query_questions = wording
        .seed_fragment_ids
        .iter()
        .map(|_| wording.question.clone())
        .collect::<Vec<_>>();
    let mut specs = query_inputs
        .into_iter()
        .enumerate()
        .map(|(ordinal, input)| QuerySpec {
            ordinal,
            seed_fragment_id: wording.seed_fragment_ids[ordinal].clone(),
            original_input: input.clone(),
            encoded_input: input,
            removed_trailing_source_lines: 0,
            model_limit_rejections: 0,
        })
        .collect::<Vec<_>>();
    let encoding_started = Instant::now();
    let (encoded, batch_receipts) = encode_queries(
        control,
        client,
        name,
        &mut specs,
        &query_questions,
        &seed_sources,
        qualification_events,
        cursor,
        next_batch_ordinal,
    )?;
    let query_encoding_ns =
        u64::try_from(encoding_started.elapsed().as_nanos()).unwrap_or(u64::MAX);

    let vector_started = Instant::now();
    let mut scored = Vec::with_capacity(encoded.len());
    for query in &encoded {
        control.check()?;
        scored.push(score_fragments(
            &query.vector,
            &repository.fragment_ids,
            vectors,
        )?);
    }
    let vector_search_ns = u64::try_from(vector_started.elapsed().as_nanos()).unwrap_or(u64::MAX);

    let mapping_started = Instant::now();
    let mut prior = BTreeSet::new();
    let mut successors = Vec::new();
    let mut query_receipts = Vec::with_capacity(encoded.len());
    for (query, (scores, ranked)) in encoded.into_iter().zip(scored) {
        let excluded_before = seeds.union(&prior).cloned().collect();
        let selected = select_successors(&ranked, &seeds, &prior, SUCCESSORS_PER_QUERY);
        ensure!(
            selected.len() <= SUCCESSORS_PER_QUERY,
            "successor_query_limit_exceeded"
        );
        prior.extend(selected.iter().cloned());
        successors.extend(selected.iter().cloned());
        query_receipts.push(QueryReceiptV1 {
            query_ordinal: u32::try_from(query.spec.ordinal)?,
            seed_fragment_id: query.spec.seed_fragment_id,
            original_input_sha256: sha256(query.spec.original_input.as_bytes()),
            encoded_input_sha256: sha256(query.spec.encoded_input.as_bytes()),
            encoded_input: query.spec.encoded_input,
            removed_trailing_source_lines: query.spec.removed_trailing_source_lines,
            model_limit_rejections: query.spec.model_limit_rejections,
            global_batch_ordinal: query.global_batch_ordinal,
            score_order_sha256: repository.score_order_sha256.clone(),
            query_vector: query.vector,
            scores,
            excluded_before,
            retained_successors: selected,
        });
    }
    ensure!(
        successors.len() <= MAX_SUCCESSORS
            && successors.iter().collect::<HashSet<_>>().len() == successors.len(),
        "successor_pool_invalid"
    );
    let mut descriptor_pool = wording.seed_fragment_ids.clone();
    descriptor_pool.extend(successors.iter().cloned());
    ensure!(
        descriptor_pool.len() <= MAX_POOL,
        "descriptor_pool_limit_exceeded"
    );
    let descriptor_mapping_ns =
        u64::try_from(mapping_started.elapsed().as_nanos()).unwrap_or(u64::MAX);

    let remaining_auth_started = Instant::now();
    for fragment_id in &successors {
        control.check()?;
        authenticator.authenticate(fragment_id)?;
    }
    let remaining_source_authentication_ns =
        u64::try_from(remaining_auth_started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let hydrated_pool = descriptor_pool.clone();
    ensure!(
        authenticator.receipt.authenticated_fragment_ids.len() == hydrated_pool.len(),
        "hydrated_pool_authentication_incomplete"
    );
    let legally_selectable_pool =
        exact_legally_selectable_pool(&hydrated_pool, repository, fragments)?;
    let prepared_state_ns = u64::try_from(request_started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let accounted_ns = round_zero_bm25_ns
        .saturating_add(seed_source_authentication_ns)
        .saturating_add(query_encoding_ns)
        .saturating_add(vector_search_ns)
        .saturating_add(descriptor_mapping_ns)
        .saturating_add(remaining_source_authentication_ns);
    ensure!(accounted_ns <= prepared_state_ns, "request_timing_overlaps");
    let timing = ArmTimingV1 {
        round_zero_bm25_ns,
        seed_source_authentication_ns,
        query_encoding_ns,
        vector_search_ns,
        descriptor_mapping_ns,
        remaining_source_authentication_ns,
        prepared_state_ns,
        unaccounted_ns: prepared_state_ns - accounted_ns,
    };
    Ok(ArmFrontierV1 {
        name: name.to_string(),
        search_count: u32::try_from(query_receipts.len())?,
        query_receipts,
        batch_receipts,
        successors,
        descriptor_pool,
        hydrated_pool,
        legally_selectable_pool,
        source_authentication: authenticator.receipt,
        token_total: 0,
        timing,
    })
}

fn finalize_arm(mut arm: ArmFrontierV1) -> ArmFrontierV1 {
    arm.token_total = arm
        .batch_receipts
        .iter()
        .map(|batch| batch.completed_tokens)
        .sum();
    arm
}

pub fn execute(
    prepared: &Path,
    prepared_sha256: &str,
    fragment_vectors: &Path,
    fragment_vectors_sha256: &str,
    state_root: &Path,
    output: &Path,
    cancel_file: &Path,
    document_execution: &Path,
    document_execution_sha256: &str,
) -> Result<()> {
    let run_control = RunControl::new(cancel_file)?;
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .context("source_root_missing")?;
    ensure!(
        output.is_absolute() && !output.starts_with(source_root) && !output.starts_with(state_root),
        "output_must_be_private_external_evidence"
    );
    let preparation_binding = bind_file(prepared, Some(prepared_sha256))?;
    let preparation: Etr1PreparationV1 = read_bound_json(&preparation_binding)?;
    let build = build_identity()?;
    let canary = preparation.authority == "synthetic_canary_only";
    ensure!(
        !build.source_dirty
            && preparation.contract == "codestory.etr1-preparation/v1"
            && (preparation.authority == "visible_development_frontier_only" || canary)
            && preparation.packet_decision == "not_evaluated"
            && preparation.parent_head == PARENT_HEAD
            && preparation.annotation_access == "not_accessed"
            && (preparation.annotations.sha256 == ANNOTATIONS_SHA256 || canary)
            && preparation.model_sha256 == MODEL_SHA256
            && preparation.tokenizer_sha256 == TOKENIZER_SHA256
            && preparation.fragments.len() == if canary { 32 } else { FRAGMENT_COUNT }
            && preparation.wordings.len() == if canary { 3 } else { WORDING_COUNT }
            && preparation.build.source_commit == build.source_commit
            && preparation.build.source_tree == build.source_tree,
        "etr1_preparation_identity_mismatch"
    );
    ensure!(
        bind_file(&preparation.method.path, Some(&preparation.method.sha256))?.bytes
            == preparation.method.bytes
            && bind_file(
                &preparation.embedding_input.path,
                Some(&preparation.embedding_input.sha256)
            )?
            .bytes
                == preparation.embedding_input.bytes,
        "etr1_preparation_child_binding_mismatch"
    );

    // The full document-vector artifact is read and authenticated before any
    // request timing begins. It is never rebuilt or paged during ETR-1.
    let (vector_binding, vector_artifact, vectors) =
        load_vector_artifact(fragment_vectors, fragment_vectors_sha256, &preparation)?;
    let document_execution = bind_file(document_execution, Some(document_execution_sha256))?;
    let execution: Value = read_bound_json(&document_execution)?;
    ensure!(
        execution["contract"] == "codestory.etr1-execution/v1"
            && execution["role"] == "documents"
            && execution["experiment_status"] == "completed"
            && execution["exit_code"] == 0
            && execution["signal"].is_null()
            && execution["annotation_access"] == "not_accessed",
        "document_execution_invalid"
    );
    let request_binding: FileBinding = serde_json::from_value(execution["request"].clone())?;
    let request: Value = read_bound_json(&request_binding)?;
    let producer: FileBinding = serde_json::from_value(request["executable"].clone())?;
    ensure!(
        bind_file(&producer.path, Some(&producer.sha256))? == producer
            && producer.sha256 == vector_artifact.binary_sha256,
        "document_producer_changed"
    );
    let inputs: Vec<FileBinding> = serde_json::from_value(request["inputs"].clone())?;
    let outputs: Vec<FileBinding> = serde_json::from_value(execution["outputs"].clone())?;
    ensure!(
        inputs.contains(&preparation.embedding_input) && outputs.contains(&vector_binding),
        "document_execution_artifact_not_bound"
    );
    let document_events: FileBinding = serde_json::from_value(execution["events"].clone())?;
    ensure!(
        bind_file(&document_events.path, Some(&document_events.sha256))? == document_events,
        "document_native_events_changed"
    );
    ensure!(
        !read_completed_events(&document_events.path)?.is_empty(),
        "document_native_events_missing"
    );
    let runtime = SidecarRuntimeConfig::local();
    let qualification_events = validate_isolated_state(state_root, &runtime)?;
    codestory_cli::install_native_embedding_client_transport()?;
    let mut residency = PerUserEmbeddingClient::for_runtime(&runtime)?.acquire_residency_lease()?;
    let initial_engine = engine_receipt(residency.identity())?;
    let client = ProductEmbeddingClient::new(&runtime);

    let fragment_map = preparation
        .fragments
        .iter()
        .map(|fragment| (fragment.fragment_id.clone(), fragment))
        .collect::<HashMap<_, _>>();
    ensure!(
        fragment_map.len() == preparation.fragments.len(),
        "fragment_map_identity_collision"
    );
    let repository_map = preparation
        .repositories
        .iter()
        .map(|repository| (repository.repository_id.clone(), repository))
        .collect::<HashMap<_, _>>();
    let mut lexical = BTreeMap::new();
    for repository in &preparation.repositories {
        ensure!(
            super::prepare::git_head(&repository.local_root)? == repository.commit,
            "repository_commit_changed"
        );
        lexical.insert(
            repository.repository_id.clone(),
            Etr1LexicalIndex::new(repository.fragment_ids.iter().map(|id| {
                fragment_map
                    .get(id)
                    .expect("prepared repository fragment identity exists")
                    .source
                    .as_str()
            }))?,
        );
    }

    let mut cursor = EventCursor::default();
    let mut next_batch_ordinal = 0_u32;
    let mut rows = Vec::with_capacity(WORDING_COUNT);
    for wording in &preparation.wordings {
        let repository = repository_map
            .get(&wording.repository_id)
            .copied()
            .context("wording_repository_missing")?;
        let index = lexical
            .get(&wording.repository_id)
            .context("wording_lexical_index_missing")?;
        let control = finalize_arm(run_arm(
            &run_control,
            "control",
            wording,
            repository,
            &fragment_map,
            &vectors,
            index,
            &client,
            &qualification_events,
            &mut cursor,
            &mut next_batch_ordinal,
        )?);
        let candidate = finalize_arm(run_arm(
            &run_control,
            "candidate",
            wording,
            repository,
            &fragment_map,
            &vectors,
            index,
            &client,
            &qualification_events,
            &mut cursor,
            &mut next_batch_ordinal,
        )?);
        ensure!(
            control.search_count == wording.seed_fragment_ids.len() as u32
                && candidate.search_count == control.search_count
                && control.descriptor_pool.len() <= MAX_POOL
                && candidate.descriptor_pool.len() <= MAX_POOL,
            "etr1_arm_budget_mismatch"
        );
        rows.push(Etr1WordingResultV1 {
            contract: ROW_CONTRACT.into(),
            case_id: wording.case_id.clone(),
            phrasing_id: wording.phrasing_id.clone(),
            repository_id: wording.repository_id.clone(),
            group: wording.group.clone(),
            question_sha256: wording.question_sha256.clone(),
            seed_fragment_ids: wording.seed_fragment_ids.clone(),
            control,
            candidate,
        });
    }
    ensure!(
        rows.len() == preparation.wordings.len(),
        "etr1_row_count_mismatch"
    );
    let final_engine = engine_receipt(&residency.revalidate()?)?;
    for key in [
        "server_instance_id",
        "load_generation",
        "model_digest",
        "ggml_build_identity",
    ] {
        ensure!(
            initial_engine[key] == final_engine[key],
            "engine_identity_changed:{key}"
        );
    }
    let event_bytes = fs::read(&qualification_events)?;
    let completed = read_completed_events(&qualification_events)?;
    ensure!(
        completed.len() == cursor.completed_events
            && rows
                .iter()
                .flat_map(|row| [&row.control, &row.candidate])
                .map(|arm| arm.batch_receipts.len())
                .sum::<usize>()
                == completed.len(),
        "qualification_event_reconciliation_failed"
    );
    let total_tokens = rows
        .iter()
        .flat_map(|row| [&row.control, &row.candidate])
        .map(|arm| arm.token_total)
        .sum::<u64>();

    let stage = stage_output_directory(output)?;
    run_control.check()?;
    let rows_directory = stage.path().join("rows");
    fs::create_dir(&rows_directory)?;
    let mut row_bindings = Vec::with_capacity(rows.len());
    for row in &rows {
        let name = format!("{}--{}.json", row.case_id, row.phrasing_id);
        let bytes = serialize_pretty(row)?;
        write_exclusive(&rows_directory.join(&name), &bytes)?;
        row_bindings.push(FileBinding {
            path: output.join("rows").join(name),
            sha256: sha256(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    write_exclusive(
        &stage.path().join("qualification-events.jsonl"),
        &event_bytes,
    )?;
    let qualification_binding = FileBinding {
        path: output.join("qualification-events.jsonl"),
        sha256: sha256(&event_bytes),
        bytes: event_bytes.len() as u64,
    };
    let manifest = Etr1RunManifestV1 {
        contract: RUN_CONTRACT.into(),
        authority: preparation.authority.clone(),
        experiment_status: "awaiting_validation".into(),
        decision: "not_evaluated".into(),
        parent_head: PARENT_HEAD.into(),
        build,
        preparation: preparation_binding,
        fragment_vectors: vector_binding,
        document_execution,
        method_sha256: preparation.method.sha256,
        annotation_access: "not_accessed".into(),
        vector_artifact_loaded_before_timing: true,
        initial_engine,
        final_engine,
        graph_invocations: 0,
        bge_invocations: 0,
        symbol_document_invocations: 0,
        host_query_invocations: 0,
        production_packet_invocations: 0,
        qualification_events: qualification_binding,
        qualification_completed_token_total: total_tokens,
        rows: row_bindings,
    };
    let manifest_bytes = serialize_pretty(&manifest)?;
    write_exclusive(&stage.path().join("run.json"), &manifest_bytes)?;
    publish_output_directory(stage, output)?;
    println!(
        "{}  {}",
        sha256(&manifest_bytes),
        output.join("run.json").display()
    );
    // Keep the residency lease alive through the complete no-clobber publish.
    drop(residency);
    drop(vector_artifact);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_vector(index: usize) -> Vec<f32> {
        let mut vector = vec![0.0; VECTOR_DIMENSION];
        vector[index] = 1.0;
        vector
    }

    #[test]
    fn normalized_dot_scores_use_fragment_id_as_only_tie_break() {
        let ids = vec!["z".to_string(), "a".to_string(), "m".to_string()];
        let vectors = HashMap::from([
            ("z".to_string(), unit_vector(0)),
            ("a".to_string(), unit_vector(0)),
            ("m".to_string(), unit_vector(1)),
        ]);
        let (scores, ranked) = score_fragments(&unit_vector(0), &ids, &vectors).unwrap();
        assert_eq!(scores, [1.0, 1.0, 0.0]);
        assert_eq!(
            ranked
                .iter()
                .map(|value| value.0.as_str())
                .collect::<Vec<_>>(),
            ["a", "z", "m"]
        );
    }

    #[test]
    fn complete_line_retry_removes_only_a_trailing_seed_line() {
        let mut spec = QuerySpec {
            ordinal: 0,
            seed_fragment_id: "seed".into(),
            original_input: "question\n\na\nb\n".into(),
            encoded_input: "question\n\na\nb\n".into(),
            removed_trailing_source_lines: 0,
            model_limit_rejections: 1,
        };
        shorten_single_query(&mut spec, "question", "a\nb\n").unwrap();
        assert_eq!(spec.encoded_input, "question\n\na\n");
        assert_eq!(spec.removed_trailing_source_lines, 1);
        assert!(shorten_single_query(&mut spec, "question", "a\nb\n").is_err());
    }

    #[test]
    fn qualification_events_use_native_completion_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"schema_version\":1,\"sequence\":0,\"action\":\"completed_tokens\",",
                "\"status\":\"completed\",\"server_event_sequence\":10,\"clock\":{},",
                "\"details\":{\"completed_tokens\":\"5\",\"native_completion_sequence\":\"1\",",
                "\"request_id\":\"first\"}}\n",
                "{\"schema_version\":1,\"sequence\":0,\"action\":\"completed_tokens\",",
                "\"status\":\"completed\",\"server_event_sequence\":11,\"clock\":{},",
                "\"details\":{\"completed_tokens\":\"7\",\"native_completion_sequence\":\"2\",",
                "\"request_id\":\"second\"}}\n",
            ),
        )
        .unwrap();

        let events = read_completed_events(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 0);
        assert_eq!(events[1].sequence, 0);

        let bytes = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            bytes.replace(
                "\"native_completion_sequence\":\"2\"",
                "\"native_completion_sequence\":\"1\"",
            ),
        )
        .unwrap();
        assert!(read_completed_events(&path).is_err());
    }

    #[test]
    fn legal_pool_deduplicates_exact_ids_without_making_seeds_compulsory() {
        let repository = PreparedRepositoryV1 {
            repository_id: "repo".into(),
            project_id: "project".into(),
            commit: "c".into(),
            local_root: PathBuf::from("/tmp/repo"),
            publication: Value::Null,
            fragment_ids: vec!["seed".into(), "successor".into()],
            score_order_sha256: "x".into(),
            base_serialized_bytes: 100,
        };
        let seed = FrozenFragmentV1 {
            fragment_id: "seed".into(),
            project_id: "project".into(),
            path: "seed.rs".into(),
            content_digest: "a".repeat(64),
            byte_range: ByteRangeV1 { start: 0, end: 1 },
            line_range: LineRangeV1 { start: 1, end: 1 },
            source: "s".into(),
            serialized_row_bytes: u32::try_from(PUBLIC_BYTES).unwrap(),
        };
        let successor = FrozenFragmentV1 {
            fragment_id: "successor".into(),
            serialized_row_bytes: 10,
            ..seed.clone()
        };
        let map = HashMap::from([("seed".into(), &seed), ("successor".into(), &successor)]);
        assert_eq!(
            exact_legally_selectable_pool(
                &["seed".into(), "successor".into(), "successor".into()],
                &repository,
                &map,
            )
            .unwrap(),
            ["successor"]
        );
    }
}
