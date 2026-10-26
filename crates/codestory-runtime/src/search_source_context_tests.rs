use super::{SOURCE_CONTEXT_TEST_HOOK, SourceContextTestStage, search_source_context};
use codestory_contracts::source_context::{
    SOURCE_CONTEXT_BOUNDS, SourceContextDeclineReason as Reason, SourceContextRequest,
    SourceContextResponse, SourceContextStatus as Status, SourceRowObservation,
};
use codestory_indexer::CancellationToken;
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        fs::create_dir(&root).unwrap();
        // Use the native spelling: macOS's default temp parent may itself be
        // reached through /var, which is a symlink to /private/var.
        let root = fs::canonicalize(root).unwrap();
        Self { _temp: temp, root }
    }

    fn write(&self, path: &str, source: impl AsRef<[u8]>) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }

    fn request(&self, observations: Vec<SourceRowObservation>) -> SourceContextRequest {
        SourceContextRequest {
            project: self.root.to_str().unwrap().to_owned(),
            cwd: self.root.to_str().unwrap().to_owned(),
            observations,
            query: None,
        }
    }
}

struct HookGuard;

impl HookGuard {
    fn install(hook: impl for<'a> FnMut(SourceContextTestStage<'a>) + 'static) -> Self {
        SOURCE_CONTEXT_TEST_HOOK.with(|current| {
            assert!(current.borrow().is_none());
            *current.borrow_mut() = Some(Box::new(hook));
        });
        Self
    }
}

impl Drop for HookGuard {
    fn drop(&mut self) {
        SOURCE_CONTEXT_TEST_HOOK.with(|current| *current.borrow_mut() = None);
    }
}

fn observed(path: &str, line: usize, source: &str) -> SourceRowObservation {
    let row = source.split_inclusive('\n').nth(line - 1).unwrap();
    SourceRowObservation {
        path: path.to_owned(),
        physical_line: line,
        text: row.strip_suffix('\n').unwrap_or(row).to_owned(),
    }
}

fn run(request: &SourceContextRequest) -> SourceContextResponse {
    search_source_context(request, &CancellationToken::new())
}

fn has_reason(response: &SourceContextResponse, reason: Reason) -> bool {
    response
        .declines
        .iter()
        .any(|decline| decline.reason == reason)
}

fn with_query(mut request: SourceContextRequest, pattern: &str) -> SourceContextRequest {
    request.query = Some(codestory_contracts::source_context::SourceContextQuery {
        pattern: pattern.to_owned(),
    });
    request
}

#[test]
fn eligible_uniform_sampling_keeps_original_indices_endpoints_and_legacy_prefix() {
    for count in [63usize, 64, 65, 128] {
        let fixture = Fixture::new();
        let source = (0..count)
            .map(|n| format!("fn red_{n:03}() {{}}\n"))
            .collect::<String>();
        fixture.write("source.rs", &source);
        let observations = (1..=count)
            .map(|line| observed("source.rs", line, &source))
            .collect();
        let request = fixture.request(observations);
        let legacy = run(&request);
        assert!(legacy.selection.is_none());
        assert_eq!(legacy.units.last().unwrap().end_line, count.min(64));
        let response = run(&with_query(request, "red|blue"));
        assert_eq!(response.status, Status::BoundedCurrentSource);
        let selection = response
            .selection
            .as_ref()
            .expect("eligible query must expose its actual admitted indices");
        let expected = if count <= 64 {
            (0..count).collect::<Vec<_>>()
        } else {
            (0..64).map(|i| i * (count - 1) / 63).collect()
        };
        assert_eq!(selection.observation_indices, expected);
        assert_eq!(response.coverage.observations_received, count);
        assert_eq!(response.coverage.observations_considered, count.min(64));
        assert_eq!(response.units.last().unwrap().end_line, count);
        if count > 64 {
            assert!(has_reason(&response, Reason::HitLimit));
        }
        assert!(response.coverage.files_read <= SOURCE_CONTEXT_BOUNDS.files);
    }
}

