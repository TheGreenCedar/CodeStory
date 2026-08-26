use super::*;
use codestory_contracts::proof_resolution::{
    CallResolutionFact, CalleeForm, CanonicalCallsiteIdentity, DependencyFileHash,
    EXACT_CALL_RESOLUTION_ALGORITHM, ExactCallsite, ExactCallsiteCorrelationFailure,
    ExactSyntaxCallsiteCorrelationInput, FileId, INTERNAL_RESOLUTION_PRODUCER,
    OrdinaryCallEdgeCorrelationInput, PROOF_RESOLUTION_FACT_SCHEMA_VERSION, ProofResolutionAdapter,
    ProofResolutionFunnelCounts, ProofResolutionFunnelRow, ProofResolutionProjection,
    ProofResolutionReason, ProofResolutionStatus, ResolutionEvidence, ResolutionEvidenceKind,
    ResolutionProvenance, correlate_exact_syntax_callsites, parse_canonical_callsite_identity,
};

const EVIDENCE_DIGEST_DOMAIN: &[u8] = b"codestory-proof-resolution-evidence-v1\0";
const FACT_ID_DOMAIN: &[u8] = b"codestory-proof-resolution-fact-id-v1\0";
const PUBLICATION_DIGEST_DOMAIN: &[u8] = b"codestory-proof-resolution-publication-v1\0";

#[cfg(debug_assertions)]
static STORE_REPLAY_WORK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[inline]
fn count_store_replay_work(amount: usize) {
    #[cfg(debug_assertions)]
    let _ = STORE_REPLAY_WORK.fetch_add(amount, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(debug_assertions))]
    let _ = amount;
}

