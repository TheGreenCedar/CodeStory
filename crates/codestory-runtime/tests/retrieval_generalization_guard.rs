//! Ensures the retrieval generalization lint script stays runnable from the workspace root.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, OnceLock};
use tempfile::TempDir;

static LINT_SCRIPT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn production_source(contents: &str) -> &str {
    match contents.find("#[cfg(test)]") {
        Some(marker) => &contents[..marker],
        None => contents,
    }
}

fn has_filename_literal(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let quote = bytes[index];
        if quote == b'"' || quote == b'\'' || quote == b'`' {
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != quote {
                end += 1;
            }
            if end > start {
                let token = &line[start..end];
                if token
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric())
                    && token.contains('.')
                    && token.chars().all(|c| {
                        c.is_ascii_lowercase()
                            || c.is_ascii_digit()
                            || c == '.'
                            || c == '_'
                            || c == '-'
                    })
                {
                    return true;
                }
            }
            index = end;
        }
        index += 1;
    }
    false
}

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn lint_script(repo_root: &Path) -> PathBuf {
    let script = repo_root.join("scripts/lint-retrieval-generalization.mjs");
    assert!(
        script.is_file(),
        "expected lint script at {}",
        script.display()
    );
    script
}

fn run_lint_with_scan_root(repo_root: &Path, script: &Path, scan_root: &Path) -> Output {
    let _guard = LINT_SCRIPT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("lock lint script subprocess");
    Command::new("node")
        .arg(script)
        .current_dir(repo_root)
        .env("CODESTORY_RETRIEVAL_GENERALIZATION_SCAN_ROOTS", scan_root)
        .output()
        .expect("run lint-retrieval-generalization.mjs against fixture")
}

fn run_lint_with_fixture(contents: &str) -> Output {
    let repo_root = workspace_root();
    let script = lint_script(&repo_root);
    let fixture_root = TempDir::new().expect("create fixture root");
    std::fs::write(fixture_root.path().join("fixture.rs"), contents).expect("write fixture");
    run_lint_with_scan_root(&repo_root, &script, fixture_root.path())
}

fn run_lint_with_named_fixtures(fixtures: &[(&str, &str)]) -> Output {
    let repo_root = workspace_root();
    let script = lint_script(&repo_root);
    let fixture_root = TempDir::new().expect("create fixture root");
    for (name, contents) in fixtures {
        std::fs::write(fixture_root.path().join(name), contents).expect("write fixture");
    }
    run_lint_with_scan_root(&repo_root, &script, fixture_root.path())
}

fn run_lint_with_prompt_script_fixture(contents: &str) -> Output {
    let repo_root = workspace_root();
    let script = lint_script(&repo_root);
    let fixture_root = TempDir::new().expect("create fixture root");
    let prompt_script = fixture_root.path().join("prompt-corpus.mjs");
    std::fs::write(
        fixture_root.path().join("fixture.rs"),
        "pub fn repository_neutral_fixture() {}\n",
    )
    .expect("write neutral Rust fixture");
    std::fs::write(&prompt_script, contents).expect("write prompt script fixture");

    let _guard = LINT_SCRIPT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("lock lint script subprocess");
    Command::new("node")
        .arg(&script)
        .current_dir(&repo_root)
        .env(
            "CODESTORY_RETRIEVAL_GENERALIZATION_SCAN_ROOTS",
            fixture_root.path(),
        )
        .env(
            "CODESTORY_RETRIEVAL_GENERALIZATION_PROMPT_SCRIPT",
            &prompt_script,
        )
        .output()
        .expect("run lint with prompt script fixture")
}

fn run_lint_with_non_rust_fixture(file_name: &str, contents: &str) -> Output {
    let repo_root = workspace_root();
    let script = lint_script(&repo_root);
    let fixture_root = TempDir::new().expect("create fixture root");
    std::fs::write(
        fixture_root.path().join("neutral.rs"),
        "pub fn repository_neutral_fixture() {}\n",
    )
    .expect("write neutral Rust fixture");
    std::fs::write(fixture_root.path().join(file_name), contents).expect("write non-Rust fixture");

    let _guard = LINT_SCRIPT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("lock lint script subprocess");
    Command::new("node")
        .arg(&script)
        .current_dir(&repo_root)
        .env(
            "CODESTORY_RETRIEVAL_GENERALIZATION_SCAN_ROOTS",
            fixture_root.path(),
        )
        .env(
            "CODESTORY_RETRIEVAL_GENERALIZATION_NON_RUST_SCAN_ROOTS",
            fixture_root.path(),
        )
        .output()
        .expect("run lint with non-Rust fixture")
}

