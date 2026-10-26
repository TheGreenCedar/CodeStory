use codestory_indexer::cancellation::CancellationToken;
use codestory_indexer::source_units::{
    SourceUnit, SourceUnitBudget, SourceUnitError, SourceUnitGap, SourceUnitSelection,
    compile_python_source_units,
};

fn budget() -> SourceUnitBudget {
    SourceUnitBudget {
        remaining_source_bytes: 1_000_000,
        remaining_work: 1_000_000,
        max_depth: 256,
    }
}

fn compile(source: &str, lines: &[usize]) -> Vec<SourceUnitSelection> {
    compile_python_source_units(source, lines, &mut budget(), &CancellationToken::new())
        .expect("valid supplied Python must yield exact units, not Unavailable")
}

fn exact(source: &str, unit: &SourceUnit, lines: (usize, usize), kind: &str, expected: &str) {
    assert_eq!((unit.start_line, unit.end_line), lines);
    assert_eq!(unit.kind, kind);
    assert!(source.is_char_boundary(unit.start_byte));
    assert!(source.is_char_boundary(unit.end_byte));
    assert_eq!(&source[unit.start_byte..unit.end_byte], expected);
    // Independent physical-line reconstruction also protects offsets when the
    // same body text occurs twice or Unicode precedes the selected declaration.
    let physical: Vec<_> = source.split_inclusive('\n').collect();
    assert_eq!(
        unit.start_byte,
        physical[..lines.0 - 1]
            .iter()
            .map(|s| s.len())
            .sum::<usize>()
    );
    assert_eq!(
        unit.end_byte,
        physical[..lines.1].iter().map(|s| s.len()).sum::<usize>()
    );
}

#[test]
fn decorated_async_owner_retains_decorators_and_partial_class_header() {
    let source = "@decorate\nclass Container:\n    @wrap(\n        'literal def fake():',\n    )\n    async def build(self):\n        value = f'class {self}'\n        return value\n";
    let selections = compile(source, &[3, 7]);
    assert_eq!(selections.len(), 2);
    for selection in selections {
        assert!(selection.gaps.is_empty());
        assert_eq!(selection.units.len(), 2);
        // Prototype AST body[0].lineno starts at the child def, so its
        // preceding decorator rows are also exact partial ancestor context.
        exact(
            source,
            &selection.units[0],
            (1, 5),
            "ancestor_header_context",
            "@decorate\nclass Container:\n    @wrap(\n        'literal def fake():',\n    )\n",
        );
        exact(
            source,
            &selection.units[1],
            (3, 8),
            "complete_AsyncFunctionDef",
            "    @wrap(\n        'literal def fake():',\n    )\n    async def build(self):\n        value = f'class {self}'\n        return value\n",
        );
    }
}

#[test]
fn nearest_two_ancestor_headers_preserve_order_and_report_deeper_ancestry() {
    let source = "def root():\n    def middle():\n        def inner():\n            def leaf():\n                return 1\n";
    let selection = compile(source, &[5]).remove(0);
    assert_eq!(selection.gaps, [SourceUnitGap::AncestorDepthExceeded]);
    assert_eq!(selection.units.len(), 3);
    exact(
        source,
        &selection.units[0],
        (2, 2),
        "ancestor_header_context",
        "    def middle():\n",
    );
    exact(
        source,
        &selection.units[1],
        (3, 3),
        "ancestor_header_context",
        "        def inner():\n",
    );
    exact(
        source,
        &selection.units[2],
        (4, 5),
        "complete_FunctionDef",
        "            def leaf():\n                return 1\n",
    );
}

#[test]
fn class_hit_returns_the_whole_decorated_class_not_just_its_header() {
    let source = "@decorate\nclass Container:\n    value = 1\n    def method(self):\n        return self.value\n";
    let selection = compile(source, &[3]).remove(0);
    assert!(selection.gaps.is_empty());
    assert_eq!(selection.units.len(), 1);
    exact(
        source,
        &selection.units[0],
        (1, 5),
        "complete_ClassDef",
        source,
    );
}

