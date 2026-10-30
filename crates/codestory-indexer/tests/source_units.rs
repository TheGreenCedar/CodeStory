use codestory_indexer::cancellation::CancellationToken;
use codestory_indexer::source_units::{
    SourceUnit, SourceUnitBudget, SourceUnitError, SourceUnitGap, SourceUnitSelection,
    compile_source_units,
};

fn budget() -> SourceUnitBudget {
    SourceUnitBudget {
        remaining_source_bytes: 1_000_000,
        remaining_work: 1_000_000,
        max_depth: 256,
    }
}

fn compile_for_python(
    source: &str,
    lines: &[usize],
    budget: &mut SourceUnitBudget,
    cancellation: &CancellationToken,
) -> Result<Vec<SourceUnitSelection>, SourceUnitError> {
    compile_source_units("py", source, lines, budget, cancellation)
}

fn compile(source: &str, lines: &[usize]) -> Vec<SourceUnitSelection> {
    compile_for_python(source, lines, &mut budget(), &CancellationToken::new())
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
            compile_for_python(source, &[1, 2], &mut budget(), &CancellationToken::new()),
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
    assert!(compile_for_python(source, &[1, 2, 2], &mut shared, &cancel).is_ok());
    assert_eq!(shared.remaining_source_bytes, source.len() - 1);
    assert_eq!(
        compile_for_python(source, &[2], &mut shared, &cancel),
        Err(SourceUnitError::SourceByteBudgetExceeded)
    );
}

#[test]
fn cancelled_exhausted_work_and_depth_fail_without_partial_units() {
    let source = "def outer():\n    def inner():\n        return 1\n";
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        compile_for_python(source, &[3], &mut budget(), &cancel),
        Err(SourceUnitError::Cancelled)
    );
    let cancel = CancellationToken::new();
    let mut work = SourceUnitBudget {
        remaining_work: 1,
        ..budget()
    };
    assert_eq!(
        compile_for_python(source, &[3], &mut work, &cancel),
        Err(SourceUnitError::WorkBudgetExceeded)
    );
    assert_eq!(work.remaining_work, 0);
    assert_eq!(
        compile_for_python(source, &[3], &mut work, &cancel),
        Err(SourceUnitError::WorkBudgetExceeded)
    );
    let mut depth = SourceUnitBudget {
        max_depth: 1,
        ..budget()
    };
    assert_eq!(
        compile_for_python(source, &[3], &mut depth, &cancel),
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
        let selected = compile_for_python(&source, &hits, &mut allowance, &cancel).unwrap();
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
            compile_for_python(&source, &requests, &mut repeated_budget, &cancel).unwrap();
        assert_eq!(repeated.len(), requests.len());
        assert!(repeated.windows(2).all(|pair| pair[0] == pair[1]));
    }
}

fn rust(source: &str, lines: &[usize]) -> Vec<SourceUnitSelection> {
    compile_source_units(
        "rs",
        source,
        lines,
        &mut budget(),
        &CancellationToken::new(),
    )
    .expect("valid supplied Rust must produce exact source units")
}

#[test]
fn rust_outer_attributes_and_docs_attach_only_to_their_named_item() {
    let source = "#![allow(dead_code)]\nfn previous() {}\n\n/// Build a value.\n#[inline]\npub async fn build() {\n    let text = r#\"fn imaginary() {}\"#;\n}\n\n/// Other item.\nfn other() {}\n";
    let selected = rust(source, &[4, 5, 7, 1, 2]);
    for selection in &selected[..3] {
        assert!(selection.gaps.is_empty());
        assert_eq!(selection.units.len(), 1);
        exact(
            source,
            &selection.units[0],
            (4, 8),
            "complete_function_item",
            "/// Build a value.\n#[inline]\npub async fn build() {\n    let text = r#\"fn imaginary() {}\"#;\n}\n",
        );
    }
    assert!(
        selected[3].units.is_empty(),
        "inner crate attributes are not attached to a neighboring item"
    );
    exact(
        source,
        &selected[4].units[0],
        (2, 2),
        "complete_function_item",
        "fn previous() {}\n",
    );
}

