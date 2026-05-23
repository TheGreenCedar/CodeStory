use anyhow::{Result, bail};
use codestory_contracts::api::{NodeKind, SearchHit};
use std::collections::HashSet;

use crate::query_resolution::{compare_resolution_hits, is_resolvable_graph_target};

pub(crate) fn normalized_drill_anchors(anchors: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    anchors
        .iter()
        .flat_map(|anchor| anchor.split(','))
        .map(str::trim)
        .filter(|anchor| !anchor.is_empty())
        .filter_map(|anchor| {
            let owned = anchor.to_string();
            seen.insert(owned.clone()).then_some(owned)
        })
        .collect()
}

pub(crate) fn validated_drill_anchors(anchors: &[String], context: &str) -> Result<Vec<String>> {
    let anchors = normalized_drill_anchors(anchors);
    if anchors.is_empty() {
        bail!("{context} must name at least one anchor");
    }
    Ok(anchors)
}

pub(crate) fn choose_drill_anchor_hit<'a>(
    anchor: &str,
    hits: &'a [SearchHit],
) -> Option<&'a SearchHit> {
    hits.iter()
        .filter(|hit| hit.kind != NodeKind::UNKNOWN && is_resolvable_graph_target(anchor, hit))
        .min_by(|left, right| compare_resolution_hits(anchor, left, right))
        .or_else(|| {
            hits.iter()
                .filter(|hit| is_resolvable_graph_target(anchor, hit))
                .min_by(|left, right| compare_resolution_hits(anchor, left, right))
        })
}
