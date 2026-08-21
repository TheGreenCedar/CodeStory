//! Dark core-snapshot adapter for indexed source call-path proof facts.
//!
//! This module is deliberately not a product facade. Its caller must already
//! be inside `PublicOperationService::run_with_cancel`, which installs the
//! complete core snapshot used for every Store read below.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use codestory_agent::indexed_source_call_path_v1::{
    AdmittedRawCallEdge, BuiltCallPathFacts, CallableContainmentEvidence, ExactScopeSelector,
    ExactSymbolSelector, FactBuildGap, IndexedCallEdgeReceipt, IndexedLineWindow,
    PinnedNodeIdentity, RawCallEdgeAdmission, ReceiptRef, ResolvedNodeIdentity, UnavailableReason,
    ValidatedCallPathContract, VerifiedDirectCallFact, VerifiedProofFact, admit_raw_call_edge,
};
use codestory_contracts::api::ApiError;
use codestory_contracts::graph::{Node, NodeId, NodeKind, ResolutionCertainty};
use codestory_store::{FileInfo, IndexPublicationRecord, Store};
use codestory_workspace::{
    ProjectRelativePathResolution, WorkspacePathIdentity, project_identity_v3,
    resolve_project_relative_path, workspace_relative_path,
};
use sha2::{Digest, Sha256};

use crate::AppController;
use crate::path_identity::OperationPathIdentityResolver;

const INDEXED_LINE_KIND: &str = "indexed_line_v1";
const MAX_LINE_WINDOW_BYTES: usize = 8_192;
const RECEIPT_DOMAIN: &[u8] = b"codestory.indexed-call-edge-receipt.v1\0";
const CALLABLE_KINDS: [NodeKind; 3] = [NodeKind::FUNCTION, NodeKind::METHOD, NodeKind::MACRO];

#[allow(dead_code)] // Task 2C wires the accepted dark contract into this leaf.
pub(crate) fn build_indexed_source_call_path_facts(
    controller: &AppController,
    contract: &ValidatedCallPathContract,
) -> Result<BuiltCallPathFacts, ApiError> {
    let publication = controller.active_core_publication().ok_or_else(|| {
        ApiError::internal("indexed call-path proof requires an active core publication")
    })?;
    let project_root = controller.require_project_root()?;
    let project_id = project_identity_v3(&project_root).project_id;
    let storage = controller.open_storage_read_only()?;
    build_from_store(
        &storage,
        &project_root,
        &project_id,
        &publication,
        contract,
        |path| fs::read(path),
    )
}

