use crate::intermediate_storage::IntermediateStorage;
use crate::structural::blanking::{
    EmbeddedRegionKind, blank_non_script_regions, byte_offset_line_col, extract_embedded_regions,
    extract_style_block_sources,
};
use crate::{get_language_for_ext, index_file};
use codestory_contracts::graph::{EdgeId, EdgeKind, NodeId, NodeKind};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use super::common::{
    StructuralSourceSpan, push_import_edge, push_member_edge, push_structural_node, push_usage_edge,
};
use super::css::collect_css_entities;

pub(crate) fn collect_html_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) {
    let path_key = path.to_string_lossy();
    // Anchor parents are persisted inside `structural_edge_id`, so an arbitrary
    // choice is not merely cosmetic: it makes the emitted edge set differ
    // between two indexings of identical bytes (CR-013). `HashMap::values()`
    // yields `RandomState` order, so both lookups below moved with the hash
    // seed. Keyed maps ordered by line make the nearest enclosing region — the
    // parent the old `.last()` was reaching for — a deterministic choice.
    let mut region_nodes: BTreeMap<u32, NodeId> = BTreeMap::new();
    let mut id_nodes: HashMap<String, NodeId> = HashMap::new();
    let mut id_nodes_by_line: BTreeMap<u32, NodeId> = BTreeMap::new();

    for (line_idx, line_text) in source.lines().enumerate() {
        let line_number = line_idx as u32 + 1;
        if let Some(region_id) =
            maybe_region_node(&path_key, file_id, storage, line_number, line_text)
        {
            region_nodes.insert(line_number, region_id);
        }
        for (id, id_start) in extract_html_ids(line_text) {
            if id_nodes.contains_key(&id) {
                continue;
            }
            let canonical = format!("html:id:{id}");
            let node_id = push_structural_node(
                storage,
                file_id,
                NodeKind::CONSTANT,
                &id,
                &canonical,
                StructuralSourceSpan::token(line_number, id_start, id.len()),
            );
            id_nodes.insert(id.clone(), node_id);
            id_nodes_by_line.entry(line_number).or_insert(node_id);
            if let Some(region_id) = nearest_at_or_above(&region_nodes, line_number) {
                push_member_edge(storage, file_id, region_id, node_id, line_number);
            } else {
                push_member_edge(storage, file_id, file_id, node_id, line_number);
            }
        }
        for class_name in extract_html_classes(line_text) {
            let css_canonical = format!("css:class:{class_name}");
            let css_id = NodeId(crate::generate_id(&css_canonical));
            if !storage.nodes.iter().any(|node| node.id == css_id) {
                storage.nodes.push(codestory_contracts::graph::Node {
                    id: css_id,
                    kind: NodeKind::CONSTANT,
                    serialized_name: class_name.clone(),
                    qualified_name: Some(class_name.clone()),
                    canonical_id: Some(css_canonical),
                    file_node_id: None,
                    start_line: Some(line_number),
                    start_col: Some(1),
                    end_line: Some(line_number),
                    end_col: Some(class_name.len().max(1) as u32),
                });
            }
            let host_id = nearest_at_or_above(&region_nodes, line_number)
                .or_else(|| nearest_at_or_above(&id_nodes_by_line, line_number))
                .unwrap_or(file_id);
            push_usage_edge(storage, file_id, host_id, css_id, line_number);
        }
    }

    for (line, col, style_source) in extract_style_block_sources(source) {
        collect_css_entities(
            path,
            &style_source,
            file_id,
            storage,
            line,
            col.saturating_sub(1) as usize,
        );
    }

    delegate_script_blocks(path, source, file_id, storage);
}

/// The entry whose line is closest to `line` without passing it.
fn nearest_at_or_above(nodes: &BTreeMap<u32, NodeId>, line: u32) -> Option<NodeId> {
    nodes.range(..=line).next_back().map(|(_, id)| *id)
}

fn maybe_region_node(
    path_key: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    line: u32,
    text: &str,
) -> Option<NodeId> {
    let lower = text.to_ascii_lowercase();
    if !lower.contains('<') {
        return None;
    }
    let region_tag = [
        "<main",
        "<body",
        "<section",
        "<article",
        "<template",
        "<div",
    ]
    .iter()
    .filter_map(|tag| lower.find(tag).map(|start| (start, *tag)))
    .min_by_key(|(start, _)| *start)?;
    let canonical = format!("html:region:{path_key}:{line}");
    Some(push_structural_node(
        storage,
        file_id,
        NodeKind::MODULE,
        &format!("region:{line}"),
        &canonical,
        StructuralSourceSpan::token(line, region_tag.0, region_tag.1.len()),
    ))
}

