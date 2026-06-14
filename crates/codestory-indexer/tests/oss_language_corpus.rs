use anyhow::{Context, Result, bail};
use codestory_contracts::events::EventBus;
use codestory_indexer::{
    IncrementalIndexingStats, WorkspaceIndexer, language_support_profile_for_ext,
    language_support_profile_for_language_name,
};
use codestory_store::Store as Storage;
use codestory_workspace::{BuildMode, RefreshInfo};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone, Copy)]
struct OssCorpusCase {
    language: &'static str,
    repo_name: &'static str,
    repo_url: &'static str,
    commit: &'static str,
    project_subdir: Option<&'static str>,
    extensions: &'static [&'static str],
    min_baseline_files: usize,
    min_baseline_loc: usize,
    min_indexed_files: usize,
    min_nodes: usize,
    max_errors: usize,
}

#[derive(Debug)]
struct RawBaseline {
    files: Vec<PathBuf>,
    file_count: usize,
    loc: usize,
}

#[derive(Debug)]
struct CorpusReport {
    language: &'static str,
    repo_name: &'static str,
    commit: &'static str,
    raw_files: usize,
    raw_loc: usize,
    codestory_input_files: usize,
    codestory_stored_files: usize,
    codestory_indexed_files: usize,
    nodes: usize,
    edges: usize,
    errors: usize,
    fatal_errors: usize,
    error_samples: Vec<String>,
    checkout_ms: u128,
    baseline_ms: u128,
    index_ms: u128,
    stats: IncrementalIndexingStats,
}

const SUPPORTED_LANGUAGE_NAMES: &[&str] = &[
    "python",
    "java",
    "rust",
    "javascript",
    "typescript",
    "cpp",
    "c",
    "go",
    "ruby",
    "php",
    "csharp",
    "kotlin",
    "swift",
    "dart",
    "bash",
    "html",
    "css",
    "sql",
];

const SKIPPED_DIRS: &[&str] = &[
    ".git",
    ".gradle",
    ".idea",
    ".vscode",
    ".build",
    ".dart_tool",
    ".swiftpm",
    "bin",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "obj",
    "packages",
    "target",
    "tmp",
    "vendor",
];

