//! Source-verified public spellings supplement search, never exact call identity.
use crate::symbol_query::{OrientationEvidence, compare_search_hits_with_project_root};
use crate::{ApiError, AppController, SearchHit, Storage};
use codestory_contracts::api::{
    SearchPublicExportEvidence, SearchPublicExportMatch, SearchVerificationTargetDto,
};
use std::collections::HashMap;
use std::path::Path;

impl AppController {
    pub(crate) fn admit_public_exports(
        &self,
        storage: &Storage,
        root: &Path,
        query: &str,
        hits: &mut Vec<SearchHit>,
    ) -> Result<SearchPublicExportEvidence, ApiError> {
        let projection = storage
            .public_export_aliases(query)
            .map_err(|error| ApiError::internal(format!("Public export projection: {error}")))?;
        let mut evidence = SearchPublicExportEvidence {
            incomplete: !projection.ready || !projection.complete,
            bounded: projection.bounded,
            matches: Vec::new(),
        };
        for binding in projection.rows {
            let Some(id) = binding.target_node_id else {
                continue;
            };
            let Some(export) = storage
                .get_file_by_id(binding.export_file_id.0)
                .map_err(|error| ApiError::internal(error.to_string()))?
            else {
                evidence.incomplete = true;
                continue;
            };
            let Some(export_path) = export.path.to_str() else {
                evidence.incomplete = true;
                continue;
            };
            let Some(path) = export.path.strip_prefix(root).ok().and_then(Path::to_str) else {
                evidence.incomplete = true;
                continue;
            };
            let mut hit = match hits
                .iter()
                .find(|hit| hit.node_id.0 == id.0.to_string())
                .cloned()
            {
                Some(hit) => hit,
                None => {
                    let Some((_, score)) =
                        crate::search::engine::search_core_symbol_names_with_scores(
                            &[(id, binding.public_name.clone())],
                            query,
                        )
                        .into_iter()
                        .next()
                    else {
                        continue;
                    };
                    match Self::build_search_hit(storage, &HashMap::new(), id, score)? {
                        Some(mut hit) => {
                            crate::agent::packet_evidence::decorate_lexical_search_hit_evidence(
                                &mut hit,
                            );
                            hit
                        }
                        None => {
                            evidence.incomplete = true;
                            continue;
                        }
                    }
                }
            };
            if hit.display_name.rsplit([':', '.']).next() == Some(binding.public_name.as_str()) {
                continue; // Canonical declaration matches retain their existing presentation.
            }
            let mut presentation = format!(
                "{} (exported as {} at {}:{}",
                hit.display_name, binding.public_name, path, binding.export_line
            );
            if let (Some(module), Some(line)) = (&binding.module_specifier, binding.import_line) {
                presentation.push_str(&format!("; named import {} at {}:{}", module, path, line));
            } else if let Some(module) = &binding.module_specifier {
                presentation.push_str(&format!("; named re-export from {module}"));
            }
            presentation.push(')');
            // Do not truncate export provenance into a misleading positive label.
            if presentation.len()
                > codestory_contracts::packet_projection_v3::SYMBOL_ID_MAX_BYTES_V3
            {
                evidence.bounded = true;
                continue;
            }
            let verification_target = SearchVerificationTargetDto {
                role: "public_export".into(),
                file_path: export_path.to_owned(),
                line: binding.export_line,
                display_name: binding.public_name.clone(),
                reason: presentation.clone(),
            };
            hit.verification_targets.push(verification_target.clone());
            if let Some(previous) = hits
                .iter_mut()
                .find(|previous| previous.node_id == hit.node_id)
            {
                *previous = hit.clone();
            } else {
                hits.push(hit.clone());
            }
            evidence.matches.push(SearchPublicExportMatch {
                node_id: hit.node_id,
                public_name: binding.public_name,
                presentation,
                verification_target,
            });
        }
        // Preserve every verified owner on a public spelling collision; never select
        // one as the unique export target.
        if evidence.matches.len() > 1 {
            evidence.incomplete = true;
        }
        Self::rank_public_exports(root, query, hits, &evidence);
        if hits.len() > 200 {
            hits.truncate(200);
            evidence.bounded = true;
        }
        Self::annotate_public_exports(query, hits, &evidence);
        Ok(evidence)
    }

    pub(crate) fn note_public_export_coverage(
        retrieval: &mut codestory_contracts::api::RetrievalStateDto,
        evidence: &SearchPublicExportEvidence,
    ) {
        if evidence.incomplete || evidence.bounded {
            let note = "Public export navigation is incomplete, unavailable, ambiguous or bounded; additional bindings may exist.";
            match &mut retrieval.fallback_message {
                Some(message) => {
                    message.push(' ');
                    message.push_str(note);
                }
                None => retrieval.fallback_message = Some(note.into()),
            }
        }
    }

