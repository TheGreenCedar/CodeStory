use super::{
    ApiError, CancellationToken, EmbeddingProfileContractDto, FileExt,
    HYBRID_RETRIEVAL_ENABLED_ENV, HashMap, IndexPublicationRecord, Instant, OwnedDeletionRoot,
    Path, PathBuf, RetrievalFallbackReasonDto, RetrievalModeDto, RetrievalStateDto, SearchEngine,
    SearchSymbolProjection, SemanticModeDto, Storage, Store, StoredSemanticDocsContractDto,
    UNIX_EPOCH, Uuid, clamp_u128_to_u32, clamp_usize_to_u32,
    embedding_runtime_availability_from_config, indexing_cancelled_error, is_indexing_cancelled,
    open_storage_for_read,
};
#[cfg(test)]
use super::{
    embedding_runtime_availability_from_env, hybrid_retrieval_enabled,
    test_sidecar_runtime_from_env,
};
#[cfg(test)]
use crate::semantic_projection::{
    LLM_DOC_EMBED_BATCH_SIZE, LLM_DOC_EMBED_BATCH_SIZE_ENV, current_embedding_contract_from_env,
};
use crate::semantic_projection::{
    SEARCH_SYMBOL_STREAM_BATCH_SIZE, SearchStateBuildStats, current_embedding_contract_for_runtime,
    load_persisted_semantic_docs_for_runtime,
};
use serde::{Deserialize, Serialize};

pub(super) fn search_index_storage_path(storage_path: &Path) -> PathBuf {
    codestory_workspace::legacy_search_directory_for_storage(storage_path)
}

pub(super) fn search_index_generation_root(storage_path: &Path) -> PathBuf {
    codestory_workspace::search_generation_directory_for_storage(storage_path)
}

pub(super) fn search_index_path_for_publication(
    storage_path: &Path,
    publication: Option<&IndexPublicationRecord>,
) -> Result<PathBuf, ApiError> {
    match publication {
        Some(publication) => Uuid::parse_str(&publication.generation_id)
            .map(|generation_id| {
                search_index_generation_root(storage_path).join(generation_id.to_string())
            })
            .map_err(|error| {
                ApiError::internal(format!(
                    "Invalid index publication generation id {}: {error}",
                    publication.generation_id
                ))
            }),
        None => Ok(search_index_storage_path(storage_path)),
    }
}

pub(super) const SEARCH_GENERATION_COMPLETION_SCHEMA_VERSION: u32 = 1;
pub(super) const SEARCH_GENERATION_COMPLETION_FILE: &str = ".codestory-complete.json";
pub(super) const SEARCH_GENERATION_COMPLETION_MAX_BYTES: u64 = 4 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SearchGenerationCompletion {
    pub(super) schema_version: u32,
    pub(super) generation_id: String,
    pub(super) symbol_count: u64,
    pub(super) tantivy_doc_count: u64,
}

pub(super) fn search_generation_completion_path(search_path: &Path) -> PathBuf {
    search_path.join(SEARCH_GENERATION_COMPLETION_FILE)
}

pub(super) fn read_search_generation_completion(
    search_path: &Path,
    expected_generation_id: &str,
) -> Option<SearchGenerationCompletion> {
    let marker_path = search_generation_completion_path(search_path);
    let metadata = std::fs::metadata(&marker_path).ok()?;
    if !metadata.is_file() || metadata.len() > SEARCH_GENERATION_COMPLETION_MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(&marker_path).ok()?;
    let marker = serde_json::from_slice::<SearchGenerationCompletion>(&bytes).ok()?;
    (marker.schema_version == SEARCH_GENERATION_COMPLETION_SCHEMA_VERSION
        && marker.generation_id == expected_generation_id)
        .then_some(marker)
}

pub(super) fn write_search_generation_completion(
    search_path: &Path,
    publication: &IndexPublicationRecord,
    symbol_count: usize,
    tantivy_doc_count: usize,
) -> Result<(), ApiError> {
    let generation_id = Uuid::parse_str(&publication.generation_id)
        .map_err(|error| {
            ApiError::internal(format!(
                "Invalid index publication generation id {}: {error}",
                publication.generation_id
            ))
        })?
        .to_string();
    let marker = SearchGenerationCompletion {
        schema_version: SEARCH_GENERATION_COMPLETION_SCHEMA_VERSION,
        generation_id,
        symbol_count: symbol_count as u64,
        tantivy_doc_count: tantivy_doc_count as u64,
    };
    let bytes = serde_json::to_vec(&marker).map_err(|error| {
        ApiError::internal(format!(
            "Failed to encode persisted search generation completion marker: {error}"
        ))
    })?;
    let marker_path = search_generation_completion_path(search_path);
    let temp_path = search_path.join(format!(".codestory-complete.{}.tmp", Uuid::new_v4()));
    let write_result = (|| -> Result<(), ApiError> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to create persisted search completion temp file {}: {error}",
                    temp_path.display()
                ))
            })?;
        std::io::Write::write_all(&mut file, &bytes).map_err(|error| {
            ApiError::internal(format!(
                "Failed to write persisted search completion temp file {}: {error}",
                temp_path.display()
            ))
        })?;
        file.sync_all().map_err(|error| {
            ApiError::internal(format!(
                "Failed to sync persisted search completion temp file {}: {error}",
                temp_path.display()
            ))
        })?;
        std::fs::rename(&temp_path, &marker_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to publish persisted search completion marker {}: {error}",
                marker_path.display()
            ))
        })?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result
}