#[test]
fn eligible_owner_ranking_uses_observed_branch_union_not_hit_count_or_unobserved_body() {
    let fixture = Fixture::new();
    let sources = [
        ("repeat.rs", "fn repeated() {\n    red();\n}\n"),
        ("body.rs", "fn body_only() {\n    red();\n    blue();\n}\n"),
        ("diverse.rs", "fn diverse() {\n    red();\n    blue();\n}\n"),
    ];
    for (path, source) in sources {
        fixture.write(path, source);
    }
    let mut observations = vec![observed("repeat.rs", 2, sources[0].1); 20];
    observations.push(observed("body.rs", 2, sources[1].1));
    observations.push(observed("diverse.rs", 2, sources[2].1));
    observations.push(observed("diverse.rs", 3, sources[2].1));
    let request = fixture.request(observations);
    let legacy = run(&request);
    assert_eq!(legacy.units[0].path, "repeat.rs");
    let single = run(&with_query(request.clone(), "red"));
    assert_eq!(single.units, legacy.units);
    assert!(
        single.selection.is_none(),
        "single terms keep ordinary behavior"
    );
    let ranked = run(&with_query(request, "red|blue"));
    assert_eq!(
        ranked.units[0].path, "diverse.rs",
        "two observed alternatives beat repeated identical hits and unobserved body matches"
    );
    assert_eq!(ranked.units[0].body, sources[2].1);
    assert_eq!(
        ranked.units[0].body_sha256,
        format!("{:x}", Sha256::digest(sources[2].1.as_bytes()))
    );
    assert!(ranked.selection.is_some());
    let ranked_sources = [
        (
            "large.rs",
            "fn large() {\n    red();\n    blue();\n    additional_work();\n}\n",
        ),
        ("first.rs", "fn first() {\n    red();\n    blue();\n}\n"),
        ("other.rs", "fn other() {\n    red();\n    blue();\n}\n"),
    ];
    let mut tie_observations = Vec::new();
    for (path, source) in ranked_sources {
        fixture.write(path, source);
        tie_observations.extend([observed(path, 2, source), observed(path, 3, source)]);
    }
    let tie = run(&with_query(fixture.request(tie_observations), "red|blue"));
    assert_eq!(
        tie.units
            .iter()
            .map(|unit| unit.path.as_str())
            .collect::<Vec<_>>(),
        ["first.rs", "other.rs", "large.rs"],
        "equal coverage uses exact standalone cost, then first original index"
    );
    let shared = "impl Holder {\n    fn first() {\n        red(); blue();\n    }\n    fn other() {\n        red(); blue();\n    }\n}\n";
    fixture.write("shared.rs", shared);
    let packet = run(&with_query(
        fixture.request(vec![
            observed("shared.rs", 3, shared),
            observed("shared.rs", 6, shared),
        ]),
        "red|blue",
    ));
    let selection = packet.selection.as_ref().unwrap();
    assert_eq!(
        packet
            .units
            .iter()
            .filter(|unit| unit.kind == "ancestor_header_context")
            .count(),
        1
    );
    assert_eq!(selection.bundles.len(), 2);
    assert_eq!(
        selection.bundles[0].required_header_indices,
        selection.bundles[1].required_header_indices
    );
    assert_eq!(selection.bundles[0].required_header_indices.len(), 1);
    for bundle in &selection.bundles {
        assert_ne!(
            packet.units[bundle.primary_unit_index].kind,
            "ancestor_header_context"
        );
        assert_eq!(
            packet.units[bundle.required_header_indices[0]].body,
            "impl Holder {\n"
        );
    }
}

