//! Core-only ranked callable locations. Ranking never walks the project tree.
use crate::search_publication::{
    SearchGenerationCatalogGuard, callable_overlay_path, read_search_generation_completion,
    search_index_path_for_publication,
};
use crate::support::{BoundedTextRead, read_file_text_limited};
use anyhow::{Result, bail};
use codestory_contracts::api::ApiError;
use codestory_contracts::graph::FileCoverageReason;
use codestory_indexer::CancellationToken;
use codestory_indexer::structural::{is_structural_candidate_path, is_structural_source_text};
use codestory_retrieval::callable_index::{
    self, CALLABLE_INDEX_FILE, Definition, PreviousGeneration, Writer,
};
use codestory_store::{CallableDefinition, CallableDefinitionFile, IndexPublicationRecord, Store};
use codestory_workspace::owned_deletion::OwnedDeletionRoot;
use codestory_workspace::{ProjectRelativePathResolution, resolve_project_relative_path};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

pub(crate) use codestory_retrieval::callable_index::Hit as CallableHit;
const MAX_RETURNED_HITS: usize = 50;
const MAX_SOURCE_VERIFICATION_BYTES: u64 = 24 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CallableCoverage {
    Ready {
        indexed_callables: usize,
        partial_files: usize,
        omitted_files: usize,
        omitted_definitions: usize,
    },
    Missing,
    Unavailable,
}

#[derive(Debug)]
pub(crate) struct CallableSearchOutcome {
    pub(crate) hits: Vec<CallableHit>,
    pub(crate) coverage: CallableCoverage,
    pub(crate) refresh_recommended: bool,
    pub(crate) stale_candidates: usize,
    pub(crate) query_truncated: bool,
    pub(crate) absent_terms: Vec<String>,
}

impl CallableSearchOutcome {
    fn unavailable(coverage: CallableCoverage) -> Self {
        Self {
            hits: vec![],
            coverage,
            refresh_recommended: true,
            stale_candidates: 0,
            query_truncated: false,
            absent_terms: Vec::new(),
        }
    }
}

/// Read only the callable artifact from this already-pinned complete core.
/// Positive matches are bounded by exact live source and Core definition witnesses.
pub(crate) fn query(
    store: &Store,
    storage_path: &Path,
    project_root: &Path,
    question: &str,
    limit: usize,
    in_scope: impl Fn(&str) -> bool,
    cancelled: impl Fn() -> bool,
) -> std::result::Result<CallableSearchOutcome, ApiError> {
    check_cancel(&cancelled)?;
    let publication = match store.get_complete_index_publication() {
        Ok(Some(publication)) => publication,
        Ok(None) => {
            return Ok(CallableSearchOutcome::unavailable(
                CallableCoverage::Missing,
            ));
        }
        Err(_) => {
            return Ok(CallableSearchOutcome::unavailable(
                CallableCoverage::Unavailable,
            ));
        }
    };
    let path = search_index_path_for_publication(storage_path, Some(&publication))?;
    if read_search_generation_completion(&path, &publication.generation_id).is_none() {
        return Ok(CallableSearchOutcome::unavailable(
            CallableCoverage::Missing,
        ));
    }
    let (_, reader) = match open_generation_index(&path, &publication) {
        Ok(selected) => selected,
        Err(coverage) => return Ok(CallableSearchOutcome::unavailable(coverage)),
    };
    let ranking_limit = limit.min(MAX_RETURNED_HITS);
    let read = reader
        .rank(
            question,
            ranking_limit.saturating_mul(4),
            in_scope,
            &cancelled,
        )
        .map(|ranked| (reader.coverage, ranked));
    let (coverage, ranked) = match read {
        Ok(ranked) => ranked,
        Err(error) => {
            check_cancel(&cancelled)?;
            tracing::debug!(%error, "callable ranking unavailable");
            return Ok(CallableSearchOutcome::unavailable(
                CallableCoverage::Unavailable,
            ));
        }
    };
    let mut outcome = CallableSearchOutcome {
        hits: Vec::with_capacity(ranking_limit),
        coverage: CallableCoverage::Ready {
            indexed_callables: coverage.indexed_callables,
            partial_files: coverage.partial_files,
            omitted_files: coverage.omitted_files,
            omitted_definitions: coverage.omitted_definitions,
        },
        refresh_recommended: false,
        stale_candidates: 0,
        query_truncated: ranked.truncated || ranking_limit < limit,
        absent_terms: ranked.absent_terms,
    };
    let mut remaining_bytes = MAX_SOURCE_VERIFICATION_BYTES;
    let mut files = HashMap::new();
    for candidate in ranked.hits {
        check_cancel(&cancelled)?;
        if let std::collections::hash_map::Entry::Vacant(entry) = files.entry(candidate.file_id) {
            if candidate.file_bytes >= remaining_bytes {
                outcome.query_truncated = true;
                continue;
            }
            let source = verify_file(store, project_root, &candidate, &mut remaining_bytes);
            entry.insert(source);
        }
        if let Some(source) = &files[&candidate.file_id]
            && matches_definition(store, &candidate, source)
        {
            outcome.hits.push(candidate);
            if outcome.hits.len() == ranking_limit {
                break;
            }
        } else {
            outcome.stale_candidates += 1;
            outcome.refresh_recommended = true;
        }
    }
    outcome.query_truncated |= ranked.has_more && outcome.hits.len() < ranking_limit;
    check_cancel(&cancelled)?;
    Ok(outcome)
}