fn build_from_store<R>(
    store: &Store,
    project_root: &Path,
    project_id: &str,
    publication: &IndexPublicationRecord,
    contract: &ValidatedCallPathContract,
    mut read_source: R,
) -> Result<BuiltCallPathFacts, ApiError>
where
    R: FnMut(&Path) -> io::Result<Vec<u8>>,
{
    let files = store.files().inventory().map_err(store_error)?;
    let file_rows = store.files().get_files().map_err(store_error)?;
    let mut path_identities = OperationPathIdentityResolver::native();
    let mut resolved = Vec::with_capacity(contract.spec().steps().len() + 1);
    let mut gaps = Vec::new();
    let mut unavailable = Vec::new();
    let selector_context = SelectorContext {
        store,
        project_root,
        project_id,
        publication,
        files: &file_rows,
    };

    for (selector_index, selector) in std::iter::once(contract.spec().start())
        .chain(contract.spec().steps().iter().map(|step| step.target()))
        .enumerate()
    {
        match resolve_symbol_selector(&selector_context, selector, &mut path_identities)? {
            SelectorResolution::Resolved(node) => resolved.push(node),
            SelectorResolution::Missing => {
                gaps.push(FactBuildGap::SelectorMissing { selector_index })
            }
            SelectorResolution::Ambiguous => {
                gaps.push(FactBuildGap::SelectorAmbiguous { selector_index })
            }
            SelectorResolution::NonCallable => {
                gaps.push(FactBuildGap::NonCallableSelector { selector_index })
            }
            SelectorResolution::Unavailable(reason) => unavailable.push(reason),
        }
    }

    for (scope_index, selector) in (contract.spec().steps().len() + 1..).zip(
        contract
            .spec()
            .traversal_prohibitions()
            .iter()
            .chain(contract.spec().projection_exclusions()),
    ) {
        match resolve_scope_selector(&selector_context, selector, &mut path_identities)? {
            SelectorResolution::Resolved(_) => {}
            SelectorResolution::Missing => gaps.push(FactBuildGap::SelectorMissing {
                selector_index: scope_index,
            }),
            SelectorResolution::Ambiguous => gaps.push(FactBuildGap::SelectorAmbiguous {
                selector_index: scope_index,
            }),
            SelectorResolution::NonCallable => gaps.push(FactBuildGap::NonCallableSelector {
                selector_index: scope_index,
            }),
            SelectorResolution::Unavailable(reason) => unavailable.push(reason),
        }
    }

    unavailable.sort();
    unavailable.dedup();
    if !unavailable.is_empty()
        || !gaps.is_empty()
        || resolved.len() != contract.spec().steps().len() + 1
    {
        gaps.sort();
        gaps.dedup();
        return Ok(BuiltCallPathFacts {
            facts: Vec::new(),
            receipts: Vec::new(),
            gaps,
            unavailable,
        });
    }

    let files_by_id = files
        .into_iter()
        .map(|file| (file.id, file))
        .collect::<HashMap<_, _>>();
    let rows_by_id = file_rows
        .into_iter()
        .map(|file| (file.id, file))
        .collect::<HashMap<_, _>>();
    let mut source_cache = HashMap::<WorkspacePathIdentity, SourceObservation>::new();
    let mut facts = Vec::new();
    let mut receipts = Vec::new();

    for (step_index, pair) in resolved.windows(2).enumerate() {
        let source = &pair[0];
        let target = &pair[1];
        let Some(source_id) = parse_pinned_node_id(&source.pinned) else {
            return Err(ApiError::internal(
                "resolved call-path source lost its numeric publication pin",
            ));
        };
        let Some(target_id) = parse_pinned_node_id(&target.pinned) else {
            return Err(ApiError::internal(
                "resolved call-path target lost its numeric publication pin",
            ));
        };
        let Some(source_node) = store.get_node(source_id).map_err(store_error)? else {
            return Err(ApiError::internal(
                "resolved call-path source disappeared from the pinned snapshot",
            ));
        };
        let Some(target_node) = store.get_node(target_id).map_err(store_error)? else {
            return Err(ApiError::internal(
                "resolved call-path target disappeared from the pinned snapshot",
            ));
        };
        let mut admitted_any = false;
        let mut step_unavailable = Vec::new();
        let mut step_gaps = Vec::new();
        let mut containment_failed = false;
        for edge in store
            .get_raw_call_edges_by_effective_source(source_id)
            .map_err(store_error)?
        {
            let RawCallEdgeAdmission::Admitted(admitted) =
                admit_raw_call_edge(&edge, source_id, target_id)
            else {
                continue;
            };
            if !is_callable(source_node.kind) || !is_callable(target_node.kind) {
                continue;
            }
            let Some(source_file_id) = source_node.file_node_id else {
                continue;
            };
            if admitted.file_node_id != source_file_id {
                continue;
            }
            let Some(containment) = authenticate_containment(
                store,
                &source_node,
                admitted.file_node_id,
                admitted.line,
            )?
            else {
                containment_failed = true;
                continue;
            };
            let Some(file) = files_by_id.get(&admitted.file_node_id.0) else {
                step_unavailable.push(UnavailableReason::SourceNotBoundToPublication);
                continue;
            };
            if !file.indexed || !file.complete {
                step_unavailable.push(UnavailableReason::SourceNotBoundToPublication);
                continue;
            }
            let Some(file_row) = rows_by_id.get(&file.id) else {
                step_unavailable.push(UnavailableReason::SourceNotBoundToPublication);
                continue;
            };
            let Some(indexed_hash) = file.content_hash.as_deref() else {
                step_unavailable.push(UnavailableReason::SourceNotBoundToPublication);
                continue;
            };
            let bound = match bind_source_once(
                project_root,
                file_row,
                indexed_hash,
                &mut path_identities,
                &mut source_cache,
                &mut read_source,
            ) {
                Ok(bound) => bound,
                Err(BindSourceError::Unavailable) => {
                    step_unavailable.push(UnavailableReason::SourceNotBoundToPublication);
                    continue;
                }
                Err(BindSourceError::InvalidUtf8) => {
                    step_gaps.push(FactBuildGap::InvalidUtf8 { step_index });
                    continue;
                }
            };
            let Some((byte_start, byte_end, text)) = complete_line(&bound.bytes, admitted.line)
            else {
                step_gaps.push(FactBuildGap::SourceLineOutOfRange { step_index });
                continue;
            };
            if byte_end - byte_start > MAX_LINE_WINDOW_BYTES {
                step_gaps.push(FactBuildGap::SourceWindowTooLarge { step_index });
                continue;
            }
            let line_window = IndexedLineWindow {
                kind: INDEXED_LINE_KIND,
                project_file_components: bound.project_file_components.clone(),
                indexed_sha256: indexed_hash.to_owned(),
                observed_sha256: bound.observed_sha256.clone(),
                anchor_line: admitted.line,
                byte_start,
                byte_end,
                text,
            };
            let receipt_id = receipt_id(
                project_id,
                publication,
                &admitted,
                source_id.0,
                target_id.0,
                indexed_hash,
            );
            let receipt_ref = ReceiptRef {
                receipt_id,
                edge_id: admitted.edge_id.0.to_string(),
            };
            receipts.push(IndexedCallEdgeReceipt {
                receipt: receipt_ref.clone(),
                source: source.clone(),
                target: target.clone(),
                certainty: ResolutionCertainty::Certain,
                callsite_identity: admitted.callsite_identity,
                containment,
                line_window,
            });
            facts.push(VerifiedProofFact::DirectCall(VerifiedDirectCallFact {
                receipt: receipt_ref,
                source: source.clone(),
                target: target.clone(),
            }));
            admitted_any = true;
        }
        if admitted_any {
            continue;
        }
        if !step_unavailable.is_empty() || !step_gaps.is_empty() {
            unavailable.append(&mut step_unavailable);
            gaps.append(&mut step_gaps);
        } else {
            gaps.push(if source_id == target_id {
                FactBuildGap::RecursiveCallNotRepresentable { step_index }
            } else if containment_failed {
                FactBuildGap::EdgeContainmentUnproven { step_index }
            } else {
                FactBuildGap::DirectCallMissing { step_index }
            });
        }
    }
    gaps.sort();
    gaps.dedup();
    unavailable.sort();
    unavailable.dedup();
    Ok(BuiltCallPathFacts {
        facts,
        receipts,
        gaps,
        unavailable,
    })
}

#[derive(Debug)]
enum SelectorResolution {
    Resolved(ResolvedNodeIdentity),
    Missing,
    Ambiguous,
    NonCallable,
    Unavailable(UnavailableReason),
}

struct SelectorContext<'a> {
    store: &'a Store,
    project_root: &'a Path,
    project_id: &'a str,
    publication: &'a IndexPublicationRecord,
    files: &'a [FileInfo],
}

