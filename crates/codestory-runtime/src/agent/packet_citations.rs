use crate::agent::packet_scoring::{normalize_identifier, packet_display_path};
use codestory_contracts::api::AgentCitationDto;

pub(crate) fn packet_citation_matching_display<'a>(
    citations: &'a [AgentCitationDto],
    display_needle: &str,
) -> Option<&'a AgentCitationDto> {
    let needle = normalize_identifier(display_needle);
    citations
        .iter()
        .find(|citation| normalize_identifier(&citation.display_name) == needle)
}

pub(crate) fn packet_citation_matching_display_contains<'a>(
    citations: &'a [AgentCitationDto],
    display_needle: &str,
) -> Option<&'a AgentCitationDto> {
    let needle = normalize_identifier(display_needle);
    citations
        .iter()
        .find(|citation| normalize_identifier(&citation.display_name).contains(&needle))
}

pub(crate) fn packet_citation_matching_path_and_display<'a>(
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

pub(crate) fn packet_command_crate_sources_contain_all(
    citations: &[AgentCitationDto],
    crate_segment: &str,
    groups: &[&[&str]],
) -> bool {
    let mut combined = String::new();
    for citation in citations
        .iter()
        .filter(|citation| packet_citation_path_contains_crate_segment(citation, crate_segment))
    {
        let Some(source) = packet_citation_source_text(citation) else {
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

pub(crate) fn packet_citation_path_contains_crate_segment(
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

pub(crate) fn packet_citation_source_text(citation: &AgentCitationDto) -> Option<String> {
    let path = citation.file_path.as_deref()?;
    std::fs::read_to_string(path).ok()
}