#[test]
fn top_level_statements_comments_and_strings_do_not_fabricate_callable_owners() {
    let source = "import os\n# def imaginary():\nvalues = (\n    'class fake:',\n    f'def {os.name}',\n)\n\nif values:\n    result = values[0]\n    print(result)\n";
    let selected = compile(source, &[1, 2, 4, 9]);
    exact(
        source,
        &selected[0].units[0],
        (1, 1),
        "complete_Import",
        "import os\n",
    );
    assert!(selected[1].units.is_empty());
    assert_eq!(selected[1].gaps, [SourceUnitGap::NoEnclosingUnit]);
    exact(
        source,
        &selected[2].units[0],
        (3, 6),
        "complete_Assign",
        "values = (\n    'class fake:',\n    f'def {os.name}',\n)\n",
    );
    exact(
        source,
        &selected[3].units[0],
        (8, 10),
        "complete_If",
        "if values:\n    result = values[0]\n    print(result)\n",
    );
}

#[test]
fn comments_inside_an_owner_do_not_extend_its_last_ast_statement() {
    let source = "def build():\n    # interior comment\n    value = 1\n    return value\n    # trailing comment\n\nnext_value = 2\n";
    let selected = compile(source, &[2, 5]);
    assert_eq!(selected[0].units.len(), 1);
    assert!(selected[0].gaps.is_empty());
    exact(
        source,
        &selected[0].units[0],
        (1, 4),
        "complete_FunctionDef",
        "def build():\n    # interior comment\n    value = 1\n    return value\n",
    );
    assert!(selected[1].units.is_empty());
    assert_eq!(selected[1].gaps, [SourceUnitGap::NoEnclosingUnit]);
}

#[test]
fn late_except_and_finally_hits_return_the_complete_top_level_try() {
    let source =
        "try:\n    value = run()\nexcept ValueError:\n    recover()\nfinally:\n    cleanup()\n";
    let selected = compile(source, &[4, 6]);
    for selection in selected {
        assert_eq!(selection.units.len(), 1);
        assert!(selection.gaps.is_empty());
        exact(source, &selection.units[0], (1, 6), "complete_Try", source);
    }
}

#[test]
fn crlf_unicode_and_unterminated_eof_preserve_exact_physical_bytes() {
    let source = "prefix = '雪'\r\n\r\n@wrap\r\nasync def build():\r\n    return '🙂'";
    let selected = compile(source, &[5]);
    assert_eq!(selected[0].units.len(), 1);
    exact(
        source,
        &selected[0].units[0],
        (3, 5),
        "complete_AsyncFunctionDef",
        "@wrap\r\nasync def build():\r\n    return '🙂'",
    );
}

#[test]
fn one_line_owner_is_complete_but_two_top_level_statements_are_ambiguous() {
    let source = "def build(): return 1\nfirst = 1; second = 2\nif first: print(first)\n";
    let selected = compile(source, &[1, 2, 3]);
    exact(
        source,
        &selected[0].units[0],
        (1, 1),
        "complete_FunctionDef",
        "def build(): return 1\n",
    );
    assert!(selected[0].gaps.is_empty());
    assert!(selected[1].units.is_empty());
    assert_eq!(selected[1].gaps, [SourceUnitGap::NoEnclosingUnit]);
    exact(
        source,
        &selected[2].units[0],
        (3, 3),
        "complete_If",
        "if first: print(first)\n",
    );
}

#[test]
fn duplicate_out_of_order_and_invalid_lines_have_one_ordered_result_each() {
    let source = "def first():\n    return 1\n\ndef second():\n    return 1\n";
    let requested = [5, 0, 2, 5, 6, 3];
    let selected = compile(source, &requested);
    assert_eq!(
        selected.iter().map(|s| s.match_line).collect::<Vec<_>>(),
        requested
    );
    assert_eq!(selected[0], selected[3]);
    exact(
        source,
        &selected[0].units[0],
        (4, 5),
        "complete_FunctionDef",
        "def second():\n    return 1\n",
    );
    exact(
        source,
        &selected[2].units[0],
        (1, 2),
        "complete_FunctionDef",
        "def first():\n    return 1\n",
    );
    for i in [1, 4] {
        assert!(selected[i].units.is_empty());
        assert_eq!(selected[i].gaps, [SourceUnitGap::InvalidMatchLine]);
    }
    assert!(selected[5].units.is_empty());
    assert_eq!(selected[5].gaps, [SourceUnitGap::NoEnclosingUnit]);
    let empty = compile("", &[0, 1]);
    assert!(
        empty
            .iter()
            .all(|s| s.units.is_empty() && s.gaps == [SourceUnitGap::InvalidMatchLine])
    );
}

