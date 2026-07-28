//! Ensures the retrieval generalization lint script stays runnable from the workspace root.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tempfile::TempDir;

static LINT_SCRIPT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Serialises the lint subprocesses. The lock guards no shared state -- only the
/// cost of running many `node` processes at once -- so a test that panics while
/// holding it has corrupted nothing. Recovering from the poison rather than
/// propagating it keeps one real failure reported as one failure: without this,
/// the first genuine assertion turns every later test into a `PoisonError`
/// panic, and the failure list says nothing about how much is actually broken.
fn lint_script_lock() -> MutexGuard<'static, ()> {
    LINT_SCRIPT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    let _guard = lint_script_lock();
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

    let _guard = lint_script_lock();
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

fn run_lint_with_non_rust_fixtures(fixtures: &[(&str, &str)]) -> Output {
    let repo_root = workspace_root();
    let script = lint_script(&repo_root);
    let fixture_root = TempDir::new().expect("create fixture root");
    std::fs::write(
        fixture_root.path().join("neutral.rs"),
        "pub fn repository_neutral_fixture() {}\n",
    )
    .expect("write neutral Rust fixture");
    for (file_name, contents) in fixtures {
        let file_path = fixture_root.path().join(file_name);
        std::fs::create_dir_all(file_path.parent().expect("fixture parent"))
            .expect("create non-Rust fixture parent");
        std::fs::write(file_path, contents).expect("write non-Rust fixture");
    }

    let _guard = lint_script_lock();
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

    let _guard = lint_script_lock();
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
    // Every planted literal, not a sample of them: asserting on three of nine
    // let the other six stop being banned without this test noticing.
    for expected in [
        "axios",
        "redis",
        "ripgrep",
        "dispatchRequest",
        "readQueryFromClient",
        "HiArgs",
        "server.c",
        "core/main.rs",
        "haystack.rs",
    ] {
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
    let rejected = run_lint_with_non_rust_fixtures(&[
        (
            "leaked.ps1",
            "$corpus = \"scripts\\cross-repo-\" + `\n  \"sourcetrail-queries.mjs\"\n",
        ),
        (
            "leaked.sh",
            "prefix=./scripts\nscript=${prefix#./}/fetch-holdout-repos.mjs\ncorpus=benchmarks/ta\\\nsks/eval-probes.json\n",
        ),
        (
            "workflow-command.yml",
            "run: |2-\n  node scripts/fetch-\\\n  holdout-repos.mjs\n",
        ),
        (
            "surrounding-command.mjs",
            "const command = \"node scripts/fetch-\" + \"holdout-repos.mjs --json\";\nconst config = \"prefix benchmarks/ta\" + \"sks/eval-probes.json suffix\";\n",
        ),
        (
            "line-continuation.mjs",
            "const script = \"scripts/fetch-holdout-\\\nrepos.mjs\";\n",
        ),
        (
            "joined-shell-word.sh",
            "node scripts/fetch-'holdout-repos.mjs'\n",
        ),
        (
            "joined-workflow-word.yml",
            "run: |\n  node scripts/fetch-'holdout-repos.mjs'\n",
        ),
        (
            "quoted-run-key.yml",
            "steps:\n  - \"run\": |\n      node scripts/fetch-'holdout-repos.mjs'\n",
        ),
        (
            "escaped-shell-word.sh",
            "node scripts/fetch\\-holdout-repos.mjs\n",
        ),
        (
            "quoted-yaml-scalar.yml",
            "value: 'clean # scripts/fetch-holdout-repos.mjs'\n",
        ),
        (
            "quoted-block-scalar.yml",
            "run: |\n  value='clean # scripts/fetch-holdout-repos.mjs'\n",
        ),
        (
            "github-script.yml",
            "uses: actions/github-script@v8\nwith:\n  script: |\n    const script = \"scripts/fetch-\" + \"holdout-repos.mjs\";\n",
        ),
        (
            "direct-harness-import.mjs",
            "import \"./scripts/codestory-agent-ab-benchmark.mjs\";\n",
        ),
        (
            "constructed-harness-import.mjs",
            "const harness = \"scripts/codestory-agent-ab-\" + \"benchmark.mjs\";\nawait import(harness);\n",
        ),
        (
            "unapproved-policy-reference.mjs",
            "const workflows = [\".github/workflows/retrieval-engine-smoke.yml\", \"unrelated.yml\"];\n",
        ),
        (
            "plugins/codestory/skills/codestory-grounding/SKILL.md",
            "Run `node scripts/fetch-holdout-repos.mjs` before grounding.\n",
        ),
        (
            ".cursor/rules/codestory.mdc",
            "Read benchmarks/tasks/eval-probes.json before answering.\n",
        ),
    ]);
    let rejected_stderr = String::from_utf8_lossy(&rejected.stderr).to_ascii_lowercase();
    assert!(
        !rejected.status.success(),
        "executable corpus dependencies must fail lint; stderr={rejected_stderr}"
    );
    for (file_name, marker) in [
        ("leaked.ps1", "scriptscrossreposourcetrailqueriesmjs"),
        ("leaked.sh", "benchmarkstasksevalprobesjson"),
        ("workflow-command.yml", "fetchholdoutreposmjs"),
        ("surrounding-command.mjs", "fetchholdoutreposmjs"),
        ("line-continuation.mjs", "fetchholdoutreposmjs"),
        ("joined-shell-word.sh", "fetch-'holdout-repos.mjs"),
        ("joined-workflow-word.yml", "fetch-'holdout-repos.mjs"),
        ("quoted-run-key.yml", "fetch-'holdout-repos.mjs"),
        ("escaped-shell-word.sh", "fetch\\-holdout-repos.mjs"),
        ("quoted-yaml-scalar.yml", "fetch-holdout-repos.mjs"),
        ("quoted-block-scalar.yml", "fetch-holdout-repos.mjs"),
        ("github-script.yml", "fetchholdoutreposmjs"),
        (
            "direct-harness-import.mjs",
            "codestory-agent-ab-benchmark.mjs",
        ),
        (
            "constructed-harness-import.mjs",
            "codestoryagentabbenchmarkmjs",
        ),
        (
            "unapproved-policy-reference.mjs",
            "retrieval-engine-smoke.yml",
        ),
        ("skill.md", "fetch-holdout-repos.mjs"),
        ("codestory.mdc", "benchmarks/tasks"),
    ] {
        assert!(
            rejected_stderr.contains(file_name) && rejected_stderr.contains(marker),
            "lint failure should identify {file_name} and {marker}; stderr={rejected_stderr}"
        );
    }

    let allowed = run_lint_with_non_rust_fixtures(&[
        (
            "prose.md",
            "The benchmark harness reads `benchmarks/tasks/eval-probes.json`; production code must not.\n",
        ),
        (
            "quoted-shell.sh",
            "value='scripts/fetch-\\\nholdout-repos.mjs'\n",
        ),
        (
            "unrelated-list.yml",
            "- scripts/fetch-\\\n- holdout-repos.mjs\n",
        ),
        (
            "template-comment.mjs",
            "const value = `${({ clean: true }).clean /* scripts/fetch-holdout-repos.mjs */}`;\n",
        ),
        (
            "quoted-shell-comment.sh",
            "value='clean\\' # scripts/fetch-holdout-repos.mjs\n",
        ),
        (
            "quoted-powershell-comment.ps1",
            "$value = 'clean`' # scripts/fetch-holdout-repos.mjs\n",
        ),
        (
            "quoted-yaml-comment.yml",
            "value: 'clean\\' # scripts/fetch-holdout-repos.mjs\n",
        ),
        (
            "folded-workflow.yml",
            "run: >-\n  node scripts/fetch-\\\n  holdout-repos.mjs\n",
        ),
        (
            "comment-only.yml",
            "# run: node scripts/fetch-\\\n# holdout-repos.mjs\nrun: echo clean\n",
        ),
        (
            "plain-apostrophe.yml",
            "message: don't load it # scripts/fetch-holdout-repos.mjs\n",
        ),
        (
            "punctuated-apostrophe.yml",
            "message: rock-'n roll # scripts/fetch-holdout-repos.mjs\n",
        ),
        (
            "doubled-single-quote.yml",
            "value: 'scripts/fetch-''holdout-repos.mjs'\n",
        ),
    ]);
    let allowed_stderr = String::from_utf8_lossy(&allowed.stderr);
    assert!(
        allowed.status.success(),
        "prose, comments, and unrelated continuations must pass lint; stderr={allowed_stderr}"
    );
}

#[test]
fn linter_binds_policy_allowances_to_the_exact_approved_use() {
    let allowed = run_lint_with_non_rust_fixtures(&[
        (
            ".github/scripts/route-ci-proof.mjs",
            "        \".github/workflows/retrieval-engine-smoke.yml\",\n",
        ),
        (
            ".github/scripts/check-workflow-policy.mjs",
            "const retrievalFile = \"retrieval-engine-smoke.yml\";\n",
        ),
    ]);
    let allowed_stderr = String::from_utf8_lossy(&allowed.stderr);
    assert!(
        allowed.status.success(),
        "the exact policy and routing references must pass lint; stderr={allowed_stderr}"
    );

    let rejected = run_lint_with_non_rust_fixtures(&[
        (
            ".github/scripts/route-ci-proof.mjs",
            "        \".github/workflows/retrieval-engine-smoke.yml\",\n        \".github/workflows/retrieval-engine-smoke.yml\",\nawait import(\".github/workflows/retrieval-engine-smoke.yml\");\n",
        ),
        (
            ".github/scripts/check-workflow-policy.mjs",
            "const retrievalFile = \"retrieval-engine-smoke.yml\";\nconst hostile = \".github/workflows/retrieval-\" + \"engine-smoke.yml\";\n",
        ),
    ]);
    let rejected_stderr = String::from_utf8_lossy(&rejected.stderr).to_ascii_lowercase();
    assert!(
        !rejected.status.success(),
        "an approved file must not hide another harness use; stderr={rejected_stderr}"
    );
    assert!(
        rejected_stderr.contains("route-ci-proof.mjs:3:await import")
            && rejected_stderr.contains("check-workflow-policy.mjs:2:")
            && rejected_stderr.contains("githubworkflowsretrievalenginesmokeyml"),
        "lint must identify both the hostile direct and split uses; stderr={rejected_stderr}"
    );
}

#[test]
fn linter_fails_closed_when_one_prompt_corpus_entry_is_not_a_literal() {
    // The repository keys have to be words this product never writes, or the
    // corpus coverage check fails first and hides the parser drift under a
    // different error.
    let output = run_lint_with_prompt_script_fixture(
        r#"
const PUBLIC_REPOS = {
  alphaprobe: { prompt: "first benchmark prompt remains a static literal for the guard" },
  betaprobe: { prompt: buildPromptAtRuntime() },
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

/// Writes one task manifest into a corpus root of its own. The lint reads extra
/// task roots additively, so the probe never touches the checked-in corpus that
/// every other run -- and every concurrent test -- derives its bans from.
fn probe_task_manifest(symbol: &str) -> String {
    format!(
        r#"{{
  "id": "generalization-lint-probe",
  "version": 1,
  "suite": "public-core",
  "task_class": "architecture_explanation",
  "repo": {{
    "name": "generalization-lint-probe-repo",
    "url": "https://github.com/generalization-probe-owner/generalization-lint-probe.git",
    "ref": "{ref_sha}"
  }},
  "prompt": "Explain how the probe repository moves a request into its own storage layer.",
  "expected_files": ["src/probe/generalization_probe_surface.ts"],
  "expected_symbols": [
    {{ "name": "{symbol}", "path": "src/probe/generalization_probe_surface.ts" }}
  ],
  "expected_claims": [{{ "text": "The probe repository owns its own request path." }}],
  "forbidden_claims": [],
  "quality_thresholds": {{
    "min_expected_anchor_recall": 0.8,
    "min_expected_file_recall": 0.8,
    "min_expected_symbol_recall": 0.8,
    "min_expected_claim_recall": 0.8,
    "min_citation_coverage": 0.8,
    "max_forbidden_claims": 0
  }}
}}
"#,
        ref_sha = "0".repeat(40),
        symbol = symbol,
    )
}

fn run_lint_with_fixture_and_task_root(contents: &str, task_root: Option<&Path>) -> Output {
    let repo_root = workspace_root();
    let script = lint_script(&repo_root);
    let fixture_root = TempDir::new().expect("create fixture root");
    std::fs::write(fixture_root.path().join("fixture.rs"), contents).expect("write fixture");

    let _guard = lint_script_lock();
    let mut command = Command::new("node");
    command.arg(&script).current_dir(&repo_root).env(
        "CODESTORY_RETRIEVAL_GENERALIZATION_SCAN_ROOTS",
        fixture_root.path(),
    );
    if let Some(task_root) = task_root {
        command.env(
            "CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_TASK_ROOTS",
            task_root,
        );
    }
    command.output().expect("run lint with probe task root")
}

#[test]
fn adding_a_benchmark_task_bans_its_symbols_without_editing_the_lint() {
    let fixture = r#"pub const PLANTED: &str = "GeneralizationProbeAnchor";"#;
    let before = run_lint_with_fixture_and_task_root(fixture, None);
    assert!(
        before.status.success(),
        "the probe symbol should be unknown before its task manifest exists, stderr={}",
        String::from_utf8_lossy(&before.stderr)
    );

    let task_root = TempDir::new().expect("create probe task root");
    std::fs::write(
        task_root.path().join("generalization-lint-probe.task.json"),
        probe_task_manifest("GeneralizationProbeAnchor"),
    )
    .expect("write probe manifest");

    let after = run_lint_with_fixture_and_task_root(fixture, Some(task_root.path()));
    let stderr = String::from_utf8_lossy(&after.stderr);
    assert!(
        !after.status.success(),
        "a new task manifest should extend the ban on its own, stderr={stderr}"
    );
    assert!(
        stderr.contains("GeneralizationProbeAnchor"),
        "lint should report the symbol the new manifest introduced, stderr={stderr}"
    );
}

/// One task manifest naming the repository it is about, written into a corpus
/// root of its own. The self-subject rule reads `repo`, so these probes are how
/// a test can ask "would a holdout called `store` be mistaken for us?" without
/// adding a task to the checked-in corpus.
fn self_subject_probe_manifest(repo_name: &str, repo_url: &str, symbol: &str) -> String {
    format!(
        r#"{{
  "id": "{repo_name}-self-subject-probe",
  "version": 1,
  "suite": "public-core",
  "task_class": "architecture_explanation",
  "repo": {{
    "name": "{repo_name}",
    "url": "{repo_url}",
    "ref": "{ref_sha}"
  }},
  "prompt": "Explain how the probe repository handles its own requests end to end.",
  "expected_files": ["src/probe_gadget.rs"],
  "expected_symbols": [
    {{ "name": "{symbol}", "path": "src/probe_gadget.rs", "kind": "function" }}
  ],
  "expected_claims": [],
  "forbidden_claims": []
}}
"#,
        ref_sha = "0".repeat(40),
    )
}

