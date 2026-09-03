//! Frozen-descriptor Phase 1A experiment. No retrieval, graph expansion, public
//! routing, or compiler changes. Both arms use one authenticated byte snapshot.

use crate::addressed_hydration::hydrate_addressed_range;
use crate::agent::packet_compiler::{COMPILER_SOURCE_TRUNCATION_SUFFIX, hydrated_source};
use crate::snippets::{
    bounded_markdown_snippet_from_reader, bounded_markdown_snippet_range_from_reader,
};
use anyhow::{Context, Result, bail, ensure};
use codestory_agent::evidence_compiler::{
    RepositoryDerivedCompilationV1, compile_repository_evidence,
};
use codestory_contracts::compilation::{
    PACKET_COMPILATION_CONTRACT_VERSION_V1, PacketAdmissionReceiptV1, PacketCompilationInputV1,
    PacketCompilationPublicationV1,
};
use codestory_contracts::evidence_address::{
    ByteRangeV1, EvidenceAnchorV1, LineRangeV1, ProjectRelativePath, SourceRangeV1,
};
use codestory_contracts::graph::{NodeId, NodeKind};
use codestory_contracts::packet_projection_v3::Sha256DigestV3Dto;
use codestory_store::CoreReadSession;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::path::Path;

/// Phase 1A retains the current admission contract and adds only the address.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessSeamDescriptor {
    pub admission: PacketAdmissionReceiptV1,
    pub path: ProjectRelativePath,
    pub symbol: Option<String>,
    pub anchor: EvidenceAnchorV1,
    pub content_digest: Sha256DigestV3Dto,
}

pub struct WitnessSeamPair {
    pub descriptors_sha256: String,
    pub control_input: PacketCompilationInputV1,
    pub addressed_input: PacketCompilationInputV1,
    pub control: RepositoryDerivedCompilationV1,
    pub addressed: RepositoryDerivedCompilationV1,
}

