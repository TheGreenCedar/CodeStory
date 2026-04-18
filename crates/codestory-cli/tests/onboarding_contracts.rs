use std::fs;
use std::path::{Path, PathBuf};

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

#[test]
fn readme_keeps_dual_track_onboarding() {
    let root = repo_root();
    let readme = fs::read_to_string(root.join("README.md")).expect("README should exist");
    assert!(readme.contains("Use CodeStory"));
    assert!(readme.contains("Hack on CodeStory"));
    assert!(readme.contains("docs/architecture/overview.md"));
    assert!(readme.contains("docs/architecture/runtime-execution-path.md"));
    assert!(readme.contains("docs/contributors/debugging.md"));
    assert!(readme.contains("docs/contributors/testing-matrix.md"));

    for path in [
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
    ] {
        assert!(
            root.join(path).exists(),
            "expected onboarding doc to exist: {path}"
        );
    }
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
