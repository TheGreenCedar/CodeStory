use std::path::Path;
use std::process::Command;

pub(crate) fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

pub(crate) fn git(project: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(args)
        .output()
        .expect("run git fixture command");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}
