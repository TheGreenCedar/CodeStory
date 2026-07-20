use anyhow::{Context, Result, bail};
use codestory_contracts::api::EmbeddingVectorPublicationIdentityDto;
use codestory_store::{RetrievalIndexManifest, SourcePolicyExclusionPolicyIdentity, Store};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

pub fn env_lock() -> MutexGuard<'static, ()> {
    // Environment guards restore their variables while unwinding. Recover the
    // mutex after a failed assertion so one primary failure does not obscure
    // the rest of the suite with poison errors.
    ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn retrieval_manifest_fixture(
    project_id: &str,
    sidecar_input_hash: &str,
) -> RetrievalIndexManifest {
    RetrievalIndexManifest {
        project_id: project_id.into(),
        lexical_version: crate::lexical_index::LEXICAL_INDEX_VERSION.into(),
        semantic_generation: crate::generation::sidecar_vector_generation(
            project_id,
            sidecar_input_hash,
        ),
        scip_revision: Some("graph-test".into()),
        built_at_epoch_ms: chrono::Utc::now().timestamp_millis(),
        disk_bytes: None,
        degraded_modes_json: "[]".into(),
        embedding_backend: Some(crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID.into()),
        embedding_dim: Some(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32),
        sidecar_schema_version: Some(crate::generation::SIDECAR_SCHEMA_VERSION),
        sidecar_input_hash: Some(sidecar_input_hash.into()),
        sidecar_generation: Some(crate::generation::sidecar_generation_id(
            project_id,
            sidecar_input_hash,
        )),
        projection_count: Some(0),
        symbol_doc_count: Some(0),
        dense_projection_count: Some(0),
        semantic_policy_version: Some(crate::generation::SEMANTIC_POLICY_VERSION.into()),
        graph_artifact_hash: Some("graph-test-hash".into()),
        dense_reason_counts_json: Some("{}".into()),
        precise_semantic_import_status: None,
        precise_semantic_import_reason: None,
        precise_semantic_import_revision: None,
        precise_semantic_import_producer: None,
    }
}

/// Publish a strict, zero-dense query fixture with the same vector-generation evidence required
/// by product `PinnedQuerySession` reads.
pub fn publish_zero_dense_pinned_query_fixture(
    project_root: &Path,
    storage_path: &Path,
    runtime: &crate::SidecarRuntimeConfig,
) -> Result<RetrievalIndexManifest> {
    let project_id = crate::index::sidecar_project_id_for_runtime(project_root, runtime)?;
    let embedding_backend = crate::embeddings::embedding_runtime_id_for_runtime(runtime);
    let embedding_dim = i32::try_from(crate::embeddings::semantic_vector_dim())
        .unwrap_or(crate::embeddings::RETRIEVAL_EMBEDDING_DIM as i32);
    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(runtime);
    let producer_compatibility_identity =
        crate::embedded_vector::vector_producer_compatibility_identity(
            &embedding_device,
            None,
            u32::try_from(embedding_dim).context("negative embedding dimension")?,
        )?;
    let mut storage = Store::open(storage_path).context("open pinned query fixture storage")?;
    let publication = storage
        .get_complete_index_publication()
        .context("load pinned query fixture publication")?
        .context("pinned query fixture requires a complete core publication")?;
    storage
        .publish_dense_anchor_generation(&publication, crate::generation::SEMANTIC_POLICY_VERSION)
        .context("publish pinned query fixture dense-anchor manifest")?;
    let input = crate::index::compute_sidecar_input_fingerprint(
        &storage,
        project_root,
        storage_path,
        &project_id,
        &embedding_backend,
        embedding_dim,
        &producer_compatibility_identity,
    )?;
    if input.dense_projection_count != 0 {
        bail!("zero-dense pinned query fixture received dense anchors");
    }
    let mut manifest = retrieval_manifest_fixture(&project_id, &input.hash);
    manifest.built_at_epoch_ms = chrono::Utc::now().timestamp_millis();
    manifest.projection_count = Some(input.projection_count);
    manifest.symbol_doc_count = Some(input.symbol_doc_count);
    manifest.dense_projection_count = Some(input.dense_projection_count);
    manifest.semantic_policy_version = input.semantic_policy_version;
    manifest.graph_artifact_hash = Some(input.graph_artifact_hash);
    manifest.dense_reason_counts_json = Some(input.dense_reason_counts_json);
    let generation = manifest
        .sidecar_generation
        .as_deref()
        .context("fixture sidecar generation")?;
    crate::lexical_index::build_lexical_shard(
        project_root,
        Some(storage_path),
        &runtime.layout.lexical_data_dir,
        generation,
        &crate::lexical_index::LexicalInputFingerprint {
            file_count: input.lexical_file_count,
            hash: input.lexical_hash.clone(),
            coverage: input.lexical_coverage.clone(),
        },
        &input.hash,
    )
    .context("publish pinned query fixture lexical shard")?;
    let revision = format!("fixture-{generation}");
    let scip_dir = runtime.layout.scip_project_dir(generation);
    std::fs::create_dir_all(&scip_dir).context("create pinned query fixture SCIP directory")?;
    let symbol = crate::scip_index::ScipSymbolRecord {
        node_id: None,
        path: "fixture.rs".into(),
        symbol: "fixture::symbol".into(),
        start_line: 1,
        end_line: 1,
    };
    let index = crate::scip_index::ScipSymbolsIndex {
        revision: revision.clone(),
        contract: crate::scip_index::ScipProofAdapterContract::graph_projection(&revision),
        symbols: vec![symbol],
        proofs: Vec::new(),
    };
    std::fs::write(
        scip_dir.join(crate::scip_index::SCIP_SYMBOLS_FILE),
        serde_json::to_vec_pretty(&index).context("serialize pinned query fixture SCIP index")?,
    )
    .context("write pinned query fixture SCIP index")?;
    std::fs::write(
        scip_dir.join(crate::scip_index::SCIP_INDEX_FILE),
        format!("codestory-scip-v1\nrevision={revision}\n"),
    )
    .context("write pinned query fixture SCIP marker")?;
    std::fs::write(scip_dir.join("revision.txt"), format!("{revision}\n"))
        .context("write pinned query fixture SCIP revision")?;
    manifest.scip_revision = Some(revision);
    storage
        .upsert_retrieval_index_manifest(&manifest)
        .context("publish pinned query fixture manifest")?;
    drop(storage);

    publish_zero_dense_vector_evidence(runtime, &manifest, &publication)?;
    Ok(manifest)
}

