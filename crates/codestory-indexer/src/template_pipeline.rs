//! Whitespace-blanking pipeline for Vue, Svelte, and Astro single-file components.
//!
//! Non-script regions are replaced with spaces (preserving newlines and byte length) so
//! JavaScript/TypeScript tree-sitter graph rules can run on the full file while keeping
//! absolute source positions aligned with the original template.

use std::path::Path;

/// Which template dialect applies to a file path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    Vue,
    Svelte,
    Astro,
}

/// Byte range in the original source (inclusive start, exclusive end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

/// A `<style>` block extracted for future CSS structural collection.
#[derive(Debug, Clone)]
pub struct StyleBlockRange {
    pub range: ByteRange,
    pub content: String,
    pub start_line: u32,
    pub start_col: u32,
}

/// Output of preparing a template file for JS/TS graph indexing.
#[derive(Debug, Clone)]
pub struct TemplatePrepareResult {
    pub blanked: String,
    /// `javascript` or `typescript` — selects tree-sitter ruleset.
    pub script_language: &'static str,
    pub style_blocks: Vec<StyleBlockRange>,
}

pub fn template_kind_for_path(path: &Path) -> Option<TemplateKind> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "vue" => Some(TemplateKind::Vue),
        "svelte" => Some(TemplateKind::Svelte),
        "astro" => Some(TemplateKind::Astro),
        _ => None,
    }
}

pub fn template_surface_language(path: &Path) -> Option<&'static str> {
    template_kind_for_path(path).map(|kind| match kind {
        TemplateKind::Vue => "vue",
        TemplateKind::Svelte => "svelte",
        TemplateKind::Astro => "astro",
    })
}

pub fn prepare_template_source(kind: TemplateKind, source: &str) -> TemplatePrepareResult {
    let mut keep_ranges = Vec::new();
    let mut style_blocks = Vec::new();
    let mut script_language = "javascript";

    collect_script_blocks(source, &mut keep_ranges, &mut script_language);
    collect_style_blocks(source, &mut style_blocks);

    if matches!(kind, TemplateKind::Astro) {
        collect_astro_frontmatter(source, &mut keep_ranges, &mut script_language);
    }
    if matches!(kind, TemplateKind::Svelte) {
        let script_ranges = keep_ranges.clone();
        collect_svelte_inline_expressions(source, &script_ranges, &mut keep_ranges);
    }

    keep_ranges.sort_by_key(|range| range.start);
    keep_ranges = merge_ranges(keep_ranges);

    let blanked = blank_source(source, &keep_ranges);

    TemplatePrepareResult {
        blanked,
        script_language,
        style_blocks,
    }
}

/// Delegates embedded `<style>` blocks to the structural CSS entity collector.
pub fn delegate_template_style_blocks(
    path: &Path,
    blocks: &[StyleBlockRange],
    file_id: codestory_contracts::graph::NodeId,
    storage: &mut crate::intermediate_storage::IntermediateStorage,
) {
    for block in blocks {
        crate::structural::collect_embedded_style_css(
            path,
            &block.content,
            file_id,
            storage,
            block.start_line,
            block.start_col,
        );
    }
}

fn collect_script_blocks(
    source: &str,
    keep_ranges: &mut Vec<ByteRange>,
    script_language: &mut &'static str,
) {
    let bytes = source.as_bytes();
    let lower = source.to_ascii_lowercase();
    let mut search_from = 0usize;

    while let Some(rel) = lower[search_from..].find("<script") {
        let start = search_from + rel;
        let Some(open_end) = lower[start..].find('>') else {
            break;
        };
        let open_end = start + open_end + 1;
        let Some(close_rel) = lower[open_end..].find("</script>") else {
            break;
        };
        let close_start = open_end + close_rel;
        let close_end = close_start + "</script>".len();
        keep_ranges.push(ByteRange {
            start: open_end,
            end: close_start.min(bytes.len()),
        });
        if let Some(lang) = script_lang_from_opening_tag(&source[start..open_end]) {
            *script_language = lang;
        }
        search_from = close_end;
    }
}