fn check_cancel(cancelled: &impl Fn() -> bool) -> std::result::Result<(), ApiError> {
    if cancelled() {
        return Err(ApiError::new("cancelled", "callable lookup was cancelled"));
    }
    Ok(())
}

struct VerifiedFile {
    relative: String,
    hash: String,
    content: String,
    offsets: Vec<usize>,
}

fn verify_file(
    store: &Store,
    root: &Path,
    hit: &CallableHit,
    remaining: &mut u64,
) -> Option<VerifiedFile> {
    let info = store.get_file_by_id(hit.file_id).ok()??;
    if !info.indexed {
        return None;
    }
    let hash = store.get_file_content_hash(hit.file_id).ok()??;
    if hash != hit.file_sha256 {
        return None;
    }
    let (absolute, relative) = contained(root, &info.path)?;
    if relative != hit.path {
        return None;
    }
    let read = read_file_text_limited(&absolute, hit.file_bytes);
    *remaining = remaining.saturating_sub(read.bytes_read());
    let BoundedTextRead::Contents {
        contents,
        bytes_read,
    } = read
    else {
        return None;
    };
    if bytes_read != hit.file_bytes
        || hex_hash(&contents) != hash
        || (is_structural_candidate_path(&info.path)
            && !is_structural_source_text(contents.as_bytes()))
    {
        return None;
    }
    Some(VerifiedFile {
        relative,
        hash,
        offsets: line_offsets(&contents),
        content: contents,
    })
}

fn matches_definition(store: &Store, candidate: &CallableHit, file: &VerifiedFile) -> bool {
    if candidate.path != file.relative
        || candidate.file_sha256 != file.hash
        || candidate.file_bytes != file.content.len() as u64
    {
        return false;
    }
    let Ok(definitions) = store.callable_definitions_for_node(candidate.file_id, candidate.node_id)
    else {
        return false;
    };
    definitions.iter().any(|definition| {
        definition.name == candidate.name
            && definition.start_line == candidate.start_line
            && extent(&file.content, &file.offsets, definition).is_some_and(
                |(first, last, end_line)| {
                    first == candidate.start_byte as usize
                        && last == candidate.end_byte as usize
                        && end_line == candidate.end_line
                        && hex_hash(&file.content[first..last]) == candidate.body_sha256
                },
            )
    })
}

fn line_offsets(source: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    offsets.extend(source.match_indices('\n').map(|(offset, _)| offset + 1));
    offsets.push(source.len());
    offsets
}