pub(super) struct SearchGenerationCatalogGuard {
    file: std::fs::File,
    path: PathBuf,
}

impl SearchGenerationCatalogGuard {
    pub(super) fn acquire(storage_path: &Path) -> Result<Self, ApiError> {
        let mut path = search_index_generation_root(storage_path).into_os_string();
        path.push(".lock");
        let path = PathBuf::from(path);
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to create search generation catalog lock directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to open search generation catalog lock {}: {error}",
                    path.display()
                ))
            })?;
        FileExt::lock_exclusive(&file).map_err(|error| {
            ApiError::internal(format!(
                "Failed to acquire search generation catalog lock {}: {error}",
                path.display()
            ))
        })?;
        Ok(Self { file, path })
    }
}

impl Drop for SearchGenerationCatalogGuard {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(
                path = %self.path.display(),
                "Failed to unlock search generation catalog: {error}"
            );
        }
    }
}

pub(super) fn inspect_search_generation(path: &Path) -> Result<Option<bool>, ApiError> {
    let lock_path = crate::search::engine::persisted_search_index_lock_path(path);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to open persisted search generation lock {}: {error}",
                lock_path.display()
            ))
        })?;
    if !FileExt::try_lock_shared(&lock).map_err(|error| {
        ApiError::internal(format!(
            "Failed to inspect persisted search generation lock {}: {error}",
            lock_path.display()
        ))
    })? {
        return Ok(None);
    }
    let generation_id = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let marker = read_search_generation_completion(path, generation_id);
    let valid = marker.is_some_and(|marker| {
        SearchEngine::open_existing(path)
            .is_ok_and(|engine| engine.tantivy_doc_count() as u64 == marker.tantivy_doc_count)
    });
    let _ = FileExt::unlock(&lock);
    Ok(Some(valid))
}

pub(super) fn try_remove_search_generation(
    deletion: &OwnedDeletionRoot,
    relative: &Path,
    path: &Path,
) -> Result<bool, ApiError> {
    let lock_path = crate::search::engine::persisted_search_index_lock_path(path);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to open persisted search generation lock {}: {error}",
                lock_path.display()
            ))
        })?;
    if !FileExt::try_lock_exclusive(&lock).map_err(|error| {
        ApiError::internal(format!(
            "Failed to lock persisted search generation {} for removal: {error}",
            path.display()
        ))
    })? {
        return Ok(false);
    }
    let removal = deletion.remove(relative);
    let _ = FileExt::unlock(&lock);
    let removed = removal.map_err(|error| {
        ApiError::internal(format!(
            "Failed to remove persisted search generation {}: {error}",
            path.display()
        ))
    })?;
    Ok(removed)
}

pub(super) fn prune_search_generations(
    storage_path: &Path,
    active_generation_id: &str,
) -> Result<(), ApiError> {
    let root = search_index_generation_root(storage_path);
    if !root.is_dir() {
        return Ok(());
    }
    let parent = root.parent().unwrap_or_else(|| Path::new("."));
    let deletion = OwnedDeletionRoot::open(parent).map_err(|error| {
        ApiError::internal(format!(
            "Failed to open persisted search generation deletion root {}: {error}",
            parent.display()
        ))
    })?;
    let relative_root = root.file_name().ok_or_else(|| {
        ApiError::internal(format!(
            "Persisted search generation root has no owned relative name: {}",
            root.display()
        ))
    })?;
    let mut generations = std::fs::read_dir(&root)
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to list persisted search generations {}: {error}",
                root.display()
            ))
        })?
        .filter_map(Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".lock"))
        .collect::<Vec<_>>();
    generations.sort_by_key(|entry| {
        std::cmp::Reverse(
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH),
        )
    });

    // During staged publication the prepared search identity is newer than
    // the still-live core. Keep the search generation bound to that live core
    // as the rollback; a concurrent prepared generation must not consume the
    // sole rollback slot merely because its completion marker was written.
    let pinned_rollback_generation_id = Store::database_complete_index_publication(storage_path)
        .ok()
        .flatten()
        .map(|publication| publication.generation_id)
        .filter(|generation_id| generation_id != active_generation_id);

    let mut rollback_retained = false;
    for entry in generations {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == active_generation_id {
            continue;
        }
        let well_formed = Uuid::parse_str(&name).is_ok();
        let inspection = if well_formed {
            inspect_search_generation(&path)?
        } else {
            Some(false)
        };
        match inspection {
            Some(true)
                if !rollback_retained
                    && pinned_rollback_generation_id
                        .as_ref()
                        .is_none_or(|generation_id| generation_id == &name) =>
            {
                rollback_retained = true
            }
            Some(_) => {
                let relative = Path::new(relative_root).join(&name);
                let _ = try_remove_search_generation(&deletion, &relative, &path)?;
            }
            None => {
                tracing::debug!(
                    path = %path.display(),
                    "Skipping locked persisted search generation during retention"
                );
            }
        }
    }
    Ok(())
}