const OSS_CORPUS: &[OssCorpusCase] = &[
    OssCorpusCase {
        language: "python",
        repo_name: "psf/requests",
        repo_url: "https://github.com/psf/requests.git",
        commit: "6f66281a1d6326b1b9c4ac09ca30de0fc4e6ef43",
        project_subdir: None,
        extensions: &["py", "pyi"],
        min_baseline_files: 20,
        min_baseline_loc: 3_000,
        min_indexed_files: 20,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "java",
        repo_name: "apache/commons-lang",
        repo_url: "https://github.com/apache/commons-lang.git",
        commit: "57f39420fef8413ea42f045f1bdba4864ff75a0c",
        project_subdir: None,
        extensions: &["java"],
        min_baseline_files: 100,
        min_baseline_loc: 20_000,
        min_indexed_files: 100,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "rust",
        repo_name: "BurntSushi/ripgrep",
        repo_url: "https://github.com/BurntSushi/ripgrep.git",
        commit: "82313cf95849bfe425109ad9506a52154879b1b1",
        project_subdir: None,
        extensions: &["rs"],
        min_baseline_files: 100,
        min_baseline_loc: 20_000,
        min_indexed_files: 100,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "javascript",
        repo_name: "expressjs/express",
        repo_url: "https://github.com/expressjs/express.git",
        commit: "dae209ae6559c29cfca2a1f4414c51d89ea643d5",
        project_subdir: None,
        extensions: &["js", "jsx", "mjs", "cjs"],
        min_baseline_files: 50,
        min_baseline_loc: 5_000,
        min_indexed_files: 50,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "typescript",
        repo_name: "vercel/swr",
        repo_url: "https://github.com/vercel/swr.git",
        commit: "f8d4995ac555f02a2784c8fc40bc819782c60568",
        project_subdir: None,
        extensions: &["ts", "tsx", "mts", "cts"],
        min_baseline_files: 100,
        min_baseline_loc: 10_000,
        min_indexed_files: 100,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "cpp",
        repo_name: "fmtlib/fmt",
        repo_url: "https://github.com/fmtlib/fmt.git",
        commit: "e8deaf2ec3b53ced589fce6f640061e5b32eeeaa",
        project_subdir: None,
        extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
        min_baseline_files: 40,
        min_baseline_loc: 10_000,
        min_indexed_files: 40,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "c",
        repo_name: "redis/redis",
        repo_url: "https://github.com/redis/redis.git",
        commit: "df63a65d4d4ee33ae67e9f101885074febe0bccb",
        project_subdir: None,
        extensions: &["c", "h"],
        min_baseline_files: 250,
        min_baseline_loc: 100_000,
        min_indexed_files: 250,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "go",
        repo_name: "gin-gonic/gin",
        repo_url: "https://github.com/gin-gonic/gin.git",
        commit: "d75fcd4c9ab260e5225de590f1f0f8c0e0e12d11",
        project_subdir: None,
        extensions: &["go"],
        min_baseline_files: 80,
        min_baseline_loc: 8_000,
        min_indexed_files: 80,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "ruby",
        repo_name: "jekyll/jekyll",
        repo_url: "https://github.com/jekyll/jekyll.git",
        commit: "202df571314ba1d18e9fccd81d12aaad4a703c38",
        project_subdir: None,
        extensions: &["rb"],
        min_baseline_files: 100,
        min_baseline_loc: 10_000,
        min_indexed_files: 100,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "php",
        repo_name: "Seldaek/monolog",
        repo_url: "https://github.com/Seldaek/monolog.git",
        commit: "04c3499db98d7471abd9261dc83232f8fe1a252d",
        project_subdir: None,
        extensions: &["php"],
        min_baseline_files: 50,
        min_baseline_loc: 5_000,
        min_indexed_files: 50,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "csharp",
        repo_name: "AutoMapper/AutoMapper",
        repo_url: "https://github.com/AutoMapper/AutoMapper.git",
        commit: "b57c206dc7291821e42bdf816a5637a5c1d8cb54",
        project_subdir: None,
        extensions: &["cs"],
        min_baseline_files: 150,
        min_baseline_loc: 15_000,
        min_indexed_files: 150,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "kotlin",
        repo_name: "square/okio",
        repo_url: "https://github.com/square/okio.git",
        commit: "722c8be0043d99b7b08d169b0ae90a24c15267ff",
        project_subdir: None,
        extensions: &["kt", "kts"],
        min_baseline_files: 100,
        min_baseline_loc: 10_000,
        min_indexed_files: 100,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "swift",
        repo_name: "Alamofire/Alamofire",
        repo_url: "https://github.com/Alamofire/Alamofire.git",
        commit: "7595cbcf59809f9977c5f6378500de2ad73b7ddb",
        project_subdir: None,
        extensions: &["swift"],
        min_baseline_files: 50,
        min_baseline_loc: 10_000,
        min_indexed_files: 50,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "dart",
        repo_name: "dart-lang/http",
        repo_url: "https://github.com/dart-lang/http.git",
        commit: "89cec60a4249ae0a0316f7a50d37ac56597f52c3",
        project_subdir: None,
        extensions: &["dart"],
        min_baseline_files: 50,
        min_baseline_loc: 5_000,
        min_indexed_files: 50,
        min_nodes: 25,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "bash",
        repo_name: "nvm-sh/nvm",
        repo_url: "https://github.com/nvm-sh/nvm.git",
        commit: "7079a5d61c2b49c7d35a72006860ce5edb0fac51",
        project_subdir: None,
        extensions: &["sh", "bash"],
        min_baseline_files: 5,
        min_baseline_loc: 3_000,
        min_indexed_files: 5,
        min_nodes: 10,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "html",
        repo_name: "mdn/learning-area",
        repo_url: "https://github.com/mdn/learning-area.git",
        commit: "ca1ff0bd06e12b96a6742ffdf040bb22966e5a5e",
        project_subdir: None,
        extensions: &["html", "htm"],
        min_baseline_files: 300,
        min_baseline_loc: 20_000,
        min_indexed_files: 300,
        min_nodes: 20,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "css",
        repo_name: "animate-css/animate.css",
        repo_url: "https://github.com/animate-css/animate.css.git",
        commit: "3f8ab233dbbd9d2fe577528d2296382954be3d1a",
        project_subdir: None,
        extensions: &["css"],
        min_baseline_files: 20,
        min_baseline_loc: 1_000,
        min_indexed_files: 20,
        min_nodes: 10,
        max_errors: 0,
    },
    OssCorpusCase {
        language: "sql",
        repo_name: "lerocha/chinook-database",
        repo_url: "https://github.com/lerocha/chinook-database.git",
        commit: "7f67772503d71ba90f19283c38e93923addb43fa",
        project_subdir: None,
        extensions: &["sql"],
        min_baseline_files: 10,
        min_baseline_loc: 10_000,
        min_indexed_files: 10,
        min_nodes: 10,
        max_errors: 0,
    },
];

