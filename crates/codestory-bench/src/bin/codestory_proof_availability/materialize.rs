use super::cli::MaterializeArgs;
use super::contracts::{
    CohortPathFileV1, OraclePathV1, OracleSourceRangeV1, QUALIFICATION_REPOSITORIES,
    canonical_cohort_path_file_sha256, canonical_corpus_sha256, validate_project_file,
};
use super::corpus::LoadedCorpusV1;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SOURCE_ENVIRONMENT_SCHEMA: &str = "codestory.proof-availability-source-environment/v1";
const MAX_DESCRIPTOR_BYTES: usize = 65_536;

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceEnvironmentDescriptorV1 {
    schema: &'static str,
    corpus_sha256: String,
    workspace_root: String,
    repositories: Vec<VerifiedSourceRepositoryV1>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct VerifiedSourceRepositoryV1 {
    repository_id: String,
    repository: String,
    commit: String,
    workspace: String,
    checkout_root: String,
    project_root: String,
    source_tree_sha256: String,
    path_file_sha256: String,
    verified_path_count: usize,
    verified_file_count: usize,
}

#[derive(Debug, Clone)]
struct TreeEntry {
    mode: String,
    kind: String,
}

pub fn verify_only(arguments: &MaterializeArgs, loaded: &LoadedCorpusV1) -> Result<()> {
    verify_only_with_registry(arguments, loaded, &QUALIFICATION_REPOSITORIES, false)
}

fn verify_only_with_registry(
    arguments: &MaterializeArgs,
    loaded: &LoadedCorpusV1,
    registry: &[(&str, &str, &str, &str)],
    allow_local_repositories: bool,
) -> Result<()> {
    if !arguments.verify_only {
        bail!("proof_availability_source_only_requires_verify_only")
    }
    loaded
        .corpus
        .validate_with_path_files_and_registry(&loaded.path_files, registry)?;
    ensure_absent(&arguments.workspace)?;
    ensure_absent(&arguments.out)?;
    if arguments.workspace == arguments.out
        || arguments.workspace.starts_with(&arguments.cache_root)
        || arguments.out.starts_with(&arguments.cache_root)
        || arguments.cache_root.starts_with(&arguments.workspace)
    {
        bail!("proof_availability_materialize_path_overlap")
    }
    let workspace_parent = arguments
        .workspace
        .parent()
        .ok_or_else(|| anyhow::anyhow!("proof_availability_workspace_parent_missing"))?;
    let output_parent = arguments
        .out
        .parent()
        .ok_or_else(|| anyhow::anyhow!("proof_availability_output_parent_missing"))?;
    if !workspace_parent.is_dir() || !output_parent.is_dir() {
        bail!("proof_availability_materialize_parent_missing")
    }

    let staging = tempfile::Builder::new()
        .prefix(".codestory-proof-source-")
        .tempdir_in(workspace_parent)?;
    let staged_workspaces = staging.path().join("workspaces");
    fs::create_dir(&staged_workspaces)?;
    let hooks = staging.path().join("empty-hooks");
    fs::create_dir(&hooks)?;

    let mut repositories = Vec::with_capacity(loaded.path_files.len());
    for path_file in &loaded.path_files {
        let cohort = loaded
            .corpus
            .cohorts
            .iter()
            .find(|cohort| cohort.repository_id == path_file.repository_id)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_materialize_cohort_missing"))?;
        if !allow_local_repositories && !path_file.repository.starts_with("https://") {
            bail!("proof_availability_repository_transport_invalid")
        }
        let checkout = staged_workspaces.join(&path_file.repository_id);
        let (tree_digest, tree) =
            stage_repository(&path_file.repository, &path_file.commit, &checkout, &hooks)?;
        if tree_digest != path_file.source_tree_sha256 || tree_digest != cohort.source_tree_sha256 {
            bail!("proof_availability_source_tree_mismatch")
        }
        let project_root = resolve_workspace(&checkout, &path_file.workspace)?;
        let verified_file_count = verify_oracle_sources(path_file, &project_root, &tree)?;
        require_head_and_clean(&checkout, &path_file.commit, &hooks)?;
        repositories.push(VerifiedSourceRepositoryV1 {
            repository_id: path_file.repository_id.clone(),
            repository: path_file.repository.clone(),
            commit: path_file.commit.clone(),
            workspace: path_file.workspace.clone(),
            checkout_root: arguments
                .workspace
                .join(&path_file.repository_id)
                .display()
                .to_string(),
            project_root: arguments
                .workspace
                .join(&path_file.repository_id)
                .join(workspace_suffix(&path_file.workspace)?)
                .display()
                .to_string(),
            source_tree_sha256: tree_digest,
            path_file_sha256: canonical_cohort_path_file_sha256(path_file)?,
            verified_path_count: path_file.paths.len(),
            verified_file_count,
        });
    }
    repositories.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    let descriptor = SourceEnvironmentDescriptorV1 {
        schema: SOURCE_ENVIRONMENT_SCHEMA,
        corpus_sha256: canonical_corpus_sha256(&loaded.corpus)?,
        workspace_root: arguments.workspace.display().to_string(),
        repositories,
    };
    let bytes = serde_json::to_vec_pretty(&descriptor)?;
    if bytes.len() > MAX_DESCRIPTOR_BYTES {
        bail!("proof_availability_source_descriptor_too_large")
    }
    let mut output = tempfile::NamedTempFile::new_in(output_parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        output
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    output.write_all(&bytes)?;
    output.write_all(b"\n")?;
    output.as_file_mut().sync_all()?;

    fs::rename(&staged_workspaces, &arguments.workspace)?;
    if let Err(error) = output.persist_noclobber(&arguments.out) {
        fs::remove_dir_all(&arguments.workspace).with_context(|| {
            format!(
                "remove newly installed source workspace {} after output failure",
                arguments.workspace.display()
            )
        })?;
        return Err(error.error.into());
    }
    Ok(())
}

fn ensure_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => bail!("proof_availability_output_exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn stage_repository(
    fetch: &str,
    commit: &str,
    checkout: &Path,
    hooks: &Path,
) -> Result<(String, BTreeMap<String, TreeEntry>)> {
    git(
        None,
        hooks,
        [
            "init",
            "--quiet",
            checkout.to_str().context("checkout UTF-8")?,
        ],
    )?;
    git(Some(checkout), hooks, ["remote", "add", "origin", fetch])?;
    git(
        Some(checkout),
        hooks,
        [
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-recurse-submodules",
            "--depth=1",
            "origin",
            commit,
        ],
    )?;
    let raw_tree = git(
        Some(checkout),
        hooks,
        ["ls-tree", "-r", "-z", "--full-tree", commit],
    )?
    .stdout;
    let tree_digest = sha256(&raw_tree);
    let tree = parse_tree(&raw_tree)?;
    git(
        Some(checkout),
        hooks,
        ["checkout", "--quiet", "--detach", commit],
    )?;
    require_head_and_clean(checkout, commit, hooks)?;
    Ok((tree_digest, tree))
}

fn git<const N: usize>(cwd: Option<&Path>, hooks: &Path, args: [&str; N]) -> Result<Output> {
    let mut command = Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", hooks.join("empty-global.gitconfig"))
        .env("GIT_ASKPASS", "")
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.display()))
        .args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().context("execute isolated git command")?;
    if !output.status.success() {
        bail!(
            "proof_availability_git_failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(output)
}

fn require_head_and_clean(checkout: &Path, commit: &str, hooks: &Path) -> Result<()> {
    let head = git(
        Some(checkout),
        hooks,
        ["rev-parse", "--verify", "HEAD^{commit}"],
    )?;
    if String::from_utf8(head.stdout)?.trim() != commit {
        bail!("proof_availability_checkout_head_mismatch")
    }
    let status = git(
        Some(checkout),
        hooks,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    if !status.stdout.is_empty() {
        bail!("proof_availability_checkout_dirty")
    }
    Ok(())
}

fn parse_tree(raw: &[u8]) -> Result<BTreeMap<String, TreeEntry>> {
    let mut tree = BTreeMap::new();
    for record in raw
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| anyhow::anyhow!("proof_availability_git_tree_invalid"))?;
        let header = std::str::from_utf8(&record[..tab])?;
        let path = std::str::from_utf8(&record[tab + 1..])?.to_owned();
        let mut fields = header.split_ascii_whitespace();
        let mode = fields.next().unwrap_or_default().to_owned();
        let kind = fields.next().unwrap_or_default().to_owned();
        if fields.next().is_none() || fields.next().is_some() {
            bail!("proof_availability_git_tree_invalid")
        }
        if path == ".gitmodules"
            || path.ends_with("/.gitmodules")
            || mode == "160000"
            || kind == "commit"
        {
            bail!("proof_availability_submodule_forbidden")
        }
        if tree.insert(path, TreeEntry { mode, kind }).is_some() {
            bail!("proof_availability_git_tree_duplicate")
        }
    }
    Ok(tree)
}

fn resolve_workspace(checkout: &Path, workspace: &str) -> Result<PathBuf> {
    let suffix = workspace_suffix(workspace)?;
    reject_symlink_components(checkout, &suffix)?;
    let root = checkout.join(suffix);
    let canonical_checkout = checkout.canonicalize()?;
    let canonical_root = root.canonicalize()?;
    if !canonical_root.starts_with(&canonical_checkout) || !canonical_root.is_dir() {
        bail!("proof_availability_workspace_escape")
    }
    Ok(canonical_root)
}

fn workspace_suffix(workspace: &str) -> Result<PathBuf> {
    if workspace == "." {
        return Ok(PathBuf::new());
    }
    let components = parse_project_path(workspace)?;
    Ok(components.into_iter().collect())
}

fn parse_project_path(path: &str) -> Result<Vec<String>> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains("//")
        || path.contains('\\')
        || path.contains('\0')
        || path.as_bytes().get(1) == Some(&b':')
    {
        bail!("proof_availability_project_path_invalid")
    }
    let components = path.split('/').map(ToOwned::to_owned).collect::<Vec<_>>();
    validate_project_file(&components)?;
    Ok(components)
}