/// The bans the lint derives from the corpus when one extra task manifest is
/// added, separated from the residual literals. The lint writes this itself, so
/// the test reads the same construction the scan uses.
fn derived_patterns_with_extra_task(manifest: &str) -> Vec<String> {
    let repo_root = workspace_root();
    let script = lint_script(&repo_root);
    let probe_root = TempDir::new().expect("create probe root");
    let task_root = probe_root.path().join("tasks");
    let scan_root = probe_root.path().join("src");
    std::fs::create_dir_all(&task_root).expect("create probe task root");
    std::fs::create_dir_all(&scan_root).expect("create probe scan root");
    std::fs::write(task_root.join("probe.task.json"), manifest).expect("write probe manifest");
    std::fs::write(scan_root.join("probe.rs"), "pub fn probe() {}\n").expect("write probe fixture");
    let dump_path = probe_root.path().join("patterns.json");

    let _guard = lint_script_lock();
    let output = Command::new("node")
        .arg(&script)
        .current_dir(&repo_root)
        .env("CODESTORY_RETRIEVAL_GENERALIZATION_SCAN_ROOTS", &scan_root)
        .env(
            "CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_TASK_ROOTS",
            &task_root,
        )
        .env(
            "CODESTORY_RETRIEVAL_GENERALIZATION_DUMP_PATTERNS",
            &dump_path,
        )
        .output()
        .expect("run lint with self-subject probe");
    assert!(
        output.status.success(),
        "lint failed to dump patterns, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dumped = std::fs::read_to_string(&dump_path).expect("read dumped patterns");
    let doc: serde_json::Value = serde_json::from_str(&dumped).expect("parse dumped patterns");
    doc.get("derived")
        .and_then(|derived| derived.as_array())
        .expect("dumped patterns carry a derived list")
        .iter()
        .filter_map(|pattern| pattern.as_str().map(str::to_owned))
        .collect()
}

/// The self-subject rule decides which tasks may not contribute symbol bans.
/// Deciding it on any crate-name token would hand that exemption to a holdout
/// repository called `store`, `runtime`, or `bench` and switch this lint off
/// for it silently. This lives beside the Rust guards on purpose: it is the
/// same contract `scripts/tests/lint-retrieval-generalization.test.mjs` states,
/// and only the Rust suite runs under the workspace test job that has no path
/// filter, so the node test alone can be skipped by a trigger that misses.
#[test]
fn a_holdout_named_after_one_of_our_crates_is_not_mistaken_for_this_repository() {
    for impostor in ["store", "runtime", "bench", "indexer"] {
        let derived = derived_patterns_with_extra_task(&self_subject_probe_manifest(
            impostor,
            &format!("https://github.com/example/{impostor}.git"),
            "probeGadgetHandler",
        ));
        assert!(
            derived
                .iter()
                .any(|pattern| pattern.contains("probeGadgetHandler")),
            "a holdout named `{impostor}` must still ban its own symbols"
        );
    }
}

#[test]
fn a_holdout_cannot_claim_the_exemption_by_calling_itself_this_repository() {
    // #1580. `repo.name` is free text a task author writes, and the exemption's
    // whole effect is that the task contributes no banned markers -- so if the
    // name were honoured, a holdout could switch the lint off for its own
    // corpus while pointing anywhere, and a diff of the lint script would show
    // nothing. Only the URL is evidence of subject.
    // Residual, deliberately not asserted: a repository genuinely *named*
    // `codestory` under another owner would still claim the exemption, because
    // `productRepositoryNames` is derived from crate-name prefixes and carries
    // no owner to compare. Closing that needs an owner pin, which is a
    // different decision; the label-only impostor below needs none.
    for url in [
        "https://github.com/axios/axios.git",
        "https://github.com/BurntSushi/ripgrep.git",
    ] {
        let derived = derived_patterns_with_extra_task(&self_subject_probe_manifest(
            "codestory",
            url,
            "probeGadgetHandler",
        ));
        assert!(
            derived
                .iter()
                .any(|pattern| pattern.contains("probeGadgetHandler")),
            "a holdout at {url} calling itself `codestory` must still ban its own symbols"
        );
    }
}

#[test]
fn this_repositorys_own_name_still_claims_the_self_subject_exemption() {
    let derived = derived_patterns_with_extra_task(&self_subject_probe_manifest(
        "codestory",
        "https://github.com/TheGreenCedar/CodeStory.git",
        "probeGadgetHandler",
    ));
    assert!(
        !derived
            .iter()
            .any(|pattern| pattern.contains("probeGadgetHandler")),
        "a task whose subject is this repository must not ban this repository's symbols"
    );
}

/// Runs the lint over a neutral fixture with extra crate names counted into the
/// workspace, which is how a test can ask what happens when a crate does not
/// follow this repository's `codestory-<layer>` naming convention without
/// creating a crate directory in the working tree.
fn run_lint_with_extra_crate_names(extra_crate_names: &[&str]) -> Output {
    let repo_root = workspace_root();
    let script = lint_script(&repo_root);
    let fixture_root = TempDir::new().expect("create fixture root");
    std::fs::write(
        fixture_root.path().join("fixture.rs"),
        "pub fn repository_neutral_fixture() {}\n",
    )
    .expect("write neutral fixture");

    let _guard = lint_script_lock();
    Command::new("node")
        .arg(&script)
        .current_dir(&repo_root)
        .env(
            "CODESTORY_RETRIEVAL_GENERALIZATION_SCAN_ROOTS",
            fixture_root.path(),
        )
        .env(
            "CODESTORY_RETRIEVAL_GENERALIZATION_EXTRA_CRATE_NAMES",
            extra_crate_names.join(if cfg!(windows) { ";" } else { ":" }),
        )
        .output()
        .expect("run lint with extra crate names")
}

#[test]
fn one_crate_off_the_naming_convention_does_not_switch_the_whole_lint_off() {
    // The repository's own name is derived from its crates so it is not written
    // down anywhere. Requiring every crate to agree makes a single vendored or
    // scratch member -- a change with nothing to do with retrieval -- fail the
    // derivation and exit 2 for the entire repository, which reads to the
    // contributor as an unrelated, unexplained CI failure.
    let output = run_lint_with_extra_crate_names(&["probe-vendor-shim"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "one crate off the convention must not stop the lint, stderr={stderr}"
    );
    assert!(
        !stderr.contains("cannot be derived"),
        "the repository's own name must still be derivable, stderr={stderr}"
    );
}

#[test]
fn a_name_this_workspace_does_not_carry_cannot_claim_the_self_subject_exemption() {
    // The other direction, and the reason the derivation exists: the exemption
    // may never move to a token that does not start a crate that is actually
    // checked in, however many members claim it. Failing closed here is the
    // point -- an undeclared name silently claiming the exemption would switch
    // this lint off for the holdout that shares it.
    let crowded: Vec<&str> = vec![
        "store-a", "store-b", "store-c", "store-d", "store-e", "store-f", "store-g", "store-h",
        "store-i", "store-j", "store-k", "store-l",
    ];
    let output = run_lint_with_extra_crate_names(&crowded);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "an undeclared name claiming the majority must fail closed, stderr={stderr}"
    );
    assert!(
        stderr.contains("cannot be derived"),
        "the refusal must name what it could not derive, stderr={stderr}"
    );
}

/// The repository paths this lint's verdict depends on, read out of the lint
/// itself so the trigger contract below cannot drift from what is guarded.
fn lint_guarded_paths() -> Vec<String> {
    let repo_root = workspace_root();
    let script = lint_script(&repo_root);
    let dump_root = TempDir::new().expect("create guarded-path dump root");
    let dump_path = dump_root.path().join("guarded.json");

    let _guard = lint_script_lock();
    let output = Command::new("node")
        .arg(&script)
        .current_dir(&repo_root)
        .env(
            "CODESTORY_RETRIEVAL_GENERALIZATION_DUMP_GUARDED_PATHS",
            &dump_path,
        )
        .output()
        .expect("run lint guarded-path dump");
    assert!(
        output.status.success(),
        "lint failed to dump its guarded paths, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&dump_path).expect("read guarded paths"))
            .expect("parse guarded paths");
    // Every group the dump carries, read from the document rather than from a
    // list written down here. A hand-picked subset is how the last version of
    // this test passed while the lint guarded 221 non-Rust files that no trigger
    // covered: the test could not see the roots it was not told to look at.
    let groups = doc
        .as_object()
        .expect("guarded-path dump is an object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for required in [
        "productionDirs",
        "productionFiles",
        "corpusDirs",
        "corpusFiles",
        "protectedNonRustDirs",
        "protectedNonRustFiles",
        "lintFiles",
    ] {
        assert!(
            groups.iter().any(|group| group == required),
            "guarded-path dump dropped the {required} surface, got {groups:?}"
        );
    }
    let mut guarded = Vec::new();
    for group in &groups {
        for entry in doc
            .get(group)
            .and_then(|value| value.as_array())
            .unwrap_or_else(|| panic!("guarded-path group {group} is not an array"))
        {
            guarded.push(entry.as_str().expect("guarded path is a string").to_owned());
        }
    }
    assert!(
        guarded.len() >= 40,
        "the lint should report every surface it reads, got {guarded:?}"
    );
    guarded
}

/// The `paths:` list of one workflow trigger. The workflow's trigger filters are
/// plain scalar sequences, so a targeted reader beats adding a YAML dependency
/// to this crate for one assertion.
fn workflow_trigger_paths(workflow: &str, trigger: &str) -> Vec<String> {
    let header = format!("  {trigger}:");
    let mut lines = workflow
        .lines()
        .skip_while(|line| line.trim_end() != header);
    assert!(
        lines.next().is_some(),
        "workflow has no `{trigger}:` trigger"
    );
    let mut paths = Vec::new();
    let mut inside_paths = false;
    for line in lines {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if line.len() - trimmed.len() <= 2 {
            break;
        }
        if trimmed == "paths:" {
            inside_paths = true;
            continue;
        }
        if inside_paths {
            match trimmed.strip_prefix("- ") {
                Some(entry) => paths.push(entry.trim().to_owned()),
                None => inside_paths = false,
            }
        }
    }
    paths
}

fn trigger_filter_covers(filter: &str, guarded: &str) -> bool {
    if filter == guarded {
        return true;
    }
    match filter.strip_suffix("/**") {
        Some(prefix) => guarded == prefix || guarded.starts_with(&format!("{prefix}/")),
        None => false,
    }
}

#[test]
fn retrieval_smoke_workflow_triggers_on_every_path_the_lint_guards() {
    // The generalization gate runs in retrieval-engine-smoke, and that workflow
    // is path-filtered. A filter that omits the guarded production, the corpus
    // the bans are derived from, or the lint itself means a PR that reintroduces
    // steering, edits the lint, or adds a pending excuse never runs the gate --
    // the gate the docs claim, not firing on the code it guards.
    let workflow_path = workspace_root().join(".github/workflows/retrieval-engine-smoke.yml");
    let workflow = std::fs::read_to_string(&workflow_path).expect("read retrieval smoke workflow");
    let guarded = lint_guarded_paths();

    for trigger in ["pull_request", "push"] {
        let filters = workflow_trigger_paths(&workflow, trigger);
        assert!(
            filters.len() > 5,
            "`{trigger}` paths did not parse, got {filters:?}"
        );
        let uncovered: Vec<&String> = guarded
            .iter()
            .filter(|path| {
                !filters
                    .iter()
                    .any(|filter| trigger_filter_covers(filter, path))
            })
            .collect();
        assert!(
            uncovered.is_empty(),
            "retrieval-engine-smoke `{trigger}` never fires on these paths the generalization \
             lint reads: {uncovered:?}"
        );
    }
}

#[test]
fn a_probe_task_root_never_writes_into_the_checked_in_corpus() {
    let corpus = workspace_root().join("benchmarks/tasks");
    let before = corpus_manifest_names(&corpus);
    let task_root = TempDir::new().expect("create probe task root");
    std::fs::write(
        task_root.path().join("generalization-lint-probe.task.json"),
        probe_task_manifest("GeneralizationProbeAnchor"),
    )
    .expect("write probe manifest");
    let output = run_lint_with_fixture_and_task_root(
        r#"pub const PLANTED: &str = "GeneralizationProbeAnchor";"#,
        Some(task_root.path()),
    );
    assert!(
        !output.status.success(),
        "the probe manifest should have extended the ban, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        before,
        corpus_manifest_names(&corpus),
        "the checked-in corpus must be untouched by a lint probe"
    );
}

/// Every banned pattern the lint reported, keyed by the fixture file it named.
/// Asserting only that a fixture's name appears in stderr cannot tell "the ban
/// I planted fired" from "some unrelated ban matched the same file", so a lost
/// ban reads as a covered one. Reading the pattern out of the report closes
/// that gap, and every test that plants a ban uses it.
fn reported_patterns_by_fixture(stderr: &str) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut reported: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for line in stderr.lines() {
        let Some(rest) = [
            "Banned pattern /",
            "Banned literal pattern /",
            "Banned compact benchmark marker /",
        ]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix)) else {
            continue;
        };
        // A pattern can contain `/` itself (`data/indexer`, `lib/axios\.js`), so
        // the header is split at the last `/ in `, not the first slash.
        let Some(split) = rest.rfind("/ in ") else {
            continue;
        };
        let (pattern, tail) = rest.split_at(split);
        let tail = &tail["/ in ".len()..];
        let Some(path_end) = tail.rfind(" (") else {
            continue;
        };
        let file = tail[..path_end]
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_owned();
        reported.entry(file).or_default().push(pattern.to_owned());
    }
    reported
}

