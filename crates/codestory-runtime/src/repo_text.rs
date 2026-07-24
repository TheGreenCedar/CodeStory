use super::{
    ApiError, AppController, HashSet, Instant, NodeId, Path, RepoTextScanStatsDto, SearchHit,
    SearchMatchQualityDto, Storage, clamp_u128_to_u32, clamp_usize_to_u32,
    compare_search_hits_with_project_root, extract_symbol_search_terms, file_text_match_line,
    read_searchable_file_contents,
};
#[cfg(test)]
use super::{SearchRepoTextMode, looks_like_repo_text_query};
#[cfg(test)]
use crate::search_intent::repo_text_auto_fallback_reason;
use crate::search_intent::text_contains_query_term;

pub(super) const REPO_TEXT_SCAN_FILE_CAP: usize = 2_000;
pub(super) const REPO_TEXT_SCAN_BYTE_CAP: usize = 32 * 1024 * 1024;
pub(super) const REPO_TEXT_SCAN_TIME_CAP_MS: u128 = 500;
pub(super) const REPO_TEXT_MAX_FILE_BYTES: u64 = 1_000_000;
#[derive(Debug, Clone)]
pub(super) struct RepoTextScan {
    pub(super) hits: Vec<SearchHit>,
    #[cfg(test)]
    pub(super) stats: RepoTextScanStatsDto,
}

impl AppController {
    #[cfg(test)]
    #[allow(dead_code)]
    fn repo_text_enabled_for_mode(
        mode: SearchRepoTextMode,
        query: &str,
        indexed_hits: &[SearchHit],
    ) -> bool {
        match mode {
            SearchRepoTextMode::Auto => {
                looks_like_repo_text_query(query)
                    || repo_text_auto_fallback_reason(query, indexed_hits).is_some()
            }
            SearchRepoTextMode::On => true,
            SearchRepoTextMode::Off => false,
        }
    }

    pub(super) fn collect_repo_text_hits(
        storage: &Storage,
        project_root: Option<&Path>,
        query: &str,
        limit: usize,
        indexed_hit_ids: &HashSet<NodeId>,
    ) -> Result<RepoTextScan, ApiError> {
        let started_at = Instant::now();
        let mut stats = RepoTextScanStatsDto {
            scanned_file_count: 0,
            scanned_byte_count: 0,
            skipped_large_file_count: 0,
            file_cap: REPO_TEXT_SCAN_FILE_CAP as u32,
            byte_cap: REPO_TEXT_SCAN_BYTE_CAP as u32,
            time_cap_ms: REPO_TEXT_SCAN_TIME_CAP_MS as u32,
            duration_ms: 0,
            truncated: false,
            reason: None,
            action: None,
        };
        if query.trim().is_empty() || limit == 0 {
            return Ok(RepoTextScan {
                hits: Vec::new(),
                #[cfg(test)]
                stats,
            });
        }

        let mut hits = Vec::new();
        let mut seen = indexed_hit_ids.clone();
        let terms = extract_symbol_search_terms(query);
        let normalized_query = query.trim().to_ascii_lowercase();
        for file in storage
            .get_files_ordered_limit(REPO_TEXT_SCAN_FILE_CAP.saturating_add(1))
            .map_err(|e| ApiError::internal(format!("Failed to load files for text search: {e}")))?
        {
            if Self::repo_text_scan_should_stop(&mut stats, &started_at) {
                break;
            }

            let path_string = file.path.to_string_lossy().to_string();
            stats.scanned_file_count = stats.scanned_file_count.saturating_add(1);
            let Ok(metadata) = std::fs::metadata(&file.path) else {
                continue;
            };
            if metadata.len() > REPO_TEXT_MAX_FILE_BYTES {
                stats.skipped_large_file_count = stats.skipped_large_file_count.saturating_add(1);
                continue;
            }
            let projected_bytes =
                u64::from(stats.scanned_byte_count).saturating_add(metadata.len());
            if projected_bytes > REPO_TEXT_SCAN_BYTE_CAP as u64 {
                Self::mark_repo_text_scan_truncated(
                    &mut stats,
                    format!(
                        "repo-text scan stopped before reading more than {} bytes",
                        REPO_TEXT_SCAN_BYTE_CAP
                    ),
                );
                break;
            }
            let Some(contents) = read_searchable_file_contents(&path_string) else {
                continue;
            };
            if contents.len() as u64 > REPO_TEXT_MAX_FILE_BYTES {
                stats.skipped_large_file_count = stats.skipped_large_file_count.saturating_add(1);
                continue;
            }
            stats.scanned_byte_count = stats
                .scanned_byte_count
                .saturating_add(clamp_usize_to_u32(contents.len()));
            let Some(line) = Self::repo_text_match_line(&contents, &path_string, query, &terms)
            else {
                continue;
            };
            let node_id = NodeId::from(codestory_contracts::graph::NodeId(file.id));
            if !seen.insert(node_id.clone()) {
                continue;
            }

            let display_name =
                Self::repo_text_display_name(project_root, &file.path, path_string.as_str());
            let score = Self::repo_text_score(
                &contents,
                &path_string,
                &normalized_query,
                &terms,
                line,
                hits.len(),
            );
            hits.push(Self::repo_text_search_hit(
                node_id,
                display_name,
                path_string,
                line,
                score,
            ));
        }

        hits.sort_by(|left, right| {
            compare_search_hits_with_project_root(project_root, query, left, right)
        });
        hits.truncate(limit);
        stats.duration_ms = clamp_u128_to_u32(started_at.elapsed().as_millis());
        Ok(RepoTextScan {
            hits,
            #[cfg(test)]
            stats,
        })
    }

