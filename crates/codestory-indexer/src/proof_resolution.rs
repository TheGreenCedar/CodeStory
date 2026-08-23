use crate::cache::{CachedCallResolutionInput, CachedIndexArtifact};
use crate::{
    callsite_identity_start_col, enclosing_callable_node_id, generate_edge_id_for_edge,
    index_feature_flags, source_content_hash,
};
use anyhow::{Context, Result, anyhow};
use codestory_contracts::graph::{Edge, EdgeKind, Node, NodeId, NodeKind};
use codestory_contracts::proof_resolution::{
    CallResolutionFact, CalleeForm, DependencyFileHash, EXACT_CALL_RESOLUTION_ALGORITHM,
    ExactCallsite, FileId, INTERNAL_RESOLUTION_PRODUCER, PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
    ProofResolutionAdapter, ProofResolutionFunnelCounts, ProofResolutionFunnelRow,
    ProofResolutionProjection, ProofResolutionReason, ProofResolutionStatus, ResolutionEvidence,
    ResolutionEvidenceKind, ResolutionProvenance,
};
use codestory_store::{IndexPublicationRecord, ProofResolutionPublication, Store};
use std::collections::{BTreeMap, HashMap, HashSet};
use tree_sitter::{Node as TsNode, Tree};

const ADAPTER_VERSION: &str = "reference-v1";