/// The corpus text a reported pattern is about, with its regex scaffolding
/// removed: identity bans carry their own boundaries, and every ban escapes its
/// literal. Comparing this against the planted text is what turns "something
/// fired" into "the ban I planted fired".
fn banned_pattern_core(pattern: &str) -> String {
    pattern
        .trim_start_matches("(?:^|[^A-Za-z0-9])")
        .trim_start_matches("(?:^|[^A-Za-z0-9_])")
        .trim_end_matches("(?![A-Za-z0-9])")
        .trim_end_matches("(?![A-Za-z0-9_])")
        .replace('\\', "")
        .to_lowercase()
}

/// True when the lint's report for `fixture` names a ban that is about
/// `planted` rather than about some incidental text in the fixture wrapper.
fn ban_fired_for(
    reported: &std::collections::BTreeMap<String, Vec<String>>,
    fixture: &str,
    planted: &str,
) -> bool {
    let planted = planted.to_lowercase();
    reported.get(fixture).is_some_and(|patterns| {
        patterns
            .iter()
            .any(|pattern| planted.contains(&banned_pattern_core(pattern)))
    })
}

fn corpus_manifest_names(corpus: &Path) -> Vec<String> {
    let mut names = std::fs::read_dir(corpus)
        .expect("read benchmark task corpus")
        .map(|entry| {
            entry
                .expect("corpus entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// Every repository named by a manifest under `benchmarks/tasks/**`, by
/// `repo.name` and by the last segment of `repo.url`.
fn corpus_repository_names() -> Vec<String> {
    let root = workspace_root().join("benchmarks/tasks");
    let mut manifests = Vec::new();
    collect_task_manifests(&root, &mut manifests);
    assert!(
        !manifests.is_empty(),
        "benchmark task corpus has no .task.json manifests at {}",
        root.display()
    );

    let mut names = std::collections::BTreeSet::new();
    for manifest_path in manifests {
        let text = std::fs::read_to_string(&manifest_path).expect("read task manifest");
        let doc: serde_json::Value = serde_json::from_str(&text).expect("parse task manifest");
        let tasks = match doc.get("tasks").and_then(|tasks| tasks.as_array()) {
            Some(tasks) => tasks.clone(),
            None => vec![doc.clone()],
        };
        for task in tasks {
            let Some(repo) = task.get("repo") else {
                continue;
            };
            if let Some(name) = repo.get("name").and_then(|name| name.as_str()) {
                names.insert(name.trim().to_owned());
            }
            if let Some(url) = repo.get("url").and_then(|url| url.as_str()) {
                let slug = url
                    .trim()
                    .trim_end_matches('/')
                    .trim_end_matches(".git")
                    .rsplit('/')
                    .next()
                    .unwrap_or_default();
                if !slug.is_empty() {
                    names.insert(slug.to_owned());
                }
            }
        }
    }
    names.into_iter().filter(|name| !name.is_empty()).collect()
}

fn collect_task_manifests(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_task_manifests(&path, out);
        } else if path.to_string_lossy().ends_with(".task.json") {
            out.push(path);
        }
    }
}

/// Corpus repository names the lint deliberately does not ban, and why. This is
/// a ruling, not an accident: the test below fails if one of these becomes
/// banned (the entry is then stale and must go) and fails if any other corpus
/// repository name is not banned.
const CORPUS_NAMES_RULED_OUT_OF_THE_BAN: &[(&str, &str)] = &[
    (
        "CodeStory",
        "this repository is its own benchmark subject; banning it forbids the product from naming itself",
    ),
    ("codestory", "same subject under its lowercase slug"),
    (
        "express",
        "codestory-indexer/src/framework_routes.rs extracts Express routes as a parser-backed product feature and has to name the framework it parses",
    ),
    (
        "fmt",
        "std::fmt; banning it would forbid every Display and Debug implementation in the tree",
    ),
    (
        "http",
        "the product speaks HTTP in its own adapters and writes the word as an identifier",
    ),
    (
        "requests",
        "plural of a word the product's own request plumbing is built from",
    ),
];

/// Every shape an identifier gives a corpus name it carries as one of its
/// words. `_` is only the most obvious glue. The same steering site is spelled
/// `sourcetrail_index`, `SourcetrailIndex`, `useSourcetrail`,
/// `SOURCETRAIL_BOOST` or `sourcetrail2` depending on the item kind, and a ban
/// anchored on alphanumeric boundaries survives only the spellings whose glue
/// happens to be punctuation -- every other spelling walks past a ban that is
/// nominally in force.
///
/// Tests generate over this list instead of naming examples. Twice now a repair
/// on this lint closed the inputs it was shown (`_` adjacency, then PascalCase
/// concatenation) and left the rest of the same class open; a shape closed here
/// is closed for every token, and a shape someone reopens is reported for every
/// token at once.
fn identifier_word_shapes(token: &str) -> Vec<(&'static str, String)> {
    let lower = token.to_ascii_lowercase();
    let upper = token.to_ascii_uppercase();
    let mut characters = lower.chars();
    let capital = match characters.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), characters.as_str()),
        None => String::new(),
    };
    vec![
        // Punctuation glue: what the boundary already understood.
        ("separator_prefix", format!("boost_{lower}_paths")),
        ("separator_suffix", format!("{lower}_command_boost")),
        ("screaming_separator", format!("{upper}_PATH_BOOST")),
        // Case glue: a word break every reader sees and no `[^A-Za-z0-9]`
        // boundary can.
        ("pascal_type", format!("{capital}Ranker")),
        ("pascal_lead", format!("{capital}IndexBoost")),
        ("pascal_middle", format!("BoostFor{capital}Index")),
        ("camel_tail", format!("boostFor{capital}")),
        ("camel_tail_acronym", format!("boostFor{upper}")),
        ("acronym_then_word", format!("{upper}Index")),
        // Digit glue: the other invisible break.
        ("digit_suffix", format!("{lower}2")),
        ("digit_prefix", format!("rank2{capital}")),
        // A digit followed by a *lowercase* token is the same break as
        // `rank2Swr`, and the lint already reads it -- but until it is
        // enumerated here nothing fails if that stops being true, so the
        // shape could be lost in silence the way the earlier ones were.
        ("digit_then_lower", format!("rank2{lower}")),
    ]
}

