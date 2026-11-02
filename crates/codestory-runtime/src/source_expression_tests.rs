use super::*;
use tempfile::TempDir;

fn request(root: &TempDir, source: &str, expression: &str) -> SourceExpressionRequest {
    std::fs::write(root.path().join("unit.go"), source).unwrap();
    let at = source.rfind(expression).unwrap();
    SourceExpressionRequest {
        project: root.path().to_string_lossy().into_owned(),
        path: "unit.go".into(),
        line: source.as_bytes()[..at]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
            + 1,
        expression: expression.into(),
    }
}

#[test]
fn source_expression_matches_exact_physical_utf8_and_projects_source_once() {
    let root = TempDir::new().unwrap();
    let source = "package sample\r\nfunc run(seed int) {\r\n  变量 := seed\r\n  target(变量)\r\n}";
    let req = request(&root, source, "变量");
    let result = analyze_expression(&req, &CancellationToken::new());
    assert_ne!(result.status, AnalysisStatus::Unavailable, "{result:?}");
    assert!(result.snapshot.as_ref().unwrap().revalidated);
    assert!(!result.snapshot.as_ref().unwrap().indexed_proof);
    assert!(!result.snapshot.as_ref().unwrap().atomic_publication);
    assert_eq!(
        result.snapshot.as_ref().unwrap().source_sha256,
        format!("{:x}", sha2::Sha256::digest(source.as_bytes()))
    );
    let mut previous_end = 0;
    for segment in &result.segments {
        assert!(segment.start_byte >= previous_end);
        assert_eq!(segment.source, source[segment.start_byte..segment.end_byte]);
        assert_eq!(
            segment.body_sha256,
            format!("{:x}", sha2::Sha256::digest(segment.source.as_bytes()))
        );
        previous_end = segment.end_byte;
    }
    for (range, index) in result
        .witnesses
        .iter()
        .map(|w| (&w.range, w.segment))
        .chain(result.boundaries.iter().map(|b| (&b.range, b.segment)))
    {
        let segment = &result.segments[index];
        assert!(
            segment.start_byte <= range.start_byte
                && range.start_byte < range.end_byte
                && range.end_byte <= segment.end_byte
        );
        assert!(source.get(range.start_byte..range.end_byte).is_some());
    }
    assert!(
        result
            .witnesses
            .iter()
            .any(|w| w.role == AnalysisRole::Definition
                && &source[w.range.start_byte..w.range.end_byte] == "变量 := seed")
    );
    let multiline = "package sample\r\nfunc run(value int) {\r\n target(\r\n value,\r\n )\r\n}";
    let req = request(&root, multiline, "target(\r\n value,\r\n )");
    assert_ne!(
        analyze_expression(&req, &CancellationToken::new()).status,
        AnalysisStatus::Unavailable
    );
    let opaque =
        "package sample\nfunc run(value int) {\n if outside { touch(&value) }\n target(value)\n}\n";
    let req = request(&root, opaque, "value");
    let mapped = analyze_expression(&req, &CancellationToken::new());
    assert!(
        mapped
            .boundaries
            .iter()
            .any(|b| b.kind == AnalysisBoundaryKind::UnresolvedBinding
                && &opaque[b.range.start_byte..b.range.end_byte] == "outside")
    );
    assert!(
        mapped
            .boundaries
            .iter()
            .any(|b| b.kind == AnalysisBoundaryKind::PossibleOpaqueMutation
                && &opaque[b.range.start_byte..b.range.end_byte] == "touch(&value)")
    );
}

