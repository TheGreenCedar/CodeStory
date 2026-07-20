use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{NodeId, NodeKind};
use std::path::Path;

use super::StructuralCollectionError;
use super::common::{StructuralSourceSpan, push_member_edge, push_structural_node};

pub(crate) fn collect_markdown_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) -> Result<(), StructuralCollectionError> {
    let mut ordinal = 0usize;
    for (line_index, line_text) in source.lines().enumerate() {
        let line = line_number(line_index);
        let trimmed = line_text.trim_start();
        let indent = line_text.len().saturating_sub(trimmed.len());

        if let Some((label, offset, len)) = markdown_heading(trimmed) {
            ordinal += 1;
            push_anchor(
                path,
                storage,
                file_id,
                NodeKind::MODULE,
                "heading",
                &label,
                ordinal,
                line,
                indent + offset,
                len,
            );
            continue;
        }
        if let Some((label, offset, len)) = markdown_reference(trimmed) {
            ordinal += 1;
            push_anchor(
                path,
                storage,
                file_id,
                NodeKind::ANNOTATION,
                "reference",
                &label,
                ordinal,
                line,
                indent + offset,
                len,
            );
            continue;
        }
        if let Some((label, offset, len)) = markdown_fence_label(trimmed) {
            ordinal += 1;
            push_anchor(
                path,
                storage,
                file_id,
                NodeKind::ANNOTATION,
                "fence",
                &label,
                ordinal,
                line,
                indent + offset,
                len,
            );
        }
    }
    Ok(())
}

pub(crate) fn collect_yaml_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) -> Result<(), StructuralCollectionError> {
    validate_yaml_source(source)?;
    let mut ordinal = 0usize;
    for (line_index, line_text) in source.lines().enumerate() {
        let line = line_number(line_index);
        let code = strip_yaml_comment(line_text);
        let trimmed = code.trim_start();
        if trimmed.is_empty()
            || trimmed.starts_with("---")
            || trimmed.starts_with("...")
            || trimmed.starts_with('#')
        {
            continue;
        }
        let indent = code.len().saturating_sub(trimmed.len());
        let (candidate, candidate_offset) = if let Some(rest) = trimmed.strip_prefix("- ") {
            (rest, indent + 2)
        } else {
            (trimmed, indent)
        };
        let Some(colon) = mapping_delimiter(candidate) else {
            continue;
        };
        let raw_key = candidate[..colon].trim();
        if raw_key.is_empty() || raw_key == "<<" {
            continue;
        }
        let key_offset = candidate[..colon].find(raw_key).unwrap_or_default();
        let label = unquote_label(raw_key);
        if label.is_empty() {
            continue;
        }
        ordinal += 1;
        push_anchor(
            path,
            storage,
            file_id,
            NodeKind::ANNOTATION,
            "mapping-key",
            label,
            ordinal,
            line,
            candidate_offset + key_offset,
            raw_key.len(),
        );
    }
    Ok(())
}

pub(crate) fn collect_toml_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) -> Result<(), StructuralCollectionError> {
    toml::from_str::<toml::Value>(source).map_err(|error| {
        StructuralCollectionError::Malformed(format!("invalid TOML syntax: {error}"))
    })?;

    let mut ordinal = 0usize;
    for (line_index, line_text) in source.lines().enumerate() {
        let line = line_number(line_index);
        let code = strip_toml_comment(line_text);
        let trimmed = code.trim();
        if trimmed.is_empty() {
            continue;
        }
        let leading = code.find(trimmed).unwrap_or_default();
        if let Some(raw_table) = toml_table_label(trimmed) {
            let label = unquote_label(raw_table);
            if !label.is_empty() {
                let offset = code.find(raw_table).unwrap_or(leading);
                ordinal += 1;
                push_anchor(
                    path,
                    storage,
                    file_id,
                    NodeKind::MODULE,
                    "table",
                    label,
                    ordinal,
                    line,
                    offset,
                    raw_table.len(),
                );
            }
            continue;
        }
        let Some(equals) = assignment_delimiter(trimmed, '=') else {
            continue;
        };
        let raw_key = trimmed[..equals].trim();
        if raw_key.is_empty() {
            continue;
        }
        let offset = leading + trimmed[..equals].find(raw_key).unwrap_or_default();
        let label = unquote_label(raw_key);
        ordinal += 1;
        push_anchor(
            path,
            storage,
            file_id,
            NodeKind::ANNOTATION,
            "key",
            label,
            ordinal,
            line,
            offset,
            raw_key.len(),
        );
    }
    Ok(())
}

