use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{NodeId, NodeKind};
use std::path::Path;

use super::common::{push_member_edge, push_structural_node};

pub(crate) fn collect_docker_compose_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) {
    let path_key = path.to_string_lossy().replace('\\', "/");
    let Some((stack_name, stack_line, stack_col)) = compose_stack_anchor(source) else {
        return;
    };
    let stack_id = push_structural_node(
        storage,
        file_id,
        NodeKind::MODULE,
        &stack_name,
        &format!("docker-compose:stack:{path_key}"),
        stack_line,
        stack_col,
    );
    push_member_edge(storage, file_id, file_id, stack_id, stack_line);
    collect_services(&path_key, source, file_id, storage, stack_id);
}

fn collect_services(
    path_key: &str,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    stack_id: NodeId,
) {
    let mut in_services = false;
    let mut service_indent = None;
    let mut current_service: Option<(usize, NodeId, String)> = None;
    let mut section: Option<(&str, usize)> = None;
    let mut anchor_index = 0usize;

    for (line_idx, line_text) in source.lines().enumerate() {
        let line = line_idx.saturating_add(1).try_into().unwrap_or(u32::MAX);
        let trimmed = line_text.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = leading_spaces(line_text);

        if indent == 0 && trimmed == "services:" {
            in_services = true;
            continue;
        }
        if !in_services {
            continue;
        }
        if indent == 0 {
            break;
        }

        if service_indent.is_none()
            && indent > 0
            && let Some(service_name) = yaml_mapping_key(trimmed)
        {
            service_indent = Some(indent);
            let service_id = push_service(
                storage,
                file_id,
                stack_id,
                path_key,
                &service_name,
                line,
                indent,
            );
            current_service = Some((indent, service_id, service_name));
            section = None;
            continue;
        }

        if let Some(expected_indent) = service_indent
            && indent == expected_indent
            && let Some(service_name) = yaml_mapping_key(trimmed)
        {
            let service_id = push_service(
                storage,
                file_id,
                stack_id,
                path_key,
                &service_name,
                line,
                indent,
            );
            current_service = Some((indent, service_id, service_name));
            section = None;
            continue;
        }

        let Some((current_indent, service_id, service_name)) = current_service.as_ref() else {
            continue;
        };
        if indent <= *current_indent {
            section = None;
            continue;
        }

        if let Some((field, section_indent)) = section
            && indent > section_indent
            && let Some((anchor, col)) = section_anchor(field, line_text)
        {
            anchor_index = anchor_index.saturating_add(1);
            push_anchor(
                storage,
                file_id,
                *service_id,
                path_key,
                service_name,
                &anchor,
                anchor_index,
                line,
                col,
            );
            continue;
        }

        let Some(field) = yaml_mapping_key(trimmed) else {
            continue;
        };
        match field.as_str() {
            "image" | "build" => {
                anchor_index = anchor_index.saturating_add(1);
                push_anchor(
                    storage,
                    file_id,
                    *service_id,
                    path_key,
                    service_name,
                    trimmed,
                    anchor_index,
                    line,
                    indent.saturating_add(1).try_into().unwrap_or(u32::MAX),
                );
            }
            "ports" | "environment" | "volumes" => {
                section = Some((
                    match field.as_str() {
                        "ports" => "ports",
                        "environment" => "environment",
                        _ => "volumes",
                    },
                    indent,
                ));
            }
            _ => {
                section = None;
            }
        }
    }
}

fn push_service(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    stack_id: NodeId,
    path_key: &str,
    service_name: &str,
    line: u32,
    indent: usize,
) -> NodeId {
    let service_id = push_structural_node(
        storage,
        file_id,
        NodeKind::FUNCTION,
        service_name,
        &format!("docker-compose:service:{path_key}:{service_name}"),
        line,
        indent.saturating_add(1).try_into().unwrap_or(u32::MAX),
    );
    push_member_edge(storage, file_id, stack_id, service_id, line);
    service_id
}

fn push_anchor(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    service_id: NodeId,
    path_key: &str,
    service_name: &str,
    anchor: &str,
    anchor_index: usize,
    line: u32,
    col: u32,
) {
    let anchor_id = push_structural_node(
        storage,
        file_id,
        NodeKind::ANNOTATION,
        anchor,
        &format!("docker-compose:anchor:{path_key}:{service_name}:{anchor_index}"),
        line,
        col,
    );
    push_member_edge(storage, file_id, service_id, anchor_id, line);
}

fn compose_stack_anchor(source: &str) -> Option<(String, u32, u32)> {
    top_level_name(source).or_else(|| top_level_services_anchor(source))
}

fn top_level_name(source: &str) -> Option<(String, u32, u32)> {
    for (line_idx, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if leading_spaces(line) == 0
            && let Some(rest) = trimmed.strip_prefix("name:")
        {
            let value_text = rest.trim_start();
            let value = clean_yaml_scalar(value_text);
            if !value.is_empty() {
                let value_offset = rest.len().saturating_sub(value_text.len())
                    + value_text.find(&value).unwrap_or(0);
                let col = "name:".len().saturating_add(value_offset).saturating_add(1);
                return Some((
                    value,
                    line_idx.saturating_add(1).try_into().unwrap_or(u32::MAX),
                    col.try_into().unwrap_or(u32::MAX),
                ));
            }
        }
    }
    None
}

fn top_level_services_anchor(source: &str) -> Option<(String, u32, u32)> {
    source.lines().enumerate().find_map(|(line_idx, line)| {
        (leading_spaces(line) == 0 && line.trim_start() == "services:").then(|| {
            (
                "services:".to_string(),
                line_idx.saturating_add(1).try_into().unwrap_or(u32::MAX),
                line.find("services:").unwrap_or(0).saturating_add(1) as u32,
            )
        })
    })
}

fn section_anchor(field: &str, line: &str) -> Option<(String, u32)> {
    let indent = leading_spaces(line);
    let trimmed = line.trim_start();
    match field {
        "ports" | "volumes" => trimmed.strip_prefix("- ").map(|_| {
            (
                trimmed.to_string(),
                indent.saturating_add(1).try_into().unwrap_or(u32::MAX),
            )
        }),
        "environment" => environment_anchor(trimmed, indent),
        _ => None,
    }
}

fn environment_anchor(trimmed: &str, indent: usize) -> Option<(String, u32)> {
    if let Some(rest) = trimmed.strip_prefix("- ") {
        let key = rest.split_once('=')?.0.trim();
        if key.is_empty() {
            return None;
        }
        return Some((
            key.to_string(),
            indent.saturating_add(3).try_into().unwrap_or(u32::MAX),
        ));
    }
    let key = yaml_mapping_key(trimmed)?;
    Some((key, indent.saturating_add(1).try_into().unwrap_or(u32::MAX)))
}

fn yaml_mapping_key(trimmed_line: &str) -> Option<String> {
    let (key, _) = trimmed_line.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || key.contains(' ') || !key.chars().all(compose_key_char) {
        return None;
    }
    Some(key.to_string())
}

fn clean_yaml_scalar(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|ch| *ch == ' ').count()
}

fn compose_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')
}
