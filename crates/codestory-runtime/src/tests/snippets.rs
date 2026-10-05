use super::{
    DIRECT_SNIPPET_MAX_BYTES, DIRECT_SNIPPET_TRUNCATION_SUFFIX, bounded_direct_markdown_snippet,
    bounded_markdown_snippet_from_path, tempdir,
};

#[test]
fn direct_markdown_snippet_is_byte_capped() {
    let text = (0..10_000)
        .map(|idx| format!("line {idx}: {}", "x".repeat(2_048)))
        .collect::<Vec<_>>()
        .join("\n");

    let snippet = bounded_direct_markdown_snippet(&text, Some(5_000), usize::MAX);

    assert!(snippet.truncated);
    assert!(snippet.markdown.len() <= DIRECT_SNIPPET_MAX_BYTES);
    assert!(
        snippet.markdown.contains("snippet truncated by byte cap"),
        "{}",
        snippet.markdown
    );
    assert!(
        snippet.markdown.ends_with("```"),
        "truncated snippet should keep a balanced closing fence:\n{}",
        snippet.markdown
    );
}

#[test]
fn file_backed_snippet_streams_and_caps_long_lines() {
    let temp = tempdir().expect("temp dir");
    let source_path = temp.path().join("long_line.rs");
    std::fs::write(
        &source_path,
        format!("pub fn alpha() {{}}\n// {}\n", "x".repeat(256 * 1024)),
    )
    .expect("write long line source");

    let snippet = bounded_markdown_snippet_from_path(
        &source_path,
        2,
        1,
        DIRECT_SNIPPET_MAX_BYTES,
        DIRECT_SNIPPET_TRUNCATION_SUFFIX,
    )
    .expect("read bounded snippet");

    assert!(snippet.truncated);
    assert!(snippet.markdown.len() <= DIRECT_SNIPPET_MAX_BYTES);
    assert!(snippet.markdown.ends_with("```"));
}

#[test]
fn path_windows_share_one_scan_and_preserve_physical_lf_and_utf8() {
    use sha2::{Digest, Sha256};
    let source = "alpha\r\nβ\u{2028}gamma\nlast";
    let mut reader = std::io::BufReader::with_capacity(1, std::io::Cursor::new(source.as_bytes()));
    let windows =
        crate::snippets::read_path_source_windows(&mut reader, &[(1, 2), (2, 9)], 1024).unwrap();
    let first = windows[0].render("src/x.rs".into(), 1, 2, 1024);
    let second = windows[1].render("src/x.rs".into(), 2, 9, 1024);
    assert!(first.snippet.contains("alpha\r\n"));
    assert!(first.snippet.contains("β\u{2028}gamma"));
    assert_eq!(first.delivered_end_line, Some(2));
    assert_eq!(
        first.source_range_sha256,
        Some(format!(
            "{:x}",
            Sha256::digest("alpha\r\nβ\u{2028}gamma\n".as_bytes())
        ))
    );
    assert_eq!(second.delivered_start_line, Some(2));
    assert_eq!(second.delivered_end_line, Some(3));
    assert_eq!(second.last_line_terminated, Some(false));
    assert_eq!(second.source_total_lines, Some(3));
    assert!(second.complete);
    assert_eq!(reader.into_inner().position(), source.len() as u64);
}

#[test]
fn path_windows_budget_omits_whole_lines_and_names_the_actual_continuation() {
    let windows = crate::snippets::read_path_source_windows(
        std::io::Cursor::new(b"one\ntwo\nthree\n"),
        &[(1, 3)],
        1024,
    )
    .unwrap();
    let window = windows[0].render("x.rs".into(), 1, 3, 24);
    assert_eq!(window.delivered_start_line, Some(1));
    assert_eq!(window.delivered_end_line, Some(1));
    assert_eq!(window.next_start_line, Some(2));
    assert!(window.truncated && !window.complete);
    assert!(window.snippet.ends_with("```"));
    assert!(!window.snippet.contains("two"));
    let empty = windows[0].render("x.rs".into(), 1, 3, 0);
    assert!(empty.snippet.is_empty());
    assert_eq!(empty.delivered_end_line, None);
    assert_eq!(empty.next_start_line, Some(1));
    assert!(!empty.complete);
}