fn collect_style_blocks(source: &str, style_blocks: &mut Vec<StyleBlockRange>) {
    let lower = source.to_ascii_lowercase();
    let mut search_from = 0usize;

    while let Some(rel) = lower[search_from..].find("<style") {
        let start = search_from + rel;
        let Some(open_end) = lower[start..].find('>') else {
            break;
        };
        let open_end = start + open_end + 1;
        let Some(close_rel) = lower[open_end..].find("</style>") else {
            break;
        };
        let close_start = open_end + close_rel;
        let close_end = close_start + "</style>".len();
        let end = close_end.min(source.len());
        let (start_line, start_col) = crate::structural::byte_offset_line_col(source, open_end);
        style_blocks.push(StyleBlockRange {
            range: ByteRange { start, end },
            content: source[open_end..close_start].to_string(),
            start_line,
            start_col,
        });
        search_from = close_end;
    }
}

fn collect_astro_frontmatter(
    source: &str,
    keep_ranges: &mut Vec<ByteRange>,
    script_language: &mut &'static str,
) {
    let trimmed = source.trim_start();
    if !trimmed.starts_with("---") {
        return;
    }
    let leading = source.len() - trimmed.len();
    let after_open = leading + 3;
    let Some(first_newline) = source[after_open..].find('\n') else {
        return;
    };
    let content_start = after_open + first_newline + 1;
    let Some(close_rel) = source[content_start..].find("\n---") else {
        return;
    };
    let close_line_start = content_start + close_rel;
    keep_ranges.push(ByteRange {
        start: content_start,
        end: close_line_start,
    });
    if source[content_start..close_line_start].contains("lang=\"ts\"")
        || source[content_start..close_line_start].contains("lang='ts'")
    {
        *script_language = "typescript";
    }
}

fn collect_svelte_inline_expressions(
    source: &str,
    script_ranges: &[ByteRange],
    keep_ranges: &mut Vec<ByteRange>,
) {
    let bytes = source.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if in_any_range(index, script_ranges) {
            index += 1;
            continue;
        }
        if bytes[index] != b'{' {
            index += 1;
            continue;
        }
        if let Some(end) = find_brace_expression_end(source, index) {
            keep_ranges.push(ByteRange { start: index, end });
            index = end;
        } else {
            index += 1;
        }
    }
}

fn find_brace_expression_end(source: &str, open_index: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(open_index) != Some(&b'{') {
        return None;
    }
    let mut depth = 0i32;
    let mut index = open_index;
    let mut in_string: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(quote) = in_string {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == quote {
                in_string = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => {
                in_string = Some(byte);
                index += 1;
            }
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth -= 1;
                index += 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => index += 1,
        }
    }
    None
}

fn script_lang_from_opening_tag(opening_tag: &str) -> Option<&'static str> {
    let lower = opening_tag.to_ascii_lowercase();
    if lower.contains("lang=\"ts\"")
        || lower.contains("lang='ts'")
        || lower.contains("lang=ts")
        || lower.contains("lang=\"typescript\"")
        || lower.contains("lang='typescript'")
        || lower.contains("lang=typescript")
    {
        Some("typescript")
    } else {
        None
    }
}

fn blank_source(source: &str, keep_ranges: &[ByteRange]) -> String {
    let bytes = source.as_bytes();
    let mut out = vec![b' '; bytes.len()];
    for (offset, byte) in bytes.iter().copied().enumerate() {
        if byte == b'\n' || byte == b'\r' {
            out[offset] = byte;
        }
    }
    for range in keep_ranges {
        let start = range.start.min(bytes.len());
        let end = range.end.min(bytes.len());
        out[start..end].copy_from_slice(&bytes[start..end]);
    }
    String::from_utf8(out).unwrap_or_else(|_| source.to_string())
}