fn delegate_script_blocks(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) {
    let script_regions: Vec<_> = extract_embedded_regions(source)
        .into_iter()
        .filter(|region| region.kind == EmbeddedRegionKind::Script)
        .collect();
    if script_regions.is_empty() {
        return;
    }

    let blanked = blank_non_script_regions(source);
    let lang = script_language_for_source(source);
    let ext = if lang == "typescript" { "ts" } else { "js" };
    let delegate_path = path.with_extension(ext);
    let Some(language_config) = get_language_for_ext(ext) else {
        for region in script_regions {
            let canonical = format!(
                "html:script:{}:{}",
                path.to_string_lossy(),
                region.start_line
            );
            let (open_line, open_col) = byte_offset_line_col(source, region.open_start_byte);
            let node_id = push_structural_node(
                storage,
                file_id,
                NodeKind::MODULE,
                &format!("script:{}", region.start_line),
                &canonical,
                StructuralSourceSpan::token(
                    open_line,
                    open_col.saturating_sub(1) as usize,
                    "<script".len(),
                ),
            );
            push_import_edge(storage, file_id, file_id, node_id, region.start_line);
        }
        return;
    };

    if let Ok(index_result) = index_file(&delegate_path, &blanked, &language_config, None, None) {
        merge_delegated_script_graph(storage, file_id, index_result, &script_regions, source);
    }
}

fn script_language_for_source(source: &str) -> &'static str {
    let lower = source.to_ascii_lowercase();
    if lower.contains("lang=\"ts\"")
        || lower.contains("lang='ts'")
        || lower.contains("lang=\"typescript\"")
    {
        "typescript"
    } else {
        "javascript"
    }
}

fn merge_delegated_script_graph(
    storage: &mut IntermediateStorage,
    host_file_id: NodeId,
    index_result: crate::IndexResult,
    script_regions: &[super::blanking::EmbeddedRegion],
    source: &str,
) {
    let delegated_file_id = index_result
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::FILE)
        .map(|node| node.id);
    let script_module = script_regions.first().map(|region| {
        let canonical = format!("html:script-block:{}", region.start_line);
        let (open_line, open_col) = byte_offset_line_col(source, region.open_start_byte);
        push_structural_node(
            storage,
            host_file_id,
            NodeKind::MODULE,
            "script",
            &canonical,
            StructuralSourceSpan::token(
                open_line,
                open_col.saturating_sub(1) as usize,
                "<script".len(),
            ),
        )
    });

    for mut node in index_result.nodes {
        if node.kind == NodeKind::FILE {
            continue;
        }
        node.file_node_id = Some(host_file_id);
        storage.nodes.push(node);
    }

    for mut edge in index_result.edges {
        if Some(edge.source) == delegated_file_id {
            edge.source = host_file_id;
        }
        if Some(edge.target) == delegated_file_id {
            edge.target = host_file_id;
        }
        if edge.resolved_source == delegated_file_id {
            edge.resolved_source = Some(host_file_id);
        }
        if edge.resolved_target == delegated_file_id {
            edge.resolved_target = Some(host_file_id);
        }
        if edge.file_node_id.is_some() {
            edge.file_node_id = Some(host_file_id);
        }
        if edge.kind == EdgeKind::CALL {
            let col = edge
                .callsite_identity
                .as_deref()
                .and_then(|identity| identity.split(':').nth(2))
                .and_then(|value| value.parse::<u32>().ok());
            edge.callsite_identity = None;
            crate::ensure_callsite_identity(&mut edge, col);
        }
        edge.id = EdgeId(crate::generate_edge_id_for_edge(
            &edge,
            crate::index_feature_flags(),
        ));
        storage.edges.push(edge);
    }

    storage
        .occurrences
        .extend(index_result.occurrences.into_iter().map(|mut occurrence| {
            if Some(NodeId(occurrence.element_id)) == delegated_file_id {
                occurrence.element_id = host_file_id.0;
            }
            occurrence.location.file_node_id = host_file_id;
            occurrence
        }));
    storage
        .component_access
        .extend(index_result.component_access);

    if let (Some(module_id), Some(first_symbol)) = (
        script_module,
        storage
            .nodes
            .iter()
            .find(|node| {
                node.file_node_id == Some(host_file_id)
                    && matches!(
                        node.kind,
                        NodeKind::FUNCTION | NodeKind::CLASS | NodeKind::METHOD
                    )
            })
            .map(|node| node.id),
    ) {
        push_import_edge(
            storage,
            host_file_id,
            module_id,
            first_symbol,
            script_regions[0].start_line,
        );
        push_member_edge(
            storage,
            host_file_id,
            host_file_id,
            module_id,
            script_regions[0].start_line,
        );
    }
}

