use crate::symbol_query::RetrievalFileRole;
use anyhow::{Context, Result, anyhow, bail};
use codestory_contracts::bounded_locks::{
    self, FileLockKind, LockDeadline, PUBLICATION_LOCK_WAIT, acquire_with_deadline,
};
use codestory_contracts::graph::NodeId;
use codestory_workspace::owned_deletion::OwnedDeletionRoot;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as NucleoConfig, Matcher, Utf32String};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
#[cfg(any(test, feature = "test-support"))]
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tantivy::collector::TopDocs;
use tantivy::doc;
use tantivy::query::QueryParser;
use tantivy::schema::{FAST, INDEXED, STORED, Schema, TEXT};
use tantivy::schema::{Field, Value};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument};

pub const EMBEDDING_DIM: usize = codestory_retrieval::RETRIEVAL_EMBEDDING_DIM;
const SEARCH_WRITER_HEAP_BYTES: usize = 20_000_000;
const EMBEDDING_PROFILE: &str = "coderank-embed";
const EMBEDDING_MODEL_ID: &str = "nomic-ai/CodeRankEmbed";
pub const SYMBOL_FULL_TEXT_INDEX_ENV: &str = "CODESTORY_SYMBOL_FULL_TEXT_INDEX";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SymbolCandidateRank {
    exact_display: u8,
    exact_terminal: u8,
    exact_leading: u8,
    fuzzy_score: u32,
}

