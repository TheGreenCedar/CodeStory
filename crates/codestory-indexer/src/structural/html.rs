use crate::intermediate_storage::IntermediateStorage;
use crate::structural::blanking::{
    EmbeddedRegionKind, blank_non_script_regions, extract_embedded_regions,
    extract_style_block_sources,
};
use crate::{get_language_for_ext, index_file};
use codestory_contracts::graph::{EdgeId, EdgeKind, NodeId, NodeKind};
use std::collections::HashMap;
use std::path::Path;

use super::common::{push_import_edge, push_member_edge, push_structural_node, push_usage_edge};
use super::css::collect_css_entities;

pub(crate) fn collect_html_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) {
    let path_key = path.to_string_lossy();
    let mut region_nodes: HashMap<u32, NodeId> = HashMap::new();
    let mut id_nodes: HashMap<String, NodeId> = HashMap::new();

    for (line_idx, line_text) in source.lines().enumerate() {
        let line_number = line_idx as u32 + 1;
        if let Some(region_id) =
            maybe_region_node(&path_key, file_id, storage, line_number, line_text)
        {
            region_nodes.insert(line_number, region_id);
        }
        for id in extract_html_ids(line_text) {
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
                line_number,
                1,
            );
            id_nodes.insert(id.clone(), node_id);
            if let Some(region_id) = region_nodes.values().last().copied() {
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
            let host_id = region_nodes
                .get(&line_number)
                .copied()
                .or_else(|| id_nodes.values().copied().next())
                .unwrap_or(file_id);
            push_usage_edge(storage, file_id, host_id, css_id, line_number);
        }
    }

    for (line, style_source) in extract_style_block_sources(source) {
        collect_css_entities(path, &style_source, file_id, storage, line);
    }

    delegate_script_blocks(path, source, file_id, storage);
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
    let is_region_tag = [
        "<main",
        "<body",
        "<section",
        "<article",
        "<template",
        "<div",
    ]
    .iter()
    .any(|tag| lower.contains(tag));
    if !is_region_tag {
        return None;
    }
    let canonical = format!("html:region:{path_key}:{line}");
    Some(push_structural_node(
        storage,
        file_id,
        NodeKind::MODULE,
        &format!("region:{line}"),
        &canonical,
        line,
        1,
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
            let node_id = push_structural_node(
                storage,
                file_id,
                NodeKind::MODULE,
                &format!("script:{}", region.start_line),
                &canonical,
                region.start_line,
                1,
            );
            push_import_edge(storage, file_id, file_id, node_id, region.start_line);
        }
        return;
    };

    if let Ok(index_result) = index_file(&delegate_path, &blanked, &language_config, None, None) {
        merge_delegated_script_graph(storage, file_id, index_result, &script_regions);
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
) {
    let delegated_file_id = index_result
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::FILE)
        .map(|node| node.id);
    let script_module = script_regions.first().map(|region| {
        let canonical = format!("html:script-block:{}", region.start_line);
        push_structural_node(
            storage,
            host_file_id,
            NodeKind::MODULE,
            "script",
            &canonical,
            region.start_line,
            1,
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

fn extract_html_ids(line: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let lower = line.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(rel) = lower[search..]
        .find("id=")
        .or_else(|| lower[search..].find("id ="))
    {
        let idx = search + rel;
        let rest = &line[idx..];
        if let Some(value) = extract_attr_value(rest)
            && !value.is_empty()
        {
            ids.push(value);
        }
        search = idx + 3;
    }
    ids
}

fn extract_html_classes(line: &str) -> Vec<String> {
    let mut classes = Vec::new();
    let lower = line.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(rel) = lower[search..]
        .find("class=")
        .or_else(|| lower[search..].find("class ="))
    {
        let idx = search + rel;
        let rest = &line[idx..];
        if let Some(value) = extract_attr_value(rest) {
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

fn extract_attr_value(text: &str) -> Option<String> {
    let after_eq = text.split('=').nth(1)?.trim();
    if let Some(stripped) = after_eq.strip_prefix('"') {
        let end = stripped.find('"')?;
        return Some(stripped[..end].to_string());
    }
    if let Some(stripped) = after_eq.strip_prefix('\'') {
        let end = stripped.find('\'')?;
        return Some(stripped[..end].to_string());
    }
    let end = after_eq
        .find(|c: char| c.is_whitespace() || c == '>')
        .unwrap_or(after_eq.len());
    Some(after_eq[..end].to_string())
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
}