pub(super) fn discard_unpublished_search_generation(
    storage_path: &Path,
    publication: &IndexPublicationRecord,
) {
    if matches!(
        Store::database_index_publication(storage_path),
        Ok(Some(live)) if live == *publication
    ) {
        return;
    }
    if let Ok(path) = search_index_path_for_publication(storage_path, Some(publication)) {
        let root = search_index_generation_root(storage_path);
        let parent = root.parent().unwrap_or_else(|| Path::new("."));
        if let (Ok(deletion), Some(relative_root), Some(generation_name)) = (
            OwnedDeletionRoot::open(parent),
            root.file_name(),
            path.file_name(),
        ) {
            let relative = Path::new(relative_root).join(generation_name);
            let _ = try_remove_search_generation(&deletion, &relative, &path);
        }
    }
}

pub(super) fn load_canonical_search_symbols(
    storage: &Storage,
    batch_size: usize,
    cancel_token: Option<&CancellationToken>,
    mut consume_batch: impl FnMut(Vec<SearchSymbolProjection>) -> Result<(), ApiError>,
) -> Result<
    (
        HashMap<codestory_contracts::graph::NodeId, String>,
        SearchStateBuildStats,
    ),
    ApiError,
> {
    let count_started = Instant::now();
    let expected_rows = storage
        .get_canonical_search_symbol_count()
        .map_err(|error| {
            ApiError::internal(format!("Failed to count canonical search symbols: {error}"))
        })?;
    let mut node_names = HashMap::with_capacity(expected_rows as usize);
    let mut after_node_id = None;
    let batch_size = batch_size.max(1);
    let mut stream_duration = count_started.elapsed();
    let mut stream_rows = 0_usize;
    let mut stream_batches = 0_usize;
    loop {
        let batch_started = Instant::now();
        let batch = storage
            .get_canonical_search_symbol_batch_after(after_node_id, batch_size)
            .map_err(|e| {
                ApiError::internal(format!("Failed to stream canonical search symbols: {e}"))
            })?;
        stream_duration = stream_duration.saturating_add(batch_started.elapsed());
        if batch.is_empty() {
            break;
        }
        after_node_id = batch.last().map(|entry| entry.node_id);
        stream_rows = stream_rows.saturating_add(batch.len());
        stream_batches = stream_batches.saturating_add(1);
        for entry in &batch {
            node_names.insert(entry.node_id, entry.display_name.clone());
        }
        consume_batch(batch)?;
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
    }
    if stream_rows != expected_rows as usize {
        return Err(ApiError::internal(format!(
            "Canonical search symbol stream count changed: expected {expected_rows}, loaded {stream_rows}"
        )));
    }
    Ok((
        node_names,
        SearchStateBuildStats {
            search_projection_rebuild_ms: 0,
            search_symbol_stream_ms: clamp_u128_to_u32(stream_duration.as_millis()),
            search_symbol_stream_rows: clamp_usize_to_u32(stream_rows),
            search_symbol_stream_batches: clamp_usize_to_u32(stream_batches),
            ..SearchStateBuildStats::default()
        },
    ))
}

pub(super) struct LoadedSearchState {
    pub(super) publication: Option<IndexPublicationRecord>,
    pub(super) node_names: HashMap<codestory_contracts::graph::NodeId, String>,
    pub(super) engine: SearchEngine,
}

#[cfg(test)]
pub(super) fn load_persisted_search_state(
    storage: &mut Storage,
    storage_path: &Path,
) -> Result<LoadedSearchState, ApiError> {
    load_persisted_search_state_for_runtime(storage, storage_path, &test_sidecar_runtime_from_env())
}