#[derive(Debug, Clone)]
pub struct EmbeddingRuntimeAvailability {
    pub available: bool,
    pub model_id: Option<String>,
    pub fallback_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingProfileContract {
    pub profile: String,
    pub backend: String,
    pub model_id: String,
    pub cache_key: String,
    pub dimension: Option<u32>,
}

fn fixed_embedding_contract() -> EmbeddingProfileContract {
    EmbeddingProfileContract {
        profile: EMBEDDING_PROFILE.into(),
        backend: "per_user_server".into(),
        model_id: EMBEDDING_MODEL_ID.into(),
        cache_key: codestory_retrieval::embedding_runtime_id(),
        dimension: Some(EMBEDDING_DIM as u32),
    }
}

pub fn embedding_runtime_availability_from_env() -> EmbeddingRuntimeAvailability {
    let runtime = codestory_retrieval::SidecarRuntimeConfig::local();
    embedding_runtime_availability_from_config(&runtime)
}

pub fn embedding_runtime_availability_from_config(
    runtime: &codestory_retrieval::SidecarRuntimeConfig,
) -> EmbeddingRuntimeAvailability {
    let probe = codestory_retrieval::probe_product_embedding_runtime_for_runtime(runtime);
    EmbeddingRuntimeAvailability {
        available: probe.reachable,
        model_id: Some(codestory_retrieval::embedding_runtime_id()),
        fallback_message: (!probe.reachable).then_some(probe.detail),
    }
}

pub fn embedding_profile_contract_from_env() -> Result<EmbeddingProfileContract> {
    Ok(fixed_embedding_contract())
}

pub fn embedding_profile_contract_from_config(
    _config: &codestory_retrieval::EmbeddingRuntimeConfig,
) -> Result<EmbeddingProfileContract> {
    Ok(fixed_embedding_contract())
}

fn env_bool_override(key: &str) -> Option<bool> {
    std::env::var(key)
        .ok()
        .and_then(|raw| match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
}

fn symbol_full_text_index_enabled_from_env() -> bool {
    env_bool_override(SYMBOL_FULL_TEXT_INDEX_ENV).unwrap_or(true)
}

#[derive(Debug, Clone)]
pub struct LlmSearchDoc {
    pub node_id: NodeId,
    pub file_role: RetrievalFileRole,
    pub doc_text: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct EmbeddingRuntime {
    model_id: String,
    backend: EmbeddingBackend,
}

#[derive(Debug, Clone)]
enum EmbeddingBackend {
    #[cfg(not(any(test, feature = "test-support")))]
    PerUser(codestory_retrieval::PerUserEmbeddingClient),
    #[cfg(any(test, feature = "test-support"))]
    HashProjection,
}

impl EmbeddingRuntime {
    pub fn from_runtime(runtime: &codestory_retrieval::SidecarRuntimeConfig) -> Result<Self> {
        #[cfg(any(test, feature = "test-support"))]
        {
            let _ = runtime;
            Ok(Self::test_runtime())
        }
        #[cfg(not(any(test, feature = "test-support")))]
        {
            codestory_retrieval::ensure_product_embedding_backend_for_runtime(runtime)?;
            Ok(Self {
                model_id: codestory_retrieval::embedding_runtime_id(),
                backend: EmbeddingBackend::PerUser(
                    codestory_retrieval::PerUserEmbeddingClient::for_runtime(runtime)?,
                ),
            })
        }
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        if query.trim().is_empty() {
            bail!("query cannot be empty for semantic retrieval");
        }
        match &self.backend {
            #[cfg(not(any(test, feature = "test-support")))]
            EmbeddingBackend::PerUser(client) => client.embed_query(query),
            #[cfg(any(test, feature = "test-support"))]
            EmbeddingBackend::HashProjection => {
                Ok(embed_text_with_hash_projection(query, EMBEDDING_DIM))
            }
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn test_runtime() -> Self {
        Self {
            model_id: codestory_retrieval::embedding_runtime_id(),
            backend: EmbeddingBackend::HashProjection,
        }
    }
}

pub struct SearchEngine {
    symbols: Vec<(Utf32String, NodeId)>,
    index: Index,
    reader: IndexReader,
    llm_docs: HashMap<NodeId, LlmSearchDoc>,
    embedding_runtime: Option<EmbeddingRuntime>,
    full_text_index_enabled: bool,
    _persisted_index_guard: Option<PersistedSearchIndexGuard>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SymbolIndexWriteStats {
    pub docs_written: usize,
    pub writer_count: usize,
    pub commit_count: usize,
    pub reload_count: usize,
    pub commit_duration: Duration,
    pub reload_duration: Duration,
}

pub struct SymbolIndexSession<'a> {
    engine: &'a mut SearchEngine,
    writer: Option<IndexWriter<TantivyDocument>>,
    name_field: Field,
    id_field: Field,
    symbols_start_len: usize,
    docs_written: usize,
    failed: bool,
    finished: bool,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SymbolIndexTestFault {
    AddDocument,
    AddDocumentAfterOne,
    Commit,
}

#[cfg(test)]
thread_local! {
    static SYMBOL_INDEX_TEST_FAULT: std::cell::Cell<Option<SymbolIndexTestFault>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn arm_symbol_index_test_fault(fault: SymbolIndexTestFault) {
    SYMBOL_INDEX_TEST_FAULT.with(|slot| slot.set(Some(fault)));
}

#[cfg(test)]
fn take_symbol_index_test_fault(expected: SymbolIndexTestFault) -> bool {
    SYMBOL_INDEX_TEST_FAULT.with(|slot| {
        if expected == SymbolIndexTestFault::AddDocument
            && slot.get() == Some(SymbolIndexTestFault::AddDocumentAfterOne)
        {
            slot.set(Some(SymbolIndexTestFault::AddDocument));
            return false;
        }
        if slot.get() == Some(expected) {
            slot.set(None);
            true
        } else {
            false
        }
    })
}

struct PersistedSearchIndexGuard {
    file: File,
    path: PathBuf,
    mode: PersistedSearchIndexLockMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersistedSearchIndexLockMode {
    Shared,
    Exclusive,
}

#[derive(Debug)]
pub(crate) struct PersistedSearchIndexBusy {
    path: PathBuf,
    mode: PersistedSearchIndexLockMode,
}

impl std::fmt::Display for PersistedSearchIndexBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "search index {} is already locked in {:?} mode",
            self.path.display(),
            self.mode
        )
    }
}

impl std::error::Error for PersistedSearchIndexBusy {}

pub(crate) fn is_persisted_search_index_busy(error: &anyhow::Error) -> bool {
    error.downcast_ref::<PersistedSearchIndexBusy>().is_some()
}

impl PersistedSearchIndexGuard {
    fn acquire_shared(search_dir: &Path) -> Result<Self> {
        Self::acquire_with_mode(search_dir, PersistedSearchIndexLockMode::Shared)
    }

    fn acquire_with_mode(search_dir: &Path, mode: PersistedSearchIndexLockMode) -> Result<Self> {
        let lock_path = persisted_search_index_lock_path(search_dir);
        if let Some(parent) = lock_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create search index lock parent {}",
                    parent.display()
                )
            })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open search index lock {}", lock_path.display()))?;
        let kind = match mode {
            PersistedSearchIndexLockMode::Shared => FileLockKind::Shared,
            PersistedSearchIndexLockMode::Exclusive => FileLockKind::Exclusive,
        };
        // The exclusive side is held for a whole search index publication, so
        // both sides wait on the publication budget. The wait is interruptible
        // through the caller's ambient cancellation.
        acquire_with_deadline(
            &file,
            kind,
            LockDeadline::after(PUBLICATION_LOCK_WAIT),
            None,
        )
        .with_context(|| {
            format!(
                "Failed to take {kind} search index lock {}",
                search_dir.display()
            )
        })?;
        Ok(Self {
            file,
            path: lock_path,
            mode,
        })
    }

    #[cfg(test)]
    fn try_acquire_shared(search_dir: &Path) -> Result<Self> {
        Self::try_acquire_with_mode(search_dir, PersistedSearchIndexLockMode::Shared)
    }

    fn try_acquire_exclusive(search_dir: &Path) -> Result<Self> {
        Self::try_acquire_with_mode(search_dir, PersistedSearchIndexLockMode::Exclusive)
    }

    fn try_acquire_with_mode(
        search_dir: &Path,
        mode: PersistedSearchIndexLockMode,
    ) -> Result<Self> {
        let lock_path = persisted_search_index_lock_path(search_dir);
        if let Some(parent) = lock_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create search index lock parent {}",
                    parent.display()
                )
            })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Failed to open search index lock {}", lock_path.display()))?;
        let kind = match mode {
            PersistedSearchIndexLockMode::Shared => FileLockKind::Shared,
            PersistedSearchIndexLockMode::Exclusive => FileLockKind::Exclusive,
        };
        let acquired = bounded_locks::try_acquire(&file, kind).with_context(|| {
            format!(
                "Failed to take {kind} search index lock {}",
                search_dir.display()
            )
        })?;
        if !acquired {
            return Err(PersistedSearchIndexBusy {
                path: search_dir.to_path_buf(),
                mode,
            }
            .into());
        }
        Ok(Self {
            file,
            path: lock_path,
            mode,
        })
    }

    fn is_exclusive(&self) -> bool {
        self.mode == PersistedSearchIndexLockMode::Exclusive
    }

    fn downgrade_to_shared(&mut self) -> Result<()> {
        if !self.is_exclusive() {
            return Ok(());
        }
        bounded_locks::downgrade_to_shared(&self.file, LockDeadline::after(PUBLICATION_LOCK_WAIT))
            .with_context(|| {
                format!(
                    "Failed to downgrade persisted search index lock {} to shared",
                    self.path.display()
                )
            })?;
        self.mode = PersistedSearchIndexLockMode::Shared;
        Ok(())
    }
}