pub fn run_witness_seam(
    pin: &CoreReadSession,
    project_root: &Path,
    publication: &PacketCompilationPublicationV1,
    descriptors: &[WitnessSeamDescriptor],
) -> Result<WitnessSeamPair> {
    ensure!(
        publication.core_generation_id == pin.identity().generation_id,
        "frozen descriptor publication differs from the core pin"
    );
    ensure!(
        !publication.project_id.is_empty(),
        "missing logical project identity"
    );
    ensure!(
        descriptors.len() == 16,
        "Phase 1A requires exactly sixteen frozen candidates"
    );
    let mut identities = BTreeSet::new();
    for (ordinal, descriptor) in descriptors.iter().enumerate() {
        ensure!(
            descriptor.admission.reserved_source_bytes == 512
                && descriptor.admission.packet_ordinal as usize == ordinal,
            "Phase 1A charge or order changed"
        );
        ensure!(
            identities.insert(&descriptor.admission.stable_identity),
            "duplicate frozen identity"
        );
    }
    let descriptors_sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(descriptors)?));
    let mut control_input = PacketCompilationInputV1 {
        contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
        publication: publication.clone(),
        admissions: descriptors
            .iter()
            .map(|descriptor| descriptor.admission.clone())
            .collect(),
        sources: Vec::new(),
        relations: Vec::new(),
        ambiguities: Vec::new(),
        admission_gaps: Vec::new(),
    };
    let mut addressed_input = control_input.clone();
    let root = project_root.canonicalize()?;
    let mut snapshots = BTreeMap::<ProjectRelativePath, String>::new();
    for descriptor in descriptors {
        let full_path = root.join(descriptor.path.as_str()).canonicalize()?;
        ensure!(
            full_path.starts_with(&root),
            "source escapes the selected repository"
        );
        let file = pin
            .storage()
            .get_file_by_path(&root.join(descriptor.path.as_str()))?
            .or(pin
                .storage()
                .get_file_by_path(Path::new(descriptor.path.as_str()))?)
            .context("frozen candidate file is absent from the pinned core")?;
        ensure!(
            file.indexed && file.complete,
            "frozen candidate has incomplete indexed source"
        );
        let indexed_digest = pin
            .storage()
            .get_file_content_hash(file.id)?
            .context("pinned core has no content digest")?;
        ensure!(
            indexed_digest.eq_ignore_ascii_case(descriptor.content_digest.as_str()),
            "descriptor content is not the pinned content"
        );
        let source = match snapshots.entry(descriptor.path.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(std::fs::read_to_string(&full_path)?)
            }
        };
        let (focus, node_bounds) = match &descriptor.anchor {
            EvidenceAnchorV1::Match {
                byte_range,
                line_range,
            } => {
                ensure!(
                    descriptor.admission.stable_identity
                        == format!("path:{}", descriptor.path.as_str())
                        || descriptor.admission.stable_identity == format!("node:{}", file.id),
                    "file identity differs from the frozen admission"
                );
                (
                    SourceRangeV1 {
                        path: descriptor.path.clone(),
                        byte_range: *byte_range,
                        line_range: *line_range,
                        content_digest: descriptor.content_digest.clone(),
                    },
                    None,
                )
            }
            EvidenceAnchorV1::IndexedNode {
                node_id,
                source_range,
            } => {
                let id = node_id
                    .as_str()
                    .strip_prefix("node:")
                    .context("expected publication-scoped node identity")?
                    .parse()?;
                let node = pin
                    .storage()
                    .get_node(NodeId(id))?
                    .context("node missing from the pinned core")?;
                ensure!(
                    node.file_node_id == Some(NodeId(file.id))
                        && descriptor.admission.stable_identity == node_id.as_str(),
                    "node/file identity mismatch"
                );
                let lines = LineRangeV1::new(
                    node.start_line.context("node start missing")?,
                    node.end_line.context("node end missing")?,
                )?;
                ensure!(
                    source_range.path == descriptor.path
                        && source_range.content_digest == descriptor.content_digest
                        && source_range.line_range == lines,
                    "node source address mismatch"
                );
                (source_range.clone(), Some(lines))
            }
            EvidenceAnchorV1::PathOnly { .. } => {
                bail!("path-only candidates cannot enter the paired source experiment")
            }
            EvidenceAnchorV1::RelationOccurrence { .. } => {
                bail!("graph evidence is disabled in Phase 1A")
            }
        };
        let mut syntax = Vec::new();
        for node in pin.storage().get_nodes_containing_source_lines(
            NodeId(file.id),
            focus.line_range.start(),
            focus.line_range.end(),
        )? {
            if node.kind != NodeKind::FILE && node.file_node_id == Some(NodeId(file.id)) {
                if let (Some(start), Some(end)) = (node.start_line, node.end_line) {
                    if let Some(span) = full_line_range(source, descriptor, start, end) {
                        syntax.push(span);
                    }
                }
            }
        }
        if node_bounds.is_some() {
            syntax.push(focus.clone());
        }
        let addressed = hydrate_addressed_range(source, &focus, &syntax, 512)
            .map_err(|gap| anyhow::anyhow!("addressed hydration failed: {gap:?}"))?;
        let control = if let Some(lines) = node_bounds {
            bounded_markdown_snippet_range_from_reader(
                Cursor::new(source.as_bytes()),
                lines.start(),
                lines.start(),
                lines.end(),
                0,
                512,
                COMPILER_SOURCE_TRUNCATION_SUFFIX,
            )?
        } else {
            bounded_markdown_snippet_from_reader(
                Cursor::new(source.as_bytes()),
                1,
                8,
                512,
                COMPILER_SOURCE_TRUNCATION_SUFFIX,
            )?
        };
        for (input, markdown) in [
            (&mut control_input, control.markdown),
            (&mut addressed_input, addressed.markdown),
        ] {
            input.sources.push(
                hydrated_source(
                    &descriptor.admission,
                    descriptor.path.as_str(),
                    descriptor.symbol.clone(),
                    &markdown,
                )
                .map_err(|gap| anyhow::anyhow!("compiler source conversion failed: {gap:?}"))?,
            );
        }
    }
    Ok(WitnessSeamPair {
        descriptors_sha256,
        control: compile_repository_evidence(&control_input),
        addressed: compile_repository_evidence(&addressed_input),
        control_input,
        addressed_input,
    })
}

fn full_line_range(
    source: &str,
    descriptor: &WitnessSeamDescriptor,
    start: u32,
    end: u32,
) -> Option<SourceRangeV1> {
    let lines = LineRangeV1::new(start, end).ok()?;
    let mut offsets = vec![0usize];
    for line in source.split_inclusive('\n') {
        offsets.push(offsets.last()? + line.len());
    }
    Some(SourceRangeV1 {
        path: descriptor.path.clone(),
        byte_range: ByteRangeV1::new(
            *offsets.get(start as usize - 1)? as u64,
            *offsets.get(end as usize)? as u64,
        )
        .ok()?,
        line_range: lines,
        content_digest: descriptor.content_digest.clone(),
    })
}
