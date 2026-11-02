//! Behavioral matrix for the private supplied-Go-buffer slice experiment.
use super::go_expression_slice::{
    GoExpressionSlice, GoSliceBoundaryKind, GoSliceGap, GoSliceRole, compile_go_expression_slice,
};
use super::{CancellationToken, SourceUnitBudget, SourceUnitError};
use std::ops::Range;

fn allowance() -> SourceUnitBudget {
    SourceUnitBudget {
        remaining_source_bytes: 2 * 1024 * 1024,
        remaining_work: 8_000_000,
        max_depth: 256,
    }
}

fn criterion(source: &str, spelling: &str) -> Range<usize> {
    let start = source.rfind(spelling).expect("fixture criterion");
    start..start + spelling.len()
}

fn slice(source: &str, spelling: &str) -> GoExpressionSlice {
    slice_exact(source, criterion(source, spelling))
}

fn slice_exact(source: &str, target: Range<usize>) -> GoExpressionSlice {
    let result = compile_go_expression_slice(
        source,
        target.clone(),
        &mut allowance(),
        &CancellationToken::new(),
    )
    .expect("valid supplied Go source");
    assert_eq!(result.criterion, target);
    for witness in &result.witnesses {
        let unit = &witness.unit;
        assert!(unit.start_byte < unit.end_byte && unit.end_byte <= source.len());
        assert!(source.is_char_boundary(unit.start_byte));
        assert!(source.is_char_boundary(unit.end_byte));
        assert!(unit.start_byte <= witness.range.start && witness.range.start < witness.range.end);
        assert!(witness.range.end <= unit.end_byte);
        assert!(source.get(witness.range.clone()).is_some());
        assert!(unit.start_byte == 0 || source.as_bytes()[unit.start_byte - 1] == b'\n');
        assert!(unit.end_byte == source.len() || source.as_bytes()[unit.end_byte - 1] == b'\n');
        assert_eq!(
            unit.start_line,
            source.as_bytes()[..unit.start_byte]
                .iter()
                .filter(|&&b| b == b'\n')
                .count()
                + 1
        );
        assert_eq!(
            unit.end_line,
            source.as_bytes()[..unit.end_byte]
                .iter()
                .filter(|&&b| b == b'\n')
                .count()
                + usize::from(source.as_bytes()[unit.end_byte - 1] != b'\n')
        );
    }
    for boundary in &result.boundaries {
        assert!(boundary.range.start < boundary.range.end);
        assert!(source.get(boundary.range.clone()).is_some());
    }
    assert!(result.witnesses.iter().any(|w| {
        w.role == GoSliceRole::Criterion
            && w.range == target
            && w.unit.start_byte <= target.start
            && target.end <= w.unit.end_byte
    }));
    if !result.gaps.is_empty() {
        assert!(
            !result.complete_local,
            "a gap cannot certify complete local analysis"
        );
    }
    result
}

fn texts<'a>(source: &'a str, result: &GoExpressionSlice, role: GoSliceRole) -> Vec<&'a str> {
    result
        .witnesses
        .iter()
        .filter(|w| w.role == role)
        .map(|w| source[w.range.clone()].trim())
        .collect()
}