impl Drop for PersistedSearchIndexGuard {
    fn drop(&mut self) {
        if let Err(error) = bounded_locks::release(&self.file) {
            tracing::warn!(
                path = %self.path.display(),
                "Failed to unlock persisted search index lock: {error}"
            );
        }
    }
}

pub(crate) fn persisted_search_index_lock_path(search_dir: &Path) -> PathBuf {
    let mut path = search_dir.as_os_str().to_os_string();
    path.push(".lock");
    PathBuf::from(path)
}

impl SearchEngine {
    fn build_schema() -> Schema {
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("name", TEXT | STORED);
        schema_builder.add_i64_field("node_id", INDEXED | STORED | FAST);
        schema_builder.build()
    }

    fn new_with_index(
        index: Index,
        persisted_index_guard: Option<PersistedSearchIndexGuard>,
    ) -> Result<Self> {
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;

        Ok(Self {
            symbols: Vec::new(),
            index,
            reader,
            llm_docs: HashMap::new(),
            embedding_runtime: None,
            full_text_index_enabled: symbol_full_text_index_enabled_from_env(),
            _persisted_index_guard: persisted_index_guard,
        })
    }

    #[cfg(test)]
    pub(crate) fn symbols(&self) -> &[(Utf32String, NodeId)] {
        &self.symbols
    }

    pub fn new(storage_path: Option<&Path>) -> Result<Self> {
        let schema = Self::build_schema();
        let index = if let Some(path) = storage_path {
            let guard = PersistedSearchIndexGuard::try_acquire_exclusive(path)?;
            return Self::new_persisted_with_guard(path, guard);
        } else {
            Index::create_in_ram(schema)
        };
        Self::new_with_index(index, None)
    }

    #[allow(dead_code)]
    pub fn open_existing(path: &Path) -> Result<Self> {
        let guard = PersistedSearchIndexGuard::acquire_shared(path)?;
        Self::open_persisted_with_guard(path, guard)
    }

    pub(crate) fn downgrade_persisted_lock_to_shared(&mut self) -> Result<()> {
        if let Some(guard) = self._persisted_index_guard.as_mut() {
            guard.downgrade_to_shared()?;
        }
        Ok(())
    }

    fn new_persisted_with_guard(path: &Path, guard: PersistedSearchIndexGuard) -> Result<Self> {
        if !guard.is_exclusive() {
            bail!(
                "Recreating persisted search index {} requires an exclusive lock",
                path.display()
            );
        }
        recreate_search_storage_dir(path)?;
        let schema = Self::build_schema();
        let index = Index::create_in_dir(path, schema)
            .with_context(|| format!("Failed to create tantivy index at {}", path.display()))?;
        let mut engine = Self::new_with_index(index, Some(guard))?;
        engine.full_text_index_enabled = true;
        Ok(engine)
    }

    fn open_persisted_with_guard(path: &Path, guard: PersistedSearchIndexGuard) -> Result<Self> {
        let index = Index::open_in_dir(path)
            .with_context(|| format!("Failed to open tantivy index at {}", path.display()))?;
        let mut engine = Self::new_with_index(index, Some(guard)).with_context(|| {
            format!("Failed to initialize tantivy reader at {}", path.display())
        })?;
        engine.full_text_index_enabled = true;
        Ok(engine)
    }

    pub fn set_embedding_runtime_for_runtime(
        &mut self,
        runtime: &codestory_retrieval::SidecarRuntimeConfig,
    ) -> Result<()> {
        self.embedding_runtime = Some(EmbeddingRuntime::from_runtime(runtime)?);
        Ok(())
    }

    pub fn full_text_doc_count(&self) -> usize {
        if !self.full_text_index_enabled {
            return self.symbols.len();
        }
        self.tantivy_doc_count()
    }

    pub(crate) fn tantivy_doc_count(&self) -> usize {
        self.reader.searcher().num_docs() as usize
    }