/// The occurrence uses 1-based byte columns and an exclusive end, including at the start of the following line.
fn extent(
    source: &str,
    offsets: &[usize],
    definition: &CallableDefinition,
) -> Option<(usize, usize, u32)> {
    let start_row = definition.start_line.checked_sub(1)? as usize;
    let end_row = definition.end_line.checked_sub(1)? as usize;
    let start = offsets
        .get(start_row)?
        .checked_add(definition.start_col.checked_sub(1)? as usize)?;
    let end = offsets
        .get(end_row)?
        .checked_add(definition.end_col.checked_sub(1)? as usize)?;
    let final_line = definition.end_line - u32::from(definition.end_col == 1);
    if definition.start_line > final_line
        || start >= *offsets.get(start_row + 1)?
        || end > *offsets.get(end_row + 1)?
        || start >= end
    {
        return None;
    }
    source.get(start..end)?;
    Some((start, end, final_line))
}

fn contained(root: &Path, path: &Path) -> Option<(PathBuf, String)> {
    let ProjectRelativePathResolution::Existing { absolute, relative } =
        resolve_project_relative_path(root, path).ok()?
    else {
        return None;
    };
    let relative = relative.to_str()?;
    #[cfg(windows)]
    let relative = relative.replace('\\', "/");
    #[cfg(not(windows))]
    let relative = relative.to_owned();
    Some((absolute, relative))
}

fn hex_hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

/// Prefer an independently repaired cache for this exact generation while
/// keeping the original published search generation observational.
pub(super) fn open_generation_index(
    search_path: &Path,
    publication: &IndexPublicationRecord,
) -> std::result::Result<(PathBuf, callable_index::Reader), CallableCoverage> {
    let overlay = callable_overlay_path(search_path, publication).filter(|path| {
        path.parent().is_some_and(|parent| {
            std::fs::symlink_metadata(parent).is_ok_and(|metadata| metadata.is_dir())
        })
    });
    let mut unavailable = false;
    for path in overlay
        .into_iter()
        .chain(std::iter::once(search_path.join(CALLABLE_INDEX_FILE)))
    {
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            _ => {
                if let Ok(reader) =
                    callable_index::open(&path, &publication.generation_id, &publication.run_id)
                {
                    return Ok((path, reader));
                }
                unavailable = true;
            }
        }
    }
    Err(if unavailable {
        CallableCoverage::Unavailable
    } else {
        CallableCoverage::Missing
    })
}