#[test]
#[ignore = "external OSS corpus; set CODESTORY_RUN_OSS_LANGUAGE_CORPUS=1 to clone/index or CODESTORY_OSS_CORPUS_DRY_RUN=1 for manifest validation"]
fn oss_language_corpus_compares_raw_baseline_to_codestory() -> Result<()> {
    validate_manifest()?;

    let dry_run = env_flag("CODESTORY_OSS_CORPUS_DRY_RUN");
    let run_corpus = env_flag("CODESTORY_RUN_OSS_LANGUAGE_CORPUS");
    let selected_languages = selected_languages()?;
    let selected_cases = selected_cases(selected_languages.as_ref())?;

    if dry_run && !run_corpus {
        println!(
            "validated {} OSS corpus manifest entries without cloning or indexing",
            selected_cases.len()
        );
        return Ok(());
    }

    if !run_corpus {
        bail!(
            "set CODESTORY_RUN_OSS_LANGUAGE_CORPUS=1 to clone and index the OSS corpus, \
             or CODESTORY_OSS_CORPUS_DRY_RUN=1 to validate only the manifest"
        );
    }

    let cache_root = corpus_cache_root();
    fs::create_dir_all(&cache_root)
        .with_context(|| format!("creating corpus cache {}", cache_root.display()))?;
    let report_path = target_artifact_root()
        .join("oss-language-corpus")
        .join("reports")
        .join("oss-language-corpus-latest.jsonl");
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating report directory {}", parent.display()))?;
    }
    let mut writer = BufWriter::new(
        File::create(&report_path)
            .with_context(|| format!("creating report {}", report_path.display()))?,
    );

    let mut failures = Vec::new();
    for case in selected_cases {
        match run_case(case, &cache_root) {
            Ok(report) => {
                let row = report_json(&report);
                writeln!(writer, "{row}")?;
                println!(
                    "{}: raw_files={} raw_loc={} codestory_indexed_files={} nodes={} edges={} errors={} index_ms={}",
                    report.language,
                    report.raw_files,
                    report.raw_loc,
                    report.codestory_indexed_files,
                    report.nodes,
                    report.edges,
                    report.errors,
                    report.index_ms
                );
            }
            Err(error) => {
                let row = json!({
                    "language": case.language,
                    "repo_name": case.repo_name,
                    "commit": case.commit,
                    "status": "failed",
                    "error": format!("{error:#}"),
                });
                writeln!(writer, "{row}")?;
                failures.push(format!("{}: {error:#}", case.language));
            }
        }
    }

    writer.flush()?;
    println!(
        "wrote OSS language corpus report to {}",
        report_path.display()
    );

    if !failures.is_empty() {
        bail!("OSS language corpus failures:\n{}", failures.join("\n"));
    }

    Ok(())
}

