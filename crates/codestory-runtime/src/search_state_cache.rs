use crate::search;
use crate::search_publication::{
    SearchGenerationCatalogGuard, discard_unpublished_search_generation,
    load_canonical_search_symbols, prune_search_generations, read_search_generation_completion,
    search_index_path_for_publication, write_search_generation_completion,
};
use crate::semantic_projection::{
    CacheRefreshStats, SEARCH_SYMBOL_STREAM_BATCH_SIZE, SearchStateBuildResult,
    SearchStateBuildStats, load_persisted_semantic_docs_for_runtime,
};
use crate::{
    AppController, SearchEngine, clamp_u128_to_u32, clamp_usize_to_u32, clear_search_engine,
    publish_search_engine, source_policy_exclusion_candidate,
};
#[cfg(test)]
use crate::{
    publication::{PublicationTestBoundary, publication_test_checkpoint},
    test_sidecar_runtime_from_env,
};
use codestory_contracts::api::ApiError;
use codestory_indexer::CancellationToken;
use codestory_store::{IndexPublicationRecord, Store};
use codestory_workspace::RefreshInputs;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;
use uuid::Uuid;

pub(super) fn is_indexing_cancelled(cancel_token: Option<&CancellationToken>) -> bool {
    cancel_token
        .map(CancellationToken::is_cancelled)
        .unwrap_or(false)
}

pub(super) fn indexing_cancelled_error() -> ApiError {
    ApiError::new("cancelled", "Indexing cancelled.")
}

pub(super) fn ensure_indexing_active(
    cancel_token: Option<&CancellationToken>,
) -> Result<(), ApiError> {
    if is_indexing_cancelled(cancel_token) {
        Err(indexing_cancelled_error())
    } else {
        Ok(())
    }
}

pub(super) fn workspace_refresh_inputs(store: &Store) -> Result<RefreshInputs, ApiError> {
    Ok(RefreshInputs {
        stored_files: store
            .files()
            .inventory()
            .map_err(|e| ApiError::internal(format!("Failed to read workspace inventory: {e}")))?,
        policy_exclusions: store
            .get_source_policy_exclusions()
            .map_err(|e| {
                ApiError::internal(format!("Failed to read source policy exclusions: {e}"))
            })?
            .iter()
            .map(source_policy_exclusion_candidate)
            .collect(),
        inventory: Default::default(),
    })
}

fn reuse_completed_search_state(
    storage: &mut Store,
    search_storage_path: &Path,
    publication: &IndexPublicationRecord,
    hydrate_semantic_docs: bool,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    cancel_token: Option<&CancellationToken>,
) -> Result<Option<SearchStateBuildResult>, ApiError> {
    let generation_id = Uuid::parse_str(&publication.generation_id)
        .map_err(|error| {
            ApiError::internal(format!(
                "Invalid index publication generation id {}: {error}",
                publication.generation_id
            ))
        })?
        .to_string();
    let Some(marker) = read_search_generation_completion(search_storage_path, &generation_id)
    else {
        return Ok(None);
    };

    let search_index_started = Instant::now();
    let mut engine = match SearchEngine::open_existing(search_storage_path) {
        Ok(engine) => engine,
        Err(error) => {
            tracing::warn!(
                path = %search_storage_path.display(),
                "Completed persisted search generation could not be reopened and will be rebuilt: {error}"
            );
            return Ok(None);
        }
    };
    engine.load_symbol_projection(std::iter::empty());
    let (node_names, mut search_stats) = load_canonical_search_symbols(
        storage,
        SEARCH_SYMBOL_STREAM_BATCH_SIZE,
        cancel_token,
        |batch| {
            engine.extend_symbol_projection(
                batch
                    .into_iter()
                    .map(|entry| (entry.node_id, entry.display_name)),
            );
            Ok(())
        },
    )?;
    if marker.symbol_count != search_stats.search_symbol_stream_rows as u64
        || engine.full_text_doc_count() != search_stats.search_symbol_stream_rows as usize
        || engine.tantivy_doc_count() as u64 != marker.tantivy_doc_count
    {
        tracing::warn!(
            path = %search_storage_path.display(),
            searchable_docs = engine.full_text_doc_count(),
            stored_docs = engine.tantivy_doc_count(),
            expected_symbols = search_stats.search_symbol_stream_rows,
            expected_stored_docs = marker.tantivy_doc_count,
            "Completed persisted search generation count validation failed and will be rebuilt"
        );
        return Ok(None);
    }
    search_stats.search_symbol_index_ms =
        clamp_u128_to_u32(search_index_started.elapsed().as_millis());
    let semantic_stats = load_persisted_semantic_docs_for_runtime(
        storage,
        &mut engine,
        hydrate_semantic_docs,
        runtime,
    )?;
    Ok(Some(SearchStateBuildResult {
        publication: Some(publication.clone()),
        node_names,
        engine,
        search_stats,
        semantic_stats,
    }))
}