/// True when `index` begins an attribute name rather than ending one.
///
/// The scan looks for a literal `id=` or `class=`, which also matches the tail
/// of `data-testid=`, `data-id=`, `uid=`, and every other attribute whose name
/// merely ends in those letters (CR-012). HTML attribute names are separated
/// from what precedes them by whitespace, the opening `<tag`, or a quote that
/// closed the previous value, so anything that could continue a name — an
/// ASCII alphanumeric, `-`, `_`, `:`, `.`, or `@` — disqualifies the match.
fn starts_attribute_name(line: &str, index: usize) -> bool {
    let Some(previous) = line[..index].chars().next_back() else {
        return true;
    };
    !(previous.is_ascii_alphanumeric() || matches!(previous, '-' | '_' | ':' | '.' | '@' | '$'))
}

fn find_attribute_start(lower: &str, line: &str, from: usize, name: &str) -> Option<usize> {
    let spaced = format!("{name} =");
    let equals = format!("{name}=");
    let mut search = from;
    while search <= lower.len() {
        let rel = lower[search..]
            .find(&equals)
            .into_iter()
            .chain(lower[search..].find(&spaced))
            .min()?;
        let index = search + rel;
        if starts_attribute_name(line, index) {
            return Some(index);
        }
        search = index + name.len();
    }
    None
}

fn extract_html_ids(line: &str) -> Vec<(String, usize)> {
    let mut ids = Vec::new();
    let lower = line.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(idx) = find_attribute_start(&lower, line, search, "id") {
        let rest = &line[idx..];
        if let Some((value, value_start)) = extract_attr_value(rest)
            && !value.is_empty()
        {
            ids.push((value.to_string(), idx + value_start));
        }
        search = idx + 3;
    }
    ids
}

fn extract_html_classes(line: &str) -> Vec<String> {
    let mut classes = Vec::new();
    let lower = line.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(idx) = find_attribute_start(&lower, line, search, "class") {
        let rest = &line[idx..];
        if let Some((value, _)) = extract_attr_value(rest) {
            for class_name in value.split_whitespace() {
                let class_name = class_name.trim();
                if !class_name.is_empty() {
                    classes.push(class_name.to_string());
                }
            }
        }
        search = idx + 6;
    }
    classes
}

