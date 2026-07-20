use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{NodeId, NodeKind};
use std::collections::VecDeque;
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
    let mut active_fence: Option<(u8, usize)> = None;
    for (line_index, line_text) in source.lines().enumerate() {
        let line = line_number(line_index);
        let trimmed = line_text.trim_start();
        let indent = line_text.len().saturating_sub(trimmed.len());

        if let Some((marker, marker_len)) = active_fence {
            if markdown_fence_closes(trimmed, marker, marker_len) {
                active_fence = None;
            }
            continue;
        }
        if let Some((marker, marker_len)) = markdown_fence_marker(trimmed) {
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
            active_fence = Some((marker, marker_len));
            continue;
        }
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
    let mut block_scalar_parent_indent = None;
    for (line_index, line_text) in source.lines().enumerate() {
        let line = line_number(line_index);
        let code = strip_yaml_comment(line_text);
        let trimmed = code.trim_start();
        let indent = code.len().saturating_sub(trimmed.len());
        if let Some(parent_indent) = block_scalar_parent_indent {
            if trimmed.is_empty() || indent > parent_indent {
                continue;
            }
            block_scalar_parent_indent = None;
        }
        if trimmed.is_empty()
            || trimmed.starts_with("---")
            || trimmed.starts_with("...")
            || trimmed.starts_with('#')
        {
            continue;
        }
        if yaml_block_scalar_header(trimmed) {
            block_scalar_parent_indent = Some(indent);
        }
        let (candidate, candidate_offset) = if let Some(rest) = trimmed.strip_prefix("- ") {
            (rest, indent + 2)
        } else {
            (trimmed, indent)
        };
        let Some(colon) = yaml_mapping_delimiter(candidate) else {
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
    let mut multiline_quote = None;
    for (line_index, line_text) in source.lines().enumerate() {
        let line = line_number(line_index);
        let masked = mask_toml_multiline_strings(line_text, &mut multiline_quote);
        let code = strip_toml_comment(&masked);
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

#[derive(Clone, Copy)]
enum ShellQuote {
    Single,
    Double,
}

#[derive(Default)]
struct ShellLexicalState {
    quote: Option<ShellQuote>,
    arithmetic_paren_depth: usize,
}

struct ShellLineAnalysis {
    masked: String,
    heredocs: Vec<(String, bool)>,
}

fn collect_script_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    family: ScriptFamily,
) {
    let mut ordinal = 0usize;
    let mut shell_heredocs = VecDeque::new();
    let mut shell_lexical_state = ShellLexicalState::default();
    let mut powershell_block_comment_depth = 0usize;
    for (line_index, line_text) in source.lines().enumerate() {
        let line = line_number(line_index);
        let masked;
        let mut new_shell_heredocs = Vec::new();
        let code = match family {
            ScriptFamily::Shell => {
                if let Some((terminator, strip_tabs)) = shell_heredocs.front() {
                    let candidate = if *strip_tabs {
                        line_text.trim_start_matches('\t')
                    } else {
                        line_text
                    };
                    if candidate.trim_end() == terminator {
                        shell_heredocs.pop_front();
                    }
                    continue;
                }
                let analysis = analyze_shell_line(line_text, &mut shell_lexical_state);
                new_shell_heredocs = analysis.heredocs;
                masked = analysis.masked;
                &masked
            }
            ScriptFamily::PowerShell => {
                masked =
                    mask_powershell_block_comments(line_text, &mut powershell_block_comment_depth);
                &masked
            }
        };
        let code = if matches!(family, ScriptFamily::PowerShell) {
            strip_script_comment(code)
        } else {
            code
        };
        let trimmed = code.trim_start();
        if trimmed.is_empty() {
            shell_heredocs.extend(new_shell_heredocs);
            continue;
        }
        let indent = code.len().saturating_sub(trimmed.len());
        let anchor = match family {
            ScriptFamily::Shell => shell_anchor(trimmed, &line_text[indent..]),
            ScriptFamily::PowerShell => powershell_anchor(trimmed),
        };
        if let Some((kind, role, label, offset, len)) = anchor {
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
        shell_heredocs.extend(new_shell_heredocs);
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

fn markdown_fence_marker(line: &str) -> Option<(u8, usize)> {
    let marker = match line.as_bytes().first().copied()? {
        b'`' => b'`',
        b'~' => b'~',
        _ => return None,
    };
    let marker_len = line.bytes().take_while(|byte| *byte == marker).count();
    (marker_len >= 3).then_some((marker, marker_len))
}

fn markdown_fence_closes(line: &str, marker: u8, opening_len: usize) -> bool {
    let marker_len = line.bytes().take_while(|byte| *byte == marker).count();
    marker_len >= opening_len && line[marker_len..].trim().is_empty()
}

fn validate_yaml_source(source: &str) -> Result<(), StructuralCollectionError> {
    let mut flow = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut block_scalar_parent_indent = None;
    for (line_index, line) in source.lines().enumerate() {
        let indentation = line
            .as_bytes()
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count();
        if line.as_bytes()[..indentation].contains(&b'\t') {
            return Err(StructuralCollectionError::Malformed(format!(
                "YAML indentation contains a tab on line {}",
                line_index + 1
            )));
        }
        let trimmed = line[indentation..].trim_end();
        if let Some(parent_indent) = block_scalar_parent_indent {
            if trimmed.is_empty() || indentation > parent_indent {
                continue;
            }
            block_scalar_parent_indent = None;
        }
        let code = strip_yaml_comment(line);
        for ch in code.chars() {
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
        if flow.is_empty() && yaml_block_scalar_header(code.trim_start()) {
            block_scalar_parent_indent = Some(indentation);
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

fn yaml_mapping_delimiter(value: &str) -> Option<usize> {
    let delimiter = assignment_delimiter(value, ':')?;
    value
        .as_bytes()
        .get(delimiter + 1)
        .is_none_or(u8::is_ascii_whitespace)
        .then_some(delimiter)
}

fn yaml_block_scalar_header(value: &str) -> bool {
    let value = value.strip_prefix("- ").unwrap_or(value);
    let scalar = yaml_mapping_delimiter(value)
        .map(|delimiter| value[delimiter + 1..].trim_start())
        .unwrap_or(value.trim_start());
    let scalar = scalar.trim();
    scalar
        .strip_prefix('|')
        .or_else(|| scalar.strip_prefix('>'))
        .is_some_and(|suffix| {
            suffix.is_empty()
                || suffix
                    .chars()
                    .all(|ch| ch.is_ascii_digit() || matches!(ch, '+' | '-'))
        })
}

#[derive(Clone, Copy)]
enum TomlMultilineQuote {
    Basic,
    Literal,
}

impl TomlMultilineQuote {
    fn delimiter(self) -> &'static [u8; 3] {
        match self {
            Self::Basic => b"\"\"\"",
            Self::Literal => b"'''",
        }
    }
}

fn mask_toml_multiline_strings(line: &str, active: &mut Option<TomlMultilineQuote>) -> String {
    let bytes = line.as_bytes();
    let mut masked = bytes.to_vec();
    let mut cursor = 0usize;
    let mut quote = None;
    let mut escaped = false;

    while cursor < bytes.len() {
        if let Some(multiline) = *active {
            let delimiter = multiline.delimiter();
            if let Some(close) = find_bytes(bytes, cursor, delimiter) {
                masked[cursor..close + delimiter.len()].fill(b' ');
                cursor = close + delimiter.len();
                *active = None;
                continue;
            }
            masked[cursor..].fill(b' ');
            break;
        }

        let byte = bytes[cursor];
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                cursor += 1;
                continue;
            }
            if active_quote == b'"' && byte == b'\\' {
                escaped = true;
                cursor += 1;
                continue;
            }
            if byte == active_quote {
                quote = None;
            }
            cursor += 1;
            continue;
        }
        if byte == b'#' {
            break;
        }
        let multiline = if bytes[cursor..].starts_with(b"\"\"\"") {
            Some(TomlMultilineQuote::Basic)
        } else if bytes[cursor..].starts_with(b"'''") {
            Some(TomlMultilineQuote::Literal)
        } else {
            None
        };
        if let Some(multiline) = multiline {
            let delimiter = multiline.delimiter();
            let content_start = cursor + delimiter.len();
            if let Some(close) = find_bytes(bytes, content_start, delimiter) {
                masked[cursor..close + delimiter.len()].fill(b' ');
                cursor = close + delimiter.len();
            } else {
                masked[cursor..].fill(b' ');
                *active = Some(multiline);
                break;
            }
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        }
        cursor += 1;
    }

    String::from_utf8(masked).expect("masking UTF-8 source with ASCII spaces preserves UTF-8")
}

fn find_bytes(haystack: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(start..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}

fn analyze_shell_line(line: &str, state: &mut ShellLexicalState) -> ShellLineAnalysis {
    let bytes = line.as_bytes();
    let mut masked = bytes.to_vec();
    let mut heredocs = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if let Some(active_quote) = state.quote {
            masked[cursor] = b' ';
            match active_quote {
                ShellQuote::Single => {
                    if byte == b'\'' {
                        state.quote = None;
                    }
                    cursor += 1;
                }
                ShellQuote::Double => {
                    if byte == b'\\' {
                        cursor = mask_shell_escape(line, &mut masked, cursor);
                    } else {
                        if byte == b'"' {
                            state.quote = None;
                        }
                        cursor += 1;
                    }
                }
            }
            continue;
        }
        if state.arithmetic_paren_depth > 0 {
            masked[cursor] = b' ';
            if byte == b'\\' {
                cursor = mask_shell_escape(line, &mut masked, cursor);
            } else {
                match byte {
                    b'\'' => state.quote = Some(ShellQuote::Single),
                    b'"' => state.quote = Some(ShellQuote::Double),
                    b'(' => state.arithmetic_paren_depth += 1,
                    b')' => state.arithmetic_paren_depth -= 1,
                    _ => {}
                }
                cursor += 1;
            }
            continue;
        }
        if byte == b'#' {
            masked[cursor..].fill(b' ');
            break;
        }
        if byte == b'\\' {
            cursor = mask_shell_escape(line, &mut masked, cursor);
            continue;
        }
        if byte == b'\'' {
            masked[cursor] = b' ';
            state.quote = Some(ShellQuote::Single);
            cursor += 1;
            continue;
        }
        if byte == b'"' {
            masked[cursor] = b' ';
            state.quote = Some(ShellQuote::Double);
            cursor += 1;
            continue;
        }
        let arithmetic_opener_len = if bytes[cursor..].starts_with(b"$((") {
            Some(3)
        } else if bytes[cursor..].starts_with(b"((") {
            Some(2)
        } else {
            None
        };
        if let Some(opener_len) = arithmetic_opener_len {
            masked[cursor..cursor + opener_len].fill(b' ');
            state.arithmetic_paren_depth = 2;
            cursor += opener_len;
            continue;
        }
        if bytes[cursor..].starts_with(b"<<<") {
            cursor += 3;
            continue;
        }
        if !bytes[cursor..].starts_with(b"<<") {
            cursor += 1;
            continue;
        }
        cursor += 2;
        let strip_tabs = bytes.get(cursor) == Some(&b'-');
        if strip_tabs {
            cursor += 1;
        }
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let delimiter_quote = bytes
            .get(cursor)
            .copied()
            .filter(|byte| matches!(byte, b'\'' | b'"'));
        if delimiter_quote.is_some() {
            cursor += 1;
        }
        let start = cursor;
        while let Some(byte) = bytes.get(cursor) {
            if delimiter_quote.is_some_and(|quote| *byte == quote)
                || delimiter_quote.is_none()
                    && (byte.is_ascii_whitespace()
                        || matches!(*byte, b';' | b'|' | b'&' | b'<' | b'>'))
            {
                break;
            }
            cursor += 1;
        }
        if cursor > start {
            heredocs.push((line[start..cursor].to_string(), strip_tabs));
        }
        if delimiter_quote.is_some() && bytes.get(cursor) == delimiter_quote.as_ref() {
            cursor += 1;
        }
    }
    ShellLineAnalysis {
        masked: String::from_utf8(masked)
            .expect("masking UTF-8 shell source with ASCII spaces preserves UTF-8"),
        heredocs,
    }
}

fn mask_shell_escape(line: &str, masked: &mut [u8], cursor: usize) -> usize {
    masked[cursor] = b' ';
    let escaped_start = cursor + 1;
    let Some(escaped) = line[escaped_start..].chars().next() else {
        return line.len();
    };
    let escaped_end = escaped_start + escaped.len_utf8();
    masked[escaped_start..escaped_end].fill(b' ');
    escaped_end
}

fn mask_powershell_block_comments(line: &str, depth: &mut usize) -> String {
    let bytes = line.as_bytes();
    let mut masked = bytes.to_vec();
    let mut cursor = 0usize;
    let mut quote = None;
    let mut escaped = false;
    while cursor < bytes.len() {
        if *depth > 0 {
            if bytes[cursor..].starts_with(b"<#") {
                masked[cursor..cursor + 2].fill(b' ');
                *depth += 1;
                cursor += 2;
            } else if bytes[cursor..].starts_with(b"#>") {
                masked[cursor..cursor + 2].fill(b' ');
                *depth -= 1;
                cursor += 2;
            } else {
                masked[cursor] = b' ';
                cursor += 1;
            }
            continue;
        }
        let byte = bytes[cursor];
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'`' {
                escaped = true;
            } else if byte == active_quote {
                quote = None;
            }
            cursor += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
            cursor += 1;
            continue;
        }
        if bytes[cursor..].starts_with(b"<#") {
            masked[cursor..cursor + 2].fill(b' ');
            *depth = 1;
            cursor += 2;
            continue;
        }
        cursor += 1;
    }
    String::from_utf8(masked).expect("masking UTF-8 source with ASCII spaces preserves UTF-8")
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

fn shell_anchor(
    masked: &str,
    original: &str,
) -> Option<(NodeKind, &'static str, String, usize, usize)> {
    if let Some(rest) = masked.strip_prefix("function ") {
        let name = script_identifier(rest)?;
        let offset = masked.find(name)?;
        return Some((
            NodeKind::FUNCTION,
            "function",
            original.get(offset..offset + name.len())?.to_string(),
            offset,
            name.len(),
        ));
    }
    if let Some(paren) = masked.find("()") {
        let raw = masked[..paren].trim();
        if is_script_identifier(raw) {
            let offset = masked.find(raw)?;
            return Some((
                NodeKind::FUNCTION,
                "function",
                original.get(offset..offset + raw.len())?.to_string(),
                offset,
                raw.len(),
            ));
        }
    }
    for prefix in ["source ", ". ", "autoload "] {
        if masked.strip_prefix(prefix).is_some() {
            let raw = original
                .get(prefix.len()..)?
                .split_ascii_whitespace()
                .next()?;
            let label = unquote_label(raw).to_string();
            if label.is_empty() {
                return None;
            }
            let offset = original.find(raw)?;
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
