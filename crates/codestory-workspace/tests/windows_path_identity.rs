#![cfg(windows)]

use codestory_workspace::{
    same_workspace_path, workspace_file_identity, workspace_path_identity,
    workspace_path_lexical_identity, workspace_relative_path,
};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn native_windows_path_identity_preserves_existing_and_missing_alias_rules() {
    let project = tempfile::tempdir().expect("project");
    let existing = project.path().join("CodeStory");
    fs::create_dir(&existing).expect("mixed-case directory");
    let existing_alias = project.path().join("codestory");

    assert_eq!(
        workspace_path_identity(&existing).expect("existing identity"),
        workspace_path_identity(&existing_alias).expect("existing alias identity")
    );
    assert!(same_workspace_path(&existing, &existing_alias));
    assert_eq!(
        workspace_relative_path(&existing, &existing_alias.join("src/lib.rs")),
        Some(PathBuf::from("src/lib.rs"))
    );

    let identity_file = existing.join("identity.txt");
    fs::write(&identity_file, "identity").expect("identity file");
    let open_file = fs::File::open(&identity_file).expect("open identity file");
    assert_eq!(
        workspace_file_identity(&open_file).expect("open-file identity"),
        workspace_path_identity(&identity_file).expect("path identity")
    );

    let missing = project.path().join("Missing").join(".").join("child");
    let missing_alias = project.path().join("missing").join("CHILD");
    assert_eq!(
        workspace_path_identity(&missing).expect("missing identity"),
        workspace_path_identity(&missing_alias).expect("missing alias identity")
    );
    assert!(same_workspace_path(&missing, &missing_alias));

    let dotted = project
        .path()
        .join("Missing")
        .join(".")
        .join("child")
        .join("..")
        .join("Älias");
    let normalized = project.path().join("missing").join("äLIAS");
    assert!(same_workspace_path(&dotted, &normalized));

    let extended = PathBuf::from(format!(r"\\?\{}", project.path().display()));
    assert_eq!(
        workspace_path_identity(&extended.join("missing")).expect("extended missing identity"),
        workspace_path_identity(&project.path().join("MISSING"))
            .expect("ordinary missing identity")
    );
    assert!(same_workspace_path(
        &extended.join("missing"),
        &project.path().join("MISSING")
    ));
    assert_ne!(
        workspace_path_identity(Path::new(r"C:missing")).expect("drive-relative identity"),
        workspace_path_identity(Path::new(r"C:\missing")).expect("drive-rooted identity")
    );
}

#[test]
fn native_windows_lexical_containment_is_case_insensitive_and_segment_bounded() {
    let root =
        workspace_path_lexical_identity(Path::new(r"C:\Source\CodeStory")).expect("root identity");
    let descendant = workspace_path_lexical_identity(Path::new(r"c:\source\codestory\src\lib.rs"))
        .expect("descendant identity");
    let sibling =
        workspace_path_lexical_identity(Path::new(r"C:\Source\CodeStory-other\src\lib.rs"))
            .expect("sibling identity");

    assert!(descendant.is_within(&root));
    assert!(!sibling.is_within(&root));
}