fn extract_attr_value(text: &str) -> Option<(&str, usize)> {
    let equals = text.find('=')?;
    let raw_after_eq = &text[equals + 1..];
    let leading = raw_after_eq
        .len()
        .saturating_sub(raw_after_eq.trim_start().len());
    let after_eq = raw_after_eq.trim_start();
    let value_start = equals.saturating_add(1).saturating_add(leading);
    if let Some(stripped) = after_eq.strip_prefix('"') {
        let end = stripped.find('"')?;
        return Some((&stripped[..end], value_start.saturating_add(1)));
    }
    if let Some(stripped) = after_eq.strip_prefix('\'') {
        let end = stripped.find('\'')?;
        return Some((&stripped[..end], value_start.saturating_add(1)));
    }
    let end = after_eq
        .find(|c: char| c.is_whitespace() || c == '>')
        .unwrap_or(after_eq.len());
    Some((&after_eq[..end], value_start))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intermediate_storage::IntermediateStorage;
    use codestory_contracts::graph::EdgeKind;
    use std::path::Path;

    #[test]
    fn collects_ids_classes_and_style_blocks() {
        let html = r#"<!doctype html>
<main id="app" class="layout primary">
  <style>.layout { }</style>
  <script>function boot() { return 1; }</script>
</main>"#;
        let mut storage = IntermediateStorage::default();
        let file_id = NodeId(99);
        collect_html_entities(Path::new("index.html"), html, file_id, &mut storage);
        assert!(
            storage
                .nodes
                .iter()
                .any(|n| n.canonical_id.as_deref() == Some("html:id:app"))
        );
        assert!(storage.edges.iter().any(|e| e.kind == EdgeKind::USAGE));
        assert!(
            storage
                .nodes
                .iter()
                .any(|n| n.canonical_id.as_deref() == Some("css:class:layout"))
        );
    }

    fn collect(source: &str) -> IntermediateStorage {
        let mut storage = IntermediateStorage::default();
        collect_html_entities(Path::new("index.html"), source, NodeId(99), &mut storage);
        storage
    }

    fn canonical_ids(storage: &IntermediateStorage) -> Vec<String> {
        let mut ids = storage
            .nodes
            .iter()
            .filter_map(|node| node.canonical_id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    #[test]
    fn only_a_real_id_attribute_mints_an_entity() {
        let storage = collect(
            r#"<main id="app">
  <button data-testid="login-submit" data-id="row-7" class="btn">Go</button>
  <input uid="u-1" aria-labelledby="app" id="search">
</main>"#,
        );
        let ids = canonical_ids(&storage);
        for expected in ["html:id:app", "html:id:search"] {
            assert!(
                ids.contains(&expected.to_string()),
                "`{expected}` is a real id attribute: {ids:?}"
            );
        }
        for forbidden in [
            "html:id:login-submit",
            "html:id:row-7",
            "html:id:u-1",
            "html:id:app-labelled",
        ] {
            assert!(
                !ids.contains(&forbidden.to_string()),
                "`{forbidden}` comes from an attribute that merely ends in `id`: {ids:?}"
            );
        }
    }

    #[test]
    fn only_a_real_class_attribute_mints_a_css_usage() {
        let storage = collect(
            r#"<main class="layout">
  <div data-class="ghost" ng-class="dynamic">x</div>
</main>"#,
        );
        let ids = canonical_ids(&storage);
        assert!(ids.contains(&"css:class:layout".to_string()), "{ids:?}");
        for forbidden in ["css:class:ghost", "css:class:dynamic"] {
            assert!(
                !ids.contains(&forbidden.to_string()),
                "`{forbidden}` comes from an attribute that merely ends in `class`: {ids:?}"
            );
        }
    }

    #[test]
    fn anchor_parents_are_the_nearest_enclosing_region_on_every_run() {
        // Two regions and two ids, so the parent choice is ambiguous unless it
        // is anchored: the old `HashMap::values().last()` picked whichever the
        // hash seed happened to yield.
        let source = r#"<section id="first">
  <p>one</p>
</section>
<article id="second">
  <p class="two">two</p>
</article>"#;
        let baseline = collect(source);
        let region_by_id = baseline
            .nodes
            .iter()
            .filter_map(|node| Some((node.id, node.canonical_id.clone()?)))
            .collect::<std::collections::HashMap<_, _>>();
        let describe = |storage: &IntermediateStorage| {
            let mut rows = storage
                .edges
                .iter()
                .map(|edge| {
                    format!(
                        "{:?}:{}->{}",
                        edge.kind,
                        region_by_id
                            .get(&edge.source)
                            .cloned()
                            .unwrap_or_else(|| edge.source.0.to_string()),
                        region_by_id
                            .get(&edge.target)
                            .cloned()
                            .unwrap_or_else(|| edge.target.0.to_string())
                    )
                })
                .collect::<Vec<_>>();
            rows.sort();
            rows
        };

        let baseline_edges = describe(&baseline);
        assert!(
            baseline_edges
                .iter()
                .any(|row| row == "MEMBER:html:region:index.html:1->html:id:first"),
            "the id on the section line belongs to that section: {baseline_edges:?}"
        );
        assert!(
            baseline_edges
                .iter()
                .any(|row| row == "MEMBER:html:region:index.html:4->html:id:second"),
            "the id on the article line belongs to that article: {baseline_edges:?}"
        );

        // Repeated indexing of identical bytes must emit an identical edge set;
        // the parent is baked into `structural_edge_id`, so drift here is
        // persisted churn.
        for _ in 0..16 {
            assert_eq!(
                describe(&collect(source)),
                baseline_edges,
                "identical source must produce an identical edge set"
            );
        }
    }
}