#[test]
fn query_matcher_dot_counts_unicode_scalars_and_preserves_case_and_shared_work() {
    use codestory_contracts::source_context::SourceContextQuery;
    use codestory_indexer::source_units::SourceUnitBudget;
    let query = |pattern: &str| SourceContextQuery {
        pattern: pattern.into(),
    };
    let budget = || SourceUnitBudget {
        remaining_source_bytes: 10000,
        remaining_work: 10000,
        max_depth: 256,
    };
    let cancel = CancellationToken::new();
    for text in ["pretty_urls", "pretty😀urls", "pretty\rurls"] {
        assert_eq!(
            super::query_branch_bits(&query("red|blue|pretty.urls"), text, &mut budget(), &cancel)
                .unwrap(),
            Some(4)
        );
    }
    for text in ["pretty\nurls", "pretty🙂🙂urls", "RED BLUE"] {
        assert_eq!(
            super::query_branch_bits(&query("red|blue|pretty.urls"), text, &mut budget(), &cancel)
                .unwrap(),
            Some(0)
        );
    }
    assert_eq!(
        super::query_branch_bits(&query("red|blue"), "red red blue", &mut budget(), &cancel)
            .unwrap(),
        Some(3)
    );
    assert_eq!(
        super::query_branch_bits(&query("red|red|blue"), "red blue", &mut budget(), &cancel)
            .unwrap(),
        Some(3),
        "duplicate alternatives never inflate coverage or shift later bits"
    );
    assert_eq!(
        super::query_branch_bits(&query("blue|red|blue"), "blue", &mut budget(), &cancel).unwrap(),
        Some(1),
        "unique alternatives preserve first occurrence order"
    );
    assert_eq!(
        super::query_branch_bits(&query("red|red"), "red", &mut budget(), &cancel).unwrap(),
        None,
        "one effective alternative retains ordinary behavior"
    );
    for pattern in [
        "red",
        "red|",
        "red|b.*",
        "red|b..e",
        "red|bl[ue]",
        "red|雪",
        "red|b\\lue",
    ] {
        assert_eq!(
            super::query_branch_bits(&query(pattern), "red blue", &mut budget(), &cancel).unwrap(),
            None
        );
    }
    let too_many = (0..17)
        .map(|i| format!("p{i:02}"))
        .collect::<Vec<_>>()
        .join("|");
    assert_eq!(
        super::query_branch_bits(&query(&too_many), "p00", &mut budget(), &cancel).unwrap(),
        None
    );
    let too_long = format!("{}|{}", "a".repeat(128), "b".repeat(128));
    assert_eq!(
        super::query_branch_bits(&query(&too_long), "a", &mut budget(), &cancel).unwrap(),
        None
    );
    let mut shared = SourceUnitBudget {
        remaining_work: 64,
        ..budget()
    };
    let mut exhausted = false;
    for _ in 0..100 {
        match super::query_branch_bits(&query("red|blue"), "red", &mut shared, &cancel) {
            Err(Reason::WorkBudgetExceeded) => {
                exhausted = true;
                break;
            }
            Ok(Some(1)) => {}
            other => panic!("eligible shared matching returned {other:?}"),
        }
    }
    assert!(
        exhausted,
        "matching cannot reset or omit shared work charges"
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        super::query_branch_bits(&query("red|blue"), "red", &mut budget(), &cancelled),
        Err(Reason::Cancelled)
    );
}

#[test]
fn eligible_selection_is_invalidated_with_all_units_at_real_final_drift_boundary() {
    let fixture = Fixture::new();
    let source = "fn build() {\n    red();\n}\n";
    fixture.write("source.rs", source);
    let seen = Rc::new(Cell::new(0));
    let stages = seen.clone();
    let path = fixture.root.join("source.rs");
    let _guard = HookGuard::install(move |stage| {
        if matches!(stage, SourceContextTestStage::BeforeFinalRecheck) {
            stages.set(stages.get() + 1);
            fs::write(&path, "fn build() {\n    blue();\n}\n").unwrap();
        }
    });
    let response = run(&with_query(
        fixture.request(vec![observed("source.rs", 2, source)]),
        "red|blue",
    ));
    assert!(seen.get() > 0);
    raw(&response);
    assert!(
        response.selection.is_none(),
        "no dangling bundle references may survive source drift"
    );
    assert!(has_reason(&response, Reason::SourceDrift));
    drop(_guard);
    let valid = "fn valid() {\n    red();\n}\n";
    let invalid = "fn invalid() {\n    red();\n}\n";
    fixture.write("valid.rs", valid);
    fixture.write("invalid.rs", invalid);
    let mut wrong = observed("invalid.rs", 2, invalid);
    wrong.text = "    red(); blue();".into();
    let mismatch = run(&with_query(
        fixture.request(vec![observed("valid.rs", 2, valid), wrong]),
        "red|blue",
    ));
    raw(&mismatch);
    assert!(has_reason(&mismatch, Reason::ObservedRowMismatch));
    assert!(
        mismatch.selection.is_none(),
        "a rejected observation cannot add feature bits or dangling references; preserve the existing global mismatch policy"
    );
}