#[cfg(debug_assertions)]
pub fn reset_store_replay_work() {
    STORE_REPLAY_WORK.store(0, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(debug_assertions)]
pub fn store_replay_work() -> usize {
    STORE_REPLAY_WORK.load(std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofResolutionPublication {
    pub core_generation_id: String,
    pub core_run_id: String,
    pub fact_schema_version: u32,
    pub adapter_roster: Vec<ProofResolutionAdapter>,
    pub complete: bool,
    pub fact_count: u64,
    pub fact_digest: String,
    pub funnel: Vec<ProofResolutionFunnelRow>,
    pub published_at_epoch_ms: i64,
}

pub fn seal_call_resolution_fact(
    mut fact: CallResolutionFact,
) -> Result<CallResolutionFact, StorageError> {
    let linear_dependency_order = matches!(
        fact.provenance.language_adapter.as_str(),
        "ruby" | "php" | "csharp" | "swift" | "dart"
    );
    if !linear_dependency_order {
        fact.provenance.dependency_file_hashes.sort();
    }
    let unique_dependencies = if linear_dependency_order {
        let mut members = HashSet::new();
        fact.provenance
            .dependency_file_hashes
            .iter()
            .all(|dependency| {
                count_store_replay_work(1);
                members.insert(dependency.file_id)
            })
    } else {
        !fact
            .provenance
            .dependency_file_hashes
            .windows(2)
            .any(|pair| pair[0].file_id == pair[1].file_id)
    };
    if !unique_dependencies {
        return Err(proof_error(
            "dependency file hashes contain a duplicate file",
        ));
    }
    fact.fact_id.clear();
    fact.provenance.evidence_sha256.clear();
    validate_fact_shape(&fact, false)?;
    let bytes = serde_json::to_vec(&fact).map_err(|error| {
        proof_error(format!("failed to serialize canonical proof fact: {error}"))
    })?;
    let evidence_sha256 = digest_hex(EVIDENCE_DIGEST_DOMAIN, &bytes);
    let fact_id = digest_hex(FACT_ID_DOMAIN, evidence_sha256.as_bytes());
    fact.fact_id = fact_id;
    fact.provenance.evidence_sha256 = evidence_sha256;
    validate_fact_shape(&fact, true)?;
    Ok(fact)
}

fn proof_error(message: impl Into<String>) -> StorageError {
    StorageError::Other(format!("proof resolution projection: {}", message.into()))
}

fn digest_hex(domain: &[u8], bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_canonical_json<T>(value: &str, label: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let parsed: T = serde_json::from_str(value)
        .map_err(|error| format!("stored {label} JSON is invalid: {error}"))?;
    let canonical = serde_json::to_string(&parsed)
        .map_err(|error| format!("stored {label} JSON cannot be serialized: {error}"))?;
    if canonical != value {
        return Err(format!(
            "stored {label} JSON bytes are not canonical typed serialization"
        ));
    }
    Ok(parsed)
}

fn validate_fact_shape(fact: &CallResolutionFact, require_seal: bool) -> Result<(), StorageError> {
    let linear_dependency_order = matches!(
        fact.provenance.language_adapter.as_str(),
        "ruby" | "php" | "csharp" | "swift" | "dart"
    );
    let dependencies_are_canonical = if linear_dependency_order {
        let mut members = HashSet::new();
        fact.provenance
            .dependency_file_hashes
            .iter()
            .all(|dependency| {
                count_store_replay_work(1);
                members.insert(dependency.file_id)
            })
    } else {
        !fact
            .provenance
            .dependency_file_hashes
            .windows(2)
            .any(|pair| pair[0].file_id >= pair[1].file_id)
    };
    if fact.callsite.file_id.0 == 0
        || fact.caller.0 == 0
        || !is_sha256(&fact.callsite.source_sha256)
        || fact.callsite.start_byte >= fact.callsite.end_byte_exclusive
        || fact.callsite.line == 0
        || fact.callsite.column == 0
        || fact.callsite.raw_target.trim().is_empty()
    {
        return Err(proof_error("fact contains an invalid callsite or caller"));
    }
    if !fact.reason.matches_status(fact.status) {
        return Err(proof_error("closed status and reason disagree"));
    }
    if fact.provenance.producer != INTERNAL_RESOLUTION_PRODUCER
        || fact.provenance.fact_schema_version != PROOF_RESOLUTION_FACT_SCHEMA_VERSION
        || fact.provenance.algorithm != EXACT_CALL_RESOLUTION_ALGORITHM
        || fact.provenance.language_adapter.trim().is_empty()
        || fact.provenance.language_adapter_version.trim().is_empty()
        || !is_sha256(&fact.provenance.parser_fingerprint)
    {
        return Err(proof_error(
            "fact provenance is not the internal schema-v1 producer",
        ));
    }
    if fact.provenance.dependency_file_hashes.is_empty()
        || fact
            .provenance
            .dependency_file_hashes
            .iter()
            .any(|dependency| dependency.file_id.0 == 0 || !is_sha256(&dependency.source_sha256))
        || !dependencies_are_canonical
    {
        return Err(proof_error(
            "dependency file hashes are empty, invalid, duplicate, or noncanonical",
        ));
    }
    let source_dependency = fact
        .provenance
        .dependency_file_hashes
        .iter()
        .find(|dependency| dependency.file_id == fact.callsite.file_id);
    if source_dependency.map(|dependency| dependency.source_sha256.as_str())
        != Some(fact.callsite.source_sha256.as_str())
    {
        return Err(proof_error(
            "callsite source hash is not bound in dependencies",
        ));
    }
    match fact.status {
        ProofResolutionStatus::Exact
            if fact.target.is_none()
                || fact.edge_id.is_none()
                || fact.raw_edge_target.is_none()
                || fact
                    .raw_callsite_identity
                    .as_deref()
                    .is_none_or(str::is_empty)
                || !fact.lookup_domain_complete
                || fact.evidence_chain.is_empty() =>
        {
            return Err(proof_error(
                "Exact requires target, edge, complete domain, and typed evidence",
            ));
        }
        ProofResolutionStatus::Exact => {}
        _ if fact.edge_id.is_some()
            || fact.raw_edge_target.is_some()
            || fact.raw_callsite_identity.is_some() =>
        {
            return Err(proof_error("only Exact may bind an ordinary CALL edge"));
        }
        _ => {}
    }
    if require_seal && (!is_sha256(&fact.fact_id) || !is_sha256(&fact.provenance.evidence_sha256)) {
        return Err(proof_error("fact id or evidence digest is invalid"));
    }
    Ok(())
}

fn validate_adapter_roster(
    facts: &[CallResolutionFact],
    adapter_roster: &[ProofResolutionAdapter],
) -> Result<(), StorageError> {
    let mut sorted_roster = adapter_roster.to_vec();
    sorted_roster.sort();
    if adapter_roster.is_empty()
        || adapter_roster.iter().any(|adapter| {
            adapter.language.trim().is_empty() || adapter.adapter_version.trim().is_empty()
        })
        || sorted_roster.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err(proof_error(
            "adapter roster is empty, invalid, or duplicated",
        ));
    }
    if facts.iter().any(|fact| {
        sorted_roster
            .iter()
            .filter(|adapter| {
                adapter.language == fact.provenance.language_adapter
                    && adapter.adapter_version == fact.provenance.language_adapter_version
            })
            .count()
            != 1
    }) {
        return Err(proof_error(
            "fact provenance adapter is not represented exactly once in the adapter roster",
        ));
    }
    Ok(())
}

fn validate_fact_seal(fact: &CallResolutionFact) -> Result<(), StorageError> {
    validate_fact_shape(fact, true)?;
    let resealed = seal_call_resolution_fact(fact.clone())?;
    if resealed.fact_id != fact.fact_id
        || resealed.provenance.evidence_sha256 != fact.provenance.evidence_sha256
    {
        return Err(proof_error("evidence digest or fact id mismatch"));
    }
    Ok(())
}

fn dependency_file_ids(fact: &CallResolutionFact) -> BTreeSet<FileId> {
    fact.provenance
        .dependency_file_hashes
        .iter()
        .map(|dependency| dependency.file_id)
        .collect()
}

struct ProofResolutionValidationContext {
    file_by_id: HashMap<i64, FileInfo>,
    file_content_hash_by_id: HashMap<i64, String>,
    rust_file_ids_by_path: HashMap<PathBuf, Vec<FileId>>,
    rust_root_count_by_directory: HashMap<PathBuf, usize>,
    node_by_id: HashMap<NodeId, Node>,
    edges: Vec<Edge>,
    edge_index_by_id: HashMap<EdgeId, usize>,
    ordinary_call_edge_indices: Vec<usize>,
    parsed_callsite_identity_by_edge: HashMap<EdgeId, CanonicalCallsiteIdentity>,
    import_relations: HashMap<(NodeId, NodeId, NodeId), ProofRelationState>,
    swift_module_import_relations: HashMap<(NodeId, NodeId), ProofRelationState>,
    swift_public_node_ids: HashSet<NodeId>,
    dart_import_visibility_by_node: HashMap<NodeId, DartImportVisibility>,
    dart_runtime_closed_nodes: HashSet<NodeId>,
    dart_overridden_owner_methods: HashSet<(NodeId, String)>,
    dart_ancestry_invalid_domains: HashSet<String>,
    typescript_directory_import_relations:
        HashMap<(NodeId, NodeId), TypescriptDirectoryImportState>,
    member_relations: HashMap<(NodeId, NodeId), ProofRelationState>,
    member_by_owner_and_name: HashMap<(NodeId, String), Option<NodeId>>,
    python_import_paths: HashMap<(NodeId, NodeId), Vec<Vec<NodeId>>>,
    python_file_ids_by_path: HashMap<PathBuf, Vec<FileId>>,
    python_attestation_error_by_file: HashMap<i64, String>,
    java_package_identity_by_file: HashMap<i64, String>,
    java_dependency_ids_by_package: HashMap<String, BTreeSet<FileId>>,
    csd_domain_identity_by_file: HashMap<i64, String>,
    csd_dependency_ids_by_domain: HashMap<String, Vec<FileId>>,
    ruby_dependency_file_ids: Vec<FileId>,
    php_namespace_identity_by_file: HashMap<i64, PhpNamespaceIdentity>,
    php_dependency_ids_by_namespace: HashMap<PhpNamespaceIdentity, Vec<FileId>>,
    php_namespace_domain_invalid: bool,
    go_package_identity_by_file: HashMap<i64, GoPackageIdentity>,
    go_dependency_ids_by_package: HashMap<GoPackageIdentity, BTreeSet<FileId>>,
    go_attestation_error_by_file: HashMap<i64, String>,
    live_go_sources_authenticated: bool,
}

#[derive(Default)]
struct DartImportVisibility {
    shown: Option<HashSet<String>>,
    hidden: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PhpNamespaceIdentity {
    Global,
    Named(String),
}

fn canonical_php_namespace_identity(name: &str) -> Option<String> {
    let components = name.split('\\').collect::<Vec<_>>();
    count_store_replay_work(components.len());
    (!components.is_empty()
        && components.iter().all(|component| {
            let mut chars = component.chars();
            chars
                .next()
                .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
                && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        }))
    .then(|| components.join("."))
}

#[allow(clippy::type_complexity)]
fn prepare_ruby_php_domain_closure(
    files: &[FileInfo],
    nodes: &HashMap<NodeId, Node>,
) -> (
    Vec<FileId>,
    HashMap<i64, PhpNamespaceIdentity>,
    HashMap<PhpNamespaceIdentity, Vec<FileId>>,
    bool,
) {
    let ruby_dependency_file_ids = files
        .iter()
        .filter(|file| file.indexed && file.language == "ruby")
        .map(|file| {
            count_store_replay_work(1);
            FileId(file.id)
        })
        .collect::<Vec<_>>();
    let php_file_ids = files
        .iter()
        .filter(|file| file.indexed && file.language == "php")
        .map(|file| {
            count_store_replay_work(1);
            file.id
        })
        .collect::<HashSet<_>>();
    let mut php_names_by_file = HashMap::<i64, Vec<String>>::new();
    for node in nodes
        .values()
        .filter(|node| node.kind == NodeKind::NAMESPACE)
    {
        count_store_replay_work(1);
        let Some(file_id) = node.file_node_id else {
            continue;
        };
        if !php_file_ids.contains(&file_id.0) {
            continue;
        }
        count_store_replay_work(1);
        if let Some(name) = canonical_php_namespace_identity(&node.serialized_name) {
            count_store_replay_work(1);
            php_names_by_file.entry(file_id.0).or_default().push(name);
        } else {
            php_names_by_file
                .entry(file_id.0)
                .or_default()
                .extend([String::new(), String::new()]);
        }
    }
    let mut identity_by_file = HashMap::new();
    let mut dependencies_by_namespace = HashMap::<PhpNamespaceIdentity, Vec<FileId>>::new();
    let mut invalid = false;
    for file in files
        .iter()
        .filter(|file| file.indexed && file.language == "php")
    {
        count_store_replay_work(1);
        let names = php_names_by_file
            .get(&file.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let identity = match names {
            [] => PhpNamespaceIdentity::Global,
            [name] if !name.is_empty() => PhpNamespaceIdentity::Named(name.clone()),
            _ => {
                invalid = true;
                continue;
            }
        };
        identity_by_file.insert(file.id, identity.clone());
        dependencies_by_namespace
            .entry(identity)
            .or_default()
            .push(FileId(file.id));
        count_store_replay_work(2);
    }
    (
        ruby_dependency_file_ids,
        identity_by_file,
        dependencies_by_namespace,
        invalid,
    )
}

fn ruby_php_dependency_ids(
    fact: &CallResolutionFact,
    context: &ProofResolutionValidationContext,
) -> Result<Vec<FileId>, StorageError> {
    let mut dependencies = Vec::new();
    let mut members = HashSet::new();
    let mut append = |file_id: FileId| {
        count_store_replay_work(1);
        if members.insert(file_id) {
            dependencies.push(file_id);
        }
    };
    append(fact.callsite.file_id);
    if fact.provenance.language_adapter == "ruby" {
        for file_id in &context.ruby_dependency_file_ids {
            append(*file_id);
        }
        return Ok(dependencies);
    }
    if context.php_namespace_domain_invalid {
        return Err(proof_error(
            "PHP namespace domain contains an invalid identity",
        ));
    }
    for file_id in std::iter::once(fact.callsite.file_id).chain(
        fact.evidence_chain
            .iter()
            .flat_map(ResolutionEvidence::node_ids)
            .chain(fact.target)
            .filter_map(|node_id| {
                context
                    .node_by_id
                    .get(&node_id)
                    .and_then(|node| node.file_node_id)
                    .map(|file| FileId(file.0))
            }),
    ) {
        count_store_replay_work(1);
        let identity = context
            .php_namespace_identity_by_file
            .get(&file_id.0)
            .ok_or_else(|| proof_error("PHP dependency has no canonical namespace identity"))?;
        let domain = context
            .php_dependency_ids_by_namespace
            .get(identity)
            .ok_or_else(|| proof_error("PHP dependency namespace domain is missing"))?;
        for dependency in domain {
            append(*dependency);
        }
    }
    Ok(dependencies)
}

fn prepare_rust_file_identity(
    file_by_id: &HashMap<i64, FileInfo>,
) -> (HashMap<PathBuf, Vec<FileId>>, HashMap<PathBuf, usize>) {
    let mut file_ids_by_path = HashMap::<PathBuf, Vec<FileId>>::new();
    let mut root_count_by_directory = HashMap::<PathBuf, usize>::new();
    for file in file_by_id.values().filter(|file| file.language == "rust") {
        count_store_replay_work(1);
        file_ids_by_path
            .entry(file.path.clone())
            .or_default()
            .push(FileId(file.id));
        if matches!(
            file.path.file_name().and_then(|name| name.to_str()),
            Some("lib.rs" | "main.rs")
        ) && let Some(directory) = file.path.parent()
        {
            *root_count_by_directory
                .entry(directory.to_path_buf())
                .or_default() += 1;
        }
    }
    (file_ids_by_path, root_count_by_directory)
}

fn rust_dependency_path_is_ancestor(
    source: &FileInfo,
    dependency: &FileInfo,
    context: &ProofResolutionValidationContext,
) -> bool {
    if dependency.language != "rust" || dependency.id == source.id {
        return false;
    }
    if context
        .rust_file_ids_by_path
        .get(&dependency.path)
        .is_none_or(|files| files.len() != 1)
    {
        return false;
    }
    let Some(file_name) = dependency.path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(directory) = dependency.path.parent() else {
        return false;
    };
    let (module_base, conflicting_form) = match file_name {
        "lib.rs" | "main.rs" => {
            if context.rust_root_count_by_directory.get(directory).copied() != Some(1) {
                return false;
            }
            (directory.to_path_buf(), None)
        }
        "mod.rs" => {
            let Some(module_directory) = directory.parent() else {
                return false;
            };
            (
                directory.to_path_buf(),
                Some(module_directory.join(format!(
                    "{}.rs",
                    directory.file_name().and_then(|name| name.to_str()).unwrap_or("")
                ))),
            )
        }
        name if name.ends_with(".rs") => (
            dependency.path.with_extension(""),
            Some(dependency.path.with_extension("").join("mod.rs")),
        ),
        _ => return false,
    };
    if conflicting_form.is_some_and(|path| {
        context
            .rust_file_ids_by_path
            .get(&path)
            .is_some_and(|files| !files.is_empty())
    }) {
        return false;
    }
    source.path.starts_with(&module_base) && source.path != dependency.path
}

fn rust_same_file_dependency_ids(
    fact: &CallResolutionFact,
    required: &BTreeSet<FileId>,
    observed: &BTreeSet<FileId>,
    context: &ProofResolutionValidationContext,
) -> Option<BTreeSet<FileId>> {
    if fact.provenance.language_adapter != "rust"
        || fact.callsite.callee_form != CalleeForm::Identifier
        || !matches!(
            fact.evidence_chain.as_slice(),
            [ResolutionEvidence::SameFileDeclaration { declaration }]
                if Some(*declaration) == fact.target
        )
        || !required.is_subset(observed)
    {
        return None;
    }
    let source = context.file_by_id.get(&fact.callsite.file_id.0)?;
    if context
        .rust_file_ids_by_path
        .get(&source.path)
        .is_none_or(|files| files.as_slice() != [fact.callsite.file_id])
    {
        return None;
    }
    observed
        .difference(required)
        .all(|file_id| {
            context
                .file_by_id
                .get(&file_id.0)
                .is_some_and(|dependency| {
                    rust_dependency_path_is_ancestor(source, dependency, context)
                })
        })
        .then(|| observed.clone())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GoPackageIdentity {
    native_directory: PathBuf,
    package_name: String,
}

fn prepare_call_edge_correlation_index(
    edges: &[Edge],
    node_by_id: &HashMap<NodeId, Node>,
) -> (Vec<usize>, HashMap<EdgeId, CanonicalCallsiteIdentity>) {
    let mut indices = Vec::new();
    let mut identities = HashMap::new();
    for (index, edge) in edges.iter().enumerate() {
        count_store_replay_work(1);
        if edge.kind != EdgeKind::CALL || !node_by_id.contains_key(&edge.target) {
            continue;
        }
        indices.push(index);
        if let Some(identity) = edge
            .callsite_identity
            .as_deref()
            .and_then(parse_canonical_callsite_identity)
        {
            identities.insert(edge.id, identity);
        }
    }
    (indices, identities)
}

fn go_package_dependency_ids(
    source_file_id: FileId,
    evidence_ids: &BTreeSet<FileId>,
    context: &ProofResolutionValidationContext,
) -> Result<BTreeSet<FileId>, StorageError> {
    let source_file = context
        .file_by_id
        .get(&source_file_id.0)
        .ok_or_else(|| proof_error("Go source dependency file is missing"))?;
    if source_file
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("_test.go"))
    {
        return Err(proof_error(
            "Go production proof cannot originate in a test file",
        ));
    }
    if let Some(error) = context.go_attestation_error_by_file.get(&source_file_id.0) {
        return Err(proof_error(format!(
            "Go source dependency is not publication-authenticated: {error}"
        )));
    }
    let source_identity = context
        .go_package_identity_by_file
        .get(&source_file_id.0)
        .ok_or_else(|| proof_error("Go source dependency has no authenticated package clause"))?;
    for file_id in evidence_ids {
        count_store_replay_work(1);
        if let Some(error) = context.go_attestation_error_by_file.get(&file_id.0) {
            return Err(proof_error(format!(
                "Go evidence dependency is not publication-authenticated: {error}"
            )));
        }
        if context.go_package_identity_by_file.get(&file_id.0) != Some(source_identity) {
            return Err(proof_error(
                "Go exact evidence crosses its authenticated native package identity",
            ));
        }
    }
    count_store_replay_work(1);
    context
        .go_dependency_ids_by_package
        .get(source_identity)
        .cloned()
        .ok_or_else(|| proof_error("Go authenticated package closure is missing"))
}

fn stored_go_package_dependency_ids(
    source_file_id: FileId,
    required_evidence_ids: &BTreeSet<FileId>,
    stored_fact_ids: &BTreeSet<FileId>,
    context: &ProofResolutionValidationContext,
) -> Result<BTreeSet<FileId>, StorageError> {
    count_store_replay_work(1);
    if !required_evidence_ids.is_subset(stored_fact_ids) {
        return Err(proof_error(
            "stored Go dependency receipt omits source or typed evidence files",
        ));
    }
    let source_file = context
        .file_by_id
        .get(&source_file_id.0)
        .ok_or_else(|| proof_error("stored Go source dependency file is missing"))?;
    let source_parent = source_file
        .path
        .parent()
        .ok_or_else(|| proof_error("stored Go source path has no package directory"))?;
    for file_id in stored_fact_ids {
        count_store_replay_work(1);
        let file = context
            .file_by_id
            .get(&file_id.0)
            .ok_or_else(|| proof_error("stored Go dependency file is missing"))?;
        if file.language != "go"
            || file.path.parent() != Some(source_parent)
            || file
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_test.go"))
        {
            return Err(proof_error(
                "stored Go dependency receipt crosses its authenticated package domain",
            ));
        }
    }
    Ok(stored_fact_ids.clone())
}

fn prepare_java_package_closure(
    file_by_id: &HashMap<i64, FileInfo>,
    node_by_id: &HashMap<NodeId, Node>,
) -> (HashMap<i64, String>, HashMap<String, BTreeSet<FileId>>) {
    let class_qualified_names = node_by_id
        .values()
        .filter(|node| {
            matches!(
                node.kind,
                NodeKind::CLASS | NodeKind::STRUCT | NodeKind::ENUM
            ) && node.file_node_id.is_some_and(|file| {
                file_by_id
                    .get(&file.0)
                    .is_some_and(|file| file.language == "java")
            })
        })
        .filter_map(|node| node.qualified_name.clone())
        .collect::<HashSet<_>>();
    let mut packages_by_file = HashMap::<i64, BTreeSet<String>>::new();
    for node in node_by_id.values().filter(|node| {
        matches!(
            node.kind,
            NodeKind::CLASS | NodeKind::STRUCT | NodeKind::ENUM
        ) && node.file_node_id.is_some_and(|file| {
            file_by_id
                .get(&file.0)
                .is_some_and(|file| file.language == "java")
        })
    }) {
        count_store_replay_work(1);
        let Some(file_id) = node.file_node_id else {
            continue;
        };
        let Some((parent, _)) = node
            .qualified_name
            .as_deref()
            .and_then(|qualified| qualified.rsplit_once('.'))
        else {
            continue;
        };
        if parent.is_empty() || class_qualified_names.contains(parent) {
            continue;
        }
        packages_by_file
            .entry(file_id.0)
            .or_default()
            .insert(parent.to_owned());
    }
    let mut package_identity_by_file = HashMap::new();
    let mut dependency_ids_by_package = HashMap::<String, BTreeSet<FileId>>::new();
    for (file_id, packages) in packages_by_file {
        count_store_replay_work(1);
        if packages.len() != 1 {
            continue;
        }
        let package = packages.into_iter().next().expect("one Java package");
        package_identity_by_file.insert(file_id, package.clone());
        dependency_ids_by_package
            .entry(package.clone())
            .or_default()
            .insert(FileId(file_id));
    }
    (package_identity_by_file, dependency_ids_by_package)
}

fn prepare_csd_domain_closure(
    files: &[FileInfo],
    node_by_id: &HashMap<NodeId, Node>,
) -> (HashMap<i64, String>, HashMap<String, Vec<FileId>>) {
    let file_language_by_id = files
        .iter()
        .map(|file| (file.id, file.language.as_str()))
        .collect::<HashMap<_, _>>();
    let mut csharp_names = HashMap::<i64, Vec<String>>::new();
    let mut csharp_members = HashMap::<i64, HashSet<String>>::new();
    for node in node_by_id
        .values()
        .filter(|node| node.kind == NodeKind::NAMESPACE)
    {
        let Some(file_id) = node.file_node_id else {
            continue;
        };
        if file_language_by_id.get(&file_id.0).copied() != Some("csharp")
            || node.serialized_name.is_empty()
        {
            continue;
        }
        let members = csharp_members.entry(file_id.0).or_default();
        if members.insert(node.serialized_name.clone()) {
            csharp_names
                .entry(file_id.0)
                .or_default()
                .push(node.serialized_name.clone());
        }
    }
    let mut identity_by_file = HashMap::new();
    let mut dependency_ids_by_domain = HashMap::<String, Vec<FileId>>::new();
    for file in files
        .iter()
        .filter(|file| matches!(file.language.as_str(), "csharp" | "swift" | "dart"))
    {
        count_store_replay_work(1);
        let identity = match file.language.as_str() {
            "csharp" => match csharp_names.get(&file.id).map(Vec::as_slice) {
                None | Some([]) => Some("csharp:global".to_string()),
                Some([name]) => Some(format!("csharp:{name}")),
                Some(_) => None,
            },
            "swift" => swift_source_domain(&file.path),
            "dart" => dart_source_domain(&file.path),
            _ => None,
        };
        let Some(identity) = identity else {
            continue;
        };
        identity_by_file.insert(file.id, identity.clone());
        dependency_ids_by_domain
            .entry(identity)
            .or_default()
            .push(FileId(file.id));
    }
    (identity_by_file, dependency_ids_by_domain)
}

fn prepare_swift_public_nodes(
    storage: &Storage,
    files: &[FileInfo],
    nodes: &HashMap<NodeId, Node>,
) -> Result<HashSet<NodeId>, StorageError> {
    let swift_file_ids = files
        .iter()
        .filter_map(|file| (file.language == "swift").then_some(file.id))
        .collect::<HashSet<_>>();
    let swift_node_ids = nodes
        .values()
        .filter_map(|node| {
            count_store_replay_work(1);
            node.file_node_id
                .filter(|file| swift_file_ids.contains(&file.0))
                .filter(|_| {
                    matches!(
                        node.kind,
                        NodeKind::STRUCT
                            | NodeKind::CLASS
                            | NodeKind::ENUM
                            | NodeKind::FUNCTION
                            | NodeKind::METHOD
                    )
                })
                .map(|_| node.id)
        })
        .collect::<Vec<_>>();
    let access = storage.get_component_access_map_for_nodes(&swift_node_ids)?;
    let mut public = HashSet::new();
    for node_id in swift_node_ids {
        count_store_replay_work(1);
        if access.get(&node_id) == Some(&AccessKind::Public) {
            public.insert(node_id);
        }
    }
    Ok(public)
}

fn prepare_dart_dispatch_closure(
    files: &[FileInfo],
    hashes: &HashMap<i64, String>,
    nodes: &HashMap<NodeId, Node>,
    domain_by_file: &HashMap<i64, String>,
    member_by_owner_and_name: &HashMap<(NodeId, String), Option<NodeId>>,
) -> (HashSet<NodeId>, HashSet<(NodeId, String)>, HashSet<String>) {
    let mut lines_by_file = HashMap::<i64, Vec<String>>::new();
    for file in files.iter().filter(|file| file.language == "dart") {
        count_store_replay_work(1);
        let Ok(bytes) = std::fs::read(&file.path) else {
            continue;
        };
        if hashes.get(&file.id) != Some(&format!("{:x}", Sha256::digest(&bytes))) {
            continue;
        }
        let Ok(source) = String::from_utf8(bytes) else {
            continue;
        };
        lines_by_file.insert(file.id, source.lines().map(str::to_string).collect());
    }
    let mut class_by_domain_and_name = HashMap::<(String, String), Option<NodeId>>::new();
    let mut class_identity_by_node = HashMap::<NodeId, (String, String)>::new();
    let mut parent_by_class = HashMap::<NodeId, Option<String>>::new();
    let mut closed = HashSet::new();
    let mut super_by_class = Vec::<(NodeId, String, String)>::new();
    for node in nodes.values().filter(|node| node.kind == NodeKind::CLASS) {
        count_store_replay_work(1);
        let Some(file) = node.file_node_id else {
            continue;
        };
        let Some(domain) = domain_by_file.get(&file.0).cloned() else {
            continue;
        };
        let Some(line) = node.start_line.and_then(|line| {
            lines_by_file
                .get(&file.0)
                .and_then(|lines| lines.get(line.saturating_sub(1) as usize))
        }) else {
            continue;
        };
        let declaration_column = line.len().saturating_sub(line.trim_start().len()) as u32 + 1;
        if node.start_col != Some(declaration_column) {
            continue;
        }
        let name = graph_leaf_name(&node.serialized_name).to_string();
        class_identity_by_node.insert(node.id, (domain.clone(), name.clone()));
        class_by_domain_and_name
            .entry((domain.clone(), name))
            .and_modify(|entry| *entry = None)
            .or_insert(Some(node.id));
        let trimmed = line.trim_start();
        if trimmed.starts_with("final class ") || trimmed.starts_with("sealed class ") {
            closed.insert(node.id);
        }
        let tokens = line
            .split(|character: char| {
                character.is_whitespace() || matches!(character, '{' | '(' | ')' | '<' | '>' | ',')
            })
            .collect::<Vec<_>>();
        let super_name = tokens
            .iter()
            .position(|token| *token == "extends")
            .and_then(|index| tokens[index + 1..].iter().find(|token| !token.is_empty()))
            .map(|name| (*name).to_string());
        parent_by_class.insert(node.id, super_name.clone());
        if let Some(super_name) = super_name {
            super_by_class.push((node.id, domain, super_name));
        }
    }
    let unique_nodes = class_by_domain_and_name
        .values()
        .filter_map(|entry| *entry)
        .collect::<HashSet<_>>();
    closed.retain(|node| unique_nodes.contains(node));
    let mut member_names_by_owner = HashMap::<NodeId, Vec<String>>::new();
    for ((owner, name), member) in member_by_owner_and_name {
        count_store_replay_work(1);
        if member.is_some() {
            member_names_by_owner
                .entry(*owner)
                .or_default()
                .push(name.clone());
        }
    }
    let mut overridden = HashSet::new();
    let mut invalid_domains = HashSet::new();
    for (class, (domain, _)) in &class_identity_by_node {
        let mut current = *class;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(current) {
                invalid_domains.insert(domain.clone());
                break;
            }
            let Some(parent_name) = parent_by_class.get(&current).and_then(Option::as_deref) else {
                break;
            };
            let Some(Some(parent)) =
                class_by_domain_and_name.get(&(domain.clone(), parent_name.to_string()))
            else {
                invalid_domains.insert(domain.clone());
                break;
            };
            current = *parent;
        }
    }
    for (subclass, domain, _) in super_by_class {
        count_store_replay_work(1);
        if let Some(names) = member_names_by_owner.get(&subclass) {
            let mut current = subclass;
            let mut visited = HashSet::new();
            while visited.insert(current) {
                let Some(parent_name) = parent_by_class.get(&current).and_then(Option::as_deref)
                else {
                    break;
                };
                let Some(Some(owner)) =
                    class_by_domain_and_name.get(&(domain.clone(), parent_name.to_string()))
                else {
                    break;
                };
                for name in names {
                    count_store_replay_work(1);
                    overridden.insert((*owner, name.clone()));
                }
                current = *owner;
            }
        }
    }
    (closed, overridden, invalid_domains)
}

fn path_normal_components(path: &Path) -> Vec<&str> {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect()
}

fn swift_source_domain(path: &Path) -> Option<String> {
    let components = path_normal_components(path);
    if let Some(module) = components
        .windows(2)
        .find_map(|pair| (pair[0] == "Sources").then_some(pair[1]))
        .filter(|module| {
            !module.is_empty()
                && module
                    .chars()
                    .all(|character| character == '_' || character.is_alphanumeric())
        })
    {
        return Some(format!("swift:Sources/{module}"));
    }
    components
        .iter()
        .any(|component| *component == "Source")
        .then(|| "swift:Source".to_string())
}

fn dart_source_domain(path: &Path) -> Option<String> {
    let components = path_normal_components(path);
    let lib = components
        .iter()
        .rposition(|component| *component == "lib")?;
    if lib >= 2 && matches!(components[lib - 2], "pkgs" | "packages") {
        Some(format!(
            "dart:{}/{}/lib",
            components[lib - 2],
            components[lib - 1]
        ))
    } else {
        Some("dart:lib".to_string())
    }
}

fn csd_dependency_ids(
    fact: &CallResolutionFact,
    context: &ProofResolutionValidationContext,
) -> Result<Vec<FileId>, StorageError> {
    let imported = fact
        .evidence_chain
        .iter()
        .any(|evidence| matches!(evidence, ResolutionEvidence::StaticImportBinding { .. }));
    let domain_file = if imported {
        fact.target
            .and_then(|target| context.node_by_id.get(&target))
            .and_then(|target| target.file_node_id)
            .map(|file| file.0)
            .ok_or_else(|| proof_error("nominal import target has no governed domain file"))?
    } else {
        fact.callsite.file_id.0
    };
    let identity = context
        .csd_domain_identity_by_file
        .get(&domain_file)
        .ok_or_else(|| proof_error("nominal exact fact has no authenticated domain identity"))?;
    let domain = context
        .csd_dependency_ids_by_domain
        .get(identity)
        .ok_or_else(|| proof_error("nominal exact fact has no complete dependency domain"))?;
    let mut dependencies = Vec::with_capacity(domain.len().saturating_add(1));
    let mut members = HashSet::new();
    for file_id in std::iter::once(fact.callsite.file_id).chain(domain.iter().copied()) {
        count_store_replay_work(1);
        if members.insert(file_id) {
            dependencies.push(file_id);
        }
    }
    Ok(dependencies)
}

fn java_same_package_dependency_ids(
    fact: &CallResolutionFact,
    required_evidence_ids: &BTreeSet<FileId>,
    context: &ProofResolutionValidationContext,
) -> Result<Option<BTreeSet<FileId>>, StorageError> {
    if fact.provenance.language_adapter != "java"
        || fact.callsite.callee_form != CalleeForm::ExplicitReceiver
    {
        return Ok(None);
    }
    let owner = match fact.evidence_chain.as_slice() {
        [
            ResolutionEvidence::ConstructorBinding { constructor },
            ResolutionEvidence::ExplicitReceiverType { receiver_type },
            ResolutionEvidence::SamePackageDeclaration { .. },
        ] if constructor == receiver_type => *constructor,
        [
            ResolutionEvidence::ExplicitReceiverType { receiver_type },
            ResolutionEvidence::SamePackageDeclaration { .. },
        ] => *receiver_type,
        _ => return Ok(None),
    };
    let package = Storage::validate_java_package_receiver(
        context,
        fact,
        owner,
        fact.target.expect("Exact Java fact has a target"),
    )?;
    let expected = context
        .java_dependency_ids_by_package
        .get(&package)
        .ok_or_else(|| proof_error("Java authenticated package closure is missing"))?;
    if !required_evidence_ids.is_subset(expected) {
        return Err(proof_error(
            "Java package dependency closure omits source or typed evidence files",
        ));
    }
    Ok(Some(expected.clone()))
}

fn python_dependency_ids(
    fact: &CallResolutionFact,
    required_evidence_ids: &BTreeSet<FileId>,
    stored_fact_ids: &BTreeSet<FileId>,
    context: &ProofResolutionValidationContext,
) -> Result<BTreeSet<FileId>, StorageError> {
    if !required_evidence_ids.is_subset(stored_fact_ids) {
        return Err(proof_error(
            "Python dependency receipt omits source or typed evidence files",
        ));
    }
    let mut expected = required_evidence_ids.clone();
    let imported = fact
        .evidence_chain
        .iter()
        .any(|evidence| matches!(evidence, ResolutionEvidence::StaticImportBinding { .. }));
    let source = context
        .file_by_id
        .get(&fact.callsite.file_id.0)
        .ok_or_else(|| proof_error("Python source dependency file is missing"))?;
    if source.language != "python"
        || source
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("py")
    {
        return Err(proof_error(
            "Python proof source is not an indexed runtime source file",
        ));
    }
    if imported {
        let module_specifier = fact
            .evidence_chain
            .iter()
            .find_map(|evidence| {
                let ResolutionEvidence::QualifiedPath { components } = evidence else {
                    return None;
                };
                components.iter().find_map(|component| {
                    context
                        .node_by_id
                        .get(component)
                        .filter(|node| node.kind == NodeKind::MODULE)
                        .map(|node| node.serialized_name.as_str())
                })
            })
            .ok_or_else(|| proof_error("Python relative import has no authenticated module"))?;
        let Some((depth, components)) = python_relative_module_components(module_specifier) else {
            return Err(proof_error(
                "Python relative import module is outside the exact subset",
            ));
        };
        let mut base = source
            .path
            .parent()
            .ok_or_else(|| proof_error("Python source path has no package directory"))?
            .to_path_buf();
        for _ in 0..depth {
            expected.insert(python_live_package_marker(context, &base)?);
            base = base.parent().map(Path::to_path_buf).ok_or_else(|| {
                proof_error("Python relative import escapes the classic package root")
            })?;
        }
        base = source
            .path
            .parent()
            .expect("checked Python source package directory")
            .to_path_buf();
        for _ in 1..depth {
            base = base.parent().map(Path::to_path_buf).ok_or_else(|| {
                proof_error("Python relative import escapes the classic package root")
            })?;
        }
        for component in &components[..components.len() - 1] {
            base.push(component);
            expected.insert(python_live_package_marker(context, &base)?);
        }
        let target = fact.target.expect("Exact Python fact has a target");
        let target_file_id = context
            .node_by_id
            .get(&target)
            .and_then(|node| node.file_node_id)
            .ok_or_else(|| proof_error("Python imported target has no source file"))?;
        let target_file = context
            .file_by_id
            .get(&target_file_id.0)
            .ok_or_else(|| proof_error("Python imported target file is missing"))?;
        let leaf = components.last().expect("relative module has a component");
        let target_ids = python_live_module_candidates(context, &base, leaf)?;
        if target_ids.as_slice() != [FileId(target_file.id)] {
            return Err(proof_error(
                "Python relative import has no unique indexed native module target",
            ));
        }
    }
    if expected != *stored_fact_ids {
        return Err(proof_error(
            "Python dependency receipt has missing or extra package dependencies",
        ));
    }
    for file_id in stored_fact_ids {
        count_store_replay_work(1);
        let file = context
            .file_by_id
            .get(&file_id.0)
            .ok_or_else(|| proof_error("Python dependency file is missing"))?;
        if file.language != "python"
            || file
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("py")
        {
            return Err(proof_error(
                "Python dependency receipt contains a non-runtime source file",
            ));
        }
        if let Some(error) = context.python_attestation_error_by_file.get(&file.id) {
            return Err(proof_error(format!(
                "Python dependency source is not publication-authenticated: {error}"
            )));
        }
    }
    Ok(expected)
}

fn prepare_python_file_ids_by_path(
    file_by_id: &HashMap<i64, FileInfo>,
) -> HashMap<PathBuf, Vec<FileId>> {
    let mut by_path = HashMap::<PathBuf, Vec<FileId>>::new();
    for file in file_by_id.values().filter(|file| file.language == "python") {
        count_store_replay_work(1);
        by_path
            .entry(file.path.clone())
            .or_default()
            .push(FileId(file.id));
    }
    by_path
}

fn python_live_package_marker(
    context: &ProofResolutionValidationContext,
    directory: &Path,
) -> Result<FileId, StorageError> {
    let directory_metadata = fs::symlink_metadata(directory)
        .map_err(|_| proof_error("Python package directory cannot be inspected"))?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        return Err(proof_error(
            "Python package directory is not a native in-project directory",
        ));
    }
    let marker = directory.join("__init__.py");
    let marker_metadata = fs::symlink_metadata(&marker)
        .map_err(|_| proof_error("Python relative import has no unique indexed package marker"))?;
    if marker_metadata.file_type().is_symlink() || !marker_metadata.is_file() {
        return Err(proof_error(
            "Python relative import package marker is not a regular source file",
        ));
    }
    let markers = context
        .python_file_ids_by_path
        .get(&marker)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let [marker] = markers else {
        return Err(proof_error(
            "Python relative import has no unique indexed package marker",
        ));
    };
    Ok(*marker)
}

fn python_live_module_candidates(
    context: &ProofResolutionValidationContext,
    base: &Path,
    leaf: &str,
) -> Result<Vec<FileId>, StorageError> {
    let file = base.join(leaf).with_extension("py");
    let package = base.join(leaf);
    let mut candidates = Vec::new();
    for path in [&file, &package.join("__init__.py")] {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(proof_error(
                    "Python relative import candidate is not a regular source file",
                ));
            }
            Ok(_) => {
                if path != &file {
                    let package_metadata = fs::symlink_metadata(&package).map_err(|_| {
                        proof_error("Python relative import package candidate cannot be inspected")
                    })?;
                    if package_metadata.file_type().is_symlink() || !package_metadata.is_dir() {
                        return Err(proof_error(
                            "Python relative import package candidate is not a native directory",
                        ));
                    }
                }
                let indexed = context
                    .python_file_ids_by_path
                    .get(path)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let [file_id] = indexed else {
                    return Err(proof_error(
                        "Python relative import candidate is present but not uniquely indexed",
                    ));
                };
                candidates.push(*file_id);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(proof_error(
                    "Python relative import candidate cannot be inspected",
                ));
            }
        }
    }
    candidates.sort();
    candidates.dedup();
    Ok(candidates)
}

fn python_relative_module_components(module: &str) -> Option<(usize, Vec<&str>)> {
    let depth = module.bytes().take_while(|byte| *byte == b'.').count();
    let components = module.get(depth..)?.split('.').collect::<Vec<_>>();
    (depth > 0
        && !components.is_empty()
        && components.iter().all(|component| {
            let mut chars = component.chars();
            chars
                .next()
                .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
                && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        }))
    .then_some((depth, components))
}

#[cfg(unix)]
type PythonNativeFileIdentity = (u64, u64);
#[cfg(not(unix))]
type PythonNativeFileIdentity = PathBuf;

fn python_native_file_identity(path: &Path) -> Result<PythonNativeFileIdentity, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("source cannot be inspected: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("source cannot be a symlink".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        fs::canonicalize(path)
            .map(|path| PathBuf::from(path.to_string_lossy().to_lowercase()))
            .map_err(|error| format!("source has no native identity: {error}"))
    }
}

