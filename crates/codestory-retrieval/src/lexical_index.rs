//! Project-local SQLite FTS lexical index.

use anyhow::{Context, Result, bail};
use codestory_contracts::api::SearchTargetDto;
use codestory_contracts::owned_artifacts::sqlite_file_with_sidecars;
use codestory_contracts::validation_receipts::{
    ArtifactSeal, SealedReceiptCache, TransferableReceipt,
};
#[cfg(test)]
use codestory_store::FileRole;
use codestory_store::{SourcePolicyExclusionPolicyIdentity, Store, SymbolSearchDoc};
use codestory_workspace::paths::sqlite_open_path;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(not(test))]
use tracing::warn;

pub const LEXICAL_INDEX_VERSION: &str = "sqlite-fts5-v2";
pub const LEXICAL_INDEX_FILE: &str = "lexical-index.sqlite3";
const LEXICAL_COMPONENT_ENVELOPE_FILE: &str = "lexical-component-envelope.json";
const LEXICAL_COMPONENT_ENVELOPE_SCHEMA_VERSION: u32 = 1;
const LEXICAL_COMPONENT_SET_FILE: &str = "lexical-component-set.json";
const LEXICAL_COMPONENT_SET_SCHEMA_VERSION: u32 = 1;
const LEXICAL_STATE_FILE: &str = "lexical-state.sqlite3";
const LEXICAL_STATE_SCHEMA_VERSION: i32 = 2;
const LEXICAL_DELTA_FILE_PREFIX: &str = "lexical-delta-";
const LEXICAL_DELTA_COMPACTION_COUNT: usize = 8;
const LEXICAL_DELTA_COMPACTION_PERCENT: u64 = 10;
#[cfg(any(test, feature = "test-support"))]
const LEGACY_INDEX_FILE: &str = "lexical-index.jsonl";
#[cfg(any(test, feature = "test-support"))]
const LEGACY_META_FILE: &str = "shard-meta.json";
#[cfg(any(test, feature = "test-support"))]
const LEGACY_STUB_MARKER: &str = ".zoekt-stub";
/// Default lexical source-file cap for scans without a pinned core publication.
/// Product scans use the active cap recorded with that publication.
pub(crate) const MAX_FILE_BYTES: u64 = codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP;
const MAX_CANDIDATES: usize = 4_096;
const COVERAGE_PATH_SAMPLE: usize = 32;

/// How many published lexical generations may hold a sealed health receipt at
/// once.
///
/// A runtime works on one project generation at a time and keeps at most the
/// outgoing one alive beside it, so the live cardinality is a handful; the
/// bound exists to make the memory ceiling hard, not to force turnover. A
/// receipt is one metadata row, so the whole cache at capacity is tens of
/// kilobytes. Reaching the bound clears the cache, which costs a re-scan and
/// never changes a verdict.
const LEXICAL_SHARD_RECEIPT_CAPACITY: usize = 256;

/// Sealed deep-verification receipts for immutable lexical shards.
///
/// The receipt records what a shard *is* — its self-consistent metadata row —
/// after a full integrity pass. It never records whether that shard satisfies
/// a particular caller's expectations; those comparisons are cheap and re-run
/// on every probe. The seal covers the shard database and every SQLite sidecar
/// identity the registry owns, so an in-place rewrite, a replacement, or a
/// stray write-ahead log invalidates the receipt instead of hiding behind it.
static LEXICAL_SHARD_RECEIPTS: SealedReceiptCache<PathBuf, LexicalShardMetadata> =
    SealedReceiptCache::new(LEXICAL_SHARD_RECEIPT_CAPACITY);

/// The state database is much smaller than the FTS component and exists only
/// to compute bounded deltas without scanning the immutable lexical base.
static LEXICAL_STATE_RECEIPTS: SealedReceiptCache<PathBuf, Arc<LexicalLogicalState>> =
    SealedReceiptCache::new(LEXICAL_SHARD_RECEIPT_CAPACITY);
static LEXICAL_COMPONENT_SET_RECEIPTS: SealedReceiptCache<PathBuf, Arc<LexicalLogicalState>> =
    SealedReceiptCache::new(LEXICAL_SHARD_RECEIPT_CAPACITY);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LexicalCoverage {
    pub discovered_files: u32,
    pub indexed_files: u32,
    pub omitted_oversized: u32,
    pub unreadable_files: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted_path_sample: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable_path_sample: Vec<String>,
}

impl LexicalCoverage {
    pub fn complete(&self) -> bool {
        self.omitted_oversized == 0 && self.unreadable_files == 0
    }

