use crate::packet_scoring::{normalize_identifier, packet_display_path};
use crate::pinned_reader::PinnedReader;
use codestory_contracts::api::AgentCitationDto;

pub fn packet_citation_matching_display<'a>(
    citations: &'a [AgentCitationDto],
    display_needle: &str,
) -> Option<&'a AgentCitationDto> {
    let needle = normalize_identifier(display_needle);
    citations
        .iter()
        .find(|citation| normalize_identifier(&citation.display_name) == needle)
}

pub fn packet_citation_matching_path_and_display<'a>(
    citations: &'a [AgentCitationDto],
    path_needle: &str,
    display_needle: &str,
) -> Option<&'a AgentCitationDto> {
    let normalized_path_needle = normalize_identifier(path_needle);
    let normalized_display_needle = normalize_identifier(display_needle);
    citations.iter().find(|citation| {
        let path_match = citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .map(|path| normalize_identifier(&path).contains(&normalized_path_needle))
            .unwrap_or(false);
        path_match
            && normalize_identifier(&citation.display_name).contains(&normalized_display_needle)
    })
}

pub fn packet_command_crate_sources_contain_all(
    citations: &[AgentCitationDto],
    crate_segment: &str,
    groups: &[&[&str]],
) -> bool {
    let mut combined = String::new();
    for citation in citations
        .iter()
        .filter(|citation| packet_citation_path_contains_crate_segment(citation, crate_segment))
    {
        let Some(source) = packet_citation_source_text(citation, None) else {
            continue;
        };
        combined.push_str(&source.to_ascii_lowercase());
        combined.push('\n');
    }
    !combined.is_empty()
        && groups.iter().all(|terms| {
            terms
                .iter()
                .any(|term| combined.contains(&term.to_ascii_lowercase()))
        })
}

pub fn packet_citation_path_contains_crate_segment(
    citation: &AgentCitationDto,
    crate_segment: &str,
) -> bool {
    let crate_segment = normalize_identifier(crate_segment);
    if crate_segment.is_empty() {
        return false;
    }
    citation
        .file_path
        .as_deref()
        .map(|path| {
            let raw = path.trim_start_matches("\\\\?\\").replace('\\', "/");
            let display = packet_display_path(path).replace('\\', "/");
            format!("{raw}\n{display}").to_ascii_lowercase()
        })
        .map(|path| {
            let needle = format!("/{crate_segment}/src/");
            path.contains(&needle)
        })
        .unwrap_or(false)
}

pub fn packet_citation_source_text(
    citation: &AgentCitationDto,
    reader: Option<&dyn PinnedReader>,
) -> Option<String> {
    citation
        .source_excerpt
        .as_deref()
        .map(str::trim)
        .filter(|excerpt| !excerpt.is_empty())
        .map(str::to_string)
        .or_else(|| reader.and_then(|reader| reader.pinned_source_text(citation)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pinned_reader::PinnedReader;
    use codestory_contracts::api::{NodeId, NodeKind, SearchHitOrigin};

    struct SnapshotReader {
        snapshot: Option<String>,
    }

    impl PinnedReader for SnapshotReader {
        fn pinned_project_id(&self) -> Option<String> {
            None
        }

        fn pinned_core_generation_id(&self) -> Option<String> {
            None
        }

        fn pinned_retrieval_generation(&self) -> Option<String> {
            None
        }

        fn pinned_source_text(&self, _citation: &AgentCitationDto) -> Option<String> {
            self.snapshot.clone()
        }
    }

    fn citation(excerpt: Option<&str>, path: Option<&str>) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId("n".to_string()),
            display_name: "symbol".to_string(),
            kind: NodeKind::FUNCTION,
            file_path: path.map(str::to_string),
            line: Some(1),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            source_excerpt: excerpt.map(str::to_string),
        }
    }

    #[test]
    fn citation_source_prefers_pinned_excerpt_over_snapshot() {
        let citation = citation(Some("fn excerpt() {}"), Some("/tmp/does-not-exist.rs"));
        let reader = SnapshotReader {
            snapshot: Some("fn snapshot() {}".to_string()),
        };
        assert_eq!(
            packet_citation_source_text(&citation, Some(&reader)).as_deref(),
            Some("fn excerpt() {}")
        );
    }

    #[test]
    fn citation_source_uses_pinned_snapshot_when_excerpt_is_missing() {
        let citation = citation(None, Some("src/lib.rs"));
        let reader = SnapshotReader {
            snapshot: Some("fn snapshot() {}".to_string()),
        };
        assert_eq!(
            packet_citation_source_text(&citation, Some(&reader)).as_deref(),
            Some("fn snapshot() {}")
        );
    }

    #[test]
    fn citation_source_never_opens_the_live_filesystem() {
        let citation = citation(None, Some("/etc/hosts"));
        assert_eq!(packet_citation_source_text(&citation, None), None);
    }
}