fn canonical_file_node_id_for_path(path: &Path) -> i64 {
    #[cfg(windows)]
    let file_identity = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    #[cfg(not(windows))]
    let file_identity = path.to_string_lossy().into_owned();
    let canonical_id = format!("{file_identity}:{file_identity}:1");
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in canonical_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash as i64
}

fn prepare_python_source_attestation(
    file_by_id: &HashMap<i64, FileInfo>,
    file_content_hash_by_id: &HashMap<i64, String>,
    authenticate_live_sources: bool,
) -> HashMap<i64, String> {
    let mut errors = HashMap::new();
    let mut native_owners = HashMap::<PythonNativeFileIdentity, i64>::new();
    let python_file_ids_by_path = prepare_python_file_ids_by_path(file_by_id);
    let mut package_ancestry_cache = HashMap::<PathBuf, Result<(), String>>::new();
    for file in file_by_id.values().filter(|file| file.language == "python") {
        count_store_replay_work(1);
        if canonical_file_node_id_for_path(&file.path) != file.id {
            errors.insert(
                file.id,
                "file row path does not reproduce its canonical file node id".to_owned(),
            );
            continue;
        }
        if !authenticate_live_sources {
            continue;
        }
        let Some(directory) = file.path.parent() else {
            errors.insert(file.id, "source has no package directory".to_owned());
            continue;
        };
        if let Err(error) = attest_python_classic_package_ancestry(
            directory,
            &python_file_ids_by_path,
            &mut package_ancestry_cache,
        ) {
            errors.insert(file.id, error);
            continue;
        }
        let identity = match python_native_file_identity(&file.path) {
            Ok(identity) => identity,
            Err(error) => {
                errors.insert(file.id, error);
                continue;
            }
        };
        if let Some(previous) = native_owners.insert(identity, file.id)
            && previous != file.id
        {
            errors.insert(previous, "native source identity is not unique".to_owned());
            errors.insert(file.id, "native source identity is not unique".to_owned());
            continue;
        }
        let bytes = match fs::read(&file.path) {
            Ok(bytes) => bytes,
            Err(error) => {
                errors.insert(file.id, format!("source cannot be read: {error}"));
                continue;
            }
        };
        count_store_replay_work(bytes.len().saturating_add(1));
        let observed = format!("{:x}", Sha256::digest(&bytes));
        if file_content_hash_by_id.get(&file.id) != Some(&observed) {
            errors.insert(
                file.id,
                "source bytes do not match the stored source hash".to_owned(),
            );
            continue;
        }
        if std::str::from_utf8(&bytes).is_err() {
            errors.insert(file.id, "source bytes are not strict UTF-8".to_owned());
        }
    }
    errors
}

fn attest_python_classic_package_ancestry(
    source_directory: &Path,
    indexed_files: &HashMap<PathBuf, Vec<FileId>>,
    cache: &mut HashMap<PathBuf, Result<(), String>>,
) -> Result<(), String> {
    let mut directory = source_directory.to_path_buf();
    let mut traversed = Vec::new();
    let mut visited = HashSet::new();
    loop {
        count_store_replay_work(1);
        if !visited.insert(directory.clone()) {
            return Err("package ancestry is cyclic".to_owned());
        }
        if let Some(result) = cache.get(&directory) {
            let result = result.clone();
            for visited in traversed {
                cache.insert(visited, result.clone());
            }
            return result;
        }
        let marker = directory.join("__init__.py");
        let result = match fs::symlink_metadata(&marker) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("package marker cannot be inspected: {error}")),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                Err("package marker is not a regular source file".to_owned())
            }
            Ok(_) => {
                let directory_metadata = fs::symlink_metadata(&directory)
                    .map_err(|error| format!("package directory cannot be inspected: {error}"));
                match directory_metadata {
                    Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                        Err("package directory is not a native directory".to_owned())
                    }
                    Err(error) => Err(error),
                    Ok(_) => match indexed_files
                        .get(&marker)
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                    {
                        [_] => {
                            traversed.push(directory.clone());
                            match directory.parent() {
                                Some(parent) => {
                                    directory = parent.to_path_buf();
                                    continue;
                                }
                                None => Err("package ancestry has no parent".to_owned()),
                            }
                        }
                        _ => Err("package marker is not uniquely indexed".to_owned()),
                    },
                }
            }
        };
        for visited in traversed {
            cache.insert(visited, result.clone());
        }
        cache.insert(directory, result.clone());
        return result;
    }
}

fn validate_python_import_target(
    context: &ProofResolutionValidationContext,
    source_file: NodeId,
    module: NodeId,
    target: NodeId,
) -> bool {
    let Some(source) = context.file_by_id.get(&source_file.0) else {
        return false;
    };
    let Some(module_node) = context.node_by_id.get(&module) else {
        return false;
    };
    let Some(target_file) = context
        .node_by_id
        .get(&target)
        .and_then(|node| node.file_node_id)
        .and_then(|file| context.file_by_id.get(&file.0))
    else {
        return false;
    };
    let module_specifier = module_node.serialized_name.as_str();
    let Some((depth, components)) = python_relative_module_components(module_specifier) else {
        return false;
    };
    let Some(mut base) = source.path.parent().map(Path::to_path_buf) else {
        return false;
    };
    for _ in 1..depth {
        let Some(parent) = base.parent() else {
            return false;
        };
        base = parent.to_path_buf();
    }
    for component in &components[..components.len() - 1] {
        base.push(component);
    }
    let base = base.join(components.last().expect("relative module leaf"));
    target_file.path == base.with_extension("py") || target_file.path == base.join("__init__.py")
}

type PreparedGoPackageClosure = (
    HashMap<i64, GoPackageIdentity>,
    HashMap<GoPackageIdentity, BTreeSet<FileId>>,
    HashMap<i64, String>,
);

fn prepare_go_package_closure(
    file_by_id: &HashMap<i64, FileInfo>,
    file_content_hash_by_id: &HashMap<i64, String>,
) -> PreparedGoPackageClosure {
    let mut identity_by_file = HashMap::new();
    let mut dependencies_by_package = HashMap::<GoPackageIdentity, BTreeSet<FileId>>::new();
    let mut errors = HashMap::new();
    for file in file_by_id.values().filter(|file| file.language == "go") {
        count_store_replay_work(1);
        match attest_go_file(file, file_content_hash_by_id) {
            Ok(identity) => {
                if !file
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with("_test.go"))
                {
                    dependencies_by_package
                        .entry(identity.clone())
                        .or_default()
                        .insert(FileId(file.id));
                }
                identity_by_file.insert(file.id, identity);
            }
            Err(error) => {
                errors.insert(file.id, error);
            }
        }
    }
    (identity_by_file, dependencies_by_package, errors)
}

fn attest_go_file(
    file: &FileInfo,
    file_content_hash_by_id: &HashMap<i64, String>,
) -> Result<GoPackageIdentity, String> {
    let expected_hash = file_content_hash_by_id
        .get(&file.id)
        .ok_or_else(|| "stored source hash is missing".to_owned())?;
    let bytes =
        fs::read(&file.path).map_err(|error| format!("source bytes cannot be read: {error}"))?;
    count_store_replay_work(bytes.len().saturating_add(1));
    let observed_hash = format!("{:x}", Sha256::digest(&bytes));
    if &observed_hash != expected_hash {
        return Err("source bytes do not match the stored source hash".to_owned());
    }
    let source =
        std::str::from_utf8(&bytes).map_err(|_| "source bytes are not strict UTF-8".to_owned())?;
    let package_name = go_package_clause_from_source(source)
        .ok_or_else(|| "source has no exact package clause".to_owned())?;
    let parent = file
        .path
        .parent()
        .ok_or_else(|| "source path has no package directory".to_owned())?;
    let native_directory = fs::canonicalize(parent)
        .map_err(|error| format!("package directory has no native identity: {error}"))?;
    Ok(GoPackageIdentity {
        native_directory,
        package_name,
    })
}

fn go_package_clause_from_source(source: &str) -> Option<String> {
    let mut block_comment = false;
    for raw_line in source.lines() {
        let mut line = raw_line.trim();
        loop {
            if block_comment {
                if let Some((_, rest)) = line.split_once("*/") {
                    block_comment = false;
                    line = rest.trim_start();
                    continue;
                }
                break;
            }
            if line.is_empty() || line.starts_with("//") {
                break;
            }
            if let Some(rest) = line.strip_prefix("/*") {
                if let Some((_, trailing)) = rest.split_once("*/") {
                    line = trailing.trim_start();
                    continue;
                }
                block_comment = true;
                break;
            }
            let rest = line.strip_prefix("package")?;
            if rest
                .chars()
                .next()
                .is_none_or(|character| !character.is_whitespace())
            {
                return None;
            }
            let mut components = rest.split_whitespace();
            let name = components.next()?;
            let trailing = components.collect::<Vec<_>>().join(" ");
            if !go_package_identifier(name) || (!trailing.is_empty() && !trailing.starts_with("//"))
            {
                return None;
            }
            return Some(name.to_owned());
        }
    }
    None
}

fn go_package_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|character| character == '_' || character.is_alphabetic())
        && chars.all(|character| character == '_' || character.is_alphanumeric())
}

#[cfg(test)]
mod go_replay_complexity_tests {
    use super::*;

    fn measured_package_replay_work(file_count: usize, lookup_count: usize) -> usize {
        let temp = tempfile::tempdir().expect("Go replay tempdir");
        let mut file_by_id = HashMap::new();
        let mut file_content_hash_by_id = HashMap::new();
        let source = b"package proof\n";
        let source_hash = format!("{:x}", Sha256::digest(source));
        for index in 0..file_count {
            let id = i64::try_from(index + 1).expect("file id");
            let path = temp.path().join(format!("file_{index}.go"));
            fs::write(&path, source).expect("write Go replay source");
            file_by_id.insert(
                id,
                FileInfo {
                    id,
                    path,
                    language: "go".to_owned(),
                    modification_time: 0,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                },
            );
            file_content_hash_by_id.insert(id, source_hash.clone());
        }
        reset_store_replay_work();
        let (
            go_package_identity_by_file,
            go_dependency_ids_by_package,
            go_attestation_error_by_file,
        ) = prepare_go_package_closure(&file_by_id, &file_content_hash_by_id);
        let context = ProofResolutionValidationContext {
            file_by_id,
            file_content_hash_by_id,
            rust_file_ids_by_path: HashMap::new(),
            rust_root_count_by_directory: HashMap::new(),
            node_by_id: HashMap::new(),
            edges: Vec::new(),
            edge_index_by_id: HashMap::new(),
            ordinary_call_edge_indices: Vec::new(),
            parsed_callsite_identity_by_edge: HashMap::new(),
            import_relations: HashMap::new(),
            swift_module_import_relations: HashMap::new(),
            swift_public_node_ids: HashSet::new(),
            dart_import_visibility_by_node: HashMap::new(),
            dart_runtime_closed_nodes: HashSet::new(),
            dart_overridden_owner_methods: HashSet::new(),
            dart_ancestry_invalid_domains: HashSet::new(),
            typescript_directory_import_relations: HashMap::new(),
            member_relations: HashMap::new(),
            member_by_owner_and_name: HashMap::new(),
            python_import_paths: HashMap::new(),
            python_file_ids_by_path: HashMap::new(),
            python_attestation_error_by_file: HashMap::new(),
            java_package_identity_by_file: HashMap::new(),
            java_dependency_ids_by_package: HashMap::new(),
            csd_domain_identity_by_file: HashMap::new(),
            csd_dependency_ids_by_domain: HashMap::new(),
            ruby_dependency_file_ids: Vec::new(),
            php_namespace_identity_by_file: HashMap::new(),
            php_dependency_ids_by_namespace: HashMap::new(),
            php_namespace_domain_invalid: false,
            go_package_identity_by_file,
            go_dependency_ids_by_package,
            go_attestation_error_by_file,
            live_go_sources_authenticated: true,
        };
        for _ in 0..lookup_count {
            go_package_dependency_ids(FileId(1), &BTreeSet::from([FileId(1)]), &context)
                .expect("package closure");
        }
        store_replay_work()
    }