#[test]
fn rust_impl_method_keeps_its_envelope_and_only_a_partial_impl_header() {
    let source = "/// Methods.\nimpl Container {\n    #![allow(unused)]\n    /// Build.\n    #[inline]\n    fn build(&self) {\n        run();\n    }\n    fn other(&self) {}\n}\n";
    let selected = rust(source, &[5, 7, 2, 3]);
    for selection in &selected[..2] {
        assert!(selection.gaps.is_empty());
        assert_eq!(selection.units.len(), 2);
        exact(
            source,
            &selection.units[0],
            (1, 2),
            "ancestor_header_context",
            "/// Methods.\nimpl Container {\n",
        );
        exact(
            source,
            &selection.units[1],
            (4, 8),
            "complete_function_item",
            "    /// Build.\n    #[inline]\n    fn build(&self) {\n        run();\n    }\n",
        );
    }
    for selection in &selected[2..] {
        assert_eq!(selection.units.len(), 1);
        exact(
            source,
            &selection.units[0],
            (1, 10),
            "complete_impl_item",
            source,
        );
    }
}

#[test]
fn rust_multiline_attribute_and_generic_where_header_preserve_separate_envelopes() {
    let source = "struct Container<T>(T);\ntrait Bound {}\n\n/// Methods.\nimpl<T> Container<T>\nwhere\n    T: Bound,\n{\n    /// Build.\n    #[cfg_attr(\n        feature = \"extra\",\n        inline(always),\n    )]\n    fn build(&self) {\n        run();\n    }\n    fn sibling(&self) {}\n}\n";
    let selected = rust(source, &[11, 15]);
    for selection in selected {
        assert!(selection.gaps.is_empty());
        assert_eq!(selection.units.len(), 2);
        exact(
            source,
            &selection.units[0],
            (4, 8),
            "ancestor_header_context",
            "/// Methods.\nimpl<T> Container<T>\nwhere\n    T: Bound,\n{\n",
        );
        exact(
            source,
            &selection.units[1],
            (9, 16),
            "complete_function_item",
            "    /// Build.\n    #[cfg_attr(\n        feature = \"extra\",\n        inline(always),\n    )]\n    fn build(&self) {\n        run();\n    }\n",
        );
    }
}

#[test]
fn rust_trait_signature_and_default_body_are_distinct_complete_items() {
    let source = "trait Action {\n    fn required(&self);\n    fn default(&self) {\n        run();\n    }\n}\n";
    let selected = rust(source, &[2, 4]);
    for selection in &selected {
        assert!(selection.gaps.is_empty());
        assert_eq!(selection.units.len(), 2);
        exact(
            source,
            &selection.units[0],
            (1, 1),
            "ancestor_header_context",
            "trait Action {\n",
        );
    }
    exact(
        source,
        &selected[0].units[1],
        (2, 2),
        "complete_function_signature_item",
        "    fn required(&self);\n",
    );
    exact(
        source,
        &selected[1].units[1],
        (3, 5),
        "complete_function_item",
        "    fn default(&self) {\n        run();\n    }\n",
    );
}

#[test]
fn rust_module_and_nearest_two_named_ancestor_headers_keep_exact_ranges() {
    let source = "mod root {\n    mod nested {\n        impl Container {\n            fn build() {\n                run();\n            }\n        }\n    }\n}\n";
    let selected = rust(source, &[1, 5]);
    exact(
        source,
        &selected[0].units[0],
        (1, 9),
        "complete_mod_item",
        source,
    );
    assert_eq!(selected[1].gaps, [SourceUnitGap::AncestorDepthExceeded]);
    assert_eq!(selected[1].units.len(), 3);
    exact(
        source,
        &selected[1].units[0],
        (2, 2),
        "ancestor_header_context",
        "    mod nested {\n",
    );
    exact(
        source,
        &selected[1].units[1],
        (3, 3),
        "ancestor_header_context",
        "        impl Container {\n",
    );
    exact(
        source,
        &selected[1].units[2],
        (4, 6),
        "complete_function_item",
        "            fn build() {\n                run();\n            }\n",
    );
}

