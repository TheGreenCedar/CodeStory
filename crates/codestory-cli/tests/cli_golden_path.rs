use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};
use tempfile::tempdir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn write_tiny_rust_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "tiny-browser-fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("write Cargo.toml");

    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src dir");
    fs::write(
        src.join("lib.rs"),
        r#"pub mod runtime;

pub struct AppController {
    project_name: String,
}

impl AppController {
    pub fn new(project_name: impl Into<String>) -> Self {
        Self {
            project_name: project_name.into(),
        }
    }

    pub fn open_project(&self) -> String {
        open_project(&self.project_name)
    }

    pub fn run_indexing(&self) -> usize {
        run_indexing(&self.open_project())
    }
}

pub fn open_project(project_name: &str) -> String {
    runtime::normalize_project(project_name)
}

pub fn run_indexing(project_path: &str) -> usize {
    runtime::schedule_index(project_path)
}
"#,
    )
    .expect("write lib.rs");
    fs::write(
        src.join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("workspace:{project_name}")
}

pub fn schedule_index(project_path: &str) -> usize {
    super::open_project(project_path).len()
}
"#,
    )
    .expect("write runtime.rs");
}

#[test]
fn callers_text_shows_only_direct_call_sites() {
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    write_tiny_rust_workspace(workspace.path());
    run_cli_json(
        workspace.path(),
        cache.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    let output = run_cli(
        workspace.path(),
        cache.path(),
        &["callers", "--query", "normalize_project"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("text response");
    assert_eq!(
        text.trim_end(),
        "open_project
src/lib.rs:24: runtime::normalize_project(project_name)"
    );
}

#[test]
fn lookup_setup_round_trips_existing_host_notes() {
    let home = tempdir().expect("scratch home");
    let original = b"# Existing notes\r\n\r\nKeep this text without a trailing newline";
    for (host, home_var, filename) in [
        ("codex", "CODEX_HOME", "AGENTS.md"),
        ("claude-code", "CLAUDE_CONFIG_DIR", "CLAUDE.md"),
    ] {
        let host_home = home.path().join(host);
        fs::create_dir(&host_home).expect("host directory");
        let instructions = host_home.join(filename);
        fs::write(&instructions, original).expect("prior instructions");
        let run = |extra: &[&str]| {
            test_support::cli_command()
                .env("HOME", home.path())
                .env("USERPROFILE", home.path())
                .env(home_var, &host_home)
                .args(["setup", host])
                .args(extra)
                .output()
                .expect("run setup")
        };
        let first = run(&[]);
        assert!(
            first.status.success(),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        let installed = fs::read(&instructions).expect("installed instructions");
        assert!(installed.starts_with(original));
        assert!(installed.windows(12).any(|bytes| bytes == b"cs refs NAME"));
        assert!(run(&[]).status.success());
        assert_eq!(fs::read(&instructions).expect("repeat install"), installed);
        assert!(run(&["--remove"]).status.success());
        assert_eq!(
            fs::read(&instructions).expect("restored instructions"),
            original
        );

        fs::write(&instructions, b"<!-- codestory-lookups:start -->\n").expect("incomplete block");
        assert!(
            !run(&[]).status.success(),
            "an incomplete block was overwritten"
        );
        assert_eq!(
            fs::read(&instructions).expect("untouched incomplete block"),
            b"<!-- codestory-lookups:start -->\n"
        );
    }
}

#[test]
fn lookup_setup_repo_note_is_owned_until_removed() {
    let home = tempdir().expect("scratch home");
    let repo = tempdir().expect("scratch checkout");
    run_git(repo.path(), &["init", "-q"]);
    let nested = repo.path().join("nested");
    fs::create_dir(&nested).expect("nested directory");
    let run = |extra: &[&str]| {
        test_support::cli_command()
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("CODEX_HOME", home.path().join(".codex"))
            .current_dir(&nested)
            .args(["setup", "codex", "--repo"])
            .args(extra)
            .output()
            .expect("run repo setup")
    };
    let root_note = repo.path().join("AGENTS.md");
    let first = run(&[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(root_note.is_file());
    assert!(!nested.join("AGENTS.md").exists());
    assert!(!home.path().join(".codex/AGENTS.md").exists());
    assert!(run(&["--remove"]).status.success());
    assert!(!root_note.exists());
}

/// Run a lookup that must succeed and return its reply.
fn run_lookup(workspace: &Path, cache_dir: &Path, args: &[&str]) -> String {
    let output = run_cli(workspace, cache_dir, args);
    assert!(
        output.status.success(),
        "lookup failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("text reply")
}

fn index_lookup_fixture(workspace: &Path, cache_dir: &Path) {
    run_cli_json(
        workspace,
        cache_dir,
        &["index", "--refresh", "full", "--format", "json"],
    );
}

/// Two methods share the name `apply`; one call is split over lines, one
/// lives in a test file, and `apply_all` / `reapply` only contain the name.
fn write_shared_name_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "shared-name-fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("write Cargo.toml");
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src dir");
    fs::write(
        src.join("lib.rs"),
        r#"pub mod audit;
pub mod ledger;

use ledger::Ledger;

pub fn settle(amount: u32) -> u32 {
    let ledger = Ledger::default();
    ledger
        .apply(amount)
}

pub fn apply_all(amounts: &[u32]) -> u32 {
    amounts.iter().map(|amount| settle(*amount)).sum()
}
"#,
    )
    .expect("write lib.rs");
    fs::write(
        src.join("ledger.rs"),
        r#"#[derive(Default)]
pub struct Ledger {
    pub total: u32,
}

impl Ledger {
    pub fn apply(&self, amount: u32) -> u32 {
        self.total + amount
    }
}
"#,
    )
    .expect("write ledger.rs");
    fs::write(
        src.join("audit.rs"),
        r#"pub struct Audit;

impl Audit {
    pub fn apply(&self, note: &str) -> usize {
        note.len()
    }

    pub fn reapply(&self, note: &str) -> usize {
        self.apply(note) * 2
    }
}
"#,
    )
    .expect("write audit.rs");
    let tests = root.join("tests");
    fs::create_dir_all(&tests).expect("create tests dir");
    fs::write(
        tests.join("ledger_test.rs"),
        r#"use shared_name_fixture::ledger::Ledger;

#[test]
fn applies_in_tests() {
    assert_eq!(Ledger::default().apply(2), 2);
}
"#,
    )
    .expect("write ledger_test.rs");
}

#[test]
fn find_and_refs_answer_from_live_text_without_an_index() {
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    write_tiny_rust_workspace(workspace.path());
    let lookup = |args: &[&str]| run_lookup(workspace.path(), cache.path(), args);

    assert_eq!(
        lookup(&["find", "schedule_index", "normalize_project", "--no-bodies"]),
        "no index; text matches only
4 matches in 2 files
src/lib.rs
  24 runtime::normalize_project(project_name)
  28 runtime::schedule_index(project_path)
src/runtime.rs
  1 pub fn normalize_project(project_name: &str) -> String {
  5 pub fn schedule_index(project_path: &str) -> usize {
"
    );
    assert_eq!(
        lookup(&["find", "--file-glob", "lib.rs"]),
        "1 files\nsrc/lib.rs\n"
    );
    // A glob narrows the search; past the limit every remaining hit is counted.
    assert_eq!(
        lookup(&[
            "find",
            "project",
            "--file-glob",
            "runtime.rs",
            "--limit",
            "2",
            "--no-bodies",
        ]),
        "no index; text matches only
4 matches in 1 files
src/runtime.rs
  1 pub fn normalize_project(project_name: &str) -> String {
  2 format!(\"workspace:{project_name}\")
src/runtime.rs: 2 more
"
    );

    assert_eq!(
        lookup(&["refs", "open_project", "--no-bodies"]),
        "no index; text matches only
5 references in 2 files
src/lib.rs
  14 pub fn open_project(&self) -> String {
  15 open_project(&self.project_name)
  19 run_indexing(&self.open_project())
  23 pub fn open_project(project_name: &str) -> String {
src/runtime.rs
  6 super::open_project(project_path).len()
"
    );
    // Whole words only: `project` is never more than part of a name here.
    assert_eq!(
        lookup(&["refs", "project"]),
        "no index; text matches only\n0 references in 0 files\n"
    );
    // Without an index a body is the few lines around a hit, never a `==`
    // declaration; hits close together share one window.
    assert_eq!(
        lookup(&["refs", "run_indexing"]),
        "no index; text matches only
3 references in 1 files
src/lib.rs
  15:         open_project(&self.project_name)
  16:     }
  17:
  18>     pub fn run_indexing(&self) -> usize {
  19>         run_indexing(&self.open_project())
  20:     }
  21: }
  22:
  ...
  24:     runtime::normalize_project(project_name)
  25: }
  26:
  27> pub fn run_indexing(project_path: &str) -> usize {
  28:     runtime::schedule_index(project_path)
  29: }
"
    );

    assert_eq!(
        lookup(&["read", "src/runtime.rs::schedule_index", "src/lib.rs:1-3"]),
        "== src/runtime.rs:5-7 schedule_index (not from index; approximate)
5: pub fn schedule_index(project_path: &str) -> usize {
6:     super::open_project(project_path).len()
7: }
== src/lib.rs:1-3
1: pub mod runtime;
2:
3: pub struct AppController {
"
    );

    assert_eq!(
        fs::read_dir(cache.path()).expect("list cache dir").count(),
        0,
        "a lookup must not create cache state"
    );
}

#[test]
fn refs_lists_every_whole_word_hit_with_enclosing_symbols() {
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    write_shared_name_workspace(workspace.path());
    index_lookup_fixture(workspace.path(), cache.path());
    let lookup = |args: &[&str]| run_lookup(workspace.path(), cache.path(), args);

    // Both definitions are listed instead of an ambiguity error. The call
    // split over lines is found on the line that names it, the call in the
    // test file is listed, and `apply_all` / `reapply` are not mentions.
    assert_eq!(
        lookup(&["refs", "apply", "--no-bodies"]),
        "apply: 2 definition(s)
  src/audit.rs:4 method Audit::apply
  src/ledger.rs:7 method Ledger::apply
3 references in 3 files
src/audit.rs
  4 [Audit::apply] pub fn apply(&self, note: &str) -> usize {
  9 [Audit::reapply] self.apply(note) * 2 -> src/audit.rs:4
src/ledger.rs
  7 [Ledger::apply] pub fn apply(&self, amount: u32) -> u32 {
src/lib.rs
  9 [settle] .apply(amount) -> src/ledger.rs:7
tests/ledger_test.rs
  5 [applies_in_tests] assert_eq!(Ledger::default().apply(2), 2);
"
    );

    // A qualifier keeps the unresolved hits and drops only what the index
    // resolved to the other definition, and says how many it dropped.
    assert_eq!(
        lookup(&["refs", "Ledger::apply", "--no-bodies"]),
        "Ledger::apply: 1 definition(s)
  src/ledger.rs:7 method Ledger::apply
2 references in 2 files
src/ledger.rs
  7 [Ledger::apply] pub fn apply(&self, amount: u32) -> u32 {
src/lib.rs
  9 [settle] .apply(amount)
tests/ledger_test.rs
  5 [applies_in_tests] assert_eq!(Ledger::default().apply(2), 2);
2 hit(s) omitted: the index resolved them to other definitions of apply
"
    );
    assert!(lookup(&["refs", "Audit.apply", "--no-bodies"]).starts_with(
        "Audit.apply: 1 definition(s)
  src/audit.rs:4 method Audit::apply
2 references in 2 files
"
    ));

    // Past the limit nothing is dropped silently, and the definitions are
    // still found in files the listing no longer reaches.
    assert_eq!(
        lookup(&["refs", "apply", "--limit", "2", "--no-bodies"]),
        "apply: 2 definition(s)
  src/audit.rs:4 method Audit::apply
  src/ledger.rs:7 method Ledger::apply
3 references in 3 files
src/audit.rs
  4 [Audit::apply] pub fn apply(&self, note: &str) -> usize {
  9 [Audit::reapply] self.apply(note) * 2 -> src/audit.rs:4
src/ledger.rs: 1 more
src/lib.rs: 1 more
tests/ledger_test.rs: 1 more
"
    );

    // By default each definition leads with its declaration, and every other
    // mention is marked inside the declaration that holds it.
    assert_eq!(
        lookup(&["refs", "apply"]),
        "apply: 2 definition(s)
  src/audit.rs:4 method Audit::apply
  == Audit::apply 4-6
  4>     pub fn apply(&self, note: &str) -> usize {
  5:         note.len()
  6:     }
  src/ledger.rs:7 method Ledger::apply
  == Ledger::apply 7-9
  7>     pub fn apply(&self, amount: u32) -> u32 {
  8:         self.total + amount
  9:     }
3 references in 3 files
src/audit.rs
  == Audit::reapply 8-10
  8:     pub fn reapply(&self, note: &str) -> usize {
  9>         self.apply(note) * 2 -> src/audit.rs:4
  10:     }
src/lib.rs
  == settle 6-10
  6: pub fn settle(amount: u32) -> u32 {
  7:     let ledger = Ledger::default();
  8:     ledger
  9>         .apply(amount) -> src/ledger.rs:7
  10: }
tests/ledger_test.rs
  == applies_in_tests 3-6
  3: #[test]
  4: fn applies_in_tests() {
  5>     assert_eq!(Ledger::default().apply(2), 2);
  6: }
"
    );
}

#[test]
fn lookups_verify_only_the_files_they_serve() {
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    write_tiny_rust_workspace(workspace.path());
    index_lookup_fixture(workspace.path(), cache.path());
    let published = cache_file_snapshots(cache.path());
    let process_state = cache_file_snapshots(&test_support::test_state_root());
    let lookup = |args: &[&str]| run_lookup(workspace.path(), cache.path(), args);

    let indexed = "open_project: 2 definition(s)
  src/lib.rs:14 method AppController::open_project
  src/lib.rs:23 function open_project
3 references in 2 files
src/lib.rs
  14 [AppController::open_project] pub fn open_project(&self) -> String {
  15 [AppController::open_project] open_project(&self.project_name) -> src/lib.rs:23
  19 [AppController::run_indexing] run_indexing(&self.open_project()) -> src/lib.rs:14
  23 [open_project] pub fn open_project(project_name: &str) -> String {
src/runtime.rs
  6 [schedule_index] super::open_project(project_path).len()
";
    assert_eq!(lookup(&["refs", "open_project", "--no-bodies"]), indexed);

    // Shift every line of one hit file and add a file the index never saw.
    let lib = workspace.path().join("src/lib.rs");
    let original = fs::read_to_string(&lib).expect("read lib.rs");
    fs::write(&lib, format!("// moved down\n// by two lines\n{original}")).expect("edit lib.rs");
    fs::write(
        workspace.path().join("src/extra.rs"),
        "pub fn reopen() -> String {\n    crate::open_project(\"again\")\n}\n",
    )
    .expect("write extra.rs");

    // The changed file is served from its live bytes: new line numbers, no
    // index facts, and a mark. The untouched file keeps its symbols.
    let listing = r#"src/extra.rs (not indexed)
  2 crate::open_project("again")
src/lib.rs (changed since indexing)
  16 pub fn open_project(&self) -> String {
  17 open_project(&self.project_name)
  21 run_indexing(&self.open_project())
  25 pub fn open_project(project_name: &str) -> String {
src/runtime.rs
  6 [schedule_index] super::open_project(project_path).len()
"#;
    assert_eq!(
        lookup(&["refs", "open_project", "--no-bodies"]),
        format!("open_project: 0 definition(s)\n6 references in 3 files\n{listing}")
    );
    assert_eq!(
        lookup(&["find", "open_project", "--no-bodies"]),
        format!("6 matches in 3 files\n{listing}")
    );
    // Bodies are live text too, and read nothing the listing did not.
    assert!(lookup(&["refs", "open_project"]).contains(
        "src/extra.rs (not indexed)
  1: pub fn reopen() -> String {
  2>     crate::open_project(\"again\")
  3: }
src/lib.rs (changed since indexing)
  13:         }
"
    ));

    // Verification is by content, not by timestamp: the same bytes written
    // again are the bytes the index describes.
    fs::write(&lib, original).expect("restore lib.rs");
    fs::remove_file(workspace.path().join("src/extra.rs")).expect("remove extra.rs");
    assert_eq!(lookup(&["refs", "open_project", "--no-bodies"]), indexed);

    assert_eq!(
        cache_file_snapshots(cache.path()),
        published,
        "lookups must not refresh, activate, or write the project cache"
    );
    assert_eq!(
        cache_file_snapshots(&test_support::test_state_root()),
        process_state,
        "lookups must not write process cache state"
    );
}

#[test]
fn read_returns_complete_declarations_for_many_selectors() {
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    write_tiny_rust_workspace(workspace.path());
    let notes = (1..=305)
        .map(|line| format!("note {line}\n"))
        .collect::<String>();
    fs::write(workspace.path().join("notes.txt"), notes).expect("write notes.txt");
    index_lookup_fixture(workspace.path(), cache.path());
    let lookup = |args: &[&str]| run_lookup(workspace.path(), cache.path(), args);

    // One call, every selector form; a bad selector gets its own line.
    assert_eq!(
        lookup(&[
            "read",
            "src/lib.rs::AppController::open_project",
            "src/lib.rs::AppController.run_indexing",
            "src/runtime.rs:6",
            "src/lib.rs:23-25",
            "src/missing.rs::anything",
            "src/lib.rs::absent",
            "nonsense",
            "src/lib.rs::",
        ]),
        "== src/lib.rs:14-16 AppController::open_project
14:     pub fn open_project(&self) -> String {
15:         open_project(&self.project_name)
16:     }
uses:
  src/lib.rs:23-25 function open_project
== src/lib.rs:18-20 AppController::run_indexing
18:     pub fn run_indexing(&self) -> usize {
19:         run_indexing(&self.open_project())
20:     }
uses:
  src/lib.rs:27-29 function run_indexing
== src/runtime.rs:5-7 schedule_index
5: pub fn schedule_index(project_path: &str) -> usize {
6:     super::open_project(project_path).len()
7: }
== src/lib.rs:23-25
23: pub fn open_project(project_name: &str) -> String {
24:     runtime::normalize_project(project_name)
25: }
== src/missing.rs::anything: no file src/missing.rs
== src/lib.rs::absent: no symbol absent in src/lib.rs; definitions there: AppController, \
AppController::new, AppController::open_project, AppController::run_indexing, open_project, \
run_indexing
== nonsense: no file nonsense
== src/lib.rs::: expected path, path::Symbol, path:LINE or path:A-B
"
    );

    // A long range is cut at 300 lines and names the range that continues.
    let long = lookup(&["read", "notes.txt:1-305"]);
    assert!(
        long.starts_with("== notes.txt:1-305\n1: note 1\n"),
        "{long}"
    );
    assert!(
        long.ends_with("300: note 300\n... 5 more lines; read notes.txt:301-305\n"),
        "{long}"
    );

    // After an edit the index no longer describes the file, so the range is
    // approximated from the live text and says so.
    let runtime = workspace.path().join("src/runtime.rs");
    let original = fs::read_to_string(&runtime).expect("read runtime.rs");
    fs::write(&runtime, format!("// shifted\n{original}")).expect("edit runtime.rs");
    assert_eq!(
        lookup(&["read", "src/runtime.rs::schedule_index", "src/runtime.rs:3"]),
        "== src/runtime.rs:6-8 schedule_index (not from index; approximate)
6: pub fn schedule_index(project_path: &str) -> usize {
7:     super::open_project(project_path).len()
8: }
== src/runtime.rs:1-8 (not from index; approximate)
1: // shifted
2: pub fn normalize_project(project_name: &str) -> String {
3:     format!(\"workspace:{project_name}\")
4: }
5:
6: pub fn schedule_index(project_path: &str) -> usize {
7:     super::open_project(project_path).len()
8: }
"
    );
}

#[test]
fn read_includes_anchor_in_long_indexed_declaration() {
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    write_tiny_rust_workspace(workspace.path());
    let source = format!(
        "\npub fn long_declaration() -> u32 {{\n{}    42\n}}\n",
        "    // body\n".repeat(640)
    );
    fs::write(workspace.path().join("src/lib.rs"), source).expect("write long declaration");
    index_lookup_fixture(workspace.path(), cache.path());
    let lookup = |selector| run_lookup(workspace.path(), cache.path(), &["read", selector]);

    let symbol = lookup("src/lib.rs::long_declaration");
    assert!(symbol.starts_with("== src/lib.rs:2-644 long_declaration\n2: pub fn long_declaration"));
    assert!(symbol.ends_with("301:     // body\n... 343 more lines; read src/lib.rs:302-644\n"));
    assert_eq!(lookup("src/lib.rs:301"), symbol);

    let boundary = lookup("src/lib.rs:302");
    assert!(boundary.lines().any(|row| row == "302:     // body"));
    assert!(boundary.ends_with("589:     // body\n... 55 more lines; read src/lib.rs:590-644\n"));

    let late = lookup("src/lib.rs:643");
    let header = late.lines().next().expect("declaration header");
    assert!(header.ends_with(" long_declaration"), "{header}");
    assert!(
        late.lines().any(|row| row == "643:     42"),
        "line selector omitted line 643 from {header}"
    );
}

/// `tally` is defined once, called twice from `twice`, imported outside any
/// declaration, and called three times from `render`, which is 170 lines long.
fn write_body_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "body-fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("write Cargo.toml");
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src dir");
    fs::write(
        src.join("lib.rs"),
        r#"pub mod report;

pub fn tally(values: &[u32]) -> u32 {
    values.iter().sum()
}

pub fn twice(values: &[u32]) -> u32 {
    let first = tally(values);
    let second = tally(values);
    first + second
}
"#,
    )
    .expect("write lib.rs");
    let mut report = String::from(
        "use crate::tally;\n\npub fn render(values: &[u32]) -> String {\n    let mut out = String::new();\n    let mut total = tally(values);\n",
    );
    for line in 6..=170 {
        report.push_str(&match line {
            9 | 170 => "    total += tally(values);\n".to_owned(),
            _ => format!("    out.push_str(\"row {line}\");\n"),
        });
    }
    report.push_str("    format!(\"{out}{total}\")\n}\n");
    fs::write(src.join("report.rs"), report).expect("write report.rs");
}

#[test]
fn refs_and_find_carry_enclosing_declarations_within_budget() {
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    write_body_workspace(workspace.path());
    index_lookup_fixture(workspace.path(), cache.path());
    let lookup = |args: &[&str]| run_lookup(workspace.path(), cache.path(), args);
    let rows = |lines: std::ops::RangeInclusive<u32>| {
        lines
            .map(|line| format!("  {line}:     out.push_str(\"row {line}\");\n"))
            .collect::<String>()
    };

    // The definition's declaration comes first and is not repeated below.
    // `twice` is printed once for its two hits. `render` is too long to print
    // whole: a window around each hit, the two near hits sharing one. The
    // import has no enclosing declaration, so it gets plain context.
    let definition = "tally: 1 definition(s)
  src/lib.rs:3 function tally
  == tally 3-5
  3> pub fn tally(values: &[u32]) -> u32 {
  4:     values.iter().sum()
  5: }
6 references in 2 files
src/lib.rs
  == twice 7-11
  7: pub fn twice(values: &[u32]) -> u32 {
  8>     let first = tally(values);
  9>     let second = tally(values);
  10:     first + second
  11: }
";
    let near_hits = format!(
        "src/report.rs
  1> use crate::tally;
  2:
  3: pub fn render(values: &[u32]) -> String {{
  4:     let mut out = String::new();
  == render 3-172
  3: pub fn render(values: &[u32]) -> String {{
  4:     let mut out = String::new();
  5>     let mut total = tally(values);
{}  9>     total += tally(values);
{}",
        rows(6..=8),
        rows(10..=21)
    );
    assert_eq!(
        lookup(&["refs", "tally"]),
        format!(
            "{definition}{near_hits}  ...
{}  170>     total += tally(values);
  171:     format!(\"{{out}}{{total}}\")
  172: }}
",
            rows(158..=169)
        )
    );

    // A body that does not fit the budget leaves its hit as a one-line row,
    // and the reply says how to read what it left out.
    assert_eq!(
        lookup(&["refs", "tally", "--body-lines", "34"]),
        format!(
            "{definition}{near_hits}  170 [render] total += tally(values);
more: read <path>::<Symbol> or <path>:A-B
"
        )
    );

    // `find` shows hits the same way; the declaring line is one more hit.
    assert_eq!(
        lookup(&["find", "tally", "--file-glob", "lib.rs"]),
        "3 matches in 1 files
src/lib.rs
  == tally 3-5
  3> pub fn tally(values: &[u32]) -> u32 {
  4:     values.iter().sum()
  5: }
  == twice 7-11
  7: pub fn twice(values: &[u32]) -> u32 {
  8>     let first = tally(values);
  9>     let second = tally(values);
  10:     first + second
  11: }
"
    );

    // The index no longer describes a changed file, so its hits get three
    // lines of live context each way and no declaration.
    let report = workspace.path().join("src/report.rs");
    let original = fs::read_to_string(&report).expect("read report.rs");
    fs::write(&report, format!("// moved\n{original}")).expect("edit report.rs");
    assert_eq!(
        lookup(&["refs", "tally"]),
        format!(
            "{definition}src/report.rs (changed since indexing)
  1: // moved
  2> use crate::tally;
  3:
  4: pub fn render(values: &[u32]) -> String {{
  5:     let mut out = String::new();
  6>     let mut total = tally(values);
  7:     out.push_str(\"row 6\");
  8:     out.push_str(\"row 7\");
  9:     out.push_str(\"row 8\");
  10>     total += tally(values);
  11:     out.push_str(\"row 10\");
  12:     out.push_str(\"row 11\");
  13:     out.push_str(\"row 12\");
  ...
  168:     out.push_str(\"row 167\");
  169:     out.push_str(\"row 168\");
  170:     out.push_str(\"row 169\");
  171>     total += tally(values);
  172:     format!(\"{{out}}{{total}}\")
  173: }}
"
        )
    );
}

#[test]
fn read_bare_path_prints_small_files_and_outlines_large_ones() {
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    write_tiny_rust_workspace(workspace.path());
    let mut long = String::from(
        "pub struct Counter {
    pub total: u32,
}

impl Counter {
    pub fn bump(&mut self) -> u32 {
        self.total += 1;
        self.total
    }
}

async fn outer() -> u32 {
    fn inner() -> u32 {
        1
    }
    let local = {
        inner()
    };
    local
}

pub fn long() -> u32 {
    let mut count = 0;
",
    );
    long.push_str(&"    count += 1;\n".repeat(400));
    long.push_str("    count\n}\n");
    let long_path = workspace.path().join("src/long.rs");
    fs::write(&long_path, &long).expect("write long.rs");
    let notes = |lines: u32| {
        (1..=lines)
            .map(|line| format!("note {line}\n"))
            .collect::<String>()
    };
    fs::write(workspace.path().join("notes.txt"), notes(305)).expect("write notes.txt");
    fs::write(workspace.path().join("log.txt"), notes(405)).expect("write log.txt");
    index_lookup_fixture(workspace.path(), cache.path());
    let lookup = |args: &[&str]| run_lookup(workspace.path(), cache.path(), args);

    // A short file is printed whole. A long one is outlined from the index,
    // nested declarations indented, in the same call.
    assert_eq!(
        lookup(&["read", "src/runtime.rs", "src/long.rs"]),
        "== src/runtime.rs:1-7
1: pub fn normalize_project(project_name: &str) -> String {
2:     format!(\"workspace:{project_name}\")
3: }
4:
5: pub fn schedule_index(project_path: &str) -> usize {
6:     super::open_project(project_path).len()
7: }
== src/long.rs (425 lines; outline)
1-3 struct Counter
  2-2 field Counter::total
6-9 method Counter::bump
12-20 function outer
  13-15 function inner
22-425 function long
read src/long.rs::<Symbol> or src/long.rs:A-B for bodies
"
    );

    // Once the file changes the outline is the lines that start a declaration
    // and open a block, and says so. Neither outline lists a local binding.
    fs::write(&long_path, format!("// shifted\n{long}")).expect("edit long.rs");
    assert_eq!(
        lookup(&["read", "src/long.rs"]),
        "== src/long.rs (426 lines; outline) (not from index; approximate)
2-4 pub struct Counter {
6-11 impl Counter {
  7-10 pub fn bump(&mut self) -> u32 {
13-21 async fn outer() -> u32 {
  14-16 fn inner() -> u32 {
23-426 pub fn long() -> u32 {
read src/long.rs::<Symbol> or src/long.rs:A-B for bodies
"
    );

    // A file of up to 400 lines is printed whole, past the 300 lines a range
    // is cut at.
    let whole = lookup(&["read", "notes.txt"]);
    assert!(
        whole.starts_with("== notes.txt:1-305\n1: note 1\n"),
        "{whole}"
    );
    assert!(whole.ends_with("304: note 304\n305: note 305\n"), "{whole}");

    // A long file with nothing to outline is read from the top.
    let plain = lookup(&["read", "log.txt"]);
    assert!(
        plain.starts_with("== log.txt:1-405\n1: note 1\n"),
        "{plain}"
    );
    assert!(
        plain.ends_with("300: note 300\n... 105 more lines; read log.txt:301-405\n"),
        "{plain}"
    );
}

/// `build` calls four functions, one on each line: `Plan::scale` in another
/// file, `short_step`, `long_step`, which is 89 lines long, and `retry`,
/// which calls `build` back. Four methods are named `run`; each calls `tick`.
fn write_uses_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "uses-fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("write Cargo.toml");
    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src dir");
    let mut lib = String::from(
        "pub mod jobs;
pub mod plan;

use plan::Plan;

pub fn build(seed: u32) -> u32 {
    let plan = Plan::default();
    let scaled = plan.scale(seed);
    let first = short_step(scaled);
    let second = long_step(first);
    retry(second)
}

pub fn retry(value: u32) -> u32 {
    if value > 10 { build(value - 10) } else { value }
}

pub fn short_step(value: u32) -> u32 {
    value + 1
}

pub fn long_step(value: u32) -> u32 {
    let mut total = value;
",
    );
    lib.push_str(&"    total += 1;\n".repeat(85));
    lib.push_str("    total\n}\n");
    fs::write(src.join("lib.rs"), lib).expect("write lib.rs");
    fs::write(
        src.join("plan.rs"),
        "#[derive(Default)]
pub struct Plan {
    pub factor: u32,
}

impl Plan {
    pub fn scale(&self, seed: u32) -> u32 {
        seed * self.factor
    }
}
",
    )
    .expect("write plan.rs");
    let mut jobs = String::from("fn tick() -> u32 {\n    1\n}\n");
    for name in ["A", "B", "C", "D"] {
        jobs.push_str(&format!(
            "\npub struct {name};\n\nimpl {name} {{\n    pub fn run(&self) -> u32 {{\n        tick()\n    }}\n}}\n"
        ));
    }
    fs::write(src.join("jobs.rs"), jobs).expect("write jobs.rs");
}

#[test]
fn refs_carries_the_declarations_its_definition_uses() {
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    write_uses_workspace(workspace.path());
    index_lookup_fixture(workspace.path(), cache.path());
    let lookup = |args: &[&str]| run_lookup(workspace.path(), cache.path(), args);

    // After the references come the declarations `build` uses, in order of
    // first use. A short one is printed whole under its file and a long one
    // is a row that says where to read it. `retry` is already printed as a
    // reference, so it is not repeated.
    let definition = "build: 1 definition(s)
  src/lib.rs:6 function build
  == build 6-12
  6> pub fn build(seed: u32) -> u32 {
  7:     let plan = Plan::default();
  8:     let scaled = plan.scale(seed);
  9:     let first = short_step(scaled);
  10:     let second = long_step(first);
  11:     retry(second)
  12: }
1 references in 1 files
src/lib.rs
";
    let retry = "  == retry 14-16
  14: pub fn retry(value: u32) -> u32 {
  15>     if value > 10 { build(value - 10) } else { value }
  16: }
build uses:
";
    let scale = "src/plan.rs
  == Plan::scale 7-9
  7:     pub fn scale(&self, seed: u32) -> u32 {
  8:         seed * self.factor
  9:     }
";
    let short_step = "src/lib.rs
  == short_step 18-20
  18: pub fn short_step(value: u32) -> u32 {
  19:     value + 1
  20: }
";
    let long_step = "  src/lib.rs:22-110 function long_step\n";
    assert_eq!(
        lookup(&["refs", "build"]),
        format!("{definition}{retry}{scale}{short_step}{long_step}")
    );

    // The one-line listing stays as it was.
    assert_eq!(
        lookup(&["refs", "build", "--no-bodies"]),
        "build: 1 definition(s)
  src/lib.rs:6 function build
1 references in 1 files
src/lib.rs
  6 [build] pub fn build(seed: u32) -> u32 {
  15 [retry] if value > 10 { build(value - 10) } else { value }
"
    );

    // The definition comes first. A used declaration that what is left does
    // not cover becomes a row, and the reply says once how to read what it
    // left out.
    let rows = "  src/lib.rs:18-20 function short_step\n  src/lib.rs:22-110 function long_step\n";
    let more = "more: read <path>::<Symbol> or <path>:A-B\n";
    assert_eq!(
        lookup(&["refs", "build", "--body-lines", "16"]),
        format!("{definition}{retry}{scale}{rows}{more}")
    );
    // The references may spend 60% of what the definition left when the used
    // declarations want the rest: three of five lines do not cover `retry`,
    // and the lines it could not spend go to `Plan::scale`.
    assert_eq!(
        lookup(&["refs", "build", "--body-lines", "13"]),
        format!(
            "{definition}  15 [retry] if value > 10 {{ build(value - 10) }} else {{ value }}
build uses:
{scale}{rows}  src/lib.rs:14-16 function retry
{more}"
        )
    );

    // More than three definitions get no bodies, so nothing follows what
    // they use; a qualifier that leaves one of them does.
    let ambiguous = lookup(&["refs", "run"]);
    assert!(
        ambiguous.starts_with("run: 4 definition(s)\n"),
        "{ambiguous}"
    );
    assert!(!ambiguous.contains("uses:"), "{ambiguous}");
    let qualified = lookup(&["refs", "A::run"]);
    assert!(
        qualified.ends_with(
            "A::run uses:
src/jobs.rs
  == tick 1-3
  1: fn tick() -> u32 {
  2:     1
  3: }
"
        ),
        "{qualified}"
    );

    // The index no longer describes a changed file, so nothing in it is
    // named as used.
    let plan = workspace.path().join("src/plan.rs");
    let original = fs::read_to_string(&plan).expect("read plan.rs");
    fs::write(&plan, format!("// moved\n{original}")).expect("edit plan.rs");
    assert_eq!(
        lookup(&["refs", "build"]),
        format!("{definition}{retry}{short_step}{long_step}")
    );
}

#[test]
fn read_lists_the_declarations_a_symbol_uses() {
    let workspace = tempdir().expect("workspace");
    let cache = tempdir().expect("cache");
    write_uses_workspace(workspace.path());
    index_lookup_fixture(workspace.path(), cache.path());
    let lookup = |args: &[&str]| run_lookup(workspace.path(), cache.path(), args);

    // A declaration read by symbol or by line is followed by where the
    // declarations it uses are, in order of first use. `retry` uses only
    // `build`, which this reply has already printed. A range and a bare path
    // get no such rows.
    let build = "== src/lib.rs:6-12 build
6: pub fn build(seed: u32) -> u32 {
7:     let plan = Plan::default();
8:     let scaled = plan.scale(seed);
9:     let first = short_step(scaled);
10:     let second = long_step(first);
11:     retry(second)
12: }
";
    let local_uses = "  src/lib.rs:18-20 function short_step
  src/lib.rs:22-110 function long_step
  src/lib.rs:14-16 function retry
";
    assert_eq!(
        lookup(&[
            "read",
            "src/lib.rs::build",
            "src/lib.rs:15",
            "src/lib.rs:14-16",
            "src/plan.rs",
        ]),
        format!(
            "{build}uses:
  src/plan.rs:7-9 method Plan::scale
{local_uses}== src/lib.rs:14-16 retry
14: pub fn retry(value: u32) -> u32 {{
15:     if value > 10 {{ build(value - 10) }} else {{ value }}
16: }}
== src/lib.rs:14-16
14: pub fn retry(value: u32) -> u32 {{
15:     if value > 10 {{ build(value - 10) }} else {{ value }}
16: }}
== src/plan.rs:1-10
1: #[derive(Default)]
2: pub struct Plan {{
3:     pub factor: u32,
4: }}
5:
6: impl Plan {{
7:     pub fn scale(&self, seed: u32) -> u32 {{
8:         seed * self.factor
9:     }}
10: }}
"
        )
    );
    assert_eq!(
        lookup(&["read", "src/lib.rs:15"]),
        "== src/lib.rs:14-16 retry
14: pub fn retry(value: u32) -> u32 {
15:     if value > 10 { build(value - 10) } else { value }
16: }
uses:
  src/lib.rs:6-12 function build
"
    );

    // Nothing in a file that changed since indexing is named as used.
    let plan = workspace.path().join("src/plan.rs");
    let original = fs::read_to_string(&plan).expect("read plan.rs");
    fs::write(&plan, format!("// moved\n{original}")).expect("edit plan.rs");
    assert_eq!(
        lookup(&["read", "src/lib.rs::build"]),
        format!("{build}uses:\n{local_uses}")
    );

    // A declaration approximated from a changed file has no index facts to
    // follow.
    let lib = workspace.path().join("src/lib.rs");
    let original = fs::read_to_string(&lib).expect("read lib.rs");
    fs::write(&lib, format!("// moved\n{original}")).expect("edit lib.rs");
    assert_eq!(
        lookup(&["read", "src/lib.rs::retry"]),
        "== src/lib.rs:15-17 retry (not from index; approximate)
15: pub fn retry(value: u32) -> u32 {
16:     if value > 10 { build(value - 10) } else { value }
17: }
"
    );
}

fn write_docker_compose_fixture(root: &Path) {
    let docker = root.join("docker");
    fs::create_dir_all(&docker).expect("create docker dir");
    fs::write(
        docker.join("app-compose.yml"),
        r#"services:
  app:
    image: tiny-browser-fixture:latest
"#,
    )
    .expect("write docker compose");
}

fn run_cli(workspace: &Path, cache_dir: &Path, args: &[&str]) -> std::process::Output {
    let mut command = test_support::cli_command();
    command
        .args(args)
        .arg("--project")
        .arg(workspace)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env("CODESTORY_TEST_EMBED_ALLOW_CPU", "1");
    command.output().expect("run codestory-cli")
}

fn run_cli_with_stdin(
    workspace: &Path,
    cache_dir: &Path,
    args: &[&str],
    stdin: &str,
) -> std::process::Output {
    let mut child = test_support::cli_command()
        .args(args)
        .arg("--project")
        .arg(workspace)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env("CODESTORY_TEST_EMBED_ALLOW_CPU", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn codestory-cli");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait codestory-cli")
}

fn run_cli_with_embedding_env(
    workspace: &Path,
    cache_dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> std::process::Output {
    let mut command = test_support::cli_command();
    command
        .args(args)
        .arg("--project")
        .arg(workspace)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env("CODESTORY_TEST_EMBED_ALLOW_CPU", "1");
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().expect("run codestory-cli")
}

fn run_cli_json(workspace: &Path, cache_dir: &Path, args: &[&str]) -> Value {
    let output = run_cli(workspace, cache_dir, args);
    assert!(
        output.status.success(),
        "command failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse json output")
}

fn run_cli_json_with_embedding_env(
    workspace: &Path,
    cache_dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Value {
    let output = run_cli_with_embedding_env(workspace, cache_dir, args, envs);
    assert!(
        output.status.success(),
        "command failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse json output")
}

fn doctor_next_commands(doctor: &Value) -> Vec<&str> {
    doctor["next_commands"]
        .as_array()
        .expect("doctor next commands")
        .iter()
        .map(|command| command.as_str().expect("next command string"))
        .collect()
}

fn assert_no_agent_proof_commands(commands: &[&str], context: &str) {
    let joined = commands.join("\n");
    for blocked in ["packet", "ground", "search", "context"] {
        assert!(
            !joined.contains(&format!("codestory-cli {blocked} ")),
            "{context} should stop before `{blocked}` proof/navigation commands: {joined}"
        );
    }
}

fn run_stdio_request(workspace: &Path, cache_dir: &Path, request: &str) -> Value {
    let mut child = test_support::cli_command()
        .arg("serve")
        .arg("--stdio")
        .arg("--refresh")
        .arg("none")
        .arg("--project")
        .arg(workspace)
        .arg("--cache-dir")
        .arg(cache_dir)
        .env("CODESTORY_TEST_EMBED_ALLOW_CPU", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stdio server");

    {
        let stdin = child.stdin.as_mut().expect("stdio stdin");
        stdin
            .write_all(request.as_bytes())
            .expect("write stdio request");
        stdin.write_all(b"\n").expect("finish stdio request");
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait for stdio server");
    assert!(
        output.status.success(),
        "stdio server failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_else(|| panic!("stdio server should write one response, got {stdout:?}"));
    serde_json::from_str(line).expect("parse stdio response")
}

#[derive(Debug)]
enum StdioToolPayload {
    SchemaV3Structured(Value),
    SchemaV3Text(Value),
    FrozenV2Preparing(Value),
    FrozenV2Error(Value),
}

impl StdioToolPayload {
    fn value(&self) -> &Value {
        match self {
            Self::SchemaV3Structured(value)
            | Self::SchemaV3Text(value)
            | Self::FrozenV2Preparing(value)
            | Self::FrozenV2Error(value) => value,
        }
    }

    fn is_preparing(&self) -> bool {
        matches!(self, Self::SchemaV3Structured(value) | Self::SchemaV3Text(value) if value["kind"] == "preparing")
            || matches!(self, Self::FrozenV2Preparing(_))
    }
}

fn stdio_tool_text(result: &serde_json::Map<String, Value>) -> Result<&str, String> {
    result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|content| content.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| "stdio tool result must include string TextContent".to_string())
}

fn validate_schema_v3_preparing(
    result: &serde_json::Map<String, Value>,
    payload: &Value,
) -> Result<(), String> {
    if result.get("isError") != Some(&Value::Bool(false)) {
        return Err("schema-v3 preparing result must set isError=false".to_string());
    }
    if payload.get("state").and_then(Value::as_str) != Some("preparing") {
        return Err("schema-v3 preparing result must set state=preparing".to_string());
    }
    if payload
        .get("retry_after_ms")
        .and_then(Value::as_u64)
        .is_none_or(|retry_after_ms| retry_after_ms == 0)
    {
        return Err("schema-v3 preparing result must include a positive retry delay".to_string());
    }
    if !payload.get("operation").is_some_and(Value::is_object) {
        return Err("schema-v3 preparing result must include an operation object".to_string());
    }
    Ok(())
}

fn decode_stdio_tool_payload(response: &Value) -> Result<StdioToolPayload, String> {
    let result = response
        .get("result")
        .and_then(Value::as_object)
        .ok_or_else(|| "stdio response must include an object tool result".to_string())?;

    match result.get("structuredContent") {
        Some(structured) if structured.get("kind").is_some() => {
            let kind = structured
                .get("kind")
                .and_then(Value::as_str)
                .ok_or_else(|| "modern stdio tool kind must be a string".to_string())?;
            let text = stdio_tool_text(result)?;
            let text_payload = serde_json::from_str::<Value>(text)
                .map_err(|error| format!("modern stdio tool text mirror must be JSON: {error}"))?;
            if structured != &text_payload {
                return Err("modern structured and text payload mirrors must agree".to_string());
            }
            if kind == "preparing" {
                validate_schema_v3_preparing(result, structured)?;
            }
            Ok(StdioToolPayload::SchemaV3Structured(structured.clone()))
        }
        Some(structured)
            if structured.get("code").and_then(Value::as_str) == Some("codestory_preparing") =>
        {
            stdio_tool_text(result)?;
            if !matches!(result.get("isError"), None | Some(Value::Bool(false))) {
                return Err("legacy preparing result must not set isError=true".to_string());
            }
            Ok(StdioToolPayload::FrozenV2Preparing(structured.clone()))
        }
        Some(structured) if structured.get("code").and_then(Value::as_str).is_some() => {
            stdio_tool_text(result)?;
            if result.get("isError") != Some(&Value::Bool(true)) {
                return Err("legacy structured semantic error must set isError=true".to_string());
            }
            Ok(StdioToolPayload::FrozenV2Error(structured.clone()))
        }
        Some(_) => Err("unrecognized structured stdio tool result shape".to_string()),
        None => {
            let text = stdio_tool_text(result)?;
            let payload = serde_json::from_str::<Value>(text)
                .map_err(|error| format!("stdio semantic error text must be JSON: {error}"))?;
            if !payload.is_object() {
                return Err("stdio text-only payload must be a JSON object".to_string());
            }
            if payload.get("kind").and_then(Value::as_str) == Some("preparing") {
                validate_schema_v3_preparing(result, &payload)?;
            } else if result.get("isError") != Some(&Value::Bool(true)) {
                return Err("stdio text-only semantic error must set isError=true".to_string());
            }
            Ok(StdioToolPayload::SchemaV3Text(payload))
        }
    }
}

fn stdio_tool_payload(response: &Value) -> StdioToolPayload {
    decode_stdio_tool_payload(response).unwrap_or_else(|error| panic!("{error}: {response:#}"))
}

#[test]
fn stdio_tool_payload_rejects_invalid_modern_result_boundaries() {
    let accepted_hostile_cases = [
        (
            "mismatched mirror",
            serde_json::json!({
                "result": {
                    "isError": false,
                    "structuredContent": {
                        "kind": "preparing",
                        "state": "preparing",
                        "retry_after_ms": 250,
                        "operation": {}
                    },
                    "content": [{
                        "type": "text",
                        "text": r#"{"kind":"preparing","state":"preparing","retry_after_ms":251,"operation":{}}"#
                    }]
                }
            }),
        ),
        (
            "malformed mirror",
            serde_json::json!({
                "result": {
                    "isError": false,
                    "structuredContent": {
                        "kind": "preparing",
                        "state": "preparing",
                        "retry_after_ms": 250,
                        "operation": {}
                    },
                    "content": [{"type": "text", "text": "{"}]
                }
            }),
        ),
        (
            "modern preparing missing text mirror",
            serde_json::json!({
                "result": {
                    "isError": false,
                    "structuredContent": {
                        "kind": "preparing",
                        "state": "preparing",
                        "retry_after_ms": 250,
                        "operation": {}
                    }
                }
            }),
        ),
        (
            "modern preparing missing isError",
            serde_json::json!({
                "result": {
                    "structuredContent": {
                        "kind": "preparing",
                        "state": "preparing",
                        "retry_after_ms": 250,
                        "operation": {}
                    },
                    "content": [{
                        "type": "text",
                        "text": r#"{"kind":"preparing","state":"preparing","retry_after_ms":250,"operation":{}}"#
                    }]
                }
            }),
        ),
        (
            "modern preparing missing operation",
            serde_json::json!({
                "result": {
                    "isError": false,
                    "structuredContent": {
                        "kind": "preparing",
                        "state": "preparing",
                        "retry_after_ms": 250
                    },
                    "content": [{
                        "type": "text",
                        "text": r#"{"kind":"preparing","state":"preparing","retry_after_ms":250}"#
                    }]
                }
            }),
        ),
    ]
    .into_iter()
    .filter_map(|(name, response)| {
        decode_stdio_tool_payload(&response).is_ok().then_some(name)
    })
    .collect::<Vec<_>>();

    assert!(
        accepted_hostile_cases.is_empty(),
        "decoder accepted hostile dual-mirror cases: {accepted_hostile_cases:?}"
    );
}

#[test]
fn stdio_tool_payload_accepts_revision_native_preparing_and_text_only_errors() {
    let frozen_transcripts: Value = serde_json::from_str(include_str!(
        "../../../scripts/tests/fixtures/codestory-v2-transcripts.json"
    ))
    .expect("frozen v2 transcript fixture");
    let frozen_v2 = serde_json::json!({
        "result": frozen_transcripts["native_v2"]["preparing"].clone()
    });
    assert_eq!(
        frozen_v2.pointer("/result/content/0/text"),
        Some(&serde_json::json!("CodeStory is preparing managed search.")),
        "fixture must retain the frozen human TextContent"
    );
    let frozen_v2 = stdio_tool_payload(&frozen_v2);
    assert!(matches!(frozen_v2, StdioToolPayload::FrozenV2Preparing(_)));
    assert_eq!(frozen_v2.value()["code"], "codestory_preparing");

    let modern = serde_json::json!({
        "result": {
            "isError": false,
            "structuredContent": {
                "kind": "preparing",
                "state": "preparing",
                "retry_after_ms": 250,
                "operation": {}
            },
            "content": [{
                "type": "text",
                "text": r#"{"kind":"preparing","state":"preparing","retry_after_ms":250,"operation":{}}"#
            }]
        }
    });
    let modern = stdio_tool_payload(&modern);
    assert!(matches!(modern, StdioToolPayload::SchemaV3Structured(_)));
    assert_eq!(modern.value()["kind"], "preparing");

    let modern_text_only = serde_json::json!({
        "result": {
            "isError": false,
            "content": [{
                "type": "text",
                "text": r#"{"kind":"preparing","state":"preparing","retry_after_ms":250,"operation":{}}"#
            }]
        }
    });
    let modern_text_only = stdio_tool_payload(&modern_text_only);
    assert!(matches!(
        modern_text_only,
        StdioToolPayload::SchemaV3Text(_)
    ));
    assert_eq!(modern_text_only.value()["kind"], "preparing");

    for fixture_name in ["unavailable", "tool_error"] {
        let legacy_error = serde_json::json!({
            "result": frozen_transcripts["native_v2"][fixture_name].clone()
        });
        let legacy_error = stdio_tool_payload(&legacy_error);
        assert!(matches!(legacy_error, StdioToolPayload::FrozenV2Error(_)));
        assert!(legacy_error.value()["code"].as_str().is_some());
    }

    let unavailable = serde_json::json!({
        "result": {
            "isError": true,
            "content": [{
                "type": "text",
                "text": r#"{"code":"codestory_unavailable","state":"unavailable"}"#
            }]
        }
    });
    let unavailable = stdio_tool_payload(&unavailable);
    assert!(matches!(unavailable, StdioToolPayload::SchemaV3Text(_)));
    assert_eq!(unavailable.value()["code"], "codestory_unavailable");
}

#[test]
fn stdio_tool_payload_rejects_invalid_result_state_and_text_error_boundaries() {
    let accepted_hostile_cases = [
        (
            "modern preparing marked error",
            serde_json::json!({
                "result": {
                    "isError": true,
                    "structuredContent": {
                        "kind": "preparing",
                        "state": "preparing",
                        "retry_after_ms": 250,
                        "operation": {}
                    },
                    "content": [{
                        "type": "text",
                        "text": r#"{"kind":"preparing","state":"preparing","retry_after_ms":250,"operation":{}}"#
                    }]
                }
            }),
        ),
        (
            "text-only error marked successful",
            serde_json::json!({
                "result": {
                    "isError": false,
                    "content": [{
                        "type": "text",
                        "text": r#"{"code":"codestory_unavailable","state":"unavailable"}"#
                    }]
                }
            }),
        ),
        (
            "malformed text-only error",
            serde_json::json!({
                "result": {
                    "isError": true,
                    "content": [{"type": "text", "text": "{"}]
                }
            }),
        ),
        (
            "text-only preparing marked error",
            serde_json::json!({
                "result": {
                    "isError": true,
                    "content": [{
                        "type": "text",
                        "text": r#"{"kind":"preparing","state":"preparing","retry_after_ms":250,"operation":{}}"#
                    }]
                }
            }),
        ),
        (
            "text-only semantic error is not an object",
            serde_json::json!({
                "result": {
                    "isError": true,
                    "content": [{"type": "text", "text": "[]"}]
                }
            }),
        ),
        (
            "legacy preparing marked error",
            serde_json::json!({
                "result": {
                    "isError": true,
                    "structuredContent": {
                        "code": "codestory_preparing",
                        "message": "CodeStory is preparing managed search.",
                        "state": "preparing",
                        "retry_after_ms": 250
                    },
                    "content": [{
                        "type": "text",
                        "text": "CodeStory is preparing managed search."
                    }]
                }
            }),
        ),
    ]
    .into_iter()
    .filter_map(|(name, response)| {
        decode_stdio_tool_payload(&response).is_ok().then_some(name)
    })
    .collect::<Vec<_>>();

    assert!(
        accepted_hostile_cases.is_empty(),
        "decoder accepted hostile result-state cases: {accepted_hostile_cases:?}"
    );
}

fn string_field<'a>(value: &'a Value, path: &[&str]) -> &'a str {
    value_at_path(value, path)
        .as_str()
        .unwrap_or_else(|| panic!("expected string at path {path:?}"))
}

fn array_is_non_empty(value: &Value, path: &[&str]) -> bool {
    value_at_path(value, path)
        .as_array()
        .is_some_and(|items| !items.is_empty())
}

fn value_at_path<'a>(value: &'a Value, path: &[&str]) -> &'a Value {
    let mut current = value;
    for key in path {
        current = value_at_path_key(current, key, path);
    }
    current
}

fn value_at_path_key<'a>(value: &'a Value, key: &str, path: &[&str]) -> &'a Value {
    match value {
        Value::Object(fields) => fields
            .get(key)
            .unwrap_or_else(|| panic!("missing key {key:?} at path {path:?}")),
        Value::Array(items) => {
            let index = key
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("expected array index at path {path:?}"));
            items
                .get(index)
                .unwrap_or_else(|| panic!("missing index {index} at path {path:?}"))
        }
        _ => panic!("cannot descend into non-container at path {path:?}, key {key:?}"),
    }
}

fn search_dir_for_storage(storage_path: &Path) -> PathBuf {
    let parent = storage_path.parent().expect("storage parent");
    let stem = storage_path
        .file_stem()
        .and_then(|value| value.to_str())
        .expect("storage file stem");
    let generation_root = parent.join(format!("{stem}.search-generations"));
    if generation_root.is_dir() {
        let mut generations = fs::read_dir(&generation_root)
            .expect("list persisted search generations")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        assert_eq!(
            generations.len(),
            1,
            "fresh CLI fixture should publish exactly one search generation"
        );
        return generations.pop().expect("published search generation");
    }
    parent.join(format!("{stem}.search"))
}

fn search_catalog_lock_for_storage(storage_path: &Path) -> PathBuf {
    let parent = storage_path.parent().expect("storage parent");
    let stem = storage_path
        .file_stem()
        .and_then(|value| value.to_str())
        .expect("storage file stem");
    parent.join(format!("{stem}.search-generations.lock"))
}

#[derive(Debug, PartialEq, Eq)]
struct CacheFileSnapshot {
    relative_path: PathBuf,
    byte_len: u64,
    sha256: String,
    modified: SystemTime,
}

fn cache_file_snapshots(root: &Path) -> Vec<CacheFileSnapshot> {
    fn visit(root: &Path, current: &Path, snapshots: &mut Vec<CacheFileSnapshot>) {
        let mut entries = fs::read_dir(current)
            .expect("read cache directory")
            .map(|entry| entry.expect("read cache entry"))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = entry.metadata().expect("read cache entry metadata");
            if metadata.is_dir() {
                visit(root, &path, snapshots);
            } else if metadata.is_file() {
                snapshots.push(CacheFileSnapshot {
                    relative_path: path
                        .strip_prefix(root)
                        .expect("cache entry below root")
                        .to_path_buf(),
                    byte_len: metadata.len(),
                    sha256: format!(
                        "{:x}",
                        Sha256::digest(fs::read(&path).expect("read cache file"))
                    ),
                    modified: metadata.modified().expect("read cache file mtime"),
                });
            }
        }
    }

    let mut snapshots = Vec::new();
    if root.is_dir() {
        visit(root, root, &mut snapshots);
    }
    snapshots
}

fn find_index_freshness(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(map) => {
            for key in ["index_freshness", "freshness"] {
                if let Some(candidate) = map.get(key)
                    && (freshness_count(
                        candidate,
                        &["changed_file_count", "changed_count", "changed"],
                    )
                    .is_some()
                        || candidate.get("not_checked_reason").is_some()
                        || candidate.get("not_checked").is_some())
                {
                    return Some(candidate);
                }
            }
            map.values().find_map(find_index_freshness)
        }
        Value::Array(items) => items.iter().find_map(find_index_freshness),
        _ => None,
    }
}

fn freshness_count(value: &Value, aliases: &[&str]) -> Option<u64> {
    aliases
        .iter()
        .find_map(|alias| value.get(*alias).and_then(Value::as_u64))
}

fn assert_stale_freshness_counts(value: &Value, context: &str) {
    let freshness = find_index_freshness(value)
        .unwrap_or_else(|| panic!("{context} should include an index freshness signal: {value:#}"));
    assert_eq!(
        freshness_count(
            freshness,
            &["changed_file_count", "changed_count", "changed"]
        ),
        Some(1),
        "{context} freshness should report one changed file: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &["new_file_count", "new_count", "new", "added_count", "added"]
        ),
        Some(1),
        "{context} freshness should report one new file: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &[
                "removed_file_count",
                "removed_count",
                "removed",
                "deleted_count",
                "deleted"
            ]
        ),
        Some(1),
        "{context} freshness should report one removed file: {freshness:#}"
    );
}

fn write_openapi_workspace(root: &Path) {
    fs::write(
        root.join("openapi.json"),
        r#"{
  "openapi": "3.1.0",
  "paths": {
    "/api/users": {
      "get": {
        "operationId": "listUsers"
      }
    }
  }
}"#,
    )
    .expect("write openapi fixture");
}

fn write_investigation_workspace(root: &Path) {
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "investigation-browser-fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("write Cargo.toml");

    let src = root.join("src");
    fs::create_dir_all(&src).expect("create src dir");
    fs::write(
        src.join("lib.rs"),
        r#"pub mod evidence;
pub mod router;

pub fn run_investigation(payload: &str) -> &'static str {
    let event = evidence::parse_investigation_event(payload);
    router::route_investigation_event(event)
}
"#,
    )
    .expect("write lib.rs");
    fs::write(
        src.join("evidence.rs"),
        r#"pub const INVESTIGATION_LITERAL: &str = "CODESTORY_INVESTIGATION_LITERAL";

pub fn parse_investigation_event(payload: &str) -> &'static str {
    let _marker = INVESTIGATION_LITERAL;
    if payload.is_empty() {
        "empty"
    } else {
        "routed"
    }
}
"#,
    )
    .expect("write evidence.rs");
    fs::write(
        src.join("router.rs"),
        r#"pub fn route_investigation_event(event: &str) -> &'static str {
    match event {
        "routed" => "source-read-with-citation",
        _ => "gap-recorded",
    }
}
"#,
    )
    .expect("write router.rs");
}

#[test]
fn read_commands_report_changed_openapi_index_freshness() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_openapi_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    thread::sleep(Duration::from_millis(25));
    fs::write(
        workspace.path().join("openapi.json"),
        r#"{
  "openapi": "3.1.0",
  "paths": {
    "/api/users": {
      "post": {
        "operationId": "createUser"
      }
    }
  }
}"#,
    )
    .expect("modify indexed openapi fixture");

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    let freshness = find_index_freshness(&doctor)
        .unwrap_or_else(|| panic!("doctor should include index freshness: {doctor:#}"));
    assert_eq!(
        freshness["status"], "stale",
        "changed indexed OpenAPI schema should make freshness stale: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &["changed_file_count", "changed_count", "changed"]
        ),
        Some(1),
        "changed indexed OpenAPI schema should be counted as changed: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &["new_file_count", "new_count", "new", "added_count", "added"]
        ),
        Some(0),
        "modified OpenAPI schema should not be counted as new: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &[
                "removed_file_count",
                "removed_count",
                "removed",
                "deleted_count",
                "deleted"
            ]
        ),
        Some(0),
        "modified OpenAPI schema should not be counted as removed: {freshness:#}"
    );
}

#[test]
fn non_openapi_json_does_not_keep_freshness_stale() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    fs::write(
        workspace.path().join("skills-lock.json"),
        r#"{"skills":[]}"#,
    )
    .expect("write non-openapi json");

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    let freshness = find_index_freshness(&doctor)
        .unwrap_or_else(|| panic!("doctor should include index freshness: {doctor:#}"));
    assert_eq!(
        freshness["status"], "fresh",
        "non-OpenAPI JSON that indexes to no symbols should not keep freshness stale: {freshness:#}"
    );
}

#[test]
fn new_openapi_with_late_paths_marks_freshness_stale() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    let filler = "x".repeat(70 * 1024);
    fs::write(
        workspace.path().join("late-openapi.yaml"),
        format!(
            "openapi: 3.1.0\ninfo:\n  title: Late Paths\n  version: 1.0.0\ncomponents:\n  schemas:\n    Filler:\n      description: \"{filler}\"\npaths:\n  /api/late:\n    get:\n      operationId: getLate\n"
        ),
    )
    .expect("write late OpenAPI fixture");

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    let freshness = find_index_freshness(&doctor)
        .unwrap_or_else(|| panic!("doctor should include index freshness: {doctor:#}"));
    assert_eq!(
        freshness["status"], "stale",
        "new OpenAPI schema with late paths should make freshness stale: {freshness:#}"
    );
    assert_eq!(
        freshness_count(
            freshness,
            &["new_file_count", "new_count", "new", "added_count", "added"]
        ),
        Some(1),
        "new OpenAPI schema with late paths should be counted as new: {freshness:#}"
    );
}

#[test]
fn doctor_next_commands_stop_at_index_repair_when_inventory_is_stale() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    fs::write(
        workspace.path().join("late-openapi.yaml"),
        "openapi: 3.1.0\ninfo:\n  title: Late Paths\n  version: 1.0.0\npaths:\n  /api/late:\n    get:\n      operationId: getLate\n",
    )
    .expect("write late OpenAPI fixture");

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    let freshness = find_index_freshness(&doctor)
        .unwrap_or_else(|| panic!("doctor should include index freshness: {doctor:#}"));
    assert_eq!(
        freshness["status"], "stale",
        "doctor should report stale freshness before recommending repair: {doctor:#}"
    );

    let next_commands = doctor_next_commands(&doctor);
    let joined = next_commands.join("\n");
    assert!(
        joined.contains("codestory-cli index --project") && joined.contains("codestory-cli doctor"),
        "stale doctor should recommend local graph repair then doctor recheck: {doctor:#}"
    );
    assert!(
        !joined.contains("retrieval index --project")
            && !joined.contains("retrieval status")
            && !joined.contains("retrieval index"),
        "stale doctor next_commands should stop before agent retrieval repair commands: {doctor:#}"
    );
    assert_no_agent_proof_commands(&next_commands, "stale doctor");

    let markdown = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "markdown"],
    );
    assert!(
        markdown.status.success(),
        "doctor markdown failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&markdown.stdout),
        String::from_utf8_lossy(&markdown.stderr)
    );
    let markdown = String::from_utf8_lossy(&markdown.stdout);
    assert!(
        markdown.contains("readiness: local_navigation=repair_index agent_packet_search=blocked"),
        "doctor markdown should show local index repair without collapsing agent retrieval readiness:\n{markdown}"
    );
}

#[test]
fn doctor_next_commands_stop_at_retrieval_repair_when_sidecar_is_not_full() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    let index = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    let index_next_commands = doctor_next_commands(&index);
    let index_next_commands_joined = index_next_commands.join("\n");
    assert!(
        index_next_commands_joined.contains("codestory-cli retrieval status")
            && index_next_commands_joined.contains("codestory-cli retrieval index"),
        "index output should recommend retrieval publication when retrieval is not full: {index:#}"
    );
    assert_no_agent_proof_commands(&index_next_commands, "index output");

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    let freshness = find_index_freshness(&doctor)
        .unwrap_or_else(|| panic!("doctor should include index freshness: {doctor:#}"));
    assert_ne!(
        freshness["status"], "stale",
        "sidecar repair test needs fresh enough inventory: {doctor:#}"
    );
    assert_ne!(
        doctor["retrieval_mode"], "full",
        "sidecar repair test needs mandatory sidecar retrieval to be not full: {doctor:#}"
    );
    assert_eq!(
        doctor["readiness_lanes"]["local_default"]["profile"], "local",
        "doctor should expose local/default retrieval as its own lane: {doctor:#}"
    );
    assert!(
        doctor["readiness_lanes"]["local_default"]["retrieval_mode"].is_string(),
        "local/default lane should expose sidecar mode: {doctor:#}"
    );
    assert!(
        doctor["readiness_lanes"]["local_default"]["next_command"]
            .as_str()
            .is_some_and(|command| command.contains("retrieval index")
                && command.contains("--profile local")),
        "local/default lane should expose a local-scoped next command: {doctor:#}"
    );
    assert_eq!(
        doctor["readiness_lanes"]["agent_packet_search"]["status"], "blocked",
        "doctor should keep agent packet/search readiness separate: {doctor:#}"
    );
    assert_eq!(
        doctor["readiness_lanes"]["agent_packet_search"]["profile"], "agent",
        "agent lane must not collapse to local when no agent run exists: {doctor:#}"
    );
    assert_eq!(
        doctor["readiness_lanes"]["agent_packet_search"]["run_id"], "shared-agent",
        "agent lane should make the missing-run repair state explicit: {doctor:#}"
    );
    assert!(
        doctor["readiness_lanes"]["agent_packet_search"]["next_command"]
            .as_str()
            .is_some_and(|command| command.contains("retrieval index")
                && command.contains("--run-id")
                && command.contains("shared-agent")),
        "agent lane should expose the agent-scoped activation command: {doctor:#}"
    );

    let next_commands = doctor_next_commands(&doctor);
    let joined = next_commands.join("\n");
    assert!(
        joined.contains("codestory-cli retrieval status")
            && joined.contains("codestory-cli retrieval index")
            && joined.contains("codestory-cli doctor"),
        "doctor should recommend retrieval publication before packet/search: {doctor:#}"
    );
    assert!(
        !joined.contains("codestory-cli index "),
        "fresh doctor should not send users back to index repair: {doctor:#}"
    );
    assert_no_agent_proof_commands(&next_commands, "sidecar doctor");

    let markdown = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "markdown"],
    );
    assert!(
        markdown.status.success(),
        "doctor markdown failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&markdown.stdout),
        String::from_utf8_lossy(&markdown.stderr)
    );
    let markdown = String::from_utf8_lossy(&markdown.stdout);
    assert!(
        markdown.contains("readiness: local_navigation=ready agent_packet_search=blocked"),
        "doctor markdown should show split readiness with blocked agent packet/search:\n{markdown}"
    );
}

#[test]
fn agent_preflight_reports_local_graph_when_retrieval_is_degraded() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    let preflight = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["agent", "preflight", "--format", "json"],
    );

    assert_eq!(preflight["usable"], true, "{preflight:#}");
    assert_eq!(preflight["mode"], "local_graph", "{preflight:#}");
    assert_eq!(preflight["local_graph"]["ready"], true, "{preflight:#}");
    assert_eq!(
        preflight["local_refresh"]["state"], "refreshed",
        "{preflight:#}"
    );
    assert_eq!(
        preflight["local_refresh"]["blocks_local_surfaces"], false,
        "{preflight:#}"
    );
    assert_eq!(
        preflight["full_retrieval"]["status"], "blocked",
        "{preflight:#}"
    );
    assert_eq!(
        preflight["local_default"]["profile"], "local",
        "preflight should expose local/default retrieval lane: {preflight:#}"
    );
    assert!(
        preflight["local_default"]["retrieval_mode"].is_string(),
        "local/default lane should expose sidecar mode: {preflight:#}"
    );
    assert!(
        preflight["local_default"]["next_command"]
            .as_str()
            .is_some_and(|command| command.contains("--profile local")),
        "local/default lane should expose a local-scoped next command: {preflight:#}"
    );
    assert_eq!(
        preflight["agent_packet_search"]["status"], "blocked",
        "preflight should expose agent packet/search lane: {preflight:#}"
    );
    assert_eq!(
        preflight["agent_packet_search"]["profile"], "agent",
        "agent preflight lane must not collapse to local when no agent run exists: {preflight:#}"
    );
    assert_eq!(
        preflight["agent_packet_search"]["run_id"], "shared-agent",
        "agent preflight lane should make the missing-run repair state explicit: {preflight:#}"
    );
    assert!(
        preflight["readiness_lanes"]["agent_packet_search"]["next_command"]
            .as_str()
            .is_some_and(|command| command.contains("retrieval index")
                && command.contains("--run-id")
                && command.contains("shared-agent")),
        "agent lane should expose the agent-scoped activation command: {preflight:#}"
    );
    let safe_surfaces = preflight["safe_surfaces"]
        .as_array()
        .expect("safe surfaces");
    for surface in ["ground", "callers", "callees", "trace"] {
        assert!(
            safe_surfaces.iter().any(|candidate| candidate == surface),
            "local graph surface {surface} should be safe: {preflight:#}"
        );
    }
    assert!(
        preflight["blocked_surfaces"]
            .as_array()
            .expect("blocked surfaces")
            .iter()
            .any(|surface| surface == "packet_full"),
        "full retrieval surfaces should be blocked: {preflight:#}"
    );
    assert!(
        preflight["next_command"]
            .as_str()
            .is_some_and(|command| command.contains("retrieval index")),
        "preflight should point at the retrieval activation path: {preflight:#}"
    );
    assert!(
        preflight["human_summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("Local graph is ready")),
        "preflight should include a human summary: {preflight:#}"
    );
}

#[test]
fn agent_preflight_refreshes_stale_local_graph_before_reporting() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    thread::sleep(Duration::from_millis(25));
    fs::write(
        workspace.path().join("src").join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("preflight-refreshed:{project_name}")
}

pub fn schedule_index(project_path: &str) -> usize {
    super::open_project(project_path).len() + 1
}
"#,
    )
    .expect("modify indexed file after indexing");

    let preflight = run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["agent", "preflight", "--format", "json"],
        &[(
            "CODESTORY_AGENT_PREFLIGHT_LOCAL_REFRESH_TIMEOUT_MS",
            "30000",
        )],
    );

    assert_eq!(preflight["usable"], true, "{preflight:#}");
    assert_eq!(preflight["mode"], "local_graph", "{preflight:#}");
    assert_eq!(preflight["local_graph"]["ready"], true, "{preflight:#}");
    assert_eq!(
        preflight["local_refresh"]["state"], "refreshed",
        "agent preflight should quietly refresh stale local freshness: {preflight:#}"
    );
    assert_eq!(
        preflight["local_refresh"]["reason"], "refreshed",
        "agent preflight should report the bounded refresh result: {preflight:#}"
    );
    assert_eq!(
        preflight["local_refresh"]["blocks_local_surfaces"], false,
        "{preflight:#}"
    );
    assert_eq!(
        preflight["full_retrieval"]["status"], "blocked",
        "local refresh must not claim full retrieval readiness: {preflight:#}"
    );
    assert_eq!(
        preflight["agent_packet_search"]["status"], "blocked",
        "agent packet/search should remain fail-closed without full sidecars: {preflight:#}"
    );
    assert!(
        preflight["blocked_surfaces"]
            .as_array()
            .expect("blocked surfaces")
            .iter()
            .any(|surface| surface == "packet_full"),
        "full retrieval surfaces should stay blocked after local refresh: {preflight:#}"
    );
}

#[test]
fn agent_preflight_reports_compact_reason_when_local_refresh_budget_expires() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    thread::sleep(Duration::from_millis(25));
    fs::write(
        workspace.path().join("src").join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("preflight-timeout:{project_name}")
}

pub fn schedule_index(project_path: &str) -> usize {
    super::open_project(project_path).len() + 2
}
"#,
    )
    .expect("modify indexed file after indexing");

    let preflight = run_cli_json_with_embedding_env(
        workspace.path(),
        cache_dir.path(),
        &["agent", "preflight", "--format", "json"],
        &[
            ("CODESTORY_TEST_EMBED_ALLOW_CPU", "1"),
            ("CODESTORY_AGENT_PREFLIGHT_LOCAL_REFRESH_TIMEOUT_MS", "0"),
        ],
    );

    assert_eq!(preflight["usable"], false, "{preflight:#}");
    assert_eq!(preflight["mode"], "blocked", "{preflight:#}");
    assert_eq!(preflight["local_graph"]["ready"], false, "{preflight:#}");
    assert_eq!(
        preflight["local_refresh"]["state"], "refreshing",
        "{preflight:#}"
    );
    assert_eq!(
        preflight["local_refresh"]["reason"], "refresh_timeout",
        "agent preflight should report one compact local refresh reason: {preflight:#}"
    );
    assert_eq!(
        preflight["local_refresh"]["blocks_local_surfaces"], true,
        "{preflight:#}"
    );
    assert!(
        preflight["blocked_surfaces"]
            .as_array()
            .expect("blocked surfaces")
            .iter()
            .any(|surface| surface == "ground"),
        "local graph surfaces should stay blocked when refresh does not finish: {preflight:#}"
    );
    assert_eq!(
        preflight["agent_packet_search"]["status"], "blocked",
        "timeout must not escalate into agent sidecar repair or readiness: {preflight:#}"
    );
}

#[test]
fn tiny_workspace_browser_loop_works_from_existing_cache() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    let tests = workspace.path().join("tests");
    fs::create_dir_all(&tests).expect("create tests dir");
    fs::write(
        tests.join("app_controller_test.rs"),
        r#"use tiny_browser_fixture::AppController;

#[test]
fn app_controller_opens_project() {
    let controller = AppController::new("demo");
    assert!(controller.open_project().contains("demo"));
}
"#,
    )
    .expect("write integration test");

    let search_dir_snapshot =
        index_tiny_workspace_for_browser_loop(workspace.path(), cache_dir.path());

    assert_doctor_reports_existing_cache_health(workspace.path(), cache_dir.path());
    search_dir_snapshot.assert_unchanged("doctor");

    assert_ground_reads_existing_cache(workspace.path(), cache_dir.path());
    search_dir_snapshot.assert_unchanged("ground");

    let node_id = ground_symbol_node_id_from_existing_cache(
        workspace.path(),
        cache_dir.path(),
        "AppController",
        None,
    );
    search_dir_snapshot.assert_unchanged("ground symbol");
    assert_exact_search_uses_core_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        "AppController",
    );
    search_dir_snapshot.assert_unchanged("product search");

    assert_symbol_reads_focus_node(workspace.path(), cache_dir.path(), &node_id);
    search_dir_snapshot.assert_unchanged("symbol");

    assert_trail_reads_focus_node(workspace.path(), cache_dir.path(), &node_id);
    search_dir_snapshot.assert_unchanged("trail");

    assert_snippet_reads_focus_node(workspace.path(), cache_dir.path(), &node_id);
    search_dir_snapshot.assert_unchanged("snippet");

    let bookmark_id = add_and_assert_bookmark_focus(workspace.path(), cache_dir.path(), &node_id);
    search_dir_snapshot.assert_unchanged("bookmark add");

    assert_context_bookmark_fails_closed_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        &bookmark_id,
    );
    search_dir_snapshot.assert_unchanged("context bookmark");

    assert_explore_outputs_focus_context(workspace.path(), cache_dir.path(), &node_id);
    search_dir_snapshot.assert_unchanged("explore");

    assert_files_and_affected_read_existing_cache(workspace.path(), cache_dir.path());
    search_dir_snapshot.assert_unchanged("files and affected");

    assert_query_search_fails_closed_without_full_sidecars(workspace.path(), cache_dir.path());
    search_dir_snapshot.assert_unchanged("query search");
    assert_packet_builds_broad_task_contract(workspace.path(), cache_dir.path());
    search_dir_snapshot.assert_unchanged("packet");
    assert_context_id_fails_closed_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        &node_id,
    );
    search_dir_snapshot.assert_unchanged("context id");
    remove_and_assert_bookmark_gone(workspace.path(), cache_dir.path(), &bookmark_id);
    search_dir_snapshot.assert_unchanged("bookmark remove");
    search_dir_snapshot.assert_unchanged("read loop");

    assert_stdio_context_id_fails_closed_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        &node_id,
    );
}

#[test]
fn python_function_body_id_snippet_retains_attached_test_cases() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    fs::write(
        workspace.path().join("test_cases.py"),
        r#"import pytest

def unrelated_before():
    return "UNRELATED_BEFORE"

# UNRELATED_COMMENT_BEFORE
@pytest.mark.parametrize(
    "mode, expected",
    [
        ("first", "FIRST"),
        ("second", "SECOND"),
    ],
)
def test_checked_modes(mode, expected):
    assert mode.upper() == expected

# UNRELATED_COMMENT_AFTER
def unrelated_after():
    return "UNRELATED_AFTER"
"#,
    )
    .expect("write decorated Python test");
    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    let node_id = ground_symbol_node_id_from_existing_cache(
        workspace.path(),
        cache_dir.path(),
        "test_checked_modes",
        Some("test_cases.py"),
    );
    let snippet = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "snippet",
            "--id",
            &node_id,
            "--function-body",
            "--context",
            "0",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_eq!(snippet["resolution"]["resolved"]["node_id"], node_id);
    assert_eq!(snippet["snippet"]["scope"], "function_body");
    assert_eq!(snippet["snippet"]["requested_context"], 0);
    assert_eq!(snippet["snippet"]["snippet_truncated"], false);
    let source = string_field(&snippet, &["snippet", "snippet"]);
    assert!(source.contains("def test_checked_modes(mode, expected):"));
    assert!(source.contains("assert mode.upper() == expected"));
    assert!(
        source.contains("@pytest.mark.parametrize("),
        "function-body source must include its attached test cases: {snippet:#}"
    );
    assert!(source.contains("\"mode, expected\""));
    assert!(source.contains("(\"first\", \"FIRST\")"));
    assert!(source.contains("(\"second\", \"SECOND\")"));
    assert!(!source.contains("unrelated_before"));
    assert!(!source.contains("unrelated_after"));
    assert!(!source.contains("UNRELATED_COMMENT"));
}

#[test]
fn snippet_exact_id_navigates_openapi_diagnostic_evidence_but_query_stays_typed() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_openapi_workspace(workspace.path());
    let _search_dir_snapshot =
        index_tiny_workspace_for_browser_loop(workspace.path(), cache_dir.path());
    let node_id = ground_symbol_node_id_from_existing_cache(
        workspace.path(),
        cache_dir.path(),
        "openapi::GET /api/users",
        Some("openapi.json"),
    );

    let snippet = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "snippet",
            &format!("--id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_eq!(
        snippet["resolution"]["resolved"]["evidence_tier"],
        "exact_source"
    );
    assert_eq!(
        snippet["resolution"]["resolved"]["evidence_producer"],
        "openapi_endpoint_schema"
    );
    assert_eq!(
        snippet["resolution"]["resolved"]["resolution_status"],
        "source_range_only"
    );
    assert!(
        snippet["resolution"]["resolved"]["eligible_for_sufficiency"].is_null(),
        "source navigation must not expose answer-sufficiency authority: {snippet:#}"
    );
    assert!(
        snippet["snippet"]["snippet"]
            .as_str()
            .is_some_and(|source| source.contains("/api/users")),
        "snippet should navigate the exact OpenAPI source range: {snippet:#}"
    );

    let query = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "snippet",
            "--query=openapi::GET /api/users",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !query.status.success(),
        "query snippets must not select source-range-only diagnostic evidence"
    );
    let failure: Value = serde_json::from_slice(&query.stdout)
        .unwrap_or_else(|error| panic!("parse query snippet failure: {error}; output={query:#?}"));
    assert!(
        failure["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("No symbol matched query")),
        "query snippet should retain typed graph filtering: {failure:#}"
    );
}

