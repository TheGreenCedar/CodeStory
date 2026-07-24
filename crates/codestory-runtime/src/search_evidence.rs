use codestory_contracts::api::{SearchHit, SearchVerificationTargetDto};
use codestory_store::{FileInfo, Store};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

struct VerifiedSourceFile {
    path: PathBuf,
    content: String,
}

struct VerifiedFileIndex<'a> {
    storage: &'a Store,
    project_root: Option<&'a Path>,
    files: Vec<FileInfo>,
    directories: HashMap<PathBuf, Vec<VerifiedSourceFile>>,
}

impl<'a> VerifiedFileIndex<'a> {
    fn load(storage: &'a Store, project_root: Option<&'a Path>) -> Option<Self> {
        Some(Self {
            storage,
            project_root,
            files: storage.get_files().ok()?,
            directories: HashMap::new(),
        })
    }

    fn hit_path(&self, hit_path: &str) -> PathBuf {
        resolve_path(self.project_root, Path::new(hit_path))
    }

    fn directory(&mut self, path: &Path) -> &[VerifiedSourceFile] {
        let directory = path.parent().unwrap_or(path).to_path_buf();
        self.directories
            .entry(directory.clone())
            .or_insert_with(|| {
                verified_files_in_directory(
                    self.storage,
                    self.project_root,
                    &self.files,
                    &directory,
                )
            })
    }
}

fn verified_files_in_directory(
    storage: &Store,
    project_root: Option<&Path>,
    files: &[FileInfo],
    directory: &Path,
) -> Vec<VerifiedSourceFile> {
    let mut verified = files
        .iter()
        .filter(|file| {
            let path = resolve_path(project_root, &file.path);
            path.parent() == Some(directory)
                && (is_cxx_header_path(path.to_string_lossy().as_ref())
                    || is_cxx_implementation_path(&path))
        })
        .filter_map(|file| verified_file(storage, project_root, file))
        .collect::<Vec<_>>();
    verified.sort_by(|left, right| left.path.cmp(&right.path));
    verified
}

pub(super) fn attach_pinned_search_evidence(
    storage: &Store,
    project_root: Option<&Path>,
    hits: &mut [SearchHit],
) {
    let Some(mut files) = VerifiedFileIndex::load(storage, project_root) else {
        return;
    };
    for hit in hits {
        let Some(file_path) = hit.file_path.as_deref() else {
            continue;
        };
        if !is_cxx_header_path(file_path) {
            continue;
        }
        let hit_path = files.hit_path(file_path);
        let siblings = files.directory(&hit_path);
        let Some(header) = siblings.iter().find(|file| file.path == hit_path) else {
            continue;
        };
        hit.verification_targets
            .extend(sibling_source_text_match_targets(
                &hit.display_name,
                header,
                siblings,
            ));
        dedupe_targets(&mut hit.verification_targets);
    }
}

fn verified_file(
    storage: &Store,
    project_root: Option<&Path>,
    file: &FileInfo,
) -> Option<VerifiedSourceFile> {
    let expected_hash = storage.get_file_content_hash(file.id).ok().flatten()?;
    let path = resolve_path(project_root, &file.path);
    let bytes = std::fs::read(&path).ok()?;
    if format!("{:x}", Sha256::digest(&bytes)) != expected_hash {
        return None;
    }
    Some(VerifiedSourceFile {
        path,
        content: String::from_utf8(bytes).ok()?,
    })
}

fn resolve_path(project_root: Option<&Path>, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(project_root) = project_root {
        project_root.join(path)
    } else {
        path.to_path_buf()
    }
}