/// True when `name` can be spelled as Rust identifier text at all. Hyphenated
/// slugs (`chinook-database`) and dotted file names (`axios.js`) can only be
/// planted as literals, so the identifier shapes do not apply to them.
fn is_identifier_text(name: &str) -> bool {
    name.chars().all(|c| c.is_ascii_alphanumeric())
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
}

/// A shape planted the way a steering site would actually be written: as the
/// declaration its casing implies, and as a table literal, because steering is
/// as often a string in a scoring table as it is a symbol.
fn shape_fixture_source(index: usize, text: &str) -> String {
    let declaration = if text.starts_with(|c: char| c.is_ascii_uppercase()) {
        format!("pub struct {text};\n")
    } else {
        format!("pub fn {text}() -> f32 {{ 1.0 }}\n")
    };
    format!("pub const PLANTED_{index}: &str = \"{text}\";\n{declaration}")
}

#[test]
fn linter_bans_holdout_repository_names_on_identifier_boundaries() {
    let ruled_out: std::collections::BTreeMap<&str, &str> =
        CORPUS_NAMES_RULED_OUT_OF_THE_BAN.iter().copied().collect();
    let names = corpus_repository_names();
    assert!(
        names.len() > 20,
        "expected the corpus to name many repositories, found {names:?}"
    );

    // The bare literal is the easy shape -- it is already delimited by its own
    // quotes, so any boundary at all reports it. Every other shape comes from
    // `identifier_word_shapes`, which enumerates the ways an identifier can
    // carry the name as one of its words rather than listing the ones someone
    // happened to think of. A ban that only survives the literal is lost in
    // practice, and only planting the evading shapes can say so.
    let mut fixtures = Vec::new();
    let mut planted: Vec<(String, String)> = Vec::new();
    let mut shapes_of_name: Vec<Vec<(String, &str)>> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let mut shapes = vec![(
            format!("repo_literal_{index}.rs"),
            format!("pub const PLANTED: &str = \"{name} cache key\";\n"),
            format!("{name} cache key"),
            "bare_literal",
        )];
        if is_identifier_text(name) {
            for (shape, text) in identifier_word_shapes(name) {
                shapes.push((
                    format!("repo_{shape}_{index}.rs"),
                    shape_fixture_source(index, &text),
                    text,
                    shape,
                ));
            }
        }
        let mut per_name = Vec::new();
        for (file_name, contents, text, shape) in shapes {
            fixtures.push((file_name.clone(), contents));
            planted.push((file_name.clone(), text));
            per_name.push((file_name, shape));
        }
        shapes_of_name.push(per_name);
    }
    let borrowed: Vec<(&str, &str)> = fixtures
        .iter()
        .map(|(name, contents)| (name.as_str(), contents.as_str()))
        .collect();
    let output = run_lint_with_named_fixtures(&borrowed);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reported_patterns = reported_patterns_by_fixture(&stderr);
    let planted_by_file: std::collections::BTreeMap<&str, &str> = planted
        .iter()
        .map(|(file, text)| (file.as_str(), text.as_str()))
        .collect();

    let mut unbanned = Vec::new();
    let mut stale_rulings = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let is_ruled_out = ruled_out.contains_key(name.as_str());
        for (fixture, shape) in &shapes_of_name[index] {
            let Some(text) = planted_by_file.get(fixture.as_str()) else {
                continue;
            };
            // The ban has to be about the name we planted, not about some other
            // corpus marker that happened to match the same fixture.
            let reported = ban_fired_for(&reported_patterns, fixture, text);
            match (reported, is_ruled_out) {
                (false, false) => unbanned.push(format!("{name} as {shape} ({text})")),
                (true, true) => stale_rulings.push(format!("{name} as {shape} ({text})")),
                _ => {}
            }
        }
    }
    assert!(
        unbanned.is_empty(),
        "these corpus repository names are not banned in the shape shown and are not ruled out \
         in CORPUS_NAMES_RULED_OUT_OF_THE_BAN: {unbanned:?}"
    );
    assert!(
        stale_rulings.is_empty(),
        "these names are ruled out of the ban but the lint bans them anyway; delete the stale \
         rulings: {stale_rulings:?}"
    );

    // The boundary must stay a boundary. A letter or digit glued to the token
    // makes a different word, and widening the ban to catch `sourcetrail_index`
    // must not also ban `tokio` (`okio`) or `answerswrongly` (`swr`).
    let unrelated = run_lint_with_fixture(
        r#"use tokio::sync::Mutex;

pub const PROSE: &str = "answers welcome";
pub const ADVERB: &str = "answerswrongly";

pub fn held() -> Mutex<u8> {
    Mutex::new(0)
}
"#,
    );
    assert!(
        unrelated.status.success(),
        "a repository name must not match inside ordinary words, stderr={}",
        String::from_utf8_lossy(&unrelated.stderr)
    );
}

