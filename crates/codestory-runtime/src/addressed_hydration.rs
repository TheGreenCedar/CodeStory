//! Benchmark-only hydration. Callers supply source authenticated to their core
//! pin; this boundary rechecks the content and coordinate binding before slicing.
//! It cannot query, select identities, traverse relations, or inspect wording.

use codestory_contracts::evidence_address::{ByteRangeV1, LineRangeV1, SourceRangeV1};
use sha2::{Digest, Sha256};
use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressedHydrationGap {
    ContentChanged,
    InvalidCoordinates,
    PathMismatch,
    SourceBudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HydratedAddressedRange {
    pub range: SourceRangeV1,
    /// Verbatim source bytes, including the original line endings.
    pub source: String,
    /// Same line-numbered presentation as the current compiler's source input.
    pub markdown: String,
    pub truncated: bool,
}

/// Select the smallest supplied syntax span containing the address. If it does
/// not fit, grow a complete-line window about the address, inside that span.
/// An oversized focus line is a typed gap, never a prefix or header substitute.
pub fn hydrate_addressed_range(
    source: &str,
    focus: &SourceRangeV1,
    syntax: &[SourceRangeV1],
    max_bytes: usize,
) -> Result<HydratedAddressedRange, AddressedHydrationGap> {
    let digest = format!("{:x}", Sha256::digest(source));
    let mut offsets = vec![0usize];
    for line in source.split_inclusive('\n') {
        offsets.push(offsets.last().copied().unwrap_or(0) + line.len());
    }
    validate_range(source, &offsets, &digest, focus)?;
    let mut bounds = (0, offsets.len() - 1);
    let mut best_bytes = u64::MAX;
    for span in syntax {
        if span.path != focus.path {
            return Err(AddressedHydrationGap::PathMismatch);
        }
        validate_range(source, &offsets, &digest, span)?;
        if span.byte_range.start() <= focus.byte_range.start()
            && span.byte_range.end() >= focus.byte_range.end()
        {
            let size = span.byte_range.end() - span.byte_range.start();
            let candidate = (
                span.line_range.start() as usize - 1,
                span.line_range.end() as usize,
            );
            if (size, candidate) < (best_bytes, bounds) {
                best_bytes = size;
                bounds = candidate;
            }
        }
    }
    let centre = (focus.line_range.start() as usize - 1)
        + (focus.line_range.end() - focus.line_range.start()) as usize / 2;
    let line_cost = |index: usize| {
        let text = source[offsets[index]..offsets[index + 1]].trim_end_matches(['\r', '\n']);
        // Marker, line number, separator, newline. Fences cost eleven bytes.
        1 + (index + 1).to_string().len().max(5) + 3 + text.len() + 1
    };
    let mut start = centre;
    let mut end = centre + 1;
    let mut bytes = 11usize.saturating_add(line_cost(centre));
    if bytes > max_bytes {
        return Err(AddressedHydrationGap::SourceBudgetExceeded);
    }
    loop {
        let mut changed = false;
        // Nearest lines first, with a stable left-before-right tie break.
        if start > bounds.0 && bytes.saturating_add(line_cost(start - 1)) <= max_bytes {
            start -= 1;
            bytes += line_cost(start);
            changed = true;
        }
        if end < bounds.1 && bytes.saturating_add(line_cost(end)) <= max_bytes {
            bytes += line_cost(end);
            end += 1;
            changed = true;
        }
        if !changed {
            break;
        }
    }
    let mut markdown = String::with_capacity(bytes);
    markdown.push_str("```text\n");
    for index in start..end {
        let text = source[offsets[index]..offsets[index + 1]].trim_end_matches(['\r', '\n']);
        let marker = if index == centre { '>' } else { ' ' };
        let _ = writeln!(markdown, "{marker}{:>5} | {text}", index + 1);
    }
    markdown.push_str("```");
    Ok(HydratedAddressedRange {
        range: SourceRangeV1 {
            path: focus.path.clone(),
            byte_range: ByteRangeV1::new(offsets[start] as u64, offsets[end] as u64)
                .map_err(|_| AddressedHydrationGap::InvalidCoordinates)?,
            line_range: LineRangeV1::new(start as u32 + 1, end as u32)
                .map_err(|_| AddressedHydrationGap::InvalidCoordinates)?,
            content_digest: focus.content_digest.clone(),
        },
        source: source[offsets[start]..offsets[end]].to_string(),
        markdown,
        truncated: (start, end) != bounds,
    })
}

fn validate_range(
    source: &str,
    offsets: &[usize],
    digest: &str,
    range: &SourceRangeV1,
) -> Result<(), AddressedHydrationGap> {
    if !range.content_digest.as_str().eq_ignore_ascii_case(digest) {
        return Err(AddressedHydrationGap::ContentChanged);
    }
    let start = usize::try_from(range.byte_range.start())
        .map_err(|_| AddressedHydrationGap::InvalidCoordinates)?;
    let end = usize::try_from(range.byte_range.end())
        .map_err(|_| AddressedHydrationGap::InvalidCoordinates)?;
    if end > source.len() || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
        return Err(AddressedHydrationGap::InvalidCoordinates);
    }
    let first_line = offsets.partition_point(|offset| *offset <= start);
    let last_line = offsets.partition_point(|offset| *offset < end);
    if first_line != range.line_range.start() as usize
        || last_line != range.line_range.end() as usize
    {
        return Err(AddressedHydrationGap::InvalidCoordinates);
    }
    Ok(())
}
