use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate has workspace parent")
        .parent()
        .expect("workspace root exists")
        .to_path_buf()
}

fn collect_markdown_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read markdown dir") {
        let entry = entry.expect("markdown entry");
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(&path, files);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            files.push(path);
        }
    }
}

fn extract_markdown_links(contents: &str) -> Vec<String> {
    let mut links = Vec::new();
    let bytes = contents.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b']' && bytes[index + 1] == b'(' {
            let mut end = index + 2;
            while end < bytes.len() && bytes[end] != b')' {
                end += 1;
            }
            if end < bytes.len() {
                links.push(contents[index + 2..end].trim().to_string());
                index = end;
            }
        }
        index += 1;
    }
    links
}

fn normalize_local_link_target(raw: &str) -> Option<String> {
    let target = raw.trim().trim_matches(|ch| ch == '<' || ch == '>');
    if target.is_empty()
        || target.starts_with('#')
        || target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with("app://")
        || target.starts_with("plugin://")
    {
        return None;
    }

    Some(
        target
            .split_once('#')
            .map(|(path, _)| path)
            .unwrap_or(target)
            .to_string(),
    )
}

fn assert_public_doc_avoids_agent_specific_framing(file: &Path, contents: &str) {
    let lowered = contents.to_lowercase();
    for blocked in [
        "codegraph",
        "codex-first",
        "codex first",
        "global codex",
        "for codex users",
        ".codex/skills",
        ".codex\\skills",
    ] {
        assert!(
            !lowered.contains(blocked),
            "public doc should not contain `{blocked}`: {}",
            file.display()
        );
    }
}