fn merge_ranges(ranges: Vec<ByteRange>) -> Vec<ByteRange> {
    if ranges.is_empty() {
        return ranges;
    }
    let mut merged = Vec::new();
    let mut current = ranges[0];
    for next in ranges.into_iter().skip(1) {
        if next.start <= current.end {
            current.end = current.end.max(next.end);
        } else {
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);
    merged
}

fn in_any_range(index: usize, ranges: &[ByteRange]) -> bool {
    ranges
        .iter()
        .any(|range| index >= range.start && index < range.end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{get_language_for_ext, index_file};
    use std::path::Path;

    fn symbol_line_col(result: &crate::IndexResult, name: &str) -> Option<(u32, u32)> {
        result
            .nodes
            .iter()
            .find(|node| node.serialized_name == name)
            .and_then(|node| Some((node.start_line?, node.start_col?)))
    }

    fn assert_symbol_on_original_source_line(
        source: &str,
        result: &crate::IndexResult,
        name: &str,
    ) {
        let (line, col) =
            symbol_line_col(result, name).unwrap_or_else(|| panic!("missing symbol {name}"));
        let line_text = source
            .lines()
            .nth(line as usize - 1)
            .unwrap_or_else(|| panic!("line {line} out of range for {name}"));
        assert!(
            line_text.contains(name),
            "expected {name} on original line {line} ({line_text:?}), got col {col}"
        );
        let name_start = line_text
            .find(name)
            .expect("name on line")
            .saturating_add(1) as u32;
        assert!(
            col <= name_start,
            "column for {name} should not be past identifier start (col {col}, name starts {name_start})"
        );
    }

    #[test]
    fn test_blank_preserves_byte_length_and_newlines() {
        let source = "<template>\n  <p>x</p>\n</template>\n<script>\nconst a = 1\n</script>\n";
        let prepared = prepare_template_source(TemplateKind::Vue, source);
        assert_eq!(prepared.blanked.len(), source.len());
        assert!(prepared.blanked.contains("const a = 1"));
        assert!(prepared.blanked.starts_with(' '));
        assert_eq!(
            prepared.blanked.chars().filter(|c| *c == '\n').count(),
            source.chars().filter(|c| *c == '\n').count()
        );
    }

    #[test]
    fn test_vue_script_symbols_match_original_line_columns() -> anyhow::Result<()> {
        let source = r#"<template>
  <p>{{ title }}</p>
</template>
<script setup lang="ts">
export const title = 'Hello'
export function greet(name: string) {
  return name
}
</script>
<style scoped>
.title { color: red; }
</style>
"#;
        let prepared = prepare_template_source(TemplateKind::Vue, source);
        assert_eq!(prepared.script_language, "typescript");
        assert_eq!(prepared.style_blocks.len(), 1);

        let language_config = get_language_for_ext("ts").expect("typescript config");
        let result = index_file(
            Path::new("App.vue"),
            &prepared.blanked,
            &language_config,
            None,
            None,
        )?;

        assert_symbol_on_original_source_line(source, &result, "greet");
        Ok(())
    }

    #[test]
    fn test_svelte_script_and_inline_expression_positions() -> anyhow::Result<()> {
        let source = r#"<script>
  export let count = 0
  export function bump() {
    count += 1
  }
</script>

<button on:click={bump}>{count}</button>

<style>
  button { font-weight: bold; }
</style>
"#;
        let prepared = prepare_template_source(TemplateKind::Svelte, source);
        let language_config = get_language_for_ext("js").expect("javascript config");
        let result = index_file(
            Path::new("Widget.svelte"),
            &prepared.blanked,
            &language_config,
            None,
            None,
        )?;

        assert_symbol_on_original_source_line(source, &result, "bump");
        Ok(())
    }

    #[test]
    fn test_astro_frontmatter_symbols_match_original_positions() -> anyhow::Result<()> {
        let source = r#"---
export function buildTitle(page: string) {
  return `Page: ${page}`
}
const site = 'codestory'
---
<html>
  <body>{site}</body>
</html>
"#;
        let prepared = prepare_template_source(TemplateKind::Astro, source);
        let language_config = get_language_for_ext("ts").expect("typescript config");
        let result = index_file(
            Path::new("src/pages/index.astro"),
            &prepared.blanked,
            &language_config,
            None,
            None,
        )?;

        assert_symbol_on_original_source_line(source, &result, "buildTitle");
        Ok(())
    }

    #[test]
    fn test_template_script_language_typescript_from_lang_attribute() {
        let source = r#"<script lang="ts">export const x = 1</script>"#;
        let prepared = prepare_template_source(TemplateKind::Vue, source);
        assert_eq!(prepared.script_language, "typescript");
    }
}