    pub fn detail(&self) -> String {
        let mut detail = format!(
            "sqlite fts5; discovered={} indexed={} omitted_oversized={} unreadable={}",
            self.discovered_files,
            self.indexed_files,
            self.omitted_oversized,
            self.unreadable_files
        );
        if !self.omitted_path_sample.is_empty() {
            detail.push_str(&format!(
                "; omitted_path_sample={}",
                self.omitted_path_sample.join(",")
            ));
        }
        if !self.unreadable_path_sample.is_empty() {
            detail.push_str(&format!(
                "; unreadable_path_sample={}",
                self.unreadable_path_sample.join(",")
            ));
        }
        detail
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalInputFingerprint {
    pub file_count: u32,
    pub hash: String,
    pub coverage: LexicalCoverage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LexicalDocumentSource {
    #[default]
    LexicalSource,
    SymbolDoc,
    ComponentReport,
}

impl LexicalDocumentSource {
    pub(crate) fn provenance_label(self) -> &'static str {
        match self {
            Self::LexicalSource => "lexical_source",
            Self::SymbolDoc => "symbol_doc",
            Self::ComponentReport => "component_report",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "lexical_source" => Ok(Self::LexicalSource),
            "symbol_doc" => Ok(Self::SymbolDoc),
            "component_report" => Ok(Self::ComponentReport),
            _ => bail!("lexical shard contains unknown document source `{value}`"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LexicalDocument {
    path: String,
    content: String,
    source: LexicalDocumentSource,
    node_id: Option<String>,
    symbol_name: Option<String>,
    start_line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LexicalShardMetadata {
    project_id: String,
    sidecar_input_hash: String,
    lexical_hash: String,
    file_count: u32,
    coverage: LexicalCoverage,
    binding_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LexicalComponentDescriptor {
    file_name: String,
    metadata: LexicalShardMetadata,
    bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LexicalDeltaDescriptor {
    ordinal: u32,
    component: LexicalComponentDescriptor,
    upsert_keys: Vec<String>,
    tombstone_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LexicalComponentSet {
    schema_version: u32,
    generation: String,
    sidecar_input_hash: String,
    lexical_hash: String,
    file_count: u32,
    coverage: LexicalCoverage,
    base: LexicalComponentDescriptor,
    deltas: Vec<LexicalDeltaDescriptor>,
    state_file: String,
    state_sha256: String,
    binding_sha256: String,
}

#[derive(Debug, Clone)]
struct LexicalLogicalState {
    fingerprint: LexicalInputFingerprint,
    documents: BTreeMap<String, String>,
    state_sha256: String,
}

#[derive(Debug, Clone)]
struct LexicalStateDelta<'a> {
    retained: u64,
    upserts: Vec<(String, &'a LexicalDocument)>,
    tombstones: Vec<String>,
}

#[derive(Debug, Clone)]
struct LexicalStateDatabaseMetadata {
    fingerprint: LexicalInputFingerprint,
    state_sha256: String,
    binding_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LexicalComponentEnvelope {
    schema_version: u32,
    generation: String,
    sidecar_input_hash: String,
    physical_project_id: String,
    physical_input_hash: String,
    lexical_hash: String,
    file_count: u32,
    coverage: LexicalCoverage,
    binding_sha256: String,
}

impl LexicalComponentEnvelope {
    fn new(generation: &str, sidecar_input_hash: &str, physical: &LexicalShardMetadata) -> Self {
        let mut envelope = Self {
            schema_version: LEXICAL_COMPONENT_ENVELOPE_SCHEMA_VERSION,
            generation: generation.to_string(),
            sidecar_input_hash: sidecar_input_hash.to_string(),
            physical_project_id: physical.project_id.clone(),
            physical_input_hash: physical.sidecar_input_hash.clone(),
            lexical_hash: physical.lexical_hash.clone(),
            file_count: physical.file_count,
            coverage: physical.coverage.clone(),
            binding_sha256: String::new(),
        };
        envelope.binding_sha256 = lexical_component_envelope_binding(&envelope);
        envelope
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != LEXICAL_COMPONENT_ENVELOPE_SCHEMA_VERSION
            || self.generation.trim().is_empty()
            || self.sidecar_input_hash.trim().is_empty()
            || self.physical_project_id.trim().is_empty()
            || self.physical_input_hash.trim().is_empty()
            || self.lexical_hash.len() != 64
            || !self
                .lexical_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.binding_sha256 != lexical_component_envelope_binding(self)
        {
            bail!("lexical component envelope is invalid");
        }
        Ok(())
    }

    fn matches_physical(&self, physical: &LexicalShardMetadata) -> bool {
        self.physical_project_id == physical.project_id
            && self.physical_input_hash == physical.sidecar_input_hash
            && self.lexical_hash == physical.lexical_hash
            && self.file_count == physical.file_count
            && self.coverage == physical.coverage
    }
}

impl LexicalComponentSet {
    fn new(
        generation: &str,
        sidecar_input_hash: &str,
        fingerprint: &LexicalInputFingerprint,
        base: LexicalComponentDescriptor,
        deltas: Vec<LexicalDeltaDescriptor>,
        state_sha256: String,
    ) -> Self {
        let mut component_set = Self {
            schema_version: LEXICAL_COMPONENT_SET_SCHEMA_VERSION,
            generation: generation.to_string(),
            sidecar_input_hash: sidecar_input_hash.to_string(),
            lexical_hash: fingerprint.hash.clone(),
            file_count: fingerprint.file_count,
            coverage: fingerprint.coverage.clone(),
            base,
            deltas,
            state_file: LEXICAL_STATE_FILE.to_string(),
            state_sha256,
            binding_sha256: String::new(),
        };
        component_set.binding_sha256 = lexical_component_set_binding(&component_set);
        component_set
    }

    fn fingerprint(&self) -> LexicalInputFingerprint {
        LexicalInputFingerprint {
            file_count: self.file_count,
            hash: self.lexical_hash.clone(),
            coverage: self.coverage.clone(),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.schema_version != LEXICAL_COMPONENT_SET_SCHEMA_VERSION
            || self.generation.trim().is_empty()
            || self.sidecar_input_hash.trim().is_empty()
            || self.lexical_hash.len() != 64
            || self.state_sha256.len() != 64
            || self.binding_sha256 != lexical_component_set_binding(self)
        {
            bail!("lexical component-set manifest is invalid");
        }
        validate_lexical_component_file_name(&self.state_file)?;
        validate_lexical_component_descriptor(&self.base)?;
        let mut files = HashSet::from([self.base.file_name.as_str(), self.state_file.as_str()]);
        let mut ordinals = HashSet::new();
        for delta in &self.deltas {
            validate_lexical_component_descriptor(&delta.component)?;
            if !files.insert(delta.component.file_name.as_str())
                || !ordinals.insert(delta.ordinal)
                || delta.ordinal == 0
            {
                bail!("lexical component-set contains duplicate component identity");
            }
            let mut delta_keys = HashSet::new();
            for key in delta.upsert_keys.iter().chain(&delta.tombstone_keys) {
                if key.is_empty() || !delta_keys.insert(key) {
                    bail!("lexical delta contains an empty or duplicate document key");
                }
            }
            if delta.upsert_keys.len() as u32 != delta.component.metadata.file_count {
                bail!("lexical delta upsert count does not match its component");
            }
        }
        if self
            .deltas
            .windows(2)
            .any(|pair| pair[0].ordinal >= pair[1].ordinal)
        {
            bail!("lexical deltas are not strictly ordered");
        }
        Ok(())
    }
}

fn lexical_component_descriptor(
    path: &Path,
    metadata: LexicalShardMetadata,
) -> Result<LexicalComponentDescriptor> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("lexical component path has no UTF-8 file name")?
        .to_string();
    let bytes = std::fs::metadata(path)
        .with_context(|| format!("inspect lexical component {}", path.display()))?
        .len();
    let descriptor = LexicalComponentDescriptor {
        file_name,
        metadata,
        bytes,
    };
    validate_lexical_component_descriptor(&descriptor)?;
    Ok(descriptor)
}

fn read_lexical_component_set(
    shard_dir: &Path,
    expected_generation: Option<&str>,
    expected_sidecar_input_hash: Option<&str>,
) -> Result<Option<LexicalComponentSet>> {
    let path = shard_dir.join(LEXICAL_COMPONENT_SET_FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect lexical component set {}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("lexical component-set manifest is not a regular file");
    }
    let component_set: LexicalComponentSet = serde_json::from_slice(
        &std::fs::read(&path)
            .with_context(|| format!("read lexical component set {}", path.display()))?,
    )
    .with_context(|| format!("parse lexical component set {}", path.display()))?;
    component_set.validate()?;
    if expected_generation.is_some_and(|expected| component_set.generation != expected)
        || expected_sidecar_input_hash
            .is_some_and(|expected| component_set.sidecar_input_hash != expected)
    {
        bail!("lexical component set does not match the retrieval publication");
    }
    Ok(Some(component_set))
}

fn publish_lexical_component_set(
    shard_dir: &Path,
    component_set: &LexicalComponentSet,
) -> Result<()> {
    component_set.validate()?;
    let path = shard_dir.join(LEXICAL_COMPONENT_SET_FILE);
    let bytes = serde_json::to_vec_pretty(component_set)?;
    codestory_workspace::atomic_file::write_file_atomic(
        &path,
        "lexical-component-set",
        |file| {
            use std::io::Write;
            file.write_all(&bytes)?;
            Ok(())
        },
        |temp_path| {
            let observed: LexicalComponentSet = serde_json::from_slice(&std::fs::read(temp_path)?)?;
            observed.validate()?;
            if &observed != component_set {
                bail!("staged lexical component set changed before publication");
            }
            Ok(())
        },
    )
}

fn validate_component_descriptor_at(
    shard_dir: &Path,
    descriptor: &LexicalComponentDescriptor,
) -> Result<()> {
    let path = shard_dir.join(&descriptor.file_name);
    let metadata = LEXICAL_SHARD_RECEIPTS.validate_sealed(
        path.clone(),
        &sqlite_file_with_sidecars(&path),
        || verify_lexical_database_contents(&path),
    )?;
    let bytes = std::fs::metadata(&path)
        .with_context(|| format!("inspect lexical component {}", path.display()))?
        .len();
    if metadata != descriptor.metadata || bytes != descriptor.bytes {
        bail!("lexical component does not match its component-set descriptor");
    }
    Ok(())
}

fn validate_lexical_component_set_files(
    shard_dir: &Path,
    component_set: &LexicalComponentSet,
) -> Result<Arc<LexicalLogicalState>> {
    let artifacts = lexical_component_set_artifacts(shard_dir, component_set);
    LEXICAL_COMPONENT_SET_RECEIPTS.validate_sealed(
        shard_dir.join(LEXICAL_COMPONENT_SET_FILE),
        &artifacts,
        || verify_lexical_component_set_files(shard_dir, component_set),
    )
}

fn lexical_component_set_artifacts(
    shard_dir: &Path,
    component_set: &LexicalComponentSet,
) -> Vec<PathBuf> {
    let mut artifacts = vec![shard_dir.join(LEXICAL_COMPONENT_SET_FILE)];
    for descriptor in std::iter::once(&component_set.base)
        .chain(component_set.deltas.iter().map(|delta| &delta.component))
    {
        artifacts.extend(sqlite_file_with_sidecars(
            &shard_dir.join(&descriptor.file_name),
        ));
    }
    artifacts.extend(sqlite_file_with_sidecars(
        &shard_dir.join(&component_set.state_file),
    ));
    artifacts
}

fn verify_lexical_component_set_files(
    shard_dir: &Path,
    component_set: &LexicalComponentSet,
) -> Result<Arc<LexicalLogicalState>> {
    validate_component_descriptor_at(shard_dir, &component_set.base)?;
    let mut logical =
        read_lexical_component_document_state(&shard_dir.join(&component_set.base.file_name))?;
    for delta in &component_set.deltas {
        validate_component_descriptor_at(shard_dir, &delta.component)?;
        let upserts =
            read_lexical_component_document_state(&shard_dir.join(&delta.component.file_name))?;
        if upserts.keys().cloned().collect::<BTreeSet<_>>()
            != delta.upsert_keys.iter().cloned().collect::<BTreeSet<_>>()
        {
            bail!("lexical delta keys do not match its component rows");
        }
        for key in &delta.tombstone_keys {
            logical.remove(key);
        }
        logical.extend(upserts);
    }
    let state = load_lexical_state_database(
        &shard_dir.join(&component_set.state_file),
        &component_set.fingerprint(),
        &component_set.state_sha256,
    )?;
    if logical != state.documents {
        bail!("lexical component chain does not reproduce its logical state");
    }
    Ok(state)
}

fn read_lexical_component_document_state(path: &Path) -> Result<BTreeMap<String, String>> {
    let connection = open_read_only(path)?;
    let mut statement = connection.prepare(
        "SELECT document_key, document_hash FROM lexical_documents ORDER BY document_key",
    )?;
    let mut rows = statement.query([])?;
    let mut documents = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let key = row.get::<_, String>(0)?;
        let hash = row.get::<_, String>(1)?;
        if key.is_empty() || hash.len() != 64 || documents.insert(key.clone(), hash).is_some() {
            bail!("lexical component contains an invalid document identity");
        }
    }
    Ok(documents)
}

fn validate_lexical_component_file_name(file_name: &str) -> Result<()> {
    let path = Path::new(file_name);
    let mut components = path.components();
    if file_name.is_empty()
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
        || matches!(file_name, "." | "..")
    {
        bail!("lexical component file name is not a safe path atom");
    }
    Ok(())
}

fn validate_lexical_component_descriptor(descriptor: &LexicalComponentDescriptor) -> Result<()> {
    validate_lexical_component_file_name(&descriptor.file_name)?;
    if descriptor.bytes == 0
        || descriptor.metadata.project_id.trim().is_empty()
        || descriptor.metadata.sidecar_input_hash.trim().is_empty()
        || descriptor.metadata.lexical_hash.len() != 64
        || descriptor.metadata.binding_sha256.len() != 64
    {
        bail!("lexical component descriptor is invalid");
    }
    Ok(())
}

fn lexical_component_set_binding(component_set: &LexicalComponentSet) -> String {
    let mut canonical = component_set.clone();
    canonical.binding_sha256.clear();
    let bytes =
        serde_json::to_vec(&canonical).expect("lexical component-set serialization is infallible");
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone)]
pub struct LexicalHit {
    pub path: String,
    pub source: LexicalDocumentSource,
    pub node_id: Option<String>,
    pub symbol_name: Option<String>,
    pub start_line: Option<u32>,
    pub target: Option<SearchTargetDto>,
    pub source_excerpt: Option<String>,
    pub score: f32,
}

pub(crate) struct LexicalSourceInput {
    coverage: LexicalCoverage,
    documents: Vec<LexicalDocument>,
    source_seals: Vec<ArtifactSeal>,
}

#[derive(Clone)]
pub(crate) struct PreparedLexicalInput {
    pub fingerprint: LexicalInputFingerprint,
    documents: Vec<LexicalDocument>,
    source_seals: Vec<ArtifactSeal>,
    /// Present for a bounded transition whose `documents` contain only the
    /// changed rows. The complete key/hash state remains canonical and is what
    /// binds the published base-plus-delta view.
    bounded_state: Option<Arc<LexicalLogicalState>>,
}

impl PreparedLexicalInput {
    pub(crate) fn document_count(&self) -> u64 {
        u64::from(self.fingerprint.file_count)
    }

    pub(crate) fn revalidate_source_seals(
        &self,
        project_root: &Path,
        storage_path: &Path,
    ) -> Result<()> {
        let observed = observe_lexical_source_seals(project_root, Some(storage_path))?;
        if observed != self.source_seals {
            bail!("lexical source identity changed after its single content inspection");
        }
        Ok(())
    }
}

struct LexicalScanOutcome {
    coverage: LexicalCoverage,
    source_seals: Vec<ArtifactSeal>,
}

#[cfg(test)]
pub fn lexical_input_fingerprint(
    project_root: &Path,
    storage_path: Option<&Path>,
) -> Result<LexicalInputFingerprint> {
    let mut documents = Vec::new();
    let scan = scan_lexical_documents(project_root, storage_path, storage_path, &mut |document| {
        documents.push(document.clone());
        Ok(())
    })?;
    prepared_lexical_fingerprint(&documents, &scan.coverage)
}

pub(crate) fn lexical_source_input(
    project_root: &Path,
    storage_path: &Path,
) -> Result<LexicalSourceInput> {
    let mut documents = Vec::new();
    let scan = scan_lexical_documents(project_root, Some(storage_path), None, &mut |document| {
        documents.push(document.clone());
        Ok(())
    })?;
    Ok(LexicalSourceInput {
        coverage: scan.coverage,
        documents,
        source_seals: scan.source_seals,
    })
}

pub(crate) fn finish_lexical_input_for_store(
    source: LexicalSourceInput,
    project_root: &Path,
    storage: &Store,
) -> Result<LexicalInputFingerprint> {
    Ok(prepare_lexical_input_for_store(source, project_root, storage)?.fingerprint)
}

pub(crate) fn prepare_lexical_input_for_store(
    mut source: LexicalSourceInput,
    project_root: &Path,
    storage: &Store,
) -> Result<PreparedLexicalInput> {
    scan_symbol_documents_from_store(project_root, storage, &mut |document| {
        source.documents.push(document.clone());
        Ok(())
    })?;
    let fingerprint = prepared_lexical_fingerprint(&source.documents, &source.coverage)?;
    Ok(PreparedLexicalInput {
        fingerprint,
        documents: source.documents,
        source_seals: source.source_seals,
        bounded_state: None,
    })
}

/// Prepare a source-identity-only lexical transition from the predecessor's
/// validated logical state. Symbol and component-report documents are retained
/// by hash; only the exact changed source rows are read from the workspace.
///
/// `Ok(None)` is a deliberate fail-closed fallback to the complete scan. It is
/// returned for policy drift, a legacy/corrupt predecessor, additions/removals,
/// or a changed source that was not already represented in the lexical state.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_bounded_lexical_input(
    project_root: &Path,
    current_storage_path: &Path,
    previous_storage_path: &Path,
    lexical_data_dir: &Path,
    previous_generation: &str,
    changed_existing_sources: &[String],
    source_seals: &[ArtifactSeal],
    expected_policy: &codestory_contracts::workspace::SourceIndexPolicy,
) -> Result<Option<PreparedLexicalInput>> {
    macro_rules! bounded_ineligible {
        ($_reason:literal) => {{
            return Ok(None);
        }};
    }
    if previous_generation.trim().is_empty()
        || changed_existing_sources.is_empty()
        || source_seals.is_empty()
    {
        bounded_ineligible!("missing_generation_changes_or_seals");
    }
    let current_policy = lexical_source_policy(project_root, Some(current_storage_path))?;
    let previous_policy = lexical_source_policy(project_root, Some(previous_storage_path))?;
    if current_policy != previous_policy
        || current_policy.policy_version != expected_policy.policy_version
        || current_policy.max_file_bytes != expected_policy.byte_cap
        || current_policy.structural_unit_cap != expected_policy.structural_unit_cap
    {
        bounded_ineligible!("source_policy_changed");
    }

    let previous_shard = shard_dir_for(lexical_data_dir, previous_generation);
    let Some(previous_set) =
        read_lexical_component_set(&previous_shard, Some(previous_generation), None)
            .ok()
            .flatten()
    else {
        bounded_ineligible!("previous_component_set_missing");
    };
    let previous_state = match validate_lexical_component_set_files(&previous_shard, &previous_set)
    {
        Ok(state) => state,
        Err(_) => bounded_ineligible!("previous_component_set_invalid"),
    };

    let mut changed_documents = Vec::with_capacity(changed_existing_sources.len());
    let mut desired_documents = previous_state.documents.clone();
    let mut previous_path: Option<&str> = None;
    for relative in changed_existing_sources {
        let relative_path = Path::new(relative);
        if relative.trim().is_empty()
            || relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
            || previous_path.is_some_and(|previous| previous >= relative.as_str())
            || current_policy.excluded_paths.contains(relative)
        {
            bounded_ineligible!("changed_path_not_canonical");
        }
        previous_path = Some(relative.as_str());
        let path = project_root.join(relative_path);
        let before = match ArtifactSeal::observe(&path) {
            Ok(seal) => seal,
            Err(_) => bounded_ineligible!("changed_source_unsealable"),
        };
        if !source_seals
            .binary_search_by(|seal| seal.path().cmp(&path))
            .ok()
            .is_some_and(|position| source_seals[position] == before)
        {
            bounded_ineligible!("changed_source_not_in_core_inventory");
        }
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => bounded_ineligible!("changed_source_missing_or_not_file"),
        };
        if metadata.len() > current_policy.max_file_bytes {
            bounded_ineligible!("changed_source_oversized");
        }
        let Some(content) =
            (match read_lexical_file_text_limited(&path, current_policy.max_file_bytes) {
                Ok(content) => content,
                Err(_) => bounded_ineligible!("changed_source_unreadable"),
            })
        else {
            bounded_ineligible!("changed_source_invalid_utf8_or_oversized");
        };
        let after = ArtifactSeal::observe(&path).with_context(|| {
            format!("seal bounded lexical source after read {}", path.display())
        })?;
        if after != before {
            bail!(
                "bounded lexical source changed while reading {}",
                path.display()
            );
        }
        let document = LexicalDocument {
            path: relative.clone(),
            content,
            source: LexicalDocumentSource::LexicalSource,
            node_id: None,
            symbol_name: None,
            start_line: None,
        };
        let key = lexical_document_key(&document)?;
        if !previous_state.documents.contains_key(&key) {
            bounded_ineligible!("changed_source_not_in_previous_lexical_state");
        }
        desired_documents.insert(key, lexical_document_hash(&document));
        changed_documents.push(document);
    }

    let fingerprint = lexical_fingerprint_from_document_hashes(
        &desired_documents,
        &previous_state.fingerprint.coverage,
    )?;
    let state_sha256 = lexical_state_digest(&fingerprint, &desired_documents);
    let desired_state = Arc::new(LexicalLogicalState {
        fingerprint: fingerprint.clone(),
        documents: desired_documents,
        state_sha256,
    });
    Ok(Some(PreparedLexicalInput {
        fingerprint,
        documents: changed_documents,
        source_seals: source_seals.to_vec(),
        bounded_state: Some(desired_state),
    }))
}

#[cfg(any(test, feature = "test-support"))]
pub fn build_lexical_shard(
    project_root: &Path,
    storage_path: Option<&Path>,
    lexical_data_dir: &Path,
    project_id: &str,
    expected: &LexicalInputFingerprint,
    sidecar_input_hash: &str,
) -> Result<LexicalInputFingerprint> {
    let shard_dir = shard_dir_for(lexical_data_dir, project_id);
    std::fs::create_dir_all(&shard_dir)
        .with_context(|| format!("create lexical shard directory {}", shard_dir.display()))?;
    let index_path = shard_dir.join(LEXICAL_INDEX_FILE);
    let (temp_path, reserved) =
        codestory_workspace::atomic_file::create_unique_temp_file(&index_path, "lexical-index")?;
    drop(reserved);
    let result: Result<LexicalInputFingerprint> = (|| {
        let rebuilt = write_lexical_database(
            &temp_path,
            project_id,
            sidecar_input_hash,
            expected,
            |visit| {
                scan_lexical_documents(project_root, storage_path, storage_path, visit)
                    .map(|scan| scan.coverage)
            },
        )?;
        // The staged file is about to be renamed away, so its verdict is not
        // receiptable: seal the published identity, never the temporary one.
        let staged = verify_lexical_database_contents(&temp_path)?;
        match_lexical_shard_expectations(
            &staged,
            project_id,
            sidecar_input_hash,
            Some((expected.file_count, expected.hash.as_str())),
        )?;
        publish_immutable_lexical_database(&temp_path, &index_path)?;
        publish_lexical_component_envelope(
            &shard_dir,
            &LexicalComponentEnvelope::new(project_id, sidecar_input_hash, &staged),
        )?;
        Ok(rebuilt)
    })();
    if result.is_err() {
        let _ = crate::copy_on_write::make_file_owner_writable(&temp_path);
        let _ = std::fs::remove_file(&temp_path);
    }
    let rebuilt = result?;

    // Old JSONL generations are migration inputs only: the new reader never opens them.
    for legacy in [LEGACY_INDEX_FILE, LEGACY_META_FILE, LEGACY_STUB_MARKER] {
        let path = shard_dir.join(legacy);
        if path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(rebuilt)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IncrementalLexicalWork {
    pub retained: u64,
    pub inserted: u64,
    pub removed: u64,
    pub direct_reference: bool,
}

fn publish_lexical_state_for_generation(
    shard_dir: &Path,
    previous_state_path: Option<&Path>,
    desired: &LexicalLogicalState,
    delta: Option<&LexicalStateDelta<'_>>,
) -> Result<()> {
    let state_path = shard_dir.join(LEXICAL_STATE_FILE);
    if state_path.is_file()
        && load_lexical_state_database(&state_path, &desired.fingerprint, &desired.state_sha256)
            .is_ok()
    {
        return Ok(());
    }
    if state_path.exists() {
        let _ = crate::copy_on_write::make_file_owner_writable(&state_path);
        std::fs::remove_file(&state_path)?;
    }
    if delta.is_some_and(|delta| delta.upserts.is_empty() && delta.tombstones.is_empty())
        && let Some(previous_state_path) = previous_state_path
    {
        let previous_artifacts = sqlite_file_with_sidecars(previous_state_path);
        let transferable = LEXICAL_STATE_RECEIPTS
            .transferable_receipt(&previous_state_path.to_path_buf(), &previous_artifacts);
        if crate::copy_on_write::reference_file(previous_state_path, &state_path)? {
            if let Some(transferable) = transferable {
                let _ = LEXICAL_STATE_RECEIPTS.install_hard_link_alias(
                    &previous_state_path.to_path_buf(),
                    &previous_artifacts,
                    state_path.clone(),
                    &sqlite_file_with_sidecars(&state_path),
                    transferable,
                    Ok::<_, anyhow::Error>,
                )?;
            }
            return Ok(());
        }
    }

    let (temp_path, reserved) =
        codestory_workspace::atomic_file::create_unique_temp_file(&state_path, "lexical-state")?;
    drop(reserved);
    std::fs::remove_file(&temp_path)?;
    let result = (|| {
        let mut reconciled = false;
        if let (Some(previous_state_path), Some(delta)) = (previous_state_path, delta)
            && crate::copy_on_write::clone_file(previous_state_path, &temp_path)?
        {
            crate::copy_on_write::make_file_owner_writable(&temp_path)?;
            reconcile_cloned_lexical_state_database(&temp_path, desired, delta)?;
            reconciled = true;
        }
        if !reconciled {
            if temp_path.exists() {
                std::fs::remove_file(&temp_path)?;
            }
            write_lexical_state_database(&temp_path, desired)?;
        }
        let observed = read_lexical_state_database(&temp_path)?;
        if observed.fingerprint != desired.fingerprint
            || observed.documents != desired.documents
            || observed.state_sha256 != desired.state_sha256
        {
            bail!("staged lexical state does not match desired logical state");
        }
        crate::copy_on_write::publish_immutable_file_atomic(&temp_path, &state_path)
    })();
    if result.is_err() && temp_path.exists() {
        let _ = crate::copy_on_write::make_file_owner_writable(&temp_path);
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn install_lexical_component_reference(source: &Path, destination: &Path) -> Result<bool> {
    if destination.is_file() {
        if codestory_workspace::same_workspace_path(source, destination) {
            return Ok(true);
        }
        let _ = crate::copy_on_write::make_file_owner_writable(destination);
        std::fs::remove_file(destination)?;
    }
    let source_key = source.to_path_buf();
    let source_artifacts = sqlite_file_with_sidecars(source);
    let transferable = LEXICAL_SHARD_RECEIPTS.transferable_receipt(&source_key, &source_artifacts);
    if crate::copy_on_write::reference_file(source, destination)? {
        if let Some(transferable) = transferable {
            let _ = LEXICAL_SHARD_RECEIPTS.install_hard_link_alias(
                &source_key,
                &source_artifacts,
                destination.to_path_buf(),
                &sqlite_file_with_sidecars(destination),
                transferable,
                Ok::<_, anyhow::Error>,
            )?;
        }
        return Ok(true);
    }
    if crate::copy_on_write::clone_file(source, destination)? {
        crate::copy_on_write::make_file_immutable(destination)?;
        return Ok(true);
    }
    Ok(false)
}

fn install_previous_lexical_components(
    previous_shard: &Path,
    shard_dir: &Path,
    component_set: &LexicalComponentSet,
) -> Result<bool> {
    for descriptor in std::iter::once(&component_set.base)
        .chain(component_set.deltas.iter().map(|delta| &delta.component))
    {
        if !install_lexical_component_reference(
            &previous_shard.join(&descriptor.file_name),
            &shard_dir.join(&descriptor.file_name),
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn write_lexical_delta_component(
    shard_dir: &Path,
    generation: &str,
    sidecar_input_hash: &str,
    ordinal: u32,
    delta: &LexicalStateDelta<'_>,
) -> Result<LexicalDeltaDescriptor> {
    let file_name = format!("{LEXICAL_DELTA_FILE_PREFIX}{ordinal:04}.sqlite3");
    let path = shard_dir.join(&file_name);
    let documents = delta
        .upserts
        .iter()
        .map(|(_, document)| (*document).clone())
        .collect::<Vec<_>>();
    let coverage = LexicalCoverage::default();
    let fingerprint = prepared_lexical_fingerprint(&documents, &coverage)?;
    let physical_project_id = format!("{generation}:delta:{ordinal}");
    let (temp_path, reserved) =
        codestory_workspace::atomic_file::create_unique_temp_file(&path, "lexical-delta")?;
    drop(reserved);
    let result = (|| {
        let rebuilt = write_lexical_database(
            &temp_path,
            &physical_project_id,
            sidecar_input_hash,
            &fingerprint,
            |visit| {
                for document in &documents {
                    visit(document)?;
                }
                Ok(coverage.clone())
            },
        )?;
        if rebuilt != fingerprint {
            bail!("staged lexical delta fingerprint changed during construction");
        }
        let metadata = verify_lexical_database_contents(&temp_path)?;
        publish_immutable_lexical_database(&temp_path, &path)?;
        Ok(LexicalDeltaDescriptor {
            ordinal,
            component: lexical_component_descriptor(&path, metadata)?,
            upsert_keys: delta.upserts.iter().map(|(key, _)| key.clone()).collect(),
            tombstone_keys: delta.tombstones.clone(),
        })
    })();
    if result.is_err() && temp_path.exists() {
        let _ = crate::copy_on_write::make_file_owner_writable(&temp_path);
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn publish_full_lexical_component_set(
    shard_dir: &Path,
    generation: &str,
    sidecar_input_hash: &str,
    expected: &PreparedLexicalInput,
    desired_state: &LexicalLogicalState,
    before_publish: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let index_path = shard_dir.join(LEXICAL_INDEX_FILE);
    let (temp_path, reserved) =
        codestory_workspace::atomic_file::create_unique_temp_file(&index_path, "lexical-index")?;
    drop(reserved);
    let result = (|| {
        let rebuilt = write_lexical_database(
            &temp_path,
            generation,
            sidecar_input_hash,
            &expected.fingerprint,
            |visit| {
                for document in &expected.documents {
                    visit(document)?;
                }
                Ok(expected.fingerprint.coverage.clone())
            },
        )?;
        if rebuilt != expected.fingerprint {
            bail!("staged lexical base fingerprint changed during construction");
        }
        let base_metadata = verify_lexical_database_contents(&temp_path)?;
        before_publish()?;
        publish_lexical_state_for_generation(shard_dir, None, desired_state, None)?;
        publish_immutable_lexical_database(&temp_path, &index_path)?;
        let base = lexical_component_descriptor(&index_path, base_metadata.clone())?;
        publish_lexical_component_envelope(
            shard_dir,
            &LexicalComponentEnvelope::new(generation, sidecar_input_hash, &base_metadata),
        )?;
        publish_lexical_component_set(
            shard_dir,
            &LexicalComponentSet::new(
                generation,
                sidecar_input_hash,
                &expected.fingerprint,
                base,
                Vec::new(),
                desired_state.state_sha256.clone(),
            ),
        )
    })();
    if result.is_err() && temp_path.exists() {
        let _ = crate::copy_on_write::make_file_owner_writable(&temp_path);
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn lexical_component_set_needs_compaction(component_set: &LexicalComponentSet) -> bool {
    let delta_bytes = component_set
        .deltas
        .iter()
        .map(|delta| delta.component.bytes)
        .sum::<u64>();
    component_set.deltas.len() >= LEXICAL_DELTA_COMPACTION_COUNT
        || delta_bytes.saturating_mul(100)
            > component_set
                .base
                .bytes
                .saturating_mul(LEXICAL_DELTA_COMPACTION_PERCENT)
}

pub(crate) fn lexical_component_bytes(shard_dir: &Path) -> Option<u64> {
    let project_id = shard_dir.file_name()?.to_str()?;
    if let Ok(Some(component_set)) = read_lexical_component_set(shard_dir, Some(project_id), None) {
        let component_bytes = component_set
            .deltas
            .iter()
            .fold(component_set.base.bytes, |total, delta| {
                total.saturating_add(delta.component.bytes)
            });
        let state_bytes = std::fs::metadata(shard_dir.join(LEXICAL_STATE_FILE))
            .ok()
            .map_or(0, |metadata| metadata.len());
        return Some(component_bytes.saturating_add(state_bytes));
    }
    std::fs::metadata(shard_dir.join(LEXICAL_INDEX_FILE))
        .ok()
        .map(|metadata| metadata.len())
}

fn schedule_lexical_compaction_if_needed(
    shard_dir: PathBuf,
    expected: &PreparedLexicalInput,
    component_set: LexicalComponentSet,
) {
    if !lexical_component_set_needs_compaction(&component_set) {
        return;
    }
    #[cfg(not(test))]
    {
        let expected = expected.clone();
        let name = format!("codestory-lexical-compact-{}", component_set.generation);
        if let Err(error) = std::thread::Builder::new().name(name).spawn(move || {
            if let Err(error) = compact_lexical_component_set(&shard_dir, &expected, &component_set)
            {
                warn!(detail = %error, "background lexical compaction did not publish");
            }
        }) {
            warn!(detail = %error, "background lexical compaction could not start");
        }
    }
    #[cfg(test)]
    let _ = (shard_dir, expected, component_set);
}

fn compact_lexical_component_set(
    shard_dir: &Path,
    expected: &PreparedLexicalInput,
    source: &LexicalComponentSet,
) -> Result<()> {
    if !lexical_component_set_needs_compaction(source) {
        return Ok(());
    }
    let Some(current) = read_lexical_component_set(
        shard_dir,
        Some(&source.generation),
        Some(&source.sidecar_input_hash),
    )?
    else {
        bail!("lexical component set disappeared before compaction");
    };
    if &current != source {
        return Ok(());
    }
    let compact_name = format!(
        "lexical-base-compacted-{}.sqlite3",
        &source.lexical_hash[..16]
    );
    let compact_path = shard_dir.join(&compact_name);
    let (temp_path, reserved) = codestory_workspace::atomic_file::create_unique_temp_file(
        &compact_path,
        "lexical-compaction",
    )?;
    drop(reserved);
    let result = (|| {
        let physical_id = format!("{}:compacted", source.generation);
        let rebuilt = write_lexical_database(
            &temp_path,
            &physical_id,
            &source.sidecar_input_hash,
            &expected.fingerprint,
            |visit| {
                for document in &expected.documents {
                    visit(document)?;
                }
                Ok(expected.fingerprint.coverage.clone())
            },
        )?;
        if rebuilt != expected.fingerprint {
            bail!("compacted lexical base does not match its logical fingerprint");
        }
        let metadata = verify_lexical_database_contents(&temp_path)?;
        publish_immutable_lexical_database(&temp_path, &compact_path)?;
        let compacted = LexicalComponentSet::new(
            &source.generation,
            &source.sidecar_input_hash,
            &expected.fingerprint,
            lexical_component_descriptor(&compact_path, metadata.clone())?,
            Vec::new(),
            source.state_sha256.clone(),
        );
        let Some(still_current) = read_lexical_component_set(
            shard_dir,
            Some(&source.generation),
            Some(&source.sidecar_input_hash),
        )?
        else {
            bail!("lexical component set disappeared during compaction");
        };
        if &still_current != source {
            return Ok(());
        }
        publish_lexical_component_envelope(
            shard_dir,
            &LexicalComponentEnvelope::new(
                &source.generation,
                &source.sidecar_input_hash,
                &metadata,
            ),
        )?;
        publish_lexical_component_set(shard_dir, &compacted)
    })();
    if result.is_err() && temp_path.exists() {
        let _ = crate::copy_on_write::make_file_owner_writable(&temp_path);
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

pub(crate) fn build_prepared_lexical_shard(
    lexical_data_dir: &Path,
    project_id: &str,
    expected: &PreparedLexicalInput,
    sidecar_input_hash: &str,
    previous_project_id: Option<&str>,
    before_publish: impl FnOnce() -> Result<()>,
) -> Result<(LexicalInputFingerprint, Option<IncrementalLexicalWork>)> {
    if expected.bounded_state.is_none()
        && prepared_lexical_fingerprint(&expected.documents, &expected.fingerprint.coverage)?
            != expected.fingerprint
    {
        bail!("prepared lexical documents do not match their fingerprint");
    }
    let shard_dir = shard_dir_for(lexical_data_dir, project_id);
    std::fs::create_dir_all(&shard_dir)?;
    let desired_state = prepared_lexical_state(expected)?;
    let mut before_publish = Some(before_publish);

    if let Some(previous_project_id) = previous_project_id {
        let previous_shard = shard_dir_for(lexical_data_dir, previous_project_id);
        let previous_component_set =
            read_lexical_component_set(&previous_shard, Some(previous_project_id), None)
                .ok()
                .flatten();
        let predecessor = if let Some(component_set) = previous_component_set {
            validate_lexical_component_set_files(&previous_shard, &component_set)
                .ok()
                .map(|state| {
                    let receipt_key = previous_shard.join(LEXICAL_COMPONENT_SET_FILE);
                    let receipt_artifacts =
                        lexical_component_set_artifacts(&previous_shard, &component_set);
                    let receipt = LEXICAL_COMPONENT_SET_RECEIPTS
                        .transferable_receipt(&receipt_key, &receipt_artifacts);
                    (
                        component_set,
                        state,
                        Some(previous_shard.join(LEXICAL_STATE_FILE)),
                        receipt.map(|receipt| (receipt_key, receipt_artifacts, receipt)),
                    )
                })
        } else {
            let previous_path = previous_shard.join(LEXICAL_INDEX_FILE);
            read_lexical_component_envelope(&previous_shard, Some(previous_project_id), None)
                .ok()
                .and_then(|envelope| {
                    verify_lexical_database_contents(&previous_path)
                        .ok()
                        .filter(|metadata| envelope.matches_physical(metadata))
                        .and_then(|metadata| {
                            let state = legacy_lexical_state(&previous_path).ok()?;
                            let base =
                                lexical_component_descriptor(&previous_path, metadata).ok()?;
                            Some((
                                LexicalComponentSet::new(
                                    previous_project_id,
                                    &envelope.sidecar_input_hash,
                                    &state.fingerprint,
                                    base,
                                    Vec::new(),
                                    state.state_sha256.clone(),
                                ),
                                state,
                                None,
                                None,
                            ))
                        })
                })
        };

        if let Some((previous_set, previous_state, previous_state_path, previous_receipt)) =
            predecessor
        {
            let delta = lexical_state_delta(&previous_state, &desired_state, &expected.documents)?;
            before_publish
                .take()
                .expect("lexical publication callback runs once")()?;
            if install_previous_lexical_components(&previous_shard, &shard_dir, &previous_set)? {
                publish_lexical_state_for_generation(
                    &shard_dir,
                    previous_state_path.as_deref(),
                    &desired_state,
                    Some(&delta),
                )?;
                if let Some((key, artifacts, receipt)) = previous_receipt {
                    let _ = LEXICAL_COMPONENT_SET_RECEIPTS
                        .refresh_after_hard_links(key, &artifacts, receipt);
                }
                let mut deltas = previous_set.deltas.clone();
                if !delta.upserts.is_empty() || !delta.tombstones.is_empty() {
                    let ordinal = deltas
                        .last()
                        .map_or(1, |delta| delta.ordinal.saturating_add(1));
                    deltas.push(write_lexical_delta_component(
                        &shard_dir,
                        project_id,
                        sidecar_input_hash,
                        ordinal,
                        &delta,
                    )?);
                }
                let base_path = shard_dir.join(&previous_set.base.file_name);
                let base_metadata = previous_set.base.metadata.clone();
                let component_set = LexicalComponentSet::new(
                    project_id,
                    sidecar_input_hash,
                    &expected.fingerprint,
                    lexical_component_descriptor(&base_path, base_metadata.clone())?,
                    deltas,
                    desired_state.state_sha256.clone(),
                );
                publish_lexical_component_envelope(
                    &shard_dir,
                    &LexicalComponentEnvelope::new(project_id, sidecar_input_hash, &base_metadata),
                )?;
                publish_lexical_component_set(&shard_dir, &component_set)?;
                let _ = LEXICAL_COMPONENT_SET_RECEIPTS.seal_produced(
                    shard_dir.join(LEXICAL_COMPONENT_SET_FILE),
                    &lexical_component_set_artifacts(&shard_dir, &component_set),
                    Arc::clone(&desired_state),
                );
                if expected.bounded_state.is_none() {
                    schedule_lexical_compaction_if_needed(
                        shard_dir.clone(),
                        expected,
                        component_set,
                    );
                }
                let inserted = u64::try_from(delta.upserts.len()).unwrap_or(u64::MAX);
                let removed = u64::try_from(previous_state.documents.len())
                    .unwrap_or(u64::MAX)
                    .saturating_sub(delta.retained);
                return Ok((
                    expected.fingerprint.clone(),
                    Some(IncrementalLexicalWork {
                        retained: delta.retained,
                        inserted,
                        removed,
                        direct_reference: inserted == 0 && removed == 0,
                    }),
                ));
            }
        }
    }

    if expected.bounded_state.is_some() {
        bail!("bounded lexical refresh has no compatible predecessor");
    }
    let remaining_before_publish = before_publish.take();
    publish_full_lexical_component_set(
        &shard_dir,
        project_id,
        sidecar_input_hash,
        expected,
        &desired_state,
        move || match remaining_before_publish {
            Some(before_publish) => before_publish(),
            None => Ok(()),
        },
    )?;
    Ok((expected.fingerprint.clone(), None))
}

fn publish_immutable_lexical_database(temp_path: &Path, index_path: &Path) -> Result<()> {
    crate::copy_on_write::publish_immutable_file_atomic(temp_path, index_path)
}

pub fn shard_has_lexical_index(shard_dir: &Path, expected_sidecar_input_hash: &str) -> bool {
    let Some(project_id) = shard_dir.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    match read_lexical_component_set(
        shard_dir,
        Some(project_id),
        Some(expected_sidecar_input_hash),
    ) {
        Ok(Some(component_set)) => {
            return validate_lexical_component_set_files(shard_dir, &component_set).is_ok();
        }
        Ok(None) => {}
        Err(_) => return false,
    }
    validate_lexical_database(
        &shard_dir.join(LEXICAL_INDEX_FILE),
        project_id,
        expected_sidecar_input_hash,
        None,
    )
    .is_ok()
}

pub fn shard_matches_lexical_input(
    lexical_data_dir: &Path,
    sidecar_generation: &str,
    expected_file_count: u32,
    expected_hash: &str,
    expected_sidecar_input_hash: &str,
) -> bool {
    let shard_dir = shard_dir_for(lexical_data_dir, sidecar_generation);
    match read_lexical_component_set(
        &shard_dir,
        Some(sidecar_generation),
        Some(expected_sidecar_input_hash),
    ) {
        Ok(Some(component_set)) => {
            return component_set.file_count == expected_file_count
                && component_set.lexical_hash == expected_hash
                && validate_lexical_component_set_files(&shard_dir, &component_set).is_ok();
        }
        Ok(None) => {}
        Err(_) => return false,
    }
    validate_lexical_database(
        &shard_dir.join(LEXICAL_INDEX_FILE),
        sidecar_generation,
        expected_sidecar_input_hash,
        Some((expected_file_count, expected_hash)),
    )
    .is_ok()
}

pub fn lexical_shard_coverage(
    lexical_data_dir: &Path,
    sidecar_generation: &str,
    expected_sidecar_input_hash: &str,
) -> Result<LexicalCoverage> {
    let shard_dir = shard_dir_for(lexical_data_dir, sidecar_generation);
    if let Some(component_set) = read_lexical_component_set(
        &shard_dir,
        Some(sidecar_generation),
        Some(expected_sidecar_input_hash),
    )? {
        validate_lexical_component_set_files(&shard_dir, &component_set)?;
        return Ok(component_set.coverage);
    }
    Ok(validate_lexical_database(
        &shard_dir.join(LEXICAL_INDEX_FILE),
        sidecar_generation,
        expected_sidecar_input_hash,
        None,
    )?
    .coverage)
}

/// Sealed-receipt accounting for one shard, for tests that must prove the deep
/// verification ran exactly as often as the seal allowed.
#[cfg(test)]
pub(crate) fn lexical_shard_receipt_stats(
    lexical_data_dir: &Path,
    sidecar_generation: &str,
) -> Option<codestory_contracts::validation_receipts::ReceiptStats> {
    let shard_dir = shard_dir_for(lexical_data_dir, sidecar_generation);
    let path = read_lexical_component_set(&shard_dir, Some(sidecar_generation), None)
        .ok()
        .flatten()
        .map_or_else(
            || shard_dir.join(LEXICAL_INDEX_FILE),
            |component_set| shard_dir.join(component_set.base.file_name),
        );
    LEXICAL_SHARD_RECEIPTS.stats(&path)
}

struct LexicalShardReceiptRefresh {
    key: PathBuf,
    artifacts: Vec<PathBuf>,
    receipt: TransferableReceipt<LexicalShardMetadata>,
}

struct LexicalStateReceiptRefresh {
    key: PathBuf,
    artifacts: Vec<PathBuf>,
    receipt: TransferableReceipt<Arc<LexicalLogicalState>>,
}

/// Receipts for one validated generation captured immediately before owned
/// retention cleanup can remove sibling hard links.
pub(crate) struct LexicalGenerationReceiptRefresh {
    shards: Vec<LexicalShardReceiptRefresh>,
    component_set: Option<LexicalStateReceiptRefresh>,
    state: Option<LexicalStateReceiptRefresh>,
}

pub(crate) fn capture_lexical_generation_receipts(
    lexical_data_dir: &Path,
    generation: &str,
) -> Result<LexicalGenerationReceiptRefresh> {
    let shard_dir = shard_dir_for(lexical_data_dir, generation);
    let component_set = read_lexical_component_set(&shard_dir, Some(generation), None)?;
    let mut shards = Vec::new();
    let mut component_set_refresh = None;
    if let Some(component_set) = component_set.as_ref() {
        for descriptor in std::iter::once(&component_set.base)
            .chain(component_set.deltas.iter().map(|delta| &delta.component))
        {
            let key = shard_dir.join(&descriptor.file_name);
            let artifacts = sqlite_file_with_sidecars(&key);
            if let Some(receipt) = LEXICAL_SHARD_RECEIPTS.transferable_receipt(&key, &artifacts) {
                shards.push(LexicalShardReceiptRefresh {
                    key,
                    artifacts,
                    receipt,
                });
            }
        }
        let key = shard_dir.join(LEXICAL_COMPONENT_SET_FILE);
        let artifacts = lexical_component_set_artifacts(&shard_dir, component_set);
        if let Some(receipt) = LEXICAL_COMPONENT_SET_RECEIPTS.transferable_receipt(&key, &artifacts)
        {
            component_set_refresh = Some(LexicalStateReceiptRefresh {
                key,
                artifacts,
                receipt,
            });
        }
    } else {
        let key = shard_dir.join(LEXICAL_INDEX_FILE);
        let artifacts = sqlite_file_with_sidecars(&key);
        if let Some(receipt) = LEXICAL_SHARD_RECEIPTS.transferable_receipt(&key, &artifacts) {
            shards.push(LexicalShardReceiptRefresh {
                key,
                artifacts,
                receipt,
            });
        }
    }
    let state_key = shard_dir.join(LEXICAL_STATE_FILE);
    let state_artifacts = sqlite_file_with_sidecars(&state_key);
    let state = LEXICAL_STATE_RECEIPTS
        .transferable_receipt(&state_key, &state_artifacts)
        .map(|receipt| LexicalStateReceiptRefresh {
            key: state_key,
            artifacts: state_artifacts,
            receipt,
        });
    Ok(LexicalGenerationReceiptRefresh {
        shards,
        component_set: component_set_refresh,
        state,
    })
}

impl LexicalGenerationReceiptRefresh {
    /// Reseal only metadata churn caused by an owned hard-link deletion.
    /// Byte-affecting drift is refused and the next reader deep-validates.
    pub(crate) fn refresh_after_owned_link_cleanup(self) -> (usize, usize) {
        let mut refreshed = 0_usize;
        let mut refused = 0_usize;
        for shard in self.shards {
            if LEXICAL_SHARD_RECEIPTS.refresh_after_hard_links(
                shard.key,
                &shard.artifacts,
                shard.receipt,
            ) {
                refreshed += 1;
            } else {
                refused += 1;
            }
        }
        if let Some(component_set) = self.component_set {
            if LEXICAL_COMPONENT_SET_RECEIPTS.refresh_after_hard_links(
                component_set.key,
                &component_set.artifacts,
                component_set.receipt,
            ) {
                refreshed += 1;
            } else {
                refused += 1;
            }
        }
        if let Some(state) = self.state {
            if LEXICAL_STATE_RECEIPTS.refresh_after_hard_links(
                state.key,
                &state.artifacts,
                state.receipt,
            ) {
                refreshed += 1;
            } else {
                refused += 1;
            }
        }
        (refreshed, refused)
    }
}

#[cfg(test)]
pub fn search_lexical_index(
    shard_dir: &Path,
    expected_sidecar_input_hash: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<LexicalHit>> {
    search_lexical_index_with_cancel(shard_dir, expected_sidecar_input_hash, query, limit, || {
        false
    })
}

pub fn search_lexical_index_with_cancel<F>(
    shard_dir: &Path,
    expected_sidecar_input_hash: &str,
    query: &str,
    limit: usize,
    cancelled: F,
) -> Result<Vec<LexicalHit>>
where
    F: Fn() -> bool + Send + Sync + 'static,
{
    let cancelled: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(cancelled);
    let result = search_lexical_index_with_cancel_inner(
        shard_dir,
        expected_sidecar_input_hash,
        query,
        limit,
        Arc::clone(&cancelled),
    );
    if result.is_err() && cancelled() {
        bail!("lexical search cancelled");
    }
    result
}

fn search_lexical_index_with_cancel_inner(
    shard_dir: &Path,
    expected_sidecar_input_hash: &str,
    query: &str,
    limit: usize,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Vec<LexicalHit>> {
    if cancelled() {
        bail!("lexical search cancelled");
    }
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(project_id) = shard_dir.file_name().and_then(|name| name.to_str()) else {
        bail!("lexical shard path has no generation directory");
    };
    if let Some(component_set) = read_lexical_component_set(
        shard_dir,
        Some(project_id),
        Some(expected_sidecar_input_hash),
    )? {
        return search_lexical_component_set(shard_dir, &component_set, query, limit, cancelled);
    }
    let index_path = shard_dir.join(LEXICAL_INDEX_FILE);
    let connection = open_read_only(&index_path)?;
    let progress_cancelled = Arc::clone(&cancelled);
    connection.progress_handler(1_000, Some(move || progress_cancelled()))?;
    let _metadata = validate_open_database_metadata(
        &connection,
        shard_dir,
        project_id,
        expected_sidecar_input_hash,
        None,
        cancelled.as_ref(),
    )?;
    let document_count: usize = connection.query_row(
        "SELECT file_count FROM lexical_metadata WHERE id = 1",
        [],
        |row| row.get::<_, u32>(0).map(|count| count as usize),
    )?;
    search_lexical_index_on_connection(
        &connection,
        query,
        limit,
        document_count,
        &mut HashMap::new(),
        cancelled.as_ref(),
    )
}

pub(crate) fn search_lexical_index_batch_with_cancel(
    shard_dir: &Path,
    expected_sidecar_input_hash: &str,
    queries: &[(String, usize)],
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Vec<Vec<LexicalHit>>> {
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    if cancelled() {
        bail!("lexical search cancelled");
    }
    let Some(project_id) = shard_dir.file_name().and_then(|name| name.to_str()) else {
        bail!("lexical shard path has no generation directory");
    };
    if let Some(component_set) = read_lexical_component_set(
        shard_dir,
        Some(project_id),
        Some(expected_sidecar_input_hash),
    )? {
        return queries
            .iter()
            .map(|(query, limit)| {
                search_lexical_component_set(
                    shard_dir,
                    &component_set,
                    query,
                    *limit,
                    Arc::clone(&cancelled),
                )
            })
            .collect();
    }
    let index_path = shard_dir.join(LEXICAL_INDEX_FILE);
    let connection = open_read_only(&index_path)?;
    let progress_cancelled = Arc::clone(&cancelled);
    connection.progress_handler(1_000, Some(move || progress_cancelled()))?;
    let _metadata = validate_open_database_metadata(
        &connection,
        shard_dir,
        project_id,
        expected_sidecar_input_hash,
        None,
        cancelled.as_ref(),
    )?;
    let document_count: usize = connection.query_row(
        "SELECT file_count FROM lexical_metadata WHERE id = 1",
        [],
        |row| row.get::<_, u32>(0).map(|count| count as usize),
    )?;
    let mut token_frequencies = HashMap::new();
    queries
        .iter()
        .map(|(query, limit)| {
            search_lexical_index_on_connection(
                &connection,
                query,
                *limit,
                document_count,
                &mut token_frequencies,
                cancelled.as_ref(),
            )
        })
        .collect()
}

fn search_lexical_component_set(
    shard_dir: &Path,
    component_set: &LexicalComponentSet,
    query: &str,
    limit: usize,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Vec<LexicalHit>> {
    if cancelled() {
        bail!("lexical search cancelled");
    }
    validate_lexical_component_set_files(shard_dir, component_set)?;
    let mut latest = HashMap::<String, Option<usize>>::new();
    for (index, delta) in component_set.deltas.iter().enumerate() {
        for key in &delta.upsert_keys {
            latest.insert(key.clone(), Some(index));
        }
        for key in &delta.tombstone_keys {
            latest.insert(key.clone(), None);
        }
    }

    let logical_count = component_set.file_count as usize;
    let component_limit = MAX_CANDIDATES;
    let base_hits = search_lexical_component(
        shard_dir,
        &component_set.base,
        query,
        component_limit,
        logical_count,
        Arc::clone(&cancelled),
    )?;
    let mut hits = Vec::new();
    for hit in base_hits {
        let key = lexical_hit_document_key(&hit)?;
        if !latest.contains_key(&key) {
            hits.push(hit);
        }
    }
    for (index, delta) in component_set.deltas.iter().enumerate() {
        let delta_hits = search_lexical_component(
            shard_dir,
            &delta.component,
            query,
            component_limit,
            logical_count,
            Arc::clone(&cancelled),
        )?;
        for hit in delta_hits {
            let key = lexical_hit_document_key(&hit)?;
            if latest.get(&key) == Some(&Some(index)) {
                hits.push(hit);
            }
        }
    }
    if cancelled() {
        bail!("lexical search cancelled");
    }
    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.node_id.cmp(&right.node_id))
            .then_with(|| left.symbol_name.cmp(&right.symbol_name))
            .then_with(|| left.start_line.cmp(&right.start_line))
    });
    let mut seen = HashSet::new();
    hits.retain(|hit| seen.insert(lexical_hit_identity(hit)));
    hits.truncate(limit);
    Ok(hits)
}

fn search_lexical_component(
    shard_dir: &Path,
    descriptor: &LexicalComponentDescriptor,
    query: &str,
    limit: usize,
    logical_document_count: usize,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Vec<LexicalHit>> {
    validate_component_descriptor_at(shard_dir, descriptor)?;
    let path = shard_dir.join(&descriptor.file_name);
    let connection = open_read_only(&path)?;
    let progress_cancelled = Arc::clone(&cancelled);
    connection.progress_handler(1_000, Some(move || progress_cancelled()))?;
    let metadata = read_open_database_metadata(&connection, cancelled.as_ref())?;
    if metadata != descriptor.metadata {
        bail!("lexical component metadata changed after validation");
    }
    search_lexical_index_on_connection(
        &connection,
        query,
        limit,
        logical_document_count,
        &mut HashMap::new(),
        cancelled.as_ref(),
    )
}

fn lexical_hit_document_key(hit: &LexicalHit) -> Result<String> {
    match hit.source {
        LexicalDocumentSource::LexicalSource => Ok(format!("source\0{}", hit.path)),
        LexicalDocumentSource::SymbolDoc | LexicalDocumentSource::ComponentReport => {
            let node_id = hit
                .node_id
                .as_deref()
                .context("lexical component hit is missing its node identity")?;
            Ok(format!("{}\0{node_id}", hit.source.provenance_label()))
        }
    }
}

fn lexical_hit_identity(hit: &LexicalHit) -> LexicalCandidateIdentity {
    let source = match hit.source {
        LexicalDocumentSource::LexicalSource => 0,
        LexicalDocumentSource::SymbolDoc => 1,
        LexicalDocumentSource::ComponentReport => 2,
    };
    (
        hit.path.clone(),
        source,
        hit.node_id.clone(),
        hit.symbol_name.clone(),
        hit.start_line,
    )
}

fn search_lexical_index_on_connection(
    connection: &Connection,
    query: &str,
    limit: usize,
    document_count: usize,
    frequency_cache: &mut HashMap<String, usize>,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Result<Vec<LexicalHit>> {
    if cancelled() {
        bail!("lexical search cancelled");
    }
    if limit == 0 {
        return Ok(Vec::new());
    }
    let tokens = lexical_query_tokens(query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let fts_query = tokens
        .iter()
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let candidate_limit = limit.saturating_mul(64).clamp(256, MAX_CANDIDATES);

    let mandatory_tokens = quoted_query_tokens(query);
    let mut token_frequencies = Vec::with_capacity(tokens.len());
    for token in &tokens {
        if cancelled() {
            bail!("lexical search cancelled");
        }
        let frequency = match frequency_cache.get(token) {
            Some(frequency) => *frequency,
            None => {
                let frequency = fts_document_frequency(connection, token)?;
                frequency_cache.insert(token.clone(), frequency);
                frequency
            }
        };
        token_frequencies.push(frequency);
    }
    let token_weights = token_frequencies
        .iter()
        .zip(tokens.iter())
        .map(|(frequency, token)| {
            let mut weight = lexical_token_weight(*frequency, document_count);
            if mandatory_tokens.iter().any(|mandatory| mandatory == token) {
                weight *= 2.0;
            }
            weight
        })
        .collect::<Vec<_>>();
    // Coverage is a count of distinct query terms. Rarity weights order
    // candidates within lexical lanes; an unmatched rare term must not veto
    // the documented two-of-three or forty-percent admission contracts.
    let required_match_count = required_lexical_match_count(tokens.len());

    let exact_candidates = query_exact_candidates(connection, query, candidate_limit)?;
    let path_candidates = query_fts_candidates(
        connection,
        &fts_query,
        candidate_limit,
        LexicalCandidateOrder::Path,
    )?;
    let content_candidates = query_fts_candidates(
        connection,
        &fts_query,
        candidate_limit,
        LexicalCandidateOrder::Content,
    )?;
    let mut symbol_candidates = query_fts_candidates(
        connection,
        &fts_query,
        candidate_limit,
        LexicalCandidateOrder::SymbolDocument,
    )?;
    rank_symbol_candidates_by_identifier_overlap(&mut symbol_candidates, &tokens, &token_weights);

    // Exact, path-BM25, content-BM25, and focused symbol documents are
    // independent lexical recall lanes. Preserve every deterministic rank a
    // document earned and fuse those ranks instead of comparing incompatible
    // raw BM25 scores or collapsing them to one best lane. The coverage rule
    // below remains the admission threshold.
    let query_shape = crate::query_features::classify_query(query).shape;
    let mut best_sublane_ranks = HashMap::new();
    for (lane, candidates) in [
        (LexicalSublane::Exact, exact_candidates.as_slice()),
        (LexicalSublane::Path, path_candidates.as_slice()),
        (LexicalSublane::Content, content_candidates.as_slice()),
        (LexicalSublane::SymbolDocument, symbol_candidates.as_slice()),
    ] {
        record_lexical_sublane_ranks(&mut best_sublane_ranks, candidates, lane);
    }

    let mut candidates = exact_candidates;
    candidates.extend(interleave_candidate_lanes(vec![
        path_candidates,
        content_candidates,
        symbol_candidates,
    ]));

    let mut hits = Vec::new();
    let mut seen = HashSet::new();
    for (index, (document, normalized_path, normalized_content)) in
        candidates.into_iter().enumerate()
    {
        if index % 64 == 0 && cancelled() {
            bail!("lexical search cancelled");
        }
        let identity = lexical_candidate_identity(&document);
        let sublane_ranks = best_sublane_ranks
            .get(&identity)
            .copied()
            .unwrap_or_default();
        if seen.contains(&identity) {
            continue;
        }
        if seen.len() == MAX_CANDIDATES {
            break;
        }
        seen.insert(identity);
        let token_match = lexical_token_match(
            &tokens,
            &token_weights,
            &normalized_path,
            &normalized_content,
        );
        if token_match.matched_count >= required_match_count
            && mandatory_tokens_match(&mandatory_tokens, &normalized_path, &normalized_content)
        {
            let (target, matched_line, source_excerpt) =
                if document.source == LexicalDocumentSource::LexicalSource {
                    lexical_source_target(
                        &document.path,
                        &document.content,
                        &tokens,
                        token_match.content_weight > 0.0,
                    )
                } else {
                    (None, None, None)
                };
            hits.push(LexicalHit {
                score: lexical_sublane_score(sublane_ranks, query_shape),
                path: document.path,
                source: document.source,
                node_id: document.node_id,
                symbol_name: document.symbol_name,
                start_line: document.start_line.or(matched_line),
                target,
                source_excerpt,
            });
        }
    }
    if cancelled() {
        bail!("lexical search cancelled");
    }
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| lexical_source_rank(left.source).cmp(&lexical_source_rank(right.source)))
            .then_with(|| left.node_id.cmp(&right.node_id))
            .then_with(|| left.symbol_name.cmp(&right.symbol_name))
            .then_with(|| left.start_line.cmp(&right.start_line))
    });
    hits.truncate(limit);
    Ok(hits)
}

fn lexical_source_rank(source: LexicalDocumentSource) -> u8 {
    match source {
        LexicalDocumentSource::SymbolDoc => 0,
        LexicalDocumentSource::LexicalSource => 1,
        LexicalDocumentSource::ComponentReport => 2,
    }
}

#[derive(Debug, Clone, Copy)]
enum LexicalCandidateOrder {
    Path,
    Content,
    SymbolDocument,
}

type LexicalCandidate = (LexicalDocument, String, String);

fn query_fts_candidates(
    connection: &Connection,
    fts_query: &str,
    candidate_limit: usize,
    order: LexicalCandidateOrder,
) -> Result<Vec<LexicalCandidate>> {
    let scoped_query = match order {
        LexicalCandidateOrder::Path => format!("path : ({fts_query})"),
        LexicalCandidateOrder::Content | LexicalCandidateOrder::SymbolDocument => {
            format!("content : ({fts_query})")
        }
    };
    let sql = match order {
        LexicalCandidateOrder::Path => {
            "SELECT d.path, d.content, lexical_fts.path, lexical_fts.content,
                    d.source, d.node_id, d.symbol_name, d.start_line
             FROM lexical_fts
             JOIN lexical_documents d ON d.id = lexical_fts.rowid
             WHERE lexical_fts MATCH ?1
             ORDER BY bm25(lexical_fts, 8.0, 1.0), d.path, d.id
             LIMIT ?2"
        }
        LexicalCandidateOrder::Content => {
            "SELECT d.path, d.content, lexical_fts.path, lexical_fts.content,
                    d.source, d.node_id, d.symbol_name, d.start_line
             FROM lexical_fts
             JOIN lexical_documents d ON d.id = lexical_fts.rowid
             WHERE lexical_fts MATCH ?1
             ORDER BY bm25(lexical_fts, 1.0, 4.0), d.path, d.id
             LIMIT ?2"
        }
        LexicalCandidateOrder::SymbolDocument => {
            "SELECT d.path, d.content, lexical_fts.path, lexical_fts.content,
                    d.source, d.node_id, d.symbol_name, d.start_line
             FROM lexical_fts
             JOIN lexical_documents d ON d.id = lexical_fts.rowid
             WHERE lexical_fts MATCH ?1 AND d.source = 'symbol_doc'
             ORDER BY bm25(lexical_fts, 1.0, 4.0), d.path, d.id
             LIMIT ?2"
        }
    };
    let mut statement = connection.prepare_cached(sql)?;
    let rows = statement.query_map(params![scoped_query, candidate_limit as i64], |row| {
        let document = LexicalDocument {
            path: row.get(0)?,
            content: row.get(1)?,
            source: LexicalDocumentSource::parse(&row.get::<_, String>(4)?).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    error.into(),
                )
            })?,
            node_id: row.get(5)?,
            symbol_name: row.get(6)?,
            start_line: row.get(7)?,
        };
        Ok((document, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn query_exact_candidates(
    connection: &Connection,
    query: &str,
    candidate_limit: usize,
) -> Result<Vec<LexicalCandidate>> {
    let mut needles = quoted_query_tokens(query);
    let intent = crate::query_features::classify_query(query).intent;
    needles.extend(
        intent
            .exact_symbols
            .into_iter()
            .chain(intent.paths)
            .map(|needle| needle.to_ascii_lowercase()),
    );
    needles.sort();
    needles.dedup();

    let mut candidates = Vec::new();
    let mut statement = connection.prepare_cached(
        "SELECT d.path, d.content, lower(lexical_fts.path), lower(lexical_fts.content),
                d.source, d.node_id, d.symbol_name, d.start_line
         FROM lexical_documents d
         JOIN lexical_fts ON lexical_fts.rowid = d.id
         WHERE lower(d.path) = ?1
            OR lower(d.symbol_name) = ?1
            OR lower(d.symbol_name) LIKE '%::' || ?1
            OR lower(d.symbol_name) LIKE '%.' || ?1
         ORDER BY d.path, d.source, d.node_id, d.symbol_name, d.start_line, d.id
         LIMIT ?2",
    )?;
    for needle in needles {
        let rows = statement.query_map(params![needle, candidate_limit as i64], |row| {
            let document = LexicalDocument {
                path: row.get(0)?,
                content: row.get(1)?,
                source: LexicalDocumentSource::parse(&row.get::<_, String>(4)?).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            error.into(),
                        )
                    },
                )?,
                node_id: row.get(5)?,
                symbol_name: row.get(6)?,
                start_line: row.get(7)?,
            };
            Ok((document, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
        })?;
        candidates.extend(rows.collect::<std::result::Result<Vec<_>, _>>()?);
        if candidates.len() >= candidate_limit {
            break;
        }
    }
    candidates.truncate(candidate_limit);
    Ok(candidates)
}

fn interleave_candidate_lanes(lanes: Vec<Vec<LexicalCandidate>>) -> Vec<LexicalCandidate> {
    let mut lanes = lanes.into_iter().map(Vec::into_iter).collect::<Vec<_>>();
    let mut candidates = Vec::new();
    loop {
        let mut added = false;
        for lane in &mut lanes {
            if let Some(candidate) = lane.next() {
                candidates.push(candidate);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    candidates
}

type LexicalCandidateIdentity = (String, u8, Option<String>, Option<String>, Option<u32>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexicalSublane {
    Exact,
    Path,
    Content,
    SymbolDocument,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LexicalSublaneRanks {
    exact: Option<u32>,
    path: Option<u32>,
    content: Option<u32>,
    symbol_document: Option<u32>,
}

impl LexicalSublaneRanks {
    fn record(&mut self, lane: LexicalSublane, rank: u32) {
        let slot = match lane {
            LexicalSublane::Exact => &mut self.exact,
            LexicalSublane::Path => &mut self.path,
            LexicalSublane::Content => &mut self.content,
            LexicalSublane::SymbolDocument => &mut self.symbol_document,
        };
        *slot = Some(slot.map_or(rank, |current| current.min(rank)));
    }
}

fn record_lexical_sublane_ranks(
    ranks: &mut HashMap<LexicalCandidateIdentity, LexicalSublaneRanks>,
    candidates: &[LexicalCandidate],
    lane: LexicalSublane,
) {
    for (index, (document, _, _)) in candidates.iter().enumerate() {
        let candidate_rank = u32::try_from(index + 1).unwrap_or(u32::MAX);
        ranks
            .entry(lexical_candidate_identity(document))
            .or_default()
            .record(lane, candidate_rank);
    }
}

fn rank_symbol_candidates_by_identifier_overlap(
    candidates: &mut [LexicalCandidate],
    tokens: &[String],
    token_weights: &[f32],
) {
    let original_order = candidates
        .iter()
        .enumerate()
        .map(|(index, (document, _, _))| (lexical_candidate_identity(document), index))
        .collect::<HashMap<_, _>>();
    candidates.sort_by(|left, right| {
        symbol_identifier_overlap(&right.0, tokens, token_weights)
            .partial_cmp(&symbol_identifier_overlap(&left.0, tokens, token_weights))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                original_order[&lexical_candidate_identity(&left.0)]
                    .cmp(&original_order[&lexical_candidate_identity(&right.0)])
            })
    });
}

fn symbol_identifier_overlap(
    document: &LexicalDocument,
    tokens: &[String],
    token_weights: &[f32],
) -> f32 {
    let Some(symbol_name) = document.symbol_name.as_deref() else {
        return 0.0;
    };
    let normalized = normalize_lexical_text(symbol_name);
    let symbol_tokens = normalized.split_whitespace().collect::<HashSet<_>>();
    tokens
        .iter()
        .zip(token_weights.iter().copied())
        .filter_map(|(token, weight)| symbol_tokens.contains(token.as_str()).then_some(weight))
        .sum()
}

fn lexical_sublane_score(
    ranks: LexicalSublaneRanks,
    shape: crate::query_features::QueryShape,
) -> f32 {
    // Fuse only deterministic ranks. BM25 values from different FTS column
    // weightings are not comparable, and aggregate file coverage must not
    // erase a focused symbol-document signal. k=20 matches the outer hybrid
    // ranker; the denominator normalizes a candidate ranked first in every
    // lexical sublane to 1.0.
    const RRF_K: f32 = 20.0;
    let (exact_weight, path_weight, content_weight, symbol_weight) = match shape {
        crate::query_features::QueryShape::PathLike => (2.0, 1.25, 1.0, 1.0),
        crate::query_features::QueryShape::SymbolLike => (2.0, 0.75, 1.0, 1.5),
        crate::query_features::QueryShape::NaturalLanguage
        | crate::query_features::QueryShape::Mixed => (2.0, 1.0, 1.0, 1.25),
    };
    let weighted_rank =
        |rank: Option<u32>, weight: f32| rank.map_or(0.0, |rank| weight / (RRF_K + rank as f32));
    let score = weighted_rank(ranks.exact, exact_weight)
        + weighted_rank(ranks.path, path_weight)
        + weighted_rank(ranks.content, content_weight)
        + weighted_rank(ranks.symbol_document, symbol_weight);
    let maximum = (exact_weight + path_weight + content_weight + symbol_weight) / (RRF_K + 1.0);
    (score / maximum).clamp(0.0, 1.0)
}

fn lexical_candidate_identity(document: &LexicalDocument) -> LexicalCandidateIdentity {
    let source = match document.source {
        LexicalDocumentSource::LexicalSource => 0,
        LexicalDocumentSource::SymbolDoc => 1,
        LexicalDocumentSource::ComponentReport => 2,
    };
    (
        document.path.clone(),
        source,
        document.node_id.clone(),
        document.symbol_name.clone(),
        document.start_line,
    )
}

pub fn shard_dir_for(lexical_data_dir: &Path, project_id: &str) -> PathBuf {
    lexical_data_dir.join("shards").join(project_id)
}

fn prepared_lexical_state(expected: &PreparedLexicalInput) -> Result<Arc<LexicalLogicalState>> {
    if let Some(state) = expected.bounded_state.as_ref() {
        if state.fingerprint != expected.fingerprint
            || lexical_state_digest(&state.fingerprint, &state.documents) != state.state_sha256
        {
            bail!("bounded lexical state does not match its fingerprint");
        }
        return Ok(Arc::clone(state));
    }
    let mut documents = BTreeMap::new();
    for document in &expected.documents {
        let key = lexical_document_key(document)?;
        if documents
            .insert(key.clone(), lexical_document_hash(document))
            .is_some()
        {
            bail!("duplicate prepared lexical document key {key:?}");
        }
    }
    if documents.len() != expected.fingerprint.file_count as usize {
        bail!("prepared lexical state count does not match its fingerprint");
    }
    let state_sha256 = lexical_state_digest(&expected.fingerprint, &documents);
    Ok(Arc::new(LexicalLogicalState {
        fingerprint: expected.fingerprint.clone(),
        documents,
        state_sha256,
    }))
}

fn lexical_state_delta<'a>(
    previous: &LexicalLogicalState,
    desired: &LexicalLogicalState,
    documents: &'a [LexicalDocument],
) -> Result<LexicalStateDelta<'a>> {
    let by_key = documents
        .iter()
        .map(|document| Ok((lexical_document_key(document)?, document)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    for (key, document) in &by_key {
        if desired.documents.get(key) != Some(&lexical_document_hash(document)) {
            bail!("prepared lexical delta document does not match desired logical state");
        }
    }
    let upserts = desired
        .documents
        .iter()
        .filter(|(key, hash)| previous.documents.get(*key) != Some(*hash))
        .map(|(key, _)| {
            by_key
                .get(key)
                .copied()
                .map(|document| (key.clone(), document))
                .context("desired lexical state is missing its source document")
        })
        .collect::<Result<Vec<_>>>()?;
    let tombstones = previous
        .documents
        .keys()
        .filter(|key| !desired.documents.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    Ok(LexicalStateDelta {
        retained: u64::try_from(desired.documents.len().saturating_sub(upserts.len()))
            .unwrap_or(u64::MAX),
        upserts,
        tombstones,
    })
}

fn lexical_state_digest(
    fingerprint: &LexicalInputFingerprint,
    documents: &BTreeMap<String, String>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codestory-lexical-state-v1\0");
    hasher.update(fingerprint.file_count.to_le_bytes());
    {
        let value = fingerprint.hash.as_str();
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    let coverage = serde_json::to_vec(&fingerprint.coverage)
        .expect("lexical coverage serialization is infallible");
    hasher.update((coverage.len() as u64).to_le_bytes());
    hasher.update(coverage);
    for (key, hash) in documents {
        for value in [key, hash] {
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
    }
    format!("{:x}", hasher.finalize())
}

fn lexical_state_metadata_binding(metadata: &LexicalStateDatabaseMetadata) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codestory-lexical-state-metadata-v1\0");
    hasher.update(metadata.fingerprint.file_count.to_le_bytes());
    for value in [
        metadata.fingerprint.hash.as_str(),
        metadata.state_sha256.as_str(),
    ] {
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    let coverage = serde_json::to_vec(&metadata.fingerprint.coverage)
        .expect("lexical coverage serialization is infallible");
    hasher.update((coverage.len() as u64).to_le_bytes());
    hasher.update(coverage);
    format!("{:x}", hasher.finalize())
}

fn write_lexical_state_database(path: &Path, state: &LexicalLogicalState) -> Result<()> {
    let mut connection = Connection::open(sqlite_open_path(path))
        .with_context(|| format!("create lexical state database {}", path.display()))?;
    connection.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = FULL;
         PRAGMA user_version = 2;
         CREATE TABLE lexical_state_documents (
             document_key TEXT PRIMARY KEY NOT NULL,
             document_hash TEXT NOT NULL
         ) WITHOUT ROWID;
         CREATE TABLE lexical_state_metadata (
             singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
             lexical_hash TEXT NOT NULL,
             file_count INTEGER NOT NULL,
             coverage_json TEXT NOT NULL,
             state_sha256 TEXT NOT NULL,
             binding_sha256 TEXT NOT NULL
         );",
    )?;
    let transaction = connection.transaction()?;
    {
        let mut insert = transaction.prepare(
            "INSERT INTO lexical_state_documents (document_key, document_hash) VALUES (?1, ?2)",
        )?;
        for (key, hash) in &state.documents {
            insert.execute(params![key, hash])?;
        }
    }
    write_lexical_state_metadata(&transaction, state)?;
    install_lexical_state_immutability(&transaction)?;
    transaction.commit()?;
    connection.execute_batch("PRAGMA optimize;")?;
    drop(connection);
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .sync_all()?;
    Ok(())
}

fn write_lexical_state_metadata(
    connection: &Connection,
    state: &LexicalLogicalState,
) -> Result<()> {
    let mut metadata = LexicalStateDatabaseMetadata {
        fingerprint: state.fingerprint.clone(),
        state_sha256: state.state_sha256.clone(),
        binding_sha256: String::new(),
    };
    metadata.binding_sha256 = lexical_state_metadata_binding(&metadata);
    connection.execute("DELETE FROM lexical_state_metadata", [])?;
    connection.execute(
        "INSERT INTO lexical_state_metadata
         (singleton, lexical_hash, file_count, coverage_json, state_sha256, binding_sha256)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        params![
            metadata.fingerprint.hash,
            metadata.fingerprint.file_count,
            serde_json::to_string(&metadata.fingerprint.coverage)?,
            metadata.state_sha256,
            metadata.binding_sha256,
        ],
    )?;
    Ok(())
}

fn install_lexical_state_immutability(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TRIGGER lexical_state_documents_no_insert
             BEFORE INSERT ON lexical_state_documents
             BEGIN SELECT RAISE(ABORT, 'immutable lexical state'); END;
         CREATE TRIGGER lexical_state_documents_no_update
             BEFORE UPDATE ON lexical_state_documents
             BEGIN SELECT RAISE(ABORT, 'immutable lexical state'); END;
         CREATE TRIGGER lexical_state_documents_no_delete
             BEFORE DELETE ON lexical_state_documents
             BEGIN SELECT RAISE(ABORT, 'immutable lexical state'); END;
         CREATE TRIGGER lexical_state_metadata_no_insert
             BEFORE INSERT ON lexical_state_metadata
             BEGIN SELECT RAISE(ABORT, 'immutable lexical state'); END;
         CREATE TRIGGER lexical_state_metadata_no_update
             BEFORE UPDATE ON lexical_state_metadata
             BEGIN SELECT RAISE(ABORT, 'immutable lexical state'); END;
         CREATE TRIGGER lexical_state_metadata_no_delete
             BEFORE DELETE ON lexical_state_metadata
             BEGIN SELECT RAISE(ABORT, 'immutable lexical state'); END;",
    )?;
    Ok(())
}

fn reconcile_cloned_lexical_state_database(
    path: &Path,
    desired: &LexicalLogicalState,
    delta: &LexicalStateDelta<'_>,
) -> Result<()> {
    let mut connection = Connection::open(sqlite_open_path(path))?;
    connection.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = FULL;
         DROP TRIGGER lexical_state_documents_no_insert;
         DROP TRIGGER lexical_state_documents_no_update;
         DROP TRIGGER lexical_state_documents_no_delete;
         DROP TRIGGER lexical_state_metadata_no_insert;
         DROP TRIGGER lexical_state_metadata_no_update;
         DROP TRIGGER lexical_state_metadata_no_delete;",
    )?;
    let transaction = connection.transaction()?;
    for key in &delta.tombstones {
        transaction.execute(
            "DELETE FROM lexical_state_documents WHERE document_key = ?1",
            params![key],
        )?;
    }
    for (key, document) in &delta.upserts {
        transaction.execute(
            "INSERT INTO lexical_state_documents (document_key, document_hash)
             VALUES (?1, ?2)
             ON CONFLICT(document_key) DO UPDATE SET document_hash = excluded.document_hash",
            params![key, lexical_document_hash(document)],
        )?;
    }
    write_lexical_state_metadata(&transaction, desired)?;
    install_lexical_state_immutability(&transaction)?;
    transaction.commit()?;
    drop(connection);
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .sync_all()?;
    Ok(())
}

fn read_lexical_state_database(path: &Path) -> Result<Arc<LexicalLogicalState>> {
    let connection = open_read_only(path)?;
    let schema_version: i32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if schema_version != LEXICAL_STATE_SCHEMA_VERSION {
        bail!("lexical state database schema version is not current");
    }
    let quick_check: String =
        connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if quick_check != "ok" {
        bail!("lexical state database failed quick_check: {quick_check}");
    }
    let metadata = connection
        .query_row(
            "SELECT lexical_hash, file_count, coverage_json, state_sha256, binding_sha256
             FROM lexical_state_metadata WHERE singleton = 1",
            [],
            |row| {
                let coverage_json: String = row.get(2)?;
                Ok(LexicalStateDatabaseMetadata {
                    fingerprint: LexicalInputFingerprint {
                        hash: row.get(0)?,
                        file_count: row.get(1)?,
                        coverage: serde_json::from_str(&coverage_json).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                error.into(),
                            )
                        })?,
                    },
                    state_sha256: row.get(3)?,
                    binding_sha256: row.get(4)?,
                })
            },
        )
        .optional()?
        .context("lexical state database metadata is missing")?;
    if metadata.binding_sha256 != lexical_state_metadata_binding(&metadata) {
        bail!("lexical state database metadata binding is invalid");
    }
    let mut statement = connection.prepare(
        "SELECT document_key, document_hash FROM lexical_state_documents ORDER BY document_key",
    )?;
    let mut rows = statement.query([])?;
    let mut documents = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let key = row.get::<_, String>(0)?;
        let hash = row.get::<_, String>(1)?;
        if key.is_empty() || hash.len() != 64 || documents.insert(key, hash).is_some() {
            bail!("lexical state database contains an invalid document identity");
        }
    }
    if documents.len() != metadata.fingerprint.file_count as usize
        || lexical_state_digest(&metadata.fingerprint, &documents) != metadata.state_sha256
    {
        bail!("lexical state database content digest does not match metadata");
    }
    Ok(Arc::new(LexicalLogicalState {
        fingerprint: metadata.fingerprint,
        documents,
        state_sha256: metadata.state_sha256,
    }))
}

fn load_lexical_state_database(
    path: &Path,
    expected_fingerprint: &LexicalInputFingerprint,
    expected_state_sha256: &str,
) -> Result<Arc<LexicalLogicalState>> {
    let state = LEXICAL_STATE_RECEIPTS.validate_sealed(
        path.to_path_buf(),
        &sqlite_file_with_sidecars(path),
        || read_lexical_state_database(path),
    )?;
    if &state.fingerprint != expected_fingerprint || state.state_sha256 != expected_state_sha256 {
        bail!("lexical state database does not match its component-set manifest");
    }
    Ok(state)
}

fn legacy_lexical_state(path: &Path) -> Result<Arc<LexicalLogicalState>> {
    let metadata = verify_lexical_database_contents(path)?;
    let connection = open_read_only(path)?;
    let mut statement = connection.prepare(
        "SELECT document_key, document_hash FROM lexical_documents ORDER BY document_key",
    )?;
    let mut rows = statement.query([])?;
    let mut documents = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let key = row.get::<_, String>(0)?;
        let hash = row.get::<_, String>(1)?;
        if documents.insert(key.clone(), hash).is_some() {
            bail!("legacy lexical database contains duplicate key {key:?}");
        }
    }
    let fingerprint = LexicalInputFingerprint {
        file_count: metadata.file_count,
        hash: metadata.lexical_hash,
        coverage: metadata.coverage,
    };
    let state_sha256 = lexical_state_digest(&fingerprint, &documents);
    Ok(Arc::new(LexicalLogicalState {
        fingerprint,
        documents,
        state_sha256,
    }))
}

fn write_lexical_database<F>(
    path: &Path,
    project_id: &str,
    sidecar_input_hash: &str,
    expected: &LexicalInputFingerprint,
    scan: F,
) -> Result<LexicalInputFingerprint>
where
    F: FnOnce(&mut dyn FnMut(&LexicalDocument) -> Result<()>) -> Result<LexicalCoverage>,
{
    let mut connection = Connection::open(sqlite_open_path(path))
        .with_context(|| format!("create lexical SQLite shard {}", path.display()))?;
    connection.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = FULL;
         PRAGMA temp_store = MEMORY;
         PRAGMA user_version = 2;
         CREATE TABLE lexical_metadata (
             id INTEGER PRIMARY KEY CHECK (id = 1),
             version TEXT NOT NULL,
             project_id TEXT NOT NULL,
             sidecar_input_hash TEXT NOT NULL,
             lexical_hash TEXT NOT NULL,
             file_count INTEGER NOT NULL,
             coverage_json TEXT NOT NULL,
             binding_sha256 TEXT NOT NULL,
             indexed_at_epoch_ms INTEGER NOT NULL
         );
         CREATE TABLE lexical_documents (
             id INTEGER PRIMARY KEY,
             document_key TEXT NOT NULL UNIQUE,
             document_hash TEXT NOT NULL,
             path TEXT NOT NULL,
             content TEXT NOT NULL,
             source TEXT NOT NULL,
             node_id TEXT,
             symbol_name TEXT,
             start_line INTEGER
         );
         CREATE VIRTUAL TABLE lexical_fts USING fts5(path, content);",
    )?;
    let transaction = connection.transaction()?;
    let mut document_hashes = BTreeMap::new();
    let mut stable_ids = HashMap::new();
    let actual = {
        let mut insert_document = transaction.prepare(
            "INSERT INTO lexical_documents
             (id, document_key, document_hash, path, content, source, node_id, symbol_name,
              start_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        let mut insert_fts = transaction
            .prepare("INSERT INTO lexical_fts(rowid, path, content) VALUES (?1, ?2, ?3)")?;
        let coverage = scan(&mut |document| {
            let document_key = lexical_document_key(document)?;
            let document_hash = lexical_document_hash(document);
            let id = stable_lexical_document_id(&document_key);
            if document_hashes
                .insert(document_key.clone(), document_hash.clone())
                .is_some()
            {
                bail!("duplicate lexical document key");
            }
            if let Some(previous) = stable_ids.insert(id, document_key.clone()) {
                bail!(
                    "lexical document identity collision between {previous:?} and {document_key:?}"
                );
            }
            insert_document.execute(params![
                id,
                document_key,
                document_hash,
                document.path,
                document.content,
                document.source.provenance_label(),
                document.node_id,
                document.symbol_name,
                document.start_line,
            ])?;
            insert_fts.execute(params![
                id,
                normalize_lexical_text(&document.path),
                normalize_lexical_text(&document.content),
            ])?;
            Ok(())
        })?;
        drop(insert_fts);
        drop(insert_document);

        let actual = lexical_fingerprint_from_document_hashes(&document_hashes, &coverage)?;
        if &actual != expected {
            return Err(crate::index::SidecarInputChanged::new(
                "lexical shard build",
                format!(
                    "{} documents with hash {}",
                    expected.file_count, expected.hash
                ),
                format!("{} documents with hash {}", actual.file_count, actual.hash),
            )
            .into());
        }
        let coverage_json = serde_json::to_string(&actual.coverage)?;
        let binding = metadata_binding(
            project_id,
            sidecar_input_hash,
            &actual.hash,
            actual.file_count,
            &coverage_json,
        );
        transaction.execute(
            "INSERT INTO lexical_metadata
             (id, version, project_id, sidecar_input_hash, lexical_hash, file_count,
              coverage_json, binding_sha256, indexed_at_epoch_ms)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                LEXICAL_INDEX_VERSION,
                project_id,
                sidecar_input_hash,
                actual.hash,
                actual.file_count,
                coverage_json,
                binding,
                chrono::Utc::now().timestamp_millis(),
            ],
        )?;
        actual
    };
    transaction.execute_batch(
        "CREATE TRIGGER lexical_documents_no_insert BEFORE INSERT ON lexical_documents
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;
         CREATE TRIGGER lexical_documents_no_update BEFORE UPDATE ON lexical_documents
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;
         CREATE TRIGGER lexical_documents_no_delete BEFORE DELETE ON lexical_documents
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;
         CREATE TRIGGER lexical_metadata_no_insert BEFORE INSERT ON lexical_metadata
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;
         CREATE TRIGGER lexical_metadata_no_update BEFORE UPDATE ON lexical_metadata
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;
         CREATE TRIGGER lexical_metadata_no_delete BEFORE DELETE ON lexical_metadata
         BEGIN SELECT RAISE(ABORT, 'immutable lexical generation'); END;",
    )?;
    transaction.commit()?;
    connection.execute_batch("PRAGMA optimize;")?;
    connection.close().map_err(|(_, error)| error)?;
    Ok(actual)
}

fn prepared_lexical_fingerprint(
    documents: &[LexicalDocument],
    coverage: &LexicalCoverage,
) -> Result<LexicalInputFingerprint> {
    let mut hashes = BTreeMap::new();
    let mut ids = HashMap::new();
    for document in documents {
        let key = lexical_document_key(document)?;
        if hashes
            .insert(key.clone(), lexical_document_hash(document))
            .is_some()
        {
            bail!("duplicate prepared lexical document key");
        }
        let id = stable_lexical_document_id(&key);
        if let Some(previous) = ids.insert(id, key.clone()) {
            bail!("lexical document identity collision between {previous:?} and {key:?}");
        }
    }
    lexical_fingerprint_from_document_hashes(&hashes, coverage)
}

/// Deep-verify one immutable lexical shard, reusing a sealed receipt when the
/// shard's native identity is unchanged, then check the caller's expectations
/// against the receipted metadata.
///
/// The two halves are deliberately separate. The deep half scans the whole FTS
/// mirror and is a fact about the artifact, so it is receiptable. The
/// expectation half is three string comparisons that depend on the caller, so
/// it runs every time and can never be answered from a receipt.
fn validate_lexical_database(
    path: &Path,
    expected_project_id: &str,
    expected_sidecar_input_hash: &str,
    expected_lexical: Option<(u32, &str)>,
) -> Result<LexicalShardMetadata> {
    let shard_dir = path
        .parent()
        .context("lexical shard has no generation directory")?;
    let metadata = LEXICAL_SHARD_RECEIPTS.validate_sealed(
        path.to_path_buf(),
        &sqlite_file_with_sidecars(path),
        || verify_lexical_database_contents(path),
    )?;
    let envelope = resolve_lexical_component_envelope(
        shard_dir,
        expected_project_id,
        expected_sidecar_input_hash,
        &metadata,
    )?;
    if !envelope.matches_physical(&metadata) {
        bail!("lexical component envelope does not match its physical database");
    }
    if let Some((file_count, lexical_hash)) = expected_lexical
        && (metadata.file_count != file_count || metadata.lexical_hash != lexical_hash)
    {
        bail!("lexical SQLite shard does not match current lexical input");
    }
    Ok(metadata)
}

/// The receiptable half: everything that depends only on the shard's own bytes.
///
/// `quick_check` is unconditional here. It used to be opt-in so that the cheap
/// callers could skip it, but with the verdict sealed to native identity the
/// page-level check is paid once per generation, and the strongest verdict is
/// the only one worth sealing.
fn verify_lexical_database_contents(path: &Path) -> Result<LexicalShardMetadata> {
    if !path.is_file() {
        bail!("lexical SQLite shard is missing");
    }
    let connection = open_read_only(path)?;
    verify_open_database_contents(&connection, &|| false)
}

fn verify_open_database_contents(
    connection: &Connection,
    cancelled: &dyn Fn() -> bool,
) -> Result<LexicalShardMetadata> {
    let metadata = read_open_database_metadata(connection, cancelled)?;
    let check: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if check != "ok" {
        bail!("lexical SQLite shard failed quick_check: {check}");
    }
    let actual_count: u32 =
        connection.query_row("SELECT count(*) FROM lexical_documents", [], |row| {
            row.get(0)
        })?;
    if actual_count != metadata.file_count {
        bail!(
            "lexical SQLite shard row count mismatch: metadata={}, actual={actual_count}",
            metadata.file_count
        );
    }
    let fts_count: u32 =
        connection.query_row("SELECT count(*) FROM lexical_fts", [], |row| row.get(0))?;
    if fts_count != actual_count {
        bail!(
            "lexical SQLite shard FTS row count mismatch: documents={actual_count}, fts={fts_count}"
        );
    }
    let mut rows = connection.prepare(
        "SELECT d.path, d.content, f.path, f.content
         FROM lexical_documents d
         LEFT JOIN lexical_fts f ON f.rowid = d.id
         ORDER BY d.id",
    )?;
    let mut rows = rows.query([])?;
    let mut row_index = 0_usize;
    while let Some(row) = rows.next()? {
        if row_index.is_multiple_of(64) && cancelled() {
            bail!("lexical validation cancelled");
        }
        row_index += 1;
        let path: String = row.get(0)?;
        let content: String = row.get(1)?;
        let fts_path: Option<String> = row.get(2)?;
        let fts_content: Option<String> = row.get(3)?;
        if fts_path != Some(normalize_lexical_text(&path))
            || fts_content != Some(normalize_lexical_text(&content))
        {
            bail!("lexical SQLite shard FTS rows do not match immutable documents");
        }
    }
    if cancelled() {
        bail!("lexical validation cancelled");
    }
    Ok(metadata)
}

fn validate_open_database_metadata(
    connection: &Connection,
    shard_dir: &Path,
    expected_project_id: &str,
    expected_sidecar_input_hash: &str,
    expected_lexical: Option<(u32, &str)>,
    cancelled: &dyn Fn() -> bool,
) -> Result<LexicalShardMetadata> {
    let metadata = read_open_database_metadata(connection, cancelled)?;
    let envelope = resolve_lexical_component_envelope(
        shard_dir,
        expected_project_id,
        expected_sidecar_input_hash,
        &metadata,
    )?;
    if !envelope.matches_physical(&metadata) {
        bail!("lexical component envelope does not match its physical database");
    }
    if let Some((file_count, lexical_hash)) = expected_lexical
        && (metadata.file_count != file_count || metadata.lexical_hash != lexical_hash)
    {
        bail!("lexical SQLite shard does not match current lexical input");
    }
    Ok(metadata)
}

/// Read the shard's own metadata row and check it is internally consistent.
///
/// Nothing here depends on the caller, which is what makes the verdict
/// receiptable.
fn read_open_database_metadata(
    connection: &Connection,
    cancelled: &dyn Fn() -> bool,
) -> Result<LexicalShardMetadata> {
    if cancelled() {
        bail!("lexical search cancelled");
    }
    let schema_version: i32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if schema_version != 2 {
        bail!("lexical SQLite shard schema version is not current");
    }
    let required_tables: u32 = connection.query_row(
        "SELECT count(*) FROM sqlite_master
         WHERE type IN ('table', 'view')
           AND name IN ('lexical_metadata', 'lexical_documents', 'lexical_fts')",
        [],
        |row| row.get(0),
    )?;
    if required_tables != 3 {
        bail!("lexical SQLite shard schema is incomplete");
    }
    let metadata = connection
        .query_row(
            "SELECT version, project_id, sidecar_input_hash, lexical_hash, file_count,
                    coverage_json, binding_sha256
             FROM lexical_metadata WHERE id = 1",
            [],
            |row| {
                let coverage_json: String = row.get(5)?;
                let coverage = serde_json::from_str(&coverage_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        error.into(),
                    )
                })?;
                Ok((
                    row.get::<_, String>(0)?,
                    LexicalShardMetadata {
                        project_id: row.get(1)?,
                        sidecar_input_hash: row.get(2)?,
                        lexical_hash: row.get(3)?,
                        file_count: row.get(4)?,
                        coverage,
                        binding_sha256: row.get(6)?,
                    },
                    coverage_json,
                ))
            },
        )
        .optional()?
        .context("lexical SQLite shard metadata is missing")?;
    let (version, metadata, coverage_json) = metadata;
    if version != LEXICAL_INDEX_VERSION {
        bail!("lexical SQLite shard version is not current");
    }
    if metadata.binding_sha256
        != metadata_binding(
            &metadata.project_id,
            &metadata.sidecar_input_hash,
            &metadata.lexical_hash,
            metadata.file_count,
            &coverage_json,
        )
    {
        bail!("lexical SQLite shard metadata binding is invalid");
    }
    Ok(metadata)
}

/// The caller-dependent half: never receipted, always re-checked.
#[cfg(any(test, feature = "test-support"))]
fn match_lexical_shard_expectations(
    metadata: &LexicalShardMetadata,
    expected_project_id: &str,
    expected_sidecar_input_hash: &str,
    expected_lexical: Option<(u32, &str)>,
) -> Result<()> {
    if metadata.project_id != expected_project_id {
        bail!("lexical SQLite shard project id does not match its generation directory");
    }
    if metadata.sidecar_input_hash != expected_sidecar_input_hash {
        bail!("lexical SQLite shard does not match the sidecar input hash");
    }
    if let Some((file_count, lexical_hash)) = expected_lexical
        && (metadata.file_count != file_count || metadata.lexical_hash != lexical_hash)
    {
        bail!("lexical SQLite shard does not match current lexical input");
    }
    Ok(())
}

fn open_read_only(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(
        sqlite_open_path(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open lexical SQLite shard {}", path.display()))?;
    connection.execute_batch("PRAGMA query_only = ON;")?;
    Ok(connection)
}

fn metadata_binding(
    project_id: &str,
    sidecar_input_hash: &str,
    lexical_hash: &str,
    file_count: u32,
    coverage_json: &str,
) -> String {
    let mut hasher = Sha256::new();
    for value in [
        LEXICAL_INDEX_VERSION,
        project_id,
        sidecar_input_hash,
        lexical_hash,
        coverage_json,
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    hasher.update(file_count.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

fn lexical_component_envelope_binding(envelope: &LexicalComponentEnvelope) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codestory-lexical-component-envelope-v1\0");
    for value in [
        envelope.generation.as_str(),
        envelope.sidecar_input_hash.as_str(),
        envelope.physical_project_id.as_str(),
        envelope.physical_input_hash.as_str(),
        envelope.lexical_hash.as_str(),
    ] {
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.update(envelope.file_count.to_le_bytes());
    let coverage = serde_json::to_vec(&envelope.coverage)
        .expect("lexical coverage serialization is infallible");
    hasher.update((coverage.len() as u64).to_le_bytes());
    hasher.update(coverage);
    format!("{:x}", hasher.finalize())
}

fn read_lexical_component_envelope(
    shard_dir: &Path,
    expected_generation: Option<&str>,
    expected_sidecar_input_hash: Option<&str>,
) -> Result<LexicalComponentEnvelope> {
    let path = shard_dir.join(LEXICAL_COMPONENT_ENVELOPE_FILE);
    let envelope: LexicalComponentEnvelope = serde_json::from_slice(
        &std::fs::read(&path)
            .with_context(|| format!("read lexical component envelope {}", path.display()))?,
    )
    .with_context(|| format!("parse lexical component envelope {}", path.display()))?;
    envelope.validate()?;
    if expected_generation.is_some_and(|expected| envelope.generation != expected)
        || expected_sidecar_input_hash
            .is_some_and(|expected| envelope.sidecar_input_hash != expected)
    {
        bail!("lexical component envelope does not match the retrieval publication");
    }
    Ok(envelope)
}

fn resolve_lexical_component_envelope(
    shard_dir: &Path,
    expected_generation: &str,
    expected_sidecar_input_hash: &str,
    physical: &LexicalShardMetadata,
) -> Result<LexicalComponentEnvelope> {
    let path = shard_dir.join(LEXICAL_COMPONENT_ENVELOPE_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                bail!("lexical component envelope is not a regular file");
            }
            read_lexical_component_envelope(
                shard_dir,
                Some(expected_generation),
                Some(expected_sidecar_input_hash),
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if physical.project_id != expected_generation
                || physical.sidecar_input_hash != expected_sidecar_input_hash
            {
                bail!("legacy lexical component is not bound to the retrieval publication");
            }
            Ok(LexicalComponentEnvelope::new(
                expected_generation,
                expected_sidecar_input_hash,
                physical,
            ))
        }
        Err(error) => Err(error)
            .with_context(|| format!("inspect lexical component envelope {}", path.display())),
    }
}

fn publish_lexical_component_envelope(
    shard_dir: &Path,
    envelope: &LexicalComponentEnvelope,
) -> Result<()> {
    envelope.validate()?;
    let path = shard_dir.join(LEXICAL_COMPONENT_ENVELOPE_FILE);
    let bytes = serde_json::to_vec_pretty(envelope)?;
    codestory_workspace::atomic_file::write_file_atomic(
        &path,
        "lexical-component-envelope",
        |file| {
            use std::io::Write;
            file.write_all(&bytes)?;
            Ok(())
        },
        |temp_path| {
            let observed: LexicalComponentEnvelope =
                serde_json::from_slice(&std::fs::read(temp_path)?)?;
            observed.validate()?;
            if &observed != envelope {
                bail!("staged lexical component envelope changed before publication");
            }
            Ok(())
        },
    )
}

fn scan_lexical_documents(
    project_root: &Path,
    source_storage_path: Option<&Path>,
    symbol_storage_path: Option<&Path>,
    visit: &mut dyn FnMut(&LexicalDocument) -> Result<()>,
) -> Result<LexicalScanOutcome> {
    let source_policy = lexical_source_policy(project_root, source_storage_path)?;
    let workspace = match source_storage_path {
        Some(storage_path) => {
            codestory_workspace::WorkspaceManifest::open_with_storage_owned_exclusions(
                project_root.to_path_buf(),
                storage_path,
            )
        }
        None => codestory_workspace::WorkspaceManifest::open(project_root.to_path_buf()),
    }
    .context("open workspace for lexical discovery")?;
    let discovered = workspace
        .source_files()
        .context("discover canonical workspace files for lexical index")?;
    let mut coverage = LexicalCoverage::default();
    let mut source_seals = Vec::new();
    for path in discovered {
        let relative = lexical_relative_path(project_root, &path);
        if source_policy.excluded_paths.contains(&relative) {
            continue;
        }
        let before = ArtifactSeal::observe(&path)
            .with_context(|| format!("seal lexical source before read {}", path.display()))?;
        source_seals.push(before.clone());
        coverage.discovered_files = coverage.discovered_files.saturating_add(1);
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                coverage.unreadable_files = coverage.unreadable_files.saturating_add(1);
                push_coverage_sample(&mut coverage.unreadable_path_sample, relative);
                continue;
            }
        };
        if metadata.len() > source_policy.max_file_bytes {
            coverage.omitted_oversized = coverage.omitted_oversized.saturating_add(1);
            push_coverage_sample(&mut coverage.omitted_path_sample, relative);
            continue;
        }
        let content = match read_lexical_file_text_limited(&path, source_policy.max_file_bytes) {
            Ok(Some(content)) => content,
            Ok(None) => {
                coverage.omitted_oversized = coverage.omitted_oversized.saturating_add(1);
                push_coverage_sample(&mut coverage.omitted_path_sample, relative);
                continue;
            }
            Err(_) => {
                coverage.unreadable_files = coverage.unreadable_files.saturating_add(1);
                push_coverage_sample(&mut coverage.unreadable_path_sample, relative);
                continue;
            }
        };
        let after = ArtifactSeal::observe(&path)
            .with_context(|| format!("seal lexical source after read {}", path.display()))?;
        if after != before {
            bail!("lexical source changed while reading {}", path.display());
        }
        visit(&LexicalDocument {
            path: relative,
            content,
            source: LexicalDocumentSource::LexicalSource,
            node_id: None,
            symbol_name: None,
            start_line: None,
        })?;
        coverage.indexed_files = coverage.indexed_files.saturating_add(1);
    }
    scan_symbol_documents(project_root, symbol_storage_path, visit)?;
    Ok(LexicalScanOutcome {
        coverage,
        source_seals,
    })
}

fn observe_lexical_source_seals(
    project_root: &Path,
    source_storage_path: Option<&Path>,
) -> Result<Vec<ArtifactSeal>> {
    let source_policy = lexical_source_policy(project_root, source_storage_path)?;
    let workspace = match source_storage_path {
        Some(storage_path) => {
            codestory_workspace::WorkspaceManifest::open_with_storage_owned_exclusions(
                project_root.to_path_buf(),
                storage_path,
            )
        }
        None => codestory_workspace::WorkspaceManifest::open(project_root.to_path_buf()),
    }
    .context("open workspace for lexical source fence")?;
    workspace
        .source_files()
        .context("discover canonical workspace files for lexical source fence")?
        .into_iter()
        .filter(|path| {
            !source_policy
                .excluded_paths
                .contains(&lexical_relative_path(project_root, path))
        })
        .map(|path| {
            ArtifactSeal::observe(&path)
                .with_context(|| format!("revalidate lexical source identity {}", path.display()))
        })
        .collect()
}

fn read_lexical_file_text_limited(path: &Path, max_bytes: u64) -> std::io::Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    read_lexical_text_limited(file, max_bytes)
}

fn read_lexical_text_limited(reader: impl Read, max_bytes: u64) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    let mut reader = reader.take(max_bytes.saturating_add(1));
    reader.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Ok(None);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LexicalSourcePolicy {
    max_file_bytes: u64,
    policy_version: String,
    structural_unit_cap: u64,
    excluded_paths: HashSet<String>,
    exclusion_evidence: Vec<(String, String, u64, u64, String, u64, u64)>,
}

fn lexical_source_policy(
    project_root: &Path,
    source_storage_path: Option<&Path>,
) -> Result<LexicalSourcePolicy> {
    let Some(storage_path) = source_storage_path else {
        return Ok(LexicalSourcePolicy {
            max_file_bytes: MAX_FILE_BYTES,
            policy_version: codestory_contracts::workspace::OVERSIZED_SOURCE_POLICY_VERSION.into(),
            structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            excluded_paths: HashSet::new(),
            exclusion_evidence: Vec::new(),
        });
    };
    if !codestory_store::core_database_exists(storage_path)
        .context("resolve pinned core publication for lexical source policy")?
    {
        bail!(
            "pinned core storage for lexical source policy is missing: {}",
            storage_path.display()
        );
    }
    let storage =
        Store::open_read_only(storage_path).context("open storage for lexical source policy")?;
    let publication = storage
        .get_complete_index_publication()
        .context("load complete core publication for lexical source policy")?
        .context("complete core publication for lexical source policy is missing")?;
    let manifest = storage
        .get_source_policy_exclusion_manifest()
        .context("load lexical source policy manifest")?
        .context("lexical source policy manifest is missing")?;
    let records = storage
        .get_source_policy_exclusions()
        .context("load lexical source policy exclusions")?;
    let project_identity = codestory_workspace::project_identity_v3(project_root);
    let validated = storage
        .validate_source_policy_exclusion_publication(
            &publication,
            &project_identity.project_id,
            &project_identity.workspace_id,
            SourcePolicyExclusionPolicyIdentity::new(
                &manifest.policy_version,
                manifest.byte_cap,
                manifest.structural_unit_cap,
            ),
        )
        .context("validate lexical source policy publication")?;
    let confirmed_publication = storage
        .get_complete_index_publication()
        .context("confirm complete core publication for lexical source policy")?;
    let confirmed_manifest = storage
        .get_source_policy_exclusion_manifest()
        .context("confirm lexical source policy manifest")?;
    let confirmed_records = storage
        .get_source_policy_exclusions()
        .context("confirm lexical source policy exclusions")?;
    if validated != manifest
        || confirmed_publication.as_ref() != Some(&publication)
        || confirmed_manifest.as_ref() != Some(&manifest)
        || confirmed_records != records
    {
        bail!("lexical source policy publication changed while it was being pinned");
    }

    let mut exclusion_evidence = records
        .iter()
        .map(|record| {
            (
                record.normalized_path.clone(),
                record.content_hash.clone(),
                record.observed_size,
                record.observed_unit_count,
                record.policy_version.clone(),
                record.byte_cap,
                record.structural_unit_cap,
            )
        })
        .collect::<Vec<_>>();
    exclusion_evidence.sort();
    Ok(LexicalSourcePolicy {
        max_file_bytes: validated.byte_cap,
        policy_version: validated.policy_version,
        structural_unit_cap: validated.structural_unit_cap,
        excluded_paths: records
            .into_iter()
            .map(|record| record.normalized_path)
            .collect(),
        exclusion_evidence,
    })
}

fn scan_symbol_documents(
    project_root: &Path,
    storage_path: Option<&Path>,
    visit: &mut dyn FnMut(&LexicalDocument) -> Result<()>,
) -> Result<()> {
    let Some(storage_path) = storage_path.filter(|path| path.is_file()) else {
        return Ok(());
    };
    let storage = Store::open(storage_path).context("open storage for lexical symbol docs")?;
    scan_symbol_documents_from_store(project_root, &storage, visit)
}

fn scan_symbol_documents_from_store(
    project_root: &Path,
    storage: &Store,
    visit: &mut dyn FnMut(&LexicalDocument) -> Result<()>,
) -> Result<()> {
    let mut after = None;
    loop {
        let batch = storage
            .get_symbol_search_docs_batch_after(after, 4096)
            .context("load symbol search docs for lexical shard")?;
        if batch.is_empty() {
            break;
        }
        after = batch.last().map(|doc| doc.node_id);
        for doc in &batch {
            visit(&symbol_document(project_root, doc))?;
        }
    }
    Ok(())
}

fn symbol_document(project_root: &Path, doc: &SymbolSearchDoc) -> LexicalDocument {
    let source = if doc.display_name.starts_with("component_report:") {
        LexicalDocumentSource::ComponentReport
    } else {
        LexicalDocumentSource::SymbolDoc
    };
    let path = doc
        .file_path
        .as_deref()
        .and_then(|path| normalize_lexical_file_path(project_root, path))
        .unwrap_or_else(|| {
            format!(
                "codestory://{}",
                doc.display_name.replace([' ', '\t', '\r', '\n'], "_")
            )
        });
    LexicalDocument {
        path,
        content: doc.doc_text.clone(),
        source,
        node_id: Some(doc.node_id.0.to_string()),
        symbol_name: Some(doc.display_name.clone()),
        start_line: doc.start_line,
    }
}

fn lexical_relative_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalize_lexical_file_path(project_root: &Path, path: &str) -> Option<String> {
    let path = Path::new(path);
    if path.is_absolute() {
        path.strip_prefix(project_root)
            .ok()
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
    } else {
        Some(path.to_string_lossy().replace('\\', "/"))
    }
}

fn push_coverage_sample(sample: &mut Vec<String>, path: String) {
    if sample.len() < COVERAGE_PATH_SAMPLE {
        sample.push(path);
    }
}

fn hash_lexical_document(hasher: &mut Sha256, document: &LexicalDocument) {
    for value in [
        document.path.as_str(),
        document.content.as_str(),
        document.source.provenance_label(),
        document.node_id.as_deref().unwrap_or_default(),
        document.symbol_name.as_deref().unwrap_or_default(),
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    hasher.update(document.start_line.unwrap_or_default().to_le_bytes());
}

fn lexical_document_key(document: &LexicalDocument) -> Result<String> {
    match document.source {
        LexicalDocumentSource::LexicalSource => Ok(format!("source\0{}", document.path)),
        LexicalDocumentSource::SymbolDoc | LexicalDocumentSource::ComponentReport => {
            let node_id = document
                .node_id
                .as_deref()
                .context("symbol lexical document is missing its node identity")?;
            Ok(format!("{}\0{node_id}", document.source.provenance_label()))
        }
    }
}

fn lexical_document_hash(document: &LexicalDocument) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codestory-lexical-document-v2\0");
    hash_lexical_document(&mut hasher, document);
    format!("{:x}", hasher.finalize())
}

fn stable_lexical_document_id(document_key: &str) -> i64 {
    let bytes = Sha256::digest(document_key.as_bytes());
    let mut prefix = [0_u8; 8];
    prefix.copy_from_slice(&bytes[..8]);
    i64::from_le_bytes(prefix) & i64::MAX
}

fn lexical_fingerprint_from_document_hashes(
    documents: &BTreeMap<String, String>,
    coverage: &LexicalCoverage,
) -> Result<LexicalInputFingerprint> {
    let file_count = u32::try_from(documents.len()).context("lexical document count overflow")?;
    let mut hasher = Sha256::new();
    hasher.update(b"codestory-sqlite-lexical-key-hash-v2\0");
    hasher.update(LEXICAL_INDEX_VERSION.as_bytes());
    hasher.update(file_count.to_le_bytes());
    let coverage_bytes = serde_json::to_vec(coverage)?;
    hasher.update((coverage_bytes.len() as u64).to_le_bytes());
    hasher.update(coverage_bytes);
    for (key, document_hash) in documents {
        if key.is_empty()
            || document_hash.len() != 64
            || !document_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("lexical document state contains an invalid key or hash");
        }
        for value in [key.as_bytes(), document_hash.as_bytes()] {
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value);
        }
    }
    Ok(LexicalInputFingerprint {
        file_count,
        hash: format!("{:x}", hasher.finalize()),
        coverage: coverage.clone(),
    })
}

#[cfg(test)]
fn lexical_documents_hash(documents: &[LexicalDocument], coverage: &LexicalCoverage) -> String {
    prepared_lexical_fingerprint(documents, coverage)
        .expect("test lexical documents are valid")
        .hash
}

fn normalize_lexical_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len() + value.len() / 8);
    let mut characters = value.chars().peekable();
    let mut previous: Option<char> = None;
    while let Some(character) = characters.next() {
        let next = characters.peek().copied();
        if character.is_uppercase()
            && previous.is_some_and(|value: char| value.is_lowercase() || value.is_numeric())
            || character.is_uppercase()
                && previous.is_some_and(|value: char| value.is_uppercase())
                && next.is_some_and(|value| value.is_lowercase())
        {
            normalized.push(' ');
        }
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else {
            normalized.push(' ');
        }
        previous = Some(character);
    }
    normalized
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LexicalSourceToken {
    normalized: String,
    start_byte: usize,
    end_byte: usize,
}

fn lexical_source_match(value: &str, query_tokens: &[String]) -> Option<LexicalSourceToken> {
    let mut characters = value.char_indices().peekable();
    let mut current = String::new();
    let mut start_byte = None;
    let mut previous: Option<char> = None;

    while let Some((byte_index, character)) = characters.next() {
        let next = characters.peek().map(|(_, character)| *character);
        let camel_boundary = character.is_uppercase()
            && previous.is_some_and(|value| value.is_lowercase() || value.is_numeric())
            || character.is_uppercase()
                && previous.is_some_and(|value| value.is_uppercase())
                && next.is_some_and(|value| value.is_lowercase());
        if camel_boundary && !current.is_empty() {
            let token = LexicalSourceToken {
                normalized: std::mem::take(&mut current),
                start_byte: start_byte.take().expect("non-empty token has a start"),
                end_byte: byte_index,
            };
            if query_tokens
                .iter()
                .any(|query| token.normalized.starts_with(query))
            {
                return Some(token);
            }
        }
        if character.is_alphanumeric() {
            start_byte.get_or_insert(byte_index);
            current.extend(character.to_lowercase());
        } else if !current.is_empty() {
            let token = LexicalSourceToken {
                normalized: std::mem::take(&mut current),
                start_byte: start_byte.take().expect("non-empty token has a start"),
                end_byte: byte_index,
            };
            if query_tokens
                .iter()
                .any(|query| token.normalized.starts_with(query))
            {
                return Some(token);
            }
        }
        previous = Some(character);
    }
    if !current.is_empty() {
        let token = LexicalSourceToken {
            normalized: current,
            start_byte: start_byte.expect("non-empty token has a start"),
            end_byte: value.len(),
        };
        if query_tokens
            .iter()
            .any(|query| token.normalized.starts_with(query))
        {
            return Some(token);
        }
    }
    None
}

fn lexical_source_target(
    file_path: &str,
    content: &str,
    query_tokens: &[String],
    content_matched: bool,
) -> (Option<SearchTargetDto>, Option<u32>, Option<String>) {
    if content_matched && let Some(matched) = lexical_source_match(content, query_tokens) {
        let start_line = content[..matched.start_byte]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            .saturating_add(1) as u32;
        let line_start = content[..matched.start_byte]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let line_end = content[matched.end_byte..]
            .find('\n')
            .map_or(content.len(), |index| matched.end_byte + index);
        let excerpt_prefix = content[line_start..matched.start_byte]
            .chars()
            .rev()
            .take(192)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>();
        let excerpt_match = content[matched.start_byte..matched.end_byte]
            .chars()
            .take(128)
            .collect::<String>();
        let excerpt_suffix = content[matched.end_byte..line_end]
            .chars()
            .take(192)
            .collect::<String>();
        let Ok(start_byte) = u32::try_from(matched.start_byte) else {
            return (
                Some(SearchTargetDto::File {
                    file_path: file_path.to_string(),
                }),
                None,
                None,
            );
        };
        let Ok(end_byte) = u32::try_from(matched.end_byte) else {
            return (
                Some(SearchTargetDto::File {
                    file_path: file_path.to_string(),
                }),
                None,
                None,
            );
        };
        return (
            Some(SearchTargetDto::FileRange {
                file_path: file_path.to_string(),
                start_byte,
                end_byte,
            }),
            Some(start_line),
            Some(format!("{excerpt_prefix}{excerpt_match}{excerpt_suffix}")),
        );
    }

    (
        Some(SearchTargetDto::File {
            file_path: file_path.to_string(),
        }),
        None,
        None,
    )
}

fn fts_document_frequency(connection: &Connection, token: &str) -> Result<usize> {
    let query = format!("\"{}\"*", token.replace('"', "\"\""));
    connection
        .prepare_cached("SELECT count(*) FROM lexical_fts WHERE lexical_fts MATCH ?1")?
        .query_row([query], |row| {
            row.get::<_, u32>(0).map(|count| count as usize)
        })
        .map_err(Into::into)
}

fn lexical_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let normalized = normalize_lexical_text(query);
    for token in normalized
        .split_whitespace()
        .filter(|token| token.len() >= 2)
        .filter(|token| !LEXICAL_STOP_WORDS.contains(token))
    {
        if !tokens.iter().any(|existing| existing == token) {
            tokens.push(token.to_string());
        }
    }
    tokens
}

fn quoted_query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut delimiter = None;
    let mut current = String::new();
    for character in query.chars() {
        if delimiter == Some(character) {
            if !current.is_empty() {
                for token in lexical_query_tokens(&current) {
                    if !tokens.iter().any(|existing| existing == &token) {
                        tokens.push(token);
                    }
                }
                current.clear();
            }
            delimiter = None;
        } else if delimiter.is_none() && matches!(character, '`' | '"') {
            delimiter = Some(character);
        } else if delimiter.is_some() {
            current.push(character);
        }
    }
    tokens
}

const LEXICAL_STOP_WORDS: &[&str] = &[
    "about", "after", "and", "are", "cite", "does", "explain", "file", "files", "flow", "flows",
    "for", "from", "how", "into", "level", "path", "source", "sources", "support", "that", "the",
    "through", "top", "what", "where", "which", "with",
];

fn lexical_token_weight(document_frequency: usize, document_count: usize) -> f32 {
    let rarity = ((document_count as f32 + 1.0) / (document_frequency as f32 + 1.0)).ln();
    (1.0 + rarity).clamp(0.25, 5.0)
}

fn required_lexical_match_count(token_count: usize) -> usize {
    match token_count {
        0 => 0,
        1 => 1,
        2 | 3 => 2,
        _ => token_count.saturating_mul(2).saturating_add(4) / 5,
    }
}

#[derive(Debug, Clone, Copy)]
struct LexicalTokenMatch {
    matched_count: usize,
    matched_weight: f32,
    path_weight: f32,
    content_weight: f32,
    total_weight: f32,
}

fn lexical_token_match(
    tokens: &[String],
    token_weights: &[f32],
    path_lower: &str,
    content_lower: &str,
) -> LexicalTokenMatch {
    let mut result = LexicalTokenMatch {
        matched_count: 0,
        matched_weight: 0.0,
        path_weight: 0.0,
        content_weight: 0.0,
        total_weight: 0.0,
    };
    for (token, weight) in tokens.iter().zip(token_weights.iter().copied()) {
        result.total_weight += weight;
        let path_factor = path_match_factor(path_lower, token);
        let content_match = content_lower.contains(token.as_str());
        if path_factor > 0.0 || content_match {
            result.matched_count += 1;
            result.matched_weight += weight;
        }
        if path_factor > 0.0 {
            result.path_weight += weight * path_factor;
        }
        if content_match {
            result.content_weight += weight;
        }
    }
    result
}

fn path_match_factor(normalized_path: &str, token: &str) -> f32 {
    if normalized_path.split_whitespace().any(|part| part == token) {
        1.0
    } else if normalized_path.contains(token) {
        0.35
    } else {
        0.0
    }
}

fn mandatory_tokens_match(
    mandatory_tokens: &[String],
    normalized_path: &str,
    normalized_content: &str,
) -> bool {
    mandatory_tokens.iter().all(|token| {
        normalized_path.contains(token.as_str()) || normalized_content.contains(token.as_str())
    })
}

#[cfg(test)]
fn score_lexical_match(
    path: &str,
    source: LexicalDocumentSource,
    token_match: &LexicalTokenMatch,
) -> f32 {
    let coverage = if token_match.total_weight <= 0.0 {
        0.0
    } else {
        token_match.matched_weight / token_match.total_weight
    };
    let path_coverage = if token_match.total_weight <= 0.0 {
        0.0
    } else {
        token_match.path_weight / token_match.total_weight
    };
    let content_coverage = if token_match.total_weight <= 0.0 {
        0.0
    } else {
        token_match.content_weight / token_match.total_weight
    };
    let role_prior = match lexical_file_role(path) {
        FileRole::Entrypoint => 1.0,
        FileRole::Source => 0.9,
        FileRole::Test => 0.55,
        FileRole::Docs => 0.50,
        FileRole::Benchmark => 0.45,
        FileRole::Generated => 0.35,
        FileRole::Vendor => 0.25,
    };
    let source_quality = match source {
        LexicalDocumentSource::SymbolDoc => 1.0,
        LexicalDocumentSource::LexicalSource => 0.9,
        LexicalDocumentSource::ComponentReport => 0.8,
    };
    (0.55 * coverage
        + 0.20 * content_coverage.clamp(0.0, 1.0)
        + 0.15 * path_coverage.clamp(0.0, 1.0)
        + 0.05 * role_prior
        + 0.05 * source_quality)
        .clamp(0.0, 1.0)
}

#[cfg(test)]
fn lexical_file_role(path: &str) -> FileRole {
    let path = Path::new(path);
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "mdx" | "rst"
            )
        })
    {
        FileRole::Docs
    } else {
        FileRole::classify_path(path)
    }
}

#[cfg(test)]
#[allow(clippy::permissions_set_readonly_false)]
pub(crate) fn make_test_file_writable(path: &Path) {
    let mut permissions = std::fs::metadata(path)
        .expect("test file metadata")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(windows)]
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions).expect("make test file writable");
}

/// Keep the fallback aligned if the default source policy changes. Product
/// scans resolve the active cap from the pinned core publication instead.
const _: () = assert!(
    MAX_FILE_BYTES >= codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP,
    "the lexical lane must admit every file the indexer does"
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    struct CountingReader {
        remaining: usize,
        bytes_read: Arc<AtomicUsize>,
    }

    impl Read for CountingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = buffer.len().min(self.remaining);
            buffer[..count].fill(b'x');
            self.remaining -= count;
            self.bytes_read.fetch_add(count, Ordering::SeqCst);
            Ok(count)
        }
    }

    fn build(project: &Path, data: &Path, generation: &str, input: &str) -> PathBuf {
        let fingerprint = lexical_input_fingerprint(project, None).expect("fingerprint");
        build_lexical_shard(project, None, data, generation, &fingerprint, input)
            .expect("build lexical shard");
        shard_dir_for(data, generation)
    }

    fn prepared_documents(documents: Vec<LexicalDocument>) -> PreparedLexicalInput {
        let coverage = LexicalCoverage {
            discovered_files: documents.len() as u32,
            indexed_files: documents.len() as u32,
            ..LexicalCoverage::default()
        };
        PreparedLexicalInput {
            fingerprint: prepared_lexical_fingerprint(&documents, &coverage).expect("fingerprint"),
            documents,
            source_seals: Vec::new(),
            bounded_state: None,
        }
    }

    fn source_document(path: &str, content: &str) -> LexicalDocument {
        LexicalDocument {
            path: path.into(),
            content: content.into(),
            source: LexicalDocumentSource::LexicalSource,
            node_id: None,
            symbol_name: None,
            start_line: None,
        }
    }

    #[test]
    fn incremental_lexical_reconciliation_matches_a_clean_same_count_build() {
        let root = TempDir::new().expect("tempdir");
        let data = root.path().join("incremental");
        let previous = prepared_documents(vec![
            source_document("src/a.rs", "old alpha"),
            source_document("src/b.rs", "removed beta"),
            source_document("src/kept.rs", "unchanged epsilon"),
        ]);
        build_prepared_lexical_shard(&data, "previous", &previous, "input-v1", None, || Ok(()))
            .expect("previous shard");
        let current = prepared_documents(vec![
            source_document("src/a.rs", "changed gamma"),
            source_document("src/c.rs", "inserted delta"),
            source_document("src/kept.rs", "unchanged epsilon"),
        ]);
        let (_, work) = build_prepared_lexical_shard(
            &data,
            "current",
            &current,
            "input-v2",
            Some("previous"),
            || Ok(()),
        )
        .expect("incremental shard");
        let Some(work) = work else {
            return;
        };
        assert_eq!(work.retained, 1);
        assert_eq!(work.inserted, 2);
        assert_eq!(work.removed, 2);

        let clean_data = root.path().join("clean");
        build_prepared_lexical_shard(
            &clean_data,
            "current",
            &current,
            "input-v2",
            None,
            || Ok(()),
        )
        .expect("clean shard");
        for query in ["gamma", "delta", "beta", "epsilon"] {
            let incremental_hits =
                search_lexical_index(&shard_dir_for(&data, "current"), "input-v2", query, 8)
                    .expect("incremental search");
            let clean_hits =
                search_lexical_index(&shard_dir_for(&clean_data, "current"), "input-v2", query, 8)
                    .expect("clean search");
            assert_eq!(
                incremental_hits
                    .iter()
                    .map(|hit| (&hit.path, hit.source, hit.node_id.as_deref()))
                    .collect::<Vec<_>>(),
                clean_hits
                    .iter()
                    .map(|hit| (&hit.path, hit.source, hit.node_id.as_deref()))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn one_document_refresh_appends_a_bounded_delta_and_compacts_off_path() {
        let root = TempDir::new().expect("tempdir");
        let data = root.path().join("delta-chain");
        let mut documents = (0..256)
            .map(|index| {
                source_document(
                    &format!("src/{index}.rs"),
                    &format!("stable_{index} {}", "padding ".repeat(96)),
                )
            })
            .collect::<Vec<_>>();
        let initial = prepared_documents(documents.clone());
        build_prepared_lexical_shard(&data, "g0", &initial, "input-0", None, || Ok(()))
            .expect("initial lexical base");
        let initial_base = shard_dir_for(&data, "g0").join(LEXICAL_INDEX_FILE);
        let initial_identity = codestory_workspace::workspace_path_identity(&initial_base)
            .expect("initial base identity");

        let mut previous = "g0".to_string();
        let mut current = initial;
        for revision in 1..=LEXICAL_DELTA_COMPACTION_COUNT {
            documents[0] = source_document(
                "src/0.rs",
                &format!(
                    "bounded_delta_revision_{revision} {}",
                    "padding ".repeat(96)
                ),
            );
            current = prepared_documents(documents.clone());
            let generation = format!("g{revision}");
            let (_, work) = build_prepared_lexical_shard(
                &data,
                &generation,
                &current,
                &format!("input-{revision}"),
                Some(&previous),
                || Ok(()),
            )
            .expect("append lexical delta");
            let work = work.expect("incremental work");
            assert_eq!(work.retained, 255);
            assert_eq!(work.inserted, 1);
            assert_eq!(work.removed, 1);
            previous = generation;
        }

        let final_shard = shard_dir_for(&data, &previous);
        let component_set = read_lexical_component_set(
            &final_shard,
            Some(&previous),
            Some(&format!("input-{}", LEXICAL_DELTA_COMPACTION_COUNT)),
        )
        .expect("read delta component set")
        .expect("delta component set");
        assert_eq!(component_set.deltas.len(), LEXICAL_DELTA_COMPACTION_COUNT);
        assert_eq!(
            codestory_workspace::workspace_path_identity(
                &final_shard.join(&component_set.base.file_name)
            )
            .expect("retained base identity"),
            initial_identity,
            "foreground one-document refreshes must retain the immutable base by identity",
        );
        assert_eq!(
            search_lexical_index(
                &final_shard,
                &format!("input-{}", LEXICAL_DELTA_COMPACTION_COUNT),
                &format!("bounded_delta_revision_{}", LEXICAL_DELTA_COMPACTION_COUNT),
                8,
            )
            .expect("search delta chain")
            .first()
            .map(|hit| hit.path.as_str()),
            Some("src/0.rs")
        );

        compact_lexical_component_set(&final_shard, &current, &component_set)
            .expect("compact outside foreground publication");
        let compacted = read_lexical_component_set(
            &final_shard,
            Some(&previous),
            Some(&format!("input-{}", LEXICAL_DELTA_COMPACTION_COUNT)),
        )
        .expect("read compacted set")
        .expect("compacted set");
        assert!(compacted.deltas.is_empty());
        assert_ne!(compacted.base.file_name, component_set.base.file_name);
        assert_eq!(
            search_lexical_index(
                &final_shard,
                &format!("input-{}", LEXICAL_DELTA_COMPACTION_COUNT),
                &format!("bounded_delta_revision_{}", LEXICAL_DELTA_COMPACTION_COUNT),
                8,
            )
            .expect("search compacted base")
            .first()
            .map(|hit| hit.path.as_str()),
            Some("src/0.rs")
        );
    }

    #[test]
    fn publication_only_lexical_churn_directly_references_the_component_without_clone() {
        let root = TempDir::new().expect("tempdir");
        let data = root.path().join("data");
        let prepared = prepared_documents(vec![
            source_document("src/a.rs", "alpha"),
            source_document("src/b.rs", "beta"),
        ]);
        build_prepared_lexical_shard(&data, "previous", &prepared, "input-v1", None, || Ok(()))
            .expect("previous shard");
        let previous_path = shard_dir_for(&data, "previous").join(LEXICAL_INDEX_FILE);

        let (_, work) = crate::copy_on_write::with_clone_disabled(|| {
            build_prepared_lexical_shard(
                &data,
                "current",
                &prepared,
                "input-v2",
                Some("previous"),
                || Ok(()),
            )
        })
        .expect("publication-only lexical shard");
        let work = work.expect("direct-reference work");
        assert!(work.direct_reference);
        assert_eq!(work.retained, 2);
        assert_eq!(work.inserted, 0);
        assert_eq!(work.removed, 0);

        let current_path = shard_dir_for(&data, "current").join(LEXICAL_INDEX_FILE);
        assert_eq!(
            codestory_workspace::workspace_path_identity(&previous_path)
                .expect("previous identity"),
            codestory_workspace::workspace_path_identity(&current_path).expect("current identity"),
            "publication-only churn must retain the exact immutable physical component",
        );
        let aliased_receipt = lexical_shard_receipt_stats(&data, "current")
            .expect("direct reference inherits the validated lexical receipt");
        assert_eq!(aliased_receipt.validations, 1);
        let component_set_key = shard_dir_for(&data, "current").join(LEXICAL_COMPONENT_SET_FILE);
        let produced_set_receipt = LEXICAL_COMPONENT_SET_RECEIPTS
            .stats(&component_set_key)
            .expect("the owning producer seals the reconciled component set");
        assert_eq!(produced_set_receipt.validations, 1);
        assert_eq!(
            search_lexical_index(&shard_dir_for(&data, "current"), "input-v2", "alpha", 8)
                .expect("search current envelope")
                .len(),
            1,
        );
        let reused_receipt = lexical_shard_receipt_stats(&data, "current")
            .expect("current lexical receipt remains sealed");
        assert_eq!(reused_receipt.validations, 1);
        assert!(reused_receipt.reuses > aliased_receipt.reuses);
        let reused_set_receipt = LEXICAL_COMPONENT_SET_RECEIPTS
            .stats(&component_set_key)
            .expect("component-set receipt remains sealed");
        assert_eq!(reused_set_receipt.validations, 1);
        assert!(reused_set_receipt.reuses > produced_set_receipt.reuses);

        let refresh = capture_lexical_generation_receipts(&data, "current")
            .expect("capture current receipts before owned cleanup");
        std::fs::remove_file(&previous_path).expect("retire predecessor hard link");
        let (refreshed, refused) = refresh.refresh_after_owned_link_cleanup();
        assert!(refreshed >= 2);
        assert_eq!(refused, 0);
        assert_eq!(
            search_lexical_index(&shard_dir_for(&data, "current"), "input-v2", "beta", 8)
                .expect("search after predecessor cleanup")
                .len(),
            1,
        );
        assert_eq!(
            lexical_shard_receipt_stats(&data, "current")
                .expect("cleanup refreshed current receipt")
                .validations,
            1,
            "owned hard-link cleanup must not force another full lexical scan",
        );
    }

    #[test]
    fn bounded_source_transition_matches_full_fingerprint_and_reads_one_document() {
        let root = TempDir::new().expect("tempdir");
        let project = root.path().join("project");
        std::fs::create_dir_all(project.join("src")).expect("create source directory");
        std::fs::write(project.join("src/a.rs"), "old alpha\n").expect("write changed source");
        std::fs::write(project.join("src/b.rs"), "stable beta\n").expect("write stable source");

        let previous_storage_path = root.path().join("previous.sqlite3");
        let current_storage_path = root.path().join("current.sqlite3");
        let mut previous_storage = Store::open(&previous_storage_path).expect("previous store");
        let mut current_storage = Store::open(&current_storage_path).expect("current store");
        publish_test_source_policy(&mut previous_storage, &project, MAX_FILE_BYTES, &[]);
        publish_test_source_policy(&mut current_storage, &project, MAX_FILE_BYTES, &[]);
        drop(previous_storage);
        drop(current_storage);

        let data = root.path().join("lexical");
        let previous = prepared_documents(vec![
            source_document("src/a.rs", "old alpha\n"),
            source_document("src/b.rs", "stable beta\n"),
        ]);
        build_prepared_lexical_shard(&data, "previous", &previous, "input-v1", None, || Ok(()))
            .expect("previous lexical generation");
        let previous_base = shard_dir_for(&data, "previous").join(LEXICAL_INDEX_FILE);
        let previous_base_identity = codestory_workspace::workspace_path_identity(&previous_base)
            .expect("previous base identity");

        std::fs::write(project.join("src/a.rs"), "changed gamma\n").expect("change one source");
        let policy = codestory_contracts::workspace::SourceIndexPolicy::default();
        let source_seals = observe_lexical_source_seals(&project, Some(&current_storage_path))
            .expect("seal complete core inventory");
        let bounded = prepare_bounded_lexical_input(
            &project,
            &current_storage_path,
            &previous_storage_path,
            &data,
            "previous",
            &["src/a.rs".into()],
            &source_seals,
            &policy,
        )
        .expect("prepare bounded transition")
        .expect("bounded transition eligible");
        assert_eq!(bounded.documents.len(), 1);
        let full = prepared_documents(vec![
            source_document("src/a.rs", "changed gamma\n"),
            source_document("src/b.rs", "stable beta\n"),
        ]);
        assert_eq!(bounded.fingerprint, full.fingerprint);

        let (_, work) = build_prepared_lexical_shard(
            &data,
            "current",
            &bounded,
            "input-v2",
            Some("previous"),
            || Ok(()),
        )
        .expect("publish bounded transition");
        let work = work.expect("bounded delta work");
        assert_eq!((work.retained, work.inserted, work.removed), (1, 1, 1));
        let current_set = read_lexical_component_set(
            &shard_dir_for(&data, "current"),
            Some("current"),
            Some("input-v2"),
        )
        .expect("read current component set")
        .expect("current component set");
        assert_eq!(
            codestory_workspace::workspace_path_identity(
                &shard_dir_for(&data, "current").join(&current_set.base.file_name),
            )
            .expect("current base identity"),
            previous_base_identity,
        );
        assert_eq!(
            search_lexical_index(&shard_dir_for(&data, "current"), "input-v2", "gamma", 8,)
                .expect("search bounded transition")
                .first()
                .map(|hit| hit.path.as_str()),
            Some("src/a.rs"),
        );
    }

    #[test]
    fn cancelled_lexical_reconciliation_leaves_no_candidate_shard() {
        let root = TempDir::new().expect("tempdir");
        let data = root.path().join("data");
        let previous = prepared_documents(vec![source_document("src/a.rs", "alpha")]);
        build_prepared_lexical_shard(&data, "previous", &previous, "input-v1", None, || Ok(()))
            .expect("previous shard");
        let current = prepared_documents(vec![source_document("src/a.rs", "changed")]);

        let error = build_prepared_lexical_shard(
            &data,
            "cancelled",
            &current,
            "input-v2",
            Some("previous"),
            || bail!("simulated lexical cancellation"),
        )
        .expect_err("cancelled lexical candidate must fail");

        assert!(format!("{error:#}").contains("simulated lexical cancellation"));
        assert!(
            !shard_dir_for(&data, "cancelled")
                .join(LEXICAL_INDEX_FILE)
                .exists()
        );
        assert_eq!(
            std::fs::read_dir(shard_dir_for(&data, "cancelled"))
                .expect("cancelled shard directory")
                .count(),
            0,
            "failed lexical staging must not leak a generation-local clone"
        );
    }

    #[test]
    fn corrupt_lexical_predecessor_falls_back_to_a_clean_candidate() {
        let root = TempDir::new().expect("tempdir");
        let data = root.path().join("data");
        let previous = prepared_documents(vec![source_document("src/a.rs", "old")]);
        build_prepared_lexical_shard(&data, "previous", &previous, "input-v1", None, || Ok(()))
            .expect("previous shard");
        let previous_path = shard_dir_for(&data, "previous").join(LEXICAL_INDEX_FILE);
        crate::copy_on_write::make_file_owner_writable(&previous_path)
            .expect("make predecessor writable");
        std::fs::write(&previous_path, b"not sqlite").expect("corrupt predecessor");
        let current = prepared_documents(vec![source_document("src/a.rs", "current needle")]);

        let (_, work) = build_prepared_lexical_shard(
            &data,
            "current",
            &current,
            "input-v2",
            Some("previous"),
            || Ok(()),
        )
        .expect("complete fallback");

        assert!(work.is_none());
        assert_eq!(
            search_lexical_index(&shard_dir_for(&data, "current"), "input-v2", "needle", 8,)
                .expect("search fallback candidate")
                .len(),
            1
        );
    }

    fn publish_test_source_policy(
        storage: &mut Store,
        project_root: &Path,
        byte_cap: u64,
        candidates: &[codestory_workspace::OversizedSourceExclusionCandidate],
    ) {
        let publication = codestory_store::IndexPublicationRecord {
            generation: 1,
            generation_id: "test-generation".to_string(),
            run_id: "test-run".to_string(),
            mode: codestory_store::IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        storage
            .put_index_publication(&publication)
            .expect("publish test core identity");
        let identity = codestory_workspace::project_identity_v3(project_root);
        storage
            .publish_source_policy_exclusion_generation(
                &publication,
                &identity.project_id,
                &identity.workspace_id,
                SourcePolicyExclusionPolicyIdentity::new(
                    codestory_contracts::workspace::OVERSIZED_SOURCE_POLICY_VERSION,
                    byte_cap,
                    codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
                ),
                candidates,
            )
            .expect("publish test source policy");
    }

    #[test]
    fn lexical_text_reader_stops_after_cap_overflow_sentinel() {
        let bytes_read = Arc::new(AtomicUsize::new(0));
        let reader = CountingReader {
            remaining: 4_096,
            bytes_read: Arc::clone(&bytes_read),
        };

        let contents = read_lexical_text_limited(reader, 64).expect("bounded read");

        assert!(contents.is_none());
        assert_eq!(bytes_read.load(Ordering::SeqCst), 65);
    }

    #[test]
    fn sqlite_fts_search_keeps_existing_scoring_and_project_isolation() {
        let project_a = TempDir::new().expect("project a");
        let project_b = TempDir::new().expect("project b");
        std::fs::create_dir_all(project_a.path().join("src")).expect("mkdir a");
        std::fs::create_dir_all(project_b.path().join("src")).expect("mkdir b");
        std::fs::write(project_a.path().join("src/a_weak.rs"), "handler once").expect("a weak");
        std::fs::write(
            project_a.path().join("src/z_strong_handler.rs"),
            "handler handler handler",
        )
        .expect("a strong");
        std::fs::write(project_b.path().join("src/handler.rs"), "project_b_handler").expect("b");
        let data = TempDir::new().expect("data");
        let shard_a = build(project_a.path(), data.path(), "a", "input-a");
        let _shard_b = build(project_b.path(), data.path(), "b", "input-b");

        let hits = search_lexical_index(&shard_a, "input-a", "handler", 1).expect("search");
        assert_eq!(hits[0].path, "src/z_strong_handler.rs");
        assert_eq!(hits[0].start_line, Some(1));
        assert_eq!(
            hits[0].source_excerpt.as_deref(),
            Some("handler handler handler")
        );
        assert_eq!(
            hits[0].target,
            Some(SearchTargetDto::FileRange {
                file_path: "src/z_strong_handler.rs".to_string(),
                start_byte: 0,
                end_byte: 7,
            })
        );
        assert!(
            search_lexical_index(&shard_a, "input-a", "project_b_handler", 8)
                .expect("isolated search")
                .is_empty()
        );
        assert!(search_lexical_index(&shard_a, "wrong-input", "handler", 8).is_err());
    }

    #[test]
    fn lexical_batch_is_serial_equivalent() {
        let project = TempDir::new().expect("project");
        std::fs::create_dir_all(project.path().join("src")).expect("src");
        std::fs::write(
            project.path().join("src/alpha_handler.rs"),
            "fn alpha_handler() { beta_router(); }",
        )
        .expect("alpha");
        std::fs::write(
            project.path().join("src/beta_router.rs"),
            "fn beta_router() {}",
        )
        .expect("beta");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "batch", "input");
        let requests = vec![
            ("alpha handler".to_string(), 4),
            ("beta router".to_string(), 2),
        ];

        let batched =
            search_lexical_index_batch_with_cancel(&shard, "input", &requests, Arc::new(|| false))
                .expect("batch search");
        let serial = requests
            .iter()
            .map(|(query, limit)| search_lexical_index(&shard, "input", query, *limit))
            .collect::<Result<Vec<_>>>()
            .expect("serial searches");
        let identity = |hits: &[LexicalHit]| {
            hits.iter()
                .map(|hit| {
                    (
                        hit.path.clone(),
                        hit.node_id.clone(),
                        hit.symbol_name.clone(),
                        hit.start_line,
                        hit.score.to_bits(),
                    )
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(
            batched
                .iter()
                .map(|hits| identity(hits))
                .collect::<Vec<_>>(),
            serial.iter().map(|hits| identity(hits)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn lexical_union_retains_an_independent_content_candidate() {
        let project = TempDir::new().expect("project");
        let path_matches = project.path().join("needle");
        std::fs::create_dir_all(&path_matches).expect("path matches");
        for index in 0..300 {
            std::fs::write(
                path_matches.join(format!("path_match_{index:03}.rs")),
                "fn unrelated() {}",
            )
            .expect("path candidate");
        }
        let content_match = project.path().join("src/content_match.rs");
        std::fs::create_dir_all(content_match.parent().expect("parent")).expect("src");
        std::fs::write(&content_match, "fn content_match() { /* needle */ }")
            .expect("content candidate");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "union", "input");

        let hits = search_lexical_index(&shard, "input", "needle", 8).expect("search");

        assert!(
            hits.iter().any(|hit| hit.path == "src/content_match.rs"),
            "the content lane must survive a large independent path lane: {hits:#?}"
        );
    }

    #[test]
    fn focused_symbol_lane_beats_scattered_whole_file_term_coverage() {
        let root = TempDir::new().expect("root");
        let generation = "focused-symbol";
        let shard = shard_dir_for(root.path(), generation);
        std::fs::create_dir_all(&shard).expect("shard");
        let documents = vec![
            LexicalDocument {
                path: "src/broad.rs".to_string(),
                content: "request dispatch output evidence indexed symbol hits retrieval shadow"
                    .to_string(),
                source: LexicalDocumentSource::LexicalSource,
                node_id: None,
                symbol_name: None,
                start_line: None,
            },
            LexicalDocument {
                path: "src/primary.rs".to_string(),
                content:
                    "fn request_dispatch_output_evidence_indexed_symbol_hits_retrieval_shadow()"
                        .to_string(),
                source: LexicalDocumentSource::SymbolDoc,
                node_id: Some("symbol:primary-request-dispatch".to_string()),
                symbol_name: Some(
                    "request_dispatch_output_evidence_indexed_symbol_hits_retrieval_shadow"
                        .to_string(),
                ),
                start_line: Some(4),
            },
            LexicalDocument {
                path: "src/output.rs".to_string(),
                content: "fn append_request_evidence_symbol() // indexed output dispatch result"
                    .to_string(),
                source: LexicalDocumentSource::SymbolDoc,
                node_id: Some("symbol:append-request-evidence".to_string()),
                symbol_name: Some("append_request_evidence_symbol".to_string()),
                start_line: Some(12),
            },
        ];
        let coverage = LexicalCoverage {
            discovered_files: 3,
            indexed_files: 3,
            ..Default::default()
        };
        let fingerprint = LexicalInputFingerprint {
            file_count: documents.len() as u32,
            hash: lexical_documents_hash(&documents, &coverage),
            coverage: coverage.clone(),
        };
        write_lexical_database(
            &shard.join(LEXICAL_INDEX_FILE),
            generation,
            "focused-symbol-input",
            &fingerprint,
            |visit| {
                for document in &documents {
                    visit(document)?;
                }
                Ok(coverage.clone())
            },
        )
        .expect("write focused lexical shard");

        let hits = search_lexical_index(
            &shard,
            "focused-symbol-input",
            "request dispatch output evidence indexed symbol hits retrieval shadow",
            3,
        )
        .expect("search");

        let focused_position = hits
            .iter()
            .position(|hit| hit.symbol_name.as_deref() == Some("append_request_evidence_symbol"))
            .expect("second-ranked focused symbol is retained");
        let broad_position = hits
            .iter()
            .position(|hit| hit.path == "src/broad.rs")
            .expect("broad source candidate is retained");
        assert!(
            focused_position < broad_position,
            "a focused symbol-document rank must survive independent sublane fusion"
        );
    }

    #[test]
    fn lexical_sublane_fusion_does_not_collapse_to_the_minimum_rank() {
        let broad_file = LexicalSublaneRanks {
            content: Some(1),
            ..Default::default()
        };
        let focused_symbol = LexicalSublaneRanks {
            symbol_document: Some(2),
            ..Default::default()
        };

        assert!(
            lexical_sublane_score(focused_symbol, crate::query_features::QueryShape::Mixed)
                > lexical_sublane_score(broad_file, crate::query_features::QueryShape::Mixed)
        );
    }

    #[test]
    fn lexical_recall_requires_quoted_entities_without_a_long_query_path_gate() {
        let project = TempDir::new().expect("project");
        std::fs::create_dir_all(project.path().join("src")).expect("src");
        std::fs::write(
            project.path().join("src/flow.rs"),
            "packet search dispatch builds ranked results through semantic graph retrieval",
        )
        .expect("complete source");
        std::fs::write(
            project.path().join("src/distractor.rs"),
            "packet search dispatch builds ranked results through semantic graph",
        )
        .expect("missing quoted entity");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "quoted", "input");

        let hits = search_lexical_index(
            &shard,
            "input",
            "explain packet search dispatch builds ranked results through semantic graph `retrieval`",
            8,
        )
        .expect("search");

        assert_eq!(
            hits.iter().map(|hit| hit.path.as_str()).collect::<Vec<_>>(),
            vec!["src/flow.rs"]
        );
    }

    #[test]
    fn lexical_admission_counts_terms_and_keeps_quoted_terms_mandatory() {
        let project = TempDir::new().expect("project");
        std::fs::create_dir_all(project.path().join("src")).expect("src");
        std::fs::write(
            project.path().join("src/text_checks.rs"),
            "fn isEmpty() -> bool { true }",
        )
        .expect("two-term source");
        std::fs::write(
            project.path().join("src/cache_state.rs"),
            "fn isEmpty() -> bool { false }",
        )
        .expect("one-term source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "term-count", "input");

        let hits = search_lexical_index(&shard, "input", "text empty predicate", 8)
            .expect("unquoted search");
        assert_eq!(
            hits.iter().map(|hit| hit.path.as_str()).collect::<Vec<_>>(),
            vec!["src/text_checks.rs"],
            "two of three non-stopwords qualify while one of three does not"
        );

        let quoted = search_lexical_index(&shard, "input", "text empty `predicate`", 8)
            .expect("quoted search");
        assert!(
            quoted.is_empty(),
            "count-based admission must not weaken quoted-term requirements"
        );

        assert_eq!(required_lexical_match_count(1), 1);
        assert_eq!(required_lexical_match_count(2), 2);
        assert_eq!(required_lexical_match_count(3), 2);
        assert_eq!(required_lexical_match_count(4), 2);
        assert_eq!(required_lexical_match_count(12), 5);
    }

    #[test]
    fn whole_file_path_only_match_has_an_explicit_file_target() {
        let project = TempDir::new().expect("project");
        std::fs::create_dir_all(project.path().join("src")).expect("src");
        std::fs::write(
            project.path().join("src/request_handler.rs"),
            "fn run() {}\n",
        )
        .expect("source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "path-target", "input");

        let hits = search_lexical_index(&shard, "input", "request_handler", 1).expect("search");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].start_line, None);
        assert_eq!(hits[0].source_excerpt, None);
        assert_eq!(
            hits[0].target,
            Some(SearchTargetDto::File {
                file_path: "src/request_handler.rs".to_string(),
            })
        );
    }

    #[test]
    fn sqlite_lexical_search_observes_cancellation() {
        let project = TempDir::new().expect("project");
        std::fs::create_dir_all(project.path().join("src")).expect("src");
        for index in 0..256 {
            std::fs::write(
                project.path().join(format!("src/{index}.rs")),
                "fn cancellation_needle() {}",
            )
            .expect("source");
        }
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "cancel", "input");
        let polls = Arc::new(AtomicUsize::new(0));
        let search_polls = Arc::clone(&polls);

        let error = search_lexical_index_with_cancel(&shard, "input", "needle", 8, move || {
            search_polls.fetch_add(1, Ordering::Relaxed) > 5
        })
        .expect_err("lexical execution should observe cancellation");

        assert!(error.to_string().contains("cancelled"));
        assert!(
            polls.load(Ordering::Relaxed) > 5,
            "lexical execution must poll cancellation during query work"
        );
    }

    #[test]
    fn broad_prompt_relevance_fixture_remains_equivalent() {
        let project = TempDir::new().expect("project");
        for (path, content) in [
            (
                "workspace/app/src/event_processor_with_jsonl_output.rs",
                "jsonl event output request runtime turn start",
            ),
            (
                "workspace/app/tests/event_processor_with_json_output.rs",
                "json event output test approval fixture",
            ),
            ("workspace/core/src/session.rs", "session bookkeeping"),
            (
                ".agents/skills/review/SKILL.md",
                "request json cli runtime thread turn start event output",
            ),
            (
                "workspace/app-protocol/schema/typescript/v2/CommandRequestParams.ts",
                "app server command request turn start request",
            ),
        ] {
            let path = project.path().join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, content).expect("fixture");
        }
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "broad", "input");

        let hits = search_lexical_index(
            &shard,
            "input",
            "Explain how `app request --json` flows from CLI into runtime thread turn start JSONL event output",
            4,
        )
        .expect("search");
        assert_eq!(
            hits.first().map(|hit| hit.path.as_str()),
            Some("workspace/app/src/event_processor_with_jsonl_output.rs")
        );
        assert!(
            hits.iter()
                .all(|hit| hit.path != "workspace/core/src/session.rs")
        );
        assert!(
            hits.iter()
                .all(|hit| hit.path != ".agents/skills/review/SKILL.md")
        );
    }

    #[test]
    fn old_jsonl_is_not_a_query_engine_and_is_removed_on_rebuild() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "pub fn handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = shard_dir_for(data.path(), "generation");
        std::fs::create_dir_all(&shard).expect("shard");
        std::fs::write(shard.join(LEGACY_INDEX_FILE), "legacy").expect("legacy index");
        std::fs::write(shard.join(LEGACY_META_FILE), "{}").expect("legacy meta");
        assert!(search_lexical_index(&shard, "input", "handler", 4).is_err());

        let _rebuilt = build(project.path(), data.path(), "generation", "input");
        let rebuilt = build(project.path(), data.path(), "generation", "input");
        assert!(!rebuilt.join(LEGACY_INDEX_FILE).exists());
        assert!(!rebuilt.join(LEGACY_META_FILE).exists());
        assert_eq!(
            search_lexical_index(&rebuilt, "input", "handler", 4)
                .expect("rebuilt search")
                .len(),
            1
        );
    }

    #[test]
    fn malformed_sqlite_shard_fails_closed() {
        let root = TempDir::new().expect("root");
        let shard = shard_dir_for(root.path(), "generation");
        std::fs::create_dir_all(&shard).expect("shard");
        std::fs::write(shard.join(LEXICAL_INDEX_FILE), b"not sqlite").expect("malformed");
        assert!(!shard_has_lexical_index(&shard, "input"));
        assert!(search_lexical_index(&shard, "input", "handler", 4).is_err());
    }

    #[test]
    fn sqlite_build_skips_stale_temporary_file_collisions() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = shard_dir_for(data.path(), "collision");
        std::fs::create_dir_all(&shard).expect("shard");
        let index = shard.join(LEXICAL_INDEX_FILE);
        let probe = codestory_workspace::atomic_file::atomic_temp_path(&index, "lexical-index");
        let counter = probe
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.rsplit('.').nth(1))
            .and_then(|value| value.parse::<u64>().ok())
            .expect("temp counter");
        let stale = (counter + 1..=counter + 32)
            .map(|counter| {
                index.with_file_name(format!(
                    ".lexical-index.{}.{}.tmp",
                    std::process::id(),
                    counter
                ))
            })
            .collect::<Vec<_>>();
        for path in &stale {
            std::fs::write(path, b"stale").expect("stale temp");
        }
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");

        build_lexical_shard(
            project.path(),
            None,
            data.path(),
            "collision",
            &fingerprint,
            "input",
        )
        .expect("collision-safe build");

        for path in stale {
            assert_eq!(std::fs::read(path).expect("stale preserved"), b"stale");
        }
    }

    #[test]
    fn camel_case_and_acronym_queries_use_the_same_fts_normalization_as_documents() {
        let project = TempDir::new().expect("project");
        std::fs::write(
            project.path().join("server.rs"),
            "struct HTTPServer; fn parseJSONResponse() {}",
        )
        .expect("source");
        std::fs::create_dir_all(project.path().join("src")).expect("src");
        std::fs::write(
            project.path().join("src/TLSHandshakeCoordinator.rs"),
            "// path-only acronym fixture",
        )
        .expect("acronym path");
        std::fs::write(
            project.path().join("src/ÜberServiceRegistry.rs"),
            "// path-only unicode fixture",
        )
        .expect("unicode path");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "case", "input");

        for query in [
            "HTTPServer",
            "http server",
            "parseJSONResponse",
            "parse json response",
        ] {
            assert_eq!(
                search_lexical_index(&shard, "input", query, 4)
                    .expect("search")
                    .first()
                    .map(|hit| hit.path.as_str()),
                Some("server.rs"),
                "query={query}"
            );
        }
        for (query, expected) in [
            ("TLSHandshakeCoordinator", "src/TLSHandshakeCoordinator.rs"),
            (
                "tls handshake coordinator",
                "src/TLSHandshakeCoordinator.rs",
            ),
            ("ÜBERServiceRegistry", "src/ÜberServiceRegistry.rs"),
            ("über service registry", "src/ÜberServiceRegistry.rs"),
        ] {
            assert_eq!(
                search_lexical_index(&shard, "input", query, 4)
                    .expect("path search")
                    .first()
                    .map(|hit| hit.path.as_str()),
                Some(expected),
                "query={query}"
            );
        }
    }

    #[test]
    fn lexical_shard_survives_data_dirs_beyond_max_path() {
        // Regression companion to the embedded vector deep-root test: the
        // lexical shard is built at the same sidecar depth, so its SQLite
        // opens must also survive cache roots beyond the Windows MAX_PATH
        // cap for non-longPathAware processes.
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn deep_handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        let mut deep_data = data.path().to_path_buf();
        let segment = "max-path-regression-padding-segment".repeat(2);
        while deep_data.as_os_str().len() < 320 {
            deep_data.push(&segment);
        }
        std::fs::create_dir_all(&deep_data).expect("create deep lexical data dir");

        let shard = build(project.path(), &deep_data, "longpath", "input");
        assert!(
            shard.join(LEXICAL_INDEX_FILE).as_os_str().len() > 260,
            "regression shard no longer exceeds MAX_PATH: {}",
            shard.display()
        );
        assert_eq!(
            search_lexical_index(&shard, "input", "deep_handler", 4)
                .expect("search lexical shard under a deep data dir")
                .first()
                .map(|hit| hit.path.as_str()),
            Some("lib.rs")
        );
    }

    #[test]
    fn deep_validation_rejects_forged_rows_without_scanning_them_on_search() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn handler() {}").expect("source");
        std::fs::write(project.path().join("other.rs"), "fn unrelated() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "binding", "input");
        let index = shard.join(LEXICAL_INDEX_FILE);
        make_test_file_writable(&index);
        let connection = Connection::open(&index).expect("open writable");
        connection
            .execute(
                "UPDATE lexical_fts SET content = 'forged'
                 WHERE rowid = (SELECT id FROM lexical_documents WHERE path = 'other.rs')",
                [],
            )
            .expect("forge FTS row");
        drop(connection);

        assert!(!shard_matches_lexical_input(
            data.path(),
            "binding",
            1,
            &lexical_input_fingerprint(project.path(), None)
                .expect("fingerprint")
                .hash,
            "input"
        ));
        assert_eq!(
            search_lexical_index(&shard, "input", "handler", 4)
                .expect("metadata-valid search")
                .first()
                .map(|hit| hit.path.as_str()),
            Some("lib.rs")
        );
    }

    #[test]
    fn canonical_discovery_coverage_and_large_corpus_are_preserved() {
        let project = TempDir::new().expect("project");
        let src = project.path().join("src");
        std::fs::create_dir_all(&src).expect("src");
        for index in 0..4_100 {
            std::fs::write(
                src.join(format!("file_{index:04}.kt")),
                format!("fun symbol_{index:04}() = {index}\n"),
            )
            .expect("source");
        }
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "large", "input");
        let coverage = lexical_shard_coverage(data.path(), "large", "input").expect("coverage");
        assert_eq!(coverage.discovered_files, 4_100);
        assert_eq!(coverage.indexed_files, 4_100);
        assert!(coverage.complete());
        assert_eq!(
            search_lexical_index(&shard, "input", "symbol_4099", 4)
                .expect("search")
                .first()
                .map(|hit| hit.path.as_str()),
            Some("src/file_4099.kt")
        );
    }

    #[test]
    fn lexical_source_set_is_canonical_workspace_discovery() {
        let project = TempDir::new().expect("project");
        for profile in codestory_contracts::language_support::LANGUAGE_SUPPORT_PROFILES {
            for extension in profile.extensions {
                std::fs::write(
                    project
                        .path()
                        .join(format!("sample_{}.{}", profile.language_name, extension)),
                    "sample source\n",
                )
                .expect("write supported source");
            }
        }
        std::fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname='sample'\n",
        )
        .expect("cargo manifest");
        std::fs::write(project.path().join("compose.yaml"), "services: {}\n")
            .expect("compose manifest");

        let workspace = codestory_workspace::WorkspaceManifest::open(project.path().to_path_buf())
            .expect("workspace");
        let expected = workspace
            .source_files()
            .expect("canonical discovery")
            .into_iter()
            .map(|path| lexical_relative_path(project.path(), &path))
            .collect::<std::collections::BTreeSet<_>>();
        let mut actual = std::collections::BTreeSet::new();
        let coverage = scan_lexical_documents(project.path(), None, None, &mut |document| {
            if document.source == LexicalDocumentSource::LexicalSource {
                actual.insert(document.path.clone());
            }
            Ok(())
        })
        .expect("lexical collection");

        assert_eq!(actual, expected);
        assert!(coverage.coverage.complete());
    }

    #[test]
    fn storage_owned_json_does_not_change_fingerprint_or_shard_materialization() {
        let project = TempDir::new().expect("project");
        let storage_path = project.path().join("cache").join("custom-core.db");
        std::fs::create_dir_all(storage_path.parent().expect("storage parent"))
            .expect("storage parent");
        let mut storage = Store::open(&storage_path).expect("custom in-worktree store");
        publish_test_source_policy(&mut storage, project.path(), MAX_FILE_BYTES, &[]);
        drop(storage);
        let sibling = project
            .path()
            .join("cache")
            .join("custom-core.search-generations-user");
        std::fs::create_dir_all(&sibling).expect("sibling user directory");
        std::fs::write(
            sibling.join("user-config.json"),
            "{\"user_token\":\"admitted_user_json\"}\n",
        )
        .expect("user json");

        let before =
            lexical_input_fingerprint(project.path(), Some(&storage_path)).expect("fingerprint");

        let legacy = codestory_workspace::legacy_search_directory_for_storage(&storage_path);
        let generations =
            codestory_workspace::search_generation_directory_for_storage(&storage_path);
        std::fs::create_dir_all(&legacy).expect("legacy search directory");
        std::fs::create_dir_all(generations.join("generation-1"))
            .expect("search generation directory");
        std::fs::write(
            legacy.join("meta.json"),
            "{\"generated_token\":\"legacy_owned_json\"}\n",
        )
        .expect("legacy metadata");
        std::fs::write(
            generations.join("generation-1").join("meta.json"),
            "{\"generated_token\":\"generation_owned_json\"}\n",
        )
        .expect("generation metadata");

        let after = lexical_input_fingerprint(project.path(), Some(&storage_path))
            .expect("post-generation fingerprint");
        assert_eq!(after, before);

        let data = TempDir::new().expect("lexical data");
        let rebuilt = build_lexical_shard(
            project.path(),
            Some(&storage_path),
            data.path(),
            "owned-exclusions",
            &before,
            "input",
        )
        .expect("materialize shard");
        assert_eq!(rebuilt, before);
        let shard = shard_dir_for(data.path(), "owned-exclusions");
        assert!(
            search_lexical_index(&shard, "input", "`generation_owned_json`", 8)
                .expect("search owned token")
                .is_empty()
        );
        assert_eq!(
            search_lexical_index(&shard, "input", "admitted_user_json", 8)
                .expect("search user token")
                .first()
                .map(|hit| hit.path.as_str()),
            Some("cache/custom-core.search-generations-user/user-config.json")
        );
    }

    #[test]
    fn pinned_policy_exclusions_filter_structural_files_without_narrowing_parser_recall() {
        let project = TempDir::new().expect("project");
        let src = project.path().join("src");
        let data = project.path().join("data");
        std::fs::create_dir_all(&src).expect("src");
        std::fs::create_dir_all(&data).expect("data");

        let widened_cap = MAX_FILE_BYTES + 1_024;
        let parser_path = src.join("widened.rs");
        let mut parser_source = b"fn widened_parser_token() {}\n".to_vec();
        parser_source.resize(MAX_FILE_BYTES as usize + 1, b' ');
        std::fs::write(&parser_path, &parser_source).expect("widened parser source");

        let json_path = data.join("config.json");
        let mut json_source = br#"{"excluded_structural_token":"x"}"#.to_vec();
        json_source.resize(
            codestory_contracts::workspace::DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP as usize + 1,
            b' ',
        );
        std::fs::write(&json_path, &json_source).expect("excluded structural source");

        let storage_root = TempDir::new().expect("storage root");
        let storage_path = storage_root.path().join("core.db");
        let mut storage = Store::open(&storage_path).expect("core storage");
        publish_test_source_policy(
            &mut storage,
            project.path(),
            widened_cap,
            &[codestory_workspace::OversizedSourceExclusionCandidate {
                normalized_path: "data/config.json".to_string(),
                content_hash: format!("{:x}", Sha256::digest(&json_source)),
                observed_size: json_source.len() as u64,
                observed_unit_count: 0,
                policy_version: codestory_contracts::workspace::OVERSIZED_SOURCE_POLICY_VERSION
                    .to_string(),
                byte_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP,
                structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            }],
        );
        drop(storage);

        let mut source_paths = std::collections::BTreeSet::new();
        let coverage =
            scan_lexical_documents(project.path(), Some(&storage_path), None, &mut |document| {
                if document.source == LexicalDocumentSource::LexicalSource {
                    source_paths.insert(document.path.clone());
                }
                Ok(())
            })
            .expect("scan pinned lexical sources");

        assert_eq!(
            source_paths,
            std::collections::BTreeSet::from(["src/widened.rs".to_string()])
        );
        assert_eq!(coverage.coverage.discovered_files, 1);
        assert_eq!(coverage.coverage.indexed_files, 1);
        assert!(coverage.coverage.complete());
    }

    #[test]
    fn pinned_lexical_scan_fails_closed_without_source_policy_publication() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn source() {}\n").expect("source");
        let storage_root = TempDir::new().expect("storage root");
        let storage_path = storage_root.path().join("core.db");
        drop(Store::open(&storage_path).expect("bare core storage"));

        let error = lexical_input_fingerprint(project.path(), Some(&storage_path))
            .expect_err("missing publication must fail closed");

        assert!(
            format!("{error:#}")
                .contains("complete core publication for lexical source policy is missing")
        );
    }

    #[test]
    fn prepared_source_seals_detect_in_place_drift_without_a_second_content_scan() {
        let project = TempDir::new().expect("project");
        let source_path = project.path().join("lib.rs");
        std::fs::write(&source_path, "fn before() {}\n").expect("source");
        let storage_root = TempDir::new().expect("storage root");
        let storage_path = storage_root.path().join("core.db");
        let mut storage = Store::open(&storage_path).expect("core storage");
        publish_test_source_policy(&mut storage, project.path(), MAX_FILE_BYTES, &[]);
        let source = lexical_source_input(project.path(), &storage_path).expect("source input");
        let prepared = prepare_lexical_input_for_store(source, project.path(), &storage)
            .expect("prepared input");

        std::fs::write(&source_path, "fn after_() {}\n").expect("rewrite same-size source");

        let error = prepared
            .revalidate_source_seals(project.path(), &storage_path)
            .expect_err("in-place source rewrite must break the publication fence");
        assert!(
            format!("{error:#}").contains("source identity changed"),
            "unexpected source-fence error: {error:#}"
        );
    }

    #[test]
    fn pinned_lexical_scan_rejects_a_foreign_project_policy() {
        let project_a = TempDir::new().expect("project a");
        let project_b = TempDir::new().expect("project b");
        std::fs::create_dir_all(project_a.path().join("data")).expect("project a data");
        std::fs::create_dir_all(project_b.path().join("data")).expect("project b data");
        std::fs::write(
            project_a.path().join("data/config.json"),
            br#"{"selected_project_token":"a"}"#,
        )
        .expect("project a source");

        let mut excluded_source = br#"{"foreign_project_token":"b"}"#.to_vec();
        excluded_source.resize(
            codestory_contracts::workspace::DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP as usize + 1,
            b' ',
        );
        std::fs::write(project_b.path().join("data/config.json"), &excluded_source)
            .expect("project b source");

        let identity_a = codestory_workspace::project_identity_v3(project_a.path());
        let identity_b = codestory_workspace::project_identity_v3(project_b.path());
        assert_ne!(identity_a.workspace_id, identity_b.workspace_id);

        let storage_root = TempDir::new().expect("foreign storage root");
        let storage_path = storage_root.path().join("core.db");
        let mut storage = Store::open(&storage_path).expect("foreign core storage");
        publish_test_source_policy(
            &mut storage,
            project_b.path(),
            MAX_FILE_BYTES,
            &[codestory_workspace::OversizedSourceExclusionCandidate {
                normalized_path: "data/config.json".to_string(),
                content_hash: format!("{:x}", Sha256::digest(&excluded_source)),
                observed_size: excluded_source.len() as u64,
                observed_unit_count: 0,
                policy_version: codestory_contracts::workspace::OVERSIZED_SOURCE_POLICY_VERSION
                    .to_string(),
                byte_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_SOURCE_BYTE_CAP,
                structural_unit_cap: codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
            }],
        );
        drop(storage);

        let error = lexical_input_fingerprint(project_a.path(), Some(&storage_path))
            .expect_err("foreign core policy must fail closed");

        assert!(format!("{error:#}").contains(
            "source policy exclusion manifest does not match the complete core publication"
        ));
    }

    #[test]
    fn omitted_inputs_are_persisted_in_readiness_metadata() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn ok() {}").expect("source");
        std::fs::write(
            project.path().join("large.rs"),
            vec![b'x'; MAX_FILE_BYTES as usize + 1],
        )
        .expect("large");
        std::fs::write(project.path().join("invalid.rs"), [0xff, 0xfe, 0xfd])
            .expect("invalid utf-8");
        let data = TempDir::new().expect("data");
        build(project.path(), data.path(), "coverage", "input");
        let coverage = lexical_shard_coverage(data.path(), "coverage", "input").expect("coverage");
        assert_eq!(coverage.omitted_oversized, 1);
        assert_eq!(coverage.unreadable_files, 1);
        assert!(!coverage.complete());
        assert_eq!(coverage.omitted_path_sample, ["large.rs"]);
        assert_eq!(coverage.unreadable_path_sample, ["invalid.rs"]);
    }

    #[test]
    fn all_omitted_inputs_still_publish_coverage_metadata() {
        let project = TempDir::new().expect("project");
        std::fs::write(
            project.path().join("large.rs"),
            vec![b'x'; MAX_FILE_BYTES as usize + 1],
        )
        .expect("large");
        std::fs::write(project.path().join("invalid.rs"), [0xff, 0xfe, 0xfd])
            .expect("invalid utf-8");
        let data = TempDir::new().expect("data");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        assert_eq!(fingerprint.file_count, 0);

        let rebuilt = build_lexical_shard(
            project.path(),
            None,
            data.path(),
            "all-omitted",
            &fingerprint,
            "input",
        )
        .expect("build empty shard");
        let coverage =
            lexical_shard_coverage(data.path(), "all-omitted", "input").expect("coverage");

        assert_eq!(rebuilt, fingerprint);
        assert_eq!(coverage.discovered_files, 2);
        assert_eq!(coverage.indexed_files, 0);
        assert_eq!(coverage.omitted_oversized, 1);
        assert_eq!(coverage.unreadable_files, 1);
        assert!(
            search_lexical_index(
                &shard_dir_for(data.path(), "all-omitted"),
                "input",
                "handler",
                4,
            )
            .expect("empty search")
            .is_empty()
        );
    }

    #[test]
    #[ignore = "measurement fixture; run with --ignored --nocapture for PR corpus/query evidence"]
    fn report_jsonl_to_sqlite_corpus_and_query_delta() {
        let root = TempDir::new().expect("root");
        let mut reports = Vec::new();
        for corpus_documents in [1_000_usize, 10_000] {
            let generation = format!("benchmark-{corpus_documents}");
            let shard = shard_dir_for(root.path(), &generation);
            std::fs::create_dir_all(&shard).expect("shard");
            let documents = (0..corpus_documents)
                .map(|index| LexicalDocument {
                    path: format!("src/file_{index:05}.rs"),
                    content: format!("pub fn symbol_{index:05}() {{ handler_{index:05}(); }}"),
                    source: LexicalDocumentSource::LexicalSource,
                    node_id: None,
                    symbol_name: None,
                    start_line: None,
                })
                .collect::<Vec<_>>();
            let coverage = LexicalCoverage {
                discovered_files: documents.len() as u32,
                indexed_files: documents.len() as u32,
                ..Default::default()
            };
            let fingerprint = LexicalInputFingerprint {
                file_count: documents.len() as u32,
                hash: lexical_documents_hash(&documents, &coverage),
                coverage: coverage.clone(),
            };
            let index_path = shard.join(LEXICAL_INDEX_FILE);
            write_lexical_database(
                &index_path,
                &generation,
                "benchmark-input",
                &fingerprint,
                |visit| {
                    for document in &documents {
                        visit(document)?;
                    }
                    Ok(coverage.clone())
                },
            )
            .expect("write sqlite");
            let jsonl = documents
                .iter()
                .flat_map(|document| {
                    let mut row = serde_json::to_vec(document).expect("serialize JSONL row");
                    row.push(b'\n');
                    row
                })
                .collect::<Vec<_>>();
            let jsonl_path = shard.join(LEGACY_INDEX_FILE);
            std::fs::write(&jsonl_path, &jsonl).expect("write JSONL");

            let mut deep_validation_micros = Vec::new();
            for _ in 0..7 {
                let started = std::time::Instant::now();
                // Measure the uncached scan on purpose: the sealed receipt
                // would answer every repeat and report the cost of a HashMap
                // lookup instead of the corpus pass this fixture reports.
                let metadata =
                    verify_lexical_database_contents(&index_path).expect("deep validation");
                match_lexical_shard_expectations(
                    &metadata,
                    &generation,
                    "benchmark-input",
                    Some((fingerprint.file_count, fingerprint.hash.as_str())),
                )
                .expect("deep validation expectations");
                deep_validation_micros.push(started.elapsed().as_micros() as u64);
            }

            let query = format!("symbol_{:05}", corpus_documents - 1);
            let mut sqlite_micros = Vec::new();
            let mut jsonl_micros = Vec::new();
            let mut sqlite_top = None;
            let mut jsonl_top = None;
            for _ in 0..21 {
                let started = std::time::Instant::now();
                let hits = search_lexical_index(&shard, "benchmark-input", &query, 8)
                    .expect("SQLite search");
                sqlite_micros.push(started.elapsed().as_micros() as u64);
                sqlite_top = hits.first().map(|hit| hit.path.clone());

                let started = std::time::Instant::now();
                let parsed = std::fs::read_to_string(&jsonl_path)
                    .expect("read JSONL")
                    .lines()
                    .map(|line| serde_json::from_str::<LexicalDocument>(line).expect("parse row"))
                    .collect::<Vec<_>>();
                let hits = legacy_full_scan_for_measurement(&parsed, &query, 8);
                jsonl_micros.push(started.elapsed().as_micros() as u64);
                jsonl_top = hits.first().map(|hit| hit.path.clone());
            }
            deep_validation_micros.sort_unstable();
            sqlite_micros.sort_unstable();
            jsonl_micros.sort_unstable();
            assert_eq!(sqlite_top, jsonl_top);
            reports.push(serde_json::json!({
                "corpus_documents": documents.len(),
                "jsonl_bytes": std::fs::metadata(jsonl_path).expect("JSONL metadata").len(),
                "sqlite_bytes": std::fs::metadata(index_path).expect("SQLite metadata").len(),
                "deep_validation_median_us": deep_validation_micros[deep_validation_micros.len() / 2],
                "jsonl_median_query_us": jsonl_micros[jsonl_micros.len() / 2],
                "sqlite_warm_median_query_us": sqlite_micros[sqlite_micros.len() / 2],
            }));
        }
        println!("{}", serde_json::json!({ "corpora": reports }));
    }

    fn legacy_full_scan_for_measurement(
        documents: &[LexicalDocument],
        query: &str,
        limit: usize,
    ) -> Vec<LexicalHit> {
        let tokens = lexical_query_tokens(query);
        let frequencies = tokens
            .iter()
            .map(|token| {
                documents
                    .iter()
                    .filter(|document| {
                        document.path.to_ascii_lowercase().contains(token.as_str())
                            || document
                                .content
                                .to_ascii_lowercase()
                                .contains(token.as_str())
                    })
                    .count()
            })
            .collect::<Vec<_>>();
        let weights = frequencies
            .iter()
            .map(|frequency| lexical_token_weight(*frequency, documents.len()))
            .collect::<Vec<_>>();
        let required = required_lexical_match_count(tokens.len());
        let mut hits = documents
            .iter()
            .filter_map(|document| {
                let token_match = lexical_token_match(
                    &tokens,
                    &weights,
                    &document.path.to_ascii_lowercase(),
                    &document.content.to_ascii_lowercase(),
                );
                (token_match.matched_count >= required).then(|| {
                    let (target, matched_line, source_excerpt) =
                        if document.source == LexicalDocumentSource::LexicalSource {
                            lexical_source_target(
                                &document.path,
                                &document.content,
                                &tokens,
                                token_match.content_weight > 0.0,
                            )
                        } else {
                            (None, None, None)
                        };
                    LexicalHit {
                        path: document.path.clone(),
                        source: document.source,
                        node_id: document.node_id.clone(),
                        symbol_name: document.symbol_name.clone(),
                        start_line: document.start_line.or(matched_line),
                        target,
                        source_excerpt,
                        score: score_lexical_match(&document.path, document.source, &token_match),
                    }
                })
            })
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.path.cmp(&right.path))
        });
        hits.truncate(limit);
        hits
    }