fn extract_inline_toml_string_array(manifest: &str, key: &str) -> Vec<String> {
    let prefix = format!("{key} = [");
    let line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with(&prefix))
        .unwrap_or_else(|| panic!("manifest should contain inline array `{key}`"));
    let values = line
        .trim()
        .strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or_else(|| panic!("manifest should use inline string array for `{key}`"));

    values
        .split(',')
        .map(|value| value.trim().trim_matches('"').to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

#[test]
fn cli_package_metadata_is_adoption_ready() {
    let root = repo_root();
    let manifest_path = root.join("crates/codestory-cli/Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("CLI manifest should exist");

    for required in [
        "description = \"Local repository evidence and grounding CLI for source-backed coding workflows.\"",
        "license = \"Apache-2.0\"",
        "repository = \"https://github.com/TheGreenCedar/CodeStory.git\"",
        "readme = \"../../README.md\"",
    ] {
        assert!(
            manifest.contains(required),
            "CLI package metadata should include `{required}`"
        );
    }

    let readme_from_manifest = manifest_path
        .parent()
        .expect("CLI manifest should have parent")
        .join("../../README.md");
    assert_eq!(
        fs::canonicalize(readme_from_manifest).expect("manifest readme path should resolve"),
        fs::canonicalize(root.join("README.md")).expect("repo README should resolve"),
        "CLI package readme should point at the repository README"
    );

    let keywords = extract_inline_toml_string_array(&manifest, "keywords");
    assert_eq!(
        keywords,
        vec!["code-search", "grounding", "cli", "agents"],
        "keywords should stay conservative and adoption-oriented"
    );
    assert!(
        keywords.len() <= 5,
        "crates.io accepts at most five package keywords"
    );
    for keyword in keywords {
        assert!(
            keyword.len() <= 20
                && keyword
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-'),
            "keyword should stay crates.io-compatible: {keyword}"
        );
    }

    let categories = extract_inline_toml_string_array(&manifest, "categories");
    assert_eq!(
        categories,
        vec!["command-line-utilities", "development-tools"],
        "categories should stay accurate and crates.io-compatible"
    );
}

#[test]
fn readme_keeps_customer_first_onboarding() {
    let root = repo_root();
    let readme = fs::read_to_string(root.join("README.md")).expect("README should exist");
    assert!(readme.contains("Public Promise"));
    assert!(readme.contains("Try It On A Repo"));
    assert!(readme.contains("What It Builds"));
    assert!(readme.contains("Local codebase grounding for coding agents"));
    assert!(readme.contains("Install As An Agent Skill"));
    assert!(readme.contains("Core Flow"));
    assert!(readme.contains("Hack On CodeStory"));
    assert!(readme.contains("A good CodeStory-backed answer should name"));
    assert!(readme.contains("local evidence layer for repositories"));
    assert!(readme.contains("explicit commands"));
    assert!(readme.contains("source-backed answers"));
    assert!(readme.contains("per-project SQLite cache is separate"));
    assert!(readme.contains("local retrieval sidecars"));
    assert!(readme.contains("does not by itself prove sidecar readiness"));
    assert!(readme.contains("environment- and repository-specific evidence"));
    assert!(readme.contains("instead of promising universal speedups or savings"));
    assert!(readme.contains("benchmark history"));
    assert!(readme.contains("checked with `doctor`"));
    assert!(readme.contains(".agents/skills/codestory-grounding/SKILL.md"));
    assert!(readme.contains("docs/usage.md"));
    assert!(readme.contains("docs/concepts/how-codestory-works.md"));
    assert!(readme.contains("docs/testing/benchmark-results.md"));
    assert!(readme.contains(
        r#""$CODESTORY_CLI" setup embeddings --project "$TARGET_WORKSPACE" --dry-run --format json"#
    ));
    assert!(readme.contains("serve --stdio"));
    assert!(readme.contains("docs/architecture/overview.md"));
    assert!(readme.contains("docs/contributors/debugging.md"));
    assert!(readme.contains("docs/contributors/testing-matrix.md"));
    assert!(
        readme.find("Try It On A Repo").expect("quickstart section")
            < readme.find("Evidence").expect("evidence section"),
        "README should show the usable path before benchmark evidence"
    );

    for path in [
        "docs/usage.md",
        "docs/concepts/how-codestory-works.md",
        "docs/architecture/overview.md",
        "docs/architecture/runtime-execution-path.md",
        "docs/architecture/subsystems/contracts.md",
        "docs/architecture/subsystems/workspace.md",
        "docs/architecture/subsystems/indexer.md",
        "docs/architecture/subsystems/runtime.md",
        "docs/architecture/subsystems/store.md",
        "docs/architecture/subsystems/cli.md",
        "docs/contributors/getting-started.md",
        "docs/contributors/debugging.md",
        "docs/contributors/testing-matrix.md",
        "docs/decision-log.md",
        ".agents/skills/codestory-grounding/scripts/setup.ps1",
        ".agents/skills/codestory-grounding/scripts/setup.sh",
        "scripts/codestory-agent-ab-benchmark.mjs",
    ] {
        assert!(
            root.join(path).exists(),
            "expected onboarding doc to exist: {path}"
        );
    }

    for path in [
        ".agents/skills/codestory-grounding/scripts/setup.ps1",
        ".agents/skills/codestory-grounding/scripts/setup.sh",
    ] {
        let setup = fs::read_to_string(root.join(path)).expect("read setup script");
        assert!(
            !setup.contains("DEFAULT_CODESTORY_REPO_REF"),
            "setup script should not pin a stale default CLI source ref: {path}"
        );
        assert!(
            setup.contains("CODESTORY_REPO_REF"),
            "setup script should keep explicit source-ref override support: {path}"
        );
        assert!(
            setup.contains("origin/HEAD"),
            "setup script should build the remote default branch when no ref is explicit: {path}"
        );
    }
}

#[test]
fn docs_drift_contracts_keep_living_sources_explicit() {
    let root = repo_root();
    let readme = fs::read_to_string(root.join("README.md")).expect("README should exist");
    let usage = fs::read_to_string(root.join("docs/usage.md")).expect("usage doc should exist");
    let testing_matrix = fs::read_to_string(root.join("docs/contributors/testing-matrix.md"))
        .expect("testing matrix should exist");
    let benchmark_scorecard = fs::read_to_string(root.join("docs/testing/benchmark-results.md"))
        .expect("benchmark scorecard should exist");

    assert!(
        readme.contains(
            r#""$CODESTORY_CLI" setup embeddings --project "$TARGET_WORKSPACE" --dry-run --format json"#
        ),
        "README quickstart should show first-run semantic setup dry-run"
    );
    assert!(
        !usage.contains("semantic_doc_scope = \"durable\""),
        "usage config example should omit the default durable semantic scope"
    );
    for accepted_scope in ["`all`", "`full`", "`all-symbols`", "`all_symbols`"] {
        assert!(
            usage.contains(accepted_scope),
            "usage docs should name accepted all-symbol semantic_doc_scope value {accepted_scope}"
        );
    }
    assert!(
        testing_matrix.contains("latest row in")
            && testing_matrix.contains("codestory-e2e-stats-log.md")
            && testing_matrix.contains("historical")
            && testing_matrix.contains("examples only"),
        "testing matrix should point current timing claims at the living stats log"
    );
    assert!(
        !testing_matrix.contains("The 2026-04-18 repo-scale baseline"),
        "testing matrix should not present an old hard-coded baseline as current"
    );
    assert!(
        benchmark_scorecard.contains("[benchmark ledger](benchmark-ledger.md)")
            && benchmark_scorecard.contains("codestory-e2e-stats-log.md"),
        "benchmark scorecard should link detailed history and living timing logs"
    );
    assert!(
        root.join("docs/testing/benchmark-ledger.md").exists(),
        "benchmark ledger should preserve detailed historical rows"
    );
}

#[test]
fn public_docs_avoid_competitor_and_agent_specific_framing() {
    let root = repo_root();
    let mut files = vec![root.join("README.md")];
    collect_markdown_files(&root.join("docs"), &mut files);
    collect_markdown_files(&root.join(".agents/skills/codestory-grounding"), &mut files);

    for file in files {
        let contents = fs::read_to_string(&file).expect("read public doc");
        assert_public_doc_avoids_agent_specific_framing(&file, &contents);
    }
}

#[test]
fn usage_doc_keeps_agent_contract_terms_out_of_operator_flow() {
    let root = repo_root();
    let usage = fs::read_to_string(root.join("docs/usage.md")).expect("usage doc should exist");
    assert!(usage.contains("Common Workflows"));
    assert!(usage.contains("I need a repo overview"));
    assert!(usage.contains("I need evidence for a broad question"));
    assert!(usage.contains("The cache or retrieval looks stale"));
    for blocked in [
        "sufficiency.avoid_opening",
        "supported-claim wording",
        "claim-ledger",
        "Support files",
    ] {
        assert!(
            !usage.contains(blocked),
            "operator usage doc should not expose agent-internal contract term {blocked}"
        );
    }
}

#[test]
fn usage_doc_names_two_readiness_tracks_and_predictable_output_modes() {
    let root = repo_root();
    let usage = fs::read_to_string(root.join("docs/usage.md")).expect("usage doc should exist");

    assert!(usage.contains("## Readiness Tracks"));
    assert!(usage.contains("### Local navigation/cache readiness"));
    assert!(usage.contains("### Agent packet/search sidecar readiness"));
    assert!(usage.contains("`local_navigation`"));
    assert!(usage.contains("`agent_packet_search`"));
    assert!(usage.contains("`retrieval_mode: \"full\"`"));
    assert!(usage.contains("## Predictable Output Modes"));
    assert!(usage.contains("Most commands default to Markdown"));
    assert!(
        usage.contains("Use `--format json` when automation needs the complete structured result")
    );
    assert!(usage.contains("Use `--output-file <PATH>`"));
    assert!(usage.contains("The parent directory must already exist"));
    assert!(usage.contains("`explore` opens the terminal UI by default"));
    assert!(usage.contains("Use `--no-tui`"));
    assert!(
        usage
            .find("## Readiness Tracks")
            .expect("readiness heading")
            < usage
                .find("## Retrieval Defaults")
                .expect("retrieval defaults heading"),
        "usage should introduce readiness tracks before retrieval defaults"
    );
}

#[test]
fn benchmark_docs_show_proof_tier_ladder() {
    let root = repo_root();
    let benchmark_scorecard = fs::read_to_string(root.join("docs/testing/benchmark-results.md"))
        .expect("benchmark scorecard should exist");

    assert!(benchmark_scorecard.contains("## Proof Tier Ladder"));
    for tier in [
        "Stats-only local regression signal",
        "Full sidecar readiness proof",
        "Real-repo drill proof",
        "Promotion-grade benchmark proof",
    ] {
        assert!(
            benchmark_scorecard.contains(tier),
            "benchmark scorecard should explain proof tier {tier}"
        );
    }
    assert!(benchmark_scorecard.contains("Full sidecar readiness, agent packet/search readiness"));
    assert!(benchmark_scorecard.contains("`retrieval_mode: \"full\"`"));
    assert!(benchmark_scorecard.contains("Generalized agent savings"));
    assert!(
        benchmark_scorecard
            .find("## Proof Tier Ladder")
            .expect("proof tier ladder")
            < benchmark_scorecard
                .find("## Promotion Rules")
                .expect("promotion rules"),
        "proof tier ladder should frame promotion rules"
    );
}

#[test]
fn markdown_links_resolve_to_existing_local_files() {
    let root = repo_root();
    let mut markdown_files = vec![root.join("README.md")];
    collect_markdown_files(&root.join("docs"), &mut markdown_files);

    for file in markdown_files {
        let contents = fs::read_to_string(&file).expect("read markdown file");
        for link in extract_markdown_links(&contents) {
            let Some(target) = normalize_local_link_target(&link) else {
                continue;
            };
            let resolved = file.parent().expect("markdown file parent").join(target);
            assert!(
                resolved.exists(),
                "broken markdown link in {} -> {}",
                file.display(),
                resolved.display()
            );
        }
    }
}

#[test]
fn codestory_grounding_skill_command_refs_track_cli_commands() {
    let root = repo_root();
    let skill_root = root.join(".agents/skills/codestory-grounding");
    let commands = [
        "index", "ground", "doctor", "search", "symbol", "trail", "snippet", "query", "explore",
        "bookmark", "context", "packet", "drill", "setup", "serve",
    ];

    for command in commands {
        let reference = skill_root.join("references").join(format!("{command}.md"));
        assert!(
            reference.exists(),
            "codestory-grounding should document `{command}` at {}",
            reference.display()
        );

        let help = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
            .arg(command)
            .arg("--help")
            .output()
            .unwrap_or_else(|error| panic!("run `{command} --help`: {error}"));
        assert!(
            help.status.success(),
            "`{command}` should remain a valid CLI subcommand\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&help.stdout),
            String::from_utf8_lossy(&help.stderr)
        );
    }

    for command in ["context", "bookmark", "doctor", "explore", "serve"] {
        let reference =
            fs::read_to_string(skill_root.join("references").join(format!("{command}.md")))
                .expect("read command reference");
        for required in ["Normal path", "Failure path", "Integration edge"] {
            assert!(
                reference.contains(required),
                "`{command}` reference should include a {required} row"
            );
        }
    }
}