pub(super) fn build_persisted_search_state_from_canonical_symbols(
    storage: &mut Store,
    search_storage_path: &Path,
    hydrate_semantic_docs: bool,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    cancel_token: Option<&CancellationToken>,
) -> Result<SearchStateBuildResult, ApiError> {
    let search_index_started = Instant::now();
    let count_started = Instant::now();
    let expected_rows = storage
        .get_canonical_search_symbol_count()
        .map_err(|error| {
            ApiError::internal(format!("Failed to count canonical search symbols: {error}"))
        })?;
    let mut stream_duration = count_started.elapsed();
    let mut engine = SearchEngine::new(Some(search_storage_path)).map_err(|error| {
        if search::engine::is_persisted_search_index_busy(&error) {
            ApiError::new(
                "cache_busy",
                format!("Failed to init search engine: {error}"),
            )
        } else {
            ApiError::internal(format!("Failed to init search engine: {error}"))
        }
    })?;
    let mut node_names = HashMap::with_capacity(expected_rows as usize);
    let mut symbol_session = engine.begin_symbol_index().map_err(|error| {
        ApiError::internal(format!("Failed to start symbol index writer: {error}"))
    })?;
    let mut after_node_id = None;
    let mut stream_rows = 0_usize;
    let mut stream_batches = 0_usize;
    loop {
        let batch_started = Instant::now();
        let batch = storage
            .get_canonical_search_symbol_batch_after(after_node_id, SEARCH_SYMBOL_STREAM_BATCH_SIZE)
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to stream canonical search symbols: {error}"
                ))
            })?;
        stream_duration = stream_duration.saturating_add(batch_started.elapsed());
        if batch.is_empty() {
            break;
        }
        after_node_id = batch.last().map(|entry| entry.node_id);
        stream_rows = stream_rows.saturating_add(batch.len());
        stream_batches = stream_batches.saturating_add(1);
        let symbols = batch
            .into_iter()
            .map(|entry| {
                node_names.insert(entry.node_id, entry.display_name.clone());
                (entry.node_id, entry.display_name)
            })
            .collect::<Vec<_>>();
        symbol_session.add_nodes(symbols).map_err(|error| {
            ApiError::internal(format!("Failed to index search nodes: {error}"))
        })?;
        #[cfg(test)]
        publication_test_checkpoint(PublicationTestBoundary::SearchSymbolPage, cancel_token)?;
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
    }
    if stream_rows != expected_rows as usize {
        return Err(ApiError::internal(format!(
            "Canonical search symbol stream count changed: expected {expected_rows}, loaded {stream_rows}"
        )));
    }
    #[cfg(test)]
    publication_test_checkpoint(PublicationTestBoundary::SearchIndexWrite, cancel_token)?;
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    let symbol_write_stats = symbol_session
        .finish()
        .map_err(|error| ApiError::internal(format!("Failed to commit symbol index: {error}")))?;
    if engine.full_text_doc_count() != stream_rows {
        return Err(ApiError::internal(format!(
            "Persisted search generation validation failed: indexed {} docs for {stream_rows} canonical symbols",
            engine.full_text_doc_count()
        )));
    }
    let search_symbol_index_ms = clamp_u128_to_u32(search_index_started.elapsed().as_millis());
    let semantic_stats = load_persisted_semantic_docs_for_runtime(
        storage,
        &mut engine,
        hydrate_semantic_docs,
        runtime,
    )?;
    Ok(SearchStateBuildResult {
        publication: None,
        node_names,
        engine,
        search_stats: SearchStateBuildStats {
            search_projection_rebuild_ms: 0,
            search_symbol_stream_ms: clamp_u128_to_u32(stream_duration.as_millis()),
            search_symbol_stream_rows: clamp_usize_to_u32(stream_rows),
            search_symbol_stream_batches: clamp_usize_to_u32(stream_batches),
            search_symbol_index_ms,
            search_symbol_index_docs_written: clamp_usize_to_u32(symbol_write_stats.docs_written),
            search_symbol_index_writer_count: clamp_usize_to_u32(symbol_write_stats.writer_count),
            search_symbol_index_commit_count: clamp_usize_to_u32(symbol_write_stats.commit_count),
            search_symbol_index_reload_count: clamp_usize_to_u32(symbol_write_stats.reload_count),
            search_symbol_index_commit_ms: clamp_u128_to_u32(
                symbol_write_stats.commit_duration.as_millis(),
            ),
            search_symbol_index_reload_ms: clamp_u128_to_u32(
                symbol_write_stats.reload_duration.as_millis(),
            ),
        },
        semantic_stats,
    })
}

#[cfg(test)]
pub(super) fn rebuild_search_state_from_storage(
    storage: &mut Store,
    storage_path: &Path,
    llm_refresh_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
    hydrate_semantic_docs: bool,
) -> Result<SearchStateBuildResult, ApiError> {
    rebuild_search_state_from_storage_for_runtime(
        storage,
        storage_path,
        llm_refresh_scope,
        hydrate_semantic_docs,
        &test_sidecar_runtime_from_env(),
        None,
        None,
    )
}

