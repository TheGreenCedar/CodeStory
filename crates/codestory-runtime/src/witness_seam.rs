//! Frozen-descriptor Phase 1A experiment. Both arms retain the same (at most
//! sixteen) admissions and use one authenticated source snapshot. Only the
//! hydration address differs; missing precision remains a typed gap.

use crate::addressed_hydration::{AddressedHydrationGap, hydrate_addressed_range};
use crate::agent::packet_compiler::{COMPILER_SOURCE_TRUNCATION_SUFFIX, hydrated_source};
use crate::snippets::{
    bounded_markdown_snippet_from_reader, bounded_markdown_snippet_range_from_reader,
};
use anyhow::{Context, Result, bail, ensure};
use codestory_agent::evidence_compiler::{
    RepositoryDerivedCompilationV1, compile_repository_evidence,
};
use codestory_contracts::compilation::{
    PACKET_COMPILATION_CONTRACT_VERSION_V1, PACKET_RETRIEVAL_SCORE_VERSION_V1,
    PacketAdmissionGapKindV1, PacketAdmissionGapV1, PacketAdmissionOriginV1,
    PacketAdmissionReceiptV1, PacketCompilationInputV1, PacketCompilationPublicationV1,
    PacketParserCompletenessV1,
};
use codestory_contracts::evidence_address::{
    ByteRangeV1, EvidenceAnchorV1, LineRangeV1, ProjectRelativePath, SourceRangeV1, StableNodeId,
};
use codestory_contracts::graph::{NodeId, NodeKind};
use codestory_contracts::packet_projection_v3::Sha256DigestV3Dto;
use codestory_retrieval::benchmark_support::WitnessLexicalPin;
use codestory_store::CoreReadSession;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessSeamDescriptor {
    pub admission: PacketAdmissionReceiptV1,
    pub path: Option<ProjectRelativePath>,
    pub symbol: Option<String>,
    pub anchor: Option<EvidenceAnchorV1>,
    pub content_digest: Option<Sha256DigestV3Dto>,
}

pub struct WitnessSeamPair {
    pub descriptors_sha256: String,
    pub control_input: PacketCompilationInputV1,
    pub addressed_input: PacketCompilationInputV1,
    pub control: RepositoryDerivedCompilationV1,
    pub addressed: RepositoryDerivedCompilationV1,
}

/// Preserve the existing lexical identities and order, including candidates
/// without a source address. Neither dropping nor padding may repair underfill.
pub fn freeze_witness_descriptors(
    pin: &CoreReadSession,
    lexical: &WitnessLexicalPin,
    project_root: &Path,
    hits: &[codestory_retrieval::CandidateHit],
) -> Result<Vec<WitnessSeamDescriptor>> {
    use codestory_contracts::api::SearchTargetDto;
    ensure!(
        hits.len() <= 16,
        "Phase 1A candidate universe exceeds sixteen"
    );
    ensure!(
        lexical.generation() == pin.identity().generation_id,
        "lexical/core pin mismatch"
    );
    let root = project_root.canonicalize()?;
    let mut descriptors = Vec::new();
    for (ordinal, hit) in hits.iter().enumerate() {
        ensure!(
            hit.source == codestory_retrieval::CandidateSource::Lexical,
            "Phase 1A permits only lexical retrieval"
        );
        let hit_path = Path::new(&hit.file_path);
        let relative = if hit_path.is_absolute() {
            hit_path.strip_prefix(&root).ok()
        } else {
            Some(hit_path)
        };
        let path = relative
            .and_then(Path::to_str)
            .and_then(|path| ProjectRelativePath::new(path).ok());
        let mut descriptor = WitnessSeamDescriptor {
            admission: PacketAdmissionReceiptV1 {
                packet_ordinal: ordinal as u32,
                stable_identity: hit
                    .packet_stable_identity()
                    .context("hit lacks stable identity")?,
                score_version: PACKET_RETRIEVAL_SCORE_VERSION_V1.into(),
                reserved_source_bytes: 512,
                origin: PacketAdmissionOriginV1::Retrieval,
            },
            path: path.clone(),
            symbol: hit
                .qualified_name
                .clone()
                .or_else(|| hit.symbol_name.clone()),
            anchor: path.clone().map(|path| EvidenceAnchorV1::PathOnly { path }),
            content_digest: None,
        };
        // A namespace may have only a diagnostic URI. It still occupies its
        // original admission; the URI never becomes a source path.
        let Some(path) = path else {
            descriptors.push(descriptor);
            continue;
        };
        if hit.node_id.is_none() {
            descriptor.admission.stable_identity = format!("path:{}", path.as_str());
        }
        let file = core_file(pin, &root, &path)?;
        let node = hit
            .node_id
            .as_ref()
            .map(|id| -> Result<_> {
                pin.storage()
                    .get_node(NodeId(id.parse()?))?
                    .context("retrieved node missing")
            })
            .transpose()?;
        if node
            .as_ref()
            .is_some_and(|node| node.kind == NodeKind::FILE || node.file_node_id.is_none())
        {
            descriptors.push(descriptor);
            continue;
        }
        let hash = source_hash(pin, Some(lexical), &path, file.as_ref().map(|file| file.id))?;
        let Some(hash) = hash else {
            descriptors.push(descriptor);
            continue;
        };
        let digest =
            Sha256DigestV3Dto::new(hash).map_err(|_| anyhow::anyhow!("invalid source digest"))?;
        descriptor.content_digest = Some(digest.clone());
        let source = authenticated_source(&root, &path, &digest)?;
        if let Some(node) = node {
            let file = file.context("indexed node file missing from core pin")?;
            ensure!(
                node.file_node_id == Some(NodeId(file.id)),
                "retrieved node/file mismatch"
            );
            if let (Some(start), Some(end)) = (node.start_line, node.end_line)
                && let Some(source_range) = full_line_range(&source, &path, &digest, start, end)
            {
                descriptor.anchor = Some(EvidenceAnchorV1::IndexedNode {
                    node_id: StableNodeId::new(descriptor.admission.stable_identity.clone())?,
                    source_range,
                });
            }
        } else if let Some(SearchTargetDto::FileRange {
            file_path,
            start_byte,
            end_byte,
        }) = &hit.target
        {
            ensure!(
                file_path == &hit.file_path,
                "lexical match path differs from its candidate"
            );
            let start = *start_byte as usize;
            let end = *end_byte as usize;
            ensure!(
                start < end
                    && end <= source.len()
                    && source.is_char_boundary(start)
                    && source.is_char_boundary(end),
                "invalid lexical match offsets"
            );
            let first = source.as_bytes()[..start]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count() as u32
                + 1;
            let last = source.as_bytes()[..end - 1]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count() as u32
                + 1;
            ensure!(
                hit.start_line == Some(first),
                "lexical matched line differs from its offsets"
            );
            descriptor.anchor = Some(EvidenceAnchorV1::Match {
                byte_range: ByteRangeV1::new(*start_byte as u64, *end_byte as u64)?,
                line_range: LineRangeV1::new(first, last)?,
            });
        }
        descriptors.push(descriptor);
    }
    Ok(descriptors)
}