#[test]
fn files_json_reports_structural_support_tiers_for_cargo_and_compose() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    write_docker_compose_fixture(workspace.path());
    index_tiny_workspace_for_browser_loop(workspace.path(), cache_dir.path());

    let files = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["files", "--refresh", "none", "--format", "json"],
    );

    assert_eq!(
        files["summary"]["framework_route_coverage_included"],
        serde_json::json!(false)
    );
    assert_eq!(
        files["summary"]["framework_route_coverage"],
        serde_json::json!([])
    );
    let full = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "files",
            "--refresh",
            "none",
            "--format",
            "json",
            "--include-framework-coverage",
        ],
    );
    assert_eq!(
        full["summary"]["framework_route_coverage_included"],
        serde_json::json!(true)
    );
    assert!(
        full["summary"]["framework_route_coverage"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
    );
    for field in [
        "files",
        "coverage_gaps",
        "policy_exclusions",
        "project_root",
        "usable",
    ] {
        assert_eq!(files.get(field), full.get(field), "{field}");
    }
    assert_eq!(
        files["summary"]["language_counts"],
        full["summary"]["language_counts"]
    );

    assert!(
        files["summary"]["language_counts"]
            .as_array()
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item["language"] == "cargo_manifest"
                        && item["support_mode"] == "structural_collector"
                        && item["evidence_tier"] == "structural_only"
                        && item["claim_label"] == "structural collector only"
                }) && items.iter().any(|item| {
                    item["language"] == "docker_compose"
                        && item["support_mode"] == "structural_collector"
                        && item["evidence_tier"] == "structural_only"
                        && item["claim_label"] == "structural collector only"
                })
            }),
        "files JSON should expose path-scoped structural support tiers for manifests and Compose: {files:#}"
    );
}