#[test]
fn linter_bans_the_audited_injection_symbols_wherever_they_regrow() {
    for symbol in [
        "SourceGroup",
        "BuildIndex",
        "IndexerCommand",
        "EventProcessor",
    ] {
        let output = run_lint_with_fixture(&format!(
            r#"pub fn planted_term() -> &'static str {{ "{symbol}" }}"#
        ));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "the deleted injection symbol `{symbol}` must fail lint wherever it regrows, stderr={stderr}"
        );
    }
}

#[test]
fn linter_leaves_words_this_product_writes_in_its_own_code() {
    let output = run_lint_with_fixture(
        r#"use serde::Serialize;

#[derive(Serialize)]
pub struct SubcommandStorage {
    pub subcommand: String,
    pub storage: String,
}

pub fn serialize_subcommand(value: &SubcommandStorage) -> String {
    serde_json::to_string(value).unwrap_or_default()
}
"#,
    );
    assert!(
        output.status.success(),
        "words the product's own upstream crates write must stay usable, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn linter_does_not_ban_the_hosting_account_a_corpus_lives_under() {
    let output = run_lint_with_fixture(
        r#"//! Licensed under the Apache License, Version 2.0.

pub fn licence_notice() -> &'static str {
    "apache square gorilla pallets"
}
"#,
    );
    assert!(
        output.status.success(),
        "an owner segment names a hosting account, not a corpus, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn linter_rejects_a_word_table_in_term_extraction() {
    let planted = run_lint_with_named_fixtures(&[(
        "search_terms.rs",
        r#"pub const PLANTED_SYMBOL_TERMS: &[&str] = &[
    "indexer",
    "service",
    "storage",
    "store",
    "posts",
    "feed",
    "auth",
    "trail",
];
"#,
    )]);
    let stderr = String::from_utf8_lossy(&planted.stderr);
    assert!(
        !planted.status.success(),
        "a domain word table in term extraction must fail lint, stderr={stderr}"
    );
    assert!(
        stderr.contains("Term vocabulary table"),
        "lint should name the vocabulary table it found, stderr={stderr}"
    );

    let stopwords = run_lint_with_named_fixtures(&[(
        "search_terms.rs",
        r#"pub const SEARCH_PLAN_STOPWORDS: &[&str] = &[
    "and",
    "explain",
    "from",
    "how",
    "into",
    "show",
    "then",
    "with",
];

pub const REASON: &str = "natural_language_filler";
"#,
    )]);
    assert!(
        stopwords.status.success(),
        "the language-level stopword list is not a repository's vocabulary, stderr={}",
        String::from_utf8_lossy(&stopwords.stderr)
    );
}