    #[test]
    fn package_file_preparation_and_fact_replay_are_independently_linear() {
        let baseline = measured_package_replay_work(32, 32);
        let more_files = measured_package_replay_work(64, 32);
        let more_facts = measured_package_replay_work(32, 64);
        let combined = measured_package_replay_work(64, 64);
        assert!(baseline > 0, "Go store replay work was not counted");
        assert!(
            more_files <= baseline * 2 + 64,
            "Go store file preparation grew superlinearly: {baseline} -> {more_files}"
        );
        assert!(
            more_facts <= baseline * 2 + 64,
            "Go store fact replay grew superlinearly: {baseline} -> {more_facts}"
        );
        assert!(
            combined <= baseline * 2 + 128,
            "combined Go store replay grew superlinearly: {baseline} -> {combined}"
        );
    }

    fn measured_repeated_callsite_index_work(count: usize) -> usize {
        let node_by_id = HashMap::from([(
            NodeId(4),
            Node {
                id: NodeId(4),
                kind: NodeKind::UNKNOWN,
                serialized_name: "target".to_owned(),
                ..Default::default()
            },
        )]);
        let edges = (0..count)
            .map(|index| Edge {
                id: EdgeId(i64::try_from(index + 1).expect("edge id")),
                source: NodeId(2),
                target: NodeId(4),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(2),
                resolved_target: Some(NodeId(3)),
                callsite_identity: Some(format!("1:2:{}:4", index + 1)),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        reset_store_replay_work();
        let (indices, identities) = prepare_call_edge_correlation_index(&edges, &node_by_id);
        assert_eq!(indices.len(), count);
        assert_eq!(identities.len(), count);
        store_replay_work()
    }

    #[test]
    fn repeated_same_line_correlation_identity_index_work_is_linear() {
        let small = measured_repeated_callsite_index_work(64);
        let large = measured_repeated_callsite_index_work(128);
        assert!(small >= 64, "callsite index work was not counted: {small}");
        assert!(
            large <= small * 2 + 64,
            "same-line callsite identity indexing grew superlinearly: {small} -> {large}"
        );
    }
}

#[cfg(test)]
mod python_replay_complexity_tests {
    use super::*;

    fn measured_python_replay_work(file_count: usize, fact_count: usize) -> usize {
        let temp = tempfile::tempdir().expect("Python replay tempdir");
        let mut file_by_id = HashMap::new();
        let mut file_content_hash_by_id = HashMap::new();
        let source = b"def target():\n    pass\n";
        let source_hash = format!("{:x}", Sha256::digest(source));
        let mut source_file_id = None;
        for index in 0..file_count {
            let path = temp.path().join(format!("file_{index}.py"));
            fs::write(&path, source).expect("write Python replay source");
            let id = canonical_file_node_id_for_path(&path);
            source_file_id.get_or_insert(id);
            file_by_id.insert(
                id,
                FileInfo {
                    id,
                    path,
                    language: "python".to_owned(),
                    modification_time: 0,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                },
            );
            file_content_hash_by_id.insert(id, source_hash.clone());
        }
        let source_file_id = source_file_id.expect("source file id");
        let fact = CallResolutionFact {
            fact_id: String::new(),
            edge_id: Some(EdgeId(1)),
            raw_edge_target: Some(NodeId(2)),
            raw_callsite_identity: Some("1:1:1:2".to_owned()),
            callsite: ExactCallsite {
                file_id: FileId(source_file_id),
                source_sha256: source_hash.clone(),
                start_byte: 0,
                end_byte_exclusive: 6,
                line: 1,
                column: 1,
                callee_form: CalleeForm::Identifier,
                raw_target: "target".to_owned(),
            },
            caller: NodeId(source_file_id),
            target: Some(NodeId(2)),
            status: ProofResolutionStatus::Exact,
            reason: ProofResolutionReason::ExactResolution,
            evidence_chain: vec![ResolutionEvidence::SameFileDeclaration {
                declaration: NodeId(2),
            }],
            lookup_domain_complete: true,
            provenance: ResolutionProvenance {
                producer: INTERNAL_RESOLUTION_PRODUCER.to_owned(),
                fact_schema_version: PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
                algorithm: EXACT_CALL_RESOLUTION_ALGORITHM.to_owned(),
                language_adapter: "python".to_owned(),
                language_adapter_version: "reference-v13".to_owned(),
                parser_fingerprint: source_hash.clone(),
                dependency_file_hashes: vec![DependencyFileHash {
                    file_id: FileId(source_file_id),
                    source_sha256: source_hash.clone(),
                }],
                evidence_sha256: String::new(),
            },
        };
        reset_store_replay_work();
        let python_file_ids_by_path = prepare_python_file_ids_by_path(&file_by_id);
        let python_attestation_error_by_file =
            prepare_python_source_attestation(&file_by_id, &file_content_hash_by_id, true);
        let context = ProofResolutionValidationContext {
            file_content_hash_by_id,
            file_by_id,
            rust_file_ids_by_path: HashMap::new(),
            rust_root_count_by_directory: HashMap::new(),
            node_by_id: HashMap::new(),
            edges: Vec::new(),
            edge_index_by_id: HashMap::new(),
            ordinary_call_edge_indices: Vec::new(),
            parsed_callsite_identity_by_edge: HashMap::new(),
            import_relations: HashMap::new(),
            swift_module_import_relations: HashMap::new(),
            swift_public_node_ids: HashSet::new(),
            dart_import_visibility_by_node: HashMap::new(),
            dart_runtime_closed_nodes: HashSet::new(),
            dart_overridden_owner_methods: HashSet::new(),
            dart_ancestry_invalid_domains: HashSet::new(),
            typescript_directory_import_relations: HashMap::new(),
            member_relations: HashMap::new(),
            member_by_owner_and_name: HashMap::new(),
            python_import_paths: HashMap::new(),
            python_file_ids_by_path,
            python_attestation_error_by_file,
            java_package_identity_by_file: HashMap::new(),
            java_dependency_ids_by_package: HashMap::new(),
            csd_domain_identity_by_file: HashMap::new(),
            csd_dependency_ids_by_domain: HashMap::new(),
            ruby_dependency_file_ids: Vec::new(),
            php_namespace_identity_by_file: HashMap::new(),
            php_dependency_ids_by_namespace: HashMap::new(),
            php_namespace_domain_invalid: false,
            go_package_identity_by_file: HashMap::new(),
            go_dependency_ids_by_package: HashMap::new(),
            go_attestation_error_by_file: HashMap::new(),
            live_go_sources_authenticated: true,
        };
        for _ in 0..fact_count {
            python_dependency_ids(
                &fact,
                &BTreeSet::from([FileId(source_file_id)]),
                &BTreeSet::from([FileId(source_file_id)]),
                &context,
            )
            .expect("Python dependency replay");
        }
        store_replay_work()
    }

    #[test]
    fn python_file_preparation_and_fact_replay_are_independently_linear() {
        let baseline = measured_python_replay_work(32, 32);
        let more_files = measured_python_replay_work(64, 32);
        let more_facts = measured_python_replay_work(32, 64);
        let combined = measured_python_replay_work(64, 64);
        assert!(baseline > 0, "Python store replay work was not counted");
        assert!(
            more_files <= baseline * 2 + 64,
            "Python store preparation grew superlinearly: {baseline} -> {more_files}"
        );
        assert!(
            more_facts <= baseline * 2 + 64,
            "Python fact replay grew superlinearly: {baseline} -> {more_facts}"
        );
        assert!(
            combined <= baseline * 2 + 128,
            "combined Python store replay grew superlinearly: {baseline} -> {combined}"
        );
    }
}

#[cfg(test)]
mod ruby_php_replay_complexity_tests {
    use super::*;

    fn measured_domain_replay_work(
        language: &str,
        file_count: usize,
        fact_count: usize,
        hostile_namespace: bool,
    ) -> usize {
        let temp = tempfile::tempdir().expect("Ruby/PHP replay tempdir");
        let mut files = Vec::new();
        let mut node_by_id = HashMap::new();
        for index in 0..file_count {
            let id = i64::try_from(index + 1).expect("file id");
            files.push(FileInfo {
                id,
                path: temp.path().join(format!(
                    "file_{index}.{}",
                    if language == "ruby" { "rb" } else { "php" }
                )),
                language: language.to_owned(),
                modification_time: 0,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: FileRole::Source,
            });
            if language == "php" {
                let namespace = Node {
                    id: NodeId(10_000 + id),
                    kind: NodeKind::NAMESPACE,
                    serialized_name: "App".to_owned(),
                    file_node_id: Some(NodeId(id)),
                    ..Default::default()
                };
                node_by_id.insert(namespace.id, namespace);
                if hostile_namespace && index == file_count - 1 {
                    let duplicate = Node {
                        id: NodeId(20_000 + id),
                        kind: NodeKind::NAMESPACE,
                        serialized_name: "Other".to_owned(),
                        file_node_id: Some(NodeId(id)),
                        ..Default::default()
                    };
                    node_by_id.insert(duplicate.id, duplicate);
                }
            }
        }
        reset_store_replay_work();
        let (
            ruby_dependency_file_ids,
            php_namespace_identity_by_file,
            php_dependency_ids_by_namespace,
            php_namespace_domain_invalid,
        ) = prepare_ruby_php_domain_closure(&files, &node_by_id);
        let file_by_id = files
            .into_iter()
            .map(|file| (file.id, file))
            .collect::<HashMap<_, _>>();
        let context = ProofResolutionValidationContext {
            file_by_id,
            file_content_hash_by_id: HashMap::new(),
            rust_file_ids_by_path: HashMap::new(),
            rust_root_count_by_directory: HashMap::new(),
            node_by_id,
            edges: Vec::new(),
            edge_index_by_id: HashMap::new(),
            ordinary_call_edge_indices: Vec::new(),
            parsed_callsite_identity_by_edge: HashMap::new(),
            import_relations: HashMap::new(),
            swift_module_import_relations: HashMap::new(),
            swift_public_node_ids: HashSet::new(),
            dart_import_visibility_by_node: HashMap::new(),
            dart_runtime_closed_nodes: HashSet::new(),
            dart_overridden_owner_methods: HashSet::new(),
            dart_ancestry_invalid_domains: HashSet::new(),
            typescript_directory_import_relations: HashMap::new(),
            member_relations: HashMap::new(),
            member_by_owner_and_name: HashMap::new(),
            python_import_paths: HashMap::new(),
            python_file_ids_by_path: HashMap::new(),
            python_attestation_error_by_file: HashMap::new(),
            java_package_identity_by_file: HashMap::new(),
            java_dependency_ids_by_package: HashMap::new(),
            csd_domain_identity_by_file: HashMap::new(),
            csd_dependency_ids_by_domain: HashMap::new(),
            ruby_dependency_file_ids,
            php_namespace_identity_by_file,
            php_dependency_ids_by_namespace,
            php_namespace_domain_invalid,
            go_package_identity_by_file: HashMap::new(),
            go_dependency_ids_by_package: HashMap::new(),
            go_attestation_error_by_file: HashMap::new(),
            live_go_sources_authenticated: true,
        };
        let fact = CallResolutionFact {
            fact_id: String::new(),
            edge_id: Some(EdgeId(1)),
            raw_edge_target: Some(NodeId(2)),
            raw_callsite_identity: Some("1:1:1:2".to_owned()),
            callsite: ExactCallsite {
                file_id: FileId(1),
                source_sha256: "0".repeat(64),
                start_byte: 1,
                end_byte_exclusive: 2,
                line: 1,
                column: 1,
                callee_form: CalleeForm::Identifier,
                raw_target: "target".to_owned(),
            },
            caller: NodeId(1),
            target: None,
            status: ProofResolutionStatus::Exact,
            reason: ProofResolutionReason::ExactResolution,
            evidence_chain: Vec::new(),
            lookup_domain_complete: true,
            provenance: ResolutionProvenance {
                producer: INTERNAL_RESOLUTION_PRODUCER.to_owned(),
                fact_schema_version: PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
                algorithm: EXACT_CALL_RESOLUTION_ALGORITHM.to_owned(),
                language_adapter: language.to_owned(),
                language_adapter_version: "reference-v2".to_owned(),
                parser_fingerprint: "0".repeat(64),
                dependency_file_hashes: Vec::new(),
                evidence_sha256: String::new(),
            },
        };
        for _ in 0..fact_count {
            let result = ruby_php_dependency_ids(&fact, &context);
            if hostile_namespace && language == "php" {
                assert!(result.is_err());
            } else {
                assert_eq!(result.expect("closed domain").len(), file_count);
            }
        }
        store_replay_work()
    }

    #[test]
    fn ruby_php_file_domain_and_fact_replay_work_is_independently_linear() {
        for language in ["ruby", "php"] {
            let baseline = measured_domain_replay_work(language, 32, 32, false);
            let more_files = measured_domain_replay_work(language, 64, 32, false);
            let more_facts = measured_domain_replay_work(language, 32, 64, false);
            let combined = measured_domain_replay_work(language, 64, 32, false);
            let hostile = measured_domain_replay_work(language, 64, 32, true);
            assert!(baseline > 0, "{language} replay work was not counted");
            assert!(
                more_files <= baseline * 2 + 128,
                "{language} file preparation grew superlinearly: {baseline} -> {more_files}"
            );
            assert!(
                more_facts <= baseline * 2 + 128,
                "{language} fact replay grew superlinearly: {baseline} -> {more_facts}"
            );
            assert!(
                combined <= baseline * 2 + 256,
                "{language} combined replay grew superlinearly: {baseline} -> {combined}"
            );
            assert!(
                hostile <= more_files + 128,
                "{language} hostile domain work was not linear: {more_files} -> {hostile}"
            );
        }
    }
}

#[derive(Default)]
struct ProofRelationState {
    admissible: usize,
    conflicting: usize,
}

impl ProofRelationState {
    fn is_unique(&self) -> bool {
        self.admissible == 1 && self.conflicting == 0
    }
}

#[derive(Default)]
struct TypescriptDirectoryImportState {
    marker: Option<&'static str>,
    marker_seen: bool,
    admissible: usize,
    conflicting: usize,
}

impl TypescriptDirectoryImportState {
    fn record(&mut self, marker: Option<&'static str>, admissible: bool) {
        self.marker_seen |= marker.is_some();
        if admissible {
            let marker = marker.expect("directory marker admission requires a marker");
            self.marker.get_or_insert(marker);
            self.admissible += 1;
        } else {
            self.conflicting += 1;
        }
    }

    fn unique_marker(&self) -> Option<&'static str> {
        (self.admissible == 1 && self.conflicting == 0)
            .then_some(self.marker)
            .flatten()
    }
}

fn typescript_directory_specifier(literal: &str) -> Option<&'static str> {
    match literal {
        "'.'" | "\".\"" => Some("."),
        "'..'" | "\"..\"" => Some(".."),
        _ => None,
    }
}

fn python_raw_import_marker_is_admissible(edge: &Edge, nodes: &HashMap<NodeId, Node>) -> bool {
    if edge.kind != EdgeKind::IMPORT
        || edge.effective_source() != edge.source
        || !edge.candidate_targets.is_empty()
    {
        return false;
    }
    let Some(file_id) = edge.file_node_id else {
        return false;
    };
    let raw_markers_are_local = [edge.source, edge.target].into_iter().all(|node_id| {
        nodes.get(&node_id).is_some_and(|node| {
            node.file_node_id == Some(file_id)
                && matches!(node.kind, NodeKind::UNKNOWN | NodeKind::MODULE)
        })
    });
    let target_relation_is_consistent = match edge.resolved_target {
        None => edge.effective_target() == edge.target,
        Some(resolved) => {
            edge.effective_target() == resolved
                && resolved != edge.source
                && resolved != edge.target
                && nodes.contains_key(&resolved)
        }
    };
    raw_markers_are_local && target_relation_is_consistent
}