#[test]
fn source_expression_declines_ambiguous_missing_and_nonexpression_without_echo() {
    let root = TempDir::new().unwrap();
    let source = "package sample\nfunc run(value int) {\n target(value, value)\n // value\n _ = \"fragment\"\n}\n";
    let mut req = request(&root, source, "value");
    req.line = 3;
    let ambiguous = analyze_expression(&req, &CancellationToken::new());
    assert_eq!(
        ambiguous.decline,
        Some(AnalysisDecline::AmbiguousExpression)
    );
    assert_eq!(ambiguous.matching_candidates_at_least, Some(2));
    assert!(ambiguous.snapshot.is_none() && ambiguous.segments.is_empty());
    for (line, expression, reason) in [
        (4, "value", AnalysisDecline::InvalidCriterion),
        (5, "fragment", AnalysisDecline::InvalidCriterion),
        (3, "absent", AnalysisDecline::MissingExpression),
        (0, "value", AnalysisDecline::InvalidLine),
        (99, "value", AnalysisDecline::InvalidLine),
    ] {
        req.line = line;
        req.expression = expression.into();
        assert_eq!(
            analyze_expression(&req, &CancellationToken::new()).decline,
            Some(reason)
        );
    }
    let overlap = "package sample\nfunc run(value int) {\n target(value+value+value)\n}\n";
    let overlapping = request(&root, overlap, "value+value");
    assert_eq!(
        analyze_expression(&overlapping, &CancellationToken::new()).decline,
        Some(AnalysisDecline::AmbiguousExpression)
    );
    let many = format!(
        "package sample\nfunc run() {{\n// {}\n}}\n",
        "x ".repeat(50_000)
    );
    req = request(&root, &many, "x");
    req.line = 3;
    let result = analyze_expression(&req, &CancellationToken::new());
    assert_eq!(result.decline, Some(AnalysisDecline::AmbiguousExpression));
    assert!(serde_json::to_vec(&result).unwrap().len() < 2048);
}

#[test]
fn source_expression_is_project_scoped_and_fail_closed_on_source_boundaries() {
    let a = TempDir::new().unwrap();
    let b = TempDir::new().unwrap();
    let ra = request(
        &a,
        "package sample\nfunc run() {\n x := 1\n target(x)\n}\n",
        "x",
    );
    let rb = request(
        &b,
        "package sample\nfunc run() {\n x := 2\n target(x)\n}\n",
        "x",
    );
    let aa = analyze_expression(&ra, &CancellationToken::new());
    let bb = analyze_expression(&rb, &CancellationToken::new());
    assert_ne!(
        aa.snapshot.unwrap().source_sha256,
        bb.snapshot.unwrap().source_sha256
    );
    let mut invalid = ra.clone();
    invalid.path = "../unit.go".into();
    assert_eq!(
        analyze_expression(&invalid, &CancellationToken::new()).decline,
        Some(AnalysisDecline::InvalidPath)
    );
    invalid.path = "unit.py".into();
    assert_eq!(
        analyze_expression(&invalid, &CancellationToken::new()).decline,
        Some(AnalysisDecline::UnsupportedSource)
    );
    std::fs::write(a.path().join("unit.go"), vec![b' '; MAX_SOURCE_BYTES + 1]).unwrap();
    assert_eq!(
        analyze_expression(&ra, &CancellationToken::new()).decline,
        Some(AnalysisDecline::FileTooLarge)
    );
    let declarations = (0..1500)
        .map(|n| format!("v{n} := {n}\n"))
        .collect::<String>();
    let heavy = format!("package sample\nfunc run() {{\n{declarations}target(0)\n}}\n");
    let bounded = request(&a, &heavy, "target(0)");
    assert_eq!(
        analyze_expression(&bounded, &CancellationToken::new()).decline,
        Some(AnalysisDecline::WorkBudgetExceeded)
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    invalid.project = "/no/such/project".into();
    assert_eq!(
        analyze_expression(&invalid, &cancellation).decline,
        Some(AnalysisDecline::Cancelled)
    );
    #[cfg(unix)]
    {
        std::fs::remove_file(a.path().join("unit.go")).unwrap();
        std::os::unix::fs::symlink(b.path().join("unit.go"), a.path().join("unit.go")).unwrap();
        let result = analyze_expression(&ra, &CancellationToken::new());
        assert_eq!(result.status, AnalysisStatus::Unavailable);
        assert!(result.snapshot.is_none());
    }
}

#[test]
fn source_expression_never_clips_required_unsupported_source_to_fit() {
    let root = TempDir::new().unwrap();
    let source = format!(
        "package sample\nfunc run(ch chan int) {{\n select {{\n default:\n // {}\n target(0)\n }}\n}}\n",
        "required_context".repeat(2000)
    );
    let req = request(&root, &source, "target(0)");
    let response = analyze_expression(&req, &CancellationToken::new());
    assert_eq!(
        response.decline,
        Some(AnalysisDecline::ResultBudgetExceeded)
    );
    assert_eq!(response.status, AnalysisStatus::Unavailable);
    assert!(
        response.segments.is_empty()
            && response.witnesses.is_empty()
            && response.snapshot.is_none()
    );
}