/// The ban set this lint had before its corpus was derived, as literal text a
/// production file could plausibly contain. Deriving the corpus is only an
/// improvement if it loses nothing, and the only way to know that is to plant
/// the old set and demand a report for every entry. Deleting a line here lowers
/// the floor, so a line may only go when the corpus surface it came from goes.
const PRE_DERIVATION_BAN_FLOOR: &[&str] = &[
    "payload_config",
    "freelancer",
    "traderotate",
    "vscode",
    "codex-rs",
    "sourcetrail",
    "extHostCommands",
    "extensionService",
    "workbench.ts",
    "codex_exec::run",
    "exec_events",
    "StorageAccess",
    "PersistentStorage",
    "SourceGroupCxxCdb",
    "IndexerJava",
    "data/indexer",
    "ExecSharedCliOptions",
    "EventProcessorWithJsonOutput",
    "Subcommand::Exec",
    "ThreadStartParams",
    "TurnStartParams",
    "chinook",
    "mdn",
    "okio",
    "monolog",
    "alamofire",
    "ChinookDatabase",
    "form-validation",
    "commonMain/kotlin/okio",
    "src/Monolog",
    "Source/Core/Session.swift",
    "SocialEntries",
    "ElsewhereFeed",
    "src/lib_cxx",
    "src/lib_java",
    "src/lib/data/storage",
    "getPayloadClient",
    "comment_submission_guard",
    "axios",
    "redis",
    "ripgrep",
    "createInstance",
    "InterceptorManager",
    "dispatchRequest",
    "readQueryFromClient",
    "processCommand",
    "aeMain",
    "aeProcessEvents",
    "HiArgs",
    "SearchWorker",
    "search_parallel",
    "adapters.js",
    "server.c",
    "ae.c",
    "networking.c",
    "core/main.rs",
    "flags/hiargs.rs",
    "haystack.rs",
    "lib/axios.js",
    "lib/core/Axios.js",
    "StringUtils",
    "commons-lang",
    "PreparedRequest",
    "HTTPAdapter",
    "createApplication",
    "app.use",
    "lib/express.js",
    "Jekyll",
    "LogRecord",
    "AbstractProcessingHandler",
    "useSWR",
    "swr",
    "gin.go",
    "RouterGroup.Handle",
    "Engine.addRoute",
    "Engine.handleHTTPRequest",
    "AutoMapper",
    "TypeMapPlanBuilder",
    "RealBufferedSource",
    "RealBufferedSink",
    "DataRequest",
    "SessionDelegate",
    "novalidate",
    "showError",
    "source/animate.css",
    "nvm",
    "install.sh nvm",
    "bash_completion __nvm",
    "--with-holdout-clone",
    "payload_collection",
];

