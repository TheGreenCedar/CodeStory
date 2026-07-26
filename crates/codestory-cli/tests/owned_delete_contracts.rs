use std::fs;
use std::path::Path;

mod test_support;

fn run_internal_owned_delete(root: &Path, relative: &str) -> std::process::Output {
    let mut command = test_support::cli_command();
    command
        .arg("internal-owned-delete")
        .arg("--root")
        .arg(root)
        .arg("--relative")
        .arg(relative);
    command.output().expect("run codestory-cli")
}

#[test]
fn internal_owned_delete_removes_targets_inside_the_codestory_cache_root() {
    let root = test_support::test_state_root()
        .join("cache")
        .join("owned-delete-accepts");
    fs::create_dir_all(root.join("generation")).expect("create owned generation");

    let output = run_internal_owned_delete(&root, "generation");

    assert!(
        output.status.success(),
        "owned deletion inside the cache root must succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !root.join("generation").exists(),
        "the owned generation should be removed"
    );
}

#[test]
fn internal_owned_delete_refuses_a_root_outside_the_codestory_cache_root() {
    let outside = test_support::test_state_root()
        .join("not-the-cache")
        .join("owned-delete-refuses");
    fs::create_dir_all(outside.join("generation")).expect("create outside target");

    let output = run_internal_owned_delete(&outside, "generation");

    assert!(
        !output.status.success(),
        "a root the process does not own must be refused\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("owned deletion refused"),
        "refusal should name the ownership boundary, stderr:\n{stderr}"
    );
    assert!(
        outside.join("generation").exists(),
        "a refused deletion must leave the target in place"
    );
}