pub(crate) fn collect_json_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) -> Result<(), StructuralCollectionError> {
    serde_json::from_str::<serde_json::Value>(source).map_err(|error| {
        StructuralCollectionError::Malformed(format!("invalid JSON syntax: {error}"))
    })?;

    let bytes = source.as_bytes();
    let mut cursor = 0usize;
    let mut ordinal = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] != b'"' {
            cursor += 1;
            continue;
        }
        let quote_start = cursor;
        cursor += 1;
        let raw_start = cursor;
        let mut escaped = false;
        while cursor < bytes.len() {
            let byte = bytes[cursor];
            if escaped {
                escaped = false;
                cursor += 1;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                cursor += 1;
                continue;
            }
            if byte == b'"' {
                break;
            }
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }
        let quote_end = cursor;
        cursor += 1;
        let mut after = cursor;
        while bytes.get(after).is_some_and(u8::is_ascii_whitespace) {
            after += 1;
        }
        if bytes.get(after) != Some(&b':') {
            continue;
        }
        let quoted = &source[quote_start..=quote_end];
        let label = serde_json::from_str::<String>(quoted).map_err(|error| {
            StructuralCollectionError::Malformed(format!("invalid JSON object key: {error}"))
        })?;
        let (line, col) = line_col_for_byte_offset(source, raw_start);
        ordinal += 1;
        push_anchor(
            path,
            storage,
            file_id,
            NodeKind::ANNOTATION,
            "object-key",
            &label,
            ordinal,
            line,
            col,
            quote_end.saturating_sub(raw_start),
        );
    }
    Ok(())
}

pub(crate) fn collect_shell_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) -> Result<(), StructuralCollectionError> {
    collect_script_entities(path, source, file_id, storage, ScriptFamily::Shell);
    Ok(())
}

pub(crate) fn collect_powershell_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) -> Result<(), StructuralCollectionError> {
    collect_script_entities(path, source, file_id, storage, ScriptFamily::PowerShell);
    Ok(())
}

#[derive(Clone, Copy)]
enum ScriptFamily {
    Shell,
    PowerShell,
}

fn collect_script_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    family: ScriptFamily,
) {
    let mut ordinal = 0usize;
    for (line_index, line_text) in source.lines().enumerate() {
        let line = line_number(line_index);
        let code = strip_script_comment(line_text);
        let trimmed = code.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let indent = code.len().saturating_sub(trimmed.len());
        let anchor = match family {
            ScriptFamily::Shell => shell_anchor(trimmed),
            ScriptFamily::PowerShell => powershell_anchor(trimmed),
        };
        let Some((kind, role, label, offset, len)) = anchor else {
            continue;
        };
        ordinal += 1;
        push_anchor(
            path,
            storage,
            file_id,
            kind,
            role,
            &label,
            ordinal,
            line,
            indent + offset,
            len,
        );
    }
}

fn markdown_heading(line: &str) -> Option<(String, usize, usize)> {
    let hash_count = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&hash_count)
        || !line
            .as_bytes()
            .get(hash_count)
            .is_some_and(u8::is_ascii_whitespace)
    {
        return None;
    }
    let content_start = hash_count
        + line[hash_count..]
            .len()
            .saturating_sub(line[hash_count..].trim_start().len());
    let raw = line[content_start..].trim_end();
    let raw = raw.trim_end_matches('#').trim_end();
    (!raw.is_empty()).then(|| (raw.to_string(), content_start, raw.len()))
}

fn markdown_reference(line: &str) -> Option<(String, usize, usize)> {
    let close = line.find("]:")?;
    let raw = line.strip_prefix('[')?.get(..close.saturating_sub(1))?;
    (!raw.is_empty()).then(|| (raw.to_string(), 1, raw.len()))
}

fn markdown_fence_label(line: &str) -> Option<(String, usize, usize)> {
    let marker = if line.starts_with("```") {
        '`'
    } else if line.starts_with("~~~") {
        '~'
    } else {
        return None;
    };
    let marker_len = line.chars().take_while(|ch| *ch == marker).count();
    let rest = line[marker_len..].trim_start();
    let label = rest
        .split(|ch: char| ch.is_ascii_whitespace() || ch == '{')
        .next()
        .unwrap_or_default();
    if label.is_empty() {
        return None;
    }
    let offset = line.find(label)?;
    Some((label.to_string(), offset, label.len()))
}

fn validate_yaml_source(source: &str) -> Result<(), StructuralCollectionError> {
    let mut flow = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    for (line_index, line) in source.lines().enumerate() {
        if line
            .as_bytes()
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .any(|byte| *byte == b'\t')
        {
            return Err(StructuralCollectionError::Malformed(format!(
                "YAML indentation contains a tab on line {}",
                line_index + 1
            )));
        }
        for ch in strip_yaml_comment(line).chars() {
            if let Some(active) = quote {
                if escaped {
                    escaped = false;
                    continue;
                }
                if active == '"' && ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == active {
                    quote = None;
                }
                continue;
            }
            match ch {
                '\'' | '"' => quote = Some(ch),
                '[' | '{' => flow.push(ch),
                ']' if flow.pop() != Some('[') => {
                    return Err(StructuralCollectionError::Malformed(format!(
                        "unmatched YAML flow delimiter on line {}",
                        line_index + 1
                    )));
                }
                '}' if flow.pop() != Some('{') => {
                    return Err(StructuralCollectionError::Malformed(format!(
                        "unmatched YAML flow delimiter on line {}",
                        line_index + 1
                    )));
                }
                _ => {}
            }
        }
    }
    if !flow.is_empty() {
        return Err(StructuralCollectionError::Malformed(
            "unterminated YAML flow collection".to_string(),
        ));
    }
    if quote.is_some() {
        return Err(StructuralCollectionError::Malformed(
            "unterminated YAML quoted scalar".to_string(),
        ));
    }
    Ok(())
}