/// The same floor for the compact scan, which rejoins split literals. Each entry
/// is written as a production file would have to write it to evade the line scan.
const PRE_DERIVATION_SPLIT_BAN_FLOOR: &[&str] = &[
    r#""CharSequence", "Utils""#,
    r#""app", ".use""#,
    r#""source/animate", ".css""#,
];

#[test]
fn linter_still_reports_every_ban_it_had_before_the_corpus_was_derived() {
    // The two fixture families must not nest as substrings: a report for
    // `joined-1.rs` must never be mistaken for a report on `floor-1.rs`, or a
    // lost ban reads as a covered one.
    let mut fixtures = Vec::new();
    for (index, planted) in PRE_DERIVATION_BAN_FLOOR.iter().enumerate() {
        fixtures.push((
            format!("floor-{index}.rs"),
            format!("pub fn planted_{index}() -> &'static str {{ \"{planted}\" }}\n"),
        ));
    }
    for (index, planted) in PRE_DERIVATION_SPLIT_BAN_FLOOR.iter().enumerate() {
        fixtures.push((
            format!("joined-{index}.rs"),
            format!("pub fn joined_planted_{index}() -> [&'static str; 2] {{ [{planted}] }}\n"),
        ));
    }
    let borrowed: Vec<(&str, &str)> = fixtures
        .iter()
        .map(|(name, contents)| (name.as_str(), contents.as_str()))
        .collect();
    let output = run_lint_with_named_fixtures(&borrowed);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the pre-derivation ban floor must fail lint, stderr={stderr}"
    );

    // A report is only proof of coverage if the ban it names is about the text
    // we planted; "the fixture appears in stderr" would also be satisfied by an
    // unrelated marker matching the same line.
    let reported = reported_patterns_by_fixture(&stderr);
    let mut lost = Vec::new();
    for (index, planted) in PRE_DERIVATION_BAN_FLOOR.iter().enumerate() {
        if !ban_fired_for(&reported, &format!("floor-{index}.rs"), planted) {
            lost.push(*planted);
        }
    }
    for (index, planted) in PRE_DERIVATION_SPLIT_BAN_FLOOR.iter().enumerate() {
        if !stderr.contains(&format!("joined-{index}.rs")) {
            lost.push(*planted);
        }
    }
    assert!(
        lost.is_empty(),
        "deriving the corpus lost these bans; derive them again or add them to \
         residualBannedLiterals in scripts/lint-retrieval-generalization.mjs: {lost:?}"
    );
}