/// Publish a replacement core identity and its strict zero-dense retrieval
/// fixture in the live SQLite database. Public-operation race tests use this to
/// force a new connection to observe generation B while an older reader keeps
/// generation A pinned in its transaction.
pub fn publish_replacement_core_and_zero_dense_fixture(
    project_root: &Path,
    storage_path: &Path,
    runtime: &crate::SidecarRuntimeConfig,
    generation: u64,
    generation_id: &str,
    run_id: &str,
) -> Result<RetrievalIndexManifest> {
    let mut storage = Store::open(storage_path).context("open replacement core fixture storage")?;
    let source_policy = storage
        .get_source_policy_exclusion_manifest()
        .context("load replacement core source policy manifest")?
        .context("replacement core fixture requires a source policy manifest")?;
    let source_policy_candidates = storage
        .get_source_policy_exclusions()
        .context("load replacement core source policy exclusions")?
        .into_iter()
        .map(
            |record| codestory_workspace::OversizedSourceExclusionCandidate {
                normalized_path: record.normalized_path,
                content_hash: record.content_hash,
                observed_size: record.observed_size,
                observed_unit_count: record.observed_unit_count,
                policy_version: record.policy_version,
                byte_cap: record.byte_cap,
                structural_unit_cap: record.structural_unit_cap,
            },
        )
        .collect::<Vec<_>>();
    let publication = codestory_store::IndexPublicationRecord {
        generation,
        generation_id: generation_id.into(),
        run_id: run_id.into(),
        mode: codestory_store::IndexPublicationMode::Full,
        published_at_epoch_ms: 2,
    };
    storage
        .publish_structural_text_unit_generation(&publication)
        .context("publish replacement core structural text unit manifest")?;
    storage
        .publish_source_policy_exclusion_generation(
            &publication,
            &source_policy.project_id,
            &source_policy.workspace_id,
            SourcePolicyExclusionPolicyIdentity::new(
                &source_policy.policy_version,
                source_policy.byte_cap,
                source_policy.structural_unit_cap,
            ),
            &source_policy_candidates,
        )
        .context("publish replacement core source policy manifest")?;
    storage
        .put_index_publication(&publication)
        .context("publish replacement core fixture identity")?;
    drop(storage);
    publish_zero_dense_pinned_query_fixture(project_root, storage_path, runtime)
}

fn publish_zero_dense_vector_evidence(
    runtime: &crate::SidecarRuntimeConfig,
    manifest: &RetrievalIndexManifest,
    publication: &codestory_store::IndexPublicationRecord,
) -> Result<()> {
    let retrieval_generation = manifest
        .sidecar_generation
        .as_deref()
        .context("fixture sidecar generation")?;
    let retrieval_input_hash = manifest
        .sidecar_input_hash
        .as_deref()
        .context("fixture sidecar input hash")?;
    let embedding_backend = manifest
        .embedding_backend
        .as_deref()
        .context("fixture embedding backend")?;
    let embedding_dim = usize::try_from(
        manifest
            .embedding_dim
            .context("fixture embedding dimension")?,
    )
    .context("negative fixture embedding dimension")?;
    let embedding_device = crate::embeddings::embedding_device_readiness_for_runtime(runtime);
    let evidence = crate::embedded_vector::build_vector_producer_evidence(
        &embedding_device,
        None,
        u32::try_from(embedding_dim).context("fixture vector dimension overflow")?,
        EmbeddingVectorPublicationIdentityDto {
            core_generation_id: publication.generation_id.clone(),
            core_run_id: publication.run_id.clone(),
            retrieval_generation: retrieval_generation.to_string(),
            retrieval_input_hash: retrieval_input_hash.to_string(),
            semantic_generation: manifest.semantic_generation.clone(),
        },
    );
    let compatibility = crate::embedded_vector::vector_compatibility_identity(&evidence)?;
    let contract = crate::embedded_vector::VectorEvidenceContract::new(
        embedding_backend,
        embedding_dim,
        crate::embeddings::PRODUCT_EMBEDDING_RUNTIME_ID,
        compatibility,
    );
    let attestation = crate::embedded_vector::EmbeddedVectorIndex::build_attested_with_points(
        &runtime.layout,
        &manifest.semantic_generation,
        retrieval_generation,
        retrieval_input_hash,
        &contract,
        &[],
        |_visit| Ok(()),
    )?;
    let generation = crate::embedded_vector::VectorGenerationManifest::new(evidence, attestation)?;
    crate::embedded_vector::EmbeddedVectorIndex::publish_generation_manifest(
        &runtime.layout,
        &manifest.semantic_generation,
        &generation,
    )
}