#[test]
fn eligible_precancel_keeps_the_existing_zero_source_open_guard() {
    let fixture = Fixture::new();
    let source = "fn build() {\n    red();\n}\n";
    fixture.write("source.rs", source);
    let opened = Rc::new(Cell::new(0));
    let count = opened.clone();
    let _guard = HookGuard::install(move |stage| {
        if matches!(stage, SourceContextTestStage::BeforeOpen { .. }) {
            count.set(count.get() + 1);
        }
    });
    let cancel = CancellationToken::new();
    cancel.cancel();
    let response = search_source_context(
        &with_query(
            fixture.request(vec![observed("source.rs", 2, source)]),
            "red|blue",
        ),
        &cancel,
    );
    raw(&response);
    assert!(has_reason(&response, Reason::Cancelled));
    assert_eq!(opened.get(), 0);
    assert!(response.selection.is_none());
}

fn raw(response: &SourceContextResponse) {
    assert_eq!(response.status, Status::RawFallback);
    assert!(
        response.units.is_empty(),
        "unverified partial extras must not escape"
    );
    assert!(!response.declines.is_empty());
}

#[test]
fn exact_units_preserve_observation_order_dedupe_and_one_compile_per_file() {
    let fixture = Fixture::new();
    let source = "def first():\n    return 1\ndef second():\n    return 2\n";
    fixture.write("pkg/source.py", source);
    let absolute = fixture.root.join("pkg/source.py");
    let mut request = fixture.request(vec![
        observed(absolute.to_str().unwrap(), 4, source),
        observed("source.py", 2, source),
        observed("source.py", 4, source),
    ]);
    request.cwd = fixture.root.join("pkg").to_str().unwrap().to_owned();
    let response = run(&request);
    assert_eq!(response.status, Status::BoundedCurrentSource);
    assert_eq!(response.units.len(), 2);
    assert_eq!(response.units[0].body, "def second():\n    return 2\n");
    assert_eq!(response.units[1].body, "def first():\n    return 1\n");
    assert_eq!(
        (response.units[0].start_line, response.units[0].end_line),
        (3, 4)
    );
    assert_eq!(
        (response.units[1].start_line, response.units[1].end_line),
        (1, 2)
    );
    for unit in &response.units {
        assert_eq!(unit.path, "pkg/source.py");
        assert_eq!(
            unit.source_sha256,
            format!("{:x}", Sha256::digest(source.as_bytes()))
        );
        assert_eq!(
            unit.body_sha256,
            format!("{:x}", Sha256::digest(unit.body.as_bytes()))
        );
        assert_eq!(unit.body, source[unit.start_byte..unit.end_byte]);
        assert_eq!(unit.kind, "complete_FunctionDef");
    }
    assert_eq!(response.coverage.observations_received, 3);
    assert_eq!(response.coverage.observations_considered, 3);
    assert_eq!(response.coverage.files_read, 1);
    assert_eq!(response.coverage.files_compiled, 1);
    assert_eq!(response.coverage.source_read_bytes, source.len() * 2);
    assert!(response.current_post_search_snapshot);
    assert!(!response.exhaustive_search && !response.atomic_publication && !response.indexed_proof);
}

