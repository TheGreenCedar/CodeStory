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
    load_persisted_semantic_docs_for_runtime, semantic_doc_stats_match_contract,
    stored_semantic_docs_contract_from_stats,
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
    hybrid_configured: bool,
) -> RetrievalStateDto {
    let fallback_reason = if !hybrid_configured {
        Some(RetrievalFallbackReasonDto::DisabledByConfig)
    } else if runtime_degraded {
        Some(RetrievalFallbackReasonDto::DegradedRuntime)
    } else if !embedding_runtime_available {
        Some(RetrievalFallbackReasonDto::MissingEmbeddingRuntime)
    } else if semantic_doc_count == 0 {
        Some(RetrievalFallbackReasonDto::MissingSemanticDocs)
    } else {
        None
    };
    let semantic_mode = if !hybrid_configured {
        SemanticModeDto::DisabledByConfig
    } else if runtime_degraded || !embedding_runtime_available || semantic_doc_count == 0 {
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
) -> Result<RetrievalStateDto, ApiError> {
    retrieval_state_from_storage_for_runtime(storage, &test_sidecar_runtime_from_env())
}

pub(super) fn retrieval_state_from_storage_for_runtime(
    storage: &Storage,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> Result<RetrievalStateDto, ApiError> {
    let stats = storage
        .get_llm_symbol_doc_stats()
        .map_err(|e| ApiError::internal(format!("Failed to query LLM symbol doc stats: {e}")))?;
    let probe = embedding_runtime_availability_from_config(runtime);
    let current_embedding = current_embedding_contract_for_runtime(runtime);
    let stored_embedding = stored_semantic_docs_contract_from_stats(&stats);
    let contract_mismatch = stats.doc_count > 0
        && probe.available
        && !current_embedding
            .as_ref()
            .is_some_and(|contract| semantic_doc_stats_match_contract(&stats, contract));
    let fallback_message = probe.fallback_message.or_else(|| {
        contract_mismatch.then(|| {
            "Stored semantic docs do not match the current embedding contract. Run `retrieval index --refresh full` before trusting hybrid retrieval."
                .to_string()
        })
    });
    Ok(retrieval_state_from_parts_with_hybrid(
        stats.doc_count,
        stats
            .embedding_model
            .clone()
            .or_else(|| {
                current_embedding
                    .as_ref()
                    .map(|contract| contract.cache_key.clone())
            })
            .or(probe.model_id),
        probe.available,
        fallback_message,
        current_embedding,
        Some(stored_embedding),
        contract_mismatch,
        runtime.retrieval.hybrid_enabled,
    ))
}
