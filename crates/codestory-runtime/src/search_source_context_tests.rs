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