#[test]
fn rust_nested_functions_win_but_closures_literals_and_macro_tokens_do_not() {
    let source = "fn outer() {\n    let closure = || {\n        let text = r#\"fn imaginary() { }\"#;\n    };\n    fn inner() {\n        tokens! { fn token_only() {} }\n    }\n    closure();\n}\n// fn comment_only() {}\n";
    let selected = rust(source, &[3, 6, 10]);
    assert!(selected[0].gaps.is_empty());
    assert_eq!(selected[0].units.len(), 1);
    exact(
        source,
        &selected[0].units[0],
        (1, 9),
        "complete_function_item",
        "fn outer() {\n    let closure = || {\n        let text = r#\"fn imaginary() { }\"#;\n    };\n    fn inner() {\n        tokens! { fn token_only() {} }\n    }\n    closure();\n}\n",
    );
    assert_eq!(selected[1].units.len(), 2);
    exact(
        source,
        &selected[1].units[0],
        (1, 1),
        "ancestor_header_context",
        "fn outer() {\n",
    );
    exact(
        source,
        &selected[1].units[1],
        (5, 7),
        "complete_function_item",
        "    fn inner() {\n        tokens! { fn token_only() {} }\n    }\n",
    );
    assert!(selected[2].units.is_empty());
    assert_eq!(selected[2].gaps, [SourceUnitGap::NoEnclosingUnit]);
}

#[test]
fn rust_crlf_unicode_eof_and_duplicate_requests_preserve_physical_bytes() {
    let source =
        "// 雪\r\n#[inline]\r\nfn first() {\r\n    let text = \"🙂\";\r\n}\r\nfn second() {}";
    let requested = [6, 4, 6, 0, 7];
    let selected = rust(source, &requested);
    assert_eq!(
        selected.iter().map(|s| s.match_line).collect::<Vec<_>>(),
        requested
    );
    assert_eq!(selected[0], selected[2]);
    exact(
        source,
        &selected[0].units[0],
        (6, 6),
        "complete_function_item",
        "fn second() {}",
    );
    exact(
        source,
        &selected[1].units[0],
        (2, 5),
        "complete_function_item",
        "#[inline]\r\nfn first() {\r\n    let text = \"🙂\";\r\n}\r\n",
    );
    for index in [3, 4] {
        assert!(selected[index].units.is_empty());
        assert_eq!(selected[index].gaps, [SourceUnitGap::InvalidMatchLine]);
    }
}

#[test]
fn rust_single_line_item_is_exact_but_adjacent_same_line_items_are_ambiguous() {
    let source = "fn inline() {}\nfn first() {} fn second() {}\n";
    let selected = rust(source, &[1, 2]);
    exact(
        source,
        &selected[0].units[0],
        (1, 1),
        "complete_function_item",
        "fn inline() {}\n",
    );
    assert!(selected[1].units.is_empty());
    assert_eq!(selected[1].gaps, [SourceUnitGap::NoEnclosingUnit]);
}

#[test]
fn rust_malformed_or_recovered_items_never_publish_a_valid_looking_prefix() {
    for source in [
        "fn broken() {\n    run();\n",
        "mod broken {\n    fn valid() {}\n",
        "#[cfg(\nfn broken() {}\n",
        "fn valid() {}\nfn broken( {\n",
        "fn valid() {}\n}\n",
    ] {
        assert_eq!(
            compile_source_units("rs", source, &[1], &mut budget(), &CancellationToken::new()),
            Err(SourceUnitError::MalformedSource)
        );
    }
}

#[test]
fn rust_oversized_ancestor_header_is_declined_without_clipping_the_function() {
    let source = format!(
        "mod {} {{\n    fn build() {{\n        run();\n    }}\n}}\n",
        "m".repeat(1100)
    );
    let selected = rust(&source, &[3]);
    assert_eq!(selected[0].gaps, [SourceUnitGap::AncestorHeaderTooLarge]);
    assert_eq!(selected[0].units.len(), 1);
    exact(
        &source,
        &selected[0].units[0],
        (2, 4),
        "complete_function_item",
        "    fn build() {\n        run();\n    }\n",
    );
}

