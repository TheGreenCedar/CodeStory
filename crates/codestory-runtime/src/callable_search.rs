//! Core-only ranked callable locations. Ranking never walks the project tree.
use crate::search_publication::{
    SearchGenerationCatalogGuard, read_search_generation_completion,
    search_index_path_for_publication,
};
use crate::support::{BoundedTextRead, read_file_text_limited};
use anyhow::{Result, bail};
use codestory_contracts::api::ApiError;
use codestory_indexer::CancellationToken;
use codestory_retrieval::callable_index::{
    self, CALLABLE_INDEX_FILE, Definition, PreviousGeneration, Writer,
};
use codestory_store::{CallableDefinition, CallableDefinitionFile, IndexPublicationRecord, Store};
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
    let artifact = path.join(CALLABLE_INDEX_FILE);
    if std::fs::symlink_metadata(&artifact).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(CallableSearchOutcome::unavailable(
            CallableCoverage::Missing,
        ));
    }
    let ranking_limit = limit.min(MAX_RETURNED_HITS);
    let read = callable_index::open(&artifact, &publication.generation_id, &publication.run_id)
        .and_then(|reader| {
            let ranked = reader.rank(
                question,
                ranking_limit.saturating_mul(4),
                in_scope,
                &cancelled,
            )?;
            Ok((reader.coverage, ranked))
        });
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
    if bytes_read != hit.file_bytes || hex_hash(&contents) != hash {
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
        let source = search_index_path_for_publication(storage_path, Some(previous))?
            .join(CALLABLE_INDEX_FILE);
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
            &cancelled,
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
        &cancelled,
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
    callable_index::open(
        &search_path.join(CALLABLE_INDEX_FILE),
        &publication.generation_id,
        &publication.run_id,
    )
    .is_ok()
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
    if !file.indexed {
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
    if hex_hash(&contents) != hash {
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
    fn callable_search_missing_or_damaged_artifact_is_observational() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let storage = cache.path().join("codestory.db");
        fs::write(root.join("lib.rs"), "pub fn report_checksum() {}\n").unwrap();
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
        let path = search_index_path_for_publication(&storage, Some(&publication))
            .unwrap()
            .join(CALLABLE_INDEX_FILE);
        let saved = cache.path().join("artifact-backup");
        fs::rename(&path, &saved).unwrap();
        let missing = current_query(&storage, root, "report checksum", |_| true);
        assert!(missing.hits.is_empty());
        assert_eq!(missing.coverage, CallableCoverage::Missing);
        assert!(missing.refresh_recommended && !path.exists());
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
            .unwrap();
        let repaired = Store::database_index_publication(&storage)
            .unwrap()
            .unwrap();
        assert_ne!(repaired, publication);
        assert_eq!(
            current_query(&storage, root, "report checksum", |_| true)
                .hits
                .len(),
            1
        );
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
            .unwrap();
        assert_eq!(
            Store::database_index_publication(&storage)
                .unwrap()
                .unwrap(),
            repaired
        );

        let path = search_index_path_for_publication(&storage, Some(&repaired))
            .unwrap()
            .join(CALLABLE_INDEX_FILE);
        let backup = cache.path().join("artifact-new-backup");
        fs::rename(&path, &backup).unwrap();
        fs::write(&path, fs::read(backup).unwrap()).unwrap();
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("DROP TABLE posting")
            .unwrap();
        let damaged_bytes = fs::read(&path).unwrap();
        let damaged = current_query(&storage, root, "report checksum", |_| true);
        assert_eq!(damaged.coverage, CallableCoverage::Unavailable);
        assert!(damaged.refresh_recommended && damaged.hits.is_empty());
        assert_eq!(fs::read(&path).unwrap(), damaged_bytes);
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
            .unwrap();
        assert_eq!(
            current_query(&storage, root, "report checksum", |_| true)
                .hits
                .len(),
            1
        );
    }
}