pub(super) fn load_persisted_search_state_for_runtime(
    storage: &mut Storage,
    storage_path: &Path,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<LoadedSearchState, ApiError> {
    let _catalog_guard = SearchGenerationCatalogGuard::acquire(storage_path)?;
    *storage = open_storage_for_read(storage_path)?;
    let publication = storage.get_complete_index_publication().map_err(|error| {
        ApiError::internal(format!(
            "Failed to read complete search publication identity: {error}"
        ))
    })?;
    if publication.is_none() {
        let mut engine = SearchEngine::new(None).map_err(|error| {
            ApiError::internal(format!("Failed to init search engine: {error}"))
        })?;
        let mut symbol_session = engine.begin_symbol_index().map_err(|error| {
            ApiError::internal(format!(
                "Failed to start legacy symbol index writer: {error}"
            ))
        })?;
        let (node_names, _) = load_canonical_search_symbols(storage, 10_000, None, |batch| {
            symbol_session
                .add_nodes(
                    batch
                        .into_iter()
                        .map(|entry| (entry.node_id, entry.display_name)),
                )
                .map(|_| ())
                .map_err(|error| {
                    ApiError::internal(format!("Failed to index legacy search nodes: {error}"))
                })
        })?;
        symbol_session.finish().map_err(|error| {
            ApiError::internal(format!("Failed to finish legacy symbol index: {error}"))
        })?;
        load_persisted_semantic_docs_for_runtime(storage, &mut engine, false, runtime)?;
        return Ok(LoadedSearchState {
            publication: None,
            node_names,
            engine,
        });
    }
    let search_storage_path =
        search_index_path_for_publication(storage_path, publication.as_ref())?;
    let completion = publication.as_ref().and_then(|publication| {
        let generation_id = Uuid::parse_str(&publication.generation_id)
            .ok()?
            .to_string();
        read_search_generation_completion(&search_storage_path, &generation_id)
    });
    if publication.is_some() && completion.is_none() {
        return Err(ApiError::new(
            "cache_busy",
            "The complete core publication does not yet have a completed search generation.",
        ));
    }
    let mut engine =
        SearchEngine::open_existing(search_storage_path.as_path()).map_err(|error| {
            ApiError::new(
                "cache_busy",
                format!(
                    "Failed to open completed search generation {}: {error}",
                    search_storage_path.display()
                ),
            )
        })?;
    engine.load_symbol_projection(std::iter::empty());
    let (node_names, stream_stats) =
        load_canonical_search_symbols(storage, SEARCH_SYMBOL_STREAM_BATCH_SIZE, None, |batch| {
            engine.extend_symbol_projection(
                batch
                    .into_iter()
                    .map(|entry| (entry.node_id, entry.display_name)),
            );
            Ok(())
        })?;
    let completion_count_mismatch = completion.as_ref().is_some_and(|marker| {
        marker.symbol_count != stream_stats.search_symbol_stream_rows as u64
            || marker.tantivy_doc_count != engine.tantivy_doc_count() as u64
    });
    if engine.full_text_doc_count() != stream_stats.search_symbol_stream_rows as usize
        || completion_count_mismatch
    {
        return Err(ApiError::new(
            "cache_busy",
            format!(
                "Completed search generation {} does not match its core symbols: streamed={}, searchable={}, marker_symbols={}, stored_docs={}, marker_docs={}.",
                search_storage_path.display(),
                stream_stats.search_symbol_stream_rows,
                engine.full_text_doc_count(),
                completion.as_ref().map_or(0, |marker| marker.symbol_count),
                engine.tantivy_doc_count(),
                completion
                    .as_ref()
                    .map_or(0, |marker| marker.tantivy_doc_count),
            ),
        ));
    }
    if publication.is_some() {
        engine
            .downgrade_persisted_lock_to_shared()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to share completed search generation {}: {error}",
                    search_storage_path.display()
                ))
            })?;
    }
    let live_publication =
        Store::database_complete_index_publication(storage_path).map_err(|error| {
            ApiError::internal(format!(
                "Failed to revalidate live publication after loading persisted search: {error}"
            ))
        })?;
    if live_publication != publication {
        return Err(ApiError::new(
            "cache_busy",
            "Core publication changed while persisted search state was loading. Retry against the new generation.",
        ));
    }
    Ok(LoadedSearchState {
        publication,
        node_names,
        engine,
    })
}

#[cfg(test)]
pub(super) fn llm_doc_embed_batch_size() -> usize {
    std::env::var(LLM_DOC_EMBED_BATCH_SIZE_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1, 2_048))
        .unwrap_or(LLM_DOC_EMBED_BATCH_SIZE)
}

