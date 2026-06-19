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
    let Some((stack_name, stack_line, stack_col)) = stack_anchor(source) else {
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
    let mut active_block: Option<(&str, usize)> = None;
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
            current_service = None;
            active_block = None;
            continue;
        }
        if !in_services {
            continue;
        }
        if indent == 0 {
            break;
        }

        if let Some(expected_indent) = service_indent
            && indent == expected_indent
            && !trimmed.starts_with('-')
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
            active_block = None;
            anchor_index = 0;
            continue;
        }

        if service_indent.is_none()
            && indent > 0
            && !trimmed.starts_with('-')
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
            active_block = None;
            anchor_index = 0;
            continue;
        }

        let Some((current_service_indent, service_id, service_name)) = current_service.as_ref()
        else {
            continue;
        };
        if indent <= *current_service_indent {
            active_block = None;
            continue;
        }

        if service_field_is(trimmed, "image") || service_field_is(trimmed, "build") {
            push_compose_anchor(
                storage,
                file_id,
                *service_id,
                path_key,
                service_name,
                field_name(trimmed).unwrap_or("field"),
                trimmed,
                line,
                indent.saturating_add(1).try_into().unwrap_or(u32::MAX),
                &mut anchor_index,
            );
            active_block = None;
            continue;
        }

        if let Some(block) = service_block(trimmed) {
            push_compose_anchor(
                storage,
                file_id,
                *service_id,
                path_key,
                service_name,
                block,
                trimmed,
                line,
                indent.saturating_add(1).try_into().unwrap_or(u32::MAX),
                &mut anchor_index,
            );
            active_block = Some((block, indent));
            continue;
        }

        let Some((block, block_indent)) = active_block else {
            continue;
        };
        if indent <= block_indent {
            active_block = None;
            continue;
        }
        match block {
            "ports" | "volumes" => {
                if let Some((anchor, col)) = list_anchor(line_text) {
                    push_compose_anchor(
                        storage,
                        file_id,
                        *service_id,
                        path_key,
                        service_name,
                        block,
                        &anchor,
                        line,
                        col,
                        &mut anchor_index,
                    );
                }
            }
            "environment" => {
                if let Some((key, col)) = environment_key_anchor(line_text) {
                    push_compose_anchor(
                        storage,
                        file_id,
                        *service_id,
                        path_key,
                        service_name,
                        block,
                        &key,
                        line,
                        col,
                        &mut anchor_index,
                    );
                }
            }
            _ => {}
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

fn push_compose_anchor(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    service_id: NodeId,
    path_key: &str,
    service_name: &str,
    anchor_kind: &str,
    anchor: &str,
    line: u32,
    col: u32,
    anchor_index: &mut usize,
) {
    *anchor_index = (*anchor_index).saturating_add(1);
    let anchor_id = push_structural_node(
        storage,
        file_id,
        NodeKind::ANNOTATION,
        anchor,
        &format!(
            "docker-compose:{anchor_kind}:{path_key}:{service_name}:{}",
            *anchor_index
        ),
        line,
        col,
    );
    push_member_edge(storage, file_id, service_id, anchor_id, line);
}

fn stack_anchor(source: &str) -> Option<(String, u32, u32)> {
    top_level_compose_name(source).or_else(|| top_level_services_anchor(source))
}

fn top_level_compose_name(source: &str) -> Option<(String, u32, u32)> {
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

fn service_field_is(trimmed_line: &str, expected: &str) -> bool {
    field_name(trimmed_line).is_some_and(|name| name == expected)
}

fn service_block(trimmed_line: &str) -> Option<&'static str> {
    match field_name(trimmed_line)? {
        "ports" => Some("ports"),
        "environment" => Some("environment"),
        "volumes" => Some("volumes"),
        _ => None,
    }
}

fn field_name(trimmed_line: &str) -> Option<&str> {
    let (key, _) = trimmed_line.split_once(':')?;
    Some(key.trim())
}

fn yaml_mapping_key(trimmed_line: &str) -> Option<String> {
    let (key, _) = trimmed_line.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || key.contains(' ') || !key.chars().all(compose_key_char) {
        return None;
    }
    Some(key.to_string())
}

fn list_anchor(line: &str) -> Option<(String, u32)> {
    let indent = leading_spaces(line);
    let trimmed = line.trim_start();
    trimmed.strip_prefix("- ")?;
    Some((
        trimmed.to_string(),
        indent.saturating_add(1).try_into().unwrap_or(u32::MAX),
    ))
}

fn environment_key_anchor(line: &str) -> Option<(String, u32)> {
    let indent = leading_spaces(line);
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("- ") {
        let value = rest.trim_start();
        let key = value
            .split(['=', ':'])
            .next()
            .map(str::trim)
            .filter(|key| !key.is_empty())?;
        let value_offset = rest.len().saturating_sub(value.len());
        return Some((
            key.to_string(),
            indent
                .saturating_add(3)
                .saturating_add(value_offset)
                .try_into()
                .unwrap_or(u32::MAX),
        ));
    }
    let key = yaml_mapping_key(trimmed)?;
    Some((key, indent.saturating_add(1).try_into().unwrap_or(u32::MAX)))
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