#[test]
fn linter_still_reports_its_bans_in_every_identifier_word_shape() {
    // The floor above plants each ban alone inside `"..."`, so the quotes
    // already delimit it and any boundary at all reports it. That is not the
    // shape a re-introduced steering site takes. Rust spells the same site
    // `sourcetrail_index`, `SourcetrailIndex`, `useSourcetrail`,
    // `SOURCETRAIL_BOOST` and `sourcetrail2`, and a ban that survives only the
    // punctuation-glued spellings is lost in practice. Planting the whole floor
    // in every shape `identifier_word_shapes` enumerates is the only way the
    // floor can tell "still banned" from "banned only in the shape nobody
    // writes" -- and generating the shapes rather than listing them is what
    // stops the next repair from closing one spelling and leaving its siblings.
    let tokens: Vec<&&str> = PRE_DERIVATION_BAN_FLOOR
        .iter()
        .filter(|planted| is_identifier_text(planted))
        .collect();
    assert!(
        tokens.len() > 30,
        "the floor should have many single-token bans to glue, found {}",
        tokens.len()
    );

    let mut fixtures = Vec::new();
    let mut planted_by_file: std::collections::BTreeMap<String, (String, &str, &str)> =
        std::collections::BTreeMap::new();
    let mut index = 0usize;
    for token in &tokens {
        for (shape, text) in identifier_word_shapes(token) {
            let file_name = format!("shape-{index}.rs");
            fixtures.push((file_name.clone(), shape_fixture_source(index, &text)));
            planted_by_file.insert(file_name, (text, shape, token));
            index += 1;
        }
    }
    assert!(
        fixtures.len() > 300,
        "every floor token should be planted in every shape, got {} fixtures",
        fixtures.len()
    );
    let borrowed: Vec<(&str, &str)> = fixtures
        .iter()
        .map(|(name, contents)| (name.as_str(), contents.as_str()))
        .collect();
    let output = run_lint_with_named_fixtures(&borrowed);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the identifier-shape ban floor must fail lint, stderr={stderr}"
    );

    let reported = reported_patterns_by_fixture(&stderr);
    let mut lost = Vec::new();
    for (file_name, (text, shape, token)) in &planted_by_file {
        if !ban_fired_for(&reported, file_name, text) {
            lost.push(format!("{token} as {shape} ({text})"));
        }
    }
    assert!(
        lost.is_empty(),
        "these bans are lost the moment an identifier glues a word to them, which is how a \
         steering site would actually spell them: {lost:?}"
    );
}

#[test]
fn linter_does_not_ban_a_corpus_name_that_is_only_a_substring_of_one_word() {
    // The other half of the class, and the reason the boundaries exist at all.
    // Making word breaks visible must not turn into substring matching: `tokio`
    // is not `okio`, `answerswrongly` is not `swr`, `plugin` is not `gin`.
    //
    // An unbroken run of one case carries no boundary a reader can see, so it
    // stays unsegmented and stays unbanned -- deliberately, and this test is
    // where that floor is written down. It is generated over the same corpus
    // names the ban test uses, so widening the ban to a new shape has to face
    // both halves of the space at once.
    let names = corpus_repository_names();
    let identifier_names: Vec<&String> = names
        .iter()
        .filter(|name| is_identifier_text(name))
        .collect();
    assert!(
        identifier_names.len() > 10,
        "expected many identifier-shaped corpus names, found {identifier_names:?}"
    );

    let mut fixtures = Vec::new();
    for (index, name) in identifier_names.iter().enumerate() {
        let lower = name.to_ascii_lowercase();
        let upper = name.to_ascii_uppercase();
        // `zz` padding, not a real prefix: the point is an unbroken run, and a
        // meaningful prefix would risk colliding with some other corpus marker
        // and testing the wrong thing.
        fixtures.push((
            format!("substring-lower-{index}.rs"),
            format!("pub const INSIDE_ONE_WORD: &str = \"zz{lower}zz\";\n"),
        ));
        fixtures.push((
            format!("substring-upper-{index}.rs"),
            format!("pub const INSIDE_ONE_WORD: &str = \"ZZ{upper}ZZ\";\n"),
        ));
    }
    // The real-world cases the boundary was introduced for, kept alongside the
    // generated ones because these are the words that actually appear in this
    // tree and a regression here breaks the build rather than a fixture.
    for (index, word) in [
        "tokio",
        "Tokio",
        "TokioRuntime",
        "useTokio",
        "answerswrongly",
        "AnswersWrongly",
        "plugin",
        "PluginHost",
        "pluginHost",
        "PLUGIN_HOST",
        "login",
        "LoginHandler",
        "origin",
        "OriginBoost",
        "ORIGIN_BOOST",
        "invite",
        "InviteToken",
        "demux",
        "DemuxState",
    ]
    .iter()
    .enumerate()
    {
        fixtures.push((
            format!("vocabulary-{index}.rs"),
            format!("pub const PRODUCT_WORD: &str = \"{word}\";\n"),
        ));
    }

    let borrowed: Vec<(&str, &str)> = fixtures
        .iter()
        .map(|(name, contents)| (name.as_str(), contents.as_str()))
        .collect();
    let output = run_lint_with_named_fixtures(&borrowed);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "a corpus name buried inside one unbroken word must not be banned; the boundaries exist \
         so that ordinary product vocabulary keeps compiling, stderr={stderr}"
    );
}
