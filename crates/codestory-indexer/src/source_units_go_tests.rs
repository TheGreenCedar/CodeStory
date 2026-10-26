//! Test-first Go named-owner coverage; no Go production implementation here.
use super::*;

fn allowance() -> SourceUnitBudget {
    SourceUnitBudget {
        remaining_source_bytes: 2 * 1024 * 1024,
        remaining_work: 8_000_000,
        max_depth: 256,
    }
}

fn compile(source: &str, lines: &[usize]) -> Vec<SourceUnitSelection> {
    compile_source_units(
        "go",
        source,
        lines,
        &mut allowance(),
        &CancellationToken::new(),
    )
    .expect("registered Go grammar must produce complete named-owner source units")
}

fn exact(
    source: &str,
    selection: &SourceUnitSelection,
    body: &str,
    first: usize,
    last: usize,
    kind: &str,
) {
    assert!(selection.gaps.is_empty(), "{:?}", selection.gaps);
    assert_eq!(selection.units.len(), 1);
    let unit = &selection.units[0];
    assert_eq!((unit.start_line, unit.end_line), (first, last));
    assert_eq!(unit.kind, kind);
    assert_eq!(&source[unit.start_byte..unit.end_byte], body);
    assert_eq!(unit.start_byte, source.find(body).unwrap());
    assert_eq!(unit.end_byte - unit.start_byte, body.len());
}

#[test]
fn named_function_and_generic_receiver_method_include_attached_docs_and_complete_closing_row() {
    let function = "// Build returns a value.\n// Its interior comment is literal source.\nfunc Build[T any](value T) T {\n    // interior\n    return value\n} // closing row\n";
    let method = "/* Value returns the stored value. */\nfunc (box *Box[T]) Value() T {\n    return box.value\n}\n";
    let source = format!(
        "package sample\n// Detached documentation.\n\n{function}// Excluded trailing comment.\n\ntype Box[T any] struct {{ value T }}\n{method}"
    );
    let selections = compile(&source, &[5, 8, 9, 3, 13, 14, 16]);
    for selection in &selections[..3] {
        exact(
            &source,
            selection,
            function,
            4,
            9,
            "complete_function_declaration",
        );
    }
    assert!(
        selections[3].units.is_empty(),
        "blank-separated comments are not attached to the next declaration"
    );
    for selection in &selections[4..] {
        exact(
            &source,
            selection,
            method,
            13,
            16,
            "complete_method_declaration",
        );
    }
}

#[test]
fn anonymous_bodies_stay_in_named_outer_and_nonowner_declarations_do_not_become_callables() {
    let outer = "func Outer() {\n    callback := func() int {\n        return 7\n    }\n    _ = callback\n    _ = `func Fake() {}`\n}\n";
    let source = format!(
        "package sample\nimport \"fmt\"\ntype Runner interface {{ Run() }}\ntype Callback func(int) int\nvar global = func() int {{\n    return 9\n}}\n{outer}"
    );
    let selections = compile(&source, &[10, 13, 14, 1, 2, 3, 4, 5, 6]);
    for selection in &selections[..3] {
        exact(
            &source,
            selection,
            outer,
            8,
            14,
            "complete_function_declaration",
        );
    }
    for selection in &selections[3..] {
        assert!(
            selection.units.is_empty(),
            "nonowner line {} fabricated a callable",
            selection.match_line
        );
        assert!(selection.gaps.contains(&SourceUnitGap::NoEnclosingUnit));
    }
}

#[test]
fn crlf_unicode_unterminated_eof_duplicates_and_invalid_lines_keep_exact_physical_extents() {
    let body = "// Echo 雪\r\nfunc Echo() string {\r\n    return \"🙂\"\r\n}";
    let source = format!("package sample\r\n{body}");
    let selections = compile(&source, &[5, 2, 4, 4, 0, 6]);
    for selection in &selections[..4] {
        exact(
            &source,
            selection,
            body,
            2,
            5,
            "complete_function_declaration",
        );
    }
    assert_eq!(selections[2], selections[3]);
    assert_eq!(
        selections.iter().map(|s| s.match_line).collect::<Vec<_>>(),
        [5, 2, 4, 4, 0, 6]
    );
    for selection in &selections[4..] {
        assert!(selection.units.is_empty());
        assert!(selection.gaps.contains(&SourceUnitGap::InvalidMatchLine));
    }
}