fn verify_oracle_sources(
    path_file: &CohortPathFileV1,
    project_root: &Path,
    tree: &BTreeMap<String, TreeEntry>,
) -> Result<usize> {
    let mut ranges_by_path = BTreeMap::<String, Vec<&OracleSourceRangeV1>>::new();
    for path in &path_file.paths {
        collect_ranges(path, &mut ranges_by_path);
    }
    let mut source_bytes = BTreeMap::new();
    for (relative, ranges) in &ranges_by_path {
        let components = parse_project_path(relative)?;
        reject_symlink_components(project_root, &components.iter().collect::<PathBuf>())?;
        let source_path = project_root.join(components.iter().collect::<PathBuf>());
        let project_relative = if path_file.workspace == "." {
            relative.clone()
        } else {
            format!("{}/{}", path_file.workspace, relative)
        };
        let entry = tree
            .get(&project_relative)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_source_not_tracked"))?;
        if entry.kind != "blob" || !matches!(entry.mode.as_str(), "100644" | "100755") {
            bail!("proof_availability_source_not_regular")
        }
        let canonical = source_path.canonicalize()?;
        if !canonical.starts_with(project_root) || !canonical.is_file() {
            bail!("proof_availability_source_escape")
        }
        let bytes = fs::read(&canonical)?;
        std::str::from_utf8(&bytes).context("proof_availability_source_invalid_utf8")?;
        for range in ranges {
            validate_source_range(range, &bytes)?;
        }
        source_bytes.insert(relative.clone(), bytes);
    }
    for path in &path_file.paths {
        for step in &path.oracle_steps {
            let bytes = source_bytes
                .get(&step.callsite_expression.path)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_source_buffer_missing"))?;
            validate_line_binding(
                path,
                step.callsite_line,
                &step.callsite_expression,
                &step.receipt_line_window,
                bytes,
            )?;
        }
    }
    Ok(ranges_by_path.len())
}

