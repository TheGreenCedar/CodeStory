use crate::candidate::{CandidateHit, CandidateSource};
use crate::config::{SidecarLayout, SidecarRuntimeConfig};
use crate::embedding_server_compat::ProductEmbeddingIdentity;
use crate::embeddings::{EmbeddingDeviceReadiness, ProductEmbeddingClient};
use crate::sidecar_search::SearchExecutionContext;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use codestory_contracts::api::{
    EMBEDDING_VECTOR_PRODUCER_EVIDENCE_VERSION, EmbeddingEngineIdentityDto,
    EmbeddingExecutionEvidenceDto, EmbeddingModelIdentityDto, EmbeddingProducerIdentityDto,
    EmbeddingVectorProducerEvidenceDto, EmbeddingVectorPublicationIdentityDto,
    EmbeddingVectorSemanticsDto,
};
use codestory_store::{FileRole, Store};
use codestory_workspace::paths::sqlite_open_path;
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

const VECTOR_INDEX_SCHEMA_VERSION: i64 =
    crate::embedding_contract::EMBEDDING_VECTOR_SCHEMA_VERSION as i64;
const VECTOR_INDEX_FILE: &str = "vectors.sqlite3";
const VECTOR_GENERATION_MANIFEST_FILE: &str = "vector-generation-manifest.json";
const VECTOR_GENERATION_MANIFEST_SCHEMA_VERSION: u32 = 2;
const VECTOR_COMPONENT_SCHEMA_VERSION: u32 = 2;
const VECTOR_DIGEST_DOMAIN: &[u8] = b"codestory-vector-digest-v1\0";
const VECTOR_COMPONENT_DIGEST_DOMAIN: &[u8] = b"codestory-vector-component-v2\0";
const VECTOR_NORM_TOLERANCE: f64 = 1.0e-3;
/// Minimum cosine supported by the source-backed development calibration.
const DENSE_ABSTENTION_ABSOLUTE_FLOOR: f32 = 0.30;
/// Maximum distance from the lane's best cosine supported by that calibration.
const DENSE_ABSTENTION_ADDITIVE_MARGIN: f32 = 0.10;
type ScoredHit = (
    f32,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmbeddedVectorHealth {
    pub ready: bool,
    pub point_count: u64,
    pub latency_ms: u64,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SemanticPoint {
    pub display_name: String,
    pub node_id: String,
    pub file_path: Option<String>,
    pub file_role: Option<FileRole>,
    pub dense_reason: Option<String>,
    pub vector: Vec<f32>,
}

/// One vector plus the immutable source-document identity that authorized it.
///
/// This type deliberately lives in retrieval until the dense-anchor contract
/// shared with the store lands. The manifest builder can translate the pinned
/// anchor-input generation into this narrow integration surface without
/// exposing storage rows to the vector database.
#[derive(Debug, Clone)]
pub(crate) struct AttestedSemanticPoint {
    pub point: SemanticPoint,
    pub document_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedVectorAnchor {
    pub node_id: String,
    pub document_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentVectorAnchor {
    pub node_id: String,
    pub document_hash: String,
    pub display_name: String,
    pub file_path: Option<String>,
    pub file_role: Option<FileRole>,
    pub dense_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IncrementalVectorWork {
    pub retained: u64,
    pub inserted: u64,
    pub removed: u64,
    pub direct_reference: bool,
}

pub(crate) struct AttestedVectorPublication<'a> {
    pub layout: &'a SidecarLayout,
    pub collection: &'a str,
    pub generation: &'a str,
    pub input_hash: &'a str,
    pub contract: &'a VectorEvidenceContract,
    pub expected_anchors: &'a [ExpectedVectorAnchor],
}

struct VectorDatabasePublication<'a> {
    layout: &'a SidecarLayout,
    collection: &'a str,
    generation: &'a str,
    input_hash: &'a str,
    contract: &'a VectorEvidenceContract,
    expected_anchors: Option<&'a BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VectorEvidenceContract {
    pub embedding_backend: String,
    pub embedding_dim: usize,
    pub producer_identity: String,
    pub evidence_contract_identity: String,
}

impl VectorEvidenceContract {
    pub(crate) fn new(
        embedding_backend: impl Into<String>,
        embedding_dim: usize,
        producer_identity: impl Into<String>,
        evidence_contract_identity: impl Into<String>,
    ) -> Self {
        Self {
            embedding_backend: embedding_backend.into(),
            embedding_dim,
            producer_identity: producer_identity.into(),
            evidence_contract_identity: evidence_contract_identity.into(),
        }
    }

    #[cfg(test)]
    fn legacy(embedding_backend: &str, embedding_dim: usize) -> Self {
        Self::new(
            embedding_backend,
            embedding_dim,
            format!("legacy-backend:{embedding_backend}"),
            "legacy-embedded-vector-v1",
        )
    }

    fn validate(&self) -> Result<()> {
        if self.embedding_backend.trim().is_empty()
            || self.producer_identity.trim().is_empty()
            || self.evidence_contract_identity.trim().is_empty()
        {
            bail!("embedded vector evidence identities must be non-empty");
        }
        if self.embedding_dim == 0 {
            bail!("embedded vector dimension must be positive");
        }
        Ok(())
    }
}

pub(crate) fn build_vector_producer_evidence(
    embedding_device: &EmbeddingDeviceReadiness,
    live_identity: Option<&ProductEmbeddingIdentity>,
    embedding_dim: u32,
    publication: EmbeddingVectorPublicationIdentityDto,
) -> EmbeddingVectorProducerEvidenceDto {
    assert_eq!(
        embedding_dim,
        crate::embedding_contract::RETRIEVAL_EMBEDDING_DIM as u32,
        "embedding evidence dimension must match retrieval policy"
    );
    let pooling = crate::embedding_contract::EMBEDDING_POOLING;
    let normalization = crate::embedding_contract::EMBEDDING_NORMALIZATION;
    let engine_build_id = live_identity
        .map(|identity| identity.ggml_build_identity.to_string())
        .unwrap_or_else(|| codestory_llama_sys::PRODUCT_EMBEDDING_RUNTIME_ID.to_string());
    let backend = live_identity
        .map(|identity| identity.backend.clone())
        .or_else(|| embedding_device.detected_provider.clone())
        .unwrap_or_else(|| "test-support".to_string());
    let device_id = live_identity
        .map(|identity| identity.execution_device_names.join(","))
        .filter(|device| !device.is_empty())
        .or_else(|| embedding_device.detected_gpu.clone())
        .or_else(|| embedding_device.detected_provider.clone())
        .unwrap_or_else(|| "test-support".to_string());
    let device_class = live_identity
        .map(|identity| identity.adapter_description.clone())
        .filter(|description| !description.trim().is_empty())
        .or_else(|| embedding_device.detected_provider.clone())
        .unwrap_or_else(|| "test-support".to_string());
    let smoke_elapsed_ms = live_identity.map(|identity| identity.smoke_ms).or_else(|| {
        (embedding_device.observation_source == "test_support"
            && embedding_device.observed_state == "accelerated")
            .then_some(0)
    });

    EmbeddingVectorProducerEvidenceDto {
        schema_version: EMBEDDING_VECTOR_PRODUCER_EVIDENCE_VERSION,
        producer: EmbeddingProducerIdentityDto {
            name: codestory_llama_sys::MODEL_PRODUCER_NAME.to_string(),
            version: codestory_llama_sys::MODEL_PRODUCER_VERSION.to_string(),
        },
        model: EmbeddingModelIdentityDto {
            model_id: crate::embedding_contract::EMBEDDING_MODEL_ID.to_string(),
            model_sha256: crate::embedding_contract::EMBEDDING_MODEL_SHA256.to_string(),
            model_size_bytes: codestory_llama_sys::MODEL_SIZE,
            tokenizer_sha256: codestory_llama_sys::MODEL_TOKENIZER_SHA256.to_string(),
            config_sha256: codestory_llama_sys::MODEL_CONFIG_SHA256.to_string(),
        },
        semantics: EmbeddingVectorSemanticsDto {
            dimension: embedding_dim,
            query_prefix: crate::embeddings::CODERANK_QUERY_PREFIX_DEFAULT.to_string(),
            document_prefix: crate::embeddings::CODERANK_DOCUMENT_PREFIX_DEFAULT.to_string(),
            pooling: pooling.to_string(),
            normalization: normalization.to_string(),
            element_type: crate::embedding_contract::EMBEDDING_ELEMENT_TYPE.to_string(),
            vector_schema_version: VECTOR_INDEX_SCHEMA_VERSION as u32,
        },
        engine: EmbeddingEngineIdentityDto {
            engine: "llama.cpp".to_string(),
            engine_build_id,
            backend,
            device_id,
            device_class,
            accelerator_kind: embedding_device
                .detected_provider
                .clone()
                .unwrap_or_else(|| embedding_device.requested_policy.to_string()),
        },
        execution: EmbeddingExecutionEvidenceDto {
            eligibility: embedding_device.requested_policy.to_string(),
            observed_state: embedding_device.observed_state.to_string(),
            observation_source: embedding_device.observation_source.to_string(),
            smoke_elapsed_ms,
            observed_at_epoch_ms: Utc::now().timestamp_millis(),
        },
        publication,
    }
}

pub(crate) fn vector_producer_compatibility_identity(
    embedding_device: &EmbeddingDeviceReadiness,
    live_identity: Option<&ProductEmbeddingIdentity>,
    embedding_dim: u32,
) -> Result<String> {
    let evidence = build_vector_producer_evidence(
        embedding_device,
        live_identity,
        embedding_dim,
        EmbeddingVectorPublicationIdentityDto {
            core_generation_id: "compatibility-core".into(),
            core_run_id: "compatibility-run".into(),
            retrieval_generation: "compatibility-retrieval".into(),
            retrieval_input_hash: "compatibility-input".into(),
            semantic_generation: "compatibility-semantic".into(),
        },
    );
    vector_compatibility_identity(&evidence)
}

pub(crate) fn producer_evidence_mismatches(
    expected: &EmbeddingVectorProducerEvidenceDto,
    observed: &EmbeddingVectorProducerEvidenceDto,
) -> Vec<String> {
    expected.compatibility_with(observed).mismatches
}

fn vector_component_is_compatible(
    expected: &EmbeddingVectorProducerEvidenceDto,
    observed: &EmbeddingVectorProducerEvidenceDto,
) -> Result<bool> {
    Ok(vector_compatibility_identity(expected)? == vector_compatibility_identity(observed)?)
}

/// Content attestation returned before the candidate database is published.
///
/// In the current component schema, `vector_digest`, `component_sha256`, and
/// the legacy-named `database_sha256` all bind the canonical physical rows,
/// independent of SQLite layout and publication identity. The generation and
/// input hash remain a separate authenticated envelope. Schema-v1 manifests
/// retain their historical whole-file `database_sha256` interpretation and
/// are never admitted for copy-on-write reuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VectorDatabaseAttestation {
    pub schema_version: i64,
    pub generation: String,
    pub input_hash: String,
    pub embedding_backend: String,
    pub embedding_dim: usize,
    pub point_count: u64,
    pub producer_identity: String,
    pub evidence_contract_identity: String,
    pub vector_digest: String,
    pub database_sha256: String,
    #[serde(default = "legacy_vector_component_schema_version")]
    pub component_schema_version: u32,
    #[serde(default)]
    pub component_sha256: String,
    #[serde(default)]
    pub database_size_bytes: u64,
}

const fn legacy_vector_component_schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VectorGenerationManifest {
    pub schema_version: u32,
    pub evidence: codestory_contracts::api::EmbeddingVectorProducerEvidenceDto,
    pub evidence_sha256: String,
    pub compatibility_sha256: String,
    pub vectors: VectorDatabaseAttestation,
}

impl VectorGenerationManifest {
    pub(crate) fn new(
        evidence: codestory_contracts::api::EmbeddingVectorProducerEvidenceDto,
        vectors: VectorDatabaseAttestation,
    ) -> Result<Self> {
        let errors = evidence.validation_errors();
        if !errors.is_empty() {
            bail!(
                "vector producer evidence is incomplete: {}",
                errors.join(", ")
            );
        }
        let evidence_sha256 = hex_digest(Sha256::digest(
            serde_json::to_vec(&evidence).context("serialize vector producer evidence")?,
        ));
        let compatibility_sha256 = vector_compatibility_identity(&evidence)?;
        if vectors.evidence_contract_identity != compatibility_sha256 {
            bail!("vector attestation does not match producer evidence");
        }
        if vectors.component_schema_version != VECTOR_COMPONENT_SCHEMA_VERSION
            || vectors.component_sha256.len() != 64
            || vectors.database_sha256 != vectors.component_sha256
            || vectors.database_size_bytes == 0
        {
            bail!("vector attestation has incompatible physical component evidence");
        }
        Ok(Self {
            schema_version: VECTOR_GENERATION_MANIFEST_SCHEMA_VERSION,
            evidence,
            evidence_sha256,
            compatibility_sha256,
            vectors,
        })
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if !matches!(
            self.schema_version,
            1 | VECTOR_GENERATION_MANIFEST_SCHEMA_VERSION
        ) {
            bail!("unsupported vector generation manifest schema");
        }
        if self.schema_version == 1 {
            if self.vectors.component_schema_version != 1
                || !self.vectors.component_sha256.is_empty()
            {
                bail!("legacy vector manifest has incompatible component metadata");
            }
            let evidence_sha256 = hex_digest(Sha256::digest(
                serde_json::to_vec(&self.evidence)
                    .context("serialize legacy vector producer evidence")?,
            ));
            if evidence_sha256 != self.evidence_sha256
                || vector_compatibility_identity(&self.evidence)? != self.compatibility_sha256
            {
                bail!("legacy vector generation manifest digest mismatch");
            }
            return Ok(());
        }
        let expected = Self::new(self.evidence.clone(), self.vectors.clone())?;
        if expected.evidence_sha256 != self.evidence_sha256 {
            bail!("vector generation evidence digest mismatch");
        }
        if expected.compatibility_sha256 != self.compatibility_sha256 {
            bail!("vector generation compatibility digest mismatch");
        }
        Ok(())
    }
}

pub(crate) fn validate_generation_evidence_for_publication(
    layout: &SidecarLayout,
    storage: &Store,
    manifest: &codestory_store::RetrievalIndexManifest,
    publication: &codestory_store::IndexPublicationRecord,
    runtime: &SidecarRuntimeConfig,
    embedding_device: &EmbeddingDeviceReadiness,
    live_identity: Option<&ProductEmbeddingIdentity>,
) -> Result<VectorGenerationManifest> {
    let generation = manifest
        .sidecar_generation
        .as_deref()
        .context("retrieval manifest is missing its generation")?;
    let input_hash = manifest
        .sidecar_input_hash
        .as_deref()
        .context("retrieval manifest is missing its input hash")?;
    let vector_manifest =
        EmbeddedVectorIndex::load_generation_manifest(layout, &manifest.semantic_generation)?;
    let evidence = &vector_manifest.evidence;
    let vectors = &vector_manifest.vectors;
    let expected_points = manifest
        .dense_projection_count
        .or(manifest.projection_count)
        .and_then(|count| u64::try_from(count).ok())
        .context("retrieval manifest has an invalid dense-anchor count")?;
    let embedding_dim = u32::try_from(crate::embeddings::RETRIEVAL_EMBEDDING_DIM)
        .context("retrieval embedding dimension overflow")?;
    let expected_evidence = build_vector_producer_evidence(
        embedding_device,
        live_identity,
        embedding_dim,
        EmbeddingVectorPublicationIdentityDto {
            core_generation_id: publication.generation_id.clone(),
            core_run_id: publication.run_id.clone(),
            retrieval_generation: generation.to_string(),
            retrieval_input_hash: input_hash.to_string(),
            semantic_generation: manifest.semantic_generation.clone(),
        },
    );
    let mismatches = producer_evidence_mismatches(&expected_evidence, evidence);
    if !mismatches.is_empty() {
        bail!(
            "retrieval vector producer evidence is incompatible with the runtime: {}",
            mismatches.join(", ")
        );
    }
    validate_execution_evidence_for_runtime(evidence, runtime, embedding_device, live_identity)?;
    if vectors.generation != generation
        || vectors.input_hash != input_hash
        || vectors.embedding_backend != manifest.embedding_backend.as_deref().unwrap_or_default()
        || vectors.embedding_dim as i32 != manifest.embedding_dim.unwrap_or_default()
        || vectors.point_count != expected_points
    {
        bail!("retrieval vector generation evidence is incompatible with the publication");
    }
    let dense_publication = storage
        .validate_dense_anchor_publication(publication)
        .context("validate dense-anchor publication for vector admission")?;
    if dense_publication.anchor_count != expected_points {
        bail!(
            "retrieval vector anchor cardinality mismatch: manifest={expected_points} core={}",
            dense_publication.anchor_count
        );
    }
    let expected_anchors = expected_vector_anchors(storage, publication)?;
    if u64::try_from(expected_anchors.len()).unwrap_or(u64::MAX) != expected_points {
        bail!(
            "retrieval vector anchor cardinality mismatch: manifest={expected_points} core={}",
            expected_anchors.len()
        );
    }
    let compatibility_identity = vector_compatibility_identity(evidence)?;
    let contract = VectorEvidenceContract::new(
        manifest.embedding_backend.as_deref().unwrap_or_default(),
        usize::try_from(manifest.embedding_dim.unwrap_or_default()).unwrap_or_default(),
        crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
        compatibility_identity,
    );
    EmbeddedVectorIndex::validate_published_attestation(
        layout,
        &manifest.semantic_generation,
        generation,
        input_hash,
        &contract,
        &expected_anchors,
        vectors,
    )?;
    Ok(vector_manifest)
}

fn expected_vector_anchors(
    storage: &Store,
    publication: &codestory_store::IndexPublicationRecord,
) -> Result<Vec<ExpectedVectorAnchor>> {
    let expected_source_identity =
        format!("core:{}:{}", publication.generation_id, publication.run_id);
    let mut anchors = Vec::new();
    let mut after = None;
    loop {
        let batch = storage
            .get_dense_anchor_inputs_batch_after(after, 4_096)
            .context("load dense anchors for vector attestation")?;
        if batch.is_empty() {
            break;
        }
        after = batch.last().map(|anchor| anchor.node_id);
        for anchor in batch {
            if anchor.source_identity != expected_source_identity {
                bail!(
                    "dense anchor {} belongs to source identity {}, expected {}",
                    anchor.node_id.0,
                    anchor.source_identity,
                    expected_source_identity
                );
            }
            anchors.push(ExpectedVectorAnchor {
                node_id: anchor.node_id.0.to_string(),
                document_hash: anchor.document_hash,
            });
        }
    }
    Ok(anchors)
}

fn validate_execution_evidence_for_runtime(
    evidence: &EmbeddingVectorProducerEvidenceDto,
    runtime: &SidecarRuntimeConfig,
    embedding_device: &EmbeddingDeviceReadiness,
    live_identity: Option<&ProductEmbeddingIdentity>,
) -> Result<()> {
    if !embedding_device.full_retrieval_allowed {
        bail!("current embedding execution is not eligible for full retrieval");
    }
    match evidence.execution.observed_state.as_str() {
        "accelerated" => {
            if runtime.embedding.allow_cpu
                || !embedding_device.accelerator_requested
                || evidence.execution.smoke_elapsed_ms.is_none()
            {
                bail!("accelerated vector evidence is missing execution proof");
            }
        }
        "cpu_explicit" => {
            if !runtime.embedding.allow_cpu || !embedding_device.cpu_allowed {
                bail!("CPU vector evidence was not produced under explicit CPU policy");
            }
        }
        observed => bail!("unsupported vector execution evidence state {observed}"),
    }

    if let Some(identity) = live_identity {
        if !matches!(identity.residency, "resident" | "sleeping")
            || !identity.worker_alive
            || identity.load_error.is_some()
            || !identity.embedded_model
            || identity.model_digest != codestory_llama_sys::MODEL_SHA256
            || identity.ggml_build_identity != codestory_llama_sys::GGML_BUILD_IDENTITY
            || identity.policy != evidence.execution.observed_state
            || (identity.policy == "accelerated"
                && (!identity.accelerator_execution_verified
                    || identity.execution_observation_source != "ggml_eval_callback"
                    || identity.encode_count == 0
                    || identity.execution_node_count == 0
                    || identity.execution_device_names.is_empty()
                    || identity.execution_backend_names.is_empty()
                    || identity.offloaded_layer_count != identity.model_layer_count
                    || identity.resident_accelerator_tensor_count == 0
                    || identity.resident_accelerator_tensor_bytes == 0))
        {
            bail!("live embedding engine does not satisfy persisted execution evidence");
        }
    } else if !cfg!(feature = "test-support") {
        bail!("live embedding engine identity is required for vector admission");
    }
    Ok(())
}

pub(crate) fn vector_compatibility_identity(
    evidence: &codestory_contracts::api::EmbeddingVectorProducerEvidenceDto,
) -> Result<String> {
    let compatible = (
        evidence.schema_version,
        &evidence.producer,
        &evidence.model,
        &evidence.semantics,
        &evidence.engine,
        evidence.execution.eligibility.as_str(),
        evidence.execution.observed_state.as_str(),
        evidence.execution.observation_source.as_str(),
        evidence.execution.smoke_elapsed_ms.is_some(),
    );
    Ok(hex_digest(Sha256::digest(
        serde_json::to_vec(&compatible).context("serialize vector compatibility identity")?,
    )))
}

#[derive(Debug, Clone)]
pub(crate) struct EmbeddedVectorIndex {
    path: PathBuf,
    generation: String,
    input_hash: String,
    embedding: ProductEmbeddingClient,
}

impl EmbeddedVectorIndex {
    pub(crate) fn open(
        layout: &SidecarLayout,
        collection: &str,
        generation: &str,
        input_hash: &str,
        embedding: ProductEmbeddingClient,
    ) -> Self {
        Self {
            path: index_path(layout, collection),
            generation: generation.to_string(),
            input_hash: input_hash.to_string(),
            embedding,
        }
    }

    #[cfg(test)]
    pub(crate) fn build_with_points(
        layout: &SidecarLayout,
        collection: &str,
        generation: &str,
        input_hash: &str,
        embedding_backend: &str,
        embedding_dim: usize,
        produce: impl FnOnce(&mut dyn FnMut(SemanticPoint) -> Result<()>) -> Result<()>,
    ) -> Result<u64> {
        let contract = VectorEvidenceContract::legacy(embedding_backend, embedding_dim);
        build_and_publish_database(
            VectorDatabasePublication {
                layout,
                collection,
                generation,
                input_hash,
                contract: &contract,
                expected_anchors: None,
            },
            || Ok(()),
            |visit| {
                produce(&mut |point| {
                    let document_hash = legacy_document_hash(&point);
                    visit(AttestedSemanticPoint {
                        point,
                        document_hash,
                    })
                })
            },
        )
        .map(|attestation| attestation.point_count)
    }

    /// Build a vector database from one independently pinned anchor set.
    ///
    /// The expected anchors must come from the core publication rather than
    /// being inferred from produced vectors. This makes missing, unexpected,
    /// duplicate, and stale-document vectors publication failures.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn build_attested_with_points(
        layout: &SidecarLayout,
        collection: &str,
        generation: &str,
        input_hash: &str,
        contract: &VectorEvidenceContract,
        expected_anchors: &[ExpectedVectorAnchor],
        produce: impl FnOnce(&mut dyn FnMut(AttestedSemanticPoint) -> Result<()>) -> Result<()>,
    ) -> Result<VectorDatabaseAttestation> {
        Self::build_attested_with_points_with_cancel(
            AttestedVectorPublication {
                layout,
                collection,
                generation,
                input_hash,
                contract,
                expected_anchors,
            },
            || Ok(()),
            produce,
        )
    }

    pub(crate) fn build_attested_with_points_with_cancel(
        publication: AttestedVectorPublication<'_>,
        before_publish: impl FnOnce() -> Result<()>,
        produce: impl FnOnce(&mut dyn FnMut(AttestedSemanticPoint) -> Result<()>) -> Result<()>,
    ) -> Result<VectorDatabaseAttestation> {
        let expected_anchors = expected_anchor_map(publication.expected_anchors)?;
        build_and_publish_database(
            VectorDatabasePublication {
                layout: publication.layout,
                collection: publication.collection,
                generation: publication.generation,
                input_hash: publication.input_hash,
                contract: publication.contract,
                expected_anchors: Some(&expected_anchors),
            },
            before_publish,
            produce,
        )
    }

    /// Reconcile a new immutable vector generation from one fully attested
    /// predecessor without rewriting unchanged vector blobs.
    ///
    /// `Ok(None)` is the deliberate first-upgrade/corruption/filesystem
    /// fallback: callers must use the complete staged builder. Once a clone is
    /// established, cancellation or candidate construction failure is
    /// returned rather than hidden behind a second build attempt.
    pub(crate) fn try_build_incremental_with_cancel(
        publication: AttestedVectorPublication<'_>,
        previous_collection: &str,
        expected_evidence: &EmbeddingVectorProducerEvidenceDto,
        current_anchors: &[CurrentVectorAnchor],
        before_publish: impl FnOnce() -> Result<()>,
        produce_missing: impl FnOnce(
            &[ExpectedVectorAnchor],
            &mut dyn FnMut(AttestedSemanticPoint) -> Result<()>,
        ) -> Result<()>,
    ) -> Result<Option<(VectorDatabaseAttestation, IncrementalVectorWork)>> {
        let expected_anchors = expected_anchor_map(publication.expected_anchors)?;
        let current_anchors = current_vector_anchor_map(current_anchors, &expected_anchors)?;
        let previous_manifest =
            match Self::load_generation_manifest(publication.layout, previous_collection) {
                Ok(manifest) => manifest,
                Err(_) => return Ok(None),
            };
        if previous_manifest.schema_version != VECTOR_GENERATION_MANIFEST_SCHEMA_VERSION
            || previous_manifest.vectors.component_schema_version != VECTOR_COMPONENT_SCHEMA_VERSION
        {
            return Ok(None);
        }
        if !vector_component_is_compatible(expected_evidence, &previous_manifest.evidence)? {
            return Ok(None);
        }
        let previous_path = index_path(publication.layout, previous_collection);
        let previous_anchors = match read_vector_anchor_map(&previous_path) {
            Ok(anchors) => anchors,
            Err(_) => return Ok(None),
        };
        if validate_database(
            &previous_path,
            &previous_manifest.vectors.generation,
            &previous_manifest.vectors.input_hash,
            publication.contract,
            &previous_anchors,
            Some(&previous_manifest.vectors),
        )
        .is_err()
        {
            return Ok(None);
        }
        crate::copy_on_write::make_file_immutable(&previous_path)?;

        let previous_current_anchors = match read_current_vector_anchor_map(&previous_path) {
            Ok(anchors) => anchors,
            Err(_) => return Ok(None),
        };

        let path = index_path(publication.layout, publication.collection);
        let parent = path
            .parent()
            .context("embedded vector index has no parent")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create embedded vector directory {}", parent.display()))?;
        let (temp_path, reserved) =
            codestory_workspace::atomic_file::create_unique_temp_file(&path, "vector-index")?;
        drop(reserved);
        std::fs::remove_file(&temp_path)
            .with_context(|| format!("release vector clone reservation {}", temp_path.display()))?;
        if previous_anchors == expected_anchors
            && previous_current_anchors == current_anchors
            && crate::copy_on_write::reference_file(&previous_path, &temp_path)?
        {
            let result = (|| {
                let mut attestation = previous_manifest.vectors.clone();
                attestation.generation = publication.generation.to_string();
                attestation.input_hash = publication.input_hash.to_string();
                before_publish()?;
                crate::copy_on_write::publish_immutable_file_atomic(&temp_path, &path)?;
                Ok((
                    attestation,
                    IncrementalVectorWork {
                        retained: u64::try_from(expected_anchors.len()).unwrap_or(u64::MAX),
                        inserted: 0,
                        removed: 0,
                        direct_reference: true,
                    },
                ))
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&temp_path);
            }
            return result.map(Some);
        }
        if !crate::copy_on_write::clone_file(&previous_path, &temp_path)? {
            return Ok(None);
        }
        crate::copy_on_write::make_file_owner_writable(&temp_path)?;

        let result = (|| {
            let work = reconcile_cloned_database(
                &temp_path,
                publication.generation,
                publication.input_hash,
                publication.contract,
                &expected_anchors,
                &current_anchors,
                produce_missing,
            )?;
            let attestation = validate_database(
                &temp_path,
                publication.generation,
                publication.input_hash,
                publication.contract,
                &expected_anchors,
                None,
            )?;
            before_publish()?;
            crate::copy_on_write::publish_immutable_file_atomic(&temp_path, &path)?;
            Ok((attestation, work))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp_path);
        }
        result.map(Some)
    }

    /// Revalidate a published database against manifest-carried evidence.
    ///
    /// Readers should call this before admitting a candidate generation. The
    /// returned value is identical to `expected_attestation` only when both
    /// canonical vector content and exact SQLite bytes still match.
    pub(crate) fn validate_published_attestation(
        layout: &SidecarLayout,
        collection: &str,
        generation: &str,
        input_hash: &str,
        contract: &VectorEvidenceContract,
        expected_anchors: &[ExpectedVectorAnchor],
        expected_attestation: &VectorDatabaseAttestation,
    ) -> Result<VectorDatabaseAttestation> {
        let expected_anchors = expected_anchor_map(expected_anchors)?;
        validate_database(
            &index_path(layout, collection),
            generation,
            input_hash,
            contract,
            &expected_anchors,
            Some(expected_attestation),
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn publish_generation_manifest(
        layout: &SidecarLayout,
        collection: &str,
        manifest: &VectorGenerationManifest,
    ) -> Result<()> {
        Self::publish_generation_manifest_with_cancel(layout, collection, manifest, || Ok(()))
    }

    pub(crate) fn publish_generation_manifest_with_cancel(
        layout: &SidecarLayout,
        collection: &str,
        manifest: &VectorGenerationManifest,
        before_publish: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        manifest.validate()?;
        let path = generation_manifest_path(layout, collection);
        let bytes =
            serde_json::to_vec_pretty(manifest).context("serialize vector generation manifest")?;
        codestory_workspace::atomic_file::write_file_atomic(
            &path,
            "vector-generation-manifest",
            |file| {
                use std::io::Write;
                file.write_all(&bytes)
                    .context("write vector generation manifest")
            },
            |temp_path| {
                let observed: VectorGenerationManifest = serde_json::from_slice(
                    &std::fs::read(temp_path)
                        .context("read temporary vector generation manifest")?,
                )
                .context("parse temporary vector generation manifest")?;
                observed.validate()?;
                if &observed != manifest {
                    bail!("temporary vector generation manifest changed before publication");
                }
                before_publish()?;
                Ok(())
            },
        )
        .with_context(|| format!("publish vector generation manifest {}", path.display()))
    }

    pub(crate) fn load_generation_manifest(
        layout: &SidecarLayout,
        collection: &str,
    ) -> Result<VectorGenerationManifest> {
        let path = generation_manifest_path(layout, collection);
        let manifest = serde_json::from_slice::<VectorGenerationManifest>(
            &std::fs::read(&path)
                .with_context(|| format!("read vector generation manifest {}", path.display()))?,
        )
        .with_context(|| format!("parse vector generation manifest {}", path.display()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Load vectors from one immutable predecessor only after its complete
    /// database and producer contract have been revalidated. Callers still
    /// match by both node and document hash before admitting a reused row.
    pub(crate) fn load_reusable_vectors(
        layout: &SidecarLayout,
        collection: &str,
        expected_evidence: &EmbeddingVectorProducerEvidenceDto,
        contract: &VectorEvidenceContract,
    ) -> Result<HashMap<(String, String), Vec<f32>>> {
        let manifest = Self::load_generation_manifest(layout, collection)?;
        if !vector_component_is_compatible(expected_evidence, &manifest.evidence)? {
            bail!("reusable vector producer evidence is incompatible");
        }
        let path = index_path(layout, collection);
        let connection = open_read_only(&path)?;
        let mut statement = connection
            .prepare("SELECT node_id, document_hash, vector FROM vectors ORDER BY node_id")?;
        let mut rows = statement.query([])?;
        let mut expected_anchors = BTreeMap::new();
        let mut vectors = HashMap::new();
        while let Some(row) = rows.next()? {
            let node_id = row.get::<_, String>(0)?;
            let document_hash = row.get::<_, String>(1)?;
            let bytes = row.get::<_, Vec<u8>>(2)?;
            validate_vector_bytes(&node_id, &bytes, contract.embedding_dim)?;
            if expected_anchors
                .insert(node_id.clone(), document_hash.clone())
                .is_some()
            {
                bail!("duplicate reusable embedded vector anchor {node_id}");
            }
            let vector = bytes
                .chunks_exact(4)
                .map(|chunk| {
                    f32::from_bits(u32::from_le_bytes(
                        chunk.try_into().expect("four-byte vector chunk"),
                    ))
                })
                .collect::<Vec<_>>();
            vectors.insert((node_id, document_hash), vector);
        }
        drop(rows);
        drop(statement);
        drop(connection);
        validate_database(
            &path,
            &manifest.vectors.generation,
            &manifest.vectors.input_hash,
            contract,
            &expected_anchors,
            Some(&manifest.vectors),
        )?;
        Ok(vectors)
    }

    pub(crate) fn health(
        layout: &SidecarLayout,
        collection: &str,
        generation: &str,
        input_hash: &str,
        expected_points: u64,
        embedding_backend: &str,
        embedding_dim: usize,
    ) -> EmbeddedVectorHealth {
        let started = Instant::now();
        let result = validate_health_database(
            &index_path(layout, collection),
            generation,
            input_hash,
            expected_points,
            embedding_backend,
            embedding_dim,
        );
        EmbeddedVectorHealth {
            ready: result.is_ok(),
            point_count: result.as_ref().map_or(0, |count| *count),
            latency_ms: started.elapsed().as_millis() as u64,
            detail: result.map_or_else(
                |error| format!("embedded vector index unavailable: {error:#}"),
                |count| format!("embedded SQLite vectors ready points_count={count}"),
            ),
        }
    }

    pub(crate) fn search(&self, query: &str, limit: usize) -> Result<Vec<CandidateHit>> {
        let vector = self.embedding.embed_query(query)?;
        search_database(
            &self.path,
            &self.generation,
            &self.input_hash,
            &vector,
            limit,
            || false,
        )
    }

    pub(crate) fn search_with_context(
        &self,
        query: &str,
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<CandidateHit>> {
        let timeout = context.timeout(std::time::Duration::from_secs(2))?;
        let vector = self
            .embedding
            .embed_query_with_control(query, Some(timeout), &|| context.is_cancelled())?;
        context.check_cancelled()?;
        let context = context.clone();
        search_database(
            &self.path,
            &self.generation,
            &self.input_hash,
            &vector,
            limit,
            move || context.is_cancelled(),
        )
    }

    pub(crate) fn search_batch(
        &self,
        queries: &[String],
        limit: usize,
        context: &SearchExecutionContext,
    ) -> Result<Vec<Vec<CandidateHit>>> {
        if queries.is_empty() {
            return Ok(Vec::new());
        }
        let mut vectors = Vec::with_capacity(queries.len());
        for query_batch in
            queries.chunks(crate::per_user_embedding::PER_USER_EMBEDDING_QUERY_BATCH_MAX)
        {
            let timeout = context.timeout(std::time::Duration::from_secs(2))?;
            let batch_vectors =
                self.embedding
                    .embed_queries_with_control(query_batch, Some(timeout), &|| {
                        context.is_cancelled()
                    })?;
            if batch_vectors.len() != query_batch.len() {
                bail!(
                    "embedding_vector_row_count_mismatch: expected={} observed={}",
                    query_batch.len(),
                    batch_vectors.len()
                );
            }
            vectors.extend(batch_vectors);
        }
        context.check_cancelled()?;
        let vector_queries = vectors
            .iter()
            .map(|vector| (vector.as_slice(), limit))
            .collect::<Vec<_>>();
        search_database_batch(
            &self.path,
            &self.generation,
            &self.input_hash,
            &vector_queries,
            || context.is_cancelled(),
        )
    }
}

pub(crate) fn index_path(layout: &SidecarLayout, collection: &str) -> PathBuf {
    layout
        .semantic_data_dir
        .join("collections")
        .join(collection)
        .join(VECTOR_INDEX_FILE)
}

fn generation_manifest_path(layout: &SidecarLayout, collection: &str) -> PathBuf {
    index_path(layout, collection)
        .parent()
        .expect("vector index path always has a collection parent")
        .join(VECTOR_GENERATION_MANIFEST_FILE)
}

#[derive(Debug)]
struct DatabaseMetadata {
    schema_version: i64,
    generation: String,
    input_hash: String,
    embedding_backend: String,
    embedding_dim: i64,
    point_count: i64,
    producer_identity: String,
    evidence_contract_identity: String,
    vector_digest: String,
    component_schema_version: u32,
    component_sha256: String,
}

fn build_and_publish_database(
    publication: VectorDatabasePublication<'_>,
    before_publish: impl FnOnce() -> Result<()>,
    produce: impl FnOnce(&mut dyn FnMut(AttestedSemanticPoint) -> Result<()>) -> Result<()>,
) -> Result<VectorDatabaseAttestation> {
    let VectorDatabasePublication {
        layout,
        collection,
        generation,
        input_hash,
        contract,
        expected_anchors,
    } = publication;
    contract.validate()?;
    if generation.trim().is_empty() || input_hash.trim().is_empty() {
        bail!("embedded vector publication identities must be non-empty");
    }
    let path = index_path(layout, collection);
    let parent = path
        .parent()
        .context("embedded vector index has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create embedded vector directory {}", parent.display()))?;
    let (temp_path, reserved) =
        codestory_workspace::atomic_file::create_unique_temp_file(&path, "vector-index")?;
    drop(reserved);
    let result = (|| {
        let actual_anchors = write_database(
            &temp_path,
            generation,
            input_hash,
            contract,
            expected_anchors,
            produce,
        )?;
        let authoritative_anchors = expected_anchors.unwrap_or(&actual_anchors);
        let attestation = validate_database(
            &temp_path,
            generation,
            input_hash,
            contract,
            authoritative_anchors,
            None,
        )?;
        before_publish()?;
        crate::copy_on_write::publish_immutable_file_atomic(&temp_path, &path)?;
        Ok(attestation)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn write_database(
    path: &Path,
    generation: &str,
    input_hash: &str,
    contract: &VectorEvidenceContract,
    expected_anchors: Option<&BTreeMap<String, String>>,
    produce: impl FnOnce(&mut dyn FnMut(AttestedSemanticPoint) -> Result<()>) -> Result<()>,
) -> Result<BTreeMap<String, String>> {
    let mut connection = Connection::open(sqlite_open_path(path))
        .with_context(|| format!("create embedded vector index {}", path.display()))?;
    // The staged file is deleted on any failure and only published after
    // `validate_database` passes, so a rollback journal adds no durability.
    // Keeping it off also matches the lexical shard builder and avoids the
    // derived `-journal` sibling, the longest path SQLite would create here.
    connection
        .execute_batch(
            "PRAGMA journal_mode=OFF;
         PRAGMA synchronous=FULL;
         CREATE TABLE metadata (
             singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
             schema_version INTEGER NOT NULL,
             generation TEXT NOT NULL,
             input_hash TEXT NOT NULL,
             embedding_backend TEXT NOT NULL,
             embedding_dim INTEGER NOT NULL,
             point_count INTEGER NOT NULL,
             producer_identity TEXT NOT NULL,
             evidence_contract_identity TEXT NOT NULL,
             vector_digest TEXT NOT NULL,
             component_schema_version INTEGER NOT NULL,
             component_sha256 TEXT NOT NULL
         );
         CREATE TABLE vectors (
             node_id TEXT PRIMARY KEY NOT NULL,
             document_hash TEXT NOT NULL,
             display_name TEXT NOT NULL,
             file_path TEXT,
             file_role TEXT,
             dense_reason TEXT,
             vector BLOB NOT NULL,
             vector_sha256 TEXT NOT NULL
         ) WITHOUT ROWID;
         CREATE TRIGGER vectors_vector_update_guard
         AFTER UPDATE OF vector ON vectors
         BEGIN
             UPDATE vectors SET vector_sha256 = 'invalid' WHERE node_id = NEW.node_id;
         END;",
        )
        .with_context(|| format!("create embedded vector schema {}", path.display()))?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .with_context(|| format!("begin embedded vector write transaction {}", path.display()))?;
    let mut actual_anchors = BTreeMap::new();
    {
        let mut insert = transaction
            .prepare(
                "INSERT INTO vectors (
                 node_id, document_hash, display_name, file_path, file_role, dense_reason,
                 vector, vector_sha256
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .with_context(|| format!("prepare embedded vector insert {}", path.display()))?;
        let mut visit = |attested: AttestedSemanticPoint| -> Result<()> {
            let AttestedSemanticPoint {
                point,
                document_hash,
            } = attested;
            if point.node_id.trim().is_empty() || document_hash.trim().is_empty() {
                bail!("embedded vector anchor identities must be non-empty");
            }
            validate_vector(&point.node_id, &point.vector, contract.embedding_dim)?;
            if let Some(expected_anchors) = expected_anchors {
                let expected_hash = expected_anchors.get(&point.node_id).with_context(|| {
                    format!("unexpected embedded vector anchor {}", point.node_id)
                })?;
                if expected_hash != &document_hash {
                    bail!(
                        "embedded vector document hash mismatch for node {}: expected {}, found {}",
                        point.node_id,
                        expected_hash,
                        document_hash
                    );
                }
            }
            if actual_anchors
                .insert(point.node_id.clone(), document_hash.clone())
                .is_some()
            {
                bail!("duplicate embedded vector anchor {}", point.node_id);
            }
            let bytes = vector_bytes(&point.vector);
            let vector_sha256 = hex_digest(Sha256::digest(&bytes));
            insert
                .execute(params![
                    point.node_id,
                    document_hash,
                    point.display_name,
                    point.file_path,
                    point.file_role.map(|role| role.as_str()),
                    point.dense_reason,
                    bytes,
                    vector_sha256,
                ])
                .with_context(|| format!("write embedded vector index {}", path.display()))?;
            Ok(())
        };
        produce(&mut visit)?;
    }
    if let Some(expected_anchors) = expected_anchors
        && &actual_anchors != expected_anchors
    {
        let missing = expected_anchors
            .keys()
            .filter(|node_id| !actual_anchors.contains_key(*node_id))
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        bail!(
            "embedded vector anchor coverage mismatch: expected {}, found {}, missing {:?}",
            expected_anchors.len(),
            actual_anchors.len(),
            missing
        );
    }
    let vector_digest = canonical_vector_component_digest(&transaction)
        .with_context(|| format!("digest embedded vector component {}", path.display()))?;
    transaction
        .execute(
            "INSERT INTO metadata VALUES (
                1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11
             )",
            params![
                VECTOR_INDEX_SCHEMA_VERSION,
                generation,
                input_hash,
                contract.embedding_backend,
                contract.embedding_dim as i64,
                actual_anchors.len() as i64,
                contract.producer_identity,
                contract.evidence_contract_identity,
                vector_digest,
                VECTOR_COMPONENT_SCHEMA_VERSION,
                vector_digest,
            ],
        )
        .with_context(|| format!("write embedded vector metadata {}", path.display()))?;
    transaction
        .commit()
        .with_context(|| format!("commit embedded vector index {}", path.display()))?;
    connection
        .execute_batch("PRAGMA optimize;")
        .with_context(|| format!("optimize embedded vector index {}", path.display()))?;
    drop(connection);
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("open embedded vector index for sync {}", path.display()))?
        .sync_all()
        .with_context(|| format!("sync embedded vector index {}", path.display()))?;
    Ok(actual_anchors)
}

fn current_vector_anchor_map(
    current_anchors: &[CurrentVectorAnchor],
    expected_anchors: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, CurrentVectorAnchor>> {
    let mut current = BTreeMap::new();
    for anchor in current_anchors {
        if anchor.node_id.trim().is_empty() || anchor.document_hash.trim().is_empty() {
            bail!("current embedded vector anchor identities must be non-empty");
        }
        if expected_anchors.get(&anchor.node_id) != Some(&anchor.document_hash) {
            bail!(
                "current embedded vector anchor {} does not match the expected publication",
                anchor.node_id
            );
        }
        if current
            .insert(anchor.node_id.clone(), anchor.clone())
            .is_some()
        {
            bail!(
                "duplicate current embedded vector anchor {}",
                anchor.node_id
            );
        }
    }
    if current.len() != expected_anchors.len() {
        bail!(
            "current embedded vector anchor coverage mismatch: expected {}, found {}",
            expected_anchors.len(),
            current.len()
        );
    }
    Ok(current)
}

fn read_vector_anchor_map(path: &Path) -> Result<BTreeMap<String, String>> {
    let connection = open_read_only(path)?;
    let mut statement =
        connection.prepare("SELECT node_id, document_hash FROM vectors ORDER BY node_id ASC")?;
    let mut rows = statement.query([])?;
    let mut anchors = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let node_id = row.get::<_, String>(0)?;
        let document_hash = row.get::<_, String>(1)?;
        if anchors.insert(node_id.clone(), document_hash).is_some() {
            bail!("duplicate embedded vector row {node_id}");
        }
    }
    Ok(anchors)
}

fn read_current_vector_anchor_map(path: &Path) -> Result<BTreeMap<String, CurrentVectorAnchor>> {
    let connection = open_read_only(path)?;
    let mut statement = connection.prepare(
        "SELECT node_id, document_hash, display_name, file_path, file_role, dense_reason
         FROM vectors ORDER BY node_id ASC",
    )?;
    let mut rows = statement.query([])?;
    let mut anchors = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let node_id = row.get::<_, String>(0)?;
        let file_role = row
            .get::<_, Option<String>>(4)?
            .map(|role| FileRole::from_db_value(&role));
        let anchor = CurrentVectorAnchor {
            node_id: node_id.clone(),
            document_hash: row.get(1)?,
            display_name: row.get(2)?,
            file_path: row.get(3)?,
            file_role,
            dense_reason: row.get(5)?,
        };
        if anchors.insert(node_id.clone(), anchor).is_some() {
            bail!("duplicate embedded vector row {node_id}");
        }
    }
    Ok(anchors)
}

fn reconcile_cloned_database(
    path: &Path,
    generation: &str,
    input_hash: &str,
    contract: &VectorEvidenceContract,
    expected_anchors: &BTreeMap<String, String>,
    current_anchors: &BTreeMap<String, CurrentVectorAnchor>,
    produce_missing: impl FnOnce(
        &[ExpectedVectorAnchor],
        &mut dyn FnMut(AttestedSemanticPoint) -> Result<()>,
    ) -> Result<()>,
) -> Result<IncrementalVectorWork> {
    let mut connection = Connection::open(sqlite_open_path(path))
        .with_context(|| format!("open cloned embedded vector index {}", path.display()))?;
    connection
        .execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=FULL;")
        .with_context(|| format!("configure cloned embedded vector index {}", path.display()))?;
    validate_sqlite_quick_check(&connection).with_context(|| {
        format!(
            "quick-check cloned embedded vector index {}",
            path.display()
        )
    })?;
    let cloned_metadata = read_metadata(&connection)?;
    if cloned_metadata.component_schema_version != VECTOR_COMPONENT_SCHEMA_VERSION
        || cloned_metadata.component_sha256.is_empty()
    {
        bail!("cloned vector database does not support incremental reconciliation");
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .with_context(|| format!("begin cloned vector reconciliation {}", path.display()))?;

    let existing = {
        let mut statement = transaction.prepare(
            "SELECT node_id, document_hash, display_name, file_path, file_role, dense_reason
             FROM vectors ORDER BY node_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let removed = existing
        .iter()
        .filter(|(node_id, document_hash, ..)| expected_anchors.get(node_id) != Some(document_hash))
        .count();
    let retained = existing.len().saturating_sub(removed);
    let retained_anchors = existing
        .iter()
        .filter(|(node_id, document_hash, ..)| expected_anchors.get(node_id) == Some(document_hash))
        .map(|(node_id, document_hash, ..)| (node_id.as_str(), document_hash.as_str()))
        .collect::<HashSet<_>>();

    for (node_id, document_hash, ..) in &existing {
        if expected_anchors.get(node_id) != Some(document_hash) {
            transaction.execute("DELETE FROM vectors WHERE node_id = ?1", params![node_id])?;
        }
    }
    for anchor in current_anchors.values() {
        let changed = transaction.execute(
            "UPDATE vectors
             SET display_name = ?2, file_path = ?3, file_role = ?4, dense_reason = ?5
             WHERE node_id = ?1 AND document_hash = ?6
               AND (display_name IS NOT ?2 OR file_path IS NOT ?3
                    OR file_role IS NOT ?4 OR dense_reason IS NOT ?5)",
            params![
                anchor.node_id,
                anchor.display_name,
                anchor.file_path,
                anchor.file_role.map(|role| role.as_str()),
                anchor.dense_reason,
                anchor.document_hash,
            ],
        )?;
        debug_assert!(changed <= 1);
    }

    let missing = expected_anchors
        .iter()
        .filter(|(node_id, document_hash)| {
            !retained_anchors.contains(&(node_id.as_str(), document_hash.as_str()))
        })
        .map(|(node_id, document_hash)| ExpectedVectorAnchor {
            node_id: node_id.clone(),
            document_hash: document_hash.clone(),
        })
        .collect::<Vec<_>>();
    let missing_map = expected_anchor_map(&missing)?;
    let mut inserted = BTreeMap::new();
    {
        let mut insert = transaction.prepare(
            "INSERT INTO vectors (
                node_id, document_hash, display_name, file_path, file_role, dense_reason,
                vector, vector_sha256
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        let mut visit = |attested: AttestedSemanticPoint| -> Result<()> {
            let AttestedSemanticPoint {
                point,
                document_hash,
            } = attested;
            let expected_hash = missing_map
                .get(&point.node_id)
                .with_context(|| format!("unexpected incremental vector {}", point.node_id))?;
            if expected_hash != &document_hash {
                bail!(
                    "incremental vector document hash mismatch for node {}",
                    point.node_id
                );
            }
            validate_vector(&point.node_id, &point.vector, contract.embedding_dim)?;
            if inserted
                .insert(point.node_id.clone(), document_hash.clone())
                .is_some()
            {
                bail!("duplicate incremental embedded vector {}", point.node_id);
            }
            let bytes = vector_bytes(&point.vector);
            let vector_sha256 = hex_digest(Sha256::digest(&bytes));
            insert.execute(params![
                point.node_id,
                document_hash,
                point.display_name,
                point.file_path,
                point.file_role.map(|role| role.as_str()),
                point.dense_reason,
                bytes,
                vector_sha256,
            ])?;
            Ok(())
        };
        produce_missing(&missing, &mut visit)?;
    }
    if inserted != missing_map {
        bail!(
            "incremental embedded vector coverage mismatch: expected {}, found {}",
            missing_map.len(),
            inserted.len()
        );
    }

    let vector_digest = canonical_vector_component_digest(&transaction)?;
    transaction.execute("DELETE FROM metadata", [])?;
    transaction.execute(
        "INSERT INTO metadata VALUES (
            1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11
         )",
        params![
            VECTOR_INDEX_SCHEMA_VERSION,
            generation,
            input_hash,
            contract.embedding_backend,
            contract.embedding_dim as i64,
            expected_anchors.len() as i64,
            contract.producer_identity,
            contract.evidence_contract_identity,
            vector_digest,
            VECTOR_COMPONENT_SCHEMA_VERSION,
            vector_digest,
        ],
    )?;
    transaction.commit()?;
    drop(connection);
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .sync_all()?;
    Ok(IncrementalVectorWork {
        retained: u64::try_from(retained).unwrap_or(u64::MAX),
        inserted: u64::try_from(missing.len()).unwrap_or(u64::MAX),
        removed: u64::try_from(removed).unwrap_or(u64::MAX),
        direct_reference: false,
    })
}

pub(crate) fn validate_database(
    path: &Path,
    generation: &str,
    input_hash: &str,
    contract: &VectorEvidenceContract,
    expected_anchors: &BTreeMap<String, String>,
    expected_attestation: Option<&VectorDatabaseAttestation>,
) -> Result<VectorDatabaseAttestation> {
    contract.validate()?;
    let connection = open_read_only(path)?;
    validate_sqlite_quick_check(&connection)
        .with_context(|| format!("quick-check embedded vector index {}", path.display()))?;
    let metadata = read_metadata(&connection)
        .with_context(|| format!("read embedded vector metadata {}", path.display()))?;
    let physical_envelope_is_compatible =
        if metadata.component_schema_version == VECTOR_COMPONENT_SCHEMA_VERSION {
            !metadata.generation.trim().is_empty() && !metadata.input_hash.trim().is_empty()
        } else {
            metadata.generation == generation && metadata.input_hash == input_hash
        };
    if metadata.schema_version != VECTOR_INDEX_SCHEMA_VERSION
        || !physical_envelope_is_compatible
        || metadata.embedding_backend != contract.embedding_backend
        || metadata.embedding_dim != contract.embedding_dim as i64
        || metadata.point_count < 0
        || metadata.point_count as usize != expected_anchors.len()
        || metadata.producer_identity != contract.producer_identity
        || metadata.evidence_contract_identity != contract.evidence_contract_identity
    {
        bail!("embedded vector metadata does not match the evidence contract");
    }
    let actual_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))
        .with_context(|| format!("count embedded vector rows {}", path.display()))?;
    if actual_count < 0 || actual_count as usize != expected_anchors.len() {
        bail!(
            "embedded vector count mismatch: expected {}, found {}",
            expected_anchors.len(),
            actual_count.max(0)
        );
    }
    let (vector_digest, actual_anchors, database_sha256, component_sha256) =
        if metadata.component_schema_version == VECTOR_COMPONENT_SCHEMA_VERSION {
            let (digest, count) =
                validate_and_digest_vector_component(&connection, expected_anchors).with_context(
                    || format!("validate embedded vector component {}", path.display()),
                )?;
            if digest != metadata.component_sha256 {
                bail!("embedded vector physical component digest does not match metadata");
            }
            (digest.clone(), count, digest.clone(), digest)
        } else if metadata.component_schema_version == 1 {
            let (digest, count) =
                validate_and_digest_vectors(&connection, contract.embedding_dim, expected_anchors)
                    .with_context(|| format!("validate embedded vector rows {}", path.display()))?;
            (digest, count, sha256_file(path)?, String::new())
        } else {
            bail!("unsupported embedded vector component schema");
        };
    if actual_anchors != expected_anchors.len() || vector_digest != metadata.vector_digest {
        bail!("embedded vector canonical digest does not match metadata");
    }
    let attestation = VectorDatabaseAttestation {
        schema_version: metadata.schema_version,
        generation: if metadata.component_schema_version == VECTOR_COMPONENT_SCHEMA_VERSION {
            generation.to_string()
        } else {
            metadata.generation
        },
        input_hash: if metadata.component_schema_version == VECTOR_COMPONENT_SCHEMA_VERSION {
            input_hash.to_string()
        } else {
            metadata.input_hash
        },
        embedding_backend: metadata.embedding_backend,
        embedding_dim: metadata.embedding_dim as usize,
        point_count: metadata.point_count as u64,
        producer_identity: metadata.producer_identity,
        evidence_contract_identity: metadata.evidence_contract_identity,
        vector_digest,
        database_sha256,
        component_schema_version: metadata.component_schema_version,
        component_sha256,
        database_size_bytes: if metadata.component_schema_version == 1 {
            0
        } else {
            std::fs::metadata(path)
                .with_context(|| format!("inspect embedded vector database {}", path.display()))?
                .len()
        },
    };
    if let Some(expected) = expected_attestation
        && expected != &attestation
    {
        bail!("embedded vector database attestation does not match the manifest");
    }
    Ok(attestation)
}

fn validate_health_database(
    path: &Path,
    generation: &str,
    input_hash: &str,
    expected_points: u64,
    embedding_backend: &str,
    embedding_dim: usize,
) -> Result<u64> {
    let connection = open_read_only(path)?;
    let metadata = read_metadata(&connection)?;
    let envelope_matches =
        vector_publication_envelope_matches(path, &metadata, generation, input_hash)?;
    if metadata.schema_version != VECTOR_INDEX_SCHEMA_VERSION
        || !envelope_matches
        || metadata.embedding_backend != embedding_backend
        || metadata.embedding_dim != embedding_dim as i64
        || metadata.point_count < 0
        || metadata.point_count as u64 != expected_points
    {
        bail!("embedded vector metadata does not match the published generation");
    }
    let actual: i64 = connection.query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))?;
    if actual < 0 || actual as u64 != expected_points {
        bail!(
            "embedded vector count mismatch: expected {expected_points}, found {}",
            actual.max(0)
        );
    }
    if metadata.component_schema_version == VECTOR_COMPONENT_SCHEMA_VERSION {
        let digest = canonical_vector_component_digest(&connection)?;
        if digest != metadata.component_sha256 {
            bail!("embedded vector physical component digest mismatch");
        }
    }
    Ok(actual as u64)
}

fn vector_publication_envelope_matches(
    path: &Path,
    metadata: &DatabaseMetadata,
    generation: &str,
    input_hash: &str,
) -> Result<bool> {
    let manifest_path = path
        .parent()
        .context("embedded vector database has no generation directory")?
        .join(VECTOR_GENERATION_MANIFEST_FILE);
    match std::fs::symlink_metadata(&manifest_path) {
        Ok(file_metadata) => {
            if file_metadata.file_type().is_symlink() || !file_metadata.is_file() {
                bail!("vector generation manifest is not a regular file");
            }
            let manifest: VectorGenerationManifest =
                serde_json::from_slice(&std::fs::read(&manifest_path).with_context(|| {
                    format!(
                        "read vector generation manifest {}",
                        manifest_path.display()
                    )
                })?)?;
            manifest.validate()?;
            Ok(metadata.point_count >= 0
                && metadata.embedding_dim >= 0
                && manifest.vectors.generation == generation
                && manifest.vectors.input_hash == input_hash
                && manifest.vectors.component_schema_version == metadata.component_schema_version
                && manifest.vectors.component_sha256 == metadata.component_sha256
                && manifest.vectors.embedding_backend == metadata.embedding_backend
                && manifest.vectors.embedding_dim == metadata.embedding_dim as usize
                && manifest.vectors.point_count == metadata.point_count as u64)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(metadata.generation == generation && metadata.input_hash == input_hash)
        }
        Err(error) => Err(error).context("inspect vector generation manifest"),
    }
}

fn expected_anchor_map(
    expected_anchors: &[ExpectedVectorAnchor],
) -> Result<BTreeMap<String, String>> {
    let mut anchors = BTreeMap::new();
    for anchor in expected_anchors {
        if anchor.node_id.trim().is_empty() || anchor.document_hash.trim().is_empty() {
            bail!("expected embedded vector anchor identities must be non-empty");
        }
        if anchors
            .insert(anchor.node_id.clone(), anchor.document_hash.clone())
            .is_some()
        {
            bail!(
                "duplicate expected embedded vector anchor {}",
                anchor.node_id
            );
        }
    }
    Ok(anchors)
}

fn validate_sqlite_quick_check(connection: &Connection) -> Result<()> {
    let quick_check: String =
        connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if quick_check != "ok" {
        bail!("embedded vector SQLite quick_check failed: {quick_check}");
    }
    Ok(())
}

fn read_metadata(connection: &Connection) -> Result<DatabaseMetadata> {
    let metadata_rows: i64 =
        connection.query_row("SELECT COUNT(*) FROM metadata", [], |row| row.get(0))?;
    if metadata_rows != 1 {
        bail!("embedded vector metadata must contain exactly one row");
    }
    let has_component_metadata =
        table_has_column(connection, "metadata", "component_schema_version")?
            && table_has_column(connection, "metadata", "component_sha256")?;
    let query = if has_component_metadata {
        "SELECT schema_version, generation, input_hash, embedding_backend,
                embedding_dim, point_count, producer_identity,
                evidence_contract_identity, vector_digest,
                component_schema_version, component_sha256
         FROM metadata WHERE singleton = 1"
    } else {
        "SELECT schema_version, generation, input_hash, embedding_backend,
                embedding_dim, point_count, producer_identity,
                evidence_contract_identity, vector_digest, 1, ''
         FROM metadata WHERE singleton = 1"
    };
    connection
        .query_row(query, [], |row| {
            Ok(DatabaseMetadata {
                schema_version: row.get(0)?,
                generation: row.get(1)?,
                input_hash: row.get(2)?,
                embedding_backend: row.get(3)?,
                embedding_dim: row.get(4)?,
                point_count: row.get(5)?,
                producer_identity: row.get(6)?,
                evidence_contract_identity: row.get(7)?,
                vector_digest: row.get(8)?,
                component_schema_version: row.get(9)?,
                component_sha256: row.get(10)?,
            })
        })
        .context("read the single embedded vector metadata row")
}

fn table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    for observed in columns {
        if observed? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn canonical_vector_component_digest(connection: &Connection) -> Result<String> {
    digest_vector_component_rows(connection, None).map(|(digest, _)| digest)
}

fn validate_and_digest_vector_component(
    connection: &Connection,
    expected_anchors: &BTreeMap<String, String>,
) -> Result<(String, usize)> {
    digest_vector_component_rows(connection, Some(expected_anchors))
}

fn digest_vector_component_rows(
    connection: &Connection,
    expected_anchors: Option<&BTreeMap<String, String>>,
) -> Result<(String, usize)> {
    if !table_has_column(connection, "vectors", "vector_sha256")? {
        bail!("embedded vector component is missing row digests");
    }
    let mut statement = connection.prepare(
        "SELECT node_id, document_hash, display_name, file_path, file_role, dense_reason,
                vector, vector_sha256
         FROM vectors ORDER BY node_id ASC",
    )?;
    let mut rows = statement.query([])?;
    let mut digest = Sha256::new();
    digest.update(VECTOR_COMPONENT_DIGEST_DOMAIN);
    let mut seen = BTreeSet::new();
    while let Some(row) = rows.next()? {
        let node_id = row.get::<_, String>(0)?;
        let document_hash = row.get::<_, String>(1)?;
        let display_name = row.get::<_, String>(2)?;
        let file_path = row.get::<_, Option<String>>(3)?;
        let file_role = row.get::<_, Option<String>>(4)?;
        let dense_reason = row.get::<_, Option<String>>(5)?;
        let vector = row.get::<_, Vec<u8>>(6)?;
        let vector_sha256 = row.get::<_, String>(7)?;
        if !seen.insert(node_id.clone()) {
            bail!("duplicate embedded vector row {node_id}");
        }
        if node_id.trim().is_empty()
            || document_hash.trim().is_empty()
            || !is_sha256_hex(&vector_sha256)
        {
            bail!("embedded vector canonical digest row identities are invalid");
        }
        let observed_vector_sha256 = hex_digest(Sha256::digest(&vector));
        if observed_vector_sha256 != vector_sha256 {
            bail!("embedded vector blob digest mismatch for node {node_id}");
        }
        if let Some(expected_anchors) = expected_anchors
            && expected_anchors.get(&node_id) != Some(&document_hash)
        {
            bail!("embedded vector document hash mismatch for node {node_id}");
        }
        hash_len_prefixed(&mut digest, node_id.as_bytes());
        hash_len_prefixed(&mut digest, document_hash.as_bytes());
        hash_len_prefixed(&mut digest, display_name.as_bytes());
        hash_len_prefixed(
            &mut digest,
            file_path.as_deref().unwrap_or_default().as_bytes(),
        );
        hash_len_prefixed(
            &mut digest,
            file_role.as_deref().unwrap_or_default().as_bytes(),
        );
        hash_len_prefixed(
            &mut digest,
            dense_reason.as_deref().unwrap_or_default().as_bytes(),
        );
        hash_len_prefixed(&mut digest, vector_sha256.as_bytes());
    }
    if let Some(expected_anchors) = expected_anchors
        && seen.len() != expected_anchors.len()
    {
        bail!(
            "embedded vector component coverage mismatch: expected {}, found {}",
            expected_anchors.len(),
            seen.len()
        );
    }
    Ok((hex_digest(digest.finalize()), seen.len()))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_and_digest_vectors(
    connection: &Connection,
    embedding_dim: usize,
    expected_anchors: &BTreeMap<String, String>,
) -> Result<(String, usize)> {
    digest_vector_rows(connection, embedding_dim, Some(expected_anchors))
}

fn digest_vector_rows(
    connection: &Connection,
    embedding_dim: usize,
    expected_anchors: Option<&BTreeMap<String, String>>,
) -> Result<(String, usize)> {
    let mut statement = connection
        .prepare("SELECT node_id, document_hash, vector FROM vectors ORDER BY node_id ASC")?;
    let mut rows = statement.query([])?;
    let mut digest = Sha256::new();
    digest.update(VECTOR_DIGEST_DOMAIN);
    let mut seen = BTreeSet::new();
    while let Some(row) = rows.next()? {
        let node_id: String = row.get(0)?;
        let document_hash: String = row.get(1)?;
        let vector: Vec<u8> = row.get(2)?;
        if !seen.insert(node_id.clone()) {
            bail!("duplicate embedded vector row {node_id}");
        }
        if node_id.trim().is_empty() || document_hash.trim().is_empty() {
            bail!("embedded vector row identities must be non-empty");
        }
        if let Some(expected_anchors) = expected_anchors {
            let expected_hash = expected_anchors
                .get(&node_id)
                .with_context(|| format!("unexpected embedded vector row {node_id}"))?;
            if expected_hash != &document_hash {
                bail!(
                    "embedded vector document hash mismatch for node {node_id}: expected {expected_hash}, found {document_hash}"
                );
            }
        }
        validate_vector_bytes(&node_id, &vector, embedding_dim)?;
        hash_len_prefixed(&mut digest, node_id.as_bytes());
        hash_len_prefixed(&mut digest, document_hash.as_bytes());
        hash_len_prefixed(&mut digest, &vector);
    }
    if let Some(expected_anchors) = expected_anchors
        && seen.len() != expected_anchors.len()
    {
        let missing = expected_anchors
            .keys()
            .filter(|node_id| !seen.contains(*node_id))
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        bail!(
            "embedded vector row coverage mismatch: expected {}, found {}, missing {:?}",
            expected_anchors.len(),
            seen.len(),
            missing
        );
    }
    Ok((hex_digest(digest.finalize()), seen.len()))
}

fn validate_vector(node_id: &str, vector: &[f32], embedding_dim: usize) -> Result<()> {
    if vector.len() != embedding_dim {
        bail!(
            "embedded vector dimension mismatch for node {node_id}: expected {embedding_dim}, found {}",
            vector.len()
        );
    }
    validate_vector_values(node_id, vector.iter().copied())
}

fn validate_vector_bytes(node_id: &str, bytes: &[u8], embedding_dim: usize) -> Result<()> {
    let expected_bytes = embedding_dim
        .checked_mul(std::mem::size_of::<f32>())
        .context("embedded vector byte width overflow")?;
    if bytes.len() != expected_bytes {
        bail!(
            "embedded vector blob width mismatch for node {node_id}: expected {expected_bytes}, found {}",
            bytes.len()
        );
    }
    validate_vector_values(
        node_id,
        bytes.chunks_exact(4).map(|chunk| {
            f32::from_bits(u32::from_le_bytes(
                chunk.try_into().expect("four-byte vector chunk"),
            ))
        }),
    )
}

fn validate_vector_values(node_id: &str, values: impl Iterator<Item = f32>) -> Result<()> {
    let mut norm_squared = 0.0_f64;
    for value in values {
        if !value.is_finite() {
            bail!("embedded vector contains a non-finite value for node {node_id}");
        }
        norm_squared += f64::from(value) * f64::from(value);
    }
    if !norm_squared.is_finite() || norm_squared <= f64::EPSILON {
        bail!("embedded vector is zero or invalid for node {node_id}");
    }
    let norm = norm_squared.sqrt();
    if (norm - 1.0).abs() > VECTOR_NORM_TOLERANCE {
        bail!("embedded vector is not L2-normalized for node {node_id}: norm={norm:.8}");
    }
    Ok(())
}

#[cfg(test)]
fn legacy_document_hash(point: &SemanticPoint) -> String {
    let mut digest = Sha256::new();
    digest.update(b"codestory-legacy-vector-document-v1\0");
    hash_len_prefixed(&mut digest, point.node_id.as_bytes());
    hash_len_prefixed(&mut digest, point.display_name.as_bytes());
    hash_len_prefixed(
        &mut digest,
        point.file_path.as_deref().unwrap_or_default().as_bytes(),
    );
    hash_len_prefixed(
        &mut digest,
        point
            .file_role
            .as_ref()
            .map(|role| role.as_str())
            .unwrap_or_default()
            .as_bytes(),
    );
    hash_len_prefixed(
        &mut digest,
        point.dense_reason.as_deref().unwrap_or_default().as_bytes(),
    );
    hash_len_prefixed(&mut digest, &vector_bytes(&point.vector));
    hex_digest(digest.finalize())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| {
        format!(
            "open embedded vector database for hashing {}",
            path.display()
        )
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("hash embedded vector database {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex_digest(digest.finalize()))
}

fn hash_len_prefixed(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Read every published vector back out of a generation.
///
/// The bake-off builds candidate backends from exactly the bytes the shipped
/// scan reads, so a candidate cannot be measured against a friendlier copy of
/// the corpus than the incumbent gets.
#[cfg(feature = "benchmark-support")]
pub(crate) fn read_published_vectors_for_benchmark(
    path: &Path,
    embedding_dim: usize,
) -> Result<Vec<(String, Vec<f32>)>> {
    let connection = open_read_only(path)?;
    let mut statement =
        connection.prepare("SELECT node_id, vector FROM vectors ORDER BY node_id ASC")?;
    let mut rows = statement.query([])?;
    let mut vectors = Vec::new();
    while let Some(row) = rows.next()? {
        let node_id: String = row.get(0)?;
        let bytes: Vec<u8> = row.get(1)?;
        validate_vector_bytes(&node_id, &bytes, embedding_dim)?;
        vectors.push((
            node_id,
            bytes
                .chunks_exact(4)
                .map(|chunk| {
                    f32::from_bits(u32::from_le_bytes(
                        chunk.try_into().expect("four-byte vector chunk"),
                    ))
                })
                .collect(),
        ));
    }
    Ok(vectors)
}

pub(crate) fn search_database(
    path: &Path,
    generation: &str,
    input_hash: &str,
    query: &[f32],
    limit: usize,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<CandidateHit>> {
    search_database_with_abstention(path, generation, input_hash, query, limit, cancelled, true)
}

fn search_database_with_abstention(
    path: &Path,
    generation: &str,
    input_hash: &str,
    query: &[f32],
    limit: usize,
    cancelled: impl Fn() -> bool,
    apply_abstention: bool,
) -> Result<Vec<CandidateHit>> {
    let mut results = search_database_batch_with_abstention(
        path,
        generation,
        input_hash,
        &[(query, limit)],
        cancelled,
        apply_abstention,
    )?;
    Ok(results.pop().unwrap_or_default())
}

pub(crate) fn search_database_batch(
    path: &Path,
    generation: &str,
    input_hash: &str,
    queries: &[(&[f32], usize)],
    cancelled: impl Fn() -> bool,
) -> Result<Vec<Vec<CandidateHit>>> {
    search_database_batch_with_abstention(path, generation, input_hash, queries, cancelled, true)
}

fn search_database_batch_with_abstention(
    path: &Path,
    generation: &str,
    input_hash: &str,
    queries: &[(&[f32], usize)],
    cancelled: impl Fn() -> bool,
    apply_abstention: bool,
) -> Result<Vec<Vec<CandidateHit>>> {
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    if queries.iter().all(|(_, limit)| *limit == 0) {
        return Ok(vec![Vec::new(); queries.len()]);
    }
    let connection = open_read_only(path)?;
    let metadata = read_metadata(&connection)?;
    if !vector_publication_envelope_matches(path, &metadata, generation, input_hash)?
        || metadata.generation.trim().is_empty()
        || metadata.input_hash.trim().is_empty()
        || queries
            .iter()
            .any(|(query, limit)| *limit != 0 && metadata.embedding_dim != query.len() as i64)
    {
        bail!("embedded vector index publication identity changed");
    }
    let query_norms = queries
        .iter()
        .map(|(query, limit)| {
            if *limit == 0 {
                return Ok(0.0);
            }
            if query.is_empty() || query.iter().any(|value| !value.is_finite()) {
                bail!("embedded vector query is empty or contains a non-finite value");
            }
            let query_norm = query
                .iter()
                .map(|value| f64::from(*value) * f64::from(*value))
                .sum::<f64>()
                .sqrt();
            if !query_norm.is_finite() || query_norm <= f64::EPSILON {
                bail!("embedded vector query has zero or invalid norm");
            }
            Ok(query_norm)
        })
        .collect::<Result<Vec<_>>>()?;
    let has_vector_hash = table_has_column(&connection, "vectors", "vector_sha256")?;
    let query = if has_vector_hash {
        "SELECT node_id, display_name, file_path, file_role, dense_reason, vector,
                vector_sha256 FROM vectors"
    } else {
        "SELECT node_id, display_name, file_path, file_role, dense_reason, vector,
                '' FROM vectors"
    };
    let mut statement = connection.prepare(query)?;
    let mut rows = statement.query([])?;
    let mut scored = queries
        .iter()
        .map(|(_, limit)| Vec::with_capacity(*limit))
        .collect::<Vec<Vec<ScoredHit>>>();
    while let Some(row) = rows.next()? {
        if cancelled() {
            bail!("embedded vector search cancelled");
        }
        let bytes: Vec<u8> = row.get(5)?;
        let expected_vector_sha256 = row.get::<_, String>(6)?;
        if !expected_vector_sha256.is_empty()
            && hex_digest(Sha256::digest(&bytes)) != expected_vector_sha256
        {
            bail!("embedded vector row digest mismatch");
        }
        let node_id = row.get::<_, String>(0)?;
        let display_name = row.get::<_, String>(1)?;
        let file_path = row.get::<_, Option<String>>(2)?;
        let file_role = row.get::<_, Option<String>>(3)?;
        let dense_reason = row.get::<_, Option<String>>(4)?;
        for (query_index, ((query, limit), query_norm)) in
            queries.iter().zip(&query_norms).enumerate()
        {
            if *limit == 0 {
                continue;
            }
            let score = cosine_similarity_bytes(query, *query_norm, &bytes)?;
            let candidate = (
                score,
                node_id.clone(),
                display_name.clone(),
                file_path.clone(),
                file_role.clone(),
                dense_reason.clone(),
            );
            let query_scored = &mut scored[query_index];
            if query_scored.len() < *limit {
                query_scored.push(candidate);
                continue;
            }
            let (worst_index, worst) = query_scored
                .iter()
                .enumerate()
                .max_by(|(_, left), (_, right)| compare_scored_hits(left, right))
                .expect("non-empty bounded score set");
            if compare_scored_hits(&candidate, worst) == Ordering::Less {
                query_scored[worst_index] = candidate;
            }
        }
    }
    scored
        .into_iter()
        .map(|mut scored| {
            scored.sort_unstable_by(compare_scored_hits);
            if apply_abstention {
                retain_dense_evidence(&mut scored);
            }
            Ok(scored_hits_to_candidates(scored))
        })
        .collect()
}

fn scored_hits_to_candidates(scored: Vec<ScoredHit>) -> Vec<CandidateHit> {
    scored
        .into_iter()
        .map(
            |(score, node_id, display_name, file_path, file_role, dense_reason)| {
                let file_path = file_path.unwrap_or_else(|| display_name.clone());
                let mut hit = CandidateHit::with_source(
                    file_path,
                    Some(display_name),
                    score,
                    CandidateSource::Semantic,
                );
                hit.node_id = Some(node_id);
                hit.file_role = file_role
                    .as_deref()
                    .map(codestory_store::FileRole::from_db_value);
                hit.add_provenance(if dense_reason.as_deref() == Some("component_report") {
                    "component_report"
                } else {
                    "dense_anchor"
                });
                hit
            },
        )
        .collect()
}

/// Drop the neighbours this lane cannot claim are related, leaving the whole
/// stage empty when none survive.
///
/// A bounded scan always fills its window: without this the top `limit`
/// vectors are reported even at zero or negative cosine, so a query with no
/// dense evidence still emits a full set of confident-looking anchors.
/// Requires `scored` sorted by descending similarity.
fn retain_dense_evidence(scored: &mut Vec<ScoredHit>) {
    let Some(best) = scored.first().map(|hit| hit.0) else {
        return;
    };
    scored.retain(|hit| {
        hit.0.is_finite()
            && hit.0 >= DENSE_ABSTENTION_ABSOLUTE_FLOOR
            && hit.0 >= best - DENSE_ABSTENTION_ADDITIVE_MARGIN
    });
}

#[cfg(feature = "semantic-calibration-support")]
pub(crate) fn search_database_for_semantic_calibration(
    path: &Path,
    generation: &str,
    input_hash: &str,
    query: &[f32],
    limit: usize,
) -> Result<Vec<CandidateHit>> {
    search_database_with_abstention(path, generation, input_hash, query, limit, || false, false)
}

fn compare_scored_hits(left: &ScoredHit, right: &ScoredHit) -> Ordering {
    right
        .0
        .total_cmp(&left.0)
        .then_with(|| left.1.cmp(&right.1))
}

pub(crate) fn open_read_only(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(
        sqlite_open_path(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open embedded vector index {}", path.display()))
}

fn vector_bytes(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(vector));
    for value in vector {
        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    bytes
}

fn cosine_similarity_bytes(query: &[f32], query_norm: f64, bytes: &[u8]) -> Result<f32> {
    if bytes.len() != std::mem::size_of_val(query) {
        bail!("embedded vector blob has an invalid width");
    }
    let mut dot = 0.0_f64;
    let mut vector_norm = 0.0_f64;
    for (query_value, chunk) in query.iter().zip(bytes.chunks_exact(4)) {
        let value = f32::from_bits(u32::from_le_bytes(chunk.try_into().expect("four bytes")));
        if !value.is_finite() {
            bail!("embedded vector contains a non-finite value during search");
        }
        dot += f64::from(*query_value) * f64::from(value);
        vector_norm += f64::from(value) * f64::from(value);
    }
    let denominator = query_norm * vector_norm.sqrt();
    if !denominator.is_finite() || denominator <= f64::EPSILON {
        bail!("embedded vector has zero or invalid norm during search");
    }
    let score = dot / denominator;
    if !score.is_finite() || !(-1.0 - 1e-6..=1.0 + 1e-6).contains(&score) {
        bail!("embedded vector cosine score is non-finite or outside [-1, 1]");
    }
    Ok(score.clamp(-1.0, 1.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SidecarLayout;
    use codestory_contracts::graph::{Node, NodeId, NodeKind};
    use codestory_store::{
        DenseAnchorInput, FileRole, IndexPublicationMode, IndexPublicationRecord,
        RetrievalIndexManifest,
    };
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn layout(root: &Path) -> SidecarLayout {
        SidecarLayout {
            lexical_data_dir: root.join("lexical"),
            semantic_data_dir: root.join("semantic"),
            scip_artifacts_root: root.join("scip"),
            state_file: root.join("state.json"),
        }
    }

    fn point(node_id: &str, vector: Vec<f32>) -> SemanticPoint {
        SemanticPoint {
            display_name: format!("symbol_{node_id}"),
            node_id: node_id.into(),
            file_path: Some(format!("src/{node_id}.rs")),
            file_role: Some(FileRole::Source),
            dense_reason: Some("public_api".into()),
            vector,
        }
    }

    fn attested_point(
        node_id: &str,
        document_hash: &str,
        vector: Vec<f32>,
    ) -> AttestedSemanticPoint {
        AttestedSemanticPoint {
            point: point(node_id, vector),
            document_hash: document_hash.into(),
        }
    }

    fn evidence_contract() -> VectorEvidenceContract {
        VectorEvidenceContract::new("backend", 2, "producer-v1", "evidence-contract-v1")
    }

    fn expected_anchors() -> Vec<ExpectedVectorAnchor> {
        vec![
            ExpectedVectorAnchor {
                node_id: "1".into(),
                document_hash: "document-1".into(),
            },
            ExpectedVectorAnchor {
                node_id: "2".into(),
                document_hash: "document-2".into(),
            },
        ]
    }

    fn current_anchor(
        node_id: &str,
        document_hash: &str,
        display_name: &str,
    ) -> CurrentVectorAnchor {
        CurrentVectorAnchor {
            node_id: node_id.into(),
            document_hash: document_hash.into(),
            display_name: display_name.into(),
            file_path: Some(format!("src/{node_id}.rs")),
            file_role: Some(FileRole::Source),
            dense_reason: Some("public_api".into()),
        }
    }

    fn accelerated_device() -> EmbeddingDeviceReadiness {
        EmbeddingDeviceReadiness {
            requested_policy: "accelerator_required",
            observed_state: "accelerated",
            observation_source: "per_user_server",
            detected_provider: Some("metal".into()),
            detected_gpu: Some("test accelerator".into()),
            accelerator_requested: true,
            accelerator_request_provider: Some("metal".into()),
            accelerator_request_device: Some("test accelerator".into()),
            cpu_allowed: false,
            full_retrieval_allowed: true,
            degraded_reason: None,
        }
    }

    fn accelerated_identity() -> ProductEmbeddingIdentity {
        ProductEmbeddingIdentity {
            instance_id: "inprocess:test".into(),
            load_generation: 1,
            model_load_count: 1,
            residency: "resident",
            worker_alive: true,
            load_error: None,
            model_digest: codestory_llama_sys::MODEL_SHA256,
            ggml_build_identity: codestory_llama_sys::GGML_BUILD_IDENTITY,
            backend: "Metal".into(),
            adapter_name: "test accelerator".into(),
            adapter_description: "test".into(),
            policy: "accelerated",
            embedded_model: true,
            materialized_path: PathBuf::from("model.gguf"),
            materialized_reused: true,
            initialization_ms: 1,
            smoke_ms: 1,
            adapter_memory_total: 1,
            adapter_memory_used_by_load: 1,
            execution_device_names: vec!["test accelerator".into()],
            execution_backend_names: vec!["Metal".into()],
            execution_observation_source: "ggml_eval_callback",
            encode_count: 1,
            execution_node_count: 1,
            resident_accelerator_tensor_count: 1,
            resident_accelerator_tensor_bytes: 1,
            model_layer_count: 13,
            offloaded_layer_count: 13,
            accelerator_execution_verified: true,
        }
    }

    fn reader_runtime(root: &Path, layout: &SidecarLayout) -> SidecarRuntimeConfig {
        let mut runtime = SidecarRuntimeConfig::local();
        runtime.cache_root = root.join("cache");
        runtime.layout = layout.clone();
        runtime.embedding.allow_cpu = false;
        runtime
    }

    fn reader_publication() -> IndexPublicationRecord {
        IndexPublicationRecord {
            generation: 1,
            generation_id: "core-generation-v1".into(),
            run_id: "core-run-v1".into(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        }
    }

    fn reader_manifest(embedding_backend: &str) -> RetrievalIndexManifest {
        RetrievalIndexManifest {
            project_id: "reader-project".into(),
            lexical_version: crate::lexical_index::LEXICAL_INDEX_VERSION.into(),
            semantic_generation: "codestory_reader_admission".into(),
            scip_revision: Some("reader-revision".into()),
            built_at_epoch_ms: 1,
            disk_bytes: None,
            degraded_modes_json: "[]".into(),
            embedding_backend: Some(embedding_backend.into()),
            embedding_dim: Some(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32),
            sidecar_schema_version: Some(crate::generation::SIDECAR_SCHEMA_VERSION),
            sidecar_input_hash: Some("reader-input-v1".into()),
            sidecar_generation: Some("reader-generation-v1".into()),
            projection_count: Some(1),
            symbol_doc_count: Some(1),
            dense_projection_count: Some(1),
            semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
            graph_artifact_hash: Some("reader-graph-v1".into()),
            dense_reason_counts_json: Some("{\"public_api\":1}".into()),
            precise_semantic_import_status: None,
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: None,
            precise_semantic_import_producer: None,
        }
    }

    fn seed_reader_store(path: &Path, publication: &IndexPublicationRecord) -> Store {
        let mut storage = Store::open(path).expect("open reader store");
        storage
            .insert_nodes_batch(&[Node {
                id: NodeId(1),
                kind: NodeKind::FUNCTION,
                serialized_name: "reader_symbol".into(),
                ..Default::default()
            }])
            .expect("insert reader node");
        storage
            .upsert_dense_anchor_inputs_batch(&[DenseAnchorInput {
                node_id: NodeId(1),
                file_node_id: None,
                kind: NodeKind::FUNCTION,
                display_name: "reader_symbol".into(),
                qualified_name: Some("reader::symbol".into()),
                file_path: Some("src/lib.rs".into()),
                start_line: Some(1),
                end_line: Some(2),
                file_role: FileRole::Source,
                source_provenance: "parser".into(),
                text: "reader semantic document".into(),
                document_hash: "reader-document-v1".into(),
                selection_reason: "public_api".into(),
                policy_version: crate::generation::SEMANTIC_POLICY_VERSION.into(),
                source_identity: "core:unpublished:unpublished".into(),
                updated_at_epoch_ms: 1,
            }])
            .expect("insert dense anchor");
        storage
            .publish_dense_anchor_generation(
                publication,
                crate::generation::SEMANTIC_POLICY_VERSION,
            )
            .expect("publish dense anchors");
        storage
            .put_index_publication(publication)
            .expect("publish core generation");
        storage
    }

    fn publish_reader_generation(
        layout: &SidecarLayout,
        storage: &Store,
        manifest: &RetrievalIndexManifest,
        publication: &IndexPublicationRecord,
        device: &EmbeddingDeviceReadiness,
        identity: &ProductEmbeddingIdentity,
        mutate_evidence: impl FnOnce(&mut EmbeddingVectorProducerEvidenceDto),
    ) -> VectorGenerationManifest {
        let mut evidence = build_vector_producer_evidence(
            device,
            Some(identity),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            EmbeddingVectorPublicationIdentityDto {
                core_generation_id: publication.generation_id.clone(),
                core_run_id: publication.run_id.clone(),
                retrieval_generation: manifest
                    .sidecar_generation
                    .clone()
                    .expect("retrieval generation"),
                retrieval_input_hash: manifest
                    .sidecar_input_hash
                    .clone()
                    .expect("retrieval input"),
                semantic_generation: manifest.semantic_generation.clone(),
            },
        );
        mutate_evidence(&mut evidence);
        let contract = VectorEvidenceContract::new(
            manifest
                .embedding_backend
                .clone()
                .expect("embedding backend"),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            vector_compatibility_identity(&evidence).expect("compatibility identity"),
        );
        let expected = expected_vector_anchors(storage, publication).expect("expected anchors");
        let mut vector = vec![0.0_f32; crate::embeddings::RETRIEVAL_EMBEDDING_DIM];
        vector[0] = 1.0;
        let attestation = EmbeddedVectorIndex::build_attested_with_points(
            layout,
            &manifest.semantic_generation,
            manifest
                .sidecar_generation
                .as_deref()
                .expect("retrieval generation"),
            manifest
                .sidecar_input_hash
                .as_deref()
                .expect("retrieval input"),
            &contract,
            &expected,
            |visit| visit(attested_point("1", "reader-document-v1", vector)),
        )
        .expect("build reader vector database");
        let generation_manifest =
            VectorGenerationManifest::new(evidence, attestation).expect("generation manifest");
        EmbeddedVectorIndex::publish_generation_manifest(
            layout,
            &manifest.semantic_generation,
            &generation_manifest,
        )
        .expect("publish generation manifest");
        generation_manifest
    }

    fn assert_evidence_mismatch(
        expected: &EmbeddingVectorProducerEvidenceDto,
        field: &str,
        mutate: impl FnOnce(&mut EmbeddingVectorProducerEvidenceDto),
    ) {
        let mut observed = expected.clone();
        mutate(&mut observed);
        assert!(
            producer_evidence_mismatches(expected, &observed)
                .iter()
                .any(|mismatch| mismatch == field),
            "missing compatibility check for {field}"
        );
    }

    #[test]
    fn immutable_index_is_generation_bound_and_ranks_cosine_similarity() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let points = [point("1", vec![1.0, 0.0]), point("2", vec![0.0, 1.0])];
        EmbeddedVectorIndex::build_with_points(
            &layout,
            "codestory_test_deadbeefdeadbeef",
            "test-deadbeefdeadbeef",
            "input",
            "backend",
            2,
            |visit| {
                for point in points {
                    visit(point)?;
                }
                Ok(())
            },
        )
        .expect("build");

        let path = index_path(&layout, "codestory_test_deadbeefdeadbeef");
        let hits = search_database(
            &path,
            "test-deadbeefdeadbeef",
            "input",
            &[0.9, 0.1],
            1,
            || false,
        )
        .expect("search");
        assert_eq!(hits[0].node_id.as_deref(), Some("1"));
        assert!(
            !EmbeddedVectorIndex::health(
                &layout,
                "codestory_test_deadbeefdeadbeef",
                "other-generation",
                "input",
                2,
                "backend",
                2,
            )
            .ready
        );
    }

    #[test]
    fn batch_scan_is_serial_equivalent_for_scores_ties_and_abstention() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let points = [
            point("a", vec![1.0, 0.0]),
            point("b", vec![1.0, 0.0]),
            point("c", vec![0.0, 1.0]),
        ];
        EmbeddedVectorIndex::build_with_points(
            &layout,
            "codestory_test_batch",
            "test-batch",
            "input",
            "backend",
            2,
            |visit| {
                for point in points {
                    visit(point)?;
                }
                Ok(())
            },
        )
        .expect("build");

        let path = index_path(&layout, "codestory_test_batch");
        let queries = [vec![0.9, 0.1], vec![0.1, 0.9], vec![-1.0, 0.0]];
        let serial = queries
            .iter()
            .map(|query| search_database(&path, "test-batch", "input", query, 2, || false))
            .collect::<Result<Vec<_>>>()
            .expect("serial scans");
        let batch_queries = queries
            .iter()
            .map(|query| (query.as_slice(), 2))
            .collect::<Vec<_>>();
        let batch = search_database_batch(&path, "test-batch", "input", &batch_queries, || false)
            .expect("batch scan");

        assert_eq!(batch, serial);
    }

    #[test]
    fn dense_search_abstains_instead_of_filling_its_window_with_unrelated_vectors() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let points = [
            point("related", vec![1.0, 0.0]),
            point("near", vec![0.707_106_77, 0.707_106_77]),
            point("distant", vec![0.258_819_04, 0.965_925_8]),
            point("opposed", vec![-1.0, 0.0]),
        ];
        EmbeddedVectorIndex::build_with_points(
            &layout,
            "codestory_abstention_deadbeefdeadbeef",
            "abstention-deadbeefdeadbeef",
            "input",
            "backend",
            2,
            |visit| {
                for point in points {
                    visit(point)?;
                }
                Ok(())
            },
        )
        .expect("build");
        let path = index_path(&layout, "codestory_abstention_deadbeefdeadbeef");

        let hits = search_database(
            &path,
            "abstention-deadbeefdeadbeef",
            "input",
            &[1.0, 0.0],
            4,
            || false,
        )
        .expect("search");
        assert_eq!(
            hits.iter()
                .map(|hit| hit.node_id.as_deref().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["related"],
            "the window must not be padded with vectors the lane cannot claim"
        );

        let abstained = search_database(
            &path,
            "abstention-deadbeefdeadbeef",
            "input",
            &[0.0, -1.0],
            4,
            || false,
        )
        .expect("search");
        assert!(
            abstained.is_empty(),
            "no positively related vector must yield no dense evidence: {abstained:?}"
        );
    }

    #[test]
    fn dense_abstention_requires_absolute_and_additive_evidence() {
        let hit = |score: f32, id: &str| {
            (
                score,
                id.to_string(),
                id.to_string(),
                Some(format!("{id}.rs")),
                None,
                None,
            )
        };
        let mut scored = vec![
            hit(0.42, "best"),
            hit(0.35, "near"),
            hit(0.29, "below-floor"),
        ];
        retain_dense_evidence(&mut scored);
        assert_eq!(
            scored
                .iter()
                .map(|candidate| candidate.1.as_str())
                .collect::<Vec<_>>(),
            vec!["best", "near"]
        );

        let mut unsupported = vec![hit(0.29, "best"), hit(0.28, "near")];
        retain_dense_evidence(&mut unsupported);
        assert!(unsupported.is_empty());
    }

    #[test]
    fn vector_publication_survives_cache_roots_beyond_max_path() {
        // Regression: NT service profiles and isolated proof harnesses resolve
        // cache roots deep enough that the staged vector database exceeds the
        // 260-character Windows MAX_PATH cap for non-longPathAware processes.
        // Publication, validation, health, and search must all keep working.
        let root = tempdir().expect("tempdir");
        let mut deep_root = root.path().to_path_buf();
        let segment = "max-path-regression-padding-segment".repeat(2);
        while deep_root.as_os_str().len() < 320 {
            deep_root.push(&segment);
        }
        std::fs::create_dir_all(&deep_root).expect("create deep cache root");
        let layout = layout(&deep_root);
        let collection = "codestory_longpath_deadbeefdeadbeef";
        EmbeddedVectorIndex::build_with_points(
            &layout,
            collection,
            "longpath-deadbeefdeadbeef",
            "input",
            "backend",
            2,
            |visit| {
                visit(point("1", vec![1.0, 0.0]))?;
                visit(point("2", vec![0.0, 1.0]))
            },
        )
        .expect("publish embedded vector index under a deep cache root");

        let path = index_path(&layout, collection);
        assert!(
            path.as_os_str().len() > 260,
            "regression layout no longer exceeds MAX_PATH: {}",
            path.display()
        );
        assert!(
            EmbeddedVectorIndex::health(
                &layout,
                collection,
                "longpath-deadbeefdeadbeef",
                "input",
                2,
                "backend",
                2,
            )
            .ready
        );
        let hits = search_database(
            &path,
            "longpath-deadbeefdeadbeef",
            "input",
            &[0.9, 0.1],
            1,
            || false,
        )
        .expect("search embedded vector index under a deep cache root");
        assert_eq!(hits[0].node_id.as_deref(), Some("1"));
    }

    #[test]
    fn query_vectors_and_scores_fail_closed_on_invalid_numeric_evidence() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        EmbeddedVectorIndex::build_with_points(
            &layout,
            "codestory_query_validation",
            "generation-v1",
            "input-v1",
            "backend",
            2,
            |visit| visit(point("1", vec![1.0, 0.0])),
        )
        .expect("build");
        let path = index_path(&layout, "codestory_query_validation");

        for query in [[f32::NAN, 0.0], [0.0, 0.0]] {
            let error = search_database(&path, "generation-v1", "input-v1", &query, 1, || false)
                .expect_err("invalid query vector must fail closed");
            assert!(
                error.to_string().contains("query"),
                "unexpected invalid query error: {error:#}"
            );
        }

        let non_finite_bytes = vector_bytes(&[f32::INFINITY, 0.0]);
        assert!(
            cosine_similarity_bytes(&[1.0, 0.0], 1.0, &non_finite_bytes)
                .expect_err("invalid stored vector must fail closed")
                .to_string()
                .contains("non-finite")
        );
    }

    #[test]
    fn attested_index_is_canonical_and_revalidates_manifest_evidence() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let contract = evidence_contract();
        let expected = expected_anchors();
        let points = [
            attested_point("2", "document-2", vec![0.0, 1.0]),
            attested_point("1", "document-1", vec![1.0, 0.0]),
        ];
        let attestation = EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "codestory_attested",
            "generation-v1",
            "input-v1",
            &contract,
            &expected,
            |visit| {
                for point in points {
                    visit(point)?;
                }
                Ok(())
            },
        )
        .expect("build attested vectors");

        assert_eq!(attestation.point_count, 2);
        assert_eq!(attestation.vector_digest.len(), 64);
        assert_eq!(attestation.database_sha256.len(), 64);
        assert_eq!(attestation.producer_identity, "producer-v1");
        let second_envelope = EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "codestory_attested_second_envelope",
            "generation-v2",
            "input-v2",
            &contract,
            &expected,
            |visit| {
                visit(attested_point("1", "document-1", vec![1.0, 0.0]))?;
                visit(attested_point("2", "document-2", vec![0.0, 1.0]))
            },
        )
        .expect("build same component under a second envelope");
        assert_eq!(
            attestation.component_sha256, second_envelope.component_sha256,
            "physical component identity must not include the core-bound publication envelope"
        );
        assert_ne!(attestation.generation, second_envelope.generation);
        assert_ne!(attestation.input_hash, second_envelope.input_hash);
        assert_eq!(
            EmbeddedVectorIndex::validate_published_attestation(
                &layout,
                "codestory_attested",
                "generation-v1",
                "input-v1",
                &contract,
                &expected,
                &attestation,
            )
            .expect("validate published attestation"),
            attestation
        );

        let connection = Connection::open(index_path(&layout, "codestory_attested"))
            .expect("open attested database");
        assert!(
            connection
                .execute(
                    "INSERT INTO metadata SELECT * FROM metadata WHERE singleton = 1",
                    [],
                )
                .is_err(),
            "metadata singleton must reject a second row"
        );
    }

    #[test]
    fn cancellation_before_vector_and_evidence_publication_preserves_prior_files() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let device = accelerated_device();
        let identity = accelerated_identity();
        let dimension = crate::embeddings::RETRIEVAL_EMBEDDING_DIM;
        let unit_vector = |axis: usize| {
            let mut vector = vec![0.0_f32; dimension];
            vector[axis] = 1.0;
            vector
        };
        let evidence = build_vector_producer_evidence(
            &device,
            Some(&identity),
            dimension as u32,
            EmbeddingVectorPublicationIdentityDto {
                core_generation_id: "core-generation-v1".into(),
                core_run_id: "core-run-v1".into(),
                retrieval_generation: "generation-v1".into(),
                retrieval_input_hash: "input-v1".into(),
                semantic_generation: "codestory_cancelled_publication".into(),
            },
        );
        let contract = VectorEvidenceContract::new(
            "backend",
            dimension,
            "producer-v1",
            vector_compatibility_identity(&evidence).expect("compatibility identity"),
        );
        let expected = expected_anchors();
        let attestation = EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "codestory_cancelled_publication",
            "generation-v1",
            "input-v1",
            &contract,
            &expected,
            |visit| {
                visit(attested_point("1", "document-1", unit_vector(0)))?;
                visit(attested_point("2", "document-2", unit_vector(1)))
            },
        )
        .expect("publish initial vectors");
        let manifest = VectorGenerationManifest::new(evidence, attestation)
            .expect("build initial evidence manifest");
        EmbeddedVectorIndex::publish_generation_manifest(
            &layout,
            "codestory_cancelled_publication",
            &manifest,
        )
        .expect("publish initial evidence manifest");

        let vector_path = index_path(&layout, "codestory_cancelled_publication");
        let evidence_path = generation_manifest_path(&layout, "codestory_cancelled_publication");
        let prior_vectors = std::fs::read(&vector_path).expect("read prior vectors");
        let prior_evidence = std::fs::read(&evidence_path).expect("read prior evidence");

        let vector_error = EmbeddedVectorIndex::build_attested_with_points_with_cancel(
            AttestedVectorPublication {
                layout: &layout,
                collection: "codestory_cancelled_publication",
                generation: "generation-v1",
                input_hash: "input-v1",
                contract: &contract,
                expected_anchors: &expected,
            },
            || bail!("simulated cancellation before vector database publication"),
            |visit| {
                visit(attested_point("1", "document-1", unit_vector(1)))?;
                visit(attested_point("2", "document-2", unit_vector(0)))
            },
        )
        .expect_err("cancelled vector publication must fail");
        assert!(vector_error.to_string().contains("simulated cancellation"));
        assert_eq!(
            std::fs::read(&vector_path).expect("read vectors after cancellation"),
            prior_vectors,
            "cancelled vector publication replaced the prior database"
        );

        let evidence_error = EmbeddedVectorIndex::publish_generation_manifest_with_cancel(
            &layout,
            "codestory_cancelled_publication",
            &manifest,
            || bail!("simulated cancellation before evidence publication"),
        )
        .expect_err("cancelled evidence publication must fail");
        assert!(
            format!("{evidence_error:#}").contains("simulated cancellation"),
            "unexpected evidence cancellation error: {evidence_error:#}"
        );
        assert_eq!(
            std::fs::read(&evidence_path).expect("read evidence after cancellation"),
            prior_evidence,
            "cancelled evidence publication replaced the prior manifest"
        );
    }

    #[test]
    fn same_generation_vector_retry_replaces_a_readonly_partial_component() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let contract = evidence_contract();
        let expected = expected_anchors();
        EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "partial",
            "generation-v1",
            "input-v1",
            &contract,
            &expected,
            |visit| {
                visit(attested_point("1", "document-1", vec![1.0, 0.0]))?;
                visit(attested_point("2", "document-2", vec![0.0, 1.0]))
            },
        )
        .expect("publish component before envelope");
        let path = index_path(&layout, "partial");
        assert!(
            std::fs::metadata(&path)
                .expect("partial permissions")
                .permissions()
                .readonly()
        );

        EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "partial",
            "generation-v1",
            "input-v1",
            &contract,
            &expected,
            |visit| {
                visit(attested_point("1", "document-1", vec![0.0, 1.0]))?;
                visit(attested_point("2", "document-2", vec![1.0, 0.0]))
            },
        )
        .expect("repair same-generation partial component");

        assert!(
            std::fs::metadata(&path)
                .expect("repaired permissions")
                .permissions()
                .readonly()
        );
        assert_eq!(
            search_database(&path, "generation-v1", "input-v1", &[1.0, 0.0], 1, || false)
                .expect("search repaired component")[0]
                .node_id
                .as_deref(),
            Some("2")
        );
    }

    #[test]
    fn attested_index_rejects_inexact_anchor_coverage_and_invalid_vectors() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let contract = evidence_contract();
        let expected = expected_anchors();

        let missing = EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "codestory_missing",
            "generation-v1",
            "input-v1",
            &contract,
            &expected,
            |visit| visit(attested_point("1", "document-1", vec![1.0, 0.0])),
        )
        .expect_err("missing anchor must fail");
        assert!(format!("{missing:#}").contains("coverage mismatch"));

        let wrong_hash = EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "codestory_wrong_hash",
            "generation-v1",
            "input-v1",
            &contract,
            &expected,
            |visit| {
                visit(attested_point("1", "stale-document", vec![1.0, 0.0]))?;
                visit(attested_point("2", "document-2", vec![0.0, 1.0]))
            },
        )
        .expect_err("wrong document hash must fail");
        assert!(format!("{wrong_hash:#}").contains("document hash mismatch"));

        for (collection, vector, expected_message) in [
            ("codestory_zero", vec![0.0, 0.0], "zero or invalid"),
            ("codestory_non_finite", vec![f32::NAN, 0.0], "non-finite"),
            (
                "codestory_not_normalized",
                vec![1.0, 1.0],
                "not L2-normalized",
            ),
        ] {
            let error = EmbeddedVectorIndex::build_attested_with_points(
                &layout,
                collection,
                "generation-v1",
                "input-v1",
                &contract,
                &[ExpectedVectorAnchor {
                    node_id: "1".into(),
                    document_hash: "document-1".into(),
                }],
                |visit| visit(attested_point("1", "document-1", vector)),
            )
            .expect_err("invalid vector must fail");
            assert!(format!("{error:#}").contains(expected_message));
        }
    }

    #[test]
    fn incremental_generation_reconciles_same_count_changes_without_rewriting_predecessor() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let device = accelerated_device();
        let identity = accelerated_identity();
        let evidence = build_vector_producer_evidence(
            &device,
            Some(&identity),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            EmbeddingVectorPublicationIdentityDto {
                core_generation_id: "core-v2".into(),
                core_run_id: "run-v2".into(),
                retrieval_generation: "generation-v2".into(),
                retrieval_input_hash: "input-v2".into(),
                semantic_generation: "current".into(),
            },
        );
        let contract = VectorEvidenceContract::new(
            "backend",
            2,
            "producer-v1",
            vector_compatibility_identity(&evidence).expect("compatibility"),
        );
        let previous_expected = vec![
            ExpectedVectorAnchor {
                node_id: "1".into(),
                document_hash: "document-1".into(),
            },
            ExpectedVectorAnchor {
                node_id: "2".into(),
                document_hash: "document-2".into(),
            },
        ];
        let previous_attestation = EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "previous",
            "generation-v1",
            "input-v1",
            &contract,
            &previous_expected,
            |visit| {
                visit(attested_point("1", "document-1", vec![1.0, 0.0]))?;
                visit(attested_point("2", "document-2", vec![0.0, 1.0]))
            },
        )
        .expect("build predecessor");
        let mut previous_evidence = evidence.clone();
        previous_evidence.publication = EmbeddingVectorPublicationIdentityDto {
            core_generation_id: "core-v1".into(),
            core_run_id: "run-v1".into(),
            retrieval_generation: "generation-v1".into(),
            retrieval_input_hash: "input-v1".into(),
            semantic_generation: "previous".into(),
        };
        let previous_manifest =
            VectorGenerationManifest::new(previous_evidence, previous_attestation)
                .expect("predecessor manifest");
        EmbeddedVectorIndex::publish_generation_manifest(&layout, "previous", &previous_manifest)
            .expect("publish predecessor manifest");
        let previous_bytes = std::fs::read(index_path(&layout, "previous")).expect("predecessor");

        let current_expected = vec![
            ExpectedVectorAnchor {
                node_id: "1".into(),
                document_hash: "document-1".into(),
            },
            ExpectedVectorAnchor {
                node_id: "3".into(),
                document_hash: "document-3".into(),
            },
        ];
        let outcome = EmbeddedVectorIndex::try_build_incremental_with_cancel(
            AttestedVectorPublication {
                layout: &layout,
                collection: "current",
                generation: "generation-v2",
                input_hash: "input-v2",
                contract: &contract,
                expected_anchors: &current_expected,
            },
            "previous",
            &evidence,
            &[
                current_anchor("1", "document-1", "renamed display metadata"),
                current_anchor("3", "document-3", "symbol_3"),
            ],
            || Ok(()),
            |missing, visit| {
                assert_eq!(missing.len(), 1);
                assert_eq!(missing[0].node_id, "3");
                visit(attested_point("3", "document-3", vec![0.0, 1.0]))
            },
        )
        .expect("incremental build");
        let Some((attestation, work)) = outcome else {
            return;
        };

        assert_eq!(work.retained, 1);
        assert_eq!(work.inserted, 1);
        assert_eq!(work.removed, 1);
        assert_eq!(attestation.point_count, 2);
        assert_eq!(
            std::fs::read(index_path(&layout, "previous")).expect("predecessor after build"),
            previous_bytes
        );
        let connection = open_read_only(&index_path(&layout, "current")).expect("current db");
        let retained_metadata: String = connection
            .query_row(
                "SELECT display_name FROM vectors WHERE node_id = '1'",
                [],
                |row| row.get(0),
            )
            .expect("retained metadata");
        assert_eq!(retained_metadata, "renamed display metadata");
        let removed_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM vectors WHERE node_id = '2'",
                [],
                |row| row.get(0),
            )
            .expect("removed row");
        assert_eq!(removed_count, 0);
        assert!(
            std::fs::metadata(index_path(&layout, "previous"))
                .expect("previous permissions")
                .permissions()
                .readonly()
        );
        assert!(
            std::fs::metadata(index_path(&layout, "current"))
                .expect("current permissions")
                .permissions()
                .readonly()
        );
    }

    #[test]
    fn publication_only_vector_churn_directly_references_the_component_without_clone() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let device = accelerated_device();
        let identity = accelerated_identity();
        let evidence = build_vector_producer_evidence(
            &device,
            Some(&identity),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            EmbeddingVectorPublicationIdentityDto {
                core_generation_id: "core-v2".into(),
                core_run_id: "run-v2".into(),
                retrieval_generation: "generation-v2".into(),
                retrieval_input_hash: "input-v2".into(),
                semantic_generation: "current".into(),
            },
        );
        let contract = VectorEvidenceContract::new(
            "backend",
            2,
            "producer-v1",
            vector_compatibility_identity(&evidence).expect("compatibility"),
        );
        let expected = expected_anchors();
        let previous_attestation = EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "previous",
            "generation-v1",
            "input-v1",
            &contract,
            &expected,
            |visit| {
                visit(attested_point("1", "document-1", vec![1.0, 0.0]))?;
                visit(attested_point("2", "document-2", vec![0.0, 1.0]))
            },
        )
        .expect("build predecessor");
        let mut previous_evidence = evidence.clone();
        previous_evidence.publication = EmbeddingVectorPublicationIdentityDto {
            core_generation_id: "core-v1".into(),
            core_run_id: "run-v1".into(),
            retrieval_generation: "generation-v1".into(),
            retrieval_input_hash: "input-v1".into(),
            semantic_generation: "previous".into(),
        };
        EmbeddedVectorIndex::publish_generation_manifest(
            &layout,
            "previous",
            &VectorGenerationManifest::new(previous_evidence, previous_attestation)
                .expect("predecessor manifest"),
        )
        .expect("publish predecessor manifest");
        let previous_path = index_path(&layout, "previous");

        let outcome = crate::copy_on_write::with_clone_disabled(|| {
            EmbeddedVectorIndex::try_build_incremental_with_cancel(
                AttestedVectorPublication {
                    layout: &layout,
                    collection: "current",
                    generation: "generation-v2",
                    input_hash: "input-v2",
                    contract: &contract,
                    expected_anchors: &expected,
                },
                "previous",
                &evidence,
                &[
                    current_anchor("1", "document-1", "symbol_1"),
                    current_anchor("2", "document-2", "symbol_2"),
                ],
                || Ok(()),
                |_, _| panic!("publication-only reuse must not request vector production"),
            )
        })
        .expect("publication-only vector build")
        .expect("direct-reference outcome");
        let (attestation, work) = outcome;
        assert!(work.direct_reference);
        assert_eq!(work.retained, 2);
        assert_eq!(work.inserted, 0);
        assert_eq!(work.removed, 0);
        let current_path = index_path(&layout, "current");
        assert_eq!(
            codestory_workspace::workspace_path_identity(&previous_path)
                .expect("previous identity"),
            codestory_workspace::workspace_path_identity(&current_path).expect("current identity"),
        );
        assert_eq!(attestation.generation, "generation-v2");
        assert_eq!(attestation.input_hash, "input-v2");
        assert!(
            std::fs::metadata(&previous_path)
                .expect("previous permissions")
                .permissions()
                .readonly()
        );
        assert!(
            std::fs::metadata(&current_path)
                .expect("current permissions")
                .permissions()
                .readonly()
        );
        EmbeddedVectorIndex::validate_published_attestation(
            &layout,
            "current",
            "generation-v2",
            "input-v2",
            &contract,
            &expected,
            &attestation,
        )
        .expect("validate current envelope over referenced vectors");
        EmbeddedVectorIndex::publish_generation_manifest(
            &layout,
            "current",
            &VectorGenerationManifest::new(evidence, attestation)
                .expect("current generation manifest"),
        )
        .expect("publish current generation manifest");
        search_database(
            &current_path,
            "generation-v2",
            "input-v2",
            &[1.0, 0.0],
            1,
            || false,
        )
        .expect("search current direct-reference envelope");
        for (generation, input_hash) in [
            ("wrong-generation", "input-v2"),
            ("generation-v2", "wrong-input"),
        ] {
            let error = search_database(
                &current_path,
                generation,
                input_hash,
                &[1.0, 0.0],
                1,
                || false,
            )
            .expect_err("direct-reference search must reject the wrong envelope");
            assert!(
                format!("{error:#}").contains("publication identity changed"),
                "unexpected error: {error:#}"
            );
        }
    }

    #[test]
    fn incremental_vector_cancellation_leaves_no_candidate_publication() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let evidence = build_vector_producer_evidence(
            &accelerated_device(),
            Some(&accelerated_identity()),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            EmbeddingVectorPublicationIdentityDto {
                core_generation_id: "core-v1".into(),
                core_run_id: "run-v1".into(),
                retrieval_generation: "generation-v1".into(),
                retrieval_input_hash: "input-v1".into(),
                semantic_generation: "previous".into(),
            },
        );
        let contract = VectorEvidenceContract::new(
            "backend",
            2,
            "producer-v1",
            vector_compatibility_identity(&evidence).expect("compatibility"),
        );
        let expected = expected_anchors();
        let previous_attestation = EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "previous",
            "generation-v1",
            "input-v1",
            &contract,
            &expected,
            |visit| {
                visit(attested_point("1", "document-1", vec![1.0, 0.0]))?;
                visit(attested_point("2", "document-2", vec![0.0, 1.0]))
            },
        )
        .expect("build predecessor");
        let manifest = VectorGenerationManifest::new(evidence.clone(), previous_attestation)
            .expect("manifest");
        EmbeddedVectorIndex::publish_generation_manifest(&layout, "previous", &manifest)
            .expect("publish predecessor manifest");

        let result = EmbeddedVectorIndex::try_build_incremental_with_cancel(
            AttestedVectorPublication {
                layout: &layout,
                collection: "cancelled",
                generation: "generation-v2",
                input_hash: "input-v2",
                contract: &contract,
                expected_anchors: &expected,
            },
            "previous",
            &evidence,
            &[
                current_anchor("1", "document-1", "symbol_1"),
                current_anchor("2", "document-2", "symbol_2"),
            ],
            || bail!("simulated incremental cancellation"),
            |missing, _| {
                assert!(missing.is_empty());
                Ok(())
            },
        );
        match result {
            Ok(None) => {}
            Err(error) => {
                assert!(format!("{error:#}").contains("simulated incremental cancellation"))
            }
            Ok(Some(_)) => panic!("cancelled vector candidate was published"),
        }
        assert!(!index_path(&layout, "cancelled").exists());
    }

    #[test]
    fn corrupt_vector_predecessor_requests_complete_fallback_without_a_candidate() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let evidence = build_vector_producer_evidence(
            &accelerated_device(),
            Some(&accelerated_identity()),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            EmbeddingVectorPublicationIdentityDto {
                core_generation_id: "core-v1".into(),
                core_run_id: "run-v1".into(),
                retrieval_generation: "generation-v1".into(),
                retrieval_input_hash: "input-v1".into(),
                semantic_generation: "previous".into(),
            },
        );
        let contract = VectorEvidenceContract::new(
            "backend",
            2,
            "producer-v1",
            vector_compatibility_identity(&evidence).expect("compatibility"),
        );
        let expected = expected_anchors();
        let attestation = EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "previous",
            "generation-v1",
            "input-v1",
            &contract,
            &expected,
            |visit| {
                visit(attested_point("1", "document-1", vec![1.0, 0.0]))?;
                visit(attested_point("2", "document-2", vec![0.0, 1.0]))
            },
        )
        .expect("build predecessor");
        let manifest =
            VectorGenerationManifest::new(evidence.clone(), attestation).expect("manifest");
        EmbeddedVectorIndex::publish_generation_manifest(&layout, "previous", &manifest)
            .expect("publish manifest");
        let previous_path = index_path(&layout, "previous");
        crate::copy_on_write::make_file_owner_writable(&previous_path)
            .expect("authorize hostile corruption");
        let connection = Connection::open(previous_path).expect("open vector db");
        connection
            .execute("DROP TRIGGER vectors_vector_update_guard", [])
            .expect("remove mutation guard");
        connection
            .execute(
                "UPDATE vectors SET vector = X'0000000000000000' WHERE node_id = '1'",
                [],
            )
            .expect("corrupt vector row");
        drop(connection);

        let outcome = EmbeddedVectorIndex::try_build_incremental_with_cancel(
            AttestedVectorPublication {
                layout: &layout,
                collection: "current",
                generation: "generation-v2",
                input_hash: "input-v2",
                contract: &contract,
                expected_anchors: &expected,
            },
            "previous",
            &evidence,
            &[
                current_anchor("1", "document-1", "symbol_1"),
                current_anchor("2", "document-2", "symbol_2"),
            ],
            || Ok(()),
            |_, _| panic!("corrupt predecessor must not enter differential production"),
        )
        .expect("fallback decision");

        assert!(outcome.is_none());
        assert!(!index_path(&layout, "current").exists());
    }

    #[test]
    fn legacy_vector_manifest_requests_complete_fallback_before_clone() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let evidence = build_vector_producer_evidence(
            &accelerated_device(),
            Some(&accelerated_identity()),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            EmbeddingVectorPublicationIdentityDto {
                core_generation_id: "core-v1".into(),
                core_run_id: "run-v1".into(),
                retrieval_generation: "generation-v1".into(),
                retrieval_input_hash: "input-v1".into(),
                semantic_generation: "previous".into(),
            },
        );
        let contract = VectorEvidenceContract::new(
            "backend",
            2,
            "producer-v1",
            vector_compatibility_identity(&evidence).expect("compatibility"),
        );
        let expected = expected_anchors();
        let attestation = EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "previous",
            "generation-v1",
            "input-v1",
            &contract,
            &expected,
            |visit| {
                visit(attested_point("1", "document-1", vec![1.0, 0.0]))?;
                visit(attested_point("2", "document-2", vec![0.0, 1.0]))
            },
        )
        .expect("build predecessor");
        let mut manifest =
            VectorGenerationManifest::new(evidence.clone(), attestation).expect("manifest");
        manifest.schema_version = 1;
        manifest.vectors.component_schema_version = 1;
        manifest.vectors.component_sha256.clear();
        std::fs::create_dir_all(
            generation_manifest_path(&layout, "previous")
                .parent()
                .expect("manifest parent"),
        )
        .expect("manifest parent");
        std::fs::write(
            generation_manifest_path(&layout, "previous"),
            serde_json::to_vec(&manifest).expect("legacy manifest JSON"),
        )
        .expect("legacy manifest");

        let outcome = EmbeddedVectorIndex::try_build_incremental_with_cancel(
            AttestedVectorPublication {
                layout: &layout,
                collection: "current",
                generation: "generation-v2",
                input_hash: "input-v2",
                contract: &contract,
                expected_anchors: &expected,
            },
            "previous",
            &evidence,
            &[
                current_anchor("1", "document-1", "symbol_1"),
                current_anchor("2", "document-2", "symbol_2"),
            ],
            || Ok(()),
            |_, _| panic!("legacy predecessor must not enter differential production"),
        )
        .expect("fallback decision");

        assert!(outcome.is_none());
        assert!(!index_path(&layout, "current").exists());
    }

    #[test]
    fn published_attestation_rejects_contract_and_database_drift() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let contract = evidence_contract();
        let expected = expected_anchors();
        let attestation = EmbeddedVectorIndex::build_attested_with_points(
            &layout,
            "codestory_drift",
            "generation-v1",
            "input-v1",
            &contract,
            &expected,
            |visit| {
                visit(attested_point("1", "document-1", vec![1.0, 0.0]))?;
                visit(attested_point("2", "document-2", vec![0.0, 1.0]))
            },
        )
        .expect("build attested vectors");

        let wrong_contract =
            VectorEvidenceContract::new("backend", 2, "different-producer", "evidence-contract-v1");
        assert!(
            EmbeddedVectorIndex::validate_published_attestation(
                &layout,
                "codestory_drift",
                "generation-v1",
                "input-v1",
                &wrong_contract,
                &expected,
                &attestation,
            )
            .is_err()
        );

        let drift_path = index_path(&layout, "codestory_drift");
        crate::copy_on_write::make_file_owner_writable(&drift_path)
            .expect("make database writable for hostile drift injection");
        std::fs::OpenOptions::new()
            .append(true)
            .open(drift_path)
            .expect("open database for drift")
            .write_all(b"drift")
            .expect("append database drift");
        assert!(
            EmbeddedVectorIndex::validate_published_attestation(
                &layout,
                "codestory_drift",
                "generation-v1",
                "input-v1",
                &contract,
                &expected,
                &attestation,
            )
            .is_err()
        );
    }

    #[test]
    fn reader_admission_revalidates_database_sha_digest_hashes_and_cardinality() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let runtime = reader_runtime(root.path(), &layout);
        let publication = reader_publication();
        let storage = seed_reader_store(&root.path().join("core.sqlite3"), &publication);
        let device = accelerated_device();
        let identity = accelerated_identity();
        let manifest = reader_manifest(&crate::embeddings::embedding_runtime_id_for_runtime(
            &runtime,
        ));
        let validate = || {
            validate_generation_evidence_for_publication(
                &layout,
                &storage,
                &manifest,
                &publication,
                &runtime,
                &device,
                Some(&identity),
            )
        };

        publish_reader_generation(
            &layout,
            &storage,
            &manifest,
            &publication,
            &device,
            &identity,
            |_| {},
        );
        validate().expect("admit complete reader generation");

        let vector_path = index_path(&layout, &manifest.semantic_generation);
        crate::copy_on_write::make_file_owner_writable(&vector_path)
            .expect("make database writable for hostile byte drift injection");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&vector_path)
            .expect("open database for exact-byte drift")
            .write_all(b"byte-drift")
            .expect("append exact-byte drift");
        let error = validate().expect_err("database SHA drift must fail admission");
        assert!(format!("{error:#}").contains("attestation"));

        publish_reader_generation(
            &layout,
            &storage,
            &manifest,
            &publication,
            &device,
            &identity,
            |_| {},
        );
        crate::copy_on_write::make_file_owner_writable(&vector_path)
            .expect("make database writable for hostile vector drift injection");
        let mut changed_vector = vec![0.0_f32; crate::embeddings::RETRIEVAL_EMBEDDING_DIM];
        changed_vector[1] = 1.0;
        Connection::open(index_path(&layout, &manifest.semantic_generation))
            .expect("open database for vector drift")
            .execute(
                "UPDATE vectors SET vector = ?1 WHERE node_id = '1'",
                params![vector_bytes(&changed_vector)],
            )
            .expect("change stored vector");
        let error = validate().expect_err("canonical vector drift must fail admission");
        assert!(format!("{error:#}").contains("canonical digest"));

        publish_reader_generation(
            &layout,
            &storage,
            &manifest,
            &publication,
            &device,
            &identity,
            |_| {},
        );
        crate::copy_on_write::make_file_owner_writable(&vector_path)
            .expect("make database writable for hostile document drift injection");
        Connection::open(index_path(&layout, &manifest.semantic_generation))
            .expect("open database for document drift")
            .execute(
                "UPDATE vectors SET document_hash = 'stale-document' WHERE node_id = '1'",
                [],
            )
            .expect("change document hash");
        let error = validate().expect_err("document hash drift must fail admission");
        assert!(format!("{error:#}").contains("document hash mismatch"));

        publish_reader_generation(
            &layout,
            &storage,
            &manifest,
            &publication,
            &device,
            &identity,
            |_| {},
        );
        crate::copy_on_write::make_file_owner_writable(&vector_path)
            .expect("make database writable for hostile cardinality drift injection");
        Connection::open(index_path(&layout, &manifest.semantic_generation))
            .expect("open database for cardinality drift")
            .execute("DELETE FROM vectors WHERE node_id = '1'", [])
            .expect("remove vector row");
        let error = validate().expect_err("vector cardinality drift must fail admission");
        assert!(format!("{error:#}").contains("count mismatch"));
    }

    #[test]
    fn reader_admission_rejects_a_generation_from_an_incompatible_producer() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let runtime = reader_runtime(root.path(), &layout);
        let publication = reader_publication();
        let storage = seed_reader_store(&root.path().join("core.sqlite3"), &publication);
        let producer_a_device = accelerated_device();
        let producer_a_identity = accelerated_identity();
        let manifest = reader_manifest(&crate::embeddings::embedding_runtime_id_for_runtime(
            &runtime,
        ));

        let published = publish_reader_generation(
            &layout,
            &storage,
            &manifest,
            &publication,
            &producer_a_device,
            &producer_a_identity,
            |_| {},
        );
        let producer_a_compatibility = vector_producer_compatibility_identity(
            &producer_a_device,
            Some(&producer_a_identity),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
        )
        .expect("producer A compatibility identity");
        assert_eq!(published.compatibility_sha256, producer_a_compatibility);

        let mut producer_b_device = producer_a_device.clone();
        producer_b_device.detected_provider = Some("cuda".into());
        producer_b_device.detected_gpu = Some("test cuda accelerator".into());
        producer_b_device.accelerator_request_provider = Some("cuda".into());
        producer_b_device.accelerator_request_device = Some("test cuda accelerator".into());
        let mut producer_b_identity = producer_a_identity.clone();
        producer_b_identity.backend = "CUDA".into();
        producer_b_identity.adapter_name = "test cuda accelerator".into();
        producer_b_identity.adapter_description = "test cuda".into();
        producer_b_identity.execution_device_names = vec!["test cuda accelerator".into()];
        let producer_b_compatibility = vector_producer_compatibility_identity(
            &producer_b_device,
            Some(&producer_b_identity),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
        )
        .expect("producer B compatibility identity");
        assert_ne!(producer_a_compatibility, producer_b_compatibility);

        let error = validate_generation_evidence_for_publication(
            &layout,
            &storage,
            &manifest,
            &publication,
            &runtime,
            &producer_b_device,
            Some(&producer_b_identity),
        )
        .expect_err("producer B must not admit producer A's persisted generation");
        let detail = format!("{error:#}");
        assert!(detail.contains("producer evidence is incompatible"));
        assert!(detail.contains("engine"));
    }

    #[test]
    fn reusable_vectors_require_exact_documents_and_compatible_producer_evidence() {
        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let runtime = reader_runtime(root.path(), &layout);
        let publication = reader_publication();
        let storage = seed_reader_store(&root.path().join("core.sqlite3"), &publication);
        let device = accelerated_device();
        let identity = accelerated_identity();
        let manifest = reader_manifest(&crate::embeddings::embedding_runtime_id_for_runtime(
            &runtime,
        ));
        let published = publish_reader_generation(
            &layout,
            &storage,
            &manifest,
            &publication,
            &device,
            &identity,
            |_| {},
        );
        let contract = VectorEvidenceContract::new(
            manifest.embedding_backend.clone().expect("backend"),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM,
            crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
            vector_compatibility_identity(&published.evidence).expect("compatibility identity"),
        );

        let reusable = EmbeddedVectorIndex::load_reusable_vectors(
            &layout,
            &manifest.semantic_generation,
            &published.evidence,
            &contract,
        )
        .expect("load compatible vectors");
        assert_eq!(
            reusable
                .get(&("1".to_string(), "reader-document-v1".to_string()))
                .expect("exact reusable document")[0],
            1.0
        );
        assert!(!reusable.contains_key(&("1".to_string(), "changed-document".to_string())));

        let mut incompatible = published.evidence.clone();
        incompatible.model.model_sha256 = "changed-model".into();
        assert!(
            EmbeddedVectorIndex::load_reusable_vectors(
                &layout,
                &manifest.semantic_generation,
                &incompatible,
                &contract,
            )
            .is_err()
        );
    }

    #[test]
    fn producer_compatibility_covers_every_evidence_group_and_execution_proof() {
        let device = accelerated_device();
        let identity = accelerated_identity();
        let publication = EmbeddingVectorPublicationIdentityDto {
            core_generation_id: "core-generation".into(),
            core_run_id: "core-run".into(),
            retrieval_generation: "retrieval-generation".into(),
            retrieval_input_hash: "retrieval-input".into(),
            semantic_generation: "semantic-generation".into(),
        };
        let expected = build_vector_producer_evidence(
            &device,
            Some(&identity),
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            publication,
        );
        assert_eq!(
            expected.semantics.dimension as usize,
            crate::embedding_contract::RETRIEVAL_EMBEDDING_DIM
        );
        assert_eq!(
            expected.semantics.pooling,
            crate::embedding_contract::EMBEDDING_POOLING
        );
        assert_eq!(
            expected.semantics.normalization,
            crate::embedding_contract::EMBEDDING_NORMALIZATION
        );
        assert_eq!(expected.engine.device_class, identity.adapter_description);
        assert_ne!(expected.engine.device_class, device.observed_state);
        let test_support_evidence = build_vector_producer_evidence(
            &device,
            None,
            crate::embeddings::RETRIEVAL_EMBEDDING_DIM as u32,
            expected.publication.clone(),
        );
        assert_eq!(test_support_evidence.engine.device_class, "metal");
        assert_ne!(
            test_support_evidence.engine.device_class,
            device.observed_state
        );

        assert_evidence_mismatch(&expected, "model", |evidence| {
            evidence.model.model_id.push_str("-changed");
        });
        let expected_compatibility =
            vector_compatibility_identity(&expected).expect("expected compatibility identity");
        let mut changed_producer_name = expected.clone();
        changed_producer_name.producer.name.push_str("-changed");
        assert_evidence_mismatch(&expected, "producer", |evidence| {
            evidence.producer.name.push_str("-changed");
        });
        assert_ne!(
            vector_compatibility_identity(&changed_producer_name)
                .expect("changed producer-name compatibility identity"),
            expected_compatibility,
            "producer implementation changes must invalidate persisted vector reuse"
        );
        let mut changed_producer_version = expected.clone();
        changed_producer_version
            .producer
            .version
            .push_str("-changed");
        assert_evidence_mismatch(&expected, "producer", |evidence| {
            evidence.producer.version.push_str("-changed");
        });
        assert_ne!(
            vector_compatibility_identity(&changed_producer_version)
                .expect("changed producer-version compatibility identity"),
            expected_compatibility,
            "producer version changes must invalidate persisted vector reuse"
        );
        assert_evidence_mismatch(&expected, "semantics", |evidence| {
            evidence.semantics.document_prefix.push_str("changed: ");
        });
        assert_evidence_mismatch(&expected, "engine", |evidence| {
            evidence.engine.engine_build_id.push_str("-changed");
        });
        assert_evidence_mismatch(&expected, "execution.eligibility", |evidence| {
            evidence.execution.eligibility = "cpu_explicit".into();
        });
        assert_evidence_mismatch(&expected, "execution.observed_state", |evidence| {
            evidence.execution.observed_state = "cpu_explicit".into();
        });
        assert_evidence_mismatch(&expected, "execution.observation_source", |evidence| {
            evidence.execution.observation_source = "metadata_only".into();
        });
        assert_evidence_mismatch(
            &expected,
            "execution.smoke_elapsed_ms_presence",
            |evidence| {
                evidence.execution.smoke_elapsed_ms = None;
            },
        );
        assert_evidence_mismatch(&expected, "publication", |evidence| {
            evidence
                .publication
                .retrieval_input_hash
                .push_str("-changed");
        });

        validate_execution_evidence_for_runtime(
            &expected,
            &reader_runtime(Path::new("."), &layout(Path::new("."))),
            &device,
            Some(&identity),
        )
        .expect("complete accelerator evidence");
        let mut incomplete = expected.clone();
        incomplete.execution.smoke_elapsed_ms = None;
        assert!(
            validate_execution_evidence_for_runtime(
                &incomplete,
                &reader_runtime(Path::new("."), &layout(Path::new("."))),
                &device,
                Some(&identity),
            )
            .expect_err("missing smoke proof must fail")
            .to_string()
            .contains("missing execution proof")
        );
        let mut partial_offload = identity;
        partial_offload.offloaded_layer_count -= 1;
        assert!(
            validate_execution_evidence_for_runtime(
                &expected,
                &reader_runtime(Path::new("."), &layout(Path::new("."))),
                &device,
                Some(&partial_offload),
            )
            .expect_err("partial accelerator execution must fail")
            .to_string()
            .contains("live embedding engine")
        );
    }

    #[test]
    #[ignore = "measurement lane; run with --release --ignored --nocapture"]
    fn embedded_vector_scan_measurement() {
        const DIMENSION: usize = 768;
        const SEARCH_RUNS: usize = 10;

        let root = tempdir().expect("tempdir");
        let layout = layout(root.path());
        let mut measurements = Vec::new();
        for point_count in [1_000_usize, 10_000, 25_000] {
            let collection = format!("codestory_measurement_{point_count}");
            let build_started = Instant::now();
            EmbeddedVectorIndex::build_with_points(
                &layout,
                &collection,
                "measurement-generation",
                "measurement-input",
                crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
                DIMENSION,
                |visit| {
                    for index in 0..point_count {
                        let mut vector = vec![0.0_f32; DIMENSION];
                        let first = index % DIMENSION;
                        let second = (index.wrapping_mul(31) + 7) % DIMENSION;
                        if first == second {
                            vector[first] = 1.0;
                        } else {
                            const NORMALIZER: f32 = 0.894_427_2;
                            vector[first] = NORMALIZER;
                            vector[second] = 0.5 * NORMALIZER;
                        }
                        visit(point(&index.to_string(), vector))?;
                    }
                    Ok(())
                },
            )
            .expect("build measurement index");
            let build_ms = build_started.elapsed().as_millis();

            let mut query = vec![0.0_f32; DIMENSION];
            query[0] = 1.0;
            query[7] = 0.5;
            let path = index_path(&layout, &collection);
            let mut search_us = Vec::with_capacity(SEARCH_RUNS);
            for _ in 0..SEARCH_RUNS {
                let started = Instant::now();
                let hits = search_database(
                    &path,
                    "measurement-generation",
                    "measurement-input",
                    &query,
                    20,
                    || false,
                )
                .expect("measure search");
                assert_eq!(hits.len(), 20);
                search_us.push(started.elapsed().as_micros());
            }
            search_us.sort_unstable();
            measurements.push(serde_json::json!({
                "points": point_count,
                "dimension": DIMENSION,
                "database_bytes": std::fs::metadata(&path).expect("index metadata").len(),
                "build_ms": build_ms,
                "warm_search_p50_us": search_us[SEARCH_RUNS / 2],
                "warm_search_p95_us": search_us[SEARCH_RUNS - 1],
            }));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&measurements).expect("serialize measurements")
        );
    }
}