#[test]
fn malformed_or_recovered_source_never_returns_a_valid_prefix_as_complete() {
    for source in [
        "def broken(:\n    pass\n",
        "def broken():\n    return (\n",
        "if True\n    pass\n",
        "def good():\n    return 1\n\nbroken =\n",
        "function foreign() { return 1; }\n",
    ] {
        assert_eq!(
            compile_python_source_units(source, &[1, 2], &mut budget(), &CancellationToken::new()),
            Err(SourceUnitError::MalformedSource)
        );
    }
}

#[test]
fn oversized_partial_ancestor_header_is_declined_without_clipping_the_owner() {
    let source = format!(
        "def outer(\n    {}\n):\n    def inner():\n        return 1\n",
        "p".repeat(1100)
    );
    let selected = compile(&source, &[5]);
    assert_eq!(selected[0].gaps, [SourceUnitGap::AncestorHeaderTooLarge]);
    assert_eq!(selected[0].units.len(), 1);
    exact(
        &source,
        &selected[0].units[0],
        (4, 5),
        "complete_FunctionDef",
        "    def inner():\n        return 1\n",
    );
}

#[test]
fn source_byte_allowance_is_shared_across_buffers_not_reset_per_compile() {
    let source = "def build():\n    return 1\n";
    let mut shared = SourceUnitBudget {
        remaining_source_bytes: source.len() * 2 - 1,
        ..budget()
    };
    let cancel = CancellationToken::new();
    assert!(compile_python_source_units(source, &[1, 2, 2], &mut shared, &cancel).is_ok());
    assert_eq!(shared.remaining_source_bytes, source.len() - 1);
    assert_eq!(
        compile_python_source_units(source, &[2], &mut shared, &cancel),
        Err(SourceUnitError::SourceByteBudgetExceeded)
    );
}

#[test]
fn cancelled_exhausted_work_and_depth_fail_without_partial_units() {
    let source = "def outer():\n    def inner():\n        return 1\n";
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        compile_python_source_units(source, &[3], &mut budget(), &cancel),
        Err(SourceUnitError::Cancelled)
    );
    let cancel = CancellationToken::new();
    let mut work = SourceUnitBudget {
        remaining_work: 1,
        ..budget()
    };
    assert_eq!(
        compile_python_source_units(source, &[3], &mut work, &cancel),
        Err(SourceUnitError::WorkBudgetExceeded)
    );
    assert_eq!(work.remaining_work, 0);
    assert_eq!(
        compile_python_source_units(source, &[3], &mut work, &cancel),
        Err(SourceUnitError::WorkBudgetExceeded)
    );
    let mut depth = SourceUnitBudget {
        max_depth: 1,
        ..budget()
    };
    assert_eq!(
        compile_python_source_units(source, &[3], &mut depth, &cancel),
        Err(SourceUnitError::DepthExceeded)
    );
}

#[test]
fn repeated_hits_and_growing_files_do_not_multiply_whole_tree_work() {
    let cancel = CancellationToken::new();
    let mut previous = None;
    for count in [32usize, 64, 128] {
        let source = (0..count)
            .map(|n| format!("def item_{n}():\n    return {n}\n"))
            .collect::<String>();
        let hits = (0..count).map(|n| n * 2 + 2).collect::<Vec<_>>();
        let mut allowance = budget();
        let before = allowance.remaining_work;
        let selected =
            compile_python_source_units(&source, &hits, &mut allowance, &cancel).unwrap();
        assert_eq!(selected.len(), count);
        assert!(
            selected
                .iter()
                .all(|s| s.units.len() == 1 && s.gaps.is_empty())
        );
        let work = before - allowance.remaining_work;
        assert!(work > 0, "shared work must account for actual compilation");
        if let Some(prior) = previous {
            assert!(
                work <= prior * 3,
                "doubling source/hits must not quadruple traversal work"
            );
        }
        previous = Some(work);
        let requests = vec![2; count * 4];
        let mut repeated_budget = SourceUnitBudget {
            remaining_work: work + requests.len() * 16,
            ..budget()
        };
        let repeated =
            compile_python_source_units(&source, &requests, &mut repeated_budget, &cancel).unwrap();
        assert_eq!(repeated.len(), requests.len());
        assert!(repeated.windows(2).all(|pair| pair[0] == pair[1]));
    }
}