#[cfg(test)]
pub(super) fn retrieval_state_from_parts(
    semantic_doc_count: u32,
    embedding_model: Option<String>,
    embedding_runtime_available: bool,
    fallback_message: Option<String>,
    current_embedding: Option<EmbeddingProfileContractDto>,
    stored_embedding: Option<StoredSemanticDocsContractDto>,
    runtime_degraded: bool,
) -> RetrievalStateDto {
    retrieval_state_from_parts_with_hybrid(
        semantic_doc_count,
        embedding_model,
        embedding_runtime_available,
        fallback_message,
        current_embedding,
        stored_embedding,
        runtime_degraded,
        false,
        hybrid_retrieval_enabled(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn retrieval_state_from_parts_with_hybrid(
    semantic_doc_count: u32,
    embedding_model: Option<String>,
    embedding_runtime_available: bool,
    fallback_message: Option<String>,
    current_embedding: Option<EmbeddingProfileContractDto>,
    stored_embedding: Option<StoredSemanticDocsContractDto>,
    runtime_degraded: bool,
    zero_dense_published: bool,
    hybrid_configured: bool,
) -> RetrievalStateDto {
    // `zero_dense_published` distinguishes a current, contract-matched full
    // publication that truthfully selected zero dense anchors from a semantic
    // lane that was never built. Only the latter is repairable by
    // `retrieval index --refresh full`.
    let semantic_lane_unbuilt = semantic_doc_count == 0 && !zero_dense_published;
    let fallback_reason = if !hybrid_configured {
        Some(RetrievalFallbackReasonDto::DisabledByConfig)
    } else if runtime_degraded {
        Some(RetrievalFallbackReasonDto::DegradedRuntime)
    } else if !embedding_runtime_available {
        Some(RetrievalFallbackReasonDto::MissingEmbeddingRuntime)
    } else if semantic_lane_unbuilt {
        Some(RetrievalFallbackReasonDto::MissingSemanticDocs)
    } else {
        None
    };
    let semantic_mode = if !hybrid_configured {
        SemanticModeDto::DisabledByConfig
    } else if runtime_degraded || !embedding_runtime_available || semantic_lane_unbuilt {
        SemanticModeDto::DegradedRuntime
    } else {
        SemanticModeDto::Enabled
    };
    let semantic_ready = semantic_mode == SemanticModeDto::Enabled;
    let mode = if semantic_ready {
        RetrievalModeDto::Hybrid
    } else {
        RetrievalModeDto::Symbolic
    };
    let fallback_message = fallback_message.or_else(|| match fallback_reason {
        Some(RetrievalFallbackReasonDto::DisabledByConfig) => Some(format!(
            "Hybrid retrieval disabled by {HYBRID_RETRIEVAL_ENABLED_ENV}=false; agent-facing retrieval is not full."
        )),
        Some(RetrievalFallbackReasonDto::MissingSemanticDocs) => Some(
            "Semantic assets are available, but semantic symbol docs have not been built yet. Run `retrieval index --refresh full` to repair full sidecar readiness."
                .to_string(),
        ),
        Some(RetrievalFallbackReasonDto::DegradedRuntime) => Some(
            "Hybrid retrieval is configured but degraded at runtime; agent-facing retrieval is not full."
                .to_string(),
        ),
        _ => None,
    });

    RetrievalStateDto {
        mode,
        hybrid_configured,
        semantic_ready,
        semantic_mode,
        semantic_doc_count,
        embedding_model,
        current_embedding,
        stored_embedding,
        fallback_reason,
        fallback_message,
    }
}

#[cfg(test)]
pub(super) fn retrieval_state_from_engine(engine: &SearchEngine) -> RetrievalStateDto {
    let probe = embedding_runtime_availability_from_env();
    let current_embedding = current_embedding_contract_from_env();
    retrieval_state_from_parts(
        engine.semantic_doc_count(),
        engine
            .embedding_model_id()
            .map(str::to_string)
            .or_else(|| {
                current_embedding
                    .as_ref()
                    .map(|contract| contract.cache_key.clone())
            })
            .or(probe.model_id),
        engine.embedding_runtime_configured(),
        if engine.embedding_runtime_configured() {
            None
        } else {
            probe.fallback_message
        },
        current_embedding,
        None,
        false,
    )
}

#[cfg(test)]
pub(super) fn retrieval_state_from_engine_with_storage_contract(
    engine: &SearchEngine,
    storage_retrieval: &RetrievalStateDto,
) -> RetrievalStateDto {
    let mut retrieval = retrieval_state_from_engine(engine);
    retrieval.stored_embedding = storage_retrieval.stored_embedding.clone();
    retrieval
}

#[cfg(test)]
pub(super) fn retrieval_state_from_storage(
    storage: &Storage,
    project_root: &Path,
) -> Result<RetrievalStateDto, ApiError> {
    retrieval_state_from_storage_for_runtime(
        storage,
        project_root,
        &test_sidecar_runtime_from_env(),
    )
}

/// Derive agent-visible retrieval readiness from the published retrieval manifest.
///
/// Semantic readiness must come from the same publication pointer that admits
/// sidecar-primary retrieval: the `retrieval_index_manifest` row for this
/// project. The legacy `llm_symbol_doc` table is intentionally cleared on
/// every semantic publication, so counting it reported `missing_semantic_docs`
/// forever on stores whose sidecar vectors were fully published — including
/// every fresh auto-bootstrap.
///
/// Manifest identity: resolving the manifest key goes through
/// `sidecar_project_id_for_root`, which re-observes project identity with
/// three git subprocesses per call (`config --get remote.origin.url`,
/// `rev-parse HEAD^{tree}`, and a workload-dependent `status --porcelain`)
/// before the manifest-row lookup, the pure contract checks, and the
/// storage-derived staleness scan. That is deliberately the same uncached
/// helper per-search sidecar admission uses
/// (`retrieval_primary::retrieval_manifest_exists`), so this projection and
/// admission can never disagree about *which manifest row is current*. Do not
/// substitute a cached identity here without proving admission reads the same
/// cache.
///
/// Agreement with admission is one-directional, not equality. Freshness comes
/// from `storage_admission_refusal_reason_for_runtime`, the subset of sidecar
/// admission derivable from `&Storage` alone: the incomplete-incremental-run
/// marker plus the manifest staleness scan. `SidecarQuery::begin` runs a
/// second gate, `validate_strict_sidecar_readiness_for_runtime`, which is
/// `pub(crate)` in `codestory-retrieval` and structurally unreachable from
/// here because it also needs the storage path, the project root's workspace
/// manifest, and a producer compatibility identity. So this projection can
/// still report hybrid for a publication that gate refuses — on
/// `indexable_file_added_or_changed_after_retrieval_manifest`,
/// `indexed_file_removed_after_retrieval_manifest`,
/// `indexed_file_error_retry_required`, `sidecar_input_hash_changed`, or the
/// two symbol-doc backend reasons. What is guaranteed is the direction that
/// matters for not over-claiming: everything consulted here also makes
/// admission refuse, so readiness never promises hybrid over a refusal this
/// projection can see. `retrieval status` runs both gates and is the surface
/// to trust for exact parity. Closing the remaining gap means making the
/// strict gate reachable with a `&Storage`-only signature, not adding another
/// private re-derivation here.
///
/// Cost: the staleness scan is two aggregate counts (symbol docs and the
/// grouped `dense_anchor_input` projection), not a row sweep — see
/// `Storage::dense_anchor_input_stats`. This runs on observational callers
/// (project open, `retrieval_state`, grounding snapshots), so it must stay
/// aggregate-only; paging `DenseAnchorInput`s here would put every anchor's
/// `document_text` on a status call. The read stays observational: no
/// probing, repair, or refresh.
pub(super) fn retrieval_state_from_storage_for_runtime(
    storage: &Storage,
    project_root: &Path,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<RetrievalStateDto, ApiError> {
    let project_id = codestory_retrieval::sidecar_project_id_for_root(project_root);
    let manifest = storage
        .get_retrieval_index_manifest(&project_id)
        .map_err(|e| {
            ApiError::internal(format!("Failed to query retrieval index manifest: {e}"))
        })?;
    let probe = embedding_runtime_availability_from_config(runtime);
    let current_embedding = current_embedding_contract_for_runtime(runtime);
    let stored_embedding = manifest
        .as_ref()
        .map(stored_semantic_docs_contract_from_manifest);
    let semantic_doc_count = manifest
        .as_ref()
        .map(published_dense_projection_count)
        .unwrap_or(0);
    // Fail closed: published vectors count as semantic readiness only while the
    // manifest still classifies as a current, non-degraded full publication
    // *and* the store the sidecar would be served from still agrees with it.
    // Manifest shape alone is not enough: a core-only refresh leaves the
    // manifest untouched while moving the symbol docs, dense anchors, and
    // indexed-file mtimes underneath it, and an interrupted incremental run
    // leaves all three untouched while still making admission refuse. Both are
    // storage-derived, so both are read here through the one helper admission
    // shares (`storage_admission_refusal_reason_for_runtime`) — see this
    // function's doc comment for the strict gate this still cannot see.
    let stale_publication = manifest.as_ref().is_some_and(|manifest| {
        !codestory_retrieval::manifest_classifies_full(manifest)
            || codestory_retrieval::storage_admission_refusal_reason_for_runtime(
                &project_id,
                storage,
                manifest,
                runtime,
            )
            .is_some()
    });
    let contract_mismatch = manifest
        .as_ref()
        .is_some_and(|manifest| !manifest_matches_current_embedding_contract(manifest, runtime));
    // A full publication may legitimately select zero dense anchors (generation
    // only requires the dense count to equal the projection count), so a
    // zero-anchor manifest still describes a *built* semantic lane. It must
    // therefore be able to degrade like any other publication: gating
    // `runtime_degraded` on `semantic_doc_count > 0` alone made a stale
    // zero-anchor sidecar fall through to `missing_semantic_docs` — "semantic
    // symbol docs have not been built yet" for a lane that was built and is
    // merely stale, and doctor's "stale or degraded" gap never fired for it.
    let zero_dense_manifest = manifest
        .as_ref()
        .is_some_and(|manifest| manifest.dense_projection_count == Some(0));
    let runtime_degraded = (semantic_doc_count > 0 || zero_dense_manifest)
        && probe.available
        && (stale_publication || contract_mismatch);
    // A *current, contract-matched* zero-anchor publication is the other half:
    // admission serves it as full, so the lane is published-and-empty rather
    // than unbuilt. Reporting `missing_semantic_docs` there would prescribe a
    // refresh that republishes the identical zero-anchor manifest and can
    // never clear the message.
    let zero_dense_published = zero_dense_manifest && !stale_publication && !contract_mismatch;
    let fallback_message = probe.fallback_message.or_else(|| {
        if !runtime_degraded {
            None
        } else if contract_mismatch {
            Some(
                "Stored semantic docs do not match the current embedding contract. Run `retrieval index --refresh full` before trusting hybrid retrieval."
                    .to_string(),
            )
        } else {
            Some(
                "The published retrieval sidecar is stale or degraded for the current contract. Run `retrieval index --refresh full` to repair full sidecar readiness."
                    .to_string(),
            )
        }
    });
    Ok(retrieval_state_from_parts_with_hybrid(
        semantic_doc_count,
        manifest
            .as_ref()
            .and_then(|manifest| manifest.embedding_backend.clone())
            .or_else(|| {
                current_embedding
                    .as_ref()
                    .map(|contract| contract.cache_key.clone())
            })
            .or(probe.model_id),
        probe.available,
        fallback_message,
        current_embedding,
        stored_embedding,
        runtime_degraded,
        zero_dense_published,
        runtime.retrieval.hybrid_enabled,
    ))
}

/// Test-support: stage the store a *served* full publication actually has.
///
/// Readiness derives freshness from
/// `storage_admission_refusal_reason_for_runtime`, which recounts the symbol
/// docs, dense anchors, and dense-reason histogram in the store and refuses a
/// manifest that disagrees with them. Upserting only the manifest row
/// therefore stages a publication the sidecar would *refuse*: a manifest whose
/// recorded counts nothing in the store backs is exactly the core-only-refresh
/// shape readiness is supposed to catch, so such a fixture can never stand in
/// for a healthy project. Seed the rows the manifest's own counts describe, so
/// a fixture that calls itself healthy is healthy.
///
/// Returns the manifest as published: `dense_reason_counts_json` is rewritten
/// to the histogram of the anchors this seeds, because admission compares the
/// manifest's copy against a recount rather than trusting it.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn publish_admissible_retrieval_manifest(
    storage: &mut Storage,
    manifest: &codestory_store::RetrievalIndexManifest,
) -> Result<codestory_store::RetrievalIndexManifest, ApiError> {
    use crate::semantic_projection::{
        DenseAnchorReason, LLM_SYMBOL_DOC_SCHEMA_VERSION, SYMBOL_SEARCH_DOC_PROVENANCE,
    };
    use codestory_contracts::graph::{Node, NodeId as CoreNodeId, NodeKind};
    use codestory_store::{DenseAnchorInput, SymbolSearchDoc};

    fn storage_error(context: &str, error: impl std::fmt::Display) -> ApiError {
        ApiError::internal(format!("{context}: {error}"))
    }

    let mut manifest = manifest.clone();
    let symbol_doc_count = manifest.symbol_doc_count.unwrap_or(0).max(0);
    let dense_count = manifest
        .dense_projection_count
        .or(manifest.projection_count)
        .unwrap_or(0)
        .max(0);
    let selection_reason = DenseAnchorReason::PublicApi.as_str().to_string();
    manifest.dense_reason_counts_json = Some(if dense_count > 0 {
        serde_json::json!({ selection_reason.clone(): dense_count }).to_string()
    } else {
        "{}".to_string()
    });

    // Nodes first: symbol docs and dense anchors are keyed by node id.
    let node_count = symbol_doc_count.max(dense_count);
    let nodes = (1..=node_count)
        .map(|id| Node {
            id: CoreNodeId(id),
            kind: NodeKind::FUNCTION,
            serialized_name: format!("admissible_{id:02}"),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    storage
        .insert_nodes_batch(&nodes)
        .map_err(|error| storage_error("Failed to seed admissible publication nodes", error))?;

    let symbol_docs = (1..=symbol_doc_count)
        .map(|id| SymbolSearchDoc {
            node_id: CoreNodeId(id),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: format!("admissible_{id:02}"),
            qualified_name: None,
            file_path: None,
            start_line: None,
            doc_text: format!("admissible_{id:02}"),
            doc_version: LLM_SYMBOL_DOC_SCHEMA_VERSION,
            doc_hash: format!("admissible-doc-{id:02}"),
            policy_version: codestory_retrieval::SEMANTIC_POLICY_VERSION.to_string(),
            source_provenance: SYMBOL_SEARCH_DOC_PROVENANCE.to_string(),
            updated_at_epoch_ms: 1,
        })
        .collect::<Vec<_>>();
    storage
        .upsert_symbol_search_docs_batch(&symbol_docs)
        .map_err(|error| {
            storage_error("Failed to seed admissible publication symbol docs", error)
        })?;

    let dense_inputs = (1..=dense_count)
        .map(|id| DenseAnchorInput {
            node_id: CoreNodeId(id),
            file_node_id: None,
            kind: NodeKind::FUNCTION,
            display_name: format!("admissible_{id:02}"),
            qualified_name: None,
            file_path: None,
            start_line: None,
            end_line: None,
            file_role: codestory_store::FileRole::Source,
            source_provenance: SYMBOL_SEARCH_DOC_PROVENANCE.to_string(),
            text: format!("admissible_{id:02}"),
            document_hash: format!("admissible-anchor-{id:02}"),
            selection_reason: selection_reason.clone(),
            policy_version: codestory_retrieval::SEMANTIC_POLICY_VERSION.to_string(),
            source_identity: format!("core:admissible_{id:02}"),
            updated_at_epoch_ms: 1,
        })
        .collect::<Vec<_>>();
    storage
        .upsert_dense_anchor_inputs_batch(&dense_inputs)
        .map_err(|error| {
            storage_error("Failed to seed admissible publication dense anchors", error)
        })?;

    storage
        .upsert_retrieval_index_manifest(&manifest)
        .map_err(|error| storage_error("Failed to publish admissible retrieval manifest", error))?;
    Ok(manifest)
}

fn published_dense_projection_count(manifest: &codestory_store::RetrievalIndexManifest) -> u32 {
    match manifest.dense_projection_count {
        Some(count) if count > 0 => u32::try_from(count).unwrap_or(u32::MAX),
        _ => 0,
    }
}

/// Manifest-only check that the published vectors were produced under the
/// embedding contract this runtime queries with.
fn manifest_matches_current_embedding_contract(
    manifest: &codestory_store::RetrievalIndexManifest,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> bool {
    let expected_backend = codestory_retrieval::embedding_runtime_id_for_runtime(runtime);
    let expected_dim = i32::try_from(codestory_retrieval::semantic_vector_dim())
        .unwrap_or(codestory_retrieval::RETRIEVAL_EMBEDDING_DIM as i32);
    manifest.embedding_backend.as_deref() == Some(expected_backend.as_str())
        && manifest.embedding_dim == Some(expected_dim)
}

/// Project the manifest's producer evidence into the consumer vocabulary of
/// [`StoredSemanticDocsContractDto`].
///
/// The manifest records the embedding identity as one opaque runtime id (its
/// `embedding_backend` column) plus a dimension and the semantic policy
/// version. That runtime id lives in the same identity space as the current
/// contract's `cache_key` (`embedding_runtime_id()`), so it maps to
/// `cache_key`. The manifest does not carry a per-doc backend label, profile,
/// doc shape, or doc version: those inputs are validated at publication time
/// and folded into the sidecar input hash, so they stay `None` here instead
/// of being fabricated in a vocabulary the doctor would flag forever.
fn stored_semantic_docs_contract_from_manifest(
    manifest: &codestory_store::RetrievalIndexManifest,
) -> StoredSemanticDocsContractDto {
    StoredSemanticDocsContractDto {
        doc_count: published_dense_projection_count(manifest),
        embedding_profile: None,
        embedding_backend: None,
        cache_key: manifest.embedding_backend.clone(),
        dimension: manifest
            .embedding_dim
            .and_then(|dimension| u32::try_from(dimension).ok()),
        doc_version: None,
        mixed_embedding_profiles: false,
        mixed_embedding_models: false,
        mixed_embedding_backends: false,
        mixed_dimensions: false,
        mixed_doc_versions: false,
        mixed_doc_shapes: false,
        doc_shape: None,
        semantic_policy_version: manifest.semantic_policy_version.clone(),
        mixed_semantic_policy_versions: false,
    }
}