impl ProofResolutionValidationContext {
    fn prepare(
        storage: &Storage,
        authenticate_live_go_sources: bool,
    ) -> Result<Self, StorageError> {
        let files = storage.get_files()?;
        let file_by_id = files.iter().cloned().map(|file| (file.id, file)).collect();
        let (rust_file_ids_by_path, rust_root_count_by_directory) =
            prepare_rust_file_identity(&file_by_id);
        let python_file_ids_by_path = prepare_python_file_ids_by_path(&file_by_id);
        let file_content_hash_by_id = storage.get_file_content_hashes()?;
        let python_attestation_error_by_file = prepare_python_source_attestation(
            &file_by_id,
            &file_content_hash_by_id,
            authenticate_live_go_sources,
        );
        let (
            go_package_identity_by_file,
            go_dependency_ids_by_package,
            go_attestation_error_by_file,
        ) = if authenticate_live_go_sources {
            prepare_go_package_closure(&file_by_id, &file_content_hash_by_id)
        } else {
            (HashMap::new(), HashMap::new(), HashMap::new())
        };
        let node_by_id: HashMap<NodeId, Node> = storage
            .get_nodes()?
            .into_iter()
            .map(|node| (node.id, node))
            .collect();
        let (java_package_identity_by_file, java_dependency_ids_by_package) =
            prepare_java_package_closure(&file_by_id, &node_by_id);
        let (csd_domain_identity_by_file, csd_dependency_ids_by_domain) =
            prepare_csd_domain_closure(&files, &node_by_id);
        let swift_public_node_ids = prepare_swift_public_nodes(storage, &files, &node_by_id)?;
        let mut dart_import_visibility_by_node = HashMap::new();
        for node in node_by_id.values().filter(|node| {
            node.file_node_id.is_some_and(|file| {
                file_by_id
                    .get(&file.0)
                    .is_some_and(|file| file.language == "dart")
            }) && node.kind == NodeKind::MODULE
                && quoted_import_literal(&node.serialized_name).is_some()
        }) {
            count_store_replay_work(1);
            dart_import_visibility_by_node.insert(
                node.id,
                DartImportVisibility {
                    shown: dart_import_combinator_names(&node.serialized_name, "show"),
                    hidden: dart_import_combinator_names(&node.serialized_name, "hide")
                        .unwrap_or_default(),
                },
            );
        }
        let (
            ruby_dependency_file_ids,
            php_namespace_identity_by_file,
            php_dependency_ids_by_namespace,
            php_namespace_domain_invalid,
        ) = prepare_ruby_php_domain_closure(&files, &node_by_id);
        let edges = storage.get_edges()?;
        let (ordinary_call_edge_indices, parsed_callsite_identity_by_edge) =
            prepare_call_edge_correlation_index(&edges, &node_by_id);
        let mut edge_index_by_id = HashMap::with_capacity(edges.len());
        let mut import_relations = HashMap::<_, ProofRelationState>::new();
        let mut swift_module_import_relations = HashMap::<_, ProofRelationState>::new();
        let mut typescript_directory_import_relations =
            HashMap::<_, TypescriptDirectoryImportState>::new();
        let mut member_relations = HashMap::<_, ProofRelationState>::new();
        let mut python_import_edges = HashMap::<NodeId, Vec<NodeId>>::new();
        for (index, edge) in edges.iter().enumerate() {
            edge_index_by_id.insert(edge.id, index);
            if edge.kind == EdgeKind::IMPORT
                && edge.source == edge.target
                && let Some(file_id) = edge.file_node_id
                && let Some(import) = node_by_id.get(&edge.source)
                && import.kind == NodeKind::MODULE
                && import.file_node_id == Some(file_id)
            {
                let state = swift_module_import_relations
                    .entry((file_id, edge.source))
                    .or_default();
                if edge.effective_source() == edge.source
                    && edge.effective_target() == edge.target
                    && edge.resolved_target.is_none()
                    && edge.candidate_targets.is_empty()
                {
                    state.admissible += 1;
                } else {
                    state.conflicting += 1;
                }
            }
            if edge.kind == EdgeKind::IMPORT
                && let (Some(file_id), Some(target)) = (edge.file_node_id, edge.resolved_target)
            {
                let admissible = edge.effective_source() == edge.source
                    && edge.effective_target() == target
                    && edge.candidate_targets.is_empty()
                    && node_by_id.get(&edge.source).is_some_and(|node| {
                        matches!(node.kind, NodeKind::MODULE | NodeKind::UNKNOWN)
                            && node.file_node_id == Some(file_id)
                    })
                    && node_by_id.get(&target).is_some_and(|node| {
                        matches!(
                            node.kind,
                            NodeKind::FUNCTION
                                | NodeKind::METHOD
                                | NodeKind::STRUCT
                                | NodeKind::CLASS
                                | NodeKind::ENUM
                        ) && node.file_node_id.is_some()
                    });
                let state = import_relations
                    .entry((file_id, edge.source, target))
                    .or_default();
                if admissible {
                    state.admissible += 1;
                } else {
                    state.conflicting += 1;
                }
            }
            if edge.kind == EdgeKind::IMPORT
                && let Some(import) = node_by_id.get(&edge.source)
                && import.kind == NodeKind::UNKNOWN
                && let Some(source_file) = import.file_node_id
            {
                let target = node_by_id.get(&edge.target);
                let marker = target
                    .filter(|target| target.kind == NodeKind::MODULE)
                    .and_then(|target| typescript_directory_specifier(&target.serialized_name));
                let admissible = marker.is_some()
                    && edge.file_node_id == Some(source_file)
                    && target.is_some_and(|target| target.file_node_id == Some(source_file))
                    && edge.effective_source() == edge.source
                    && edge.effective_target() == edge.target
                    && edge.resolved_target.is_none()
                    && edge.candidate_targets.is_empty();
                typescript_directory_import_relations
                    .entry((source_file, edge.source))
                    .or_default()
                    .record(marker, admissible);
            }
            if python_raw_import_marker_is_admissible(edge, &node_by_id) {
                python_import_edges
                    .entry(edge.source)
                    .or_default()
                    .push(edge.target);
            }
            if edge.kind == EdgeKind::MEMBER {
                let owner = node_by_id.get(&edge.source);
                let member = node_by_id.get(&edge.target);
                let admissible = edge.effective_source() == edge.source
                    && edge.effective_target() == edge.target
                    && edge.candidate_targets.is_empty()
                    && owner.is_some_and(|owner| {
                        matches!(
                            (owner.kind, member.map(|member| member.kind)),
                            (
                                NodeKind::MODULE,
                                Some(
                                    NodeKind::MODULE
                                        | NodeKind::FUNCTION
                                        | NodeKind::STRUCT
                                        | NodeKind::CLASS
                                        | NodeKind::ENUM
                                )
                            ) | (NodeKind::STRUCT | NodeKind::ENUM, Some(NodeKind::METHOD))
                                | (NodeKind::CLASS, Some(NodeKind::METHOD | NodeKind::FUNCTION))
                        )
                    })
                    && member.is_some_and(|member| {
                        member.file_node_id.is_some() && edge.file_node_id == member.file_node_id
                    });
                let state = member_relations
                    .entry((edge.source, edge.target))
                    .or_default();
                if admissible {
                    state.admissible += 1;
                } else {
                    state.conflicting += 1;
                }
            }
        }
        let python_import_targets = python_import_edges
            .values()
            .flatten()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut python_import_paths = HashMap::<_, Vec<Vec<NodeId>>>::new();
        for &source in python_import_edges
            .keys()
            .filter(|source| !python_import_targets.contains(source))
        {
            let Some(file_id) = node_by_id.get(&source).and_then(|node| node.file_node_id) else {
                continue;
            };
            let mut path = vec![source];
            let mut visited = BTreeSet::from([source]);
            let mut current = source;
            while let Some([next]) = python_import_edges.get(&current).map(Vec::as_slice) {
                if !visited.insert(*next) {
                    break;
                }
                path.push(*next);
                let Some(node) = node_by_id.get(next) else {
                    break;
                };
                if node.kind == NodeKind::MODULE {
                    python_import_paths
                        .entry((file_id, source))
                        .or_default()
                        .push(path);
                    break;
                }
                current = *next;
            }
        }
        let mut member_by_owner_and_name = HashMap::new();
        for (&(owner, member), state) in &member_relations {
            if !state.is_unique() {
                continue;
            }
            let Some(member_node) = node_by_id.get(&member) else {
                continue;
            };
            let key = (
                owner,
                graph_leaf_name(&member_node.serialized_name).to_string(),
            );
            member_by_owner_and_name
                .entry(key)
                .and_modify(|candidate| *candidate = None)
                .or_insert(Some(member));
        }
        let (
            dart_runtime_closed_nodes,
            dart_overridden_owner_methods,
            dart_ancestry_invalid_domains,
        ) = prepare_dart_dispatch_closure(
            &files,
            &file_content_hash_by_id,
            &node_by_id,
            &csd_domain_identity_by_file,
            &member_by_owner_and_name,
        );
        Ok(Self {
            file_by_id,
            file_content_hash_by_id,
            rust_file_ids_by_path,
            rust_root_count_by_directory,
            node_by_id,
            edges,
            edge_index_by_id,
            ordinary_call_edge_indices,
            parsed_callsite_identity_by_edge,
            import_relations,
            swift_module_import_relations,
            swift_public_node_ids,
            dart_import_visibility_by_node,
            dart_runtime_closed_nodes,
            dart_overridden_owner_methods,
            dart_ancestry_invalid_domains,
            typescript_directory_import_relations,
            member_relations,
            member_by_owner_and_name,
            python_import_paths,
            python_file_ids_by_path,
            python_attestation_error_by_file,
            java_package_identity_by_file,
            java_dependency_ids_by_package,
            csd_domain_identity_by_file,
            csd_dependency_ids_by_domain,
            ruby_dependency_file_ids,
            php_namespace_identity_by_file,
            php_dependency_ids_by_namespace,
            php_namespace_domain_invalid,
            go_package_identity_by_file,
            go_dependency_ids_by_package,
            go_attestation_error_by_file,
            live_go_sources_authenticated: authenticate_live_go_sources,
        })
    }

    fn has_import(&self, file: NodeId, import: NodeId, target: NodeId) -> bool {
        self.import_relations
            .get(&(file, import, target))
            .is_some_and(ProofRelationState::is_unique)
    }

    fn has_swift_module_import(&self, file: NodeId, import: NodeId) -> bool {
        self.swift_module_import_relations
            .get(&(file, import))
            .is_some_and(ProofRelationState::is_unique)
    }

    fn typescript_directory_import(&self, file: NodeId, import: NodeId) -> Option<&'static str> {
        self.typescript_directory_import_relations
            .get(&(file, import))
            .and_then(TypescriptDirectoryImportState::unique_marker)
    }

    fn typescript_directory_marker_seen(&self, file: NodeId, import: NodeId) -> bool {
        self.typescript_directory_import_relations
            .get(&(file, import))
            .is_some_and(|state| state.marker_seen)
    }

    fn has_member(&self, owner: NodeId, member: NodeId) -> bool {
        self.member_relations
            .get(&(owner, member))
            .is_some_and(ProofRelationState::is_unique)
    }

    fn has_cpp_member_definition(&self, owner: NodeId, member: NodeId) -> bool {
        if self.has_member(owner, member) {
            return true;
        }
        let (Some(owner_node), Some(member_node)) =
            (self.node_by_id.get(&owner), self.node_by_id.get(&member))
        else {
            return false;
        };
        if !matches!(owner_node.kind, NodeKind::CLASS | NodeKind::STRUCT)
            || member_node.kind != NodeKind::FUNCTION
            || owner_node.file_node_id.is_none()
            || owner_node.file_node_id != member_node.file_node_id
        {
            return false;
        }
        let member_name = graph_leaf_name(&member_node.serialized_name);
        let owner_name = owner_node
            .qualified_name
            .as_deref()
            .unwrap_or(&owner_node.serialized_name);
        let member_identity = member_node
            .qualified_name
            .as_deref()
            .unwrap_or(&member_node.serialized_name);
        if member_identity != format!("{owner_name}::{member_name}") {
            return false;
        }
        self.member_by_owner_and_name
            .get(&(owner, member_name.to_string()))
            .is_some_and(Option::is_some)
    }

    fn has_member_for_language(&self, language: &str, owner: NodeId, member: NodeId) -> bool {
        self.has_member(owner, member)
            || (language == "cpp" && self.has_cpp_member_definition(owner, member))
    }

    fn has_python_import_path(&self, file: NodeId, components: &[NodeId]) -> bool {
        let Some(import) = components.first() else {
            return false;
        };
        self.python_import_paths
            .get(&(file, *import))
            .is_some_and(|paths| {
                paths
                    .iter()
                    .filter(|path| path.as_slice() == components)
                    .count()
                    == 1
            })
    }
}