    pub(crate) fn annotate_public_exports(
        query: &str,
        hits: &mut [SearchHit],
        evidence: &SearchPublicExportEvidence,
    ) {
        for hit in hits {
            if let Some(binding) = evidence
                .matches
                .iter()
                .find(|binding| binding.node_id == hit.node_id)
            {
                if !hit.verification_targets.iter().any(|target| {
                    target.role == binding.verification_target.role
                        && target.file_path == binding.verification_target.file_path
                        && target.line == binding.verification_target.line
                        && target.display_name == binding.verification_target.display_name
                }) {
                    hit.verification_targets
                        .push(binding.verification_target.clone());
                }
                hit.match_quality = Some(if query == binding.public_name {
                    codestory_contracts::api::SearchMatchQualityDto::Exact
                } else {
                    codestory_contracts::api::SearchMatchQualityDto::NormalizedExact
                });
            }
        }
    }

    pub(crate) fn rank_public_exports(
        root: &Path,
        query: &str,
        hits: &mut Vec<SearchHit>,
        evidence: &SearchPublicExportEvidence,
    ) {
        if evidence.matches.is_empty() {
            return;
        }
        let mut ranking = OrientationEvidence::default();
        ranking.public_exports.extend(
            evidence
                .matches
                .iter()
                .map(|value| (value.node_id.clone(), value.public_name.clone())),
        );
        let (mut aliases, mut ordinary): (Vec<_>, Vec<_>) = std::mem::take(hits)
            .into_iter()
            .partition(|hit| ranking.public_exports.contains_key(&hit.node_id));
        aliases.sort_by(|left, right| {
            compare_search_hits_with_project_root(Some(root), query, left, right, Some(&ranking))
        });
        for alias in aliases {
            let at = ordinary
                .iter()
                .position(|other| {
                    compare_search_hits_with_project_root(
                        Some(root),
                        query,
                        &alias,
                        other,
                        Some(&ranking),
                    )
                    .is_lt()
                })
                .unwrap_or(ordinary.len());
            ordinary.insert(at, alias);
        }
        // Existing full-retrieval candidates retain their relative RRF order.
        *hits = ordinary;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_export_merge_preserves_unrelated_full_candidate_order_and_identity() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("owner.ts"), "class _Painter {}\n").unwrap();
        let hit = |id: &str, name: &str, kind: &str, path: &str, score: f32| -> SearchHit {
            serde_json::from_value(serde_json::json!({"node_id":id,"display_name":name,"kind":kind,
                "file_path":root.path().join(path),"line":1,"score":score,"origin":"indexed_symbol","resolvable":true})).unwrap()
        };
        let a = hit("a", "painter", "FUNCTION", "callbacks.ts", 0.5);
        let b = hit("b", "painter", "FUNCTION", "callbacks2.ts", 0.9);
        let owner = hit("owner", "_Painter", "CLASS", "owner.ts", 0.1);
        // Deliberately preserve the producer's b-before-a order rather than
        // recomputing the full candidate list with the core comparator.
        let mut hits = vec![b.clone(), a.clone(), owner.clone()];
        let evidence = SearchPublicExportEvidence {
            matches: vec![SearchPublicExportMatch {
                node_id: owner.node_id.clone(),
                public_name: "Painter".into(),
                presentation: "_Painter (exported as Painter at entry.ts:2)".into(),
                verification_target: SearchVerificationTargetDto {
                    role: "public_export".into(),
                    file_path: "entry.ts".into(),
                    line: 2,
                    display_name: "Painter".into(),
                    reason: "explicit named export".into(),
                },
            }],
            incomplete: false,
            bounded: false,
        };
        AppController::rank_public_exports(root.path(), "Painter", &mut hits, &evidence);
        assert_eq!(
            hits.iter().map(|hit| &hit.node_id).collect::<Vec<_>>(),
            vec![&owner.node_id, &b.node_id, &a.node_id]
        );
        assert_eq!(hits[0].display_name, "_Painter");
        assert_eq!(hits[0].file_path, owner.file_path);
        AppController::annotate_public_exports("Painter", &mut hits, &evidence);
        assert_eq!(
            hits[0].match_quality,
            Some(codestory_contracts::api::SearchMatchQualityDto::Exact)
        );
        let mut canonical = vec![b.clone(), a.clone()];
        AppController::rank_public_exports(
            root.path(),
            "Painter",
            &mut canonical,
            &Default::default(),
        );
        assert_eq!(
            canonical.iter().map(|hit| &hit.node_id).collect::<Vec<_>>(),
            vec![&b.node_id, &a.node_id]
        );
    }
}
