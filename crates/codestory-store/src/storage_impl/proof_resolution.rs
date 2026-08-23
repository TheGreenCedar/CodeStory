use super::*;
use codestory_contracts::proof_resolution::{
    CallResolutionFact, CalleeForm, DependencyFileHash, EXACT_CALL_RESOLUTION_ALGORITHM,
    ExactCallsite, ExactCallsiteCorrelationFailure, ExactSyntaxCallsiteCorrelationInput, FileId,
    INTERNAL_RESOLUTION_PRODUCER, OrdinaryCallEdgeCorrelationInput,
    PROOF_RESOLUTION_FACT_SCHEMA_VERSION, ProofResolutionAdapter, ProofResolutionFunnelCounts,
    ProofResolutionFunnelRow, ProofResolutionProjection, ProofResolutionReason,
    ProofResolutionStatus, ResolutionEvidence, ResolutionEvidenceKind, ResolutionProvenance,
    correlate_exact_syntax_callsites,
};

const EVIDENCE_DIGEST_DOMAIN: &[u8] = b"codestory-proof-resolution-evidence-v1\0";
const FACT_ID_DOMAIN: &[u8] = b"codestory-proof-resolution-fact-id-v1\0";
const PUBLICATION_DIGEST_DOMAIN: &[u8] = b"codestory-proof-resolution-publication-v1\0";

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
    fact.provenance.dependency_file_hashes.sort();
    if fact
        .provenance
        .dependency_file_hashes
        .windows(2)
        .any(|pair| pair[0].file_id == pair[1].file_id)
    {
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

fn validate_fact_shape(fact: &CallResolutionFact, require_seal: bool) -> Result<(), StorageError> {
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
        || fact
            .provenance
            .dependency_file_hashes
            .windows(2)
            .any(|pair| pair[0].file_id >= pair[1].file_id)
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
                    let adapter_roster =
                        serde_json::from_str(&adapter_roster_json).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                adapter_roster_json.len(),
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?;
                    let funnel = serde_json::from_str(&funnel_json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            funnel_json.len(),
                            rusqlite::types::Type::Text,
                            Box::new(error),
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
        sql.push_str(" ORDER BY file_id, start_byte, end_byte_exclusive, fact_id");
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
            let evidence_chain: Vec<ResolutionEvidence> = serde_json::from_str(&evidence_json)
                .map_err(|error| {
                    proof_error(format!("stored evidence JSON is invalid: {error}"))
                })?;
            let dependency_file_hashes: Vec<DependencyFileHash> =
                serde_json::from_str(&dependency_json).map_err(|error| {
                    proof_error(format!("stored dependency JSON is invalid: {error}"))
                })?;
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
    ) -> Result<(), StorageError> {
        for fact in facts {
            validate_fact_seal(fact)?;
        }
        let graph_edges = self.get_edges()?;
        let nodes = self
            .get_nodes()?
            .into_iter()
            .map(|node| (node.id, node))
            .collect::<HashMap<_, _>>();
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
        let ordinary_edge_indices = graph_edges
            .iter()
            .enumerate()
            .filter_map(|(index, edge)| {
                (edge.kind == EdgeKind::CALL && nodes.contains_key(&edge.target)).then_some(index)
            })
            .collect::<Vec<_>>();
        let edge_inputs = ordinary_edge_indices
            .iter()
            .map(|index| {
                let edge = &graph_edges[*index];
                let raw = &nodes[&edge.target];
                OrdinaryCallEdgeCorrelationInput {
                    file_id: edge.file_node_id.map(|file| FileId(file.0)),
                    line: edge.line,
                    caller: edge.effective_source(),
                    target: edge.effective_target(),
                    raw_edge_target: edge.target,
                    raw_file_id: raw.file_node_id.map(|file| FileId(file.0)),
                    raw_line: raw.start_line,
                    raw_target: graph_leaf_name(&raw.serialized_name),
                    callsite_identity: edge.callsite_identity.as_deref(),
                    semantic_exact: edge.resolved_target == Some(edge.effective_target())
                        && edge.candidate_targets.is_empty(),
                }
            })
            .collect::<Vec<_>>();
        let correlations = correlate_exact_syntax_callsites(&syntax_inputs, &edge_inputs)
            .into_iter()
            .map(|result| {
                result.map(|edge_index| graph_edges[ordinary_edge_indices[edge_index]].id)
            })
            .collect::<Vec<_>>();
        let mut fact_correlations = vec![None; facts.len()];
        for (correlation_index, fact_index) in exact_fact_indices.iter().copied().enumerate() {
            fact_correlations[fact_index] = Some(correlations[correlation_index]);
        }
        for (fact_index, fact) in facts.iter().enumerate() {
            self.validate_fact_against_graph(fact, &graph_edges, fact_correlations[fact_index])?;
        }
        Ok(())
    }

    fn validate_fact_against_graph(
        &self,
        fact: &CallResolutionFact,
        graph_edges: &[Edge],
        correlation: Option<Result<EdgeId, ExactCallsiteCorrelationFailure>>,
    ) -> Result<(), StorageError> {
        let stored_source_hash = self
            .get_file_content_hash(fact.callsite.file_id.0)?
            .ok_or_else(|| proof_error("callsite file has no publication-bound source hash"))?;
        if stored_source_hash != fact.callsite.source_sha256 {
            return Err(proof_error(
                "callsite source hash does not match the graph file",
            ));
        }
        let caller = self
            .get_node(fact.caller)?
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

        let mut required_dependency_ids = BTreeSet::from([fact.callsite.file_id]);
        let mut evidence_node_ids = fact
            .evidence_chain
            .iter()
            .flat_map(ResolutionEvidence::node_ids)
            .collect::<Vec<_>>();
        if let Some(target) = fact.target {
            evidence_node_ids.push(target);
        }
        evidence_node_ids.sort_unstable();
        evidence_node_ids.dedup();
        for node_id in evidence_node_ids {
            let node = self
                .get_node(node_id)?
                .ok_or_else(|| proof_error("typed evidence references a missing graph node"))?;
            if let Some(file_id) = node.file_node_id {
                required_dependency_ids.insert(FileId(file_id.0));
            }
        }
        if dependency_file_ids(fact) != required_dependency_ids {
            return Err(proof_error(
                "dependency hashes do not exactly cover source, import, package, and target files",
            ));
        }
        for dependency in &fact.provenance.dependency_file_hashes {
            let dependency_file = self
                .get_files()?
                .into_iter()
                .find(|file| file.id == dependency.file_id.0)
                .ok_or_else(|| proof_error("dependency file record is missing"))?;
            if !dependency_file.indexed || !dependency_file.complete {
                return Err(proof_error(
                    "dependency file is not indexed-complete in the graph",
                ));
            }
            let stored = self
                .get_file_content_hash(dependency.file_id.0)?
                .ok_or_else(|| {
                    proof_error("dependency file has no publication-bound source hash")
                })?;
            if stored != dependency.source_sha256 {
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
        let target_node = self
            .get_node(target)?
            .ok_or_else(|| proof_error("exact target node is missing"))?;
        if target_node.file_node_id.is_none() {
            return Err(proof_error("exact target has no indexed dependency file"));
        }
        let raw_placeholder = self
            .get_node(raw_edge_target)?
            .ok_or_else(|| proof_error("raw CALL placeholder node is missing"))?;
        if raw_placeholder.file_node_id != Some(NodeId(fact.callsite.file_id.0))
            || raw_placeholder.start_line != Some(fact.callsite.line)
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
        match (fact.callsite.callee_form, fact.evidence_chain.as_slice()) {
            (CalleeForm::Identifier, [ResolutionEvidence::SameFileDeclaration { declaration }])
                if *declaration == target =>
            {
                let target_node = self
                    .get_node(target)?
                    .ok_or_else(|| proof_error("same-file declaration target is missing"))?;
                if target_node.file_node_id != Some(NodeId(fact.callsite.file_id.0)) {
                    return Err(proof_error(
                        "SameFileDeclaration is not the exact target in the source file",
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
                let import_node = self
                    .get_node(*import)?
                    .ok_or_else(|| proof_error("static import binding is missing"))?;
                if import_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                    || graph_leaf_name(&import_node.serialized_name) != fact.callsite.raw_target
                    || graph_edges
                        .iter()
                        .filter(|candidate| {
                            candidate.kind == EdgeKind::IMPORT
                                && candidate.file_node_id == Some(NodeId(fact.callsite.file_id.0))
                                && candidate.source == *import
                                && candidate.resolved_target == Some(target)
                        })
                        .count()
                        != 1
                {
                    return Err(proof_error(
                        "StaticImportBinding is not the unique source import bound to target",
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
                let owner_node = self
                    .get_node(*owner)?
                    .ok_or_else(|| proof_error("implicit receiver owner is missing"))?;
                let member = |member: NodeId| {
                    graph_edges
                        .iter()
                        .filter(|candidate| {
                            candidate.kind == EdgeKind::MEMBER
                                && candidate.effective_source() == *owner
                                && candidate.effective_target() == member
                        })
                        .count()
                        == 1
                };
                if !matches!(owner_node.kind, NodeKind::STRUCT | NodeKind::CLASS)
                    || owner_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                    || target_node.file_node_id != Some(NodeId(fact.callsite.file_id.0))
                    || !member(fact.caller)
                    || !member(target)
                {
                    return Err(proof_error(
                        "ImplicitReceiver does not own caller and target through inherent membership",
                    ));
                }
            }
            _ => {
                return Err(proof_error(
                    "typed evidence has no implemented exact semantic validator",
                ));
            }
        }
        let edge = graph_edges
            .iter()
            .find(|edge| edge.id == edge_id)
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
        let mut fields = callsite.split('|').next().unwrap_or_default().split(':');
        let parsed_file = fields.next().and_then(|value| value.parse::<i64>().ok());
        let parsed_line = fields.next().and_then(|value| value.parse::<u32>().ok());
        let parsed_discriminator = fields.next().and_then(|value| value.parse::<u32>().ok());
        let parsed_raw_target = fields.next().and_then(|value| value.parse::<i64>().ok());
        if fields.next().is_some()
            || parsed_file != Some(fact.callsite.file_id.0)
            || parsed_line != Some(fact.callsite.line)
            || parsed_discriminator.is_none()
            || parsed_raw_target != Some(raw_edge_target.0)
        {
            return Err(proof_error(
                "matching ordinary CALL edge has a different exact callsite identity",
            ));
        }
        Ok(())
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
        let mut facts = projection.facts.clone();
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
        self.validate_facts_against_graph(&facts)?;
        if facts.windows(2).any(|pair| {
            pair[0].callsite.file_id == pair[1].callsite.file_id
                && pair[0].callsite.start_byte == pair[1].callsite.start_byte
                && pair[0].callsite.end_byte_exclusive == pair[1].callsite.end_byte_exclusive
        }) {
            return Err(proof_error(
                "more than one fact owns the same exact callsite",
            ));
        }
        let mut exact_edges = BTreeSet::new();
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
        adapter_roster.dedup();
        if adapter_roster.is_empty()
            || adapter_roster.iter().any(|adapter| {
                adapter.language.trim().is_empty() || adapter.adapter_version.trim().is_empty()
            })
        {
            return Err(proof_error("adapter roster is empty or invalid"));
        }
        let mut funnel = projection.funnel.clone();
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
        self.validate_facts_against_graph(&facts)?;
        Ok(manifest)
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
        self.validate_proof_resolution_publication(previous)?;
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
        self.validate_proof_resolution_publication(next).map(Some)
    }
}

fn graph_leaf_name(name: &str) -> &str {
    name.rsplit(['.', ':'])
        .find(|part| !part.is_empty())
        .unwrap_or(name)
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
    for fact in facts {
        let evidence_kind = fact.evidence_chain.first().map(ResolutionEvidence::kind);
        let counts = rows
            .entry((
                fact.provenance.language_adapter.clone(),
                Some(fact.callsite.callee_form),
                evidence_kind,
            ))
            .or_default();
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
    result
}