    pub(super) fn repo_text_scan_should_stop(
        stats: &mut RepoTextScanStatsDto,
        started_at: &Instant,
    ) -> bool {
        if (stats.scanned_file_count as usize) >= REPO_TEXT_SCAN_FILE_CAP {
            Self::mark_repo_text_scan_truncated(
                stats,
                format!(
                    "repo-text scan stopped after scanning {} files",
                    REPO_TEXT_SCAN_FILE_CAP
                ),
            );
            return true;
        }
        if started_at.elapsed().as_millis() > REPO_TEXT_SCAN_TIME_CAP_MS {
            Self::mark_repo_text_scan_truncated(
                stats,
                format!(
                    "repo-text scan stopped after {} ms",
                    REPO_TEXT_SCAN_TIME_CAP_MS
                ),
            );
            return true;
        }
        false
    }

    fn repo_text_display_name(
        project_root: Option<&Path>,
        file_path: &Path,
        fallback: &str,
    ) -> String {
        project_root
            .and_then(|root| file_path.strip_prefix(root).ok())
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .or_else(|| {
                file_path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| fallback.to_string())
    }

    fn repo_text_match_line(
        contents: &str,
        path: &str,
        query: &str,
        terms: &[String],
    ) -> Option<u32> {
        if let Some(line) = file_text_match_line(contents, query, terms) {
            return Some(line);
        }
        if terms.is_empty() {
            return None;
        }

        let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
        let mut distinct_hit_terms = HashSet::new();
        for (term_index, term) in terms.iter().enumerate() {
            if text_contains_query_term(&normalized_path, term) {
                distinct_hit_terms.insert(term_index);
            }
        }
        let path_has_term = !distinct_hit_terms.is_empty();

        let mut best_line = None;
        let mut best_score = 0usize;
        for (index, line) in contents.lines().enumerate() {
            let normalized_line = line.to_ascii_lowercase();
            let mut line_score = 0usize;
            for (term_index, term) in terms.iter().enumerate() {
                if text_contains_query_term(&normalized_line, term) {
                    distinct_hit_terms.insert(term_index);
                    line_score += 1;
                }
            }
            if line_score > best_score {
                best_score = line_score;
                best_line = Some((index + 1).min(u32::MAX as usize) as u32);
            }
        }

        let required_hits = if path_has_term { 2 } else { 3.min(terms.len()) };
        if distinct_hit_terms.len() < required_hits {
            return None;
        }

        best_line.or(Some(1))
    }

    fn repo_text_term_hits(text: &str, terms: &[String]) -> f32 {
        terms
            .iter()
            .filter(|term| text_contains_query_term(text, term))
            .count() as f32
    }

    fn repo_text_score(
        contents: &str,
        path: &str,
        normalized_query: &str,
        terms: &[String],
        line: u32,
        hit_index: usize,
    ) -> f32 {
        let normalized_contents = contents.to_ascii_lowercase();
        let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
        let line_text = contents
            .lines()
            .nth(line.saturating_sub(1) as usize)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let exact_line_match = !normalized_query.is_empty() && line_text.contains(normalized_query);
        let exact_file_match =
            !normalized_query.is_empty() && normalized_contents.contains(normalized_query);
        let path_term_hits = Self::repo_text_term_hits(&normalized_path, terms);
        let line_term_hits = Self::repo_text_term_hits(&line_text, terms);
        let file_term_hits = Self::repo_text_term_hits(&normalized_contents, terms);

        let mut score = 100.0;
        if exact_line_match {
            score += 220.0;
        } else if exact_file_match {
            score += 140.0;
        }
        score += path_term_hits * 35.0;
        score += line_term_hits * 28.0;
        score += file_term_hits * 6.0;
        score - (hit_index as f32 * 0.01)
    }

    fn repo_text_search_hit(
        node_id: NodeId,
        display_name: String,
        path_string: String,
        line: u32,
        score: f32,
    ) -> SearchHit {
        SearchHit {
            node_id,
            display_name,
            kind: codestory_contracts::api::NodeKind::FILE,
            file_path: Some(path_string),
            line: Some(line),
            score,
            origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
            match_quality: Some(SearchMatchQualityDto::RepoText),
            resolvable: false,
            evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::LexicalSource),
            evidence_producer: Some("repo_text_fallback".to_string()),
            resolution_status: Some(
                codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
            ),
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: Some(true),
            score_breakdown: None,
        }
    }

    fn mark_repo_text_scan_truncated(stats: &mut RepoTextScanStatsDto, reason: String) {
        stats.truncated = true;
        stats.reason = Some(reason);
        stats.action = Some(
            "Narrow the query or use indexed symbol search with repo_text=off for deterministic results."
                .to_string(),
        );
    }
}