    fn shard_modified_nanos(index: &Path) -> u64 {
        std::fs::metadata(index)
            .expect("shard metadata")
            .modified()
            .expect("shard mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("shard mtime after epoch")
            .as_nanos() as u64
    }

    fn restore_shard_modified_nanos(index: &Path, nanos: u64) {
        let file = std::fs::File::options()
            .write(true)
            .open(index)
            .expect("open shard to restore times");
        let restored = std::time::UNIX_EPOCH + std::time::Duration::from_nanos(nanos);
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(restored)
                .set_accessed(restored),
        )
        .expect("restore shard times");
    }

    #[test]
    fn repeated_probes_of_one_generation_deep_scan_the_shard_once() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "pub fn handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "receipt-reuse", "input");
        let fingerprint = lexical_input_fingerprint(project.path(), None).expect("fingerprint");
        assert_eq!(
            lexical_shard_receipt_stats(data.path(), "receipt-reuse"),
            None,
            "publishing a shard must not seal a receipt; only a read path may"
        );

        // Exactly the pair a single health probe performs, then the finalize
        // fence's stricter check over the same generation.
        assert!(shard_has_lexical_index(&shard, "input"));
        lexical_shard_coverage(data.path(), "receipt-reuse", "input").expect("coverage");
        assert!(shard_matches_lexical_input(
            data.path(),
            "receipt-reuse",
            fingerprint.file_count,
            &fingerprint.hash,
            "input",
        ));