#[test]
fn crlf_unicode_and_unterminated_eof_are_literal_not_reformatted() {
    let fixture = Fixture::new();
    let source = "# 雪\r\n@wrap\r\nasync def build():\r\n    return '🙂'";
    fixture.write("source.py", source);
    let response = run(&fixture.request(vec![
        observed("source.py", 2, source),
        observed("source.py", 4, source),
    ]));
    assert_eq!(response.status, Status::BoundedCurrentSource);
    assert_eq!(response.units.len(), 1);
    let unit = &response.units[0];
    assert_eq!(unit.body, "@wrap\r\nasync def build():\r\n    return '🙂'");
    assert_eq!(unit.body, source[unit.start_byte..unit.end_byte]);
    assert_eq!(unit.kind, "complete_AsyncFunctionDef");
    assert_eq!(
        unit.body_sha256,
        format!("{:x}", Sha256::digest(unit.body.as_bytes()))
    );
}

#[test]
fn rust_and_python_observations_share_order_deduplication_and_verified_source() {
    let fixture = Fixture::new();
    let rust =
        "// 雪\r\n#[inline]\r\nfn first() {\r\n    let text = \"🙂\";\r\n}\r\nfn second() {}";
    let python = "def build():\n    return 1\n";
    fixture.write("pkg/source.rs", rust);
    fixture.write("pkg/source.py", python);
    let absolute = fixture.root.join("pkg/source.rs");
    let mut request = fixture.request(vec![
        observed(absolute.to_str().unwrap(), 6, rust),
        observed("source.py", 2, python),
        observed("source.rs", 4, rust),
        observed("source.rs", 6, rust),
    ]);
    request.cwd = fixture.root.join("pkg").to_str().unwrap().to_owned();
    let response = run(&request);
    assert_eq!(response.status, Status::BoundedCurrentSource);
    assert!(
        response.declines.is_empty(),
        "supported Rust must reach compilation"
    );
    assert_eq!(response.units.len(), 3);
    assert_eq!(response.units[0].body, "fn second() {}");
    assert_eq!(response.units[1].body, python);
    assert_eq!(
        response.units[2].body,
        "#[inline]\r\nfn first() {\r\n    let text = \"🙂\";\r\n}\r\n"
    );
    assert_eq!(
        (response.units[0].start_line, response.units[0].end_line),
        (6, 6)
    );
    assert_eq!(
        (response.units[2].start_line, response.units[2].end_line),
        (2, 5)
    );
    for unit in &response.units {
        let source = if unit.path.ends_with(".rs") {
            rust
        } else {
            python
        };
        assert_eq!(unit.body, source[unit.start_byte..unit.end_byte]);
        assert_eq!(
            unit.source_sha256,
            format!("{:x}", Sha256::digest(source.as_bytes()))
        );
        assert_eq!(
            unit.body_sha256,
            format!("{:x}", Sha256::digest(unit.body.as_bytes()))
        );
    }
    assert_eq!(response.units[0].kind, "complete_function_item");
    assert_eq!(response.units[1].kind, "complete_FunctionDef");
    assert_eq!(response.coverage.files_read, 2);
    assert_eq!(response.coverage.files_compiled, 2);
    assert_eq!(
        response.coverage.source_read_bytes,
        (rust.len() + python.len()) * 2
    );
    assert!(response.current_post_search_snapshot);
    assert!(!response.exhaustive_search && !response.atomic_publication && !response.indexed_proof);
}

#[test]
fn rust_refuses_malformed_owners_and_oversized_whole_units_without_clipping() {
    let fixture = Fixture::new();
    fixture.write("broken.rs", "fn valid() {}\nfn broken() {\n");
    let broken = run(&fixture.request(vec![observed(
        "broken.rs",
        1,
        "fn valid() {}\nfn broken() {\n",
    )]));
    raw(&broken);
    assert!(has_reason(&broken, Reason::MalformedSource));
    let body = format!(
        "fn build() {{\n    let text = \"{}\";\n}}\n",
        "x".repeat(SOURCE_CONTEXT_BOUNDS.display_source_bytes)
    );
    fixture.write("large.rs", &body);
    let large = run(&fixture.request(vec![observed("large.rs", 1, &body)]));
    raw(&large);
    assert!(has_reason(&large, Reason::UnitTooLarge));
    assert!(large.coverage.source_read_bytes <= SOURCE_CONTEXT_BOUNDS.aggregate_read_bytes);
}

