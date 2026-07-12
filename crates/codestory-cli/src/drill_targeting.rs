use anyhow::{Result, bail};
use std::collections::HashSet;

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