fn run_case(case: &OssCorpusCase, cache_root: &Path) -> Result<CorpusReport> {
    let checkout_started = Instant::now();
    let checkout_root = ensure_checkout(case, cache_root)?;
    let project_root = if let Some(subdir) = case.project_subdir {
        checkout_root.join(subdir)
    } else {
        checkout_root
    }
    .canonicalize()
    .with_context(|| format!("canonicalizing project root for {}", case.repo_name))?;
    let checkout_ms = checkout_started.elapsed().as_millis();

    let baseline_started = Instant::now();
    let baseline = raw_baseline(&project_root, case.extensions)?;
    let baseline_ms = baseline_started.elapsed().as_millis();
    assert_baseline_thresholds(case, &baseline)?;

    let index_started = Instant::now();
    let mut storage = Storage::new_in_memory()?;
    let event_bus = EventBus::new();
    let refresh_info = RefreshInfo {
        mode: BuildMode::Incremental,
        files_to_index: baseline.files.clone(),
        files_to_remove: Vec::new(),
        existing_file_ids: HashMap::new(),
    };
    let indexer = WorkspaceIndexer::new(project_root);
    let stats = indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;
    let index_ms = index_started.elapsed().as_millis();

    let stored_files = storage.get_files()?;
    let codestory_indexed_files = stored_files.iter().filter(|file| file.indexed).count();
    let nodes = storage.get_nodes()?;
    let edges = storage.get_edges()?;
    let errors = storage.get_errors(None)?;
    let fatal_errors = errors.iter().filter(|error| error.is_fatal).count();
    let error_samples = errors
        .iter()
        .take(10)
        .map(|error| {
            format!(
                "{:?}: fatal={} line={:?} column={:?}: {}",
                error.index_step, error.is_fatal, error.line, error.column, error.message
            )
        })
        .collect();

    let report = CorpusReport {
        language: case.language,
        repo_name: case.repo_name,
        commit: case.commit,
        raw_files: baseline.file_count,
        raw_loc: baseline.loc,
        codestory_input_files: baseline.files.len(),
        codestory_stored_files: stored_files.len(),
        codestory_indexed_files,
        nodes: nodes.len(),
        edges: edges.len(),
        errors: errors.len(),
        fatal_errors,
        error_samples,
        checkout_ms,
        baseline_ms,
        index_ms,
        stats,
    };

    assert_codestory_thresholds(case, &report)?;
    Ok(report)
}

fn validate_manifest() -> Result<()> {
    let expected: BTreeSet<&str> = SUPPORTED_LANGUAGE_NAMES.iter().copied().collect();
    let actual: BTreeSet<&str> = OSS_CORPUS.iter().map(|case| case.language).collect();
    if expected != actual {
        let missing: Vec<&str> = expected.difference(&actual).copied().collect();
        let extra: Vec<&str> = actual.difference(&expected).copied().collect();
        bail!(
            "OSS corpus manifest must match supported language names; missing={missing:?} extra={extra:?}"
        );
    }

    let mut repos = HashSet::new();
    for case in OSS_CORPUS {
        if !repos.insert(case.repo_name) {
            bail!("duplicate OSS corpus repo {}", case.repo_name);
        }
        let profile = language_support_profile_for_language_name(case.language)
            .with_context(|| format!("{} is not a supported language", case.language))?;
        if profile.language_name != case.language {
            bail!(
                "{} profile normalized to unexpected language {}",
                case.language,
                profile.language_name
            );
        }
        for extension in case.extensions {
            let ext_profile = language_support_profile_for_ext(extension).with_context(|| {
                format!(
                    "{} corpus extension .{} is not routed by CodeStory",
                    case.language, extension
                )
            })?;
            if ext_profile.language_name != case.language {
                bail!(
                    "{} corpus extension .{} routes to {}, not {}",
                    case.language,
                    extension,
                    ext_profile.language_name,
                    case.language
                );
            }
        }
    }

    Ok(())
}

fn selected_cases(
    selected_languages: Option<&HashSet<String>>,
) -> Result<Vec<&'static OssCorpusCase>> {
    let cases: Vec<&OssCorpusCase> = OSS_CORPUS
        .iter()
        .filter(|case| {
            selected_languages
                .map(|languages| languages.contains(case.language))
                .unwrap_or(true)
        })
        .collect();
    if cases.is_empty() {
        bail!("CODESTORY_OSS_CORPUS_LANGUAGES did not select any corpus cases");
    }
    Ok(cases)
}