#[test]
fn one_observed_row_mismatch_invalidates_all_extra_units() {
    let fixture = Fixture::new();
    let first = "def first():\n    return 1\n";
    let second = "def second():\n    return 2\r\n";
    fixture.write("first.py", first);
    fixture.write("second.py", second);
    let mut wrong = observed("second.py", 2, second);
    wrong.text = wrong.text.trim_end_matches('\r').to_owned();
    let response = run(&fixture.request(vec![observed("first.py", 2, first), wrong]));
    raw(&response);
    assert!(has_reason(&response, Reason::ObservedRowMismatch));
}

#[test]
fn source_change_before_final_recheck_invalidates_all_files() {
    let fixture = Fixture::new();
    let source = "def build():\n    return 1\n";
    fixture.write("first.py", source);
    fixture.write("second.py", source);
    let stages = Rc::new(Cell::new(0));
    let seen = stages.clone();
    let path = fixture.root.join("first.py");
    let _guard = HookGuard::install(move |stage| {
        if matches!(stage, SourceContextTestStage::BeforeFinalRecheck) {
            seen.set(seen.get() + 1);
            fs::write(&path, "def build():\n    return 2\n").unwrap();
        }
    });
    let response = run(&fixture.request(vec![
        observed("first.py", 2, source),
        observed("second.py", 2, source),
    ]));
    assert!(
        stages.get() > 0,
        "actual final recheck stage was not exercised"
    );
    raw(&response);
    assert!(has_reason(&response, Reason::SourceDrift));
}

#[test]
fn replacing_project_identity_before_recheck_refuses_existing_extras() {
    let fixture = Fixture::new();
    let source = "def build():\n    return 1\n";
    fixture.write("source.py", source);
    let root = fixture.root.clone();
    let moved = root.with_file_name("old-project");
    let stages = Rc::new(Cell::new(0));
    let seen = stages.clone();
    let _guard = HookGuard::install(move |stage| {
        if matches!(stage, SourceContextTestStage::BeforeFinalRecheck) && seen.get() == 0 {
            seen.set(1);
            fs::rename(&root, &moved).unwrap();
            fs::create_dir(&root).unwrap();
            fs::write(root.join("source.py"), source).unwrap();
        }
    });
    let response = run(&fixture.request(vec![observed("source.py", 2, source)]));
    assert_eq!(stages.get(), 1);
    raw(&response);
    assert!(has_reason(&response, Reason::SourceDrift));
}

#[test]
fn explicit_project_cwd_and_source_paths_cannot_escape() {
    let fixture = Fixture::new();
    let source = "def build():\n    return 1\n";
    fixture.write("source.py", source);
    let outside = fixture.root.with_file_name("outside.py");
    fs::write(&outside, source).unwrap();
    let mut relative_project = fixture.request(vec![observed("source.py", 2, source)]);
    relative_project.project = ".".to_owned();
    let mut wrong_cwd = fixture.request(vec![observed("source.py", 2, source)]);
    wrong_cwd.cwd = fixture.root.parent().unwrap().to_str().unwrap().to_owned();
    let traversal = fixture.request(vec![observed("../outside.py", 2, source)]);
    let absolute_outside = fixture.request(vec![observed(outside.to_str().unwrap(), 2, source)]);
    for request in [relative_project, wrong_cwd, traversal, absolute_outside] {
        raw(&run(&request));
    }
}

#[test]
fn malformed_binary_and_unsupported_sources_have_explicit_no_extra_declines() {
    let fixture = Fixture::new();
    fixture.write("broken.py", "def broken(:\n    pass\n");
    fixture.write("binary.py", b"def build():\n    return 1\n\xff");
    fixture.write("source.txt", "def build():\n    return 1\n");
    for (path, reason, text) in [
        ("broken.py", Reason::MalformedSource, "def broken(:"),
        ("binary.py", Reason::InvalidUtf8, "def build():"),
        ("source.txt", Reason::UnsupportedSource, "def build():"),
    ] {
        let request = fixture.request(vec![SourceRowObservation {
            path: path.to_owned(),
            physical_line: 1,
            text: text.to_owned(),
        }]);
        let response = run(&request);
        raw(&response);
        assert!(has_reason(&response, reason));
    }
}