impl Storage {
    pub fn proof_resolution_fact_count(&self) -> Result<u64, StorageError> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM proof_resolution_fact", [], |row| {
                    row.get(0)
                })?;
        Ok(count.max(0) as u64)
    }

    pub fn get_proof_resolution_publication(
        &self,
    ) -> Result<Option<ProofResolutionPublication>, StorageError> {
        self.conn
            .query_row(
                "SELECT core_generation_id, core_run_id, fact_schema_version,
                        adapter_roster_json, complete, fact_count, fact_digest,
                        funnel_json, published_at_epoch_ms
                 FROM proof_resolution_publication WHERE id = 1",
                [],
                |row| {
                    let adapter_roster_json: String = row.get(3)?;
                    let funnel_json: String = row.get(7)?;
                    let adapter_roster = parse_canonical_json(
                        &adapter_roster_json,
                        "adapter roster",
                    )
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            adapter_roster_json.len(),
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
                        )
                    })?;
                    let funnel = parse_canonical_json(&funnel_json, "funnel").map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            funnel_json.len(),
                            rusqlite::types::Type::Text,
                            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
                        )
                    })?;
                    Ok(ProofResolutionPublication {
                        core_generation_id: row.get(0)?,
                        core_run_id: row.get(1)?,
                        fact_schema_version: row.get::<_, i64>(2)?.max(0) as u32,
                        adapter_roster,
                        complete: row.get::<_, i64>(4)? == 1,
                        fact_count: row.get::<_, i64>(5)?.max(0) as u64,
                        fact_digest: row.get(6)?,
                        funnel,
                        published_at_epoch_ms: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub fn get_exact_proof_resolution_fact_by_edge(
        &self,
        edge_id: EdgeId,
    ) -> Result<Option<CallResolutionFact>, StorageError> {
        self.read_proof_resolution_facts(Some(edge_id))
            .map(
                |mut facts| {
                    if facts.len() == 1 { facts.pop() } else { None }
                },
            )
    }

    pub fn get_proof_resolution_facts(&self) -> Result<Vec<CallResolutionFact>, StorageError> {
        self.read_proof_resolution_facts(None)
    }

    fn read_proof_resolution_facts(
        &self,
        edge_id: Option<EdgeId>,
    ) -> Result<Vec<CallResolutionFact>, StorageError> {
        let mut sql = "SELECT fact_id, edge_id, raw_edge_target_id, raw_callsite_identity,
                              file_id, source_sha256, start_byte,
                              end_byte_exclusive, line, column, callee_form, raw_target,
                              caller_node_id, target_node_id, status, reason, evidence_json,
                              dependency_json, lookup_domain_complete, producer,
                              fact_schema_version, algorithm, language_adapter,
                              language_adapter_version, parser_fingerprint, evidence_digest
                       FROM proof_resolution_fact"
            .to_owned();
        if edge_id.is_some() {
            sql.push_str(" WHERE edge_id = ?1 AND status = 'exact'");
        }
        sql.push_str(" ORDER BY rowid");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = match edge_id {
            Some(edge_id) => stmt.query(params![edge_id.0])?,
            None => stmt.query([])?,
        };
        let mut facts = Vec::new();
        while let Some(row) = rows.next()? {
            let callee_form_text: String = row.get(10)?;
            let status_text: String = row.get(14)?;
            let reason_text: String = row.get(15)?;
            let evidence_json: String = row.get(16)?;
            let dependency_json: String = row.get(17)?;
            let callee_form = CalleeForm::from_label(&callee_form_text)
                .ok_or_else(|| proof_error("stored callee form is outside the closed domain"))?;
            let status = ProofResolutionStatus::from_label(&status_text)
                .ok_or_else(|| proof_error("stored status is outside the closed domain"))?;
            let reason = ProofResolutionReason::from_label(&reason_text)
                .ok_or_else(|| proof_error("stored reason is outside the closed domain"))?;
            let evidence_chain: Vec<ResolutionEvidence> =
                parse_canonical_json(&evidence_json, "evidence").map_err(proof_error)?;
            let dependency_file_hashes: Vec<DependencyFileHash> =
                parse_canonical_json(&dependency_json, "dependency").map_err(proof_error)?;
            facts.push(CallResolutionFact {
                fact_id: row.get(0)?,
                edge_id: row.get::<_, Option<i64>>(1)?.map(EdgeId),
                raw_edge_target: row.get::<_, Option<i64>>(2)?.map(NodeId),
                raw_callsite_identity: row.get(3)?,
                callsite: ExactCallsite {
                    file_id: FileId(row.get(4)?),
                    source_sha256: row.get(5)?,
                    start_byte: row
                        .get::<_, i64>(6)?
                        .try_into()
                        .map_err(|_| proof_error("stored callsite start byte is negative"))?,
                    end_byte_exclusive: row
                        .get::<_, i64>(7)?
                        .try_into()
                        .map_err(|_| proof_error("stored callsite end byte is negative"))?,
                    line: row
                        .get::<_, i64>(8)?
                        .try_into()
                        .map_err(|_| proof_error("stored callsite line is outside u32"))?,
                    column: row
                        .get::<_, i64>(9)?
                        .try_into()
                        .map_err(|_| proof_error("stored callsite column is outside u32"))?,
                    callee_form,
                    raw_target: row.get(11)?,
                },
                caller: NodeId(row.get(12)?),
                target: row.get::<_, Option<i64>>(13)?.map(NodeId),
                status,
                reason,
                evidence_chain,
                lookup_domain_complete: row.get::<_, i64>(18)? == 1,
                provenance: ResolutionProvenance {
                    producer: row.get(19)?,
                    fact_schema_version: row.get::<_, i64>(20)?.max(0) as u32,
                    algorithm: row.get(21)?,
                    language_adapter: row.get(22)?,
                    language_adapter_version: row.get(23)?,
                    parser_fingerprint: row.get(24)?,
                    dependency_file_hashes,
                    evidence_sha256: row.get(25)?,
                },
            });
        }
        Ok(facts)
    }

    fn validate_facts_against_graph(
        &self,
        facts: &[CallResolutionFact],
        authenticate_live_go_sources: bool,
    ) -> Result<(), StorageError> {
        for fact in facts {
            validate_fact_seal(fact)?;
        }
        let context =
            ProofResolutionValidationContext::prepare(self, authenticate_live_go_sources)?;
        let exact_fact_indices = facts
            .iter()
            .enumerate()
            .filter_map(|(index, fact)| {
                (fact.status == ProofResolutionStatus::Exact).then_some(index)
            })
            .collect::<Vec<_>>();
        let syntax_inputs = exact_fact_indices
            .iter()
            .map(|index| {
                let fact = &facts[*index];
                ExactSyntaxCallsiteCorrelationInput {
                    file_id: fact.callsite.file_id,
                    line: fact.callsite.line,
                    start_byte: fact.callsite.start_byte,
                    end_byte_exclusive: fact.callsite.end_byte_exclusive,
                    column: fact.callsite.column,
                    caller: fact.caller,
                    target: fact.target.expect("Exact shape requires a target"),
                    raw_target: &fact.callsite.raw_target,
                }
            })
            .collect::<Vec<_>>();
        let constructor_evidence_nodes = facts
            .iter()
            .filter_map(
                |fact| match (fact.callsite.callee_form, fact.evidence_chain.as_slice()) {
                    (
                        CalleeForm::ExplicitReceiver,
                        [
                            ResolutionEvidence::ConstructorBinding { constructor },
                            ResolutionEvidence::ExplicitReceiverType { receiver_type },
                            ResolutionEvidence::SameFileDeclaration { .. },
                        ],
                    ) if constructor == receiver_type => Some(*constructor),
                    (
                        CalleeForm::ExplicitReceiver,
                        [
                            ResolutionEvidence::StaticImportBinding {
                                declaration: owner, ..
                            },
                            ResolutionEvidence::ConstructorBinding { constructor },
                            ResolutionEvidence::ExplicitReceiverType { receiver_type },
                            ResolutionEvidence::SameFileDeclaration { .. },
                        ],
                    ) if owner == constructor && constructor == receiver_type => Some(*constructor),
                    (
                        CalleeForm::ExplicitReceiver,
                        [
                            ResolutionEvidence::StaticImportBinding {
                                declaration: owner, ..
                            },
                            ResolutionEvidence::ConstructorBinding { constructor },
                            ResolutionEvidence::ExplicitReceiverType { receiver_type },
                            ResolutionEvidence::QualifiedPath { .. },
                        ],
                    ) if owner == constructor && constructor == receiver_type => Some(*constructor),
                    _ => None,
                },
            )
            .collect::<BTreeSet<_>>();
        let ordinary_edge_indices = context
            .ordinary_call_edge_indices
            .iter()
            .copied()
            .filter(|index| {
                !constructor_evidence_nodes.contains(&context.edges[*index].effective_target())
            })
            .collect::<Vec<_>>();
        let edge_inputs = ordinary_edge_indices
            .iter()
            .map(|index| {
                let edge = &context.edges[*index];
                let raw = &context.node_by_id[&edge.target];
                let direct_member_edge = edge.target == edge.effective_target()
                    && (matches!(raw.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                        || raw.file_node_id != edge.file_node_id);
                OrdinaryCallEdgeCorrelationInput {
                    file_id: edge.file_node_id.map(|file| FileId(file.0)),
                    line: edge.line,
                    caller: edge.effective_source(),
                    target: edge.effective_target(),
                    raw_edge_target: edge.target,
                    raw_file_id: if direct_member_edge {
                        edge.file_node_id.map(|file| FileId(file.0))
                    } else {
                        raw.file_node_id.map(|file| FileId(file.0))
                    },
                    raw_line: if direct_member_edge {
                        edge.line
                    } else {
                        raw.start_line
                    },
                    raw_target: graph_leaf_name(&raw.serialized_name),
                    callsite_identity: edge.callsite_identity.as_deref(),
                    semantic_exact: edge.resolved_target == Some(edge.effective_target())
                        && edge.candidate_targets.is_empty()
                        && context
                            .parsed_callsite_identity_by_edge
                            .get(&edge.id)
                            .is_some_and(|identity| {
                                Some(identity.file_id)
                                    == edge.file_node_id.map(|file| FileId(file.0))
                                    && Some(identity.line) == edge.line
                                    && identity.raw_target == edge.target
                            }),
                }
            })
            .collect::<Vec<_>>();
        let correlations = correlate_exact_syntax_callsites(&syntax_inputs, &edge_inputs)
            .into_iter()
            .map(|result| {
                result.map(|edge_index| context.edges[ordinary_edge_indices[edge_index]].id)
            })
            .collect::<Vec<_>>();
        let mut fact_correlations = vec![None; facts.len()];
        for (correlation_index, fact_index) in exact_fact_indices.iter().copied().enumerate() {
            fact_correlations[fact_index] = Some(correlations[correlation_index]);
        }
        for (fact_index, fact) in facts.iter().enumerate() {
            Self::validate_fact_against_graph(fact, &context, fact_correlations[fact_index])?;
        }
        Ok(())
    }

    fn validate_fact_against_graph(
        fact: &CallResolutionFact,
        context: &ProofResolutionValidationContext,
        correlation: Option<Result<EdgeId, ExactCallsiteCorrelationFailure>>,
    ) -> Result<(), StorageError> {
        let stored_source_hash = context
            .file_content_hash_by_id
            .get(&fact.callsite.file_id.0)
            .ok_or_else(|| proof_error("callsite file has no publication-bound source hash"))?;
        if stored_source_hash != &fact.callsite.source_sha256 {
            return Err(proof_error(
                "callsite source hash does not match the graph file",
            ));
        }
        let caller = context
            .node_by_id
            .get(&fact.caller)
            .ok_or_else(|| proof_error("caller node is missing"))?;
        if caller.file_node_id != Some(NodeId(fact.callsite.file_id.0))
            || caller
                .start_line
                .is_some_and(|line| line > fact.callsite.line)
            || caller
                .end_line
                .is_some_and(|line| line < fact.callsite.line)
        {
            return Err(proof_error(
                "caller containment does not match the exact callsite",
            ));
        }

        let csd_language = matches!(
            fact.provenance.language_adapter.as_str(),
            "csharp" | "swift" | "dart"
        );
        let linear_language =
            matches!(fact.provenance.language_adapter.as_str(), "ruby" | "php") || csd_language;
        let mut required_dependency_ids = if linear_language {
            BTreeSet::new()
        } else {
            BTreeSet::from([fact.callsite.file_id])
        };
        let mut evidence_node_ids = fact
            .evidence_chain
            .iter()
            .flat_map(ResolutionEvidence::node_ids)
            .collect::<Vec<_>>();
        if let Some(target) = fact.target {
            evidence_node_ids.push(target);
        }
        if linear_language {
            let mut members = HashSet::new();
            evidence_node_ids.retain(|node_id| {
                count_store_replay_work(1);
                members.insert(*node_id)
            });
        } else {
            evidence_node_ids.sort_unstable();
            evidence_node_ids.dedup();
        }
        for node_id in evidence_node_ids {
            let node = context
                .node_by_id
                .get(&node_id)
                .ok_or_else(|| proof_error("typed evidence references a missing graph node"))?;
            if !linear_language && let Some(file_id) = node.file_node_id {
                required_dependency_ids.insert(FileId(file_id.0));
            }
        }
        if fact.status == ProofResolutionStatus::Exact && fact.provenance.language_adapter == "go" {
            if fact
                .evidence_chain
                .iter()
                .any(|evidence| matches!(evidence, ResolutionEvidence::StaticImportBinding { .. }))
            {
                return Err(proof_error(
                    "Go exact import evidence has no authenticated module-domain receipt",
                ));
            }
            required_dependency_ids = if context.live_go_sources_authenticated {
                go_package_dependency_ids(fact.callsite.file_id, &required_dependency_ids, context)?
            } else {
                stored_go_package_dependency_ids(
                    fact.callsite.file_id,
                    &required_dependency_ids,
                    &dependency_file_ids(fact),
                    context,
                )?
            };
        }
        if linear_language && !csd_language {
            let expected = if fact.status == ProofResolutionStatus::Exact {
                ruby_php_dependency_ids(fact, context)?
            } else {
                vec![fact.callsite.file_id]
            };
            let observed = fact
                .provenance
                .dependency_file_hashes
                .iter()
                .map(|dependency| dependency.file_id)
                .collect::<Vec<_>>();
            count_store_replay_work(observed.len().saturating_add(expected.len()));
            if observed != expected {
                return Err(proof_error(format!(
                    "dependency hashes do not exactly cover the Ruby/PHP governed domain: observed={observed:?} required={expected:?}"
                )));
            }
        }
        if csd_language {
            let expected = if fact.status == ProofResolutionStatus::Exact {
                csd_dependency_ids(fact, context)?
            } else {
                vec![fact.callsite.file_id]
            };
            let observed = fact
                .provenance
                .dependency_file_hashes
                .iter()
                .map(|dependency| dependency.file_id)
                .collect::<Vec<_>>();
            count_store_replay_work(observed.len().saturating_add(expected.len()));
            if observed != expected {
                return Err(proof_error(format!(
                    "dependency hashes do not exactly cover the C#/Swift/Dart governed domain: observed={observed:?} required={expected:?}"
                )));
            }
        }
        let observed_dependency_ids = if !linear_language {
            dependency_file_ids(fact)
        } else {
            BTreeSet::new()
        };
        if fact.status == ProofResolutionStatus::Exact
            && let Some(java_dependencies) =
                java_same_package_dependency_ids(fact, &required_dependency_ids, context)?
        {
            required_dependency_ids = java_dependencies;
        }
        if fact.status == ProofResolutionStatus::Exact
            && fact.provenance.language_adapter == "python"
        {
            required_dependency_ids = python_dependency_ids(
                fact,
                &required_dependency_ids,
                &observed_dependency_ids,
                context,
            )?;
        }
        if !linear_language
            && let Some(rust_dependencies) = rust_same_file_dependency_ids(
                fact,
                &required_dependency_ids,
                &observed_dependency_ids,
                context,
            )
        {
            required_dependency_ids = rust_dependencies;
        }
        if !linear_language && observed_dependency_ids != required_dependency_ids {
            return Err(proof_error(format!(
                "dependency hashes do not exactly cover source, import, package, and target files for {}: observed={observed_dependency_ids:?} required={required_dependency_ids:?}",
                fact.provenance.language_adapter
            )));
        }
        for dependency in &fact.provenance.dependency_file_hashes {
            let dependency_file = context
                .file_by_id
                .get(&dependency.file_id.0)
                .ok_or_else(|| proof_error("dependency file record is missing"))?;
            if !dependency_file.indexed
                || (fact.status == ProofResolutionStatus::Exact && !dependency_file.complete)
            {
                return Err(proof_error(
                    "dependency file is not indexed-complete in the graph",
                ));
            }
            let stored = context
                .file_content_hash_by_id
                .get(&dependency.file_id.0)
                .ok_or_else(|| {
                    proof_error("dependency file has no publication-bound source hash")
                })?;
            if stored != &dependency.source_sha256 {
                return Err(proof_error("dependency file hash does not match the graph"));
            }
        }

        if fact.status != ProofResolutionStatus::Exact {
            if !fact.evidence_chain.is_empty() {
                return Err(proof_error(
                    "non-Exact fact cannot carry authoritative evidence",
                ));
            }
            return Ok(());
        }
        let edge_id = fact.edge_id.expect("shape validation requires exact edge");
        let target = fact.target.expect("shape validation requires exact target");
        let raw_edge_target = fact
            .raw_edge_target
            .expect("shape validation requires raw edge target");
        let raw_callsite_identity = fact
            .raw_callsite_identity
            .as_deref()
            .expect("shape validation requires raw callsite identity");
        let target_node = context
            .node_by_id
            .get(&target)
            .ok_or_else(|| proof_error("exact target node is missing"))?;
        if target_node.file_node_id.is_none() {
            return Err(proof_error("exact target has no indexed dependency file"));
        }
        if fact.provenance.language_adapter == "dart" {
            let direct_construction = fact
                .evidence_chain
                .iter()
                .any(|evidence| matches!(evidence, ResolutionEvidence::ConstructorBinding { .. }));
            let receiver_owner = fact
                .evidence_chain
                .iter()
                .find_map(|evidence| match evidence {
                    ResolutionEvidence::ExplicitReceiverType { receiver_type } => {
                        Some(*receiver_type)
                    }
                    ResolutionEvidence::ImplicitReceiver { owner } => Some(*owner),
                    _ => None,
                });
            if let Some(owner) = receiver_owner {
                let ancestry_invalid = context
                    .node_by_id
                    .get(&owner)
                    .and_then(|node| node.file_node_id)
                    .and_then(|file| context.csd_domain_identity_by_file.get(&file.0))
                    .is_none_or(|domain| context.dart_ancestry_invalid_domains.contains(domain));
                if ancestry_invalid
                    || (!direct_construction
                        && (!context.dart_runtime_closed_nodes.contains(&owner)
                            || context
                                .dart_overridden_owner_methods
                                .contains(&(owner, fact.callsite.raw_target.clone()))))
                {
                    return Err(proof_error(
                        "Dart receiver dispatch is not closed by its complete library domain",
                    ));
                }
            }
        }
        if fact.provenance.language_adapter == "go"
            && (!matches!(caller.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                || !matches!(target_node.kind, NodeKind::FUNCTION | NodeKind::METHOD))
        {
            return Err(proof_error(
                "Go exact caller and target must be source-level callables",
            ));
        }
        if fact.provenance.language_adapter == "go"
            && fact.callsite.callee_form == CalleeForm::Identifier
            && target_node.kind != NodeKind::FUNCTION
        {
            return Err(proof_error(
                "Go identifier evidence target is not a FUNCTION graph node",
            ));
        }
        for evidence in &fact.evidence_chain {
            let ResolutionEvidence::StaticImportBinding { import, .. } = evidence else {
                continue;
            };
            let import_node = context
                .node_by_id
                .get(import)
                .ok_or_else(|| proof_error("static import binding is missing"))?;
            if !proof_import_node_kind_is_literal(
                &fact.provenance.language_adapter,
                import_node.kind,
            ) {
                return Err(proof_error(
                    "StaticImportBinding node kind is not literal for the language adapter",
                ));
            }
        }
        let raw_placeholder = context
            .node_by_id
            .get(&raw_edge_target)
            .ok_or_else(|| proof_error("raw CALL placeholder node is missing"))?;
        let direct_callable_target = (matches!(
            fact.callsite.callee_form,
            CalleeForm::ImplicitReceiver | CalleeForm::ExplicitReceiver | CalleeForm::NamedImport
        ) || matches!(
            fact.provenance.language_adapter.as_str(),
            "ruby" | "php" | "csharp" | "swift" | "dart"
        )) && raw_edge_target == target
            && matches!(raw_placeholder.kind, NodeKind::FUNCTION | NodeKind::METHOD);
        if !direct_callable_target
            && (raw_placeholder.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                || raw_placeholder.start_line != Some(fact.callsite.line))
            || graph_leaf_name(&raw_placeholder.serialized_name) != fact.callsite.raw_target
        {
            return Err(proof_error(
                "raw CALL callsite placeholder does not match file, line, and target spelling",
            ));
        }
        match correlation.expect("Exact fact has a correlation result") {
            Ok(edge_id) if Some(edge_id) == fact.edge_id => {}
            Ok(_) => {
                return Err(proof_error(
                    "Exact fact binds the wrong ordinary edge for its canonical callsite",
                ));
            }
            Err(_) => {
                return Err(proof_error(
                    "matching ordinary CALL edge canonical callsite identity does not form one complete mapping",
                ));
            }
        }
        if matches!(
            fact.callsite.callee_form,
            CalleeForm::ImplicitReceiver | CalleeForm::ExplicitReceiver
        ) && if fact.provenance.language_adapter == "python" {
            target_node.kind != NodeKind::FUNCTION
        } else if fact.provenance.language_adapter == "cpp" {
            !matches!(target_node.kind, NodeKind::METHOD | NodeKind::FUNCTION)
        } else {
            target_node.kind != NodeKind::METHOD
        } {
            return Err(proof_error(
                "receiver evidence target is not a METHOD graph node",
            ));
        }
        match (fact.callsite.callee_form, fact.evidence_chain.as_slice()) {
            (CalleeForm::Identifier, [ResolutionEvidence::SameFileDeclaration { declaration }])
                if *declaration == target =>
            {
                if target_node.file_node_id != Some(NodeId(fact.callsite.file_id.0)) {
                    return Err(proof_error(
                        "SameFileDeclaration is not the exact target in the source file",
                    ));
                }
            }
            (
                CalleeForm::Identifier,
                [ResolutionEvidence::SamePackageDeclaration { declaration }],
            ) if matches!(fact.provenance.language_adapter.as_str(), "go" | "kotlin")
                && *declaration == target =>
            {
                if target_node.file_node_id.is_none()
                    || target_node.file_node_id == Some(NodeId(fact.callsite.file_id.0))
                {
                    return Err(proof_error(
                        "SamePackageDeclaration is not an exact cross-file package target",
                    ));
                }
            }
            (
                CalleeForm::NamedImport,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration,
                    },
                ],
            ) if *declaration == target => {
                let import_node = context
                    .node_by_id
                    .get(import)
                    .ok_or_else(|| proof_error("static import binding is missing"))?;
                if import_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                    || (!matches!(
                        fact.provenance.language_adapter.as_str(),
                        "php" | "dart" | "swift"
                    ) && graph_leaf_name(&import_node.serialized_name)
                        != fact.callsite.raw_target)
                    || !(if context
                        .typescript_directory_marker_seen(NodeId(fact.callsite.file_id.0), *import)
                    {
                        typescript_directory_import_target_is_authenticated(
                            context, fact, *import, target,
                        )
                    } else if matches!(fact.provenance.language_adapter.as_str(), "java" | "kotlin")
                    {
                        target_node
                            .qualified_name
                            .as_deref()
                            .is_some_and(|name| name.ends_with(&fact.callsite.raw_target))
                    } else if fact.provenance.language_adapter == "dart" {
                        target_node.file_node_id.is_some_and(|target_file| {
                            dart_literal_import_target_is_authenticated(
                                context,
                                fact,
                                *import,
                                target_file,
                            )
                        })
                    } else if fact.provenance.language_adapter == "swift" {
                        context.has_swift_module_import(NodeId(fact.callsite.file_id.0), *import)
                            && context.swift_public_node_ids.contains(&target)
                            && target_node.file_node_id.is_some_and(|target_file| {
                                context
                                    .file_by_id
                                    .get(&target_file.0)
                                    .and_then(|file| swift_project_module(&file.path))
                                    == Some(import_node.serialized_name.as_str())
                            })
                    } else {
                        context.has_import(NodeId(fact.callsite.file_id.0), *import, target)
                    })
                {
                    return Err(proof_error(
                        "StaticImportBinding is not the unique source import bound to target",
                    ));
                }
            }
            (
                CalleeForm::NamedImport,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration,
                    },
                    ResolutionEvidence::QualifiedPath { components },
                ],
            ) if *declaration == target
                && components.last() == Some(&target)
                && components.len() >= 2 =>
            {
                let source_file = NodeId(fact.callsite.file_id.0);
                let import_node = context
                    .node_by_id
                    .get(import)
                    .ok_or_else(|| proof_error("static import binding is missing"))?;
                let python_module_index = (fact.provenance.language_adapter == "python")
                    .then(|| {
                        components.iter().rposition(|component| {
                            context
                                .node_by_id
                                .get(component)
                                .is_some_and(|node| node.kind == NodeKind::MODULE)
                        })
                    })
                    .flatten();
                let python_relative = python_module_index.is_some_and(|module_index| {
                    import_node.kind == NodeKind::UNKNOWN
                        && components.first() == Some(import)
                        && components.last() == Some(&target)
                        && import_node.file_node_id == Some(source_file)
                        && graph_leaf_name(&import_node.serialized_name) == fact.callsite.raw_target
                        && context.has_python_import_path(source_file, &components[..=module_index])
                        && validate_python_import_target(
                            context,
                            source_file,
                            components[module_index],
                            target,
                        )
                });
                if !python_relative
                    && (import_node.kind != NodeKind::MODULE
                        || import_node.file_node_id != Some(source_file)
                        || graph_leaf_name(&import_node.serialized_name)
                            != fact.callsite.raw_target
                        || !context.has_import(source_file, *import, target)
                        || components
                            .windows(2)
                            .any(|pair| !context.has_member(pair[0], pair[1])))
                {
                    return Err(proof_error(
                        "StaticImportBinding path is not one unique source IMPORT and MEMBER chain",
                    ));
                }
            }
            (
                CalleeForm::ImplicitReceiver,
                [
                    ResolutionEvidence::ImplicitReceiver { owner },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) if *declaration == target => {
                let owner_node = context
                    .node_by_id
                    .get(owner)
                    .ok_or_else(|| proof_error("implicit receiver owner is missing"))?;
                let go_owner = fact.provenance.language_adapter == "go";
                if !matches!(
                    owner_node.kind,
                    NodeKind::STRUCT | NodeKind::CLASS | NodeKind::ENUM
                ) || if go_owner {
                    owner_node.file_node_id.is_none()
                        || target_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                } else {
                    owner_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                        || target_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                } || !context.has_member_for_language(
                    &fact.provenance.language_adapter,
                    *owner,
                    fact.caller,
                ) || !context.has_member_for_language(
                    &fact.provenance.language_adapter,
                    *owner,
                    target,
                ) {
                    return Err(proof_error(
                        "ImplicitReceiver does not own caller and target through inherent membership",
                    ));
                }
            }
            (
                CalleeForm::ImplicitReceiver,
                [
                    ResolutionEvidence::ImplicitReceiver { owner },
                    ResolutionEvidence::SamePackageDeclaration { declaration },
                ],
            ) if fact.provenance.language_adapter == "go" && *declaration == target => {
                let owner_node = context
                    .node_by_id
                    .get(owner)
                    .ok_or_else(|| proof_error("Go implicit receiver owner is missing"))?;
                if owner_node.kind != NodeKind::STRUCT
                    || owner_node.file_node_id.is_none()
                    || target_node.file_node_id.is_none()
                    || target_node.file_node_id == Some(NodeId(fact.callsite.file_id.0))
                    || !context.has_member(*owner, fact.caller)
                    || !context.has_member(*owner, target)
                {
                    return Err(proof_error(
                        "Go ImplicitReceiver does not own caller and target through direct membership",
                    ));
                }
            }
            (
                CalleeForm::ImplicitReceiver,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: imported_owner,
                    },
                    ResolutionEvidence::ImplicitReceiver { owner },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) if *imported_owner == *owner && *declaration == target => {
                let source_file = NodeId(fact.callsite.file_id.0);
                let import_node = context
                    .node_by_id
                    .get(import)
                    .ok_or_else(|| proof_error("imported implicit receiver binding is missing"))?;
                let owner_node = context
                    .node_by_id
                    .get(owner)
                    .ok_or_else(|| proof_error("imported implicit receiver owner is missing"))?;
                let caller_node = context
                    .node_by_id
                    .get(&fact.caller)
                    .ok_or_else(|| proof_error("imported implicit receiver caller is missing"))?;
                if import_node.kind != NodeKind::MODULE
                    || import_node.file_node_id != Some(source_file)
                    || !matches!(owner_node.kind, NodeKind::STRUCT | NodeKind::ENUM)
                    || owner_node.file_node_id.is_none()
                    || owner_node.file_node_id == Some(source_file)
                    || caller_node.kind != NodeKind::METHOD
                    || caller_node.file_node_id != Some(source_file)
                    || target_node.file_node_id != Some(source_file)
                    || !context.has_import(source_file, *import, *owner)
                    || !context.has_member(*owner, fact.caller)
                    || !context.has_member(*owner, target)
                {
                    return Err(proof_error(
                        "imported ImplicitReceiver does not name one IMPORT owner and its literal caller/target members",
                    ));
                }
            }
            (CalleeForm::QualifiedPath, [ResolutionEvidence::QualifiedPath { components }])
                if components.last() == Some(&target) && components.len() >= 2 =>
            {
                if components
                    .windows(2)
                    .any(|pair| !context.has_member(pair[0], pair[1]))
                {
                    return Err(proof_error(
                        "QualifiedPath is not one unique literal MEMBER chain",
                    ));
                }
            }
            (
                CalleeForm::QualifiedPath,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: owner,
                    },
                    ResolutionEvidence::QualifiedPath { components },
                ],
            ) if components.first() == Some(owner) && components.last() == Some(&target) => {
                let import_node = context
                    .node_by_id
                    .get(import)
                    .ok_or_else(|| proof_error("qualified import binding is missing"))?;
                if import_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                    || !context.has_import(NodeId(fact.callsite.file_id.0), *import, *owner)
                    || components
                        .windows(2)
                        .any(|pair| !context.has_member(pair[0], pair[1]))
                {
                    return Err(proof_error(
                        "qualified imported path is not one unique IMPORT and MEMBER chain",
                    ));
                }
            }
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::ConstructorBinding { constructor },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) if *constructor == *receiver_type && *declaration == target => {
                let owner = *constructor;
                let owner_node = context
                    .node_by_id
                    .get(&owner)
                    .ok_or_else(|| proof_error("constructor receiver owner is missing"))?;
                if !matches!(
                    owner_node.kind,
                    NodeKind::CLASS | NodeKind::STRUCT | NodeKind::ENUM
                ) || if fact.provenance.language_adapter == "go" {
                    owner_node.file_node_id.is_none()
                        || target_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                } else {
                    owner_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                        || owner_node.file_node_id != target_node.file_node_id
                } || !context.has_member(owner, target)
                {
                    return Err(proof_error(
                        "ConstructorBinding and ExplicitReceiverType do not name the unique target owner/member",
                    ));
                }
            }
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) if *declaration == target => {
                let owner = *receiver_type;
                let owner_node = context
                    .node_by_id
                    .get(&owner)
                    .ok_or_else(|| proof_error("explicit receiver type is missing"))?;
                if !matches!(
                    owner_node.kind,
                    NodeKind::CLASS | NodeKind::STRUCT | NodeKind::ENUM
                ) || if fact.provenance.language_adapter == "go" {
                    owner_node.file_node_id.is_none()
                        || target_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                } else {
                    owner_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                        || owner_node.file_node_id != target_node.file_node_id
                } || !context.has_member(owner, target)
                {
                    return Err(proof_error(
                        "ExplicitReceiverType does not name the unique target owner/member",
                    ));
                }
            }
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::ConstructorBinding { constructor },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SamePackageDeclaration { declaration },
                ],
            ) if matches!(fact.provenance.language_adapter.as_str(), "go" | "java")
                && *constructor == *receiver_type
                && *declaration == target =>
            {
                if fact.provenance.language_adapter == "go" {
                    Self::validate_go_package_receiver(
                        context,
                        fact.callsite.file_id,
                        *constructor,
                        target,
                        target_node,
                    )?;
                } else {
                    Self::validate_java_package_receiver(context, fact, *constructor, target)?;
                }
            }
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SamePackageDeclaration { declaration },
                ],
            ) if matches!(fact.provenance.language_adapter.as_str(), "go" | "java")
                && *declaration == target =>
            {
                if fact.provenance.language_adapter == "go" {
                    Self::validate_go_package_receiver(
                        context,
                        fact.callsite.file_id,
                        *receiver_type,
                        target,
                        target_node,
                    )?;
                } else {
                    Self::validate_java_package_receiver(context, fact, *receiver_type, target)?;
                }
            }
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: owner,
                    },
                    ResolutionEvidence::ConstructorBinding { constructor },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::QualifiedPath { components },
                ],
            ) if *owner == *constructor && *owner == *receiver_type => {
                Self::validate_imported_receiver_evidence(
                    fact,
                    context,
                    *import,
                    *owner,
                    target,
                    target_node,
                    Some(components),
                )?;
            }
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: owner,
                    },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::QualifiedPath { components },
                ],
            ) if *owner == *receiver_type => {
                Self::validate_imported_receiver_evidence(
                    fact,
                    context,
                    *import,
                    *owner,
                    target,
                    target_node,
                    Some(components),
                )?;
            }
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: owner,
                    },
                    ResolutionEvidence::ConstructorBinding { constructor },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) if *owner == *constructor && *owner == *receiver_type && *declaration == target => {
                Self::validate_imported_receiver_evidence(
                    fact,
                    context,
                    *import,
                    *owner,
                    target,
                    target_node,
                    None,
                )?;
            }
            (
                CalleeForm::ExplicitReceiver,
                [
                    ResolutionEvidence::StaticImportBinding {
                        import,
                        declaration: owner,
                    },
                    ResolutionEvidence::ExplicitReceiverType { receiver_type },
                    ResolutionEvidence::SameFileDeclaration { declaration },
                ],
            ) if *owner == *receiver_type && *declaration == target => {
                Self::validate_imported_receiver_evidence(
                    fact,
                    context,
                    *import,
                    *owner,
                    target,
                    target_node,
                    None,
                )?;
            }
            _ => {
                return Err(proof_error(
                    "typed evidence has no implemented exact semantic validator",
                ));
            }
        }
        let edge = context
            .edge_index_by_id
            .get(&edge_id)
            .map(|index| &context.edges[*index])
            .ok_or_else(|| proof_error("matching ordinary CALL edge is missing"))?;
        if edge.kind != EdgeKind::CALL
            || edge.effective_source() != fact.caller
            || edge.effective_target() != target
            || edge.resolved_target != Some(target)
            || edge.target != raw_edge_target
            || edge.file_node_id != Some(NodeId(fact.callsite.file_id.0))
            || edge.line != Some(fact.callsite.line)
            || !edge.candidate_targets.is_empty()
        {
            return Err(proof_error(
                "matching ordinary CALL edge has different kind, endpoints, candidates, file, or callsite",
            ));
        }
        let callsite = edge
            .callsite_identity
            .as_deref()
            .ok_or_else(|| proof_error("matching ordinary CALL edge has no exact callsite"))?;
        if callsite != raw_callsite_identity {
            return Err(proof_error(
                "matching ordinary CALL edge has a different canonical callsite identity",
            ));
        }
        let parsed = parse_canonical_callsite_identity(callsite).ok_or_else(|| {
            proof_error("matching ordinary CALL edge has a noncanonical callsite identity")
        })?;
        if parsed.file_id != fact.callsite.file_id
            || parsed.line != fact.callsite.line
            || parsed.column_or_ordinal == 0
            || parsed.raw_target != raw_edge_target
        {
            return Err(proof_error(
                "matching ordinary CALL edge has a different exact callsite identity",
            ));
        }
        Ok(())
    }

    fn validate_imported_receiver_evidence(
        fact: &CallResolutionFact,
        context: &ProofResolutionValidationContext,
        import: NodeId,
        owner: NodeId,
        target: NodeId,
        target_node: &Node,
        components: Option<&[NodeId]>,
    ) -> Result<(), StorageError> {
        let import_node = context
            .node_by_id
            .get(&import)
            .ok_or_else(|| proof_error("imported receiver binding is missing"))?;
        let owner_node = context
            .node_by_id
            .get(&owner)
            .ok_or_else(|| proof_error("imported receiver owner is missing"))?;
        let source_file = NodeId(fact.callsite.file_id.0);
        let java_kotlin_literal_import =
            matches!(fact.provenance.language_adapter.as_str(), "java" | "kotlin")
                && matches!(import_node.kind, NodeKind::MODULE | NodeKind::UNKNOWN)
                && owner_node.qualified_name.as_deref()
                    == Some(import_node.serialized_name.as_str());
        let python_relative = fact.provenance.language_adapter == "python"
            && components.is_some_and(|components| {
                components.len() >= 4
                    && components[components.len() - 2] == owner
                    && components.last() == Some(&target)
                    && context
                        .has_python_import_path(source_file, &components[..components.len() - 2])
                    && validate_python_import_target(
                        context,
                        source_file,
                        components[components.len() - 3],
                        owner,
                    )
            });
        let swift_module_import = fact.provenance.language_adapter == "swift"
            && components.is_some_and(|components| components == [owner, target])
            && import_node.kind == NodeKind::MODULE
            && context.has_swift_module_import(source_file, import)
            && context.swift_public_node_ids.contains(&owner)
            && context.swift_public_node_ids.contains(&target)
            && owner_node
                .file_node_id
                .and_then(|file_id| context.file_by_id.get(&file_id.0))
                .and_then(|file| swift_project_module(&file.path))
                == Some(import_node.serialized_name.as_str())
            && {
                let mut expected = context
                    .file_by_id
                    .values()
                    .filter(|file| {
                        file.indexed
                            && file.language == "swift"
                            && swift_project_module(&file.path)
                                == Some(import_node.serialized_name.as_str())
                    })
                    .map(|file| FileId(file.id))
                    .collect::<BTreeSet<_>>();
                expected.insert(fact.callsite.file_id);
                !expected.is_empty() && dependency_file_ids(fact) == expected
            };
        let dart_literal_import = fact.provenance.language_adapter == "dart"
            && components.is_some_and(|components| components == [owner, target])
            && target_node.file_node_id.is_some_and(|target_file| {
                dart_literal_import_target_is_authenticated(context, fact, import, target_file)
            });
        if import_node.file_node_id != Some(source_file)
            || !matches!(
                owner_node.kind,
                NodeKind::CLASS | NodeKind::STRUCT | NodeKind::ENUM
            )
            || owner_node.file_node_id != target_node.file_node_id
            || !(python_relative
                || java_kotlin_literal_import
                || swift_module_import
                || dart_literal_import
                || context.has_import(source_file, import, owner))
            || !context.has_member(owner, target)
        {
            return Err(proof_error(
                "imported receiver evidence does not name one IMPORT owner and one MEMBER target",
            ));
        }
        Ok(())
    }

    fn validate_go_package_receiver(
        context: &ProofResolutionValidationContext,
        source_file: FileId,
        owner: NodeId,
        target: NodeId,
        target_node: &Node,
    ) -> Result<(), StorageError> {
        let owner_node = context
            .node_by_id
            .get(&owner)
            .ok_or_else(|| proof_error("Go receiver owner is missing"))?;
        if !matches!(owner_node.kind, NodeKind::STRUCT | NodeKind::CLASS)
            || owner_node.file_node_id.is_none()
            || target_node.file_node_id.is_none()
            || target_node.file_node_id == Some(NodeId(source_file.0))
            || !context.has_member(owner, target)
        {
            return Err(proof_error(
                "Go receiver evidence does not name one direct owner/member",
            ));
        }
        Ok(())
    }

    fn validate_java_package_receiver(
        context: &ProofResolutionValidationContext,
        fact: &CallResolutionFact,
        owner: NodeId,
        target: NodeId,
    ) -> Result<String, StorageError> {
        let source_file = fact.callsite.file_id;
        let owner_node = context
            .node_by_id
            .get(&owner)
            .ok_or_else(|| proof_error("Java same-package receiver owner is missing"))?;
        let target_node = context
            .node_by_id
            .get(&target)
            .ok_or_else(|| proof_error("Java same-package receiver target is missing"))?;
        let caller_node = context
            .node_by_id
            .get(&fact.caller)
            .ok_or_else(|| proof_error("Java same-package receiver caller is missing"))?;
        let owner_file = owner_node
            .file_node_id
            .ok_or_else(|| proof_error("Java same-package receiver owner has no source file"))?;
        let target_file = target_node
            .file_node_id
            .ok_or_else(|| proof_error("Java same-package receiver target has no source file"))?;
        let source_package = context
            .java_package_identity_by_file
            .get(&source_file.0)
            .ok_or_else(|| proof_error("Java proof source has no unique package identity"))?;
        let target_package = context
            .java_package_identity_by_file
            .get(&target_file.0)
            .ok_or_else(|| proof_error("Java receiver target has no unique package identity"))?;
        let owner_qualified = owner_node
            .qualified_name
            .as_deref()
            .ok_or_else(|| proof_error("Java receiver owner has no qualified identity"))?;
        let target_owner_qualified = target_node
            .qualified_name
            .as_deref()
            .and_then(|qualified| qualified.rsplit_once('.'))
            .map(|(owner, _)| owner);
        let caller_package_prefix = format!("{source_package}.");
        if owner_node.kind != NodeKind::CLASS
            || target_node.kind != NodeKind::METHOD
            || caller_node.kind != NodeKind::METHOD
            || caller_node.file_node_id != Some(NodeId(source_file.0))
            || owner_file == NodeId(source_file.0)
            || owner_file != target_file
            || source_package != target_package
            || owner_qualified
                .strip_prefix(&caller_package_prefix)
                .is_none_or(|owner_name| owner_name.is_empty() || owner_name.contains('.'))
            || target_owner_qualified != Some(owner_qualified)
            || caller_node
                .qualified_name
                .as_deref()
                .is_none_or(|qualified| {
                    qualified
                        .strip_prefix(&caller_package_prefix)
                        .is_none_or(|relative| !relative.contains('.'))
                })
            || !context.has_member(owner, target)
        {
            return Err(proof_error(
                "Java same-package receiver does not name one authenticated package owner/member",
            ));
        }
        Ok(source_package.clone())
    }

    pub fn replace_proof_resolution_projection(
        &mut self,
        publication: &IndexPublicationRecord,
        projection: &ProofResolutionProjection,
    ) -> Result<ProofResolutionPublication, StorageError> {
        if publication.generation_id.trim().is_empty()
            || publication.run_id.trim().is_empty()
            || publication.published_at_epoch_ms < 0
        {
            return Err(proof_error("core publication identity is invalid"));
        }
        if let Some(existing) = self.get_proof_resolution_publication()?
            && existing.core_generation_id == publication.generation_id
            && existing.core_run_id == publication.run_id
        {
            return Err(proof_error(
                "rows are immutable within an already receipted staged publication",
            ));
        }
        let mut linear_facts = Vec::new();
        let mut facts = projection
            .facts
            .iter()
            .filter_map(|fact| {
                if matches!(
                    fact.provenance.language_adapter.as_str(),
                    "ruby" | "php" | "csharp" | "swift" | "dart"
                ) {
                    count_store_replay_work(1);
                    linear_facts.push(fact.clone());
                    None
                } else {
                    Some(fact.clone())
                }
            })
            .collect::<Vec<_>>();
        facts.sort_by(|left, right| {
            (
                left.callsite.file_id,
                left.callsite.start_byte,
                left.callsite.end_byte_exclusive,
                left.fact_id.as_str(),
            )
                .cmp(&(
                    right.callsite.file_id,
                    right.callsite.start_byte,
                    right.callsite.end_byte_exclusive,
                    right.fact_id.as_str(),
                ))
        });
        facts.extend(linear_facts);
        self.validate_facts_against_graph(&facts, true)?;
        let mut exact_callsites = HashSet::new();
        if facts.iter().any(|fact| {
            !exact_callsites.insert((
                fact.callsite.file_id,
                fact.callsite.start_byte,
                fact.callsite.end_byte_exclusive,
            ))
        }) {
            return Err(proof_error(
                "more than one fact owns the same exact callsite",
            ));
        }
        let mut exact_edges = HashSet::new();
        for edge_id in facts
            .iter()
            .filter(|fact| fact.status == ProofResolutionStatus::Exact)
            .filter_map(|fact| fact.edge_id)
        {
            if !exact_edges.insert(edge_id) {
                return Err(proof_error(
                    "one ordinary CALL edge backs more than one Exact fact",
                ));
            }
        }

        let mut adapter_roster = projection.adapter_roster.clone();
        adapter_roster.sort();
        validate_adapter_roster(&facts, &adapter_roster)?;
        let mut linear_funnel = Vec::new();
        let mut funnel = projection
            .funnel
            .iter()
            .filter_map(|row| {
                if matches!(
                    row.language.as_str(),
                    "ruby" | "php" | "csharp" | "swift" | "dart"
                ) {
                    count_store_replay_work(1);
                    linear_funnel.push(row.clone());
                    None
                } else {
                    Some(row.clone())
                }
            })
            .collect::<Vec<_>>();
        funnel.sort_by(|left, right| {
            (
                left.language.as_str(),
                left.callee_form.map(CalleeForm::as_str),
                left.evidence_kind.map(|kind| kind.as_str()),
            )
                .cmp(&(
                    right.language.as_str(),
                    right.callee_form.map(CalleeForm::as_str),
                    right.evidence_kind.map(|kind| kind.as_str()),
                ))
        });
        funnel.extend(linear_funnel);
        if funnel.iter().any(|row| row.language.trim().is_empty()) {
            return Err(proof_error("funnel contains an empty language"));
        }
        let expected_funnel = recompute_funnel(&facts);
        if funnel != expected_funnel {
            return Err(proof_error(
                "funnel does not deterministically match the fact rows",
            ));
        }
        let fact_digest = publication_integrity_digest(&facts, &adapter_roster, &funnel)?;
        let manifest = ProofResolutionPublication {
            core_generation_id: publication.generation_id.clone(),
            core_run_id: publication.run_id.clone(),
            fact_schema_version: PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
            adapter_roster,
            complete: true,
            fact_count: facts.len() as u64,
            fact_digest,
            funnel,
            published_at_epoch_ms: publication.published_at_epoch_ms,
        };
        let adapter_roster_json = serde_json::to_string(&manifest.adapter_roster)
            .map_err(|error| proof_error(format!("failed to serialize adapter roster: {error}")))?;
        let funnel_json = serde_json::to_string(&manifest.funnel)
            .map_err(|error| proof_error(format!("failed to serialize funnel: {error}")))?;
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM proof_resolution_publication", [])?;
        tx.execute("DELETE FROM proof_resolution_fact", [])?;
        {
            let mut statement = tx.prepare(
                "INSERT INTO proof_resolution_fact (
                    fact_id, edge_id, raw_edge_target_id, raw_callsite_identity,
                    file_id, source_sha256, start_byte,
                    end_byte_exclusive, line, column, callee_form, raw_target,
                    caller_node_id, target_node_id, status, reason, evidence_json,
                    dependency_json, lookup_domain_complete, producer,
                    fact_schema_version, algorithm, language_adapter,
                    language_adapter_version, parser_fingerprint, evidence_digest
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
                    ?25, ?26
                 )",
            )?;
            for fact in &facts {
                let evidence_json =
                    serde_json::to_string(&fact.evidence_chain).map_err(|error| {
                        proof_error(format!("failed to serialize typed evidence: {error}"))
                    })?;
                let dependency_json = serde_json::to_string(
                    &fact.provenance.dependency_file_hashes,
                )
                .map_err(|error| {
                    proof_error(format!("failed to serialize dependency hashes: {error}"))
                })?;
                statement.execute(params![
                    fact.fact_id,
                    fact.edge_id.map(|edge_id| edge_id.0),
                    fact.raw_edge_target.map(|node_id| node_id.0),
                    fact.raw_callsite_identity,
                    fact.callsite.file_id.0,
                    fact.callsite.source_sha256,
                    i64::try_from(fact.callsite.start_byte)
                        .map_err(|_| proof_error("callsite start byte exceeds SQLite integer"))?,
                    i64::try_from(fact.callsite.end_byte_exclusive)
                        .map_err(|_| proof_error("callsite end byte exceeds SQLite integer"))?,
                    i64::from(fact.callsite.line),
                    i64::from(fact.callsite.column),
                    fact.callsite.callee_form.as_str(),
                    fact.callsite.raw_target,
                    fact.caller.0,
                    fact.target.map(|target| target.0),
                    fact.status.as_str(),
                    fact.reason.as_str(),
                    evidence_json,
                    dependency_json,
                    i64::from(fact.lookup_domain_complete),
                    fact.provenance.producer,
                    i64::from(fact.provenance.fact_schema_version),
                    fact.provenance.algorithm,
                    fact.provenance.language_adapter,
                    fact.provenance.language_adapter_version,
                    fact.provenance.parser_fingerprint,
                    fact.provenance.evidence_sha256,
                ])?;
            }
        }
        tx.execute(
            "INSERT INTO proof_resolution_publication (
                id, core_generation_id, core_run_id, fact_schema_version,
                adapter_roster_json, complete, fact_count, fact_digest,
                funnel_json, published_at_epoch_ms
             ) VALUES (1, ?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8)",
            params![
                manifest.core_generation_id,
                manifest.core_run_id,
                i64::from(manifest.fact_schema_version),
                adapter_roster_json,
                i64::try_from(manifest.fact_count)
                    .map_err(|_| proof_error("fact count exceeds SQLite integer"))?,
                manifest.fact_digest,
                funnel_json,
                manifest.published_at_epoch_ms,
            ],
        )?;
        tx.commit()?;
        Ok(manifest)
    }

    pub fn validate_proof_resolution_publication(
        &self,
        publication: &IndexPublicationRecord,
    ) -> Result<ProofResolutionPublication, StorageError> {
        let (manifest, facts) = self.validate_proof_resolution_receipt(publication)?;
        self.validate_facts_against_graph(&facts, true)?;
        Ok(manifest)
    }

    pub(crate) fn validate_stored_proof_resolution_publication(
        &self,
        publication: &IndexPublicationRecord,
    ) -> Result<ProofResolutionPublication, StorageError> {
        let (manifest, facts) = self.validate_proof_resolution_receipt(publication)?;
        self.validate_facts_against_graph(&facts, false)?;
        Ok(manifest)
    }

    fn validate_proof_resolution_receipt(
        &self,
        publication: &IndexPublicationRecord,
    ) -> Result<(ProofResolutionPublication, Vec<CallResolutionFact>), StorageError> {
        let manifest = self
            .get_proof_resolution_publication()?
            .ok_or_else(|| proof_error("complete publication receipt is missing"))?;
        if !manifest.complete
            || manifest.fact_schema_version != PROOF_RESOLUTION_FACT_SCHEMA_VERSION
            || manifest.core_generation_id != publication.generation_id
            || manifest.core_run_id != publication.run_id
            || manifest.published_at_epoch_ms != publication.published_at_epoch_ms
        {
            return Err(proof_error(
                "complete publication receipt does not match the core publication",
            ));
        }
        let facts = self.get_proof_resolution_facts()?;
        validate_adapter_roster(&facts, &manifest.adapter_roster)?;
        let expected_funnel = recompute_funnel(&facts);
        if manifest.funnel != expected_funnel
            || manifest.fact_count != facts.len() as u64
            || manifest.fact_digest
                != publication_integrity_digest(&facts, &manifest.adapter_roster, &manifest.funnel)?
        {
            return Err(proof_error(
                "fact rows do not match their publication digest",
            ));
        }
        for fact in &facts {
            validate_fact_seal(fact)?;
        }
        Ok((manifest, facts))
    }

    /// Rebind an already authenticated proof projection to a semantic-only
    /// core publication. Facts, roster, funnel, and their integrity digest are
    /// unchanged. A migrated database with no projection remains absent.
    pub fn rebind_proof_resolution_publication(
        &mut self,
        previous: &IndexPublicationRecord,
        next: &IndexPublicationRecord,
    ) -> Result<Option<ProofResolutionPublication>, StorageError> {
        if self.get_proof_resolution_publication()?.is_none() {
            return Ok(None);
        }
        // A semantic-only republish is allowed to operate from the stored core
        // after working-tree sources disappear. The previous publication was
        // graph-validated before promotion; rebind authenticates its immutable
        // fact rows and receipt without consulting live source bytes.
        self.validate_stored_proof_resolution_publication(previous)?;
        if next.generation_id.trim().is_empty()
            || next.run_id.trim().is_empty()
            || next.published_at_epoch_ms < 0
        {
            return Err(proof_error("new semantic publication identity is invalid"));
        }
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE proof_resolution_publication
             SET core_generation_id = ?1, core_run_id = ?2, published_at_epoch_ms = ?3
             WHERE id = 1 AND core_generation_id = ?4 AND core_run_id = ?5
               AND published_at_epoch_ms = ?6",
            params![
                next.generation_id,
                next.run_id,
                next.published_at_epoch_ms,
                previous.generation_id,
                previous.run_id,
                previous.published_at_epoch_ms,
            ],
        )?;
        if changed != 1 {
            return Err(proof_error(
                "proof publication changed during semantic identity rebind",
            ));
        }
        tx.commit()?;
        self.validate_stored_proof_resolution_publication(next)
            .map(Some)
    }
}