        let stats = lexical_shard_receipt_stats(data.path(), "receipt-reuse")
            .expect("a successful deep verification seals a receipt");
        assert_eq!(
            (stats.validations, stats.reuses, stats.invalidations),
            (1, 2, 0),
            "three probes of one unchanged generation must scan the FTS mirror once"
        );
    }

    #[test]
    fn a_sealed_receipt_cannot_hide_in_place_corruption_of_its_shard() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn handler() {}").expect("source");
        std::fs::write(project.path().join("other.rs"), "fn unrelated() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "receipt-seal", "input");
        let index = shard.join(LEXICAL_INDEX_FILE);

        lexical_shard_coverage(data.path(), "receipt-seal", "input")
            .expect("healthy generation verifies");
        assert!(
            lexical_shard_receipt_stats(data.path(), "receipt-seal").is_some(),
            "the healthy verdict must be sealed, or corruption below proves nothing"
        );

        // Rewrite the shard where it lies, keeping the forged text the same
        // length as the document it shadows, then put the modification time
        // back: exactly what a restore-in-place or a torn write leaves behind,
        // and invisible to a length-and-mtime check.
        let published = std::fs::metadata(&index).expect("shard metadata");
        let modified = shard_modified_nanos(&index);
        let length = published.len();
        let permissions = published.permissions();
        make_test_file_writable(&index);
        let connection = Connection::open(&index).expect("open writable");
        connection
            .execute(
                "UPDATE lexical_fts SET content = 'fn unrelated() {;'
                 WHERE rowid = (SELECT id FROM lexical_documents WHERE path = 'other.rs')",
                [],
            )
            .expect("forge FTS row");
        drop(connection);
        restore_shard_modified_nanos(&index, modified);
        std::fs::set_permissions(&index, permissions.clone()).expect("restore shard permissions");
        let corrupted = std::fs::metadata(&index).expect("shard metadata");
        assert_eq!(
            shard_modified_nanos(&index),
            modified,
            "the corruption must be invisible to a modification-time check"
        );
        assert_eq!(
            corrupted.len(),
            length,
            "the corruption must be invisible to a file-length check"
        );
        assert_eq!(
            corrupted.permissions().readonly(),
            permissions.readonly(),
            "the corruption must be invisible to a permission check"
        );

        let after = lexical_shard_coverage(data.path(), "receipt-seal", "input");

        assert!(
            after.is_err(),
            "the sealed receipt answered for bytes that no longer exist: {after:?}"
        );
        assert!(
            format!("{:#}", after.expect_err("corrupt shard"))
                .contains("FTS rows do not match immutable documents"),
            "the re-run deep scan must report the corruption it found"
        );
        assert!(!shard_has_lexical_index(&shard, "input"));
        assert_eq!(
            lexical_shard_receipt_stats(data.path(), "receipt-seal"),
            None,
            "a failed verification must leave no receipt behind"
        );
    }

    #[test]
    fn rebuilding_a_damaged_generation_in_place_reseals_it_as_healthy() {
        let project = TempDir::new().expect("project");
        std::fs::write(project.path().join("lib.rs"), "fn handler() {}").expect("source");
        let data = TempDir::new().expect("data");
        let shard = build(project.path(), data.path(), "receipt-repair", "input");
        let index = shard.join(LEXICAL_INDEX_FILE);
        lexical_shard_coverage(data.path(), "receipt-repair", "input").expect("healthy");

        make_test_file_writable(&index);
        std::fs::write(&index, b"not sqlite").expect("corrupt shard");
        assert!(lexical_shard_coverage(data.path(), "receipt-repair", "input").is_err());

        // The same generation id rebuilt in place: this is the self-repair the
        // finalize fall-through reaches.
        let _repaired = build(project.path(), data.path(), "receipt-repair", "input");

        lexical_shard_coverage(data.path(), "receipt-repair", "input")
            .expect("the repaired generation verifies again");
        assert!(shard_has_lexical_index(&shard, "input"));
    }
}