#[test]
fn retrieval_generalization_lint_script_exits_clean_with_extra_fixture_root() {
    let repo_root = workspace_root();
    let script = lint_script(&repo_root);
    let fixture_root = TempDir::new().expect("create fixture root");

    let _guard = LINT_SCRIPT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("lock lint script subprocess");
    let output = Command::new("node")
        .arg(&script)
        .current_dir(&repo_root)
        .env(
            "CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_SCAN_ROOTS",
            fixture_root.path(),
        )
        .output()
        .expect("run lint-retrieval-generalization.mjs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "lint script should exit 0 when retrieval integration trees are clean; stderr={stderr}"
    );
    let production_file_count = stdout
        .split(" production file(s)")
        .next()
        .and_then(|prefix| prefix.split_whitespace().last())
        .and_then(|value| value.parse::<u32>().ok())
        .expect("parse production file count from lint stdout");
    assert!(
        production_file_count > 0,
        "extra fixture root should not replace the real production scan roots, stdout={stdout}"
    );
}

#[test]
fn ranker_production_has_no_filename_literals() {
    let repo_root = workspace_root();
    let ranker = repo_root.join("crates/codestory-retrieval/src/ranker.rs");
    assert!(ranker.is_file(), "expected ranker at {}", ranker.display());

    let contents = std::fs::read_to_string(&ranker).expect("read ranker.rs");
    let production = production_source(&contents);
    let offending_line = production
        .lines()
        .enumerate()
        .find(|(_, line)| has_filename_literal(line));

    assert!(
        offending_line.is_none(),
        "ranker production should not contain filename literals, found: {:?}",
        offending_line
    );
}

#[test]
fn linter_catches_production_literals_after_early_cfg_test_items() {
    let output = run_lint_with_fixture(
        r#"
#[cfg(test)]
use fixture::test_only_import;

pub fn production_between_cfg_items() -> &'static str {
    "neutral"
}

#[cfg(test)]
mod tests {
    const TEST_ONLY_PATH: &str = "codex-rs/test/src/lib.rs";
}

pub fn leaked_production_path() -> &'static str {
    "codex-rs/prod/src/lib.rs"
}
"#,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "fixture with production repo literal after cfg(test) should fail lint; stderr={stderr}"
    );
    assert!(
        stderr.contains("codex-rs/prod/src/lib.rs"),
        "lint failure should report the later production literal, stderr={stderr}"
    );
    assert!(
        !stderr.contains("codex-rs/test/src/lib.rs"),
        "lint should mask cfg(test) module literals, stderr={stderr}"
    );
}

#[test]
fn linter_ignores_fake_cfg_test_text_inside_comments_and_strings() {
    let output = run_lint_with_fixture(
        r##"
// #[cfg(test)]
pub const NOTE: &str = "#[cfg(test)]";
pub const RAW_NOTE: &str = r#"#[cfg(test)]"#;

pub fn leaked_production_path() -> &'static str {
    "codex-rs/prod/src/lib.rs"
}
"##,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "fake cfg(test) text in comments/strings must not mask later production, stderr={stderr}"
    );
    assert!(
        stderr.contains("codex-rs/prod/src/lib.rs"),
        "lint failure should report the production literal after fake cfg text, stderr={stderr}"
    );
}

#[test]
fn linter_catches_current_holdout_literals_in_production() {
    let output = run_lint_with_fixture(
        r#"
pub fn leaked_holdout_probe() -> &'static [&'static str] {
    &[
        "axios",
        "redis",
        "ripgrep",
        "dispatchRequest",
        "readQueryFromClient",
        "HiArgs",
        "server.c",
        "core/main.rs",
        "haystack.rs",
    ]
}
"#,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "fixture with current holdout literals should fail lint; stderr={stderr}"
    );
    for expected in ["dispatchRequest", "readQueryFromClient", "core/main.rs"] {
        assert!(
            stderr.contains(expected),
            "lint failure should report current holdout literal {expected}, stderr={stderr}"
        );
    }
}

#[test]
fn linter_catches_cross_repo_query_catalog_phrases_in_production() {
    let output = run_lint_with_fixture(
        r#"
pub fn leaked_cross_repo_query() -> &'static str {
    "project loads settings refreshes source groups computes refresh info and builds an index"
}
"#,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "fixture with a cross-repo query phrase should fail lint; stderr={stderr}"
    );
    assert!(
        stderr.contains(
            "project loads settings refreshes source groups computes refresh info and builds an index"
        ),
        "lint failure should report the query-catalog phrase, stderr={stderr}"
    );
}

