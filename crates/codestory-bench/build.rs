use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let hooks = out_dir.join("proof-build-empty-hooks");
    fs::create_dir_all(&hooks).expect("create empty proof build hooks directory");
    let global_config = out_dir.join("proof-build-empty-global.gitconfig");
    fs::write(&global_config, []).expect("write empty proof build Git config");
    let repository = Path::new(&env::var("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"))
        .parent()
        .and_then(Path::parent)
        .expect("CodeStory repository root")
        .to_path_buf();
    emit_rerun_inputs(&repository, &hooks, &global_config);
    let source_commit = git(
        &repository,
        &hooks,
        &global_config,
        &["rev-parse", "HEAD^{commit}"],
    );
    let source_tree = git(
        &repository,
        &hooks,
        &global_config,
        &["rev-parse", "HEAD^{tree}"],
    );
    let source_dirty = !git_bytes(
        &repository,
        &hooks,
        &global_config,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .is_empty();
    let rustc = env::var_os("RUSTC").expect("Cargo-selected RUSTC");
    let output = Command::new(&rustc)
        .arg("-vV")
        .output()
        .expect("run Cargo-selected rustc -vV");
    assert!(
        output.status.success(),
        "Cargo-selected rustc -vV failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rustc_vv = String::from_utf8(output.stdout).expect("rustc -vV is UTF-8");
    assert!(!rustc_vv.trim().is_empty(), "rustc -vV is empty");
    let profile = env::var("PROFILE").expect("Cargo PROFILE");
    fs::write(out_dir.join("codestory-proof-rustc-vv.txt"), rustc_vv)
        .expect("write embedded rustc identity");
    fs::write(out_dir.join("codestory-proof-build-profile.txt"), profile)
        .expect("write embedded build profile");
    fs::write(
        out_dir.join("codestory-proof-source-commit.txt"),
        source_commit,
    )
    .expect("write embedded source commit");
    fs::write(out_dir.join("codestory-proof-source-tree.txt"), source_tree)
        .expect("write embedded source tree");
    fs::write(
        out_dir.join("codestory-proof-source-dirty.txt"),
        if source_dirty { "true" } else { "false" },
    )
    .expect("write embedded source dirtiness");
}

fn git(repository: &Path, hooks: &Path, global_config: &Path, arguments: &[&str]) -> String {
    String::from_utf8(git_bytes(repository, hooks, global_config, arguments))
        .expect("Git identity is UTF-8")
        .trim()
        .to_owned()
}

fn git_bytes(repository: &Path, hooks: &Path, global_config: &Path, arguments: &[&str]) -> Vec<u8> {
    let output = git_command(repository, hooks, global_config)
        .args(arguments)
        .output()
        .expect("run Git for proof build provenance");
    assert!(
        output.status.success(),
        "Git build provenance command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn emit_rerun_inputs(repository: &Path, hooks: &Path, global_config: &Path) {
    let git_dir = PathBuf::from(git(
        repository,
        hooks,
        global_config,
        &["rev-parse", "--absolute-git-dir"],
    ));
    let common = PathBuf::from(git(
        repository,
        hooks,
        global_config,
        &["rev-parse", "--git-common-dir"],
    ));
    let common_dir = if common.is_absolute() {
        common
    } else {
        repository.join(common)
    };
    let mut paths = BTreeSet::from([
        git_dir.join("HEAD"),
        git_dir.join("index"),
        common_dir.join("packed-refs"),
    ]);
    let worktree_git_link = repository.join(".git");
    if fs::symlink_metadata(&worktree_git_link).is_ok_and(|metadata| metadata.is_file()) {
        paths.insert(worktree_git_link);
    }
    let symbolic = git_command(repository, hooks, global_config)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .expect("resolve Git symbolic HEAD");
    if symbolic.status.success() {
        let reference = String::from_utf8(symbolic.stdout)
            .expect("Git reference is UTF-8")
            .trim()
            .to_owned();
        paths.insert(common_dir.join(reference));
    }
    for tracked in git_bytes(repository, hooks, global_config, &["ls-files", "-z"])
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let tracked = String::from_utf8(tracked.to_vec()).expect("tracked path is UTF-8");
        let top_level = tracked.split('/').next().expect("tracked path component");
        paths.insert(repository.join(top_level));
    }
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn git_command(repository: &Path, hooks: &Path, global_config: &Path) -> Command {
    let mut command = Command::new("git");
    for (name, _) in env::vars_os() {
        if name
            .to_string_lossy()
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("GIT_"))
        {
            command.env_remove(name);
        }
    }
    command
        .current_dir(repository)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", global_config)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.display()));
    command
}