struct SearchDirSnapshot {
    path: PathBuf,
    marker: PathBuf,
}

fn index_tiny_workspace_for_browser_loop(workspace: &Path, cache_dir: &Path) -> SearchDirSnapshot {
    let index = run_cli_json(
        workspace,
        cache_dir,
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index["summary"]["stats"]["node_count"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        "index should discover symbols in the tiny workspace"
    );
    let storage_path = PathBuf::from(string_field(&index, &["storage_path"]));
    SearchDirSnapshot::capture(search_dir_for_storage(&storage_path))
}

fn assert_doctor_reports_existing_cache_health(workspace: &Path, cache_dir: &Path) {
    let doctor = run_cli_json(workspace, cache_dir, &["doctor", "--format", "json"]);
    assert!(
        doctor["checks"]
            .as_array()
            .is_some_and(|checks| !checks.is_empty()),
        "doctor should emit checks"
    );
    assert!(
        doctor["environment"]
            .as_array()
            .expect("doctor environment")
            .iter()
            .any(|row| row["name"] == "CODESTORY_EMBED_ALLOW_CPU"),
        "doctor should expose the explicit CPU policy"
    );
}

fn assert_ground_reads_existing_cache(workspace: &Path, cache_dir: &Path) {
    let ground = run_cli_json(
        workspace,
        cache_dir,
        &["ground", "--refresh", "none", "--format", "json"],
    );
    assert!(
        array_is_non_empty(&ground, &["root_symbols"]) || array_is_non_empty(&ground, &["files"]),
        "ground should return project grounding data"
    );
}

fn ground_symbol_node_id_from_existing_cache(
    workspace: &Path,
    cache_dir: &Path,
    symbol: &str,
    file_path_suffix: Option<&str>,
) -> String {
    let ground = run_cli_json(
        workspace,
        cache_dir,
        &["ground", "--refresh", "none", "--format", "json"],
    );
    let mut symbols = Vec::new();
    if let Some(root_symbols) = ground["root_symbols"].as_array() {
        symbols.extend(root_symbols.iter());
    }
    if let Some(files) = ground["files"].as_array() {
        for file in files {
            if let Some(file_symbols) = file["symbols"].as_array() {
                symbols.extend(file_symbols.iter());
            }
        }
    }
    let label_prefix = format!("{symbol} @ ");
    let hit = symbols
        .into_iter()
        .find(|item| {
            let Some(label) = item["label"].as_str() else {
                return false;
            };
            label.starts_with(&label_prefix)
                && file_path_suffix.is_none_or(|suffix| label.contains(&suffix.replace('\\', "/")))
        })
        .unwrap_or_else(|| panic!("ground should find {symbol} in the existing cache: {ground:#}"));
    string_field(hit, &["id"]).to_string()
}

fn assert_exact_search_uses_core_without_full_sidecars(
    workspace: &Path,
    cache_dir: &Path,
    query: &str,
) {
    let output = run_cli(
        workspace,
        cache_dir,
        &[
            "search",
            "--query",
            query,
            "--repo-text",
            "off",
            "--limit",
            "5",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        output.status.success(),
        "exact search should use the complete core without retrieval sidecars: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let search: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("parse exact-search output: {error}; output={output:#?}"));
    assert_eq!(search["schema_version"], 3, "{search:#}");
    assert_eq!(search["kind"], "complete", "{search:#}");
    assert_eq!(search["status"], "available", "{search:#}");
    assert_eq!(search["retrieval"]["state"], "symbolic", "{search:#}");
    assert!(
        search["retrieval"]["generation_id"].is_null()
            && search["publication"]["retrieval"].is_null(),
        "exact search must not imply a sidecar publication: {search:#}"
    );
    assert!(
        search["gaps"].as_array().is_some_and(Vec::is_empty),
        "an intentionally symbolic search must not report retrieval unavailable: {search:#}"
    );
    assert!(
        search["evidence"].as_array().is_some_and(|rows| {
            rows.iter().any(|row| {
                row["path"]
                    .as_str()
                    .is_some_and(|path| !path.starts_with('/'))
                    && row["excerpt"]
                        .as_str()
                        .is_some_and(|excerpt| excerpt.contains(query))
            })
        }),
        "exact search must return project-relative source evidence for {query}: {search:#}"
    );
}

#[cfg(unix)]
#[test]
fn core_navigation_reads_the_immutable_core_when_the_search_catalog_is_unwritable() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    let index = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    let storage_path = PathBuf::from(string_field(&index, &["storage_path"]));
    let catalog_lock = search_catalog_lock_for_storage(&storage_path);
    assert!(
        catalog_lock.is_file(),
        "index should publish a catalog lock"
    );
    fs::set_permissions(&catalog_lock, fs::Permissions::from_mode(0o400))
        .expect("make search catalog lock read-only");
    let before = cache_file_snapshots(cache_dir.path());

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "search",
            "--query",
            "AppController",
            "--repo-text",
            "off",
            "--limit",
            "5",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );

    let named_outputs = ["symbol", "callers"].map(|command| {
        (
            command,
            run_cli(
                workspace.path(),
                cache_dir.path(),
                &[
                    command,
                    "--query",
                    "AppController",
                    "--refresh",
                    "none",
                    "--format",
                    "json",
                ],
            ),
        )
    });

    fs::set_permissions(&catalog_lock, fs::Permissions::from_mode(0o600))
        .expect("restore search catalog lock permissions");
    assert!(
        output.status.success(),
        "core-only exact search must not open the search-generation catalog: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let search: Value = serde_json::from_slice(&output.stdout).expect("parse exact search JSON");
    assert!(
        search["evidence"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| {
                row["excerpt"]
                    .as_str()
                    .is_some_and(|excerpt| excerpt.contains("AppController"))
            })),
        "core-only exact search should return source evidence: {search:#}"
    );
    for (command, named) in named_outputs {
        assert!(
            named.status.success(),
            "{command} by indexed name must not initialize retrieval or attach search: stdout={} stderr={}",
            String::from_utf8_lossy(&named.stdout),
            String::from_utf8_lossy(&named.stderr)
        );
        let named: Value =
            serde_json::from_slice(&named.stdout).expect("parse name-selected navigation");
        if command == "symbol" {
            assert_eq!(named["symbol"]["node"]["display_name"], "AppController");
        } else {
            assert!(
                named["trail"]["trail"]["nodes"]
                    .as_array()
                    .is_some_and(|nodes| nodes.iter().any(|node| node["label"] == "AppController")),
                "{named:#}"
            );
        }
    }
    assert_eq!(
        cache_file_snapshots(cache_dir.path()),
        before,
        "Core navigation must not mutate cache bytes or mtimes"
    );
}

#[test]
fn cold_exact_search_returns_project_unavailable_without_creating_cache_files() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    let before = cache_file_snapshots(cache_dir.path());

    let output = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "search",
            "--query",
            "AppController",
            "--repo-text",
            "off",
            "--limit",
            "5",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );

    assert!(
        !output.status.success(),
        "cold exact search must fail closed"
    );
    let failure: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "parse cold exact-search failure: {error}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(
        failure["error"]["code"], "project_unavailable",
        "{failure:#}"
    );
    assert_eq!(
        cache_file_snapshots(cache_dir.path()),
        before,
        "cold exact search must not create cache files"
    );
}