fn selected_languages() -> Result<Option<HashSet<String>>> {
    let value = match env::var("CODESTORY_OSS_CORPUS_LANGUAGES") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => return Ok(None),
    };

    let supported: HashSet<&str> = SUPPORTED_LANGUAGE_NAMES.iter().copied().collect();
    let mut selected = HashSet::new();
    for part in value.split(',') {
        let language = part.trim().to_ascii_lowercase();
        if language.is_empty() {
            continue;
        }
        if !supported.contains(language.as_str()) {
            bail!("unknown CODESTORY_OSS_CORPUS_LANGUAGES entry {language}");
        }
        selected.insert(language);
    }
    Ok(Some(selected))
}

fn assert_baseline_thresholds(case: &OssCorpusCase, baseline: &RawBaseline) -> Result<()> {
    if baseline.file_count < case.min_baseline_files {
        bail!(
            "{} raw baseline found {} files, below threshold {}",
            case.language,
            baseline.file_count,
            case.min_baseline_files
        );
    }
    if baseline.loc < case.min_baseline_loc {
        bail!(
            "{} raw baseline found {} LOC, below threshold {}",
            case.language,
            baseline.loc,
            case.min_baseline_loc
        );
    }
    Ok(())
}

fn assert_codestory_thresholds(case: &OssCorpusCase, report: &CorpusReport) -> Result<()> {
    if report.codestory_input_files != report.raw_files {
        bail!(
            "{} CodeStory input file count {} did not match raw baseline count {}",
            case.language,
            report.codestory_input_files,
            report.raw_files
        );
    }
    if report.codestory_stored_files != report.raw_files {
        bail!(
            "{} CodeStory stored {} files, but raw baseline found {}",
            case.language,
            report.codestory_stored_files,
            report.raw_files
        );
    }
    if report.codestory_indexed_files < case.min_indexed_files {
        bail!(
            "{} CodeStory indexed {} files, below threshold {}",
            case.language,
            report.codestory_indexed_files,
            case.min_indexed_files
        );
    }
    if report.nodes < case.min_nodes {
        bail!(
            "{} CodeStory emitted {} nodes, below threshold {}",
            case.language,
            report.nodes,
            case.min_nodes
        );
    }
    if report.errors > case.max_errors {
        bail!(
            "{} CodeStory emitted {} errors, above threshold {}; samples: {:?}",
            case.language,
            report.errors,
            case.max_errors,
            report.error_samples
        );
    }
    if report.fatal_errors > 0 {
        bail!(
            "{} CodeStory emitted {} fatal errors",
            case.language,
            report.fatal_errors
        );
    }
    Ok(())
}

fn raw_baseline(root: &Path, extensions: &[&str]) -> Result<RawBaseline> {
    let wanted: HashSet<String> = extensions
        .iter()
        .map(|extension| normalize_extension(extension))
        .collect();
    let mut files = Vec::new();
    collect_matching_files(root, &wanted, &mut files)?;
    files.sort();

    let mut loc = 0usize;
    for file in &files {
        let bytes = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        if bytes.is_empty() {
            continue;
        }
        loc += bytes.iter().filter(|byte| **byte == b'\n').count();
        if !bytes.ends_with(b"\n") {
            loc += 1;
        }
    }

    Ok(RawBaseline {
        file_count: files.len(),
        files,
        loc,
    })
}

fn collect_matching_files(
    dir: &Path,
    wanted_extensions: &HashSet<String>,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries =
        fs::read_dir(dir).with_context(|| format!("reading directory {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            collect_matching_files(&path, wanted_extensions, out)?;
        } else if file_type.is_file() && path_has_extension(&path, wanted_extensions) {
            out.push(path);
        }
    }
    Ok(())
}

fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| SKIPPED_DIRS.contains(&name))
        .unwrap_or(false)
}

fn path_has_extension(path: &Path, wanted_extensions: &HashSet<String>) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(normalize_extension)
        .map(|extension| wanted_extensions.contains(&extension))
        .unwrap_or(false)
}