fn resolve_scope_selector(
    context: &SelectorContext<'_>,
    selector: &ExactScopeSelector,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    match selector {
        ExactScopeSelector::PinnedNode(identity) => resolve_pinned(context, identity, identities),
        ExactScopeSelector::CanonicalId(value) => resolve_canonical(context, value, identities),
        ExactScopeSelector::QualifiedName {
            qualified_name,
            project_file_components,
        } => resolve_qualified(
            context,
            qualified_name,
            project_file_components.as_deref(),
            identities,
        ),
    }
}

fn resolve_symbol_selector(
    context: &SelectorContext<'_>,
    selector: &ExactSymbolSelector,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    match selector {
        ExactSymbolSelector::PinnedNode(identity) => resolve_pinned(context, identity, identities),
        ExactSymbolSelector::CanonicalId(value) => resolve_canonical(context, value, identities),
        ExactSymbolSelector::QualifiedName {
            qualified_name,
            project_file_components,
        } => resolve_qualified(
            context,
            qualified_name,
            project_file_components.as_deref(),
            identities,
        ),
    }
}

fn resolve_pinned(
    context: &SelectorContext<'_>,
    identity: &PinnedNodeIdentity,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    if identity.project_id != context.project_id
        || identity.core_generation_id != context.publication.generation_id
        || identity.core_run_id != context.publication.run_id
    {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::PublicationPinMismatch,
        ));
    }
    let Some(node_id) = parse_pinned_node_id(identity) else {
        return Ok(SelectorResolution::Missing);
    };
    let Some(node) = context.store.get_node(node_id).map_err(store_error)? else {
        return Ok(SelectorResolution::Missing);
    };
    resolved_node(context, node, identities)
}

fn resolve_canonical(
    context: &SelectorContext<'_>,
    canonical_id: &str,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    let matches = context
        .store
        .node_ids_by_canonical_ids(&[canonical_id.to_owned()])
        .map_err(store_error)?
        .remove(canonical_id)
        .unwrap_or_default();
    resolve_unique_node(context, matches, identities)
}

fn resolve_qualified(
    context: &SelectorContext<'_>,
    qualified_name: &str,
    components: Option<&[String]>,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    let selected_files = match components {
        Some(components) => {
            let requested = components.iter().collect::<PathBuf>();
            let Ok(ProjectRelativePathResolution::Existing { absolute, .. }) =
                resolve_project_relative_path(context.project_root, &requested)
            else {
                return Ok(SelectorResolution::Unavailable(
                    UnavailableReason::SourceNotBoundToPublication,
                ));
            };
            let Ok(wanted) = identities.resolve(&absolute) else {
                return Ok(SelectorResolution::Unavailable(
                    UnavailableReason::SourceNotBoundToPublication,
                ));
            };
            context
                .files
                .iter()
                .filter(|file| {
                    stored_absolute(context.project_root, &file.path)
                        .and_then(|path| identities.resolve(&path).ok())
                        .is_some_and(|identity| identity == wanted)
                })
                .collect::<Vec<_>>()
        }
        None => {
            let mut files = context.files.iter().collect::<Vec<_>>();
            files.sort_by_key(|file| file.id);
            files
        }
    };
    if selected_files.is_empty() {
        return Ok(SelectorResolution::Missing);
    }
    let mut matches = BTreeSet::new();
    for file in selected_files {
        let Some(path) = file.path.to_str() else {
            return Ok(SelectorResolution::Unavailable(
                UnavailableReason::SourceNotBoundToPublication,
            ));
        };
        for kind in CALLABLE_KINDS {
            matches.extend(
                context
                    .store
                    .node_ids_by_file_identity_qualified_name_and_kind(path, qualified_name, kind)
                    .map_err(store_error)?,
            );
        }
    }
    resolve_unique_node(context, matches.into_iter().collect(), identities)
}

fn resolve_unique_node(
    context: &SelectorContext<'_>,
    mut matches: Vec<NodeId>,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    matches.sort();
    matches.dedup();
    match matches.as_slice() {
        [] => Ok(SelectorResolution::Missing),
        [node_id] => match context.store.get_node(*node_id).map_err(store_error)? {
            Some(node) => resolved_node(context, node, identities),
            None => Ok(SelectorResolution::Missing),
        },
        _ => Ok(SelectorResolution::Ambiguous),
    }
}