#[test]
fn linter_catches_manifest_prompts_forbidden_claims_and_partial_holdout_paths() {
    let prompt = "A bug report says response helpers sometimes choose the wrong status, body, or content type when callers use res.send, res.json, or sendFile. Identify the primary files and functions to inspect before editing.";
    let forbidden_claim =
        "Project::buildIndex directly parses source files instead of building indexing tasks.";
    let output = run_lint_with_fixture(
        r#"
pub const LEAKED_MANIFEST_PROMPT: &str =
    "A bug report says response helpers sometimes choose the wrong status, body, or content type when callers use res.send, res.json, or sendFile. Identify the primary files and functions to inspect before editing.";
pub const LEAKED_FORBIDDEN_CLAIM: &str =
    "Project::buildIndex directly parses source files instead of building indexing tasks.";
pub const LEAKED_PARTIAL_HOLDOUT_PATH: &str = "/data/indexer/";
pub const LEAKED_EVAL_MANIFEST_PROBE: &str = "run_exec_session";
pub const LEAKED_EVAL_SOURCE_PROBE: &str = "createCacheHelper";
"#,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "fixture with manifest prompt, forbidden claim, and partial holdout path should fail lint; stderr={stderr}"
    );
    for expected in [
        prompt,
        forbidden_claim,
        "/data/indexer/",
        "run_exec_session",
        "createCacheHelper",
    ] {
        assert!(
            stderr.contains(expected),
            "lint failure should report {expected}, stderr={stderr}"
        );
    }
}

#[test]
fn linter_structurally_rejects_production_dependencies_on_eval_corpora() {
    let output = run_lint_with_fixture(
        r#"
pub const LEAKED_TASK_CORPUS: &str = "benchmarks/tasks/holdout-retrieval/axios-request-dispatch.task.json";
pub const LEAKED_QUERY_CORPUS: &str = "scripts/cross-repo-sourcetrail-queries.mjs";
pub const LEAKED_EVAL_PROBES: &str = "benchmarks/tasks/eval-probes.json";
"#,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "production references to eval/query corpora must fail lint; stderr={stderr}"
    );
    for expected in [
        "benchmarks/tasks",
        "scripts/cross-repo-sourcetrail-queries.mjs",
        "benchmarks/tasks/eval-probes.json",
    ] {
        assert!(
            stderr.contains(expected),
            "lint failure should identify corpus boundary {expected}; stderr={stderr}"
        );
    }
}

#[test]
fn linter_rejects_constructed_eval_corpus_dependencies() {
    let output = run_lint_with_fixture(
        r#"
pub const LEAKED_TASK_CORPUS: &str = concat!("benchmarks", "/tasks", "/eval-probes.json");
pub const LEAKED_PACKET_FIXTURE: &str = concat!("crates/codestory-cli/tests/fixtures/", "packet_search_eval");
pub const LEAKED_QUALITY_FIXTURE: &str = concat!("crates/codestory-bench/tests/", "fixtures/agent_quality");
pub const LEAKED_DEPENDENCY: &str = include_str!(concat!("../../benchmarks/", "tasks/eval-probes.json"));
"#,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "constructed production corpus dependencies must fail lint; stderr={stderr}"
    );
    for expected in ["benchmarkstasks", "packetsearcheval", "agentquality"] {
        assert!(
            stderr.to_lowercase().contains(expected),
            "lint failure should identify constructed boundary {expected}; stderr={stderr}"
        );
    }
}

#[test]
fn linter_rejects_direct_and_split_non_rust_corpus_dependencies() {
    let cases = [
        (
            "leaked.ps1",
            "$corpus = \"scripts\\cross-repo-\" + `\n  \"sourcetrail-queries.mjs\"\n",
            "scriptscrossreposourcetrailqueriesmjs",
        ),
        (
            "leaked.sh",
            "corpus=\"benchmarks/tasks/\"\\\n\"eval-probes.json\"\n",
            "benchmarkstasksevalprobesjson",
        ),
        (
            "leaked.mjs",
            "export const corpus = import(\"./FETCH-HOLDOUT-REPOS.MJS\");\n",
            "fetch-holdout-repos.mjs",
        ),
    ];
    for (file_name, contents, expected) in cases {
        let output = run_lint_with_non_rust_fixture(file_name, contents);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "protected non-Rust corpus dependency in {file_name} must fail lint; stderr={stderr}"
        );
        assert!(
            stderr.to_ascii_lowercase().contains(expected),
            "lint failure should identify {expected} in {file_name}; stderr={stderr}"
        );
    }
}