fn ensure_checkout(case: &OssCorpusCase, cache_root: &Path) -> Result<PathBuf> {
    let checkout_root = cache_root.join(sanitize_repo_name(case.repo_name));
    fs::create_dir_all(&checkout_root)
        .with_context(|| format!("creating checkout cache {}", checkout_root.display()))?;

    if !checkout_root.join(".git").is_dir() {
        if fs::read_dir(&checkout_root)?.next().is_some() {
            bail!(
                "cache path {} exists but is not an empty git checkout",
                checkout_root.display()
            );
        }
        run_git(&checkout_root, &["init"])?;
        run_git(&checkout_root, &["remote", "add", "origin", case.repo_url])?;
    } else {
        run_git(
            &checkout_root,
            &["remote", "set-url", "origin", case.repo_url],
        )?;
    }

    run_git(
        &checkout_root,
        &["fetch", "--depth", "1", "origin", case.commit],
    )?;
    run_git(
        &checkout_root,
        &[
            "-c",
            "advice.detachedHead=false",
            "checkout",
            "--detach",
            "FETCH_HEAD",
        ],
    )?;
    let head = git_stdout(&checkout_root, &["rev-parse", "HEAD"])?;
    if head.trim() != case.commit {
        bail!(
            "{} checkout head {} did not match expected {}",
            case.repo_name,
            head.trim(),
            case.commit
        );
    }

    Ok(checkout_root)
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("running git {:?} in {}", args, cwd.display()))?;
    if !output.status.success() {
        bail!(
            "git {:?} failed in {}\nstdout:\n{}\nstderr:\n{}",
            args,
            cwd.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("running git {:?} in {}", args, cwd.display()))?;
    if !output.status.success() {
        bail!(
            "git {:?} failed in {}\nstdout:\n{}\nstderr:\n{}",
            args,
            cwd.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn corpus_cache_root() -> PathBuf {
    env::var_os("CODESTORY_OSS_CORPUS_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            target_artifact_root()
                .join("oss-language-corpus")
                .join("repos")
        })
}

fn target_artifact_root() -> PathBuf {
    if let Some(target_dir) = env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) {
        if target_dir.is_absolute() {
            target_dir
        } else {
            workspace_root().join(target_dir)
        }
    } else {
        workspace_root().join("target")
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn report_json(report: &CorpusReport) -> serde_json::Value {
    let mut timings = BTreeMap::new();
    timings.insert("checkout_ms", report.checkout_ms);
    timings.insert("raw_baseline_ms", report.baseline_ms);
    timings.insert("codestory_index_ms", report.index_ms);

    json!({
        "language": report.language,
        "repo_name": report.repo_name,
        "commit": report.commit,
        "status": "passed",
        "raw_without_codestory": {
            "files": report.raw_files,
            "loc": report.raw_loc,
        },
        "with_codestory": {
            "input_files": report.codestory_input_files,
            "stored_files": report.codestory_stored_files,
            "indexed_files": report.codestory_indexed_files,
            "nodes": report.nodes,
            "edges": report.edges,
            "errors": report.errors,
            "fatal_errors": report.fatal_errors,
            "error_samples": report.error_samples,
        },
        "timings": timings,
        "indexing_stats": {
            "parse_index_ms": report.stats.parse_index_ms,
            "edge_resolution_ms": report.stats.edge_resolution_ms,
            "artifact_cache_hits": report.stats.artifact_cache_hits,
            "artifact_cache_misses": report.stats.artifact_cache_misses,
            "artifact_cache_writes": report.stats.artifact_cache_writes,
            "resolved_calls": report.stats.resolved_calls,
            "resolved_imports": report.stats.resolved_imports,
            "unresolved_calls_end": report.stats.unresolved_calls_end,
            "unresolved_imports_end": report.stats.unresolved_imports_end,
            "resolution_ran": report.stats.resolution_ran,
        },
    })
}

fn normalize_extension(extension: &str) -> String {
    extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
}

fn sanitize_repo_name(repo_name: &str) -> String {
    repo_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| {
            matches!(
                value.trim(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
            )
        })
        .unwrap_or(false)
}