fn resolved_node(
    context: &SelectorContext<'_>,
    node: Node,
    identities: &mut OperationPathIdentityResolver,
) -> Result<SelectorResolution, ApiError> {
    if !is_callable(node.kind) {
        return Ok(SelectorResolution::NonCallable);
    }
    let Some(file_id) = node.file_node_id else {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    };
    let Some(file) = context.files.iter().find(|file| file.id == file_id.0) else {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    };
    let Some(absolute) = stored_absolute(context.project_root, &file.path) else {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    };
    if identities.resolve(&absolute).is_err() {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    }
    let Some(relative) = workspace_relative_path(context.project_root, &absolute) else {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    };
    let Some(components) = relative
        .iter()
        .map(|part| part.to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
    else {
        return Ok(SelectorResolution::Unavailable(
            UnavailableReason::SourceNotBoundToPublication,
        ));
    };
    Ok(SelectorResolution::Resolved(ResolvedNodeIdentity {
        pinned: PinnedNodeIdentity {
            project_id: context.project_id.to_owned(),
            core_generation_id: context.publication.generation_id.clone(),
            core_run_id: context.publication.run_id.clone(),
            node_id: node.id.0.to_string(),
        },
        canonical_id: node
            .canonical_id
            .unwrap_or_else(|| format!("node:{}", node.id.0)),
        qualified_name: node.qualified_name.unwrap_or(node.serialized_name),
        project_file_components: components,
    }))
}

fn authenticate_containment(
    store: &Store,
    selected_source: &Node,
    file_node_id: NodeId,
    line: u32,
) -> Result<Option<CallableContainmentEvidence>, ApiError> {
    let projections = store
        .get_callable_projection_states_for_file(file_node_id.0)
        .map_err(store_error)?;
    let ids = projections
        .iter()
        .map(|projection| projection.node_id)
        .collect::<Vec<_>>();
    let nodes = store.get_nodes_by_ids(&ids).map_err(store_error)?;
    let mut candidates = projections
        .into_iter()
        .filter_map(|projection| {
            let node = nodes.get(&projection.node_id)?;
            (is_callable(node.kind)
                && node.file_node_id == Some(file_node_id)
                && projection.file_id == file_node_id.0
                && projection.start_line <= line
                && line <= projection.end_line)
                .then_some(projection)
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|projection| {
        (
            projection.end_line.saturating_sub(projection.start_line),
            projection.node_id,
        )
    });
    let Some(smallest) = candidates.first() else {
        return Ok(None);
    };
    let smallest_span = smallest.end_line.saturating_sub(smallest.start_line);
    if candidates
        .iter()
        .take_while(|candidate| {
            candidate.end_line.saturating_sub(candidate.start_line) == smallest_span
        })
        .count()
        != 1
        || smallest.node_id != selected_source.id
    {
        return Ok(None);
    }
    Ok(Some(CallableContainmentEvidence {
        file_node_id,
        owner_node_id: smallest.node_id,
        start_line: smallest.start_line,
        end_line: smallest.end_line,
    }))
}

struct BoundSource {
    bytes: Vec<u8>,
    observed_sha256: String,
    project_file_components: Vec<String>,
}

enum SourceObservation {
    Bound(BoundSource),
    Unavailable,
    InvalidUtf8 { observed_sha256: String },
}

enum BindSourceError {
    Unavailable,
    InvalidUtf8,
}

fn bind_source_once<'a, R>(
    project_root: &Path,
    file: &FileInfo,
    indexed_hash: &str,
    identities: &mut OperationPathIdentityResolver,
    cache: &'a mut HashMap<WorkspacePathIdentity, SourceObservation>,
    read_source: &mut R,
) -> Result<&'a BoundSource, BindSourceError>
where
    R: FnMut(&Path) -> io::Result<Vec<u8>>,
{
    let absolute = stored_absolute(project_root, &file.path).ok_or(BindSourceError::Unavailable)?;
    let ProjectRelativePathResolution::Existing { absolute, relative } =
        resolve_project_relative_path(project_root, &absolute)
            .map_err(|_| BindSourceError::Unavailable)?
    else {
        return Err(BindSourceError::Unavailable);
    };
    let identity = identities
        .resolve(&absolute)
        .map_err(|_| BindSourceError::Unavailable)?;
    if !cache.contains_key(&identity) {
        let observation = match read_source(&absolute) {
            Err(_) => SourceObservation::Unavailable,
            Ok(bytes) => {
                let observed_sha256 = sha256_hex(&bytes);
                if std::str::from_utf8(&bytes).is_err() {
                    SourceObservation::InvalidUtf8 { observed_sha256 }
                } else {
                    let Some(project_file_components) = relative
                        .iter()
                        .map(|part| part.to_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                    else {
                        cache.insert(identity.clone(), SourceObservation::Unavailable);
                        return Err(BindSourceError::Unavailable);
                    };
                    SourceObservation::Bound(BoundSource {
                        bytes,
                        observed_sha256,
                        project_file_components,
                    })
                }
            }
        };
        cache.insert(identity.clone(), observation);
    }
    let Some(observation) = cache.get(&identity) else {
        return Err(BindSourceError::Unavailable);
    };
    match observation {
        SourceObservation::Bound(bound) if bound.observed_sha256 == indexed_hash => Ok(bound),
        SourceObservation::Bound(_) | SourceObservation::Unavailable => {
            Err(BindSourceError::Unavailable)
        }
        SourceObservation::InvalidUtf8 { observed_sha256 } if observed_sha256 != indexed_hash => {
            Err(BindSourceError::Unavailable)
        }
        SourceObservation::InvalidUtf8 { .. } => Err(BindSourceError::InvalidUtf8),
    }
}

fn complete_line(bytes: &[u8], requested_line: u32) -> Option<(usize, usize, String)> {
    if requested_line == 0 {
        return None;
    }
    let mut line = 1_u32;
    let mut start = 0_usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            if line == requested_line {
                let end = index + 1;
                return Some((
                    start,
                    end,
                    std::str::from_utf8(&bytes[start..end]).ok()?.to_owned(),
                ));
            }
            line += 1;
            start = index + 1;
        }
    }
    if line == requested_line && start < bytes.len() {
        return Some((
            start,
            bytes.len(),
            std::str::from_utf8(&bytes[start..]).ok()?.to_owned(),
        ));
    }
    None
}