#[test]
fn path_windows_oversized_line_is_explicit_and_empty_eof_is_not_an_omission() {
    use codestory_contracts::api::SourceRangeOmissionDto;
    let bytes = format!("{}\nok\n", "β".repeat(256));
    let windows = crate::snippets::read_path_source_windows(
        std::io::Cursor::new(bytes.as_bytes()),
        &[(1, 1), (2, 9), (9, 9)],
        128,
    )
    .unwrap();
    let large = windows[0].render("x.rs".into(), 1, 1, 128);
    assert_eq!(
        large.omission_reason,
        Some(SourceRangeOmissionDto::LineExceedsByteBudget)
    );
    assert_eq!(large.next_start_line, Some(1));
    assert!(large.snippet.is_empty());
    let later = windows[1].render("x.rs".into(), 2, 9, 128);
    assert_eq!(later.delivered_start_line, Some(2));
    assert_eq!(later.source_total_lines, Some(2));
    assert!(later.complete);
    let beyond = windows[2].render("x.rs".into(), 9, 9, 128);
    assert!(beyond.complete && beyond.snippet.is_empty());
    assert_eq!(beyond.next_start_line, None);
    let empty =
        crate::snippets::read_path_source_windows(std::io::Cursor::new(b""), &[(1, 1)], 128)
            .unwrap();
    assert_eq!(
        empty[0].render("x".into(), 1, 1, 128).source_total_lines,
        Some(0)
    );
}

#[test]
fn path_windows_shared_budget_keeps_later_omission_receipts() {
    let windows = crate::snippets::read_path_source_windows(
        std::io::Cursor::new(b"a\n"),
        &[(1, 1), (1, 1), (1, 1)],
        1024,
    )
    .unwrap();
    let result = crate::snippets::render_path_source_windows(
        windows
            .into_iter()
            .map(|window| ("x.rs".into(), 1, 1, window)),
        22,
    );
    assert_eq!(result.len(), 3);
    assert!(result[0].complete);
    assert_eq!(result[0].snippet.len(), 22);
    assert!(result[1..].iter().all(|window| !window.complete
        && window.snippet.is_empty()
        && window.delivered_start_line.is_none()
        && window.next_start_line == Some(1)));
    assert_eq!(
        result
            .iter()
            .map(|window| window.snippet.len())
            .sum::<usize>(),
        22
    );
}

#[test]
fn path_window_complete_does_not_invent_whole_file_coverage() {
    let windows =
        crate::snippets::read_path_source_windows(std::io::Cursor::new(b"a\nb\n"), &[(1, 1)], 1024)
            .unwrap();
    let result = windows[0].render("x.rs".into(), 1, 1, 1024);
    assert!(result.complete);
    assert_eq!(result.source_total_lines, None);
    assert_eq!(result.delivered_end_line, Some(1));
    assert_eq!(result.last_line_terminated, Some(true));
}

#[test]
fn path_window_rejects_invalid_utf8_without_a_lossy_source_claim() {
    let result = crate::snippets::read_path_source_windows(
        std::io::Cursor::new([0xff, b'\n']),
        &[(1, 1)],
        1024,
    );
    assert!(matches!(result, Err(error) if error.kind() == std::io::ErrorKind::InvalidData));
}

#[test]
fn path_window_rejects_known_nonregular_path_before_invoking_the_opener() {
    let directory = tempdir().unwrap();
    let mut opened = false;
    let result = crate::controller_files::open_regular_source_file_with(directory.path(), |_| {
        opened = true;
        panic!("a known nonregular path must be rejected before any potentially blocking open");
    });
    assert!(!opened);
    assert!(matches!(result, Err(error) if error.kind() == std::io::ErrorKind::InvalidInput));
    let file = directory.path().join("regular.rs");
    std::fs::write(&file, "source").unwrap();
    let opened = crate::controller_files::open_regular_source_file_with(&file, |path| {
        std::fs::File::open(path)
    })
    .unwrap();
    assert!(opened.metadata().unwrap().is_file());
}