#[test]
fn hit_and_file_limits_keep_bounded_coverage_and_first_seen_units() {
    let fixture = Fixture::new();
    let source = "def build():\n    return 1\n";
    fixture.write("source.py", source);
    let response = run(&fixture.request(vec![
        observed("source.py", 2, source);
        SOURCE_CONTEXT_BOUNDS.hits + 1
    ]));
    assert_eq!(response.status, Status::BoundedCurrentSource);
    assert_eq!(
        response.coverage.observations_considered,
        SOURCE_CONTEXT_BOUNDS.hits
    );
    assert_eq!(response.units.len(), 1);
    assert!(has_reason(&response, Reason::HitLimit));
    let observations = (0..=SOURCE_CONTEXT_BOUNDS.files)
        .map(|n| {
            let path = format!("file_{n}.py");
            fixture.write(&path, source);
            observed(&path, 2, source)
        })
        .collect();
    let response = run(&fixture.request(observations));
    assert_eq!(response.status, Status::BoundedCurrentSource);
    assert_eq!(response.coverage.files_read, SOURCE_CONTEXT_BOUNDS.files);
    assert_eq!(response.units.len(), SOURCE_CONTEXT_BOUNDS.files);
    assert!(has_reason(&response, Reason::FileLimit));
}

#[test]
fn file_and_complete_unit_byte_limits_decline_without_clipping() {
    let fixture = Fixture::new();
    let source = format!(
        "def build():\n    return '{}'\n",
        "x".repeat(SOURCE_CONTEXT_BOUNDS.display_source_bytes)
    );
    fixture.write("source.py", &source);
    let response = run(&fixture.request(vec![observed("source.py", 1, &source)]));
    raw(&response);
    assert!(has_reason(&response, Reason::UnitTooLarge));
    let large = format!(
        "#{}\ndef build(): return 1\n",
        "x".repeat(SOURCE_CONTEXT_BOUNDS.per_file_bytes)
    );
    fixture.write("large.py", &large);
    let response = run(&fixture.request(vec![observed("large.py", 2, &large)]));
    raw(&response);
    assert!(has_reason(&response, Reason::FileTooLarge));
}

#[test]
fn initial_reads_and_final_rechecks_share_one_aggregate_byte_budget() {
    let fixture = Fixture::new();
    let source = format!("#{}\ndef build():\n    return 1\n", "x".repeat(150_000));
    let observations = (0..8)
        .map(|n| {
            let path = format!("file_{n}.py");
            fixture.write(&path, &source);
            observed(&path, 3, &source)
        })
        .collect();
    let response = run(&fixture.request(observations));
    raw(&response);
    assert!(has_reason(&response, Reason::ReadBudgetExceeded));
    assert!(response.coverage.source_read_bytes <= SOURCE_CONTEXT_BOUNDS.aggregate_read_bytes);
}

#[test]
fn cancellation_at_real_open_boundary_does_not_publish_partial_context() {
    let fixture = Fixture::new();
    let source = "def build():\n    return 1\n";
    fixture.write("source.py", source);
    let cancellation = CancellationToken::new();
    let cancelled = cancellation.clone();
    let stages = Rc::new(Cell::new(0));
    let seen = stages.clone();
    let _guard = HookGuard::install(move |stage| {
        if matches!(stage, SourceContextTestStage::BeforeOpen { .. }) {
            seen.set(seen.get() + 1);
            cancelled.cancel();
        }
    });
    let response = search_source_context(
        &fixture.request(vec![observed("source.py", 2, source)]),
        &cancellation,
    );
    assert!(stages.get() > 0);
    raw(&response);
    assert!(has_reason(&response, Reason::Cancelled));
}

