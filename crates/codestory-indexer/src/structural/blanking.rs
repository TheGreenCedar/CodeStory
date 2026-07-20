//! Whitespace-blanking for embedded `<script>` / `<style>` regions (byte-length preserving).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedRegionKind {
    Script,
    Style,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedRegion {
    pub kind: EmbeddedRegionKind,
    pub open_start_byte: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: u32,
    pub start_col: u32,
}

/// Regions between paired tags (`<script>...</script>`, `<style>...</style>`), case-insensitive.
pub fn extract_embedded_regions(source: &str) -> Vec<EmbeddedRegion> {
    let lower = source.to_ascii_lowercase();
    let mut regions = Vec::new();
    for (open_tag, close_tag, kind) in [
        ("<script", "</script>", EmbeddedRegionKind::Script),
        ("<style", "</style>", EmbeddedRegionKind::Style),
    ] {
        let mut search_from = 0usize;
        while let Some(open_rel) = lower[search_from..].find(open_tag) {
            let open_start = search_from + open_rel;
            let content_start = lower[open_start..]
                .find('>')
                .map(|idx| open_start + idx + 1)
                .unwrap_or(open_start);
            let close_rel = lower[content_start..].find(close_tag);
            let Some(close_rel) = close_rel else {
                break;
            };
            let content_end = content_start + close_rel;
            let (start_line, start_col) = byte_offset_line_col(source, content_start);
            regions.push(EmbeddedRegion {
                kind,
                open_start_byte: open_start,
                start_byte: content_start,
                end_byte: content_end,
                start_line,
                start_col,
            });
            search_from = content_end + close_tag.len();
        }
    }
    regions.sort_by_key(|region| region.start_byte);
    regions
}

/// Replace every byte outside `keep` regions with ASCII space; newlines and length are preserved.
pub fn blank_outside_regions(source: &str, keep: &[EmbeddedRegion]) -> String {
    if keep.is_empty() {
        return " ".repeat(source.len());
    }
    let mut out = source.to_owned();
    let mut cursor = 0usize;
    for region in keep {
        let keep_start = region.start_byte.min(out.len());
        let keep_end = region.end_byte.min(out.len());
        if cursor < keep_start {
            blank_range(&mut out, cursor, keep_start);
        }
        cursor = keep_end;
    }
    let end = out.len();
    if cursor < end {
        blank_range(&mut out, cursor, end);
    }
    out
}

/// Keep only script bytes; blank everything else (for delegated JS/TS graph parse).
pub fn blank_non_script_regions(source: &str) -> String {
    let regions: Vec<_> = extract_embedded_regions(source)
        .into_iter()
        .filter(|region| region.kind == EmbeddedRegionKind::Script)
        .collect();
    blank_outside_regions(source, &regions)
}

/// Extract inner text for each `<style>` block.
pub fn extract_style_block_sources(source: &str) -> Vec<(u32, u32, String)> {
    extract_embedded_regions(source)
        .into_iter()
        .filter(|region| region.kind == EmbeddedRegionKind::Style)
        .map(|region| {
            let slice = source
                .get(region.start_byte..region.end_byte)
                .unwrap_or_default();
            (region.start_line, region.start_col, slice.to_string())
        })
        .collect()
}

pub(crate) fn byte_offset_line_col(source: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(source.len());
    let prefix = &source.as_bytes()[..offset];
    let line = prefix
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        .saturating_add(1)
        .try_into()
        .unwrap_or(u32::MAX);
    let line_start = prefix
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index.saturating_add(1));
    let col = offset
        .saturating_sub(line_start)
        .saturating_add(1)
        .try_into()
        .unwrap_or(u32::MAX);
    (line, col)
}

fn blank_range(out: &mut str, start: usize, end: usize) {
    // SAFETY: we only replace ASCII spaces/newlines in place; UTF-8 scalar boundaries stay valid.
    let bytes = unsafe { out.as_bytes_mut() };
    for byte in &mut bytes[start..end] {
        if *byte != b'\n' && *byte != b'\r' {
            *byte = b' ';
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blanking_preserves_length_and_newlines() {
        let source = "<div>\n  <script>\nlet x = 1;\n</script>\n</div>";
        let regions = extract_embedded_regions(source);
        let blanked = blank_outside_regions(source, &regions);
        assert_eq!(blanked.len(), source.len());
        assert!(blanked.contains("let x = 1;"));
        assert!(blanked.contains("let x = 1;"));
        assert!(blanked.as_bytes()[0] == b' ');
        assert!(blanked.as_bytes()[4] == b' ');
    }

    #[test]
    fn extracts_script_and_style_regions() {
        let source = "<style>.a{}</style><script>const a=1</script>";
        let regions = extract_embedded_regions(source);
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].kind, EmbeddedRegionKind::Style);
        assert_eq!(regions[1].kind, EmbeddedRegionKind::Script);
    }
}