fn assert_members(source: &str, result: &GoExpressionSlice, role: GoSliceRole, expected: &[&str]) {
    let actual = result
        .witnesses
        .iter()
        .filter(|w| w.role == role)
        .map(|w| (w.range.start, w.range.end))
        .collect::<std::collections::BTreeSet<_>>();
    let expected = expected
        .iter()
        .flat_map(|text| {
            source
                .match_indices(text)
                .map(|(start, _)| (start, start + text.len()))
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected, "exact semantic membership for {role:?}");
}

fn assert_loop_boundary(source: &str, result: &GoExpressionSlice) {
    let start = source.find("for ").unwrap();
    let end = source.rfind("\n    }").unwrap() + "\n    }".len();
    assert!(
        result.boundaries.iter().any(|b| {
            b.kind == GoSliceBoundaryKind::UnsupportedConstruct && b.range == (start..end)
        }),
        "incomplete loop precision needs the relevant loop's source boundary"
    );
}

fn boundary_contains(
    source: &str,
    result: &GoExpressionSlice,
    kind: GoSliceBoundaryKind,
    spelling: &str,
) -> bool {
    result.boundaries.iter().any(|b| {
        if b.kind != kind {
            return false;
        }
        let text = source[b.range.clone()].trim();
        match kind {
            GoSliceBoundaryKind::UnsupportedConstruct => text.contains(spelling),
            GoSliceBoundaryKind::ClosureWrite => {
                text == spelling || text == format!("func() {{ {spelling} }}")
            }
            _ => text == spelling,
        }
    })
}

#[test]
fn branch_alternatives_keep_their_binding_guard_and_opaque_inputs() {
    let source = "package sample\nimport conn \"example/connection\"\nvar secure bool\nfunc dial(address string) {\n    var transport conn.Transport\n    if secure {\n        transport = conn.Secure()\n    } else {\n        transport = conn.Plain()\n    }\n    irrelevant := 99\n    _ = irrelevant\n    consume(address, transport)\n}\n";
    let result = slice(source, "transport");
    assert_members(
        source,
        &result,
        GoSliceRole::Definition,
        &["transport = conn.Secure()", "transport = conn.Plain()"],
    );
    assert!(texts(source, &result, GoSliceRole::Binding).contains(&"var transport conn.Transport"));
    assert!(texts(source, &result, GoSliceRole::Control).contains(&"if secure {"));
    assert!(
        !result
            .witnesses
            .iter()
            .any(|w| { source[w.range.clone()].contains("irrelevant := 99") }),
        "an enclosing-function copy is not a dependency slice"
    );
    assert!(boundary_contains(
        source,
        &result,
        GoSliceBoundaryKind::PackageInput,
        "secure"
    ));
    assert!(!boundary_contains(
        source,
        &result,
        GoSliceBoundaryKind::ParameterInput,
        "address"
    ));
    for call in ["conn.Secure()", "conn.Plain()"] {
        assert!(boundary_contains(
            source,
            &result,
            GoSliceBoundaryKind::OpaqueCall,
            call
        ));
    }
    assert!(result.complete_local && result.gaps.is_empty());
    let one_arm = "package sample\nfunc run(flag bool, seed int) {\n    value := seed\n    if flag {\n        value = 2\n    }\n    target(value)\n}\n";
    let result = slice(one_arm, "value");
    assert_members(
        one_arm,
        &result,
        GoSliceRole::Definition,
        &["value := seed", "value = 2"],
    );
    assert!(texts(one_arm, &result, GoSliceRole::Control).contains(&"if flag {"));
    assert!(result.complete_local);
}

#[test]
fn inner_shadow_does_not_reach_the_outer_use() {
    let source = "package sample\nfunc run() {\n    value := 1\n    {\n        value := 2\n        consume(value)\n    }\n    target(value)\n}\n";
    let result = slice(source, "value");
    assert_members(source, &result, GoSliceRole::Definition, &["value := 1"]);
    assert!(!texts(source, &result, GoSliceRole::Definition).contains(&"value := 2"));
    assert!(result.complete_local);
    let inner = slice(source, "consume(value)");
    assert_members(source, &inner, GoSliceRole::Definition, &["value := 2"]);
    let header = "package sample\nfunc run(a, b int) {\n    value := a\n    if value := b; value > 0 {\n        consume(value)\n    }\n    target(value)\n}\n";
    let outer = slice(header, "target(value)");
    assert_members(header, &outer, GoSliceRole::Definition, &["value := a"]);
    assert!(outer.complete_local);
    let short = "package sample\nfunc run(a, b int) {\n    value := a\n    value, extra := b, 0\n    _ = extra\n    target(value)\n}\n";
    let result = slice(short, "target(value)");
    if result.complete_local {
        assert_members(
            short,
            &result,
            GoSliceRole::Definition,
            &["value, extra := b, 0"],
        );
    } else {
        assert!(result.gaps.contains(&GoSliceGap::UnsupportedConstruct));
        assert!(boundary_contains(
            short,
            &result,
            GoSliceBoundaryKind::UnsupportedConstruct,
            "value, extra := b, 0"
        ));
    }
}

#[test]
fn unconditional_overwrite_kills_prior_value_and_ignores_future_write() {
    let source = "package sample\nfunc run() {\n    value := 1\n    value = 7\n    target(value)\n    value = 9\n}\n";
    let result = slice(source, "target(value)");
    assert_members(source, &result, GoSliceRole::Definition, &["value = 7"]);
    assert!(result.complete_local);
    let row = "package sample\nfunc run(a, b int) {\n    value := a; value = b; target(value)\n}\n";
    let result = slice(row, "target(value)");
    assert_members(row, &result, GoSliceRole::Definition, &["value = b"]);
    assert!(
        result
            .witnesses
            .iter()
            .any(|w| w.role == GoSliceRole::Definition
                && row[w.unit.start_byte..w.unit.end_byte].contains("value := a")),
        "rendered old-write bytes must not become reaching-definition membership"
    );
    assert!(boundary_contains(
        row,
        &result,
        GoSliceBoundaryKind::ParameterInput,
        "b"
    ));
    assert!(!boundary_contains(
        row,
        &result,
        GoSliceBoundaryKind::ParameterInput,
        "a"
    ));
    assert!(result.complete_local);
    let update = "package sample\nfunc run(seed int) {\n    value := seed\n    value = value + 1\n    target(value)\n}\n";
    let result = slice(update, "target(value)");
    assert_members(
        update,
        &result,
        GoSliceRole::Definition,
        &["value := seed", "value = value + 1"],
    );
    assert!(result.complete_local);
    let tuple = "package sample\nfunc run(a, b int) {\n    x, y := a, b\n    x, y = y, x\n    target(x)\n}\n";
    let result = slice(tuple, "target(x)");
    if result.complete_local {
        assert!(result.boundaries.iter().any(
            |b| b.kind == GoSliceBoundaryKind::ParameterInput && &tuple[b.range.clone()] == "b"
        ));
        assert!(!result.boundaries.iter().any(
            |b| b.kind == GoSliceBoundaryKind::ParameterInput && &tuple[b.range.clone()] == "a"
        ));
    } else {
        assert!(result.gaps.contains(&GoSliceGap::UnsupportedConstruct));
        assert!(boundary_contains(
            tuple,
            &result,
            GoSliceBoundaryKind::UnsupportedConstruct,
            "x, y = y, x"
        ));
    }
}

#[test]
fn preceding_early_return_is_a_control_dependency() {
    let source = "package sample\nfunc run(enabled bool) {\n    if !enabled {\n        return\n    }\n    value := 5\n    target(value)\n}\n";
    let result = slice(source, "value");
    assert_members(source, &result, GoSliceRole::Definition, &["value := 5"]);
    assert!(texts(source, &result, GoSliceRole::Control).contains(&"if !enabled {"));
    assert!(texts(source, &result, GoSliceRole::Control).contains(&"return"));
    assert!(boundary_contains(
        source,
        &result,
        GoSliceBoundaryKind::ParameterInput,
        "enabled"
    ));
    assert!(result.complete_local);
    let irrelevant = "package sample\nfunc run(enabled, noisy bool, value int) {\n    if !enabled {\n        return\n    }\n    if noisy {\n        observe(value)\n    }\n    target(value)\n}\n";
    let result = slice(irrelevant, "target(value)");
    assert!(
        !texts(irrelevant, &result, GoSliceRole::Control)
            .iter()
            .any(|t| t.contains("noisy"))
    );
    assert!(texts(irrelevant, &result, GoSliceRole::Control).contains(&"return"));
    let no_exit = "package sample\nfunc run(enabled bool, value int) {\n    if !enabled {\n        observe(value)\n    }\n    target(value)\n}\n";
    assert!(
        texts(
            no_exit,
            &slice(no_exit, "target(value)"),
            GoSliceRole::Control
        )
        .is_empty()
    );
}

#[test]
fn loop_continue_uses_the_current_error_definition_and_reports_precision_gaps() {
    let source = "package sample\nfunc retry(limit int) {\n    var err error\n    for n := 0; n <= limit; n++ {\n        err = request(n)\n        if err != nil {\n            continue\n        }\n        target(0)\n    }\n}\n";
    let result = slice(source, "target(0)");
    assert!(texts(source, &result, GoSliceRole::Definition).contains(&"err = request(n)"));
    let controls = texts(source, &result, GoSliceRole::Control);
    assert!(controls.contains(&"for n := 0; n <= limit; n++ {"));
    assert!(controls.contains(&"if err != nil {"));
    assert!(controls.contains(&"continue"));
    assert!(boundary_contains(
        source,
        &result,
        GoSliceBoundaryKind::OpaqueCall,
        "request(n)"
    ));
    if !result.complete_local {
        assert!(result.gaps.contains(&GoSliceGap::LoopCarriedState));
        assert_loop_boundary(source, &result);
    }
    let transitive = "package sample\nfunc retry(items []int) {\n    for _, item := range items {\n        value, err := first(item)\n        if err != nil {\n            continue\n        }\n        value, err = second(value)\n        if err != nil {\n            continue\n        }\n        target(value)\n    }\n}\n";
    let result = slice(transitive, "target(value)");
    let guards = result
        .witnesses
        .iter()
        .filter(|w| {
            w.role == GoSliceRole::Control && &transitive[w.range.clone()] == "if err != nil {"
        })
        .map(|w| (w.range.start, w.range.end))
        .collect::<std::collections::BTreeSet<_>>();
    let expected_guards = transitive
        .match_indices("if err != nil {")
        .map(|(start, text)| (start, start + text.len()))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(guards, expected_guards);
    let continues = result
        .witnesses
        .iter()
        .filter(|w| w.role == GoSliceRole::Control && &transitive[w.range.clone()] == "continue")
        .map(|w| (w.range.start, w.range.end))
        .collect::<std::collections::BTreeSet<_>>();
    let expected_continues = transitive
        .match_indices("continue")
        .map(|(start, text)| (start, start + text.len()))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(continues, expected_continues);
    if result.complete_local {
        let definitions = texts(transitive, &result, GoSliceRole::Definition);
        assert!(definitions.contains(&"value, err := first(item)"));
        assert!(definitions.contains(&"value, err = second(value)"));
        assert_eq!(
            texts(transitive, &result, GoSliceRole::Control)
                .iter()
                .filter(|&&t| t == "if err != nil {")
                .count(),
            2
        );
    } else {
        assert!(
            result.gaps.contains(&GoSliceGap::LoopCarriedState)
                || result.gaps.contains(&GoSliceGap::UnsupportedConstruct)
        );
        assert_loop_boundary(transitive, &result);
    }
    let carried = "package sample\nfunc run(n, seed int) {\n    value := seed\n    for i := 0; i < n; i++ {\n        if i == 1 { continue }\n        value = value + i\n    }\n    target(value)\n}\n";
    let result = slice(carried, "target(value)");
    if result.complete_local {
        assert_members(
            carried,
            &result,
            GoSliceRole::Definition,
            &["value := seed", "value = value + i", "i := 0"],
        );
    } else {
        assert!(result.gaps.contains(&GoSliceGap::LoopCarriedState));
        assert_loop_boundary(carried, &result);
    }
}

#[test]
fn logged_status_is_not_promoted_to_a_success_guard() {
    let source = "package sample\ntype Reply interface { Status() int }\nfunc run(resp Reply, err error) {\n    if err != nil {\n        return\n    }\n    observe(resp.Status())\n    finish(0)\n}\n";
    let result = slice(source, "finish(0)");
    assert_members(
        source,
        &result,
        GoSliceRole::Control,
        &["if err != nil {", "return"],
    );
    assert!(boundary_contains(
        source,
        &result,
        GoSliceBoundaryKind::ParameterInput,
        "err"
    ));
    assert!(
        !texts(source, &result, GoSliceRole::Control)
            .iter()
            .any(|text| text.contains("Status"))
    );
    let guarded = "package sample\ntype Reply interface { Status() int }\nfunc run(resp Reply) {\n    if resp.Status() == 1 {\n        finish(0)\n    }\n}\n";
    assert!(
        texts(guarded, &slice(guarded, "finish(0)"), GoSliceRole::Control)
            .contains(&"if resp.Status() == 1 {")
    );
}

#[test]
fn escaped_and_closure_writes_cannot_silently_certify_the_prior_value() {
    for (source, kind, effect) in [
        (
            "package sample\nfunc run() {\n    value := 1\n    mutate(&value)\n    target(value)\n}\n",
            GoSliceBoundaryKind::IndirectWrite,
            "mutate(&value)",
        ),
        (
            "package sample\nfunc run() {\n    value := 1\n    alias := &value\n    mutate(alias)\n    target(value)\n}\n",
            GoSliceBoundaryKind::IndirectWrite,
            "mutate(alias)",
        ),
        (
            "package sample\nfunc run() {\n    value := 1\n    callback := func() { value = 2 }\n    callback()\n    target(value)\n}\n",
            GoSliceBoundaryKind::ClosureWrite,
            "value = 2",
        ),
        (
            "package sample\nfunc run() {\n    value := 1\n    callback := func() { value = 2 }\n    opaque(callback)\n    target(value)\n}\n",
            GoSliceBoundaryKind::ClosureWrite,
            "value = 2",
        ),
        (
            "package sample\ntype Number int\nfunc (n *Number) change() { *n = 2 }\nfunc run() {\n value := Number(1)\n value.change()\n target(value)\n}\n",
            GoSliceBoundaryKind::IndirectWrite,
            "value.change()",
        ),
    ] {
        let result = slice(source, "target(value)");
        assert!(
            !result.complete_local,
            "opaque mutation cannot prove the reaching value"
        );
        assert!(result.gaps.contains(&GoSliceGap::UnknownMutation));
        assert!(boundary_contains(source, &result, kind, effect));
    }
    let aggregate = "package sample\nfunc run(values []int) {\n    alias := values\n    alias[0] = 2\n    target(values[0])\n}\n";
    let result = slice(aggregate, "target(values[0])");
    assert!(!result.complete_local);
    assert!(result.gaps.contains(&GoSliceGap::UnknownMutation));
    assert!(boundary_contains(
        aggregate,
        &result,
        GoSliceBoundaryKind::IndirectWrite,
        "alias[0] = 2"
    ));
}

#[test]
fn invalid_unsupported_and_exhausted_inputs_never_claim_complete_analysis() {
    let source = "package sample\r\nfunc run() {\r\n    value := \"雪\"\r\n    target(value)\r\n}";
    let exact = slice(source, "value");
    assert_members(
        source,
        &exact,
        GoSliceRole::Definition,
        &["value := \"雪\""],
    );
    assert!(exact.complete_local);
    let physical = "package sample\r\n// 文\u{2028}\r\nfunc run() {\r\n    变量 := \"雪\u{2028}\"; unrelated := 99\r\n    _ = unrelated\r\n    target(变量)\r\n}";
    let result = slice(physical, "变量");
    assert_members(
        physical,
        &result,
        GoSliceRole::Definition,
        &["变量 := \"雪\u{2028}\""],
    );
    assert!(
        result
            .witnesses
            .iter()
            .any(|w| w.role == GoSliceRole::Definition
                && physical[w.unit.start_byte..w.unit.end_byte].contains("unrelated := 99")),
        "co-rendered sibling bytes are not dependency membership"
    );
    assert!(result.complete_local);
    let target = criterion(source, "value");
    let mut shared = SourceUnitBudget {
        remaining_source_bytes: source.len() * 2 - 1,
        ..allowance()
    };
    compile_go_expression_slice(
        source,
        target.clone(),
        &mut shared,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(shared.remaining_source_bytes, source.len() - 1);
    assert_eq!(
        compile_go_expression_slice(
            source,
            target.clone(),
            &mut shared,
            &CancellationToken::new()
        ),
        Err(SourceUnitError::SourceByteBudgetExceeded)
    );
    for (mut budget, expected) in [
        (
            SourceUnitBudget {
                remaining_source_bytes: 0,
                ..allowance()
            },
            SourceUnitError::SourceByteBudgetExceeded,
        ),
        (
            SourceUnitBudget {
                remaining_work: 0,
                ..allowance()
            },
            SourceUnitError::WorkBudgetExceeded,
        ),
        (
            SourceUnitBudget {
                max_depth: 0,
                ..allowance()
            },
            SourceUnitError::DepthExceeded,
        ),
    ] {
        assert_eq!(
            compile_go_expression_slice(
                source,
                target.clone(),
                &mut budget,
                &CancellationToken::new()
            ),
            Err(expected)
        );
    }
    let mut measured = allowance();
    compile_go_expression_slice(
        source,
        target.clone(),
        &mut measured,
        &CancellationToken::new(),
    )
    .unwrap();
    let used_work = allowance().remaining_work - measured.remaining_work;
    assert!(
        used_work > source.len(),
        "parsing and analysis must debit shared work"
    );
    let mut shared_work = SourceUnitBudget {
        remaining_work: used_work + source.len(),
        ..allowance()
    };
    compile_go_expression_slice(
        source,
        target.clone(),
        &mut shared_work,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        compile_go_expression_slice(
            source,
            target.clone(),
            &mut shared_work,
            &CancellationToken::new()
        ),
        Err(SourceUnitError::WorkBudgetExceeded)
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let mut budget = allowance();
    let before = budget.clone();
    assert_eq!(
        compile_go_expression_slice(source, target, &mut budget, &cancelled),
        Err(SourceUnitError::Cancelled)
    );
    assert_eq!(budget, before);
    let malformed = "package sample\nfunc run() {\n    target(0)\n";
    assert_eq!(
        compile_go_expression_slice(
            malformed,
            criterion(malformed, "target(0)"),
            &mut allowance(),
            &CancellationToken::new()
        ),
        Err(SourceUnitError::MalformedSource)
    );
    let unicode = source.find('雪').unwrap();
    for range in [
        0..0,
        Range { start: 3, end: 1 },
        source.len()..source.len() + 1,
        unicode + 1..unicode + 2,
        unicode..unicode + '雪'.len_utf8(),
        0..7,
        source.find("value :=").unwrap()..source.find("target").unwrap() + 6,
    ] {
        let result =
            compile_go_expression_slice(source, range, &mut allowance(), &CancellationToken::new())
                .unwrap();
        assert!(!result.complete_local);
        assert!(result.gaps.contains(&GoSliceGap::InvalidCriterion));
    }
    let unsupported = "package sample\nfunc run(ch chan int) {\n    value := 1\n    select {\n    case value = <-ch:\n    default:\n    }\n    target(value)\n}\n";
    let result = slice(unsupported, "target(value)");
    assert!(!result.complete_local);
    assert!(result.gaps.contains(&GoSliceGap::UnsupportedConstruct));
    assert!(boundary_contains(
        unsupported,
        &result,
        GoSliceBoundaryKind::UnsupportedConstruct,
        "select"
    ));
    let backward = "package sample\nfunc run() {\n    value := 1\nagain:\n    target(value)\n    value = 2\n    goto again\n}\n";
    let result = slice(backward, "target(value)");
    assert!(
        !result.complete_local,
        "a later jump can revisit the criterion after a write"
    );
    assert!(result.gaps.contains(&GoSliceGap::UnsupportedConstruct));
    assert!(boundary_contains(
        backward,
        &result,
        GoSliceBoundaryKind::UnsupportedConstruct,
        "goto again"
    ));
}

#[test]
fn adversarial_pointer_read_after_pointee_overwrite_is_current_or_unknown() {
    let source = "package sample\nfunc run(seed int) {\n value := seed\n alias := &value\n value = 2\n target(*alias)\n}\n";
    // Selecting the dereference itself avoids an enclosing call's independent
    // conservative mutation boundary masking a stale pointee snapshot.
    let result = slice(source, "*alias");
    eprintln!("dereference after pointee overwrite result: {result:#?}");
    if result.complete_local {
        assert!(result.gaps.is_empty());
        assert!(texts(source, &result, GoSliceRole::Definition).contains(&"value = 2"));
        assert!(!texts(source, &result, GoSliceRole::Definition).contains(&"value := seed"));
    } else {
        assert!(result.gaps.iter().any(|gap| matches!(
            gap,
            GoSliceGap::UnknownMutation | GoSliceGap::UnsupportedConstruct
        )));
        assert!(
            result.boundaries.iter().any(|boundary| {
                matches!(
                    boundary.kind,
                    GoSliceBoundaryKind::IndirectWrite | GoSliceBoundaryKind::UnsupportedConstruct
                ) && matches!(
                    &source[boundary.range.clone()],
                    "*alias" | "alias" | "&value" | "alias := &value"
                )
            }),
            "unknown dereference must retain the relevant alias operation, not an unrelated call"
        );
    }
}

#[test]
fn adversarial_evaluated_mutation_effects_survive_blank_and_condition_results() {
    let sources = [
        "package sample\nfunc run() {\n value := 1\n _ = mutate(&value)\n target(value)\n}\n",
        "package sample\nfunc run() {\n value := 1\n if mutate(&value) {}\n target(value)\n}\n",
        "package sample\nfunc run() {\n value := 1\n var ignored = mutate(&value)\n _ = ignored\n target(value)\n}\n",
        "package sample\nfunc run() {\n value := 1\n mutate(&value)(value)\n}\n",
    ];
    // Evaluate both paths before the group assertion so one failure does not
    // prevent the second independent effect-discard path being retained.
    let results = sources
        .iter()
        .map(|source| slice(source, "value"))
        .collect::<Vec<_>>();
    let own_source = "package sample\nfunc run(value int) { target(value) }\n";
    let own = slice(own_source, "target(value)");
    let argument_source = "package sample\nfunc run(value int) { target(mutate(&value), value) }\n";
    let argument = slice(argument_source, "target(mutate(&value), value)");
    eprintln!(
        "ordered evaluation effects: {results:#?}; selected own call: {own:#?}; evaluated argument: {argument:#?}"
    );
    assert!(
        own.complete_local && own.gaps.is_empty(),
        "the selected call's own opaque effect is outgoing from its local dependency criterion"
    );
    assert!(
        !argument.complete_local
            && argument.gaps.contains(&GoSliceGap::UnknownMutation)
            && boundary_contains(
                argument_source,
                &argument,
                GoSliceBoundaryKind::IndirectWrite,
                "mutate(&value)"
            ),
        "effects evaluated in an argument are incoming to the enclosing selected call"
    );
    assert!(
        sources.iter().zip(&results).all(|(source, result)| {
            !result.complete_local
                && result.gaps.contains(&GoSliceGap::UnknownMutation)
                && boundary_contains(
                    source,
                    result,
                    GoSliceBoundaryKind::IndirectWrite,
                    "mutate(&value)",
                )
        }),
        "evaluated mutations cannot disappear when their return values are discarded: {results:#?}"
    );
}

#[test]
fn adversarial_short_circuit_rhs_retains_its_left_control_or_a_located_gap() {
    let sources = [
        (
            "package sample\nfunc run(enabled bool, value int) {\n _ = enabled && test(value)\n}\n",
            "enabled && test(value)",
        ),
        (
            "package sample\nfunc run(enabled bool, value int) {\n _ = enabled || test(value)\n}\n",
            "enabled || test(value)",
        ),
    ];
    let results = sources
        .iter()
        .map(|(source, _)| slice(source, "value"))
        .collect::<Vec<_>>();
    eprintln!("short-circuit RHS results: {results:#?}");
    assert!(
        sources
            .iter()
            .zip(&results)
            .all(|((source, binary), result)| {
                let binary_range = criterion(source, binary);
                let right_start = source.rfind("test(value)").unwrap();
                let input = boundary_contains(
                    source,
                    result,
                    GoSliceBoundaryKind::ParameterInput,
                    "enabled",
                );
                let left_control = result.witnesses.iter().any(|witness| {
                    witness.role == GoSliceRole::Control
                        && witness.range.start == binary_range.start
                        && witness.range.end >= binary_range.start + "enabled".len()
                        && witness.range.end <= right_start
                });
                let located_gap = !result.complete_local
                    && result.gaps.contains(&GoSliceGap::UnsupportedConstruct)
                    && result.boundaries.iter().any(|boundary| {
                        boundary.kind == GoSliceBoundaryKind::UnsupportedConstruct
                            && boundary.range == binary_range
                    });
                (input && left_control) || located_gap
            }),
        "both operators conditionally execute their exact RHS criterion: {results:#?}"
    );
}

#[test]
fn adversarial_shadowed_predeclared_type_cannot_erase_possible_reference_effects() {
    // Both are valid Go bindings: a package type declaration and a function
    // type parameter may shadow a predeclared type spelling.
    let sources = [
        (
            "package sample\ntype int []byte\nfunc run(values int) {\n mutate(values)\n target(values[0])\n}\n",
            "type int []byte",
        ),
        (
            "package sample\nfunc run[int ~[]byte](values int) {\n mutate(values)\n target(values[0])\n}\n",
            "int ~[]byte",
        ),
    ];
    let results = sources
        .iter()
        .map(|(source, _)| slice(source, "values[0]"))
        .collect::<Vec<_>>();
    eprintln!("shadowed predeclared type results: {results:#?}");
    assert!(
        sources
            .iter()
            .zip(&results)
            .all(|((source, declaration), result)| {
                let mutation = criterion(source, "mutate(values)");
                let declaration = criterion(source, declaration);
                let parameter = criterion(source, "values int");
                let located = result.boundaries.iter().any(|boundary| {
                    (boundary.kind == GoSliceBoundaryKind::IndirectWrite
                        && boundary.range == mutation)
                        || (boundary.kind == GoSliceBoundaryKind::UnsupportedConstruct
                            && [declaration.clone(), parameter.clone()]
                                .iter()
                                .any(|region| {
                                    region.start <= boundary.range.start
                                        && boundary.range.end <= region.end
                                }))
                });
                !result.complete_local
                    && result.gaps.iter().any(|gap| {
                        matches!(
                            gap,
                            GoSliceGap::UnknownMutation | GoSliceGap::UnsupportedConstruct
                        )
                    })
                    && located
            }),
        "type spelling alone cannot certify a scalar or discard an opaque reference effect: {results:#?}"
    );
}

#[test]
fn ownership_receive_retains_state_read_uncertainty() {
    let sources = [
        "package sample\nfunc run(ch chan int) {\n mutate(ch)\n target(<-ch)\n}\n",
        "package sample\nfunc run(ch chan int) {\n target(<-ch)\n}\n",
    ];
    let results = sources
        .iter()
        .map(|source| slice(source, "<-ch"))
        .collect::<Vec<_>>();
    eprintln!("receive ownership observations: {results:#?}");
    assert!(
        sources.iter().zip(&results).all(|(source, result)| {
            let receive = criterion(source, "<-ch");
            let own_gap = result.boundaries.iter().any(|boundary| {
                boundary.kind == GoSliceBoundaryKind::UnsupportedConstruct
                    && boundary.range == receive
            });
            let prior_content = boundary_contains(
                source,
                result,
                GoSliceBoundaryKind::IndirectWrite,
                "mutate(ch)",
            ) && result.gaps.contains(&GoSliceGap::UnknownMutation);
            !result.complete_local
                && result.gaps.iter().any(|gap| {
                    matches!(
                        gap,
                        GoSliceGap::UnsupportedConstruct | GoSliceGap::UnknownMutation
                    )
                })
                && (own_gap || prior_content)
        }),
        "a receive is not just a copied channel descriptor: {results:#?}"
    );
}

#[test]
fn ownership_saved_method_value_preserves_receiver_uncertainty() {
    let source = "package sample\ntype Number int\nfunc (n *Number) change() { *n = 2 }\nfunc run() {\n var value Number\n change := value.change\n change()\n target(value)\n}\n";
    let result = slice(source, "value");
    eprintln!("saved method-value ownership observation: {result:#?}");
    assert!(!result.complete_local);
    assert!(result.gaps.iter().any(|gap| matches!(
        gap,
        GoSliceGap::UnknownMutation | GoSliceGap::UnsupportedConstruct
    )));
    assert!(
        result.boundaries.iter().any(|boundary| {
            matches!(
                boundary.kind,
                GoSliceBoundaryKind::IndirectWrite | GoSliceBoundaryKind::UnsupportedConstruct
            ) && matches!(
                &source[boundary.range.clone()],
                "value.change" | "change := value.change" | "change()"
            )
        }),
        "ordinary storage cannot erase a possible implicit receiver address"
    );
}

#[test]
fn ownership_scoped_effects_and_loop_invariance_matrix() {
    // Exact fixture argument spans avoid accidentally selecting a later &x or
    // write when the criterion precedes the loop's end. No compiler accessor.
    let argument = |source: &str, name: &str| {
        let call = source.find("target(x, y)").expect("fixture target");
        let start = call + "target(".len() + if name == "x" { 0 } else { "x, ".len() };
        slice_exact(source, start..start + name.len())
    };
    let uncertain = |result: &GoExpressionSlice| {
        !result.complete_local
            && result.gaps.iter().any(|gap| {
                matches!(
                    gap,
                    GoSliceGap::UnknownMutation
                        | GoSliceGap::UnsupportedConstruct
                        | GoSliceGap::LoopCarriedState
                )
            })
    };
    let covered = |source: &str, result: &GoExpressionSlice, spelling: &str| {
        let at = source.find(spelling).expect("fixture source operation");
        result
            .boundaries
            .iter()
            .any(|boundary| boundary.range.start <= at && at + spelling.len() <= boundary.range.end)
    };
    let no_unrelated_writes = |source: &str, result: &GoExpressionSlice| {
        !result.boundaries.iter().any(|boundary| {
            boundary.kind == GoSliceBoundaryKind::IndirectWrite
                && [
                    "remember(alias)",
                    "trigger()",
                    "touch(&struct{}{})",
                    "remember(&struct{ P *int }{P: &x})",
                ]
                .contains(&&source[boundary.range.clone()])
        })
    };

    let exposure_cases = [
        (
            "discard",
            "package sample\nfunc run() {\n x := 1\n y := 2\n _ = &x\n trigger()\n target(x, y)\n}\n",
            "trigger()",
        ),
        (
            "alias-overwrite",
            "package sample\nfunc run() {\n x := 1\n y := 2\n alias := &x\n alias = nil\n trigger()\n target(x, y)\n}\n",
            "trigger()",
        ),
        (
            "branch-join",
            "package sample\nfunc run(flag bool) {\n x := 1\n y := 2\n if flag { _ = &x }\n trigger()\n target(x, y)\n}\n",
            "trigger()",
        ),
        (
            "outer-exposure-survives-scope",
            "package sample\nfunc run() {\n x := 1\n y := 2\n { _ = &x }\n trigger()\n target(x, y)\n}\n",
            "trigger()",
        ),
        (
            "nested-fresh-temporary",
            "package sample\nfunc run() {\n x := 1\n y := 2\n remember(&struct{ P *int }{P: &x})\n trigger()\n target(x, y)\n}\n",
            "&struct{ P *int }{P: &x}",
        ),
        (
            "inherited-loop-exposure",
            "package sample\nfunc run(again bool) {\n x := 1\n y := 2\n _ = &x\n for again {\n  trigger()\n  target(x, y)\n }\n}\n",
            "trigger()",
        ),
        (
            "cycle-exposure-after-criterion",
            "package sample\nfunc run(again bool) {\n x := 1\n y := 2\n for again {\n  trigger()\n  target(x, y)\n  _ = &x\n }\n}\n",
            "&x",
        ),
    ];
    let exposure_results = exposure_cases
        .iter()
        .map(|(name, source, cause)| {
            (
                *name,
                *source,
                *cause,
                argument(source, "x"),
                argument(source, "y"),
            )
        })
        .collect::<Vec<_>>();

    let positive = "package sample\nfunc run(limit int) {\n x := 1\n y := 2\n scratch := 0\n for n := 0; n < limit; n++ {\n  scratch = n\n  touch(&struct{}{})\n  target(x, y)\n }\n}\n";
    let positive_result = argument(positive, "y");
    let shadow = "package sample\nfunc run() {\n x := 1\n y := 2\n { x := 3; _ = &x }\n trigger()\n target(x, y)\n}\n";
    let shadow_result = argument(shadow, "x");
    let empty = "package sample\nfunc run() {\n x := 1\n y := 2\n touch(&struct{}{})\n trigger()\n target(x, y)\n}\n";
    let empty_result = argument(empty, "y");

    let writes = ["x = 3", "x += n", "x, scratch = 3, n"];
    let mut write_results = Vec::new();
    for write in writes {
        for after in [false, true] {
            let body = if after {
                format!("  target(x, y)\n  {write}\n")
            } else {
                format!("  {write}\n  target(x, y)\n")
            };
            let source = format!(
                "package sample\nfunc run(limit int) {{\n x := 1\n y := 2\n scratch := 0\n for n := 0; n < limit; n++ {{\n{body} }}\n}}\n"
            );
            let result = argument(&source, "x");
            write_results.push((source, write, after, result));
        }
    }
    let post = "package sample\nfunc run(again bool) {\n x := 1\n y := 2\n for ; again; x++ {\n  target(x, y)\n }\n}\n";
    let post_result = argument(post, "x");
    let nested = "package sample\nfunc run(again bool) {\n x := 1\n y := 2\n for again {\n  target(x, y)\n  for again { x = 3 }\n }\n}\n";
    let nested_result = argument(nested, "x");
    let unknown_control = "package sample\nfunc run(again bool, ch chan int) {\n x := 1\n y := 2\n for again {\n  select { case <-ch: default: }\n  target(x, y)\n }\n}\n";
    let unknown_result = argument(unknown_control, "y");
    // A zero-argument operation after the criterion can affect referenced
    // storage before the same criterion on a later iteration, even with no
    // exposed local binding targets. A bare y still copies its own binding.
    let shared = "package sample\nfunc run(again bool, data []int) {\n y := 2\n for again {\n  target(data[0], y)\n  trigger()\n }\n}\n";
    let shared_read = slice(shared, "data[0]");
    let shared_bare = slice(shared, "y");

    // All rows are computed before assertions, preserving both the positive
    // precision failure and conservative negative outcomes on this baseline.
    eprintln!(
        "exposure rows: {exposure_results:#?}; positive: {positive_result:#?}; shadow: {shadow_result:#?}; empty temporary: {empty_result:#?}; writes: {write_results:#?}; post: {post_result:#?}; nested: {nested_result:#?}; unsupported control: {unknown_result:#?}"
    );
    eprintln!("whole-cycle shared-content read: {shared_read:#?}; bare binding: {shared_bare:#?}");
    assert!(uncertain(&shared_read) && covered(shared, &shared_read, "trigger()"));
    let shared_start = shared.find("for again {").unwrap();
    let shared_end = shared.rfind("\n }").unwrap() + "\n }".len();
    assert!(
        !shared_bare
            .boundaries
            .iter()
            .any(|boundary| { boundary.range == (shared_start..shared_end) }),
        "content-only uncertainty cannot become an unrelated bare-binding loop dependency"
    );
    for result in [&shared_read, &shared_bare] {
        assert!(texts(shared, result, GoSliceRole::Control).contains(&"for again {"));
    }
    assert!(
        exposure_results.iter().all(|(_, source, cause, x, y)| {
            let loop_controls = !source.contains("for again {")
                || [x, y].iter().all(|result| {
                    texts(source, result, GoSliceRole::Control).contains(&"for again {")
                });
            uncertain(x)
                && covered(source, x, cause)
                && no_unrelated_writes(source, y)
                && loop_controls
        }),
        "exposure persists but does not turn every other local into a write target: {exposure_results:#?}"
    );
    assert!(
        no_unrelated_writes(empty, &empty_result),
        "an empty fresh temporary alone cannot expose unrelated y"
    );
    assert!(
        no_unrelated_writes(shadow, &shadow_result)
            || (uncertain(&shadow_result) && covered(shadow, &shadow_result, "{ x := 3; _ = &x }")),
        "inner identity must not become an optimistic outer-safety proof; located shadow ambiguity is allowed"
    );

    let header = "for n := 0; n < limit; n++ {";
    let header_start = positive.find(header).unwrap();
    let header_end = header_start + header.len();
    let loop_end = positive.rfind("\n }").unwrap() + "\n }".len();
    let post_start = positive.find("n++").unwrap();
    assert!(texts(positive, &positive_result, GoSliceRole::Definition).contains(&"y := 2"));
    assert!(!texts(positive, &positive_result, GoSliceRole::Definition).contains(&"scratch = n"));
    assert!(texts(positive, &positive_result, GoSliceRole::Control).contains(&header));
    assert!(
        positive_result.gaps.contains(&GoSliceGap::LoopCarriedState)
            && !positive_result.complete_local,
        "an invariant value cannot clear carried loop-control uncertainty"
    );
    assert!(
        positive_result.boundaries.iter().any(|boundary| {
            header_start <= boundary.range.start
                && boundary.range.end <= header_end
                && boundary.range.start <= post_start
                && post_start + "n++".len() <= boundary.range.end
        }),
        "the retained control gap must explain the unresolved loop header/post state"
    );
    assert!(
        !positive_result.boundaries.iter().any(|boundary| {
            boundary.kind == GoSliceBoundaryKind::UnsupportedConstruct
                && boundary.range == (header_start..loop_end)
        }),
        "supported control plus an unchanged binding must not use the whole body as its value fallback"
    );
    assert!(no_unrelated_writes(positive, &positive_result));

    assert!(
        write_results.iter().all(|(source, write, _, result)| {
            let controlled = texts(source, result, GoSliceRole::Control)
                .iter()
                .any(|text| text.starts_with("for "));
            controlled
                && (texts(source, result, GoSliceRole::Definition).contains(write)
                    || (uncertain(result) && covered(source, result, write)))
        }),
        "before/after direct, compound and parallel writes need their definitions or owning uncertainty"
    );
    assert!(
        uncertain(&post_result)
            && covered(post, &post_result, "x++")
            && texts(post, &post_result, GoSliceRole::Control).contains(&"for ; again; x++ {")
    );
    assert!(
        uncertain(&nested_result)
            && covered(nested, &nested_result, "x = 3")
            && texts(nested, &nested_result, GoSliceRole::Control).contains(&"for again {")
    );
    assert!(
        uncertain(&unknown_result)
            && covered(
                unknown_control,
                &unknown_result,
                "select { case <-ch: default: }"
            )
            && texts(unknown_control, &unknown_result, GoSliceRole::Control)
                .contains(&"for again {"),
        "unmodeled control is allowed to retain the full owning loop/source fallback"
    );
}

#[test]
fn literal_keys_preserve_ambiguity_value_inputs_and_key_effects() {
    // A named composite alone does not say whether a naked identifier labels a
    // field or evaluates a map/index key. Do not turn spelling into a binding.
    let named = [
        (
            "global-name",
            "package sample\nvar Field = 99\nfunc run(payload int) {\n item := Unknown{Field: payload}\n target(item)\n}\n",
            "Unknown{Field: payload}",
            "Field",
            None,
        ),
        (
            "local-name",
            "package sample\nfunc run(payload int) {\n Field := 99\n item := Unknown{Field: payload}\n target(item)\n}\n",
            "Unknown{Field: payload}",
            "Field",
            Some("Field := 99"),
        ),
        (
            "named-map",
            "package sample\ntype Entries map[int]int\nfunc run(seed, payload int) {\n key := seed\n item := Entries{key: payload}\n target(item)\n}\n",
            "Entries{key: payload}",
            "key",
            Some("key := seed"),
        ),
        (
            "unknown-qualified",
            "package sample\nfunc run(seed, payload int) {\n key := seed\n item := external.Entries{key: payload}\n target(item)\n}\n",
            "external.Entries{key: payload}",
            "key",
            Some("key := seed"),
        ),
    ];
    let observations = named
        .iter()
        .map(|(name, source, literal, key, declaration)| {
            let literal_range = criterion(source, literal);
            let offset = literal.find('{').unwrap() + 1;
            let key_range = literal_range.start + offset..literal_range.start + offset + key.len();
            let selected = compile_go_expression_slice(
                source,
                key_range.clone(),
                &mut allowance(),
                &CancellationToken::new(),
            )
            .expect("valid supplied literal-key syntax");
            (
                *name,
                *source,
                *literal,
                *key,
                *declaration,
                key_range,
                slice(source, literal),
                selected,
            )
        })
        .collect::<Vec<_>>();
    let explicit = "package sample\nfunc run(key, payload int) {\n item := map[int]int{key: payload}\n target(item)\n}\n";
    let explicit_literal = "map[int]int{key: payload}";
    let explicit_result = slice(explicit, explicit_literal);
    let effect_sources = [
        (
            "package sample\nfunc run(payload int) {\n x := 1\n _ = Unknown{choose(&x): payload}\n trigger()\n target(x)\n}\n",
            "choose(&x)",
        ),
        (
            "package sample\nfunc run(payload int) {\n x := 1\n _ = Unknown{choose(&struct{ P *int }{P: &x}): payload}\n trigger()\n target(x)\n}\n",
            "choose(&struct{ P *int }{P: &x})",
        ),
    ];
    let effect_results = effect_sources
        .iter()
        .map(|(source, call)| (*source, *call, slice(source, "x")))
        .collect::<Vec<_>>();
    // Preserve every row before the first baseline label assertion fails.
    eprintln!(
        "named literal/key observations: {observations:#?}; explicit map: {explicit_result:#?}; complex key effects: {effect_results:#?}"
    );

    assert!(
        observations.iter().all(
            |(_, source, literal, _, declaration, key_range, result, _)| {
                let no_value_label = !result.boundaries.iter().any(|boundary| {
                    boundary.kind == GoSliceBoundaryKind::PackageInput
                        && boundary.range == *key_range
                });
                let no_invented_definition = declaration.is_none_or(|declaration| {
                    let range = criterion(source, declaration);
                    !result.witnesses.iter().any(|witness| {
                        matches!(witness.role, GoSliceRole::Binding | GoSliceRole::Definition)
                            && witness.range == range
                    })
                });
                let unresolved_key_source = result.boundaries.iter().any(|boundary| {
                    boundary.kind == GoSliceBoundaryKind::UnsupportedConstruct
                        && boundary.range.start <= key_range.start
                        && key_range.end <= boundary.range.end
                        && criterion(source, literal).start <= boundary.range.start
                        && boundary.range.end <= criterion(source, literal).end
                });
                !result.complete_local
                    && result.gaps.contains(&GoSliceGap::UnsupportedConstruct)
                    && no_value_label
                    && no_invented_definition
                    && unresolved_key_source
                    && boundary_contains(
                        source,
                        result,
                        GoSliceBoundaryKind::ParameterInput,
                        "payload",
                    )
            }
        ),
        "ambiguous named keys need unresolved source, not invented package/local bindings: {observations:#?}"
    );

    assert!(
        observations
            .iter()
            .all(|(_, source, _, _, declaration, key_range, _, selected)| {
                let no_package_key = !selected.boundaries.iter().any(|boundary| {
                    boundary.kind == GoSliceBoundaryKind::PackageInput
                        && boundary.range == *key_range
                });
                let no_local_key = declaration.is_none_or(|declaration| {
                    let range = criterion(source, declaration);
                    !selected.witnesses.iter().any(|witness| {
                        matches!(witness.role, GoSliceRole::Binding | GoSliceRole::Definition)
                            && witness.range == range
                    })
                });
                selected.criterion == *key_range
                    && !selected.complete_local
                    && selected.gaps.iter().any(|gap| {
                        matches!(
                            gap,
                            GoSliceGap::UnsupportedConstruct | GoSliceGap::InvalidCriterion
                        )
                    })
                    && no_package_key
                    && no_local_key
            }),
        "selecting the key itself cannot certify a package/local value use: {observations:#?}"
    );

    assert!(boundary_contains(
        explicit,
        &explicit_result,
        GoSliceBoundaryKind::ParameterInput,
        "payload"
    ));
    let actual_map_key = boundary_contains(
        explicit,
        &explicit_result,
        GoSliceBoundaryKind::ParameterInput,
        "key",
    );
    let unresolved_map_source = !explicit_result.complete_local
        && explicit_result
            .gaps
            .contains(&GoSliceGap::UnsupportedConstruct)
        && explicit_result.boundaries.iter().any(|boundary| {
            boundary.kind == GoSliceBoundaryKind::UnsupportedConstruct
                && boundary.range == criterion(explicit, explicit_literal)
        });
    assert!(
        actual_map_key || unresolved_map_source,
        "a syntactic map key is a value input when modeled, otherwise retain the full unresolved literal"
    );
    assert!(
        effect_results.iter().all(|(source, call, result)| {
            !result.complete_local
                && result.gaps.contains(&GoSliceGap::UnknownMutation)
                && [*call, "trigger()"].iter().any(|effect| {
                    boundary_contains(source, result, GoSliceBoundaryKind::IndirectWrite, effect)
                })
        }),
        "classifying a key cannot skip its calls or nested local-address exposure: {effect_results:#?}"
    );
}