#[test]
fn language_dispatch_preserves_python_and_rejects_unsupported_extensions() {
    let source = "@wrap\nasync def build():\n    return '雪'\n";
    let selected = compile_source_units(
        "py",
        source,
        &[3, 2],
        &mut budget(),
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0].units, selected[1].units);
    exact(
        source,
        &selected[0].units[0],
        (1, 3),
        "complete_AsyncFunctionDef",
        source,
    );
    for extension in ["js", "txt", "RS", ""] {
        assert_eq!(
            compile_source_units(
                extension,
                "fn build() {}",
                &[1],
                &mut budget(),
                &CancellationToken::new()
            ),
            Err(SourceUnitError::UnsupportedSyntax)
        );
    }
}

#[test]
fn rust_and_python_compilations_share_source_allowance_without_reset() {
    let py = "def build():\n    return 1\n";
    let rs = "fn build() {\n    run();\n}\n";
    let cancel = CancellationToken::new();
    for (first_ext, first, next_ext, next) in [("py", py, "rs", rs), ("rs", rs, "py", py)] {
        let mut shared = SourceUnitBudget {
            remaining_source_bytes: first.len() + next.len() - 1,
            ..budget()
        };
        assert!(compile_source_units(first_ext, first, &[2], &mut shared, &cancel).is_ok());
        assert_eq!(shared.remaining_source_bytes, next.len() - 1);
        assert_eq!(
            compile_source_units(next_ext, next, &[2], &mut shared, &cancel),
            Err(SourceUnitError::SourceByteBudgetExceeded)
        );
    }
}

#[test]
fn rust_cancellation_work_and_depth_exhaustion_return_no_partial_units() {
    let source = "mod outer {\n    mod inner {\n        fn build() {\n            run();\n        }\n    }\n}\n";
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        compile_source_units("rs", source, &[4], &mut budget(), &cancel),
        Err(SourceUnitError::Cancelled)
    );
    let cancel = CancellationToken::new();
    let mut work = SourceUnitBudget {
        remaining_work: 1,
        ..budget()
    };
    assert_eq!(
        compile_source_units("rs", source, &[4], &mut work, &cancel),
        Err(SourceUnitError::WorkBudgetExceeded)
    );
    assert_eq!(work.remaining_work, 0);
    assert_eq!(
        compile_source_units("rs", source, &[4], &mut work, &cancel),
        Err(SourceUnitError::WorkBudgetExceeded)
    );
    let mut depth = SourceUnitBudget {
        max_depth: 1,
        ..budget()
    };
    assert_eq!(
        compile_source_units("rs", source, &[4], &mut depth, &cancel),
        Err(SourceUnitError::DepthExceeded)
    );
}

#[test]
fn rust_repeated_hits_and_growing_files_do_not_multiply_whole_tree_work() {
    let cancel = CancellationToken::new();
    let mut previous = None;
    for count in [32usize, 64, 128] {
        let source = (0..count)
            .map(|n| format!("fn item_{n}() -> usize {{\n    {n}\n}}\n"))
            .collect::<String>();
        let hits = (0..count).map(|n| n * 3 + 2).collect::<Vec<_>>();
        let mut allowance = budget();
        let before = allowance.remaining_work;
        let selected = compile_source_units("rs", &source, &hits, &mut allowance, &cancel).unwrap();
        assert_eq!(selected.len(), count);
        assert!(
            selected
                .iter()
                .all(|s| s.units.len() == 1 && s.gaps.is_empty())
        );
        let work = before - allowance.remaining_work;
        assert!(work > 0);
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
            compile_source_units("rs", &source, &requests, &mut repeated_budget, &cancel).unwrap();
        assert_eq!(repeated.len(), requests.len());
        assert!(repeated.windows(2).all(|pair| pair[0] == pair[1]));
    }
}