#[test]
fn pre_cancelled_request_performs_no_source_open() {
    let fixture = Fixture::new();
    let source = "def build():\n    return 1\n";
    fixture.write("source.py", source);
    let opens = Rc::new(Cell::new(0));
    let seen = opens.clone();
    let _guard = HookGuard::install(move |stage| {
        if matches!(stage, SourceContextTestStage::BeforeOpen { .. }) {
            seen.set(seen.get() + 1);
        }
    });
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let response = search_source_context(
        &fixture.request(vec![observed("source.py", 2, source)]),
        &cancellation,
    );
    raw(&response);
    assert!(has_reason(&response, Reason::Cancelled));
    assert_eq!(opens.get(), 0);
}

#[cfg(unix)]
#[test]
fn source_context_real_open_swaps_are_bounded_and_never_read_outside() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    for mode in ["fifo", "leaf_symlink", "ancestor_symlink"] {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "search_source_context::tests::source_context_open_swap_child",
                "--nocapture",
            ])
            .env("CODESTORY_SOURCE_CONTEXT_OPEN_SWAP", mode)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("{mode} blocked at source open/read boundary");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{mode}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(unix)]
#[test]
fn source_context_open_swap_child() {
    use std::os::unix::fs::{MetadataExt, symlink};
    let Ok(mode) = std::env::var("CODESTORY_SOURCE_CONTEXT_OPEN_SWAP") else {
        return;
    };
    let fixture = Fixture::new();
    let source = "def build():\n    return 1\n";
    fixture.write("nested/source.py", source);
    let outside_dir = fixture.root.with_file_name("outside");
    fs::create_dir(&outside_dir).unwrap();
    let outside = outside_dir.join("source.py");
    fs::write(&outside, "def outside():\n    return 'owned canary'\n").unwrap();
    let outside_meta = fs::metadata(&outside).unwrap();
    let outside_id = (outside_meta.dev(), outside_meta.ino());
    let opened = Rc::new(Cell::new(0));
    let did_swap = opened.clone();
    let outside_reads = Rc::new(Cell::new(0));
    let reads = outside_reads.clone();
    let outside_bytes = Rc::new(Cell::new(0));
    let bytes_read = outside_bytes.clone();
    let root = fixture.root.clone();
    let mode_for_hook = mode.clone();
    let _guard = HookGuard::install(move |stage| match stage {
        SourceContextTestStage::BeforeOpen { relative }
            if relative.ends_with(Path::new("source.py")) && did_swap.get() == 0 =>
        {
            did_swap.set(1);
            let target = root.join("nested/source.py");
            match mode_for_hook.as_str() {
                "fifo" => {
                    fs::remove_file(&target).unwrap();
                    assert!(
                        std::process::Command::new("/usr/bin/mkfifo")
                            .arg(&target)
                            .status()
                            .unwrap()
                            .success()
                    );
                }
                "leaf_symlink" => {
                    fs::remove_file(&target).unwrap();
                    symlink(&outside, &target).unwrap();
                }
                "ancestor_symlink" => {
                    fs::rename(root.join("nested"), root.join("saved")).unwrap();
                    symlink(&outside_dir, root.join("nested")).unwrap();
                }
                _ => panic!("unexpected fixture mode"),
            }
        }
        SourceContextTestStage::BeforeRead { file } => {
            let m = file.metadata().unwrap();
            if (m.dev(), m.ino()) == outside_id {
                reads.set(reads.get() + 1);
            }
        }
        SourceContextTestStage::AfterRead { file, bytes } => {
            let m = file.metadata().unwrap();
            if (m.dev(), m.ino()) == outside_id {
                bytes_read.set(bytes_read.get() + bytes);
            }
        }
        _ => {}
    });
    let response = run(&fixture.request(vec![observed("nested/source.py", 2, source)]));
    assert_eq!(opened.get(), 1, "real pre-open mutation stage must execute");
    assert_eq!(
        outside_reads.get(),
        0,
        "outside native handle must never reach read"
    );
    assert_eq!(
        outside_bytes.get(),
        0,
        "outside bytes must never be consumed"
    );
    raw(&response);
    println!("mode={mode} actual_swap=1 outside_read_handles=0 outside_bytes=0");
}
