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