#[test]
fn malformed_source_and_same_row_sibling_declarations_never_claim_an_isolated_complete_owner() {
    let broken = "package sample\nfunc Good() {}\nfunc Broken() {\n";
    assert_eq!(
        compile_source_units(
            "go",
            broken,
            &[2],
            &mut allowance(),
            &CancellationToken::new()
        ),
        Err(SourceUnitError::MalformedSource)
    );
    let source = "package sample\nfunc Left() {}; func Right() {}\n";
    let selections = compile(source, &[2]);
    assert!(
        selections[0].units.is_empty(),
        "a physical row containing sibling declarations is ambiguous"
    );
    assert!(selections[0].gaps.contains(&SourceUnitGap::NoEnclosingUnit));
    for (source, hits) in [
        (
            "package sample\n// Left docs\nfunc Left() {\n    println(1)\n}; func Right() {}\n",
            vec![2, 3, 4, 5],
        ),
        (
            "package sample\n// Left docs\nfunc Left() {}; func Right() {}\n",
            vec![2, 3],
        ),
        (
            "package sample\nfunc Left() {\n    println(1)\n}; var X = 1\n",
            vec![2, 3, 4],
        ),
        (
            "package sample\nfunc Left() {\n    println(1)\n}; type Extra struct{}\n",
            vec![2, 3, 4],
        ),
        ("package sample; func Left() {}\n", vec![1]),
        ("package sample\nimport \"fmt\"; func Left() {}\n", vec![2]),
    ] {
        let source = format!("{source}func Good() {{}}\n");
        let good_line = source.lines().count();
        for selection in compile(&source, &hits) {
            assert!(
                selection.units.is_empty(),
                "a prior hit cannot return a sibling-contaminated physical envelope"
            );
            assert!(selection.gaps.contains(&SourceUnitGap::NoEnclosingUnit));
        }
        let good = compile(&source, &[good_line]);
        exact(
            &source,
            &good[0],
            "func Good() {}\n",
            good_line,
            good_line,
            "complete_function_declaration",
        );
    }
}

#[test]
fn go_uses_existing_shared_source_work_depth_and_cancellation_allowances() {
    let source = "package sample\nfunc Build() {\n    value := 1\n    _ = value\n}\n";
    let cancel = CancellationToken::new();
    let mut budget = SourceUnitBudget {
        remaining_source_bytes: source.len() * 2 - 1,
        ..allowance()
    };
    compile_source_units("go", source, &[3], &mut budget, &cancel).unwrap();
    assert_eq!(budget.remaining_source_bytes, source.len() - 1);
    assert_eq!(
        compile_source_units("go", source, &[3], &mut budget, &cancel),
        Err(SourceUnitError::SourceByteBudgetExceeded)
    );
    let mut budget = SourceUnitBudget {
        remaining_work: 0,
        ..allowance()
    };
    assert_eq!(
        compile_source_units("go", source, &[3], &mut budget, &cancel),
        Err(SourceUnitError::WorkBudgetExceeded)
    );
    let deep = "package sample\nfunc Build() {\n    if true {\n        if true {\n            println(1)\n        }\n    }\n}\n";
    let mut budget = SourceUnitBudget {
        max_depth: 2,
        ..allowance()
    };
    assert_eq!(
        compile_source_units("go", deep, &[5], &mut budget, &cancel),
        Err(SourceUnitError::DepthExceeded)
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        compile_source_units("go", source, &[3], &mut allowance(), &cancelled),
        Err(SourceUnitError::Cancelled)
    );
    let mut measured = allowance();
    compile_source_units("go", source, &[3], &mut measured, &cancel).unwrap();
    let used = allowance().remaining_work - measured.remaining_work;
    assert!(used > source.len());
    let mut shared = SourceUnitBudget {
        remaining_work: used + source.len(),
        ..allowance()
    };
    compile_source_units("go", source, &[3], &mut shared, &cancel).unwrap();
    assert_eq!(
        compile_source_units("go", source, &[3], &mut shared, &cancel),
        Err(SourceUnitError::WorkBudgetExceeded),
        "Go must not reset shared work between supplied buffers"
    );
}