fn receipt_id(
    project_id: &str,
    publication: &IndexPublicationRecord,
    admitted: &AdmittedRawCallEdge,
    source_id: i64,
    target_id: i64,
    indexed_hash: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(RECEIPT_DOMAIN);
    for part in [
        project_id.as_bytes(),
        publication.generation_id.as_bytes(),
        publication.run_id.as_bytes(),
        &admitted.edge_id.0.to_le_bytes(),
        &source_id.to_le_bytes(),
        &target_id.to_le_bytes(),
        &admitted.file_node_id.0.to_le_bytes(),
        indexed_hash.as_bytes(),
        &admitted.line.to_le_bytes(),
        admitted.callsite_identity.as_bytes(),
    ] {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    format!("indexed-call-edge:{:x}", digest.finalize())
}

fn stored_absolute(project_root: &Path, stored: &Path) -> Option<PathBuf> {
    let requested = if stored.is_absolute() {
        stored.to_path_buf()
    } else {
        project_root.join(stored)
    };
    match resolve_project_relative_path(project_root, &requested).ok()? {
        ProjectRelativePathResolution::Existing { absolute, .. } => Some(absolute),
        _ => None,
    }
}

fn parse_pinned_node_id(identity: &PinnedNodeIdentity) -> Option<NodeId> {
    let value = identity.node_id.parse::<i64>().ok()?;
    (value.to_string() == identity.node_id).then_some(NodeId(value))
}

fn is_callable(kind: NodeKind) -> bool {
    CALLABLE_KINDS.contains(&kind)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn store_error(error: codestory_store::StorageError) -> ApiError {
    ApiError::internal(format!("indexed call-path Store read failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    use codestory_agent::indexed_source_call_path_v1::{
        ClauseAnchor, ClauseClassification, ProofContractField, UnvalidatedCallPathContract,
        UnvalidatedCallPathSpec, UnvalidatedDirectCallStep, UnvalidatedExactSymbolSelector,
        ValidationOutcome, validate_contract,
    };
    use codestory_contracts::graph::{
        CallableProjectionState, Edge, EdgeId, EdgeKind, Node, ResolutionCertainty,
    };
    use codestory_store::{FileInfo, FileRole, IndexPublicationMode};
    use tempfile::TempDir;

    struct Fixture {
        _root: TempDir,
        root: PathBuf,
        store: Store,
        publication: IndexPublicationRecord,
        project_id: String,
        contract: ValidatedCallPathContract,
        source_path: PathBuf,
    }

    fn canonical(value: &str) -> UnvalidatedExactSymbolSelector {
        UnvalidatedExactSymbolSelector::CanonicalId(value.to_owned())
    }

    fn contract(start: &str, targets: &[&str]) -> ValidatedCallPathContract {
        let source = "exact direct ordered call path";
        let mut clauses = vec![ClauseAnchor {
            clause_id: "start".to_owned(),
            start: 0,
            end: source.len(),
            quote: source.to_owned(),
            classification: ClauseClassification::ResolvedMaterial {
                fields: vec![ProofContractField::Start],
            },
        }];
        for (index, _) in targets.iter().enumerate() {
            let step = u8::try_from(index).unwrap();
            clauses.push(ClauseAnchor {
                clause_id: format!("step-{index}"),
                start: 0,
                end: source.len(),
                quote: source.to_owned(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![
                        ProofContractField::StepTarget { step },
                        ProofContractField::Directness { step },
                        ProofContractField::Ordering { step },
                        ProofContractField::Relation { step },
                    ],
                },
            });
        }
        let input = UnvalidatedCallPathContract::new(
            source,
            clauses,
            UnvalidatedCallPathSpec {
                start: canonical(start),
                steps: targets
                    .iter()
                    .map(|target| UnvalidatedDirectCallStep {
                        target: canonical(target),
                    })
                    .collect(),
                prohibit_traversal_through: Vec::new(),
                exclude_from_projection: Vec::new(),
            },
        );
        match validate_contract(input).unwrap() {
            ValidationOutcome::Validated { contract, .. } => *contract,
            other => panic!("expected validated contract, got {other:?}"),
        }
    }

    fn node(
        id: i64,
        kind: NodeKind,
        name: &str,
        canonical_id: &str,
        file_id: i64,
        start_line: u32,
        end_line: u32,
    ) -> Node {
        Node {
            id: NodeId(id),
            kind,
            serialized_name: name.to_owned(),
            qualified_name: Some(format!("crate::{name}")),
            canonical_id: Some(canonical_id.to_owned()),
            file_node_id: Some(NodeId(file_id)),
            start_line: Some(start_line),
            start_col: Some(1),
            end_line: Some(end_line),
            end_col: Some(1),
        }
    }

    fn projection(
        file_id: i64,
        node_id: i64,
        start_line: u32,
        end_line: u32,
    ) -> CallableProjectionState {
        CallableProjectionState {
            file_id,
            symbol_key: format!("symbol-{node_id}"),
            node_id: NodeId(node_id),
            signature_hash: node_id,
            normalized_signature: Some(format!("signature-{node_id}")),
            body_hash: node_id,
            start_line,
            end_line,
        }
    }

    fn fixture(bytes: &[u8]) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let source_path = root.join("src/lib.rs");
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(&source_path, bytes).unwrap();
        let mut store = Store::new_in_memory().unwrap();
        let file = FileInfo {
            id: 1,
            path: source_path.clone(),
            language: "rust".to_owned(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: 2,
            file_role: FileRole::Source,
        };
        store.insert_file(&file).unwrap();
        store
            .update_file_metadata(&file, Some(&sha256_hex(bytes)))
            .unwrap();
        store
            .insert_node(&Node {
                id: NodeId(1),
                kind: NodeKind::FILE,
                serialized_name: source_path.to_string_lossy().into_owned(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: None,
                start_line: Some(1),
                start_col: Some(1),
                end_line: Some(2),
                end_col: Some(1),
            })
            .unwrap();
        store
            .insert_node(&node(2, NodeKind::FUNCTION, "source", "source-id", 1, 1, 1))
            .unwrap();
        store
            .insert_node(&node(3, NodeKind::FUNCTION, "target", "target-id", 1, 2, 2))
            .unwrap();
        store
            .insert_edge(&Edge {
                id: EdgeId(10),
                source: NodeId(2),
                target: NodeId(3),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(3)),
                confidence: Some(1.0),
                certainty: Some(ResolutionCertainty::Certain),
                callsite_identity: Some("1:1:0:3|rust".to_owned()),
                candidate_targets: Vec::new(),
            })
            .unwrap();
        store
            .upsert_callable_projection_states(&[
                projection(1, 1, 1, 2),
                projection(1, 2, 1, 1),
                projection(1, 3, 2, 2),
            ])
            .unwrap();
        let publication = IndexPublicationRecord {
            generation: 4,
            generation_id: "generation-4".to_owned(),
            run_id: "run-4".to_owned(),
            mode: IndexPublicationMode::Full,
            published_at_epoch_ms: 1,
        };
        let project_id = project_identity_v3(&root).project_id;
        Fixture {
            _root: temp,
            root,
            store,
            publication,
            project_id,
            contract: contract("source-id", &["target-id"]),
            source_path,
        }
    }

    #[test]
    fn builds_a_hash_bound_raw_edge_receipt_and_reads_source_once() {
        let fixture = fixture(b"fn source() { target(); }\r\nfn target() {}\n");
        fixture
            .store
            .insert_edge(&Edge {
                id: EdgeId(12),
                source: NodeId(2),
                target: NodeId(3),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(3)),
                confidence: Some(1.0),
                certainty: Some(ResolutionCertainty::Certain),
                callsite_identity: Some("1:1:1:3|rust".to_owned()),
                candidate_targets: Vec::new(),
            })
            .unwrap();
        fixture
            .store
            .insert_node(&Node {
                id: NodeId(6),
                kind: NodeKind::FILE,
                serialized_name: fixture.root.join("src/other.rs").display().to_string(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: None,
                start_line: Some(1),
                start_col: Some(1),
                end_line: Some(1),
                end_col: Some(1),
            })
            .unwrap();
        fixture
            .store
            .insert_edge(&Edge {
                id: EdgeId(13),
                source: NodeId(2),
                target: NodeId(3),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(6)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(3)),
                confidence: Some(1.0),
                certainty: Some(ResolutionCertainty::Certain),
                callsite_identity: Some("6:1:0:3|wrong-file".to_owned()),
                candidate_targets: Vec::new(),
            })
            .unwrap();
        let reads = Cell::new(0);
        let built = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &fixture.contract,
            |path| {
                reads.set(reads.get() + 1);
                fs::read(path)
            },
        )
        .unwrap();

        assert_eq!(reads.get(), 1);
        assert_eq!(built.facts.len(), 2);
        assert!(built.gaps.is_empty());
        assert!(built.unavailable.is_empty());
        assert_eq!(built.receipts.len(), 2);
        let receipt = &built.receipts[0];
        assert_eq!(receipt.receipt.edge_id, "10");
        assert_eq!(receipt.certainty, ResolutionCertainty::Certain);
        assert_eq!(receipt.callsite_identity, "1:1:0:3|rust");
        assert_eq!(receipt.containment.owner_node_id, NodeId(2));
        assert_eq!(receipt.line_window.kind, "indexed_line_v1");
        assert_eq!(receipt.line_window.text, "fn source() { target(); }\r\n");
        assert_eq!(receipt.line_window.byte_start, 0);
        assert_eq!(receipt.line_window.byte_end, 27);

        let repeated = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(repeated.receipts[0].receipt, receipt.receipt);
        assert_eq!(repeated.receipts, built.receipts);
    }

    #[test]
    fn source_binding_fails_closed_for_hash_and_utf8_drift() {
        let missing_fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        let file = missing_fixture.store.files().get_files().unwrap().remove(0);
        missing_fixture
            .store
            .update_file_metadata(&file, None)
            .unwrap();
        let reads = Cell::new(0);
        let missing_result = build_from_store(
            &missing_fixture.store,
            &missing_fixture.root,
            &missing_fixture.project_id,
            &missing_fixture.publication,
            &missing_fixture.contract,
            |path| {
                reads.set(reads.get() + 1);
                fs::read(path)
            },
        )
        .unwrap();
        assert_eq!(reads.get(), 0);
        assert_eq!(
            missing_result.unavailable,
            vec![UnavailableReason::SourceNotBoundToPublication]
        );

        let hash_fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        fs::write(
            &hash_fixture.source_path,
            b"fn source() { changed(); }\nfn target() {}\n",
        )
        .unwrap();
        let hash_result = build_from_store(
            &hash_fixture.store,
            &hash_fixture.root,
            &hash_fixture.project_id,
            &hash_fixture.publication,
            &hash_fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            hash_result.unavailable,
            vec![UnavailableReason::SourceNotBoundToPublication]
        );
        assert!(hash_result.facts.is_empty());

        let utf8_fixture = fixture(&[b'f', b'n', b' ', 0xff, b'\n']);
        let utf8_result = build_from_store(
            &utf8_fixture.store,
            &utf8_fixture.root,
            &utf8_fixture.project_id,
            &utf8_fixture.publication,
            &utf8_fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            utf8_result.gaps,
            vec![FactBuildGap::InvalidUtf8 { step_index: 0 }]
        );
        assert!(utf8_result.facts.is_empty());
    }

    #[test]
    fn hash_mismatch_precedes_invalid_utf8_and_is_cached_without_reread() {
        let fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        fixture
            .store
            .insert_edge(&Edge {
                id: EdgeId(15),
                source: NodeId(2),
                target: NodeId(3),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(3)),
                confidence: Some(1.0),
                certainty: Some(ResolutionCertainty::Certain),
                callsite_identity: Some("1:1:1:3|second-callsite".to_owned()),
                candidate_targets: Vec::new(),
            })
            .unwrap();
        fs::write(&fixture.source_path, [0xff, b'\n']).unwrap();
        let reads = Cell::new(0);

        let built = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &fixture.contract,
            |path| {
                reads.set(reads.get() + 1);
                fs::read(path)
            },
        )
        .unwrap();

        assert_eq!(reads.get(), 1);
        assert_eq!(
            built.unavailable,
            vec![UnavailableReason::SourceNotBoundToPublication]
        );
        assert!(built.gaps.is_empty());
        assert!(built.facts.is_empty());
    }

    #[test]
    fn containment_rejects_nested_and_equal_smallest_owners() {
        for nested in [true, false] {
            let mut fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
            let intruder_id = if nested { 4 } else { 5 };
            fixture
                .store
                .insert_node(&node(
                    intruder_id,
                    NodeKind::METHOD,
                    "inner",
                    &format!("inner-{intruder_id}"),
                    1,
                    1,
                    1,
                ))
                .unwrap();
            fixture
                .store
                .upsert_callable_projection_states(&[
                    if nested {
                        projection(1, 2, 1, 2)
                    } else {
                        projection(1, 2, 1, 1)
                    },
                    projection(1, intruder_id, 1, 1),
                ])
                .unwrap();
            let built = build_from_store(
                &fixture.store,
                &fixture.root,
                &fixture.project_id,
                &fixture.publication,
                &fixture.contract,
                |path| fs::read(path),
            )
            .unwrap();
            assert_eq!(
                built.gaps,
                vec![FactBuildGap::EdgeContainmentUnproven { step_index: 0 }]
            );
            assert!(built.facts.is_empty());
        }
    }

    #[test]
    fn line_windows_preserve_lf_crlf_last_line_and_exact_cap() {
        assert_eq!(
            complete_line(b"one\ntwo", 1),
            Some((0, 4, "one\n".to_owned()))
        );
        assert_eq!(
            complete_line(b"one\ntwo", 2),
            Some((4, 7, "two".to_owned()))
        );
        assert_eq!(
            complete_line(b"one\r\ntwo\r\n", 1),
            Some((0, 5, "one\r\n".to_owned()))
        );
        assert_eq!(complete_line(b"one\n", 3), None);
        assert_eq!(
            complete_line(&vec![b'a'; MAX_LINE_WINDOW_BYTES], 1)
                .unwrap()
                .1,
            MAX_LINE_WINDOW_BYTES
        );
        assert!(
            complete_line(&vec![b'a'; MAX_LINE_WINDOW_BYTES + 1], 1)
                .unwrap()
                .1
                > MAX_LINE_WINDOW_BYTES
        );

        let mut fixture = fixture(b"fn source() { target(); }\nfn target() {}");
        fixture
            .store
            .insert_node(&node(
                4,
                NodeKind::FUNCTION,
                "other_target",
                "other-target-id",
                1,
                2,
                2,
            ))
            .unwrap();
        fixture
            .store
            .insert_edge(&Edge {
                id: EdgeId(14),
                source: NodeId(2),
                target: NodeId(4),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(3),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(4)),
                confidence: Some(1.0),
                certainty: Some(ResolutionCertainty::Certain),
                callsite_identity: Some("1:3:0:4|out-of-range".to_owned()),
                candidate_targets: Vec::new(),
            })
            .unwrap();
        fixture
            .store
            .upsert_callable_projection_states(&[projection(1, 2, 1, 3)])
            .unwrap();
        let built = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &contract("source-id", &["other-target-id"]),
            |path| fs::read(path),
        )
        .unwrap();
        assert!(built.facts.is_empty());
        assert_eq!(
            built.gaps,
            vec![FactBuildGap::SourceLineOutOfRange { step_index: 0 }]
        );
    }

    #[test]
    fn builder_accepts_exactly_eight_kib_and_rejects_eight_kib_plus_one() {
        for (length, accepted) in [
            (MAX_LINE_WINDOW_BYTES, true),
            (MAX_LINE_WINDOW_BYTES + 1, false),
        ] {
            let fixture = fixture(&vec![b'a'; length]);
            let built = build_from_store(
                &fixture.store,
                &fixture.root,
                &fixture.project_id,
                &fixture.publication,
                &fixture.contract,
                |path| fs::read(path),
            )
            .unwrap();
            if accepted {
                assert_eq!(built.facts.len(), 1);
                assert!(built.gaps.is_empty());
            } else {
                assert!(built.facts.is_empty());
                assert_eq!(
                    built.gaps,
                    vec![FactBuildGap::SourceWindowTooLarge { step_index: 0 }]
                );
            }
        }
    }

    #[test]
    fn synthetic_self_edge_is_positive_but_an_ordinary_missing_edge_is_only_unknown() {
        let fixture = fixture(b"fn source() { source(); }\nfn target() {}\n");
        fixture
            .store
            .insert_edge(&Edge {
                id: EdgeId(11),
                source: NodeId(2),
                target: NodeId(2),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(1),
                resolved_source: Some(NodeId(2)),
                resolved_target: Some(NodeId(2)),
                confidence: Some(1.0),
                certainty: Some(ResolutionCertainty::Certain),
                callsite_identity: Some("1:1:0:2|synthetic".to_owned()),
                candidate_targets: Vec::new(),
            })
            .unwrap();
        let recursive = contract("source-id", &["source-id"]);
        let self_edge = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &recursive,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(self_edge.facts.len(), 1);
        assert!(self_edge.gaps.is_empty());

        let missing = contract("target-id", &["source-id"]);
        let absent = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &missing,
            |path| fs::read(path),
        )
        .unwrap();
        assert!(absent.facts.is_empty());
        assert_eq!(
            absent.gaps,
            vec![FactBuildGap::DirectCallMissing { step_index: 0 }]
        );
    }

    #[test]
    fn exact_selector_resolution_never_picks_first_or_prefix_matches() {
        let fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        fixture
            .store
            .insert_node(&node(
                4,
                NodeKind::METHOD,
                "duplicate",
                "source-id",
                1,
                1,
                1,
            ))
            .unwrap();
        fixture
            .store
            .insert_node(&node(
                5,
                NodeKind::STRUCT,
                "structural",
                "structural-id",
                1,
                1,
                1,
            ))
            .unwrap();
        let ambiguous = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &fixture.contract,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            ambiguous.gaps,
            vec![FactBuildGap::SelectorAmbiguous { selector_index: 0 }]
        );

        let missing = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &contract("source", &["target-id"]),
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            missing.gaps,
            vec![FactBuildGap::SelectorMissing { selector_index: 0 }]
        );

        let non_callable = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &contract("structural-id", &["target-id"]),
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            non_callable.gaps,
            vec![FactBuildGap::NonCallableSelector { selector_index: 0 }]
        );
        let non_callable_target = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &contract("target-id", &["structural-id"]),
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            non_callable_target.gaps,
            vec![FactBuildGap::NonCallableSelector { selector_index: 1 }]
        );
    }

    #[test]
    fn qualified_resolution_uses_exact_names_components_and_native_file_identity() {
        let fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        let files = fixture.store.files().get_files().unwrap();
        let context = SelectorContext {
            store: &fixture.store,
            project_root: &fixture.root,
            project_id: &fixture.project_id,
            publication: &fixture.publication,
            files: &files,
        };
        let exact_path = ["src".to_owned(), "lib.rs".to_owned()];
        let exact = resolve_qualified(
            &context,
            "crate::target",
            Some(&exact_path),
            &mut OperationPathIdentityResolver::native(),
        )
        .unwrap();
        assert!(matches!(
            exact,
            SelectorResolution::Resolved(node) if node.pinned.node_id == "3"
        ));

        for wrong_path in [
            vec!["src".to_owned(), "index".to_owned()],
            vec!["src".to_owned(), "indexer".to_owned()],
        ] {
            assert!(matches!(
                resolve_qualified(
                    &context,
                    "crate::target",
                    Some(&wrong_path),
                    &mut OperationPathIdentityResolver::native(),
                )
                .unwrap(),
                SelectorResolution::Unavailable(UnavailableReason::SourceNotBoundToPublication)
            ));
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&fixture.source_path, fixture.root.join("src/alias.rs"))
                .unwrap();
            let alias_path = ["src".to_owned(), "alias.rs".to_owned()];
            assert!(matches!(
                resolve_qualified(
                    &context,
                    "crate::target",
                    Some(&alias_path),
                    &mut OperationPathIdentityResolver::native(),
                )
                .unwrap(),
                SelectorResolution::Resolved(node) if node.pinned.node_id == "3"
            ));

            let outside = tempfile::tempdir().unwrap();
            fs::write(outside.path().join("outside.rs"), "fn outside() {}\n").unwrap();
            std::os::unix::fs::symlink(outside.path(), fixture.root.join("src/escape")).unwrap();
            let escape_path = [
                "src".to_owned(),
                "escape".to_owned(),
                "outside.rs".to_owned(),
            ];
            assert!(matches!(
                resolve_qualified(
                    &context,
                    "crate::target",
                    Some(&escape_path),
                    &mut OperationPathIdentityResolver::native(),
                )
                .unwrap(),
                SelectorResolution::Unavailable(UnavailableReason::SourceNotBoundToPublication)
            ));
        }

        fixture
            .store
            .insert_node(&node(
                4,
                NodeKind::METHOD,
                "source",
                "another-source-id",
                1,
                1,
                1,
            ))
            .unwrap();
        assert!(matches!(
            resolve_qualified(
                &context,
                "crate::source",
                None,
                &mut OperationPathIdentityResolver::native(),
            )
            .unwrap(),
            SelectorResolution::Ambiguous
        ));
    }

    #[test]
    fn pin_mismatch_and_missing_recursive_edges_are_typed_unknown_or_unavailable() {
        let fixture = fixture(b"fn source() { target(); }\nfn target() {}\n");
        let mut wrong_publication = fixture.publication.clone();
        wrong_publication.run_id = "other-run".to_owned();
        let pin = PinnedNodeIdentity {
            project_id: fixture.project_id.clone(),
            core_generation_id: fixture.publication.generation_id.clone(),
            core_run_id: fixture.publication.run_id.clone(),
            node_id: "2".to_owned(),
        };
        let files = fixture.store.files().get_files().unwrap();
        let context = SelectorContext {
            store: &fixture.store,
            project_root: &fixture.root,
            project_id: &fixture.project_id,
            publication: &wrong_publication,
            files: &files,
        };
        assert!(matches!(
            resolve_pinned(&context, &pin, &mut OperationPathIdentityResolver::native(),).unwrap(),
            SelectorResolution::Unavailable(UnavailableReason::PublicationPinMismatch)
        ));

        let recursive = contract("source-id", &["source-id"]);
        let built = build_from_store(
            &fixture.store,
            &fixture.root,
            &fixture.project_id,
            &fixture.publication,
            &recursive,
            |path| fs::read(path),
        )
        .unwrap();
        assert_eq!(
            built.gaps,
            vec![FactBuildGap::RecursiveCallNotRepresentable { step_index: 0 }]
        );
        assert!(built.facts.is_empty());
    }
}
