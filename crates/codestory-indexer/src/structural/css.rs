use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{NodeId, NodeKind};
use std::collections::HashSet;
use std::path::Path;

use super::common::{push_member_edge, push_structural_node, push_usage_edge};

pub(crate) fn collect_css_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    line_offset: u32,
) {
    let path_key = path.to_string_lossy();
    let mut seen_classes = HashSet::new();
    let mut seen_ids = HashSet::new();
    let mut seen_vars = HashSet::new();

    for (line_idx, line_text) in source.lines().enumerate() {
        let line_number = line_idx as u32 + line_offset;
        collect_css_tokens_on_line(
            &path_key,
            file_id,
            storage,
            line_number,
            line_text,
            &mut seen_classes,
            &mut seen_ids,
            &mut seen_vars,
        );
        collect_css_imports(path, file_id, storage, line_number, line_text);
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_css_tokens_on_line(
    path_key: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    line: u32,
    text: &str,
    seen_classes: &mut HashSet<String>,
    seen_ids: &mut HashSet<String>,
    seen_vars: &mut HashSet<String>,
) {
    let mut cursor = 0usize;
    while let Some(rel) = text[cursor..].find('.') {
        let start = cursor + rel + 1;
        let name = take_css_ident(&text[start..]);
        if name.is_empty() {
            cursor = start;
            continue;
        }
        if seen_classes.insert(name.to_string()) {
            let canonical = format!("css:class:{name}");
            let node_id = push_structural_node(
                storage,
                file_id,
                NodeKind::CONSTANT,
                name,
                &canonical,
                line,
                start as u32,
            );
            push_member_edge(storage, file_id, file_id, node_id, line);
        }
        cursor = start + name.len();
    }

    cursor = 0;
    while let Some(rel) = text[cursor..].find('#') {
        let start = cursor + rel + 1;
        let name = take_css_ident(&text[start..]);
        if name.is_empty() {
            cursor = start;
            continue;
        }
        if seen_ids.insert(name.to_string()) {
            let canonical = format!("css:id:{name}");
            let node_id = push_structural_node(
                storage,
                file_id,
                NodeKind::CONSTANT,
                name,
                &canonical,
                line,
                start as u32,
            );
            push_member_edge(storage, file_id, file_id, node_id, line);
        }
        cursor = start + name.len();
    }

    cursor = 0;
    while let Some(rel) = text[cursor..].find("--") {
        let start = cursor + rel;
        let name = take_css_var_name(&text[start..]);
        if name.is_empty() {
            cursor = start + 2;
            continue;
        }
        if seen_vars.insert(name.to_string()) {
            let canonical = format!("css:var:{name}");
            let node_id = push_structural_node(
                storage,
                file_id,
                NodeKind::VARIABLE,
                name,
                &canonical,
                line,
                start as u32,
            );
            push_member_edge(storage, file_id, file_id, node_id, line);
        }
        cursor = start + name.len();
    }

    let _ = path_key;
}

fn collect_css_imports(
    path: &Path,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    line: u32,
    text: &str,
) {
    let lower = text.to_ascii_lowercase();
    if !lower.contains("@import") {
        return;
    }
    let import_path = extract_quoted_path(text).unwrap_or_default();
    if import_path.is_empty() {
        return;
    }
    let canonical = format!("file:{}", import_path);
    let target_id = NodeId(crate::generate_id(&canonical));
    storage.nodes.push(codestory_contracts::graph::Node {
        id: target_id,
        kind: NodeKind::FILE,
        serialized_name: import_path.clone(),
        qualified_name: Some(import_path.clone()),
        canonical_id: Some(canonical),
        file_node_id: None,
        start_line: Some(line),
        start_col: Some(1),
        end_line: Some(line),
        end_col: Some(import_path.len().max(1) as u32),
    });
    push_usage_edge(storage, file_id, file_id, target_id, line);
    let _ = path;
}

fn take_css_ident(text: &str) -> &str {
    let end = text
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        .unwrap_or(text.len());
    &text[..end]
}

fn take_css_var_name(text: &str) -> &str {
    if !text.starts_with("--") {
        return "";
    }
    let rest = &text[2..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .unwrap_or(rest.len());
    if end == 0 {
        return "";
    }
    &text[..2 + end]
}

fn extract_quoted_path(text: &str) -> Option<String> {
    for quote in ['"', '\''] {
        if let Some(start) = text.find(quote) {
            let rest = &text[start + 1..];
            if let Some(end) = rest.find(quote) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intermediate_storage::IntermediateStorage;
    use codestory_contracts::graph::{EdgeKind, NodeKind};
    use std::path::Path;

    #[test]
    fn collects_class_id_and_custom_property() {
        let mut storage = IntermediateStorage::default();
        let file_id = NodeId(42);
        collect_css_entities(
            Path::new("styles.css"),
            ".btn { color: var(--primary); }\n#app { }",
            file_id,
            &mut storage,
            1,
        );
        let kinds: Vec<_> = storage.nodes.iter().map(|n| n.kind).collect();
        assert!(kinds.contains(&NodeKind::CONSTANT));
        assert!(kinds.contains(&NodeKind::VARIABLE));
        assert!(storage.edges.iter().any(|e| e.kind == EdgeKind::MEMBER));
    }
}