/// Repair only the optional search cache for an already-pinned complete core.
/// The writer validates its source inventory again before the atomic move.
pub(super) fn repair_pinned_generation(
    storage_path: &Path,
    root: &Path,
    publication: &IndexPublicationRecord,
    max_file_bytes: u64,
    cancel: Option<&CancellationToken>,
    validate_sources: impl FnOnce() -> bool,
) -> Result<bool> {
    let cancelled = || cancel.is_some_and(CancellationToken::is_cancelled);
    if cancelled() {
        return Ok(false);
    }
    let path = search_index_path_for_publication(storage_path, Some(publication))
        .map_err(|error| anyhow::anyhow!("{}", error.message))?;
    if generation_has_index(&path, publication) {
        return Ok(false);
    }
    let Some(artifact) = callable_overlay_path(&path, publication) else {
        return Ok(false);
    };
    let storage = Store::open_observational(storage_path)?;
    let snapshot = storage.read_snapshot()?;
    if snapshot
        .storage()
        .get_complete_index_publication()?
        .as_ref()
        != Some(publication)
    {
        return Ok(false);
    }
    let _guard = SearchGenerationCatalogGuard::acquire(storage_path)
        .map_err(|error| anyhow::anyhow!("{}", error.message))?;
    if read_search_generation_completion(&path, &publication.generation_id).is_none()
        || generation_has_index(&path, publication)
        || cancelled()
    {
        return Ok(false);
    }

    let folder = artifact.parent().expect("overlay has an owned directory");
    match std::fs::create_dir(folder) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    if !std::fs::symlink_metadata(folder)?.is_dir() {
        return Ok(false);
    }
    match std::fs::symlink_metadata(&artifact) {
        Ok(_)
            if !callable_index::has_generation_binding(
                &artifact,
                &publication.generation_id,
                &publication.run_id,
            ) =>
        {
            return Ok(false);
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    let temporary_name = format!(".{}.staged", uuid::Uuid::new_v4());
    let temporary = folder.join(&temporary_name);
    let deletion = OwnedDeletionRoot::open(folder)?;
    callable_index::write(
        &temporary,
        &publication.generation_id,
        &publication.run_id,
        None,
        |writer| populate(writer, snapshot.storage(), root, max_file_bytes, &cancelled),
        cancelled,
    )?;
    let result = (|| {
        if cancelled()
            || !validate_sources()
            || cancelled()
            || Store::database_complete_index_publication(storage_path)?.as_ref()
                != Some(publication)
            || cancelled()
        {
            return Ok(false);
        }
        callable_index::publish_staged(
            &temporary,
            &artifact,
            &publication.generation_id,
            &publication.run_id,
        )?;
        Ok(generation_has_index(&path, publication))
    })();
    if temporary.exists() {
        let _ = deletion.remove(Path::new(&temporary_name));
    }
    result
}

/// A callable failure leaves the normal core/symbol generation usable.
/// Fresh stages bind the artifact to their own generation and run.
pub(super) fn prepare_generation(
    store: &Store,
    storage_path: &Path,
    root: &Path,
    publication: &IndexPublicationRecord,
    max_file_bytes: u64,
    cancel: Option<&CancellationToken>,
    previous: Option<(&IndexPublicationRecord, &[i64])>,
) {
    if let Err(error) = prepare_generation_inner(
        store,
        storage_path,
        root,
        publication,
        max_file_bytes,
        cancel,
        previous,
    ) && !cancel.is_some_and(CancellationToken::is_cancelled)
    {
        tracing::warn!(
            ?error,
            "callable index preparation failed; core publication remains usable"
        );
    }
}

fn prepare_generation_inner(
    store: &Store,
    storage_path: &Path,
    root: &Path,
    publication: &IndexPublicationRecord,
    max_file_bytes: u64,
    cancel: Option<&CancellationToken>,
    previous: Option<(&IndexPublicationRecord, &[i64])>,
) -> std::result::Result<(), ApiError> {
    let cancelled = || cancel.is_some_and(CancellationToken::is_cancelled);
    check_cancel(&cancelled)?;
    let path = search_index_path_for_publication(storage_path, Some(publication))?;
    // The catalog lock protects predecessor files from generation retirement.
    let _guard = SearchGenerationCatalogGuard::acquire(storage_path)?;
    if read_search_generation_completion(&path, &publication.generation_id).is_none()
        || generation_has_index(&path, publication)
    {
        return Ok(());
    }
    let artifact = path.join(CALLABLE_INDEX_FILE);
    if let Some((previous, affected_files)) = previous {
        let previous_path = search_index_path_for_publication(storage_path, Some(previous))?;
        let source = open_generation_index(&previous_path, previous)
            .ok()
            .map(|(path, _)| path)
            .unwrap_or_else(|| previous_path.join(CALLABLE_INDEX_FILE));
        let mut changed = affected_files.to_vec();
        changed.sort_unstable();
        changed.dedup();
        let result = callable_index::write(
            &artifact,
            &publication.generation_id,
            &publication.run_id,
            Some(PreviousGeneration {
                path: &source,
                generation: &previous.generation_id,
                run: &previous.run_id,
                changed_file_ids: &changed,
            }),
            |writer| {
                for &id in &changed {
                    if let Some(info) = store.get_file_by_id(id)? {
                        let file = CallableDefinitionFile {
                            id,
                            path: info.path,
                            indexed: info.indexed,
                            complete: info.complete,
                            content_hash: store.get_file_content_hash(id)?,
                        };
                        populate_file(writer, store, root, file, max_file_bytes, &cancelled)?;
                    }
                }
                Ok(())
            },
            cancelled,
        );
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                check_cancel(&cancelled)?;
                tracing::debug!(%error, "callable predecessor could not be reused");
            }
        }
    }
    // A semantic-only republish may run after every source file was removed.
    // Legacy cores without this optional artifact acquire it on normal indexing.
    if publication.mode == codestory_store::IndexPublicationMode::SemanticProjection {
        return Ok(());
    }
    if let Err(error) = callable_index::write(
        &artifact,
        &publication.generation_id,
        &publication.run_id,
        None,
        |writer| populate(writer, store, root, max_file_bytes, &cancelled),
        cancelled,
    ) {
        check_cancel(&cancelled)?;
        tracing::warn!(%error, "callable index was not published");
    }
    Ok(())
}