fn graph_leaf_name(name: &str) -> &str {
    name.rsplit(['.', ':'])
        .find(|part| !part.is_empty())
        .unwrap_or(name)
}

fn swift_project_module(path: &std::path::Path) -> Option<&str> {
    let mut components = path.components().filter_map(|component| match component {
        std::path::Component::Normal(value) => value.to_str(),
        _ => None,
    });
    while let Some(component) = components.next() {
        if component == "Sources" {
            return components.next().filter(|module| {
                !module.is_empty()
                    && module
                        .chars()
                        .all(|character| character == '_' || character.is_alphanumeric())
            });
        }
    }
    None
}

fn dart_library_root(path: &std::path::Path) -> Option<&std::path::Path> {
    let mut current = path.parent();
    while let Some(directory) = current {
        if directory.file_name().and_then(|name| name.to_str()) == Some("lib") {
            return Some(directory);
        }
        current = directory.parent();
    }
    None
}

fn quoted_import_literal(surface: &str) -> Option<&str> {
    let start = surface.find(['\'', '"'])?;
    let quote = surface.as_bytes()[start] as char;
    let rest = &surface[start + quote.len_utf8()..];
    Some(&rest[..rest.find(quote)?])
}

fn dart_import_combinator_names(surface: &str, keyword: &str) -> Option<HashSet<String>> {
    let marker = format!(" {keyword} ");
    let (_, tail) = surface.split_once(&marker)?;
    let end = [" show ", " hide "]
        .into_iter()
        .filter_map(|next| tail.find(next))
        .min()
        .unwrap_or(tail.len());
    Some(
        tail[..end]
            .trim_end_matches(';')
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn dart_literal_import_target_is_authenticated(
    context: &ProofResolutionValidationContext,
    fact: &CallResolutionFact,
    import: NodeId,
    target_file: NodeId,
) -> bool {
    let (Some(source), Some(target), Some(import_node)) = (
        context.file_by_id.get(&fact.callsite.file_id.0),
        context.file_by_id.get(&target_file.0),
        context.node_by_id.get(&import),
    ) else {
        return false;
    };
    let Some(uri) = quoted_import_literal(&import_node.serialized_name) else {
        return false;
    };
    let imported_declaration = fact
        .evidence_chain
        .iter()
        .find_map(|evidence| match evidence {
            ResolutionEvidence::StaticImportBinding {
                import: evidence_import,
                declaration,
            } if *evidence_import == import => Some(*declaration),
            _ => None,
        });
    let Some(imported_name) = imported_declaration
        .and_then(|declaration| context.node_by_id.get(&declaration))
        .map(|node| graph_leaf_name(&node.serialized_name))
    else {
        return false;
    };
    let relative = std::path::Path::new(uri);
    let Some(source_directory) = source.path.parent() else {
        return false;
    };
    let expected_path = source_directory.join(relative);
    let exact_native_target = std::fs::symlink_metadata(&expected_path)
        .ok()
        .is_some_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
        && std::fs::canonicalize(&expected_path).ok() == std::fs::canonicalize(&target.path).ok();
    let same_library = dart_library_root(&source.path)
        .zip(dart_library_root(&target.path))
        .is_some_and(|(source_root, target_root)| {
            std::fs::canonicalize(source_root).ok() == std::fs::canonicalize(target_root).ok()
        });
    let expected_dependencies = csd_dependency_ids(fact, context).ok();
    let observed_dependencies = fact
        .provenance
        .dependency_file_hashes
        .iter()
        .map(|dependency| dependency.file_id)
        .collect::<Vec<_>>();
    import_node.kind == NodeKind::MODULE
        && import_node.file_node_id == Some(NodeId(fact.callsite.file_id.0))
        && relative
            .extension()
            .and_then(|extension| extension.to_str())
            == Some("dart")
        && relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        && source.id != target.id
        && context
            .dart_import_visibility_by_node
            .get(&import)
            .is_some_and(|visibility| {
                visibility
                    .shown
                    .as_ref()
                    .is_none_or(|shown| shown.contains(imported_name))
                    && !visibility.hidden.contains(imported_name)
            })
        && target.language == "dart"
        && target.indexed
        && exact_native_target
        && same_library
        && expected_dependencies.as_deref() == Some(observed_dependencies.as_slice())
}

fn proof_import_node_kind_is_literal(language: &str, kind: NodeKind) -> bool {
    match language {
        "rust" => kind == NodeKind::MODULE,
        "java" | "kotlin" | "csharp" | "swift" | "dart" | "javascript" | "typescript" | "tsx"
        | "python" | "php" | "ruby" => {
            matches!(kind, NodeKind::MODULE | NodeKind::UNKNOWN)
        }
        _ => false,
    }
}

fn normalize_stored_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            std::path::Component::Normal(name) => normalized.push(name),
        }
    }
    Some(normalized)
}