fn sibling_source_text_match_targets(
    display_name: &str,
    header: &VerifiedSourceFile,
    siblings: &[VerifiedSourceFile],
) -> Vec<SearchVerificationTargetDto> {
    let qualified_name = display_name
        .split_once('(')
        .map_or(display_name, |(name, _)| name)
        .trim();
    if !qualified_name.contains("::") {
        return Vec::new();
    }
    let Some(stem) = header.path.file_stem() else {
        return Vec::new();
    };
    siblings
        .iter()
        .filter(|candidate| {
            candidate.path.file_stem() == Some(stem) && is_cxx_implementation_path(&candidate.path)
        })
        .filter_map(|candidate| {
            let line = line_containing(&candidate.content, qualified_name)?;
            Some(target(
                "source_text_match",
                candidate,
                line,
                display_name,
                "verified same-stem C/C++ source contains the exact qualified-name text; inspect context before assigning a semantic role",
            ))
        })
        .collect()
}

fn target(
    role: &str,
    file: &VerifiedSourceFile,
    line: u32,
    display_name: &str,
    reason: &str,
) -> SearchVerificationTargetDto {
    SearchVerificationTargetDto {
        role: role.to_string(),
        file_path: file.path.to_string_lossy().into_owned(),
        line,
        display_name: display_name.to_string(),
        reason: reason.to_string(),
    }
}

fn dedupe_targets(targets: &mut Vec<SearchVerificationTargetDto>) {
    let mut seen = HashSet::new();
    targets.retain(|target| {
        seen.insert((
            target.role.clone(),
            target.file_path.clone(),
            target.line,
            target.display_name.clone(),
        ))
    });
}

fn line_containing(content: &str, pattern: &str) -> Option<u32> {
    content
        .lines()
        .position(|line| line.contains(pattern))
        .map(|index| (index + 1) as u32)
}

fn is_cxx_header_path(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("h" | "hpp" | "hh" | "hxx")
    )
}

fn is_cxx_implementation_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("cpp" | "cc" | "cxx" | "c")
    )
}

#[cfg(test)]
mod tests {
    use super::{FileInfo, Path, PathBuf, SearchHit, Sha256, Store, attach_pinned_search_evidence};
    use codestory_contracts::api::{NodeId, NodeKind, SearchHitOrigin};
    use codestory_store::FileRole;
    use sha2::Digest;

    #[test]
    fn counterpart_targets_are_bound_to_verified_publication_bytes() {
        let project = tempfile::tempdir().expect("project");
        let header_path = PathBuf::from("src/Project.h");
        let implementation_path = PathBuf::from("src/Project.cpp");
        let header = b"class Project { void buildIndex(); };\n";
        let implementation = b"#include \"Project.h\"\n\nvoid Project::buildIndex() {}\n";
        std::fs::create_dir_all(project.path().join("src")).expect("source directory");
        std::fs::write(project.path().join(&header_path), header).expect("header");
        std::fs::write(project.path().join(&implementation_path), implementation)
            .expect("implementation");

        let storage = Store::new_in_memory().expect("storage");
        insert_file(&storage, 1, &header_path, header);
        insert_file(&storage, 2, &implementation_path, implementation);
        let mut hit = project_hit();
        attach_pinned_search_evidence(
            &storage,
            Some(project.path()),
            std::slice::from_mut(&mut hit),
        );
        assert_eq!(hit.verification_targets.len(), 1);
        assert_eq!(hit.verification_targets[0].line, 3);
        assert_eq!(hit.verification_targets[0].role, "source_text_match");
        assert!(
            !hit.verification_targets[0].reason.contains("definition")
                && !hit.verification_targets[0].reason.contains("declaration"),
            "byte matches must not claim parser-backed semantics"
        );

        std::fs::write(
            project.path().join(&implementation_path),
            "void unrelated() {}\n",
        )
        .expect("mutate implementation");
        assert_eq!(
            hit.verification_targets[0].display_name, "Project::buildIndex",
            "already-produced evidence must not follow newer workspace bytes"
        );

        let mut after_mutation = project_hit();
        attach_pinned_search_evidence(
            &storage,
            Some(project.path()),
            std::slice::from_mut(&mut after_mutation),
        );
        assert!(
            after_mutation.verification_targets.is_empty(),
            "runtime must fail closed when source bytes no longer match the pinned publication"
        );
    }