pub(super) fn generation_has_index(
    search_path: &Path,
    publication: &IndexPublicationRecord,
) -> bool {
    open_generation_index(search_path, publication).is_ok()
}

fn populate(
    writer: &mut Writer<'_>,
    store: &Store,
    root: &Path,
    cap: u64,
    cancelled: &impl Fn() -> bool,
) -> Result<()> {
    let mut after = None;
    loop {
        let batch = store.callable_definition_files_after(after)?;
        if batch.is_empty() {
            return Ok(());
        }
        after = batch.last().map(|file| file.id);
        for file in batch {
            populate_file(writer, store, root, file, cap, cancelled)?;
        }
    }
}

fn populate_file(
    writer: &mut Writer<'_>,
    store: &Store,
    root: &Path,
    file: CallableDefinitionFile,
    cap: u64,
    cancelled: &impl Fn() -> bool,
) -> Result<()> {
    if cancelled() {
        bail!("callable index build cancelled");
    }
    let partial = !file.complete;
    if !file.indexed
        || (partial
            && is_structural_candidate_path(&file.path)
            && store
                .get_file_coverage_reasons(file.id)?
                .contains(&FileCoverageReason::Binary))
    {
        return writer.file_status(file.id, partial, true, 0);
    }
    let definitions = store.callable_definitions_for_file(file.id)?;
    if definitions.is_empty() {
        return writer.file_status(file.id, partial, false, 0);
    }
    let (Some(hash), Some((absolute, relative))) = (file.content_hash, contained(root, &file.path))
    else {
        return writer.file_status(file.id, partial, true, 0);
    };
    let BoundedTextRead::Contents { contents, .. } = read_file_text_limited(&absolute, cap) else {
        return writer.file_status(file.id, partial, true, 0);
    };
    if hex_hash(&contents) != hash
        || (is_structural_candidate_path(&file.path)
            && !is_structural_source_text(contents.as_bytes()))
    {
        return writer.file_status(file.id, partial, true, 0);
    }
    let offsets = line_offsets(&contents);
    let mut unique = BTreeMap::new();
    let mut invalid = 0;
    for definition in &definitions {
        let Some((first, last, end_line)) = extent(&contents, &offsets, definition) else {
            invalid += 1;
            continue;
        };
        if u32::try_from(last).is_err() {
            invalid += 1;
            continue;
        }
        let key = (definition.start_line, end_line);
        let current = unique.entry(key).or_insert((first, last, definition));
        if last - first > current.1 - current.0 {
            *current = (first, last, definition);
        }
    }
    writer.file_status(file.id, partial, false, invalid)?;
    if unique.is_empty() {
        return Ok(());
    }
    writer.file(file.id, &relative, &hash, contents.len())?;
    for ((start_line, end_line), (first, last, definition)) in unique {
        if cancelled() {
            bail!("callable index build cancelled");
        }
        let body = &contents[first..last];
        writer.definition(
            Definition {
                node_id: definition.node_id,
                file_id: file.id,
                name: &definition.name,
                start_line,
                end_line,
                start_byte: first as u32,
                end_byte: last as u32,
                body_sha256: &hex_hash(body),
            },
            &relative,
            body,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppController;
    use codestory_contracts::api::IndexMode;
    use std::fs;

    fn current_query(
        storage_path: &Path,
        root: &Path,
        question: &str,
        in_scope: impl Fn(&str) -> bool,
    ) -> CallableSearchOutcome {
        let store = Store::open_observational(storage_path).unwrap();
        let snapshot = store.read_snapshot().unwrap();
        query(
            snapshot.storage(),
            storage_path,
            root,
            question,
            1,
            in_scope,
            || false,
        )
        .unwrap()
    }

    #[test]
    fn callable_search_uses_exact_source_and_scopes_before_limit() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let storage = cache.path().join("codestory.db");
        let primary = root.join("lib.rs");
        let source =
            "// café\npub fn compute() -> &'static str {\n    \"amberSignal envelope lookup\"\n}\n";
        fs::write(&primary, source).unwrap();
        fs::write(
            root.join("secondary.rs"),
            "pub fn alternate() -> &'static str { \"amberSignal\" }\n",
        )
        .unwrap();
        let controller = AppController::new_with_config(crate::test_sidecar_runtime_from_env());
        controller
            .open_project_summary_with_storage_path(root.to_path_buf(), storage.clone())
            .unwrap();
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .unwrap();
        let original = Store::database_index_publication(&storage)
            .unwrap()
            .unwrap();
        let question = "How is the amberSignal envelope lookup performed?";
        let first = current_query(&storage, root, question, |_| true);
        assert_eq!(first.hits.len(), 1);
        let hit = &first.hits[0];
        assert_eq!(hit.path, "lib.rs");
        assert_eq!((hit.start_line, hit.end_line), (2, 4));
        assert_eq!(hit.file_sha256, hex_hash(source));
        assert_eq!(
            hit.body_sha256,
            hex_hash(&source[hit.start_byte as usize..hit.end_byte as usize])
        );
        let scoped = current_query(&storage, root, question, |path| path == "secondary.rs");
        assert_eq!(scoped.hits[0].path, "secondary.rs");
        assert!(!scoped.refresh_recommended);
        // No definition shape or source range changes with this edit.
        fs::write(&primary, source.replace("amberSignal", "blackSignal")).unwrap();
        let stale = current_query(&storage, root, question, |path| path == "lib.rs");
        assert!(stale.hits.is_empty());
        assert_eq!(stale.stale_candidates, 1);
        assert!(stale.refresh_recommended);
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
            .unwrap();
        let current = Store::database_index_publication(&storage)
            .unwrap()
            .unwrap();
        assert_ne!(current.generation_id, original.generation_id);
        let updated = current_query(&storage, root, "blackSignal envelope", |path| {
            path == "lib.rs"
        });
        assert_eq!(updated.hits[0].node_id, hit.node_id);
        assert_ne!(updated.hits[0].file_sha256, hit.file_sha256);
        assert!(!updated.refresh_recommended);
    }

    #[test]
    fn callable_search_repair_preserves_full_core_and_prior_search_generation() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let storage = cache.path().join("codestory.db");
        fs::write(root.join("lib.rs"), "/// Check quota and return the saved checksum.\npub fn report_checksum(value: usize) -> usize {\n    value.wrapping_add(7)\n}\n").unwrap();
        let controller = AppController::new_with_config(crate::test_sidecar_runtime_from_env());
        controller
            .open_project_summary_with_storage_path(root.to_path_buf(), storage.clone())
            .unwrap();
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .unwrap();
        let publication = Store::database_index_publication(&storage)
            .unwrap()
            .unwrap();
        let core_path = codestory_store::resolve_core_database_path(&storage).unwrap();
        let core_bytes = fs::read(&core_path).unwrap();
        let counts = |storage: &Path| {
            let store = Store::open_observational(storage).unwrap();
            let connection = store.get_connection();
            ["symbol_search_doc", "dense_anchor_input"].map(|table| {
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap()
            })
        };
        let original_counts = counts(&storage);
        assert!(
            original_counts.iter().all(|n| *n > 0),
            "{original_counts:?}"
        );
        let path = search_index_path_for_publication(&storage, Some(&publication)).unwrap();
        let primary = path.join(CALLABLE_INDEX_FILE);
        let overlay = callable_overlay_path(&path, &publication).unwrap();
        let saved = cache.path().join("artifact-backup");
        fs::rename(&primary, &saved).unwrap();
        let missing = current_query(&storage, root, "report checksum", |_| true);
        assert_eq!(missing.coverage, CallableCoverage::Missing);
        assert!(missing.refresh_recommended && !primary.exists() && !overlay.exists());

        let refresh = || {
            controller
                .run_indexing_blocking_without_runtime_refresh_with_options(
                    IndexMode::Incremental,
                    true,
                )
                .unwrap()
        };
        let original_marker =
            fs::read(crate::search_publication::search_generation_completion_path(&path)).unwrap();
        let repaired = refresh();
        let probe = repaired.incremental_plan_probe.unwrap();
        assert_eq!(
            probe.outcome,
            codestory_contracts::api::IncrementalPlanProbeOutcomeDto::SearchGenerationIncomplete
        );
        assert!(probe.skipped_search_state_rebuild);
        assert!(overlay.is_file() && !primary.exists());
        assert_eq!(
            fs::read(crate::search_publication::search_generation_completion_path(&path)).unwrap(),
            original_marker
        );
        assert_eq!(
            current_query(&storage, root, "report checksum", |_| true)
                .hits
                .len(),
            1
        );
        let unchanged = refresh();
        assert_eq!(
            unchanged.incremental_plan_probe.unwrap().outcome,
            codestory_contracts::api::IncrementalPlanProbeOutcomeDto::ShortCircuited
        );

        fs::rename(&overlay, cache.path().join("overlay-backup")).unwrap();
        fs::write(&primary, fs::read(saved).unwrap()).unwrap();
        rusqlite::Connection::open(&primary)
            .unwrap()
            .execute_batch("DROP TABLE posting")
            .unwrap();
        let damaged_bytes = fs::read(&primary).unwrap();
        let damaged = current_query(&storage, root, "report checksum", |_| true);
        assert_eq!(damaged.coverage, CallableCoverage::Unavailable);
        assert!(damaged.refresh_recommended && damaged.hits.is_empty());

        assert!(
            !repair_pinned_generation(&storage, root, &publication, 2_000_000, None, || false)
                .unwrap()
        );
        assert!(
            !overlay.exists(),
            "failed source revalidation must keep the staged file invisible"
        );
        fs::create_dir(&overlay).unwrap();
        fs::write(overlay.join("marker"), b"outside overlay ownership").unwrap();
        assert_eq!(
            refresh().incremental_plan_probe.unwrap().outcome,
            codestory_contracts::api::IncrementalPlanProbeOutcomeDto::ShortCircuited
        );
        assert_eq!(
            fs::read(overlay.join("marker")).unwrap(),
            b"outside overlay ownership"
        );
        fs::remove_file(overlay.join("marker")).unwrap();
        fs::remove_dir(&overlay).unwrap();

        fs::write(&overlay, b"foreign regular destination").unwrap();
        assert_eq!(
            refresh().incremental_plan_probe.unwrap().outcome,
            codestory_contracts::api::IncrementalPlanProbeOutcomeDto::ShortCircuited,
        );
        assert_eq!(fs::read(&overlay).unwrap(), b"foreign regular destination");
        fs::remove_file(&overlay).unwrap();

        refresh();
        // A valid generation/run header proves ownership even if postings are damaged.
        let original_overlay = cache.path().join("owned-overlay-backup");
        fs::rename(&overlay, &original_overlay).unwrap();
        fs::write(&overlay, fs::read(original_overlay).unwrap()).unwrap();
        rusqlite::Connection::open(&overlay)
            .unwrap()
            .execute_batch("DROP TABLE posting")
            .unwrap();
        let mut permissions = fs::metadata(&overlay).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&overlay, permissions).unwrap();
        assert!(callable_index::has_generation_binding(
            &overlay,
            &publication.generation_id,
            &publication.run_id
        ));
        assert!(current_query(&storage, root, "report checksum", |_| true).refresh_recommended);

        refresh();
        assert_eq!(
            fs::read(&primary).unwrap(),
            damaged_bytes,
            "never rewrite the original search generation"
        );
        let repaired = current_query(&storage, root, "report checksum", |_| true);
        assert_eq!(repaired.hits.len(), 1);
        assert!(!repaired.refresh_recommended);
        assert_eq!(
            Store::database_index_publication(&storage)
                .unwrap()
                .unwrap(),
            publication
        );
        assert_eq!(counts(&storage), original_counts);
        assert_eq!(fs::read(&core_path).unwrap(), core_bytes);
    }
}