pub(crate) fn collect_call_resolution_inputs(
    tree: &Tree,
    source: &str,
    language: &str,
    file_id: NodeId,
    nodes: &[Node],
    edges: &mut [Edge],
) -> Vec<CachedCallResolutionInput> {
    if !matches!(language, "typescript" | "tsx" | "rust") {
        return Vec::new();
    }
    let node_map = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let callable_map = nodes
        .iter()
        .cloned()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let file_fallback = nodes
        .iter()
        .find(|node| node.kind == NodeKind::FILE)
        .map(|node| node.id)
        .unwrap_or(file_id);
    let source_sha256 = source_content_hash(source.as_bytes());
    let mut inputs = Vec::new();
    let mut assigned_edge_indexes = HashSet::new();
    collect_calls(tree.root_node(), source, &mut |callee, form, raw_target| {
        let line = callee.start_position().row as u32 + 1;
        let column = callee.start_position().column as u32 + 1;
        let mut matching_edge_indexes = edges
            .iter()
            .enumerate()
            .filter(|edge| {
                let edge = edge.1;
                edge.kind == EdgeKind::CALL
                    && edge.file_node_id == Some(file_id)
                    && edge.line == Some(line)
                    && node_map
                        .get(&edge.target)
                        .is_some_and(|node| short_name(&node.serialized_name) == raw_target)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        matching_edge_indexes.sort_by_key(|index| {
            (
                edges[*index]
                    .callsite_identity
                    .as_deref()
                    .and_then(callsite_identity_start_col)
                    .unwrap_or(u32::MAX),
                edges[*index].id,
            )
        });
        let selected_edge_index = matching_edge_indexes
            .into_iter()
            .find(|index| !assigned_edge_indexes.contains(index));
        if let Some(selected_edge_index) = selected_edge_index {
            assigned_edge_indexes.insert(selected_edge_index);
            let edge = &mut edges[selected_edge_index];
            let markers = edge
                .callsite_identity
                .as_deref()
                .into_iter()
                .flat_map(|identity| identity.split('|').skip(1))
                .collect::<Vec<_>>()
                .join("|");
            edge.callsite_identity = Some(if markers.is_empty() {
                format!("{}:{line}:{column}:{}", file_id.0, edge.target.0)
            } else {
                format!("{}:{line}:{column}:{}|{markers}", file_id.0, edge.target.0)
            });
            edge.id = codestory_contracts::graph::EdgeId(generate_edge_id_for_edge(
                edge,
                index_feature_flags(),
            ));
        }
        let matching_callers = selected_edge_index
            .into_iter()
            .map(|index| edges[index].effective_source())
            .collect::<HashSet<_>>();
        let caller = (matching_callers.len() == 1)
            .then(|| *matching_callers.iter().next().expect("one matching caller"))
            .or_else(|| enclosing_callable_node_id(&callable_map, line))
            .unwrap_or(file_fallback);
        inputs.push(CachedCallResolutionInput {
            callsite: ExactCallsite {
                file_id: FileId(file_id.0),
                source_sha256: source_sha256.clone(),
                start_byte: callee.start_byte() as u64,
                end_byte_exclusive: callee.end_byte() as u64,
                line,
                column,
                callee_form: form,
                raw_target,
            },
            caller,
            language: language.to_string(),
            adapter_version: ADAPTER_VERSION.to_string(),
            parser_fingerprint: format!("tree-sitter-{language}-grammar"),
        });
    });
    inputs.sort_by_key(|input| (input.callsite.start_byte, input.callsite.end_byte_exclusive));
    inputs
}

fn collect_calls(
    node: TsNode<'_>,
    source: &str,
    emit: &mut impl FnMut(TsNode<'_>, CalleeForm, String),
) {
    if node.kind() == "call_expression"
        && let Some(function) = node.child_by_field_name("function")
        && let Some((callee, form, raw_target)) = classify_callee(function, source)
    {
        emit(callee, form, raw_target);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_calls(child, source, emit);
    }
}

fn classify_callee<'tree>(
    function: TsNode<'tree>,
    source: &str,
) -> Option<(TsNode<'tree>, CalleeForm, String)> {
    let text = |node: TsNode<'tree>| node.utf8_text(source.as_bytes()).ok().map(str::to_string);
    match function.kind() {
        "identifier" | "type_identifier" => {
            Some((function, CalleeForm::Identifier, text(function)?))
        }
        "field_expression" => {
            let field = function.child_by_field_name("field")?;
            let receiver = function.child_by_field_name("value")?;
            let form = if text(receiver)?.trim() == "self" {
                CalleeForm::ImplicitReceiver
            } else {
                CalleeForm::ExplicitReceiver
            };
            Some((field, form, text(field)?))
        }
        "member_expression" => {
            let property = function.child_by_field_name("property")?;
            Some((property, CalleeForm::ExplicitReceiver, text(property)?))
        }
        "scoped_identifier" => {
            let name = function.child_by_field_name("name")?;
            Some((name, CalleeForm::QualifiedPath, text(name)?))
        }
        _ => {
            let mut cursor = function.walk();
            let leaf = function
                .named_children(&mut cursor)
                .last()
                .unwrap_or(function);
            Some((
                leaf,
                CalleeForm::DynamicAccess,
                text(leaf).unwrap_or_else(|| function.kind().to_string()),
            ))
        }
    }
}

pub fn rematerialize_proof_resolution_projection(
    store: &mut Store,
    publication: &IndexPublicationRecord,
) -> Result<ProofResolutionPublication> {
    let files = store.get_files()?;
    let file_by_id = files
        .iter()
        .map(|file| (file.id, file))
        .collect::<HashMap<_, _>>();
    let nodes = store.get_nodes()?;
    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let edges = store.get_edges()?;
    let mut inputs = Vec::new();
    for entry in store.get_index_artifact_cache_entries()? {
        let artifact: CachedIndexArtifact = serde_json::from_slice(&entry.artifact_blob)
            .with_context(|| {
                format!(
                    "proof resolution parser cache is incompatible for {}",
                    entry.file_path.display()
                )
            })?;
        if artifact.resolution_input_schema_version != 1 {
            return Err(anyhow!(
                "proof resolution parser cache has no schema-v1 call inputs for {}",
                entry.file_path.display()
            ));
        }
        for input in artifact.call_resolution_inputs {
            let Some(file) = file_by_id.get(&input.callsite.file_id.0) else {
                continue;
            };
            if file.indexed
                && store.get_file_content_hash(file.id)?.as_deref()
                    == Some(input.callsite.source_sha256.as_str())
            {
                inputs.push(input);
            }
        }
    }
    inputs.sort_by(|left, right| {
        left.callsite
            .file_id
            .cmp(&right.callsite.file_id)
            .then(left.callsite.start_byte.cmp(&right.callsite.start_byte))
            .then(
                left.callsite
                    .end_byte_exclusive
                    .cmp(&right.callsite.end_byte_exclusive),
            )
    });
    if inputs.windows(2).any(|pair| {
        pair[0].callsite.file_id == pair[1].callsite.file_id
            && pair[0].callsite.start_byte == pair[1].callsite.start_byte
            && pair[0].callsite.end_byte_exclusive == pair[1].callsite.end_byte_exclusive
    }) {
        return Err(anyhow!(
            "proof resolution projection has duplicate exact callsites"
        ));
    }

    let mut facts = Vec::with_capacity(inputs.len());
    let mut adapters = file_by_id
        .values()
        .filter(|file| matches!(file.language.as_str(), "typescript" | "tsx" | "rust"))
        .map(|file| (file.language.clone(), ADAPTER_VERSION.to_string()))
        .collect::<BTreeMap<_, _>>();
    for input in inputs {
        adapters.insert(input.language.clone(), input.adapter_version.clone());
        facts.push(resolve_input(
            store,
            &file_by_id,
            &node_by_id,
            &edges,
            input,
        )?);
    }
    let funnel = build_funnel(&facts);
    let projection = ProofResolutionProjection {
        adapter_roster: adapters
            .into_iter()
            .map(|(language, adapter_version)| ProofResolutionAdapter {
                language,
                adapter_version,
            })
            .collect(),
        facts,
        funnel,
    };
    store
        .replace_proof_resolution_projection(publication, &projection)
        .map_err(Into::into)
}

fn resolve_input(
    store: &Store,
    files: &HashMap<i64, &codestory_store::FileInfo>,
    nodes: &HashMap<NodeId, &Node>,
    edges: &[Edge],
    mut input: CachedCallResolutionInput,
) -> Result<CallResolutionFact> {
    let file = files
        .get(&input.callsite.file_id.0)
        .ok_or_else(|| anyhow!("proof callsite file is missing"))?;
    let matching_edges = edges
        .iter()
        .filter(|edge| {
            edge.kind == EdgeKind::CALL
                && edge.file_node_id == Some(NodeId(input.callsite.file_id.0))
                && edge.line == Some(input.callsite.line)
                && edge
                    .callsite_identity
                    .as_deref()
                    .and_then(callsite_identity_start_col)
                    == Some(input.callsite.column)
                && edge.effective_source() == input.caller
                && nodes.get(&edge.target).is_some_and(|node| {
                    short_name(&node.serialized_name) == input.callsite.raw_target
                })
        })
        .collect::<Vec<_>>();

    let supported = matches!(
        input.callsite.callee_form,
        CalleeForm::Identifier | CalleeForm::ImplicitReceiver
    );
    let same_file_bindings = nodes
        .values()
        .filter(|node| {
            node.file_node_id == Some(NodeId(input.callsite.file_id.0))
                && matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                && short_name(&node.serialized_name) == input.callsite.raw_target
        })
        .count();
    let (mut status, mut reason, mut edge, mut target) = if !file.complete {
        (
            ProofResolutionStatus::IncompleteDomain,
            ProofResolutionReason::LookupDomainIncomplete,
            None,
            None,
        )
    } else if !supported {
        (
            ProofResolutionStatus::Unsupported,
            ProofResolutionReason::UnsupportedConstruct,
            None,
            None,
        )
    } else if same_file_bindings > 1
        || matching_edges.len() > 1
        || matching_edges
            .first()
            .is_some_and(|edge| !edge.candidate_targets.is_empty())
    {
        (
            ProofResolutionStatus::Ambiguous,
            ProofResolutionReason::MultipleBindings,
            None,
            None,
        )
    } else if let Some(edge) = matching_edges.first().copied() {
        let target = edge.effective_target();
        if edge.resolved_target.is_some()
            && nodes
                .get(&target)
                .is_some_and(|node| matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD))
        {
            (
                ProofResolutionStatus::Exact,
                ProofResolutionReason::ExactResolution,
                Some(edge),
                Some(target),
            )
        } else {
            (
                ProofResolutionStatus::MissingBinding,
                ProofResolutionReason::MissingBinding,
                None,
                None,
            )
        }
    } else {
        (
            ProofResolutionStatus::MissingBinding,
            ProofResolutionReason::MissingBinding,
            None,
            None,
        )
    };

    let mut evidence_chain = Vec::new();
    if let (Some(exact_edge), Some(exact_target)) = (edge, target) {
        let target_file = nodes.get(&exact_target).and_then(|node| node.file_node_id);
        if target_file == Some(NodeId(input.callsite.file_id.0)) {
            if input.callsite.callee_form == CalleeForm::ImplicitReceiver {
                if let Some(owner) = inherent_owner(edges, nodes, input.caller, input.callsite.line)
                {
                    evidence_chain.push(ResolutionEvidence::ImplicitReceiver { owner });
                } else {
                    status = ProofResolutionStatus::MissingBinding;
                    reason = ProofResolutionReason::MissingBinding;
                    edge = None;
                    target = None;
                }
            }
            if status == ProofResolutionStatus::Exact {
                evidence_chain.push(ResolutionEvidence::SameFileDeclaration {
                    declaration: exact_target,
                });
            }
        } else {
            input.callsite.callee_form = CalleeForm::NamedImport;
            if let Some(import) = static_import_node(
                edges,
                nodes,
                NodeId(input.callsite.file_id.0),
                exact_edge,
                exact_target,
                &input.callsite.raw_target,
            ) {
                evidence_chain.push(ResolutionEvidence::StaticImportBinding {
                    import,
                    declaration: exact_target,
                });
            } else {
                status = ProofResolutionStatus::MissingBinding;
                reason = ProofResolutionReason::MissingBinding;
                edge = None;
                target = None;
            }
        }
    }

    let mut dependency_ids = HashSet::from([NodeId(input.callsite.file_id.0)]);
    for node_id in evidence_chain
        .iter()
        .flat_map(ResolutionEvidence::node_ids)
        .chain(target)
    {
        if let Some(file_id) = nodes.get(&node_id).and_then(|node| node.file_node_id) {
            dependency_ids.insert(file_id);
        }
    }
    let mut dependency_file_hashes = dependency_ids
        .into_iter()
        .map(|file_id| {
            let source_sha256 = store
                .get_file_content_hash(file_id.0)?
                .ok_or_else(|| anyhow!("proof dependency file {} has no source hash", file_id.0))?;
            Ok(DependencyFileHash {
                file_id: FileId(file_id.0),
                source_sha256,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    dependency_file_hashes.sort();
    codestory_store::seal_call_resolution_fact(CallResolutionFact {
        fact_id: String::new(),
        edge_id: edge.map(|edge| edge.id),
        callsite: input.callsite,
        caller: input.caller,
        target,
        status,
        reason,
        evidence_chain,
        lookup_domain_complete: status != ProofResolutionStatus::IncompleteDomain,
        provenance: ResolutionProvenance {
            producer: INTERNAL_RESOLUTION_PRODUCER.to_string(),
            fact_schema_version: PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
            algorithm: EXACT_CALL_RESOLUTION_ALGORITHM.to_string(),
            language_adapter: input.language,
            language_adapter_version: input.adapter_version,
            parser_fingerprint: input.parser_fingerprint,
            dependency_file_hashes,
            evidence_sha256: String::new(),
        },
    })
    .map_err(Into::into)
}

fn inherent_owner(
    edges: &[Edge],
    nodes: &HashMap<NodeId, &Node>,
    caller: NodeId,
    line: u32,
) -> Option<NodeId> {
    edges
        .iter()
        .find(|edge| {
            edge.kind == EdgeKind::MEMBER
                && edge.effective_target() == caller
                && nodes
                    .get(&edge.effective_source())
                    .is_some_and(|node| matches!(node.kind, NodeKind::STRUCT | NodeKind::CLASS))
        })
        .map(Edge::effective_source)
        .or_else(|| {
            nodes
                .values()
                .filter(|node| {
                    matches!(node.kind, NodeKind::STRUCT | NodeKind::CLASS)
                        && node.start_line.is_some_and(|start| start <= line)
                        && node.end_line.is_some_and(|end| end >= line)
                })
                .min_by_key(|node| node.end_line.unwrap_or(line) - node.start_line.unwrap_or(line))
                .map(|node| node.id)
        })
}

fn static_import_node(
    edges: &[Edge],
    nodes: &HashMap<NodeId, &Node>,
    source_file: NodeId,
    call_edge: &Edge,
    target: NodeId,
    raw_target: &str,
) -> Option<NodeId> {
    if nodes
        .get(&call_edge.target)
        .is_some_and(|node| node.file_node_id == Some(source_file))
    {
        return Some(call_edge.target);
    }
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::IMPORT && edge.file_node_id == Some(source_file))
        .flat_map(|edge| [edge.source, edge.target])
        .find(|node_id| {
            *node_id != target
                && nodes.get(node_id).is_some_and(|node| {
                    node.file_node_id == Some(source_file)
                        && short_name(&node.serialized_name) == raw_target
                })
        })
}

fn build_funnel(facts: &[CallResolutionFact]) -> Vec<ProofResolutionFunnelRow> {
    let mut rows = BTreeMap::<
        (String, Option<CalleeForm>, Option<ResolutionEvidenceKind>),
        ProofResolutionFunnelCounts,
    >::new();
    for fact in facts {
        let evidence_kinds = if fact.evidence_chain.is_empty() {
            vec![None]
        } else {
            fact.evidence_chain
                .iter()
                .map(|evidence| Some(evidence.kind()))
                .collect()
        };
        for evidence_kind in evidence_kinds {
            let counts = rows
                .entry((
                    fact.provenance.language_adapter.clone(),
                    Some(fact.callsite.callee_form),
                    evidence_kind,
                ))
                .or_default();
            counts.syntax_calls += 1;
            counts.adapter_supported += u64::from(matches!(
                fact.callsite.callee_form,
                CalleeForm::Identifier | CalleeForm::NamedImport | CalleeForm::ImplicitReceiver
            ));
            match fact.status {
                ProofResolutionStatus::Exact => counts.exact += 1,
                ProofResolutionStatus::Ambiguous => counts.ambiguous += 1,
                ProofResolutionStatus::Unsupported => counts.unsupported += 1,
                ProofResolutionStatus::MissingBinding => counts.missing_binding += 1,
                ProofResolutionStatus::IncompleteDomain => counts.incomplete_domain += 1,
            }
            if fact.edge_id.is_some() {
                counts.exact_call_linked += 1;
                counts.proof_shape_admitted += 1;
                counts.authoritative_receipts += 1;
            }
        }
    }
    rows.into_iter()
        .map(
            |((language, callee_form, evidence_kind), counts)| ProofResolutionFunnelRow {
                language,
                callee_form,
                evidence_kind,
                counts,
            },
        )
        .collect()
}

fn short_name(name: &str) -> &str {
    name.rsplit(['.', ':'])
        .find(|part| !part.is_empty())
        .unwrap_or(name)
}