fn reject_symlink_components(root: &Path, suffix: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    for component in suffix.components() {
        current.push(component);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            bail!("proof_availability_source_symlink_forbidden")
        }
    }
    Ok(())
}

fn collect_ranges<'a>(
    path: &'a OraclePathV1,
    ranges: &mut BTreeMap<String, Vec<&'a OracleSourceRangeV1>>,
) {
    let mut push = |range: &'a OracleSourceRangeV1| {
        ranges.entry(range.path.clone()).or_default().push(range);
    };
    for step in &path.oracle_steps {
        push(&step.caller.range);
        push(&step.callsite_expression);
        push(&step.receipt_line_window);
        push(&step.target.range);
    }
    for mutation in &path.negative_mutations {
        push(&mutation.source_audit.caller.range);
        push(&mutation.source_audit.target.range);
        push(&mutation.source_audit.caller_body);
    }
}

fn validate_source_range(range: &OracleSourceRangeV1, bytes: &[u8]) -> Result<()> {
    if range.file_byte_length != u64::try_from(bytes.len())? {
        bail!("proof_availability_source_length_mismatch")
    }
    let start = usize::try_from(range.start_byte)?;
    let end = usize::try_from(range.end_byte)?;
    if start >= end || end > bytes.len() {
        bail!("proof_availability_source_range_invalid")
    }
    let text = std::str::from_utf8(bytes).context("proof_availability_source_invalid_utf8")?;
    if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
        bail!("proof_availability_source_range_not_utf8")
    }
    if sha256(&bytes[start..end]) != range.sha256 {
        bail!("proof_availability_source_range_hash_mismatch")
    }
    Ok(())
}