#[test]
fn linter_fails_closed_when_one_prompt_corpus_entry_is_not_a_literal() {
    let output = run_lint_with_prompt_script_fixture(
        r#"
const PUBLIC_REPOS = {
  first: { prompt: "first benchmark prompt remains a static literal for the guard" },
  second: { prompt: buildPromptAtRuntime() },
};
const ALL_REPOS = { ...PUBLIC_REPOS };
"#,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "partial prompt parser drift must fail closed; stderr={stderr}"
    );
    assert!(
        stderr.contains("discovered 2 prompt properties but parsed 1 literal prompts"),
        "failure should report the partial corpus parse, stderr={stderr}"
    );
}

#[test]
fn linter_catches_nested_manifest_derived_claims_in_production_only() {
    let nested_manifest_claim =
        "The top-level request helper opens a Session and delegates to Session.request.";

    let test_only_output = run_lint_with_fixture(
        r#"
#[cfg(test)]
mod tests {
    const TEST_ONLY_EXPECTED_CLAIM: &str =
        "The top-level request helper opens a Session and delegates to Session.request.";
}

pub fn generic_production_note() -> &'static str {
    "generic role coverage should stay repository neutral"
}
"#,
    );
    let test_only_stderr = String::from_utf8_lossy(&test_only_output.stderr);
    assert!(
        test_only_output.status.success(),
        "nested manifest-derived claims should be allowed inside cfg(test) items; stderr={test_only_stderr}"
    );

    let output = run_lint_with_fixture(
        r#"
pub fn leaked_nested_manifest_claim() -> &'static str {
    "The top-level request helper opens a Session and delegates to Session.request."
}
"#,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "fixture with nested manifest-derived claim should fail lint; stderr={stderr}"
    );
    assert!(
        stderr.contains(nested_manifest_claim),
        "lint failure should report the nested manifest-derived claim, stderr={stderr}"
    );
}

#[test]
fn linter_catches_split_benchmark_family_literals_in_production() {
    let output = run_lint_with_fixture(
        r##"
pub fn leaked_split_family_markers() -> Vec<String> {
    vec![
        ["use", "s", "wr"].concat(),
        ["string", "utils"].concat(),
        ["charsequence", "utils"].concat(),
        ["source/animate", ".css"].concat(),
        [
            "s",
            "wr",
        ].concat(),
        [
            "auto",
            "mapper",
        ].concat(),
        [
            r#"s"#,
            r#"wr"#,
        ].concat(),
        [
            r#"string"#,
            r#"utils"#,
        ].concat(),
    ]
}
"##,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "split benchmark-family literals should fail lint; stderr={stderr}"
    );
    for expected in [
        "swr",
        "useswr",
        "stringutils",
        "automapper",
        "sourceanimatecss",
    ] {
        assert!(
            stderr.to_ascii_lowercase().contains(expected),
            "lint failure should report compact benchmark marker {expected}; stderr={stderr}"
        );
    }
}

#[test]
fn linter_masks_preceding_attrs_for_cfg_test_items() {
    let output = run_lint_with_fixture(
        r#"
#[doc = "codex-rs/test-only"]
#[cfg(test)]
mod tests {
    const TEST_ONLY_PATH: &str = "codex-rs/test/src/lib.rs";
}

pub fn production_path() -> &'static str {
    "workspace/app/src/lib.rs"
}
"#,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "doc attrs attached to cfg(test) items should be masked with the item, stderr={stderr}"
    );
}

#[test]
fn linter_masks_test_only_cfg_attr_and_equivalent_cfg_forms() {
    let output = run_lint_with_fixture(
        r#"
#[cfg_attr(test, doc = "codex-rs/test-only")]
pub fn production_path() -> &'static str {
    "workspace/app/src/lib.rs"
}

#[cfg(not(not(test)))]
mod tests {
    const TEST_ONLY_PATH: &str = "codex-rs/test/src/lib.rs";
}
"#,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "test-only cfg_attr and logically test-only cfg forms should be masked, stderr={stderr}"
    );
}

#[test]
fn linter_scans_production_files_with_diagnostic_or_test_like_names() {
    let output = run_lint_with_named_fixtures(&[
        (
            "test_support.rs",
            r#"pub fn leaked_test_support_path() -> &'static str { "codex-rs/test-support/src/lib.rs" }"#,
        ),
        (
            "eval_probes.rs",
            r#"pub fn leaked_eval_probe_path() -> &'static str { "codex-rs/eval/src/lib.rs" }"#,
        ),
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "production files should not be excluded solely by basename, stderr={stderr}"
    );
    for file in ["test_support.rs", "eval_probes.rs"] {
        assert!(
            stderr.contains(file),
            "lint should report banned literals in {file}, stderr={stderr}"
        );
    }
}