    #[cfg(feature = "benchmark-support")]
    pub fn tantivy_segment_count(&self) -> usize {
        self.reader.searcher().segment_readers().len()
    }

    pub fn index_llm_symbol_docs(&mut self, docs: Vec<LlmSearchDoc>) {
        self.llm_docs.clear();
        for doc in docs {
            self.insert_llm_symbol_doc(doc);
        }
    }

    pub fn clear_llm_symbol_docs(&mut self) {
        self.llm_docs.clear();
    }

    pub fn extend_llm_symbol_docs<I>(&mut self, docs: I)
    where
        I: IntoIterator<Item = LlmSearchDoc>,
    {
        for doc in docs {
            self.insert_llm_symbol_doc(doc);
        }
    }

    fn insert_llm_symbol_doc(&mut self, doc: LlmSearchDoc) {
        let node_id = doc.node_id;
        self.llm_docs.insert(node_id, doc);
    }

    pub fn begin_symbol_index(&mut self) -> Result<SymbolIndexSession<'_>> {
        let writer = self
            .full_text_index_enabled
            .then(|| self.index.writer(SEARCH_WRITER_HEAP_BYTES))
            .transpose()?;
        let schema = self.index.schema();
        let name_field = schema.get_field("name")?;
        let id_field = schema.get_field("node_id")?;
        let symbols_start_len = self.symbols.len();
        Ok(SymbolIndexSession {
            engine: self,
            writer,
            name_field,
            id_field,
            symbols_start_len,
            docs_written: 0,
            failed: false,
            finished: false,
        })
    }

    #[cfg(any(test, feature = "benchmark-support"))]
    pub fn index_nodes(&mut self, nodes: Vec<(NodeId, String)>) -> Result<()> {
        let mut session = self.begin_symbol_index()?;
        session.add_nodes(nodes)?;
        session.finish()?;
        Ok(())
    }

    pub fn load_symbol_projection<I>(&mut self, symbols: I)
    where
        I: IntoIterator<Item = (NodeId, String)>,
    {
        self.symbols.clear();
        self.extend_symbol_projection(symbols);
    }

    pub fn extend_symbol_projection<I>(&mut self, symbols: I)
    where
        I: IntoIterator<Item = (NodeId, String)>,
    {
        self.symbols.extend(
            symbols
                .into_iter()
                .map(|(id, name)| (Utf32String::from(name.as_str()), id)),
        );
    }

    #[cfg(test)]
    pub fn search_symbol(&self, query: &str) -> Vec<NodeId> {
        if query.is_empty() {
            return Vec::new();
        }
        self.search_symbol_with_scores(query)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    pub fn search_symbol_with_scores(&self, query: &str) -> Vec<(NodeId, f32)> {
        search_symbols_with_scores(&self.symbols, query)
    }

    pub fn search_full_text(&self, query_str: &str) -> Result<Vec<NodeId>> {
        if query_str.is_empty() {
            return Ok(Vec::new());
        }
        if !self.full_text_index_enabled {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();
        let schema = self.index.schema();
        let name_field = schema.get_field("name")?;
        let id_field = schema.get_field("node_id")?;

        let query_parser = QueryParser::for_index(&self.index, vec![name_field]);
        let query = query_parser
            .parse_query(query_str)
            .context("Failed to parse tantivy query")?;

        let top_docs = searcher.search(&query, &TopDocs::with_limit(20).order_by_score())?;

        let mut results = Vec::new();
        let mut seen = HashSet::new();
        for (_score, doc_address) in top_docs {
            let retrieved_doc: TantivyDocument = searcher.doc(doc_address)?;
            if let Some(id_val) = retrieved_doc.get_first(id_field).and_then(|v| v.as_i64()) {
                let id = NodeId(id_val);
                if seen.insert(id) {
                    results.push(id);
                }
            }
        }

        Ok(results)
    }
}

impl SymbolIndexSession<'_> {
    pub fn add_nodes<I>(&mut self, nodes: I) -> Result<usize>
    where
        I: IntoIterator<Item = (NodeId, String)>,
    {
        if self.failed {
            bail!("symbol index session has failed");
        }
        let start_count = self.docs_written;
        for (id, name) in nodes {
            #[cfg(test)]
            if take_symbol_index_test_fault(SymbolIndexTestFault::AddDocument) {
                self.failed = true;
                bail!("injected symbol index add-document failure");
            }
            let fuzzy_name = Utf32String::from(name.as_str());
            if let Some(writer) = self.writer.as_mut()
                && let Err(add_error) = writer.add_document(doc!(
                    self.name_field => name,
                    self.id_field => id.0
                ))
            {
                self.failed = true;
                let mut failed_writer = self.writer.take().expect("failed writer is present");
                // add_document only reports a disconnected/killed pipeline.
                // Closing it and joining the workers recovers the original
                // I/O error. Drop already joins these workers on failure.
                let prepared = failed_writer.prepare_commit()?;
                // A rejected add must never publish even if all workers
                // unexpectedly completed successfully.
                prepared.abort().map_err(|abort_error| {
                    let message = format!(
                        "Failed to abort rejected symbol index write: {abort_error}; add-document error: {add_error}"
                    );
                    anyhow::Error::new(abort_error).context(message)
                })?;
                return Err(add_error.into());
            }
            self.engine.symbols.push((fuzzy_name, id));
            self.docs_written = self.docs_written.saturating_add(1);
        }
        Ok(self.docs_written.saturating_sub(start_count))
    }

    pub fn finish(mut self) -> Result<SymbolIndexWriteStats> {
        if self.failed {
            bail!("cannot finish a failed symbol index session");
        }
        let writer_count = usize::from(self.writer.is_some());
        let mut commit_count = 0;
        let mut reload_count = 0;
        let mut commit_duration = Duration::ZERO;
        let mut reload_duration = Duration::ZERO;
        if let Some(mut writer) = self.writer.take() {
            #[cfg(test)]
            if take_symbol_index_test_fault(SymbolIndexTestFault::Commit) {
                bail!("injected symbol index commit failure");
            }
            let commit_started = Instant::now();
            writer.commit()?;
            commit_duration = commit_started.elapsed();
            commit_count = 1;
            let reload_started = Instant::now();
            self.engine.reader.reload()?;
            reload_duration = reload_started.elapsed();
            reload_count = 1;
        }
        self.finished = true;
        Ok(SymbolIndexWriteStats {
            docs_written: self.docs_written,
            writer_count,
            commit_count,
            reload_count,
            commit_duration,
            reload_duration,
        })
    }
}