fn strip_yaml_comment(line: &str) -> &str {
    strip_comment(line, '#')
}

fn strip_toml_comment(line: &str) -> &str {
    strip_comment(line, '#')
}

fn strip_script_comment(line: &str) -> &str {
    strip_comment(line, '#')
}

fn strip_comment(line: &str, comment: char) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            value if value == comment => return &line[..index],
            _ => {}
        }
    }
    line
}

fn mapping_delimiter(value: &str) -> Option<usize> {
    assignment_delimiter(value, ':')
}

fn assignment_delimiter(value: &str, delimiter: char) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            value if value == delimiter => return Some(index),
            _ => {}
        }
    }
    None
}

fn toml_table_label(line: &str) -> Option<&str> {
    if let Some(inner) = line
        .strip_prefix("[[")
        .and_then(|value| value.strip_suffix("]]"))
    {
        return Some(inner.trim());
    }
    line.strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .map(str::trim)
}

fn shell_anchor(line: &str) -> Option<(NodeKind, &'static str, String, usize, usize)> {
    if let Some(rest) = line.strip_prefix("function ") {
        let name = script_identifier(rest)?;
        let offset = line.find(name)?;
        return Some((
            NodeKind::FUNCTION,
            "function",
            name.to_string(),
            offset,
            name.len(),
        ));
    }
    if let Some(paren) = line.find("()") {
        let raw = line[..paren].trim();
        if is_script_identifier(raw) {
            let offset = line.find(raw)?;
            return Some((
                NodeKind::FUNCTION,
                "function",
                raw.to_string(),
                offset,
                raw.len(),
            ));
        }
    }
    for prefix in ["source ", ". ", "autoload "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let raw = rest.split_ascii_whitespace().next()?;
            let label = unquote_label(raw).to_string();
            if label.is_empty() {
                return None;
            }
            let offset = line.find(raw)?;
            return Some((NodeKind::ANNOTATION, "import", label, offset, raw.len()));
        }
    }
    None
}

fn powershell_anchor(line: &str) -> Option<(NodeKind, &'static str, String, usize, usize)> {
    let lower = line.to_ascii_lowercase();
    for prefix in ["function ", "filter ", "workflow "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let original_rest = &line[line.len().saturating_sub(rest.len())..];
            let name = script_identifier(original_rest)?;
            let offset = line.find(name)?;
            return Some((
                NodeKind::FUNCTION,
                "function",
                name.to_string(),
                offset,
                name.len(),
            ));
        }
    }
    if lower.starts_with("import-module ") {
        let raw = line["import-module ".len()..]
            .split_ascii_whitespace()
            .next()?;
        let label = unquote_label(raw).to_string();
        let offset = line.find(raw)?;
        return Some((NodeKind::ANNOTATION, "import", label, offset, raw.len()));
    }
    if let Some(rest) = line.strip_prefix(". ") {
        let raw = rest.split_ascii_whitespace().next()?;
        let label = unquote_label(raw).to_string();
        let offset = line.find(raw)?;
        return Some((NodeKind::ANNOTATION, "import", label, offset, raw.len()));
    }
    None
}

fn script_identifier(value: &str) -> Option<&str> {
    let end = value
        .char_indices()
        .take_while(|(_, ch)| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':' | '.' | '/')
        })
        .map(|(index, ch)| index + ch.len_utf8())
        .last()?;
    let identifier = &value[..end];
    is_script_identifier(identifier).then_some(identifier)
}

fn is_script_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':' | '.' | '/'))
}

fn unquote_label(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

#[allow(clippy::too_many_arguments)]
fn push_anchor(
    path: &Path,
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    kind: NodeKind,
    role: &str,
    label: &str,
    ordinal: usize,
    line: u32,
    zero_based_col: usize,
    byte_len: usize,
) {
    let path_key = path.to_string_lossy().replace('\\', "/");
    let node_id = push_structural_node(
        storage,
        file_id,
        kind,
        label,
        &format!("structural-generic:{path_key}:{role}:{ordinal}:{line}:{zero_based_col}:{label}"),
        StructuralSourceSpan::token(line, zero_based_col, byte_len.max(1)),
    );
    push_member_edge(storage, file_id, file_id, node_id, line);
}

fn line_col_for_byte_offset(source: &str, offset: usize) -> (u32, usize) {
    let prefix = &source.as_bytes()[..offset.min(source.len())];
    let line = prefix.iter().filter(|byte| **byte == b'\n').count() + 1;
    let col = prefix
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(prefix.len(), |newline| prefix.len() - newline - 1);
    (line.try_into().unwrap_or(u32::MAX), col)
}

fn line_number(index: usize) -> u32 {
    index.saturating_add(1).try_into().unwrap_or(u32::MAX)
}
