use super::*;
use codestory_contracts::proof_resolution::{
    CallResolutionFact, CalleeForm, DependencyFileHash, EXACT_CALL_RESOLUTION_ALGORITHM,
    ExactCallsite, FileId, INTERNAL_RESOLUTION_PRODUCER, PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
    ProofResolutionAdapter, ProofResolutionFunnelRow, ProofResolutionProjection,
    ProofResolutionReason, ProofResolutionStatus, ResolutionEvidence, ResolutionProvenance,
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
        || fact.provenance.parser_fingerprint.trim().is_empty()
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
                || !fact.lookup_domain_complete
                || fact.evidence_chain.is_empty() =>
        {
            return Err(proof_error(
                "Exact requires target, edge, complete domain, and typed evidence",
            ));
        }
        ProofResolutionStatus::Exact => {}
        _ if fact.edge_id.is_some() => {
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
        let mut sql = "SELECT fact_id, edge_id, file_id, source_sha256, start_byte,
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
            let callee_form_text: String = row.get(8)?;
            let status_text: String = row.get(12)?;
            let reason_text: String = row.get(13)?;
            let evidence_json: String = row.get(14)?;
            let dependency_json: String = row.get(15)?;
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
                callsite: ExactCallsite {
                    file_id: FileId(row.get(2)?),
                    source_sha256: row.get(3)?,
                    start_byte: row
                        .get::<_, i64>(4)?
                        .try_into()
                        .map_err(|_| proof_error("stored callsite start byte is negative"))?,
                    end_byte_exclusive: row
                        .get::<_, i64>(5)?
                        .try_into()
                        .map_err(|_| proof_error("stored callsite end byte is negative"))?,
                    line: row
                        .get::<_, i64>(6)?
                        .try_into()
                        .map_err(|_| proof_error("stored callsite line is outside u32"))?,
                    column: row
                        .get::<_, i64>(7)?
                        .try_into()
                        .map_err(|_| proof_error("stored callsite column is outside u32"))?,
                    callee_form,
                    raw_target: row.get(9)?,
                },
                caller: NodeId(row.get(10)?),
                target: row.get::<_, Option<i64>>(11)?.map(NodeId),
                status,
                reason,
                evidence_chain,
                lookup_domain_complete: row.get::<_, i64>(16)? == 1,
                provenance: ResolutionProvenance {
                    producer: row.get(17)?,
                    fact_schema_version: row.get::<_, i64>(18)?.max(0) as u32,
                    algorithm: row.get(19)?,
                    language_adapter: row.get(20)?,
                    language_adapter_version: row.get(21)?,
                    parser_fingerprint: row.get(22)?,
                    dependency_file_hashes,
                    evidence_sha256: row.get(23)?,
                },
            });
        }
        Ok(facts)
    }

    fn validate_fact_against_graph(&self, fact: &CallResolutionFact) -> Result<(), StorageError> {
        validate_fact_seal(fact)?;
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
            return Ok(());
        }
        let edge_id = fact.edge_id.expect("shape validation requires exact edge");
        let target = fact.target.expect("shape validation requires exact target");
        let edge = self
            .get_edges()?
            .into_iter()
            .find(|edge| edge.id == edge_id)
            .ok_or_else(|| proof_error("matching ordinary CALL edge is missing"))?;
        if edge.kind != EdgeKind::CALL
            || edge.effective_source() != fact.caller
            || edge.effective_target() != target
            || edge.resolved_target != Some(target)
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
            .and_then(|identity| identity.split('|').next())
            .ok_or_else(|| proof_error("matching ordinary CALL edge has no exact callsite"))?;
        let mut fields = callsite.split(':');
        let parsed_file = fields.next().and_then(|value| value.parse::<i64>().ok());
        let parsed_line = fields.next().and_then(|value| value.parse::<u32>().ok());
        let parsed_column = fields.next().and_then(|value| value.parse::<u32>().ok());
        let parsed_raw_target = fields.next().and_then(|value| value.parse::<i64>().ok());
        if fields.next().is_some()
            || parsed_file != Some(fact.callsite.file_id.0)
            || parsed_line != Some(fact.callsite.line)
            || parsed_column != Some(fact.callsite.column)
            || parsed_raw_target != Some(edge.target.0)
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
        for fact in &facts {
            self.validate_fact_against_graph(fact)?;
        }
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
        let fact_digest = publication_fact_digest(&facts)?;
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
                    fact_id, edge_id, file_id, source_sha256, start_byte,
                    end_byte_exclusive, line, column, callee_form, raw_target,
                    caller_node_id, target_node_id, status, reason, evidence_json,
                    dependency_json, lookup_domain_complete, producer,
                    fact_schema_version, algorithm, language_adapter,
                    language_adapter_version, parser_fingerprint, evidence_digest
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24
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
        if manifest.fact_count != facts.len() as u64
            || manifest.fact_digest != publication_fact_digest(&facts)?
        {
            return Err(proof_error(
                "fact rows do not match their publication digest",
            ));
        }
        for fact in &facts {
            self.validate_fact_against_graph(fact)?;
        }
        Ok(manifest)
    }
}

fn publication_fact_digest(facts: &[CallResolutionFact]) -> Result<String, StorageError> {
    let mut hasher = Sha256::new();
    hasher.update(PUBLICATION_DIGEST_DOMAIN);
    for fact in facts {
        let bytes = serde_json::to_vec(fact)
            .map_err(|error| proof_error(format!("failed to serialize fact row: {error}")))?;
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