impl Drop for SymbolIndexSession<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.engine.symbols.truncate(self.symbols_start_len);
        }
    }
}

pub(crate) fn search_symbols_with_scores(
    symbols: &[(Utf32String, NodeId)],
    query: &str,
) -> Vec<(NodeId, f32)> {
    if query.is_empty() {
        return Vec::new();
    }
    if !crate::agent::nucleo_policy::nucleo_full_scan_enabled() {
        return Vec::new();
    }

    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );

    const SYMBOL_SCAN_CHUNK: usize = 256;
    let mut matches = if symbols.len() >= SYMBOL_SCAN_CHUNK {
        symbols
            .par_chunks(SYMBOL_SCAN_CHUNK)
            .flat_map(|chunk| {
                let mut matcher = Matcher::new(NucleoConfig::DEFAULT);
                chunk
                    .iter()
                    .filter_map(|(name, id)| {
                        pattern
                            .score(name.slice(..), &mut matcher)
                            .map(|score| (*id, score, symbol_candidate_rank(query, name, score)))
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    } else {
        let mut matcher = Matcher::new(NucleoConfig::DEFAULT);
        symbols
            .iter()
            .filter_map(|(name, id)| {
                pattern
                    .score(name.slice(..), &mut matcher)
                    .map(|score| (*id, score, symbol_candidate_rank(query, name, score)))
            })
            .collect::<Vec<_>>()
    };

    matches.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| right.1.cmp(&left.1)));

    let mut seen = HashSet::new();
    matches
        .into_iter()
        .map(|(id, score, _)| (id, score as f32))
        .filter(|(id, _)| seen.insert(*id))
        .take(200)
        .collect()
}

/// Rank canonical core symbols without opening a persisted search generation.
///
/// Exact core search uses this adapter after streaming the immutable core's
/// identity projection. Keeping UTF-32 conversion here preserves the same
/// matcher and ordering as the resident search engine without acquiring or
/// creating any search-generation artifact.
pub(crate) fn search_core_symbol_names_with_scores(
    symbols: &[(NodeId, String)],
    query: &str,
) -> Vec<(NodeId, f32)> {
    let symbols = symbols
        .iter()
        .map(|(id, name)| (Utf32String::from(name.as_str()), *id))
        .collect::<Vec<_>>();
    search_symbols_with_scores(&symbols, query)
}

fn symbol_candidate_rank(query: &str, name: &Utf32String, score: u32) -> SymbolCandidateRank {
    let query = query.trim().to_ascii_lowercase();
    let display = name.to_string();
    let display_lower = display.to_ascii_lowercase();
    let terminal_lower = display
        .rsplit([':', '.', '/', '\\'])
        .next()
        .unwrap_or(display.as_str())
        .to_ascii_lowercase();
    let leading_lower = display
        .split("::")
        .next()
        .unwrap_or(display.as_str())
        .to_ascii_lowercase();

    SymbolCandidateRank {
        exact_display: u8::from(display_lower == query),
        exact_terminal: u8::from(terminal_lower == query),
        exact_leading: u8::from(leading_lower == query),
        fuzzy_score: score,
    }
}

fn recreate_search_storage_dir(path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).with_context(|| {
        format!(
            "Failed to create search index parent directory {}",
            parent.display()
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        anyhow!(
            "Search index path has no owned relative name: {}",
            path.display()
        )
    })?;
    let deletion = OwnedDeletionRoot::open(parent).with_context(|| {
        format!(
            "Failed to open search index deletion root {}",
            parent.display()
        )
    })?;
    deletion
        .remove(Path::new(name))
        .with_context(|| format!("Failed to clear search index artifact {}", path.display()))?;
    std::fs::create_dir_all(path)
        .with_context(|| format!("Failed to create search index dir {}", path.display()))?;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
fn embed_text_with_hash_projection(text: &str, dim: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; dim];

    for token in text.split_whitespace() {
        let norm = token.trim().to_ascii_lowercase();
        if norm.is_empty() {
            continue;
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        norm.hash(&mut hasher);
        let hash = hasher.finish();
        let index = (hash as usize) % dim;
        let sign = if ((hash >> 8) & 1) == 0 { 1.0 } else { -1.0 };
        vector[index] += sign;

        // Add a tiny secondary feature for short-range context.
        let index2 = ((hash >> 17) as usize) % dim;
        vector[index2] += 0.5 * sign;
    }

    l2_normalize(&mut vector);
    vector
}

#[cfg(any(test, feature = "test-support"))]
fn l2_normalize(values: &mut [f32]) {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return;
    }
    for value in values {
        *value /= norm;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // Delegate real persistence and locking; only segment creation is refused.
    #[derive(Clone, Debug)]
    struct FailingSegmentDirectory {
        inner: tantivy::directory::MmapDirectory,
        fail_segments: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl tantivy::Directory for FailingSegmentDirectory {
        fn get_file_handle(
            &self,
            path: &Path,
        ) -> std::result::Result<
            std::sync::Arc<dyn tantivy::directory::FileHandle>,
            tantivy::directory::error::OpenReadError,
        > {
            self.inner.get_file_handle(path)
        }

        fn delete(
            &self,
            path: &Path,
        ) -> std::result::Result<(), tantivy::directory::error::DeleteError> {
            self.inner.delete(path)
        }

        fn exists(
            &self,
            path: &Path,
        ) -> std::result::Result<bool, tantivy::directory::error::OpenReadError> {
            self.inner.exists(path)
        }

        fn open_write(
            &self,
            path: &Path,
        ) -> std::result::Result<
            tantivy::directory::WritePtr,
            tantivy::directory::error::OpenWriteError,
        > {
            if path
                .extension()
                .is_some_and(|extension| extension == "store")
                && self.fail_segments.load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(tantivy::directory::error::OpenWriteError::wrap_io_error(
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "segment write refused",
                    ),
                    path.to_path_buf(),
                ));
            }
            self.inner.open_write(path)
        }

        fn atomic_read(
            &self,
            path: &Path,
        ) -> std::result::Result<Vec<u8>, tantivy::directory::error::OpenReadError> {
            self.inner.atomic_read(path)
        }

        fn atomic_write(&self, path: &Path, data: &[u8]) -> std::io::Result<()> {
            self.inner.atomic_write(path, data)
        }

        fn sync_directory(&self) -> std::io::Result<()> {
            self.inner.sync_directory()
        }

        fn watch(
            &self,
            callback: tantivy::directory::WatchCallback,
        ) -> tantivy::Result<tantivy::directory::WatchHandle> {
            self.inner.watch(callback)
        }
    }

    #[test]
    fn symbol_index_worker_failure_preserves_cause_and_previous_commit() -> Result<()> {
        let directory = tempdir()?;
        let fail_segments = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let index = Index::create(
            FailingSegmentDirectory {
                inner: tantivy::directory::MmapDirectory::open(directory.path())?,
                fail_segments: fail_segments.clone(),
            },
            SearchEngine::build_schema(),
            tantivy::IndexSettings::default(),
        )?;
        let mut engine = SearchEngine::new_with_index(index, None)?;
        // Select the real full-text path without mutating process configuration.
        engine.full_text_index_enabled = true;
        engine.index_nodes(vec![(NodeId(1), "PreviousCompleteSymbol".into())])?;
        let previous_meta = std::fs::read(directory.path().join("meta.json"))?;
        fail_segments.store(true, std::sync::atomic::Ordering::SeqCst);

        let error = {
            let mut session = engine.begin_symbol_index()?;
            // More than Tantivy's bounded document queue: the failed real worker
            // disconnects the producer without a sleep or scheduler assumption.
            let error = session
                .add_nodes((2..20_002).map(|id| (NodeId(id), "UnpublishedPrivateSymbol".into())))
                .expect_err("segment I/O failure must stop the producer");
            assert!(
                error.to_string().contains("segment write refused"),
                "original worker cause was lost: {error:#}"
            );
            assert!(!error.to_string().contains("UnpublishedPrivateSymbol"));
            assert!(
                session
                    .add_nodes(vec![(NodeId(30_000), "Later".into())])
                    .is_err()
            );
            session
                .finish()
                .expect_err("a failed session must never commit");
            error
        };
        assert!(matches!(
            error.downcast_ref::<tantivy::TantivyError>(),
            Some(tantivy::TantivyError::OpenWriteError(
                tantivy::directory::error::OpenWriteError::IoError { io_error, .. }
            )) if io_error.kind() == std::io::ErrorKind::PermissionDenied
        ));
        assert_eq!(engine.symbols().len(), 1);
        assert_eq!(
            engine.search_symbol("PreviousCompleteSymbol"),
            vec![NodeId(1)]
        );
        engine.reader.reload()?;
        assert_eq!(engine.tantivy_doc_count(), 1);
        assert_eq!(
            std::fs::read(directory.path().join("meta.json"))?,
            previous_meta
        );
        let reopened = SearchEngine::new_with_index(Index::open_in_dir(directory.path())?, None)?;
        assert_eq!(reopened.tantivy_doc_count(), 1);
        Ok(())
    }

    #[test]
    fn symbol_index_session_commits_and_reloads_once_across_windows() -> Result<()> {
        let directory = tempdir()?;
        let mut engine = SearchEngine::new(Some(directory.path()))?;
        let stats = {
            let mut session = engine.begin_symbol_index()?;
            assert_eq!(
                session.add_nodes(vec![(NodeId(1), "Alpha".to_string())])?,
                1
            );
            assert_eq!(session.add_nodes(vec![(NodeId(2), "Beta".to_string())])?, 1);
            session.finish()?
        };

        assert_eq!(stats.docs_written, 2);
        assert_eq!(stats.writer_count, 1);
        assert_eq!(stats.commit_count, 1);
        assert_eq!(stats.reload_count, 1);
        assert_eq!(engine.tantivy_doc_count(), 2);
        assert_eq!(engine.search_symbol("Alpha"), vec![NodeId(1)]);
        Ok(())
    }

    #[test]
    fn unfinished_symbol_index_session_restores_projection_without_committing() -> Result<()> {
        let directory = tempdir()?;
        let mut engine = SearchEngine::new(Some(directory.path()))?;
        {
            let mut session = engine.begin_symbol_index()?;
            session.add_nodes(vec![(NodeId(1), "Partial".to_string())])?;
        }

        assert!(engine.symbols().is_empty());
        assert_eq!(engine.tantivy_doc_count(), 0);
        Ok(())
    }

    #[test]
    fn symbol_index_add_failure_rolls_back_projection_and_uncommitted_docs() -> Result<()> {
        let directory = tempdir()?;
        let mut engine = SearchEngine::new(Some(directory.path()))?;
        let error = {
            let mut session = engine.begin_symbol_index()?;
            session.add_nodes(vec![(NodeId(1), "Accepted".to_string())])?;
            arm_symbol_index_test_fault(SymbolIndexTestFault::AddDocument);
            session
                .add_nodes(vec![(NodeId(2), "Rejected".to_string())])
                .expect_err("injected add-document failure")
        };

        assert!(error.to_string().contains("add-document failure"));
        assert!(engine.symbols().is_empty());
        assert_eq!(engine.tantivy_doc_count(), 0);
        Ok(())
    }

    #[test]
    fn symbol_index_commit_failure_rolls_back_projection_and_uncommitted_docs() -> Result<()> {
        let directory = tempdir()?;
        let mut engine = SearchEngine::new(Some(directory.path()))?;
        let error = {
            let mut session = engine.begin_symbol_index()?;
            session.add_nodes(vec![(NodeId(1), "Uncommitted".to_string())])?;
            arm_symbol_index_test_fault(SymbolIndexTestFault::Commit);
            session.finish().expect_err("injected commit failure")
        };

        assert!(error.to_string().contains("commit failure"));
        assert!(engine.symbols().is_empty());
        assert_eq!(engine.tantivy_doc_count(), 0);
        Ok(())
    }

    #[test]
    fn exact_symbol_search_prioritizes_exact_display_name_candidates() -> Result<()> {
        let mut engine = SearchEngine::new(None)?;

        engine.index_nodes(vec![
            (NodeId(1), "StorageAccess::~StorageAccess".to_string()),
            (NodeId(2), "StorageAccess::getFileContent".to_string()),
            (NodeId(3), "ComponentFactory::getStorageAccess".to_string()),
            (NodeId(4), "StorageAccess".to_string()),
        ])?;

        let results = engine.search_symbol("StorageAccess");

        assert_eq!(results.first(), Some(&NodeId(4)));
        Ok(())
    }

    #[test]
    /// `SearchEngine::new` reads `SYMBOL_FULL_TEXT_INDEX` during construction,
    /// so the flag must be true for the whole process: the parent re-executes
    /// this test in a child with the variable set, leaving the parent's
    /// environment untouched.
    fn symbol_full_text_index_can_be_disabled_for_projection_only_search() -> Result<()> {
        const CHILD_MARKER: &str = "CODESTORY_SEARCH_NO_FULL_TEXT_CHILD";
        if std::env::var_os(CHILD_MARKER).is_none() {
            let output = std::process::Command::new(
                std::env::current_exe().expect("test executable"),
            )
            .args([
                "--exact",
                "search::engine::tests::symbol_full_text_index_can_be_disabled_for_projection_only_search",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            .env(SYMBOL_FULL_TEXT_INDEX_ENV, "false")
            .output()
            .expect("run child with full-text indexing disabled");
            assert!(
                output.status.success(),
                "child stdout:\n{}\nchild stderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        }
        let mut engine = SearchEngine::new(None)?;

        engine.index_nodes(vec![
            (NodeId(1), "AlphaSymbol".to_string()),
            (NodeId(2), "BetaSymbol".to_string()),
        ])?;

        assert_eq!(engine.full_text_doc_count(), 2);
        assert_eq!(engine.search_symbol("Beta"), vec![NodeId(2)]);
        assert!(engine.search_full_text("betasymbol")?.is_empty());
        Ok(())
    }

    #[test]
    fn test_open_existing_reuses_persisted_index_without_recreating_dir() -> Result<()> {
        let dir = tempdir()?;
        let search_dir = dir.path().join("search");
        let marker = search_dir.join("keep.txt");

        let mut engine = SearchEngine::new(Some(search_dir.as_path()))?;
        engine.index_nodes(vec![
            (NodeId(1), "AlphaSymbol".to_string()),
            (NodeId(2), "BetaSymbol".to_string()),
        ])?;
        std::fs::write(&marker, "marker")?;
        drop(engine);

        let mut reopened = SearchEngine::open_existing(search_dir.as_path())?;
        reopened.load_symbol_projection(vec![
            (NodeId(1), "AlphaSymbol".to_string()),
            (NodeId(2), "BetaSymbol".to_string()),
        ]);

        assert!(
            marker.exists(),
            "opening an existing index should not recreate the dir"
        );
        assert_eq!(reopened.search_full_text("betasymbol")?, vec![NodeId(2)]);
        assert_eq!(reopened.search_symbol("Beta"), vec![NodeId(2)]);
        Ok(())
    }

    #[test]
    fn persisted_search_index_lock_is_sibling_and_held_by_engine() -> Result<()> {
        let dir = tempdir()?;
        let search_dir = dir.path().join("codestory.search");
        let lock_path = persisted_search_index_lock_path(&search_dir);

        let mut engine = SearchEngine::new(Some(search_dir.as_path()))?;
        assert!(search_dir.exists());
        assert!(lock_path.exists());
        let busy = match PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path()) {
            Ok(_) => panic!("new persisted engine should hold an exclusive search-index lock"),
            Err(error) => error,
        };
        assert!(is_persisted_search_index_busy(&busy));
        assert!(
            PersistedSearchIndexGuard::try_acquire_shared(search_dir.as_path()).is_err(),
            "exclusive search-index lock should block readers while rebuilding"
        );

        engine.index_nodes(vec![(NodeId(1), "Locked Symbol".to_string())])?;
        drop(engine);

        let _guard = PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path())?;
        assert!(
            lock_path.exists(),
            "recreating the search dir must not delete its sibling lock"
        );
        Ok(())
    }

    #[test]
    fn persisted_search_index_reader_blocks_writer_until_dropped() -> Result<()> {
        let dir = tempdir()?;
        let search_dir = dir.path().join("search");

        let mut engine = SearchEngine::new(Some(search_dir.as_path()))?;
        engine.index_nodes(vec![(NodeId(1), "Before Rebuild".to_string())])?;
        drop(engine);

        let existing = SearchEngine::open_existing(search_dir.as_path())?;
        {
            let _second_reader =
                PersistedSearchIndexGuard::try_acquire_shared(search_dir.as_path())?;
            assert!(
                PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path()).is_err(),
                "open_existing should hold a shared search-index lock"
            );
        }
        assert!(
            PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path()).is_err(),
            "open_existing should keep writers out while the reader is alive"
        );

        drop(existing);
        let mut rebuilt = SearchEngine::new(Some(search_dir.as_path()))?;
        assert!(
            PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path()).is_err(),
            "rebuild should keep an exclusive search-index lock while recreating the index"
        );
        assert!(
            PersistedSearchIndexGuard::try_acquire_shared(search_dir.as_path()).is_err(),
            "exclusive rebuild lock should block readers"
        );
        rebuilt.index_nodes(vec![(NodeId(2), "After Rebuild".to_string())])?;
        assert_eq!(rebuilt.search_full_text("after")?, vec![NodeId(2)]);
        drop(rebuilt);

        let _guard = PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path())?;
        Ok(())
    }

    #[test]
    fn persisted_search_index_missing_dir_rebuild_holds_exclusive_lock() -> Result<()> {
        let dir = tempdir()?;
        let search_dir = dir.path().join("missing.search");

        assert!(
            SearchEngine::open_existing(search_dir.as_path()).is_err(),
            "a missing persisted generation must not open as an index"
        );
        let mut engine = SearchEngine::new(Some(search_dir.as_path()))?;
        assert!(
            PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path()).is_err(),
            "rebuild after an open failure should keep an exclusive search-index lock"
        );
        assert!(
            PersistedSearchIndexGuard::try_acquire_shared(search_dir.as_path()).is_err(),
            "rebuild after an open failure should block readers until rebuilt"
        );

        engine.index_nodes(vec![(NodeId(1), "Recovered Symbol".to_string())])?;
        assert_eq!(engine.search_full_text("recovered")?, vec![NodeId(1)]);
        drop(engine);

        let _guard = PersistedSearchIndexGuard::try_acquire_exclusive(search_dir.as_path())?;
        Ok(())
    }
}