    #[test]
    fn comments_strings_and_calls_never_receive_semantic_roles() {
        let project = tempfile::tempdir().expect("project");
        let header_path = PathBuf::from("src/Project.h");
        let header = b"class Project { static void buildIndex(); };\n";
        let sources: [(&str, &[u8]); 3] = [
            ("src/Project.cpp", b"// void Project::buildIndex() {}\n"),
            (
                "src/Project.cc",
                b"const char* name = \"Project::buildIndex()\";\n",
            ),
            (
                "src/Project.cxx",
                b"void run() { Project::buildIndex(); }\n",
            ),
        ];
        std::fs::create_dir_all(project.path().join("src")).expect("source directory");
        std::fs::write(project.path().join(&header_path), header).expect("header");

        let storage = Store::new_in_memory().expect("storage");
        insert_file(&storage, 1, &header_path, header);
        for (index, (path, content)) in sources.iter().enumerate() {
            std::fs::write(project.path().join(path), content).expect("source match");
            insert_file(&storage, index as i64 + 2, Path::new(path), content);
        }
        let mut hit = project_hit();
        attach_pinned_search_evidence(
            &storage,
            Some(project.path()),
            std::slice::from_mut(&mut hit),
        );

        assert_eq!(hit.verification_targets.len(), 3);
        assert!(hit.verification_targets.iter().all(|target| {
            target.role == "source_text_match"
                && target.reason.contains("inspect context")
                && !target.reason.contains("definition")
                && !target.reason.contains("declaration")
                && !target.reason.contains("implementation")
        }));
    }

    #[test]
    fn unrelated_pure_virtual_text_cannot_infer_interface_implementations() {
        let project = tempfile::tempdir().expect("project");
        let sources = [
            (
                "src/StorageAccess.h",
                "class StorageAccess {\npublic:\n TextAccess getFileContent() const;\n virtual void unrelated() = 0;\n};\n",
            ),
            (
                "src/PersistentStorage.h",
                "class PersistentStorage\n    : public StorageAccess\n{\n};\n",
            ),
            (
                "src/PersistentStorage.cpp",
                "#include \"PersistentStorage.h\"\n\nTextAccess PersistentStorage::getFileContent() const {}\n",
            ),
            (
                "src/StorageCache.cpp",
                "TextAccess StorageCache::getFileContent() const {}\n",
            ),
        ];
        std::fs::create_dir_all(project.path().join("src")).expect("source directory");
        let storage = Store::new_in_memory().expect("storage");
        for (index, (path, content)) in sources.iter().enumerate() {
            std::fs::write(project.path().join(path), content).expect("source");
            insert_file(
                &storage,
                index as i64 + 1,
                Path::new(path),
                content.as_bytes(),
            );
        }
        let mut hit = SearchHit {
            display_name: "StorageAccess::getFileContent".to_string(),
            file_path: Some("src/StorageAccess.h".to_string()),
            ..project_hit()
        };

        attach_pinned_search_evidence(
            &storage,
            Some(project.path()),
            std::slice::from_mut(&mut hit),
        );

        assert!(
            hit.verification_targets.is_empty(),
            "independent byte fragments must not infer an interface relationship"
        );
    }

    fn insert_file(storage: &Store, id: i64, path: &Path, content: &[u8]) {
        let file = FileInfo {
            id,
            path: path.to_path_buf(),
            language: "cpp".to_string(),
            modification_time: 1,
            indexed: true,
            complete: true,
            line_count: content.iter().filter(|byte| **byte == b'\n').count() as u32,
            file_role: FileRole::Source,
        };
        storage.insert_file(&file).expect("insert file");
        storage
            .update_file_metadata(&file, Some(&format!("{:x}", Sha256::digest(content))))
            .expect("store content identity");
    }

    fn project_hit() -> SearchHit {
        SearchHit {
            node_id: NodeId("project".to_string()),
            display_name: "Project::buildIndex".to_string(),
            kind: NodeKind::METHOD,
            file_path: Some("src/Project.h".to_string()),
            line: Some(1),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            source_excerpt: None,
            verification_targets: Vec::new(),
            score_breakdown: None,
        }
    }
}