type SearchCompletionValidator<'a> =
    &'a mut dyn FnMut(&IndexPublicationRecord) -> Result<(), ApiError>;

pub(super) fn rebuild_search_state_from_storage_for_runtime(
    storage: &mut Store,
    storage_path: &Path,
    _llm_refresh_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
    hydrate_semantic_docs: bool,
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
    cancel_token: Option<&CancellationToken>,
    mut validate_before_completion: Option<SearchCompletionValidator<'_>>,
) -> Result<SearchStateBuildResult, ApiError> {
    let publication = storage.get_index_publication().map_err(|error| {
        ApiError::internal(format!(
            "Failed to read search publication identity: {error}"
        ))
    })?;
    let _catalog_guard = publication
        .as_ref()
        .map(|_| SearchGenerationCatalogGuard::acquire(storage_path))
        .transpose()?;
    let search_storage_path =
        search_index_path_for_publication(storage_path, publication.as_ref())?;
    let reused = match publication.as_ref() {
        Some(publication) => reuse_completed_search_state(
            storage,
            &search_storage_path,
            publication,
            hydrate_semantic_docs,
            runtime,
            cancel_token,
        )?,
        None => None,
    };
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    #[cfg(test)]
    publication_test_checkpoint(PublicationTestBoundary::SearchBuild, cancel_token)?;
    let built_new = reused.is_none();
    let mut result = match reused {
        Some(result) => result,
        None => build_persisted_search_state_from_canonical_symbols(
            storage,
            search_storage_path.as_path(),
            hydrate_semantic_docs,
            runtime,
            cancel_token,
        )
        .map_err(|mut error| {
            error.message = format!("Failed to rebuild search state: {}", error.message);
            error
        })?,
    };
    if is_indexing_cancelled(cancel_token) {
        return Err(indexing_cancelled_error());
    }
    #[cfg(test)]
    publication_test_checkpoint(PublicationTestBoundary::SearchValidation, cancel_token)?;
    if result.engine.full_text_doc_count() != result.node_names.len() {
        return Err(ApiError::internal(format!(
            "Prepared search generation contains {} searchable symbols for {} core symbols",
            result.engine.full_text_doc_count(),
            result.node_names.len()
        )));
    }
    if built_new && let Some(publication) = publication.as_ref() {
        if is_indexing_cancelled(cancel_token) {
            return Err(indexing_cancelled_error());
        }
        if let Some(validate) = validate_before_completion.as_mut()
            && let Err(error) = validate(publication)
        {
            drop(result);
            discard_unpublished_search_generation(storage_path, publication);
            return Err(error);
        }
        #[cfg(test)]
        publication_test_checkpoint(PublicationTestBoundary::SearchCompletion, cancel_token)?;
        write_search_generation_completion(
            &search_storage_path,
            publication,
            result.node_names.len(),
            result.engine.tantivy_doc_count(),
        )?;
    }
    if publication.is_some() {
        result
            .engine
            .downgrade_persisted_lock_to_shared()
            .map_err(|error| {
                ApiError::internal(format!(
                    "Failed to share completed search generation {}: {error}",
                    search_storage_path.display()
                ))
            })?;
    }
    if publication.is_some()
        && let Some(active_generation_id) =
            search_storage_path.file_name().and_then(|id| id.to_str())
        && let Err(error) = prune_search_generations(storage_path, active_generation_id)
    {
        tracing::warn!(
            generation_id = %active_generation_id,
            "Failed to prune persisted search generations after publication: {}",
            error.message
        );
    }
    result.publication = publication;
    Ok(result)
}

pub(super) fn refresh_caches(
    controller: &AppController,
    storage: &mut Store,
    storage_path: &Path,
    llm_refresh_scope: Option<&HashSet<codestory_contracts::graph::NodeId>>,
) -> Result<CacheRefreshStats, ApiError> {
    let refreshed = rebuild_search_state_from_storage_for_runtime(
        storage,
        storage_path,
        llm_refresh_scope,
        true,
        &controller.runtime_config,
        None,
        None,
    );

    match refreshed {
        Ok(result) => Ok(publish_prepared_search_state(controller, result)),
        Err(error) => {
            tracing::warn!(
                "Failed to rebuild search caches from storage: {}",
                error.message
            );
            let mut state = controller.state.lock();
            state.node_names.clear();
            clear_search_engine(&mut state);
            controller.sidecar_query_cache.lock().clear();
            state.is_indexing = false;
            Err(error)
        }
    }
}

pub(super) fn publish_prepared_search_state(
    controller: &AppController,
    result: SearchStateBuildResult,
) -> CacheRefreshStats {
    let publish_started = Instant::now();
    let mut state = controller.state.lock();
    state.node_names = result.node_names;
    publish_search_engine(&mut state, result.engine, result.publication);
    controller.sidecar_query_cache.lock().clear();
    state.is_indexing = false;
    CacheRefreshStats {
        search_stats: result.search_stats,
        semantic_stats: result.semantic_stats,
        runtime_cache_publish_ms: Some(clamp_u128_to_u32(publish_started.elapsed().as_millis())),
    }
}