fn assert_product_search_fails_closed_on_stale_core(
    workspace: &Path,
    cache_dir: &Path,
    query: &str,
) {
    let output = run_cli(
        workspace,
        cache_dir,
        &[
            "search",
            "--query",
            query,
            "--repo-text",
            "off",
            "--limit",
            "5",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !output.status.success(),
        "product search should fail closed on a stale core publication"
    );
    let failure: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("parse stale-core failure: {error}; output={output:#?}"));
    assert_eq!(
        failure["error"]["code"], "project_unavailable",
        "{failure:#}"
    );
    assert_eq!(
        failure["error"]["message"], "exact_search requires a fresh complete core publication",
        "{failure:#}"
    );
}

fn assert_retrieval_failure_output(output: std::process::Output) {
    let failure = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        failure.contains("retrieval is unavailable or degraded")
            && (failure.contains("expected profile=agent mode=full")
                || failure.contains("retrieval_manifest_missing")),
        "command should explain the mandatory retrieval gate, got: {failure}"
    );
}

fn assert_symbol_reads_focus_node(workspace: &Path, cache_dir: &Path, node_id: &str) {
    let symbol = run_cli_json(
        workspace,
        cache_dir,
        &[
            "symbol",
            &format!("--id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_eq!(
        string_field(&symbol, &["symbol", "node", "display_name"]),
        "AppController"
    );
}

fn assert_trail_reads_focus_node(workspace: &Path, cache_dir: &Path, node_id: &str) {
    let trail = run_cli_json(
        workspace,
        cache_dir,
        &[
            "trail",
            &format!("--id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        array_is_non_empty(&trail, &["trail", "trail", "nodes"]),
        "trail should return at least the focus node"
    );

    let trail_story = run_cli_json(
        workspace,
        cache_dir,
        &[
            "trail",
            &format!("--id={node_id}"),
            "--story",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        string_field(&trail_story, &["trail", "story", "summary"]).contains("Story trail"),
        "trail --story JSON should include a readable story summary"
    );
    assert!(
        trail_story["trail"]["story"]["entry_points"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "trail --story should describe entry points"
    );
    assert!(
        trail_story["trail"]["story"]["test_scope"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item
                .as_str()
                .is_some_and(|text| text.contains("tests and benches excluded")))),
        "trail --story should make test scope explicit"
    );
}

fn assert_snippet_reads_focus_node(workspace: &Path, cache_dir: &Path, node_id: &str) {
    let snippet = run_cli_json(
        workspace,
        cache_dir,
        &[
            "snippet",
            &format!("--id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        string_field(&snippet, &["snippet", "snippet"]).contains("AppController"),
        "snippet should include source for AppController"
    );
}

fn add_and_assert_bookmark_focus(workspace: &Path, cache_dir: &Path, node_id: &str) -> String {
    let bookmark = run_cli_json(
        workspace,
        cache_dir,
        &[
            "bookmark",
            "add",
            &format!("--id={node_id}"),
            "--comment",
            "entry point under review",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    let bookmark_id = string_field(&bookmark, &["bookmark", "bookmark", "id"]).to_string();
    assert_eq!(
        string_field(&bookmark, &["bookmark", "bookmark", "node_id"]),
        node_id
    );
    assert_eq!(
        string_field(&bookmark, &["category", "name"]),
        "Investigation"
    );

    let bookmarks = run_cli_json(
        workspace,
        cache_dir,
        &["bookmark", "list", "--format", "json"],
    );
    assert!(
        bookmarks["bookmarks"]
            .as_array()
            .expect("bookmarks")
            .iter()
            .any(|bookmark| bookmark["bookmark"]["id"] == bookmark_id),
        "bookmark list should include the saved focus"
    );
    bookmark_id
}

fn assert_context_bookmark_fails_closed_without_full_sidecars(
    workspace: &Path,
    cache_dir: &Path,
    bookmark_id: &str,
) {
    let output = run_cli(
        workspace,
        cache_dir,
        &[
            "context",
            "--bookmark",
            bookmark_id,
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !output.status.success(),
        "context --bookmark should fail closed without full sidecars"
    );
    assert_retrieval_failure_output(output);
}

fn assert_explore_outputs_focus_context(workspace: &Path, cache_dir: &Path, node_id: &str) {
    let explore = run_cli_json(
        workspace,
        cache_dir,
        &[
            "explore",
            &format!("--id={node_id}"),
            "--no-tui",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_eq!(
        string_field(&explore, &["profile", "requested"]),
        "default",
        "explore should preserve the default profile unless --profile is explicit"
    );
    assert_eq!(
        string_field(&explore, &["profile", "caller_scope"]),
        "production-only",
        "default explore should keep production-only caller scope"
    );
    assert_eq!(
        string_field(&explore, &["search", "selected", "display_name"]),
        "AppController"
    );
    assert_eq!(
        string_field(&explore, &["status", "refresh"]),
        "none",
        "explore should report the read-command refresh mode"
    );
    assert!(
        array_is_non_empty(&explore, &["status", "next_commands"]),
        "explore JSON should include target-aware next commands"
    );
    assert!(
        explore["status"]["layer_notes"]
            .as_array()
            .expect("layer notes")
            .iter()
            .any(|note| note
                .as_str()
                .is_some_and(|note| note.starts_with("query_resolution:"))),
        "explore status should preserve the query-resolution layer"
    );
    assert!(
        explore["status"]["layer_notes"]
            .as_array()
            .expect("layer notes")
            .iter()
            .any(|note| note
                .as_str()
                .is_some_and(|note| note.starts_with("snippet_context:"))),
        "explore status should preserve the snippet-context layer"
    );
    assert!(
        array_is_non_empty(&explore, &["trail", "trail", "nodes"]),
        "explore JSON should include trail detail"
    );
    assert!(
        string_field(&explore, &["relationship_evidence", "map_source"]).contains("trail_context"),
        "explore JSON should expose relationship evidence source"
    );
    assert!(
        string_field(&explore, &["snippet", "snippet"]).contains("AppController"),
        "explore JSON should include snippet detail"
    );
    assert!(
        array_is_non_empty(&explore, &["source_packet", "files"]),
        "explore JSON should include grouped source packet files"
    );
    assert!(
        array_is_non_empty(&explore, &["source_packet", "notes"]),
        "explore JSON should include source packet budget/coverage notes"
    );

    let profiled_explore = run_cli_json(
        workspace,
        cache_dir,
        &[
            "explore",
            &format!("--id={node_id}"),
            "--profile",
            "test-impact",
            "--no-tui",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_eq!(
        string_field(&profiled_explore, &["profile", "requested"]),
        "test-impact",
        "explicit explore profiles should be reflected in JSON output"
    );
    assert_eq!(
        string_field(&profiled_explore, &["profile", "caller_scope"]),
        "include-tests-and-benches",
        "test-impact profile should include test and bench neighbors"
    );
    assert!(
        profiled_explore["profile"]["depth"]
            .as_u64()
            .unwrap_or_default()
            >= 4,
        "test-impact profile should raise the default depth floor"
    );

    let explore_markdown = run_cli(
        workspace,
        cache_dir,
        &[
            "explore",
            &format!("--id={node_id}"),
            "--no-tui",
            "--refresh",
            "none",
            "--format",
            "markdown",
        ],
    );
    assert!(
        explore_markdown.status.success(),
        "explore markdown failed: {}",
        String::from_utf8_lossy(&explore_markdown.stderr)
    );
    let explore_markdown = String::from_utf8_lossy(&explore_markdown.stdout);
    for expected in [
        "# Explore",
        "status:",
        "profile:",
        "search:",
        "results:",
        "resolution:",
        "navigation:",
        "relationship evidence:",
        "symbol:",
        "trail:",
        "snippet:",
        "source packet:",
        "snippet_context:",
        "semantic_runtime:",
        "output_write:",
    ] {
        assert!(
            explore_markdown.contains(expected),
            "explore markdown should contain `{expected}`:\n{explore_markdown}"
        );
    }
}

fn assert_files_and_affected_read_existing_cache(workspace: &Path, cache_dir: &Path) {
    let files = run_cli_json(
        workspace,
        cache_dir,
        &[
            "files",
            "--role",
            "test",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        files["summary"]["file_count"]
            .as_u64()
            .is_some_and(|count| count >= 1),
        "files JSON should keep whole-index file_count: {files:#}"
    );
    assert_eq!(
        files["summary"]["indexed_file_count"].as_u64(),
        files["summary"]["file_count"].as_u64(),
        "files JSON should keep indexed_file_count whole-index for the fully indexed fixture: {files:#}"
    );
    assert!(
        files["summary"]["filtered_file_count"]
            .as_u64()
            .is_some_and(|count| count >= 1),
        "files JSON should include filtered_file_count: {files:#}"
    );
    assert!(
        files["summary"]["file_count"].as_u64() > files["summary"]["filtered_file_count"].as_u64(),
        "role-filtered files JSON should keep whole-index file_count distinct from filtered_file_count: {files:#}"
    );
    assert_eq!(
        files["summary"]["visible_file_count"].as_u64(),
        files["files"].as_array().map(|items| items.len() as u64),
        "visible_file_count should match returned rows: {files:#}"
    );
    assert!(
        files["summary"]["language_counts"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["language"] == "rust"
                && item["support_mode"] == "parser_backed_graph"
                && item["evidence_tier"] == "graph_fidelity"
                && item["claim_label"] == "parser-backed graph, fidelity-gated")),
        "files JSON should include language counts with support tiers: {files:#}"
    );
    assert_eq!(
        files["summary"]["framework_route_coverage_included"],
        serde_json::json!(false),
        "default files JSON should omit the framework route catalog: {files:#}"
    );
    assert_eq!(
        files["summary"]["framework_route_coverage"],
        serde_json::json!([]),
        "default files JSON should omit the framework route catalog: {files:#}"
    );
    let files_with_routes = run_cli_json(
        workspace,
        cache_dir,
        &[
            "files",
            "--role",
            "test",
            "--refresh",
            "none",
            "--format",
            "json",
            "--include-framework-coverage",
        ],
    );
    assert!(
        files_with_routes["summary"]["framework_route_coverage"]
            .as_array()
            .is_some_and(
                |items| items.iter().any(|item| item["framework"] == "express"
                    && item["promotable"] == true
                    && item["coverage_evidence"].is_string()
                    && item["unsupported_patterns"].is_array())
                    && items.iter().any(|item| item["framework"] == "nextjs"
                        && item["confidence_floor"] == "file_convention"
                        && item["known_gaps"].is_array())
                    && items.iter().any(|item| item["framework"] == "gin"
                        && item["handler_link_support"] == "not_claimed_text_only")
            ),
        "opt-in files JSON should include framework route coverage matrix: {files_with_routes:#}"
    );
    assert!(
        files["files"]
            .as_array()
            .expect("files")
            .iter()
            .any(|file| file["path"]
                .as_str()
                .is_some_and(|path| path.contains("app_controller_test.rs"))
                && file["role"] == "test"),
        "files --role test should list inferred test files: {files:#}"
    );

    let limited_files = run_cli_json(
        workspace,
        cache_dir,
        &[
            "files",
            "--language",
            "rust",
            "--limit",
            "1",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        limited_files["summary"]["filtered_file_count"].as_u64()
            > limited_files["summary"]["visible_file_count"].as_u64(),
        "limited files JSON should report filtered rows before truncation and visible rows after truncation: {limited_files:#}"
    );
    assert_eq!(
        limited_files["summary"]["visible_file_count"].as_u64(),
        limited_files["files"]
            .as_array()
            .map(|items| items.len() as u64),
        "limited visible_file_count should match returned rows: {limited_files:#}"
    );
    assert_eq!(
        limited_files["summary"]["truncated"].as_bool(),
        Some(true),
        "limited files JSON should mark truncation: {limited_files:#}"
    );

    let files_markdown = run_cli(
        workspace,
        cache_dir,
        &[
            "files",
            "--language",
            "rust",
            "--limit",
            "3",
            "--refresh",
            "none",
            "--format",
            "markdown",
            "--include-framework-coverage",
        ],
    );
    assert!(
        files_markdown.status.success(),
        "files markdown failed: {}",
        String::from_utf8_lossy(&files_markdown.stderr)
    );
    let files_markdown = String::from_utf8_lossy(&files_markdown.stdout);
    assert!(
        files_markdown.contains("# indexed files")
            && files_markdown.contains("whole index files:")
            && files_markdown.contains("filtered files:")
            && files_markdown.contains("visible rows:")
            && files_markdown.contains("truncated:")
            && files_markdown.contains("languages:")
            && files_markdown.contains("rust=")
            && files_markdown.contains("[parser_backed_graph; graph_fidelity]")
            && files_markdown.contains("language_support_claims:")
            && files_markdown.contains("parser-backed graph, fidelity-gated")
            && files_markdown.contains("coverage:")
            && files_markdown.contains("framework route coverage:"),
        "files markdown should summarize inventory, support tiers, and coverage:\n{files_markdown}"
    );

    let affected = run_cli_json(
        workspace,
        cache_dir,
        &[
            "affected",
            "src/lib.rs",
            "--depth",
            "2",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert_eq!(affected["matched_file_count"], 1);
    assert!(
        affected["impacted_symbols"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| {
                item["graph_depth"].is_number()
                    && item["reason"].is_string()
                    && item["confidence"].is_string()
            })),
        "affected JSON should expand changed files to symbols with graph evidence: {affected:#}"
    );
    let tests = affected["impacted_tests"]
        .as_array()
        .expect("affected impacted_tests array");
    assert!(
        tests.iter().any(|item| {
            item["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("tests/app_controller_test.rs"))
        }),
        "the controlled AppController change should select its integration test: {affected:#}"
    );
    assert!(
        tests.iter().all(|item| {
            item["graph_depth"].is_number()
                && item["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("focused test hint"))
                && item["confidence"]
                    .as_str()
                    .is_some_and(|confidence| !confidence.is_empty())
        }),
        "affected test hints should expose graph evidence, confidence, and caveat language: {affected:#}"
    );

    let affected_stdin = run_cli_with_stdin(
        workspace,
        cache_dir,
        &[
            "affected",
            "--stdin",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
        "src/runtime.rs\n",
    );
    assert!(
        affected_stdin.status.success(),
        "affected --stdin failed: {}",
        String::from_utf8_lossy(&affected_stdin.stderr)
    );
    let affected_stdin: Value =
        serde_json::from_slice(&affected_stdin.stdout).expect("parse affected stdin json");
    assert_eq!(affected_stdin["changed_paths"][0], "src/runtime.rs");
    assert_eq!(affected_stdin["matched_file_count"], 1);

    let affected_markdown = run_cli(
        workspace,
        cache_dir,
        &[
            "affected",
            "src/lib.rs",
            "--refresh",
            "none",
            "--format",
            "markdown",
        ],
    );
    assert!(
        affected_markdown.status.success(),
        "affected markdown failed: {}",
        String::from_utf8_lossy(&affected_markdown.stderr)
    );
    let affected_markdown = String::from_utf8_lossy(&affected_markdown.stdout);
    assert!(
        affected_markdown.contains("# affected analysis")
            && affected_markdown.contains("matched files:")
            && affected_markdown.contains("impacted symbols:")
            && affected_markdown.contains("): "),
        "affected markdown should summarize impact:\n{affected_markdown}"
    );
}

#[test]
fn affected_without_explicit_paths_fails_closed_instead_of_discovering_git() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    run_git(workspace.path(), &["init"]);
    run_git(workspace.path(), &["add", "."]);
    run_git(
        workspace.path(),
        &[
            "-c",
            "user.email=codestory@example.test",
            "-c",
            "user.name=CodeStory Test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "fixture",
        ],
    );
    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    fs::write(
        workspace.path().join("src/runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("workspace:{project_name}")
}

pub fn schedule_index(project_path: &str) -> usize {
    super::open_project(project_path).len()
}

pub fn changed_after_index() -> bool {
    true
}
"#,
    )
    .expect("modify runtime fixture after index");

    let missing_paths = run_cli(
        workspace.path(),
        cache_dir.path(),
        &["affected", "--refresh", "none", "--format", "json"],
    );
    assert!(
        !missing_paths.status.success(),
        "affected without paths must fail closed: {}",
        String::from_utf8_lossy(&missing_paths.stderr)
    );
    let missing_output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&missing_paths.stdout),
        String::from_utf8_lossy(&missing_paths.stderr)
    );
    assert!(
        missing_output.contains("never discovers git") && missing_output.contains("affected.paths"),
        "CLI affected must require an explicit path source like MCP: {missing_output}"
    );

    let changes_only = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "affected",
            "--changes",
            "head",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !changes_only.status.success(),
        "--changes without PATH or --stdin must error: {}",
        String::from_utf8_lossy(&changes_only.stderr)
    );

    let affected = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "affected",
            "src/runtime.rs",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        affected["changed_paths"]
            .as_array()
            .expect("changed paths")
            .iter()
            .any(|path| path == "src/runtime.rs"),
        "explicit path analysis should keep the supplied path: {affected:#}"
    );
    assert_eq!(
        affected["completeness"]["complete"], false,
        "a source change after publication must not produce a complete impact claim: {affected:#}"
    );
    assert!(
        affected["uncovered_inputs"]
            .as_array()
            .is_some_and(|inputs| inputs.iter().any(|input| {
                input["path"] == "src/runtime.rs" && input["classification"] == "stale_index"
            })),
        "the exact modified source should carry a stale-index classification: {affected:#}"
    );
    assert!(affected.get("next_commands").is_none());
    assert!(
        affected["follow_ups"]
            .as_array()
            .is_some_and(|follow_ups| follow_ups.iter().any(|follow_up| {
                follow_up["action"] == "refresh_stale_index"
                    && follow_up["invocation"]["program"] == "codestory-cli"
                    && follow_up["invocation"]["args"]
                        .as_array()
                        .is_some_and(|args| {
                            args.windows(2)
                                .any(|pair| pair[0] == "--refresh" && pair[1] == "incremental")
                        })
            })),
        "positive stale evidence should recommend the focused structured incremental repair: {affected:#}"
    );
    assert!(
        !affected.to_string().contains("--refresh full")
            && !affected.to_string().contains("doctor --project"),
        "proven source drift should not emit generic full-refresh or doctor advice: {affected:#}"
    );

    let query = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "query",
            "trail(symbol: 'AppController')",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !query.status.success(),
        "ordinary graph queries must not inherit affected's stale-source admission"
    );
    let query_output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&query.stdout),
        String::from_utf8_lossy(&query.stderr)
    );
    assert!(
        query_output.contains("graph_assisted requires a fresh complete core publication"),
        "query must remain fail-closed on the stale source after affected observation: {query_output}"
    );

    let refreshed = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "affected",
            "src/runtime.rs",
            "--refresh",
            "incremental",
            "--format",
            "json",
        ],
    );
    assert!(
        refreshed["changed_paths"]
            .as_array()
            .expect("changed paths")
            .iter()
            .any(|path| path == "src/runtime.rs"),
        "explicit incremental refresh should keep the supplied path: {refreshed:#}"
    );
    assert_eq!(refreshed["matched_file_count"], 1, "{refreshed:#}");
}

fn run_git(workspace: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_query_search_fails_closed_without_full_sidecars(workspace: &Path, cache_dir: &Path) {
    let output = run_cli(
        workspace,
        cache_dir,
        &[
            "query",
            "search(query: 'AppController') | limit(1)",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !output.status.success(),
        "query search DSL must fail closed without full sidecars\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let failure: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("parse query-search refusal: {error}; output={output:#?}"));
    assert_eq!(
        failure["error"]["code"], "retrieval_unavailable",
        "query search DSL must report the retrieval gate: {failure:#}"
    );
    assert_retrieval_failure_output(output);
}

fn assert_context_id_fails_closed_without_full_sidecars(
    workspace: &Path,
    cache_dir: &Path,
    node_id: &str,
) {
    let output = run_cli(
        workspace,
        cache_dir,
        &[
            "context",
            &format!("--id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !output.status.success(),
        "context --id should fail closed without full sidecars"
    );
    assert_retrieval_failure_output(output);
}

fn remove_and_assert_bookmark_gone(workspace: &Path, cache_dir: &Path, bookmark_id: &str) {
    let removed = run_cli_json(
        workspace,
        cache_dir,
        &["bookmark", "remove", bookmark_id, "--format", "json"],
    );
    assert_eq!(string_field(&removed, &["removed_id"]), bookmark_id);
    let bookmarks_after_remove = run_cli_json(
        workspace,
        cache_dir,
        &["bookmark", "list", "--format", "json"],
    );
    assert!(
        bookmarks_after_remove["bookmarks"]
            .as_array()
            .expect("bookmarks after remove")
            .iter()
            .all(|bookmark| bookmark["bookmark"]["id"] != bookmark_id),
        "bookmark remove should persistently delete the saved focus"
    );
}

fn assert_packet_builds_broad_task_contract(workspace: &Path, cache_dir: &Path) {
    let args = [
        "packet",
        "--question",
        "Explain how AppController routes project opening through normalize_project",
        "--budget",
        "tiny",
        "--format",
        "json",
    ];
    let output = run_cli(workspace, cache_dir, &args);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stderr}\n{stdout}");
        assert!(
            combined.contains("retrieval is unavailable or degraded")
                || combined.contains("retrieval_manifest_missing"),
            "packet without full retrieval should fail closed with a retrieval diagnostic, got stdout:\n{stdout}\nstderr:\n{stderr}"
        );
        return;
    }
    let packet: Value = serde_json::from_slice(&output.stdout).expect("parse packet json output");
    assert_eq!(
        packet["budget"]["requested"], "tiny",
        "packet should honor the requested budget: {packet:#}"
    );
    assert!(
        packet.pointer("/plan/task_class").is_none(),
        "public packet JSON must not expose task_class: {packet:#}"
    );
    assert_eq!(
        packet["answer_sufficiency"], "not_asserted",
        "public packets must not assert semantic sufficiency: {packet:#}"
    );
    assert!(
        packet.get("compilation_status").is_none(),
        "Horizon A retrieval-first packets must omit compilation_status: {packet:#}"
    );
    assert!(
        array_is_non_empty(&packet, &["plan", "queries"]),
        "packet should expose planned retrieval queries: {packet:#}"
    );
    assert!(
        packet
            .pointer("/disposition/kind")
            .and_then(Value::as_str)
            .is_some(),
        "packet should expose disposition kind: {packet:#}"
    );
    assert!(
        array_is_non_empty(&packet, &["answer", "retrieval_trace", "steps"]),
        "packet should include the underlying retrieval trace: {packet:#}"
    );
    assert_eq!(
        packet["answer"]["retrieval_version"], "sidecar",
        "successful packet output must come from mandatory sidecar retrieval: {packet:#}"
    );
}

fn assert_stdio_context_id_fails_closed_without_full_sidecars(
    workspace: &Path,
    cache_dir: &Path,
    node_id: &str,
) {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "context",
            "arguments": {
                "id": node_id
            }
        }
    })
    .to_string();
    let stdio = run_stdio_request(workspace, cache_dir, &request);
    let decoded = stdio_tool_payload(&stdio);
    let payload = decoded.value();
    let code = payload["code"].as_str();
    let preparing = decoded.is_preparing();
    assert!(
        preparing
            || matches!(
                code,
                Some("codestory_tool_blocked" | "codestory_unavailable")
            ),
        "stdio context --id should fail closed with a typed unavailable or preparing result: {stdio:#}"
    );
    if preparing {
        assert_ne!(
            stdio["result"].get("isError").and_then(Value::as_bool),
            Some(true),
            "preparing should be a successful structured result: {stdio:#}"
        );
        assert_eq!(
            payload["state"], "preparing",
            "preparing should retain the revision-native state tag: {stdio:#}"
        );
        assert!(
            payload["retry_after_ms"]
                .as_u64()
                .is_some_and(|retry_after_ms| retry_after_ms > 0),
            "preparing should include a positive retry delay: {stdio:#}"
        );
    } else {
        assert_eq!(
            stdio["result"]["isError"], true,
            "stdio context --id should return a tool error without full retrieval: {stdio:#}"
        );
    }
    if code == Some("codestory_tool_blocked") {
        let status = payload["status"].as_str();
        assert!(
            status.is_some_and(|status| matches!(status, "repair_setup" | "blocked")),
            "stdio context --id should fail closed before serving context: {stdio:#}"
        );
    }
}

impl SearchDirSnapshot {
    fn capture(path: PathBuf) -> Self {
        fs::metadata(&path)
            .expect("search dir metadata before read commands")
            .modified()
            .expect("search dir modified before read commands");
        let marker = path.join("codestory-read-cache-marker.txt");
        fs::write(&marker, "preserve across read commands")
            .expect("write search dir preservation marker");
        Self { path, marker }
    }

    fn assert_unchanged(&self, step: &str) {
        fs::metadata(&self.path)
            .expect("search dir metadata after read commands")
            .modified()
            .expect("search dir modified after read commands");
        assert!(
            self.marker.exists(),
            "{step} should not recreate the persisted search directory"
        );
    }
}

#[test]
fn bookmarks_degrade_gracefully_after_reindex_removes_target() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );

    assert_exact_search_uses_core_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        "normalize_project",
    );
    let node_id = ground_symbol_node_id_from_existing_cache(
        workspace.path(),
        cache_dir.path(),
        "normalize_project",
        Some("src/runtime.rs"),
    );

    let bookmark = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &[
            "bookmark",
            "add",
            &format!("--id={node_id}"),
            "--comment",
            "target that will disappear",
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    let bookmark_id = string_field(&bookmark, &["bookmark", "bookmark", "id"]).to_string();

    thread::sleep(Duration::from_millis(25));
    fs::write(
        workspace.path().join("src").join("runtime.rs"),
        r#"pub fn replacement_runtime_entry(project_name: &str) -> String {
    format!("replacement:{project_name}")
}
"#,
    )
    .expect("remove bookmarked symbol");

    run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "incremental", "--format", "json"],
    );

    let bookmarks = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["bookmark", "list", "--format", "json"],
    );
    let listed = bookmarks["bookmarks"].as_array().expect("bookmarks");
    assert_eq!(
        listed.len(),
        1,
        "a removed target must leave the annotation visible and user-owned: {bookmarks:#}"
    );
    assert!(
        listed
            .iter()
            .all(|bookmark| bookmark["stale"].as_bool() == Some(true)
                && bookmark["bookmark"]["resolution_status"] == "orphaned"
                && bookmark["bookmark"]["orphan_reason"] == "target_deleted"),
        "bookmark list should report a typed orphan reason: {bookmarks:#}"
    );

    test_support::publish_agent_query_fixture(workspace.path(), cache_dir.path());
    let context = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "context",
            "--bookmark",
            &bookmark_id,
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !context.status.success(),
        "context --bookmark should not silently ignore a removed bookmark"
    );
    let failure = format!(
        "{}{}",
        String::from_utf8_lossy(&context.stdout),
        String::from_utf8_lossy(&context.stderr)
    );
    assert!(
        failure.contains("Bookmark not found") || failure.contains("is an orphan"),
        "context --bookmark should explain orphaned or missing bookmark focus, got: {failure}"
    );
}

#[test]
fn read_commands_report_stale_index_freshness_without_refreshing_cache() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_tiny_rust_workspace(workspace.path());
    fs::write(
        workspace.path().join("src").join("removed_after_index.rs"),
        "pub fn removed_after_index() {}\n",
    )
    .expect("write pre-index file");

    let index = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    let storage_path = PathBuf::from(string_field(&index, &["storage_path"]));
    let search_dir = search_dir_for_storage(&storage_path);
    let database_path = codestory_runtime::resolve_core_database_path(&storage_path)
        .expect("resolve published core image");
    let database_before = fs::read(&database_path).expect("published core bytes");
    assert!(!database_before.is_empty());
    let pointer_path = storage_path.parent().unwrap().join("core/publication.json");
    let pointer_before = fs::read(&pointer_path).expect("publication pointer");
    let storage_before = fs::metadata(&database_path)
        .expect("storage metadata before read")
        .modified()
        .expect("storage modified before read");
    let search_dir_before = fs::metadata(&search_dir)
        .expect("search dir metadata before read")
        .modified()
        .expect("search dir modified before read");

    let search_payload_before = cache_file_snapshots(&search_dir);
    assert!(
        !search_payload_before.is_empty(),
        "fixture must contain persisted search payloads"
    );

    thread::sleep(Duration::from_millis(25));
    fs::write(
        workspace.path().join("src").join("runtime.rs"),
        r#"pub fn normalize_project(project_name: &str) -> String {
    format!("changed:{project_name}")
}
"#,
    )
    .expect("modify indexed file after indexing");
    fs::write(
        workspace.path().join("src").join("new_after_index.rs"),
        "pub fn new_after_index() {}\n",
    )
    .expect("write new file after indexing");
    fs::remove_file(workspace.path().join("src").join("removed_after_index.rs"))
        .expect("remove indexed file after indexing");

    assert_product_search_fails_closed_on_stale_core(
        workspace.path(),
        cache_dir.path(),
        "AppController",
    );

    let doctor = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["doctor", "--format", "json"],
    );
    assert_stale_freshness_counts(&doctor, "doctor");

    let storage_after = fs::metadata(&database_path)
        .expect("storage metadata after read")
        .modified()
        .expect("storage modified after read");
    let search_dir_after = fs::metadata(&search_dir)
        .expect("search dir metadata after read")
        .modified()
        .expect("search dir modified after read");
    assert_eq!(
        storage_before, storage_after,
        "read freshness checks should not mutate the SQLite cache"
    );
    assert_eq!(fs::read(&database_path).unwrap(), database_before);
    assert_eq!(fs::read(&pointer_path).unwrap(), pointer_before);
    assert_eq!(
        codestory_runtime::resolve_core_database_path(&storage_path).unwrap(),
        database_path,
        "observational reads must retain the same active image"
    );
    assert_eq!(
        cache_file_snapshots(&search_dir),
        search_payload_before,
        "read freshness checks must preserve persisted search bytes and metadata"
    );
    assert_eq!(
        search_dir_before, search_dir_after,
        "read freshness checks should not recreate the persisted search directory"
    );
}

#[test]
fn context_json_reports_deep_trace_by_default() {
    let workspace = tempdir().expect("workspace dir");
    let cache_dir = tempdir().expect("cache dir");
    write_investigation_workspace(workspace.path());

    let index = run_cli_json(
        workspace.path(),
        cache_dir.path(),
        &["index", "--refresh", "full", "--format", "json"],
    );
    assert!(
        index["summary"]["stats"]["node_count"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        "index should discover investigation fixture symbols"
    );

    assert_exact_search_uses_core_without_full_sidecars(
        workspace.path(),
        cache_dir.path(),
        "parse_investigation_event",
    );
    let node_id = ground_symbol_node_id_from_existing_cache(
        workspace.path(),
        cache_dir.path(),
        "parse_investigation_event",
        None,
    );

    let context = run_cli(
        workspace.path(),
        cache_dir.path(),
        &[
            "context",
            &format!("--id={node_id}"),
            "--refresh",
            "none",
            "--format",
            "json",
        ],
    );
    assert!(
        !context.status.success(),
        "context --id should fail closed without full sidecars"
    );
    assert_retrieval_failure_output(context);
}
mod test_support;
