use super::{BufRead, Path};
use std::fmt::Write as _;
use std::io;

pub(super) const DIRECT_SNIPPET_CONTEXT_LINE_CAP: usize = 50;
pub(crate) const DIRECT_SNIPPET_MAX_BYTES: usize = 64 * 1024;
pub(crate) const DIRECT_SNIPPET_TRUNCATION_SUFFIX: &str =
    "\n... snippet truncated by byte cap\n```";

#[derive(Debug, Clone)]
pub(crate) struct BoundedSnippet {
    pub(crate) markdown: String,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BoundedSnippetRangeOptions<'a> {
    pub(crate) focus_line: u32,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    pub(crate) context_lines: usize,
    pub(crate) max_bytes: usize,
    pub(crate) truncation_suffix: &'a str,
}

#[cfg(test)]
pub(crate) fn markdown_snippet(text: &str, focus_line: Option<u32>, context: usize) -> String {
    let all_lines: Vec<&str> = text.lines().collect();
    if all_lines.is_empty() {
        return String::new();
    }

    let line_index = focus_line
        .and_then(|line| line.checked_sub(1))
        .map(|line| line as usize)
        .unwrap_or(0)
        .min(all_lines.len().saturating_sub(1));

    let start = line_index.saturating_sub(context);
    let end = (line_index + context + 1).min(all_lines.len());

    let mut out = String::new();
    out.push_str("```text\n");
    for (idx, line) in all_lines[start..end].iter().enumerate() {
        let source_line = start + idx + 1;
        let marker = if source_line == line_index + 1 {
            ">"
        } else {
            " "
        };
        let _ = writeln!(out, "{marker}{source_line:>5} | {line}");
    }
    out.push_str("```");
    out
}

fn truncate_to_byte_cap(mut text: String, max_bytes: usize, suffix: &str) -> BoundedSnippet {
    if text.len() <= max_bytes {
        return BoundedSnippet {
            markdown: text,
            truncated: false,
        };
    }

    let mut keep = max_bytes.saturating_sub(suffix.len());
    while keep > 0 && !text.is_char_boundary(keep) {
        keep -= 1;
    }
    text.truncate(keep);
    text.push_str(suffix);
    if text.len() > max_bytes {
        let mut hard_keep = max_bytes;
        while hard_keep > 0 && !text.is_char_boundary(hard_keep) {
            hard_keep -= 1;
        }
        text.truncate(hard_keep);
    }

    BoundedSnippet {
        markdown: text,
        truncated: true,
    }
}

#[cfg(test)]
pub(crate) fn bounded_direct_markdown_snippet(
    text: &str,
    focus_line: Option<u32>,
    context: usize,
) -> BoundedSnippet {
    let markdown = markdown_snippet(
        text,
        focus_line,
        context.min(DIRECT_SNIPPET_CONTEXT_LINE_CAP),
    );
    truncate_to_byte_cap(
        markdown,
        DIRECT_SNIPPET_MAX_BYTES,
        DIRECT_SNIPPET_TRUNCATION_SUFFIX,
    )
}

pub(super) fn bounded_markdown_snippet_from_path(
    path: &Path,
    focus_line: u32,
    context: usize,
    max_bytes: usize,
    truncation_suffix: &str,
) -> io::Result<BoundedSnippet> {
    let file = std::fs::File::open(path)?;
    let mut reader = io::BufReader::new(file);
    let context = context.min(DIRECT_SNIPPET_CONTEXT_LINE_CAP);
    let focus = focus_line.max(1) as usize;
    let start = focus.saturating_sub(context).max(1);
    let end = focus.saturating_add(context);
    let mut line_no = 0usize;
    let mut line = String::new();
    let mut out = String::from("```text\n");
    let mut truncated = false;

    loop {
        let (read, line_truncated) = read_line_capped(&mut reader, &mut line, max_bytes)?;
        if read == 0 {
            break;
        }
        line_no = line_no.saturating_add(1);
        if line_no > end {
            break;
        }
        if line_no >= start {
            truncated |= line_truncated;
            let marker = if line_no == focus { ">" } else { " " };
            let trimmed = line.trim_end_matches(['\r', '\n']);
            let _ = writeln!(out, "{marker}{line_no:>5} | {trimmed}");
        }
    }

    Ok(finish_bounded_file_snippet(
        out,
        truncated,
        max_bytes,
        truncation_suffix,
    ))
}

pub(super) fn bounded_markdown_snippet_range_from_path(
    path: &Path,
    focus_line: u32,
    start_line: u32,
    end_line: u32,
    context: usize,
    max_bytes: usize,
    truncation_suffix: &str,
) -> io::Result<BoundedSnippet> {
    let file = std::fs::File::open(path)?;
    let mut reader = io::BufReader::new(file);
    let context = context.min(DIRECT_SNIPPET_CONTEXT_LINE_CAP) as u32;
    let focus = focus_line.max(1);
    let start = start_line.saturating_sub(context).max(1);
    let end = end_line.max(start_line).saturating_add(context);
    let mut line_no = 0u32;
    let mut line = String::new();
    let mut out = String::from("```text\n");
    let mut truncated = false;

    loop {
        let (read, line_truncated) = read_line_capped(&mut reader, &mut line, max_bytes)?;
        if read == 0 {
            break;
        }
        line_no = line_no.saturating_add(1);
        if line_no > end {
            break;
        }
        if line_no >= start {
            truncated |= line_truncated;
            let marker = if line_no == focus { ">" } else { " " };
            let trimmed = line.trim_end_matches(['\r', '\n']);
            let _ = writeln!(out, "{marker}{line_no:>5} | {trimmed}");
        }
    }

    Ok(finish_bounded_file_snippet(
        out,
        truncated,
        max_bytes,
        truncation_suffix,
    ))
}

fn finish_bounded_file_snippet(
    mut out: String,
    truncated: bool,
    max_bytes: usize,
    truncation_suffix: &str,
) -> BoundedSnippet {
    if out == "```text\n" {
        return BoundedSnippet {
            markdown: String::new(),
            truncated: false,
        };
    }
    out.push_str("```");
    if out.len() > max_bytes {
        return truncate_to_byte_cap(out, max_bytes, truncation_suffix);
    }
    if truncated {
        if out.ends_with("```") {
            out.truncate(out.len().saturating_sub(3));
        }
        if out.len().saturating_add(truncation_suffix.len()) <= max_bytes {
            out.push_str(truncation_suffix);
            return BoundedSnippet {
                markdown: out,
                truncated: true,
            };
        }
        return truncate_to_byte_cap(out, max_bytes, truncation_suffix);
    }
    BoundedSnippet {
        markdown: out,
        truncated: false,
    }
}

fn read_line_capped<R: BufRead>(
    reader: &mut R,
    out: &mut String,
    max_line_bytes: usize,
) -> io::Result<(usize, bool)> {
    out.clear();
    let mut total = 0usize;
    let mut truncated = false;

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok((total, truncated));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take_len = newline.map(|pos| pos + 1).unwrap_or(available.len());
        let chunk = &available[..take_len];
        total = total.saturating_add(chunk.len());

        if out.len() < max_line_bytes {
            let remaining = max_line_bytes - out.len();
            let copy_len = chunk.len().min(remaining);
            out.push_str(&String::from_utf8_lossy(&chunk[..copy_len]));
            truncated |= copy_len < chunk.len();
        } else if !chunk.is_empty() {
            truncated = true;
        }

        reader.consume(take_len);
        if newline.is_some() {
            return Ok((total, truncated));
        }
    }
}