fn validate_line_binding(
    _path: &OraclePathV1,
    declared_line: u32,
    expression: &OracleSourceRangeV1,
    line: &OracleSourceRangeV1,
    bytes: &[u8],
) -> Result<()> {
    let start = usize::try_from(expression.start_byte)?;
    let end = usize::try_from(expression.end_byte)?;
    if bytes[start..end].contains(&b'\n') {
        bail!("proof_availability_call_expression_multiline")
    }
    let line_start = bytes[..start]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    let line_end = bytes[end..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |position| end + position + 1);
    let actual_line =
        u32::try_from(bytes[..start].iter().filter(|byte| **byte == b'\n').count() + 1)?;
    if declared_line != actual_line
        || line.start_byte != u64::try_from(line_start)?
        || line.end_byte != u64::try_from(line_end)?
        || expression.start_byte < line.start_byte
        || expression.end_byte > line.end_byte
    {
        bail!("proof_availability_receipt_line_window_mismatch")
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::super::contracts::{
        CORPUS_SCHEMA, CohortV1, CorpusV1, LengthDistributionEntryV1, PATH_FILE_SCHEMA,
    };
    use super::*;
    use serde_json::{Value, json};

    fn run_git(cwd: &Path, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn local_repository() -> (tempfile::TempDir, String, Vec<u8>) {
        let root = tempfile::tempdir().unwrap();
        run_git(root.path(), &["init", "--quiet"]);
        run_git(root.path(), &["config", "user.name", "Fixture"]);
        run_git(
            root.path(),
            &["config", "user.email", "fixture@example.invalid"],
        );
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(
            root.path().join("src/lib.rs"),
            b"pub fn run() { call(); }\n",
        )
        .unwrap();
        run_git(root.path(), &["add", "src/lib.rs"]);
        run_git(root.path(), &["commit", "--quiet", "-m", "fixture"]);
        let commit = String::from_utf8(run_git(root.path(), &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let tree = run_git(
            root.path(),
            &["ls-tree", "-r", "-z", "--full-tree", &commit],
        );
        (root, commit, tree)
    }

    const SOURCE: &[u8] = b"pub fn caller() { target(); }\n";

    fn cohort_repository() -> (tempfile::TempDir, String, Vec<u8>) {
        let root = tempfile::tempdir().unwrap();
        run_git(root.path(), &["init", "--quiet"]);
        run_git(root.path(), &["config", "user.name", "Fixture"]);
        run_git(
            root.path(),
            &["config", "user.email", "fixture@example.invalid"],
        );
        for area in 0..5 {
            let directory = root.path().join(format!("src/area{area}"));
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("file.rs"), SOURCE).unwrap();
        }
        run_git(root.path(), &["add", "src"]);
        run_git(root.path(), &["commit", "--quiet", "-m", "fixture"]);
        let commit = String::from_utf8(run_git(root.path(), &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let tree = run_git(
            root.path(),
            &["ls-tree", "-r", "-z", "--full-tree", &commit],
        );
        (root, commit, tree)
    }

    fn selector(symbol: &str, path: &str) -> Value {
        json!({
            "kind":"qualified_name",
            "qualified_name":symbol,
            "project_file_components":path.split('/').collect::<Vec<_>>()
        })
    }

    fn source_range(path: &str, start: usize, end: usize) -> Value {
        json!({
            "path":path,
            "start_byte":start,
            "end_byte":end,
            "file_byte_length":SOURCE.len(),
            "sha256":sha256(&SOURCE[start..end]),
        })
    }

    fn declaration(symbol: &str, path: &str, start: usize, end: usize) -> Value {
        json!({
            "symbol":symbol,
            "selector":selector(symbol, path),
            "range":source_range(path, start, end),
        })
    }

    fn oracle_path(repository_id: &str, length: usize, ordinal: usize) -> Value {
        let case_id = format!("{repository_id}-case-{ordinal:02}");
        let path = format!("src/area{}/file.rs", ordinal % 5);
        let start_symbol = format!("{case_id}::start");
        let targets = (0..length)
            .map(|step| format!("{case_id}::target_{step}"))
            .collect::<Vec<_>>();
        let steps = targets
            .iter()
            .map(|target| json!({"target":selector(target, &path)}))
            .collect::<Vec<_>>();
        let spec = json!({
            "start":selector(&start_symbol, &path),
            "steps":steps,
            "prohibit_traversal_through":[],
            "exclude_from_projection":[],
        });
        let oracle_steps = targets
            .iter()
            .enumerate()
            .map(|(step, target)| {
                let caller = if step == 0 {
                    start_symbol.clone()
                } else {
                    targets[step - 1].clone()
                };
                json!({
                    "caller":declaration(&caller, &path, 0, 13),
                    "callsite_line":1,
                    "callsite_expression":source_range(&path, 18, 26),
                    "receipt_line_window":source_range(&path, 0, SOURCE.len()),
                    "target":declaration(target, &path, 18, 26),
                })
            })
            .collect::<Vec<_>>();
        let mut fields = vec![json!({"kind":"start"})];
        for step in 0..length {
            for kind in ["step_target", "directness", "ordering", "relation"] {
                fields.push(json!({"kind":kind,"step":step}));
            }
        }
        let absent_target = format!("{case_id}::absent_target");
        let absent_source = format!("{case_id}::absent_source");
        let mut target_spec = spec.clone();
        target_spec["steps"][0]["target"] = selector(&absent_target, &path);
        let mut source_spec = spec.clone();
        source_spec["start"] = selector(&absent_source, &path);
        let source_text = "exact direct ordered call path";
        json!({
            "case_id":case_id,
            "language":"rust",
            "source_text":source_text,
            "clauses":[{
                "clause_id":"material",
                "start_byte":0,
                "end_byte_exclusive":source_text.len(),
                "quote":source_text,
                "classification":{"kind":"resolved_material","fields":fields},
            }],
            "spec":spec,
            "oracle_steps":oracle_steps,
            "negative_mutations":[
                {
                    "mutation_id":format!("{case_id}-target"),
                    "path_id":case_id,
                    "kind":"replace_step_target",
                    "step_index":0,
                    "mutated_spec":target_spec,
                    "source_audit":{
                        "caller":declaration(&start_symbol, &path, 0, 13),
                        "target":declaration(&absent_target, &path, 18, 26),
                        "caller_body":source_range(&path, 0, SOURCE.len()),
                        "finding":"no_direct_call",
                    },
                },
                {
                    "mutation_id":format!("{case_id}-source"),
                    "path_id":case_id,
                    "kind":"replace_step_source",
                    "step_index":0,
                    "mutated_spec":source_spec,
                    "source_audit":{
                        "caller":declaration(&absent_source, &path, 0, 13),
                        "target":declaration(&targets[0], &path, 18, 26),
                        "caller_body":source_range(&path, 0, SOURCE.len()),
                        "finding":"no_direct_call",
                    },
                },
            ],
            "audit":{
                "source_area":format!("area-{}", ordinal % 5),
                "curator":"path-curator@example.invalid",
                "reviewer":"path-reviewer@example.invalid",
                "review_date":"2026-08-21",
            },
        })
    }

    fn local_inputs(
        repository: &str,
        commit: &str,
        tree_sha256: &str,
    ) -> (LoadedCorpusV1, Vec<(String, String, String, String)>) {
        let ids = ["codestory-rust", "vite-ts-js", "flask-python", "gin-go"];
        let mut path_files = Vec::new();
        let mut cohorts = Vec::new();
        let mut registry = Vec::new();
        for id in ids {
            let mut ordinal = 0usize;
            let paths = [10usize, 7, 5, 3, 3, 2]
                .into_iter()
                .enumerate()
                .flat_map(|(index, count)| std::iter::repeat_n(index + 1, count))
                .map(|length| {
                    let path = oracle_path(id, length, ordinal);
                    ordinal += 1;
                    path
                })
                .collect::<Vec<_>>();
            let path_file: CohortPathFileV1 = serde_json::from_value(json!({
                "schema":PATH_FILE_SCHEMA,
                "repository_id":id,
                "repository":repository,
                "commit":commit,
                "workspace":".",
                "source_tree_sha256":tree_sha256,
                "curator":"cohort-curator@example.invalid",
                "reviewer":"cohort-reviewer@example.invalid",
                "review_date":"2026-08-21",
                "source_area_requirement":{"kind":"required_at_least_five"},
                "paths":paths,
            }))
            .unwrap();
            let path_file_sha256 = canonical_cohort_path_file_sha256(&path_file).unwrap();
            cohorts.push(CohortV1 {
                repository_id: id.into(),
                repository: repository.into(),
                commit: commit.into(),
                workspace: ".".into(),
                path_file: format!("paths/{id}.json"),
                path_file_sha256,
                source_tree_sha256: tree_sha256.into(),
                path_count: 30,
                positive_step_count: 78,
                path_length_distribution: [10u8, 7, 5, 3, 3, 2]
                    .into_iter()
                    .enumerate()
                    .map(|(index, count)| LengthDistributionEntryV1 {
                        path_length: (index + 1) as u8,
                        path_count: count,
                    })
                    .collect(),
            });
            registry.push((
                id.to_owned(),
                repository.to_owned(),
                commit.to_owned(),
                ".".to_owned(),
            ));
            path_files.push(path_file);
        }
        let corpus = CorpusV1 {
            schema: CORPUS_SCHEMA.into(),
            corpus_id: "proof-availability-v1".into(),
            thresholds_sha256: "a".repeat(64),
            methodology_sha256: "b".repeat(64),
            curator: "corpus-curator@example.invalid".into(),
            reviewer: "corpus-reviewer@example.invalid".into(),
            review_date: "2026-08-21".into(),
            cohorts,
            positive_request_count: 120,
            positive_step_count: 312,
            negative_request_count: 240,
        };
        (LoadedCorpusV1 { corpus, path_files }, registry)
    }

    #[test]
    fn project_paths_reject_root_escape_and_separator_attacks() {
        for invalid in [
            "",
            "/src/lib.rs",
            "src/",
            "src//lib.rs",
            ".",
            "..",
            "src/../lib.rs",
            "src\\lib.rs",
            "C:/src/lib.rs",
            "src/a:b.rs",
            "~/src/lib.rs",
            "src/\0lib.rs",
        ] {
            assert!(parse_project_path(invalid).is_err(), "{invalid:?}");
        }
        assert_eq!(parse_project_path("src/index").unwrap(), ["src", "index"]);
        assert_ne!(
            parse_project_path("src/index").unwrap(),
            parse_project_path("src/indexer").unwrap()
        );
    }

    #[test]
    fn raw_tree_digest_and_submodule_rejection_are_byte_exact() {
        let raw = b"100644 blob aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\tsrc/lib.rs\0";
        assert_eq!(sha256(raw), format!("{:x}", Sha256::digest(raw)));
        assert_eq!(parse_tree(raw).unwrap()["src/lib.rs"].mode, "100644");
        assert!(
            parse_tree(b"160000 commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\tdep\0").is_err()
        );
        assert!(
            parse_tree(b"100644 blob aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\t.gitmodules\0")
                .is_err()
        );
        assert!(
            parse_tree(
                b"100644 blob aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\tnested/.gitmodules\0"
            )
            .is_err()
        );
    }

    #[test]
    fn local_git_checkout_is_detached_exact_and_dirty_states_fail_closed() {
        let (origin, commit, raw_tree) = local_repository();
        let staging = tempfile::tempdir().unwrap();
        let checkout = staging.path().join("checkout");
        let hooks = staging.path().join("hooks");
        fs::create_dir(&hooks).unwrap();
        let (digest, tree) =
            stage_repository(origin.path().to_str().unwrap(), &commit, &checkout, &hooks).unwrap();
        assert_eq!(digest, sha256(&raw_tree));
        assert_eq!(tree["src/lib.rs"].kind, "blob");
        require_head_and_clean(&checkout, &commit, &hooks).unwrap();
        assert!(require_head_and_clean(&checkout, &"0".repeat(40), &hooks).is_err());
        fs::write(checkout.join("src/lib.rs"), b"dirty\n").unwrap();
        assert!(require_head_and_clean(&checkout, &commit, &hooks).is_err());
        run_git(&checkout, &["checkout", "--quiet", "--", "src/lib.rs"]);
        fs::write(checkout.join("untracked.txt"), b"untracked\n").unwrap();
        assert!(require_head_and_clean(&checkout, &commit, &hooks).is_err());
    }

    #[test]
    fn verify_only_materializes_four_local_sources_atomically_without_cache_or_product_artifacts() {
        let (origin, commit, raw_tree) = cohort_repository();
        let tree_sha256 = sha256(&raw_tree);
        let (loaded, owned_registry) =
            local_inputs(origin.path().to_str().unwrap(), &commit, &tree_sha256);
        let registry = owned_registry
            .iter()
            .map(|(id, repository, commit, workspace)| {
                (
                    id.as_str(),
                    repository.as_str(),
                    commit.as_str(),
                    workspace.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let root = tempfile::tempdir().unwrap();
        let arguments = MaterializeArgs {
            corpus: root.path().join("unused-corpus.json"),
            workspace: root.path().join("workspace"),
            cache_root: root.path().join("cache-must-not-exist"),
            out: root.path().join("source-environment.json"),
            verify_only: true,
        };

        verify_only_with_registry(&arguments, &loaded, &registry, true).unwrap();

        assert!(!arguments.cache_root.exists());
        assert!(arguments.workspace.is_dir());
        let descriptor: Value = serde_json::from_slice(&fs::read(&arguments.out).unwrap()).unwrap();
        assert_eq!(descriptor["schema"], SOURCE_ENVIRONMENT_SCHEMA);
        assert_eq!(descriptor["repositories"].as_array().unwrap().len(), 4);
        assert_eq!(
            descriptor
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["corpus_sha256", "repositories", "schema", "workspace_root"]
        );
        for repository in descriptor["repositories"].as_array().unwrap() {
            assert_eq!(
                repository
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                [
                    "checkout_root",
                    "commit",
                    "path_file_sha256",
                    "project_root",
                    "repository",
                    "repository_id",
                    "source_tree_sha256",
                    "verified_file_count",
                    "verified_path_count",
                    "workspace",
                ]
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&arguments.out).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let failure_root = tempfile::tempdir().unwrap();
        let (mut invalid, owned_registry) =
            local_inputs(origin.path().to_str().unwrap(), &commit, &tree_sha256);
        invalid.path_files[3].source_tree_sha256 = "c".repeat(64);
        invalid.corpus.cohorts[3].source_tree_sha256 = "c".repeat(64);
        invalid.corpus.cohorts[3].path_file_sha256 =
            canonical_cohort_path_file_sha256(&invalid.path_files[3]).unwrap();
        let registry = owned_registry
            .iter()
            .map(|(id, repository, commit, workspace)| {
                (
                    id.as_str(),
                    repository.as_str(),
                    commit.as_str(),
                    workspace.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let failed = MaterializeArgs {
            corpus: failure_root.path().join("unused-corpus.json"),
            workspace: failure_root.path().join("workspace"),
            cache_root: failure_root.path().join("cache-must-not-exist"),
            out: failure_root.path().join("source-environment.json"),
            verify_only: true,
        };
        assert!(verify_only_with_registry(&failed, &invalid, &registry, true).is_err());
        assert!(!failed.workspace.exists());
        assert!(!failed.out.exists());
        assert!(!failed.cache_root.exists());
    }

    #[test]
    fn range_validation_rejects_length_hash_bounds_and_utf8_drift() {
        let bytes = "aéz\n".as_bytes();
        let valid = OracleSourceRangeV1 {
            path: "src/lib.rs".into(),
            start_byte: 1,
            end_byte: 3,
            file_byte_length: bytes.len() as u64,
            sha256: sha256(&bytes[1..3]),
        };
        validate_source_range(&valid, bytes).unwrap();
        let mut invalid = valid.clone();
        invalid.file_byte_length += 1;
        assert!(validate_source_range(&invalid, bytes).is_err());
        let mut invalid = valid.clone();
        invalid.sha256 = "0".repeat(64);
        assert!(validate_source_range(&invalid, bytes).is_err());
        let mut invalid = valid.clone();
        invalid.start_byte = 2;
        assert!(validate_source_range(&invalid, bytes).is_err());
        let mut invalid = valid;
        invalid.end_byte = 99;
        assert!(validate_source_range(&invalid, bytes).is_err());
        let non_utf8 = [0xff, b'\n'];
        let invalid = OracleSourceRangeV1 {
            path: "src/lib.rs".into(),
            start_byte: 0,
            end_byte: 1,
            file_byte_length: 2,
            sha256: sha256(&non_utf8[..1]),
        };
        assert!(validate_source_range(&invalid, &non_utf8).is_err());
    }

    #[test]
    fn existing_and_symlink_outputs_are_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("existing");
        fs::write(&file, b"keep").unwrap();
        assert!(ensure_absent(&file).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = root.path().join("link");
            symlink(&file, &link).unwrap();
            assert!(ensure_absent(&link).is_err());
            let source_root = root.path().join("source-root");
            fs::create_dir(&source_root).unwrap();
            let real = source_root.join("real");
            fs::create_dir(&real).unwrap();
            symlink(&real, source_root.join("linked")).unwrap();
            assert!(reject_symlink_components(&source_root, Path::new("linked/file.rs")).is_err());
        }
        assert!(ensure_absent(&root.path().join("missing")).is_ok());
        assert_eq!(fs::read(file).unwrap(), b"keep");
    }

    #[test]
    fn exact_ranges_and_line_windows_cover_lf_crlf_and_final_lines() {
        for (bytes, expression, line, line_number) in [
            (b"call();\n".as_slice(), (0, 6), (0, 8), 1),
            (b"x\r\ncall();\r\n".as_slice(), (3, 9), (3, 12), 2),
            (b"x\ncall();".as_slice(), (2, 8), (2, 9), 2),
        ] {
            let expression = OracleSourceRangeV1 {
                path: "src/lib.rs".into(),
                start_byte: expression.0,
                end_byte: expression.1,
                file_byte_length: bytes.len() as u64,
                sha256: sha256(&bytes[expression.0 as usize..expression.1 as usize]),
            };
            let window = OracleSourceRangeV1 {
                path: "src/lib.rs".into(),
                start_byte: line.0,
                end_byte: line.1,
                file_byte_length: bytes.len() as u64,
                sha256: sha256(&bytes[line.0 as usize..line.1 as usize]),
            };
            validate_source_range(&expression, bytes).unwrap();
            validate_source_range(&window, bytes).unwrap();
            validate_line_binding_dummy(line_number, &expression, &window, bytes).unwrap();
        }

        let bytes = b"call();\nnext();\n";
        let expression = OracleSourceRangeV1 {
            path: "src/lib.rs".into(),
            start_byte: 0,
            end_byte: 6,
            file_byte_length: bytes.len() as u64,
            sha256: sha256(&bytes[..6]),
        };
        let window = OracleSourceRangeV1 {
            path: "src/lib.rs".into(),
            start_byte: 0,
            end_byte: 8,
            file_byte_length: bytes.len() as u64,
            sha256: sha256(&bytes[..8]),
        };
        assert!(validate_line_binding_dummy(2, &expression, &window, bytes).is_err());
        let mut short_window = window.clone();
        short_window.end_byte -= 1;
        assert!(validate_line_binding_dummy(1, &expression, &short_window, bytes).is_err());
        let mut multiline = expression;
        multiline.end_byte = 10;
        assert!(validate_line_binding_dummy(1, &multiline, &window, bytes).is_err());
    }

    fn validate_line_binding_dummy(
        line: u32,
        expression: &OracleSourceRangeV1,
        window: &OracleSourceRangeV1,
        bytes: &[u8],
    ) -> Result<()> {
        let dummy: OraclePathV1 = serde_json::from_value(serde_json::json!({
            "case_id":"dummy","language":"rust","source_text":"x","clauses":[],
            "spec":{"start":{"kind":"canonical_id","canonical_id":"x"},"steps":[{"target":{"kind":"canonical_id","canonical_id":"y"}}],"prohibit_traversal_through":[],"exclude_from_projection":[]},
            "oracle_steps":[],"negative_mutations":[],"audit":{"source_area":"x","curator":"a","reviewer":"b","review_date":"2026-08-21"}
        }))?;
        validate_line_binding(&dummy, line, expression, window, bytes)
    }

    #[test]
    fn materializer_source_has_no_product_execution_dependency() {
        let source = include_str!("materialize.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        for forbidden in [
            "codestory_runtime",
            "codestory_indexer",
            "codestory_store",
            "IndexService",
            "proof_qualification_support",
            "execute_observed",
            "Store::",
            "Runtime::",
        ] {
            assert!(
                !production.contains(forbidden),
                "forbidden source dependency {forbidden}"
            );
        }
    }
}