fn stored_paths_match(left: &Path, right: &Path) -> bool {
    let native_match = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            match (
                std::fs::symlink_metadata(left),
                std::fs::symlink_metadata(right),
            ) {
                (Ok(left), Ok(right)) => {
                    Some((left.dev(), left.ino()) == (right.dev(), right.ino()))
                }
                (Err(left), _) if left.kind() == std::io::ErrorKind::NotFound => None,
                (_, Err(right)) if right.kind() == std::io::ErrorKind::NotFound => None,
                _ => Some(false),
            }
        }
        #[cfg(not(unix))]
        {
            match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
                (Ok(left), Ok(right)) => Some(
                    left.to_string_lossy()
                        .eq_ignore_ascii_case(&right.to_string_lossy()),
                ),
                (Err(left), _) if left.kind() == std::io::ErrorKind::NotFound => None,
                (_, Err(right)) if right.kind() == std::io::ErrorKind::NotFound => None,
                _ => Some(false),
            }
        }
    };
    native_match.unwrap_or_else(|| {
        let (Some(left), Some(right)) = (normalize_stored_path(left), normalize_stored_path(right))
        else {
            return false;
        };
        #[cfg(windows)]
        {
            left.to_string_lossy()
                .eq_ignore_ascii_case(&right.to_string_lossy())
        }
        #[cfg(not(windows))]
        {
            left == right
        }
    })
}

fn typescript_source_extension_is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "ts" | "tsx"))
}

fn typescript_directory_import_target_is_authenticated(
    context: &ProofResolutionValidationContext,
    fact: &CallResolutionFact,
    import: NodeId,
    target: NodeId,
) -> bool {
    if !matches!(
        fact.provenance.language_adapter.as_str(),
        "typescript" | "tsx"
    ) {
        return false;
    }
    let source_file_id = NodeId(fact.callsite.file_id.0);
    let Some(import_node) = context.node_by_id.get(&import) else {
        return false;
    };
    if import_node.file_node_id != Some(source_file_id)
        || import_node.kind != NodeKind::UNKNOWN
        || graph_leaf_name(&import_node.serialized_name) != fact.callsite.raw_target
    {
        return false;
    }
    let Some(module_specifier) = context.typescript_directory_import(source_file_id, import) else {
        return false;
    };
    let Some(source_file) = context.file_by_id.get(&fact.callsite.file_id.0) else {
        return false;
    };
    if !matches!(source_file.language.as_str(), "typescript" | "tsx")
        || !typescript_source_extension_is_supported(&source_file.path)
    {
        return false;
    }
    let Some(target_file_id) = context
        .node_by_id
        .get(&target)
        .filter(|node| {
            matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                && graph_leaf_name(&node.serialized_name) == fact.callsite.raw_target
        })
        .and_then(|node| node.file_node_id)
    else {
        return false;
    };
    if !dependency_file_ids(fact).contains(&FileId(target_file_id.0)) {
        return false;
    }
    let Some(target_file) = context.file_by_id.get(&target_file_id.0) else {
        return false;
    };
    if !matches!(target_file.language.as_str(), "typescript" | "tsx") {
        return false;
    }
    let Some(parent) = source_file.path.parent() else {
        return false;
    };
    let Some(directory) = normalize_stored_path(&parent.join(module_specifier)) else {
        return false;
    };
    [directory.join("index.ts"), directory.join("index.tsx")]
        .into_iter()
        .filter_map(|path| normalize_stored_path(&path))
        .any(|path| stored_paths_match(&target_file.path, &path))
}

fn publication_integrity_digest(
    facts: &[CallResolutionFact],
    adapter_roster: &[ProofResolutionAdapter],
    funnel: &[ProofResolutionFunnelRow],
) -> Result<String, StorageError> {
    let mut hasher = Sha256::new();
    hasher.update(PUBLICATION_DIGEST_DOMAIN);
    for value in [
        serde_json::to_vec(adapter_roster),
        serde_json::to_vec(funnel),
    ] {
        let bytes = value.map_err(|error| {
            proof_error(format!(
                "failed to serialize publication integrity row: {error}"
            ))
        })?;
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    for fact in facts {
        let bytes = serde_json::to_vec(fact)
            .map_err(|error| proof_error(format!("failed to serialize fact row: {error}")))?;
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn recompute_funnel(facts: &[CallResolutionFact]) -> Vec<ProofResolutionFunnelRow> {
    let mut rows = BTreeMap::<
        (String, Option<CalleeForm>, Option<ResolutionEvidenceKind>),
        ProofResolutionFunnelCounts,
    >::new();
    let mut linear_rows = HashMap::<
        (String, Option<CalleeForm>, Option<ResolutionEvidenceKind>),
        ProofResolutionFunnelCounts,
    >::new();
    for fact in facts {
        let evidence_kind = fact.evidence_chain.first().map(ResolutionEvidence::kind);
        let key = (
            fact.provenance.language_adapter.clone(),
            Some(fact.callsite.callee_form),
            evidence_kind,
        );
        let counts = if matches!(
            fact.provenance.language_adapter.as_str(),
            "ruby" | "php" | "csharp" | "swift" | "dart"
        ) {
            count_store_replay_work(1);
            linear_rows.entry(key).or_default()
        } else {
            rows.entry(key).or_default()
        };
        counts.syntax_calls += 1;
        counts.adapter_supported += u64::from(fact.status != ProofResolutionStatus::Unsupported);
        match fact.status {
            ProofResolutionStatus::Exact => counts.exact += 1,
            ProofResolutionStatus::Ambiguous => counts.ambiguous += 1,
            ProofResolutionStatus::Unsupported => counts.unsupported += 1,
            ProofResolutionStatus::MissingBinding => counts.missing_binding += 1,
            ProofResolutionStatus::IncompleteDomain => counts.incomplete_domain += 1,
        }
        counts.exact_call_linked +=
            u64::from(fact.status == ProofResolutionStatus::Exact && fact.edge_id.is_some());
    }
    let mut result = rows
        .into_iter()
        .map(
            |((language, callee_form, evidence_kind), counts)| ProofResolutionFunnelRow {
                language,
                callee_form,
                evidence_kind,
                counts,
            },
        )
        .collect::<Vec<_>>();
    result.sort_by(|left, right| {
        (
            left.language.as_str(),
            left.callee_form.map(CalleeForm::as_str),
            left.evidence_kind.map(|kind| kind.as_str()),
        )
            .cmp(&(
                right.language.as_str(),
                right.callee_form.map(CalleeForm::as_str),
                right.evidence_kind.map(|kind| kind.as_str()),
            ))
    });
    for language in ["csharp", "dart", "php", "ruby", "swift"] {
        for callee_form in [
            CalleeForm::Constructor,
            CalleeForm::DynamicAccess,
            CalleeForm::ExplicitReceiver,
            CalleeForm::Identifier,
            CalleeForm::ImplicitReceiver,
            CalleeForm::NamedImport,
            CalleeForm::QualifiedPath,
        ] {
            for evidence_kind in [
                None,
                Some(ResolutionEvidenceKind::ConstructorBinding),
                Some(ResolutionEvidenceKind::ExplicitReceiverType),
                Some(ResolutionEvidenceKind::ImplicitReceiver),
                Some(ResolutionEvidenceKind::QualifiedPath),
                Some(ResolutionEvidenceKind::SameFileDeclaration),
                Some(ResolutionEvidenceKind::SamePackageDeclaration),
                Some(ResolutionEvidenceKind::StaticImportBinding),
            ] {
                let key = (language.to_owned(), Some(callee_form), evidence_kind);
                count_store_replay_work(1);
                if let Some(counts) = linear_rows.remove(&key) {
                    result.push(ProofResolutionFunnelRow {
                        language: language.to_owned(),
                        callee_form: Some(callee_form),
                        evidence_kind,
                        counts,
                    });
                }
            }
        }
    }
    debug_assert!(linear_rows.is_empty());
    result
}