pub fn run_witness_seam(
    pin: &CoreReadSession,
    lexical: Option<&WitnessLexicalPin>,
    project_root: &Path,
    publication: &PacketCompilationPublicationV1,
    descriptors: &[WitnessSeamDescriptor],
) -> Result<WitnessSeamPair> {
    ensure!(
        publication.core_generation_id == pin.identity().generation_id
            && lexical.is_none_or(|lexical| lexical.generation() == publication.core_generation_id)
            && publication.retrieval_generation.as_deref()
                == lexical.map(WitnessLexicalPin::input_hash),
        "frozen descriptor publication differs from the core/lexical pin"
    );
    ensure!(
        !publication.project_id.is_empty(),
        "missing logical project identity"
    );
    ensure!(
        descriptors.len() <= 16,
        "Phase 1A candidate universe exceeds sixteen"
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
        if descriptor.anchor.is_none()
            || matches!(descriptor.anchor, Some(EvidenceAnchorV1::PathOnly { .. }))
        {
            for input in [&mut control_input, &mut addressed_input] {
                record_gap(
                    input,
                    descriptor,
                    PacketAdmissionGapKindV1::SourceUnavailable,
                );
            }
            continue;
        }
        let path = descriptor
            .path
            .as_ref()
            .context("address lacks source path")?;
        let digest = descriptor
            .content_digest
            .as_ref()
            .context("address lacks source digest")?;
        let file = core_file(pin, &root, path)?;
        let indexed_hash = source_hash(pin, lexical, path, file.as_ref().map(|file| file.id))?
            .context("source has no pinned content binding")?;
        ensure!(
            indexed_hash == digest.as_str(),
            "descriptor content is not the pinned content"
        );
        let source = match snapshots.entry(path.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(authenticated_source(&root, path, digest)?)
            }
        };
        let completeness = match &file {
            Some(file) if file.indexed && file.complete => PacketParserCompletenessV1::Complete,
            Some(file) if file.indexed => PacketParserCompletenessV1::Partial,
            _ => PacketParserCompletenessV1::Unknown,
        };
        let (focus, node_bounds) = match descriptor.anchor.as_ref().expect("checked above") {
            EvidenceAnchorV1::Match {
                byte_range,
                line_range,
            } => {
                ensure!(
                    descriptor.admission.stable_identity == format!("path:{}", path.as_str()),
                    "file identity differs from frozen admission"
                );
                (
                    SourceRangeV1 {
                        path: path.clone(),
                        byte_range: *byte_range,
                        line_range: *line_range,
                        content_digest: digest.clone(),
                    },
                    None,
                )
            }
            EvidenceAnchorV1::IndexedNode {
                node_id,
                source_range,
            } => {
                let file = file.as_ref().context("indexed node lacks its core file")?;
                let id = node_id
                    .as_str()
                    .strip_prefix("node:")
                    .context("expected node identity")?
                    .parse()?;
                let node = pin
                    .storage()
                    .get_node(NodeId(id))?
                    .context("node missing from pinned core")?;
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
                    source_range.path == *path
                        && source_range.content_digest == *digest
                        && source_range.line_range == lines,
                    "node source address mismatch"
                );
                (source_range.clone(), Some(lines))
            }
            EvidenceAnchorV1::RelationOccurrence { .. } => {
                bail!("graph evidence is disabled in Phase 1A")
            }
            EvidenceAnchorV1::PathOnly { .. } => unreachable!(),
        };
        let mut syntax = Vec::new();
        if let Some(file) = &file {
            for node in pin.storage().get_nodes_containing_source_lines(
                NodeId(file.id),
                focus.line_range.start(),
                focus.line_range.end(),
            )? {
                if node.kind != NodeKind::FILE
                    && node.file_node_id == Some(NodeId(file.id))
                    && let (Some(start), Some(end)) = (node.start_line, node.end_line)
                    && let Some(span) = full_line_range(source, path, digest, start, end)
                {
                    syntax.push(span);
                }
            }
        }
        if node_bounds.is_some() {
            syntax.push(focus.clone());
        }
        let addressed = match hydrate_addressed_range(source, &focus, &syntax, 512) {
            Ok(value) => Some(value.markdown),
            Err(AddressedHydrationGap::SourceBudgetExceeded) => {
                record_gap(
                    &mut addressed_input,
                    descriptor,
                    PacketAdmissionGapKindV1::SourceBudgetExceeded,
                );
                None
            }
            Err(gap) => bail!("addressed hydration integrity failed: {gap:?}"),
        };
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
            (&mut control_input, Some(control.markdown)),
            (&mut addressed_input, addressed),
        ] {
            if let Some(markdown) = markdown {
                let mut hydrated = hydrated_source(
                    &descriptor.admission,
                    path.as_str(),
                    descriptor.symbol.clone(),
                    &markdown,
                )
                .map_err(|gap| anyhow::anyhow!("compiler source conversion failed: {gap:?}"))?;
                hydrated.parser_completeness = completeness;
                input.sources.push(hydrated);
            }
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

fn record_gap(
    input: &mut PacketCompilationInputV1,
    descriptor: &WitnessSeamDescriptor,
    kind: PacketAdmissionGapKindV1,
) {
    input.admission_gaps.push(PacketAdmissionGapV1 {
        kind,
        stable_identity: Some(descriptor.admission.stable_identity.clone()),
        exact_selector_ordinal: None,
    });
}

fn core_file(
    pin: &CoreReadSession,
    root: &Path,
    path: &ProjectRelativePath,
) -> Result<Option<codestory_store::FileInfo>> {
    Ok(pin
        .storage()
        .get_file_by_path(&root.join(path.as_str()))?
        .or(pin.storage().get_file_by_path(Path::new(path.as_str()))?))
}

fn source_hash(
    pin: &CoreReadSession,
    lexical: Option<&WitnessLexicalPin>,
    path: &ProjectRelativePath,
    file_id: Option<i64>,
) -> Result<Option<String>> {
    let core = file_id
        .map(|id| pin.storage().get_file_content_hash(id))
        .transpose()?
        .flatten();
    let lexical = lexical.and_then(|pin| pin.source_hash(path.as_str()));
    if let (Some(core), Some(lexical)) = (&core, lexical) {
        ensure!(core == lexical, "core and lexical source bindings disagree");
    }
    Ok(core.or_else(|| lexical.map(str::to_owned)))
}

fn authenticated_source(
    root: &Path,
    path: &ProjectRelativePath,
    digest: &Sha256DigestV3Dto,
) -> Result<String> {
    let full_path = root.join(path.as_str()).canonicalize()?;
    ensure!(
        full_path.starts_with(root),
        "source escapes the selected repository"
    );
    let source = std::fs::read_to_string(full_path)?;
    ensure!(
        format!("{:x}", Sha256::digest(&source)) == digest.as_str(),
        "source changed since publication"
    );
    Ok(source)
}

fn full_line_range(
    source: &str,
    path: &ProjectRelativePath,
    digest: &Sha256DigestV3Dto,
    start: u32,
    end: u32,
) -> Option<SourceRangeV1> {
    let lines = LineRangeV1::new(start, end).ok()?;
    let mut offsets = vec![0usize];
    for line in source.split_inclusive('\n') {
        offsets.push(offsets.last()? + line.len());
    }
    Some(SourceRangeV1 {
        path: path.clone(),
        content_digest: digest.clone(),
        line_range: lines,
        byte_range: ByteRangeV1::new(
            *offsets.get(start as usize - 1)? as u64,
            *offsets.get(end as usize)? as u64,
        )
        .ok()?,
    })
}
