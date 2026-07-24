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
                let mut verified = self
                    .files
                    .iter()
                    .filter(|file| {
                        let path = resolve_path(self.project_root, &file.path);
                        path.parent() == Some(directory.as_path())
                            && (is_cxx_header_path(path.to_string_lossy().as_ref())
                                || is_cxx_implementation_path(&path))
                    })
                    .filter_map(|file| verified_file(self.storage, self.project_root, file))
                    .collect::<Vec<_>>();
                verified.sort_by(|left, right| left.path.cmp(&right.path));
                verified
            })
    }
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
            .extend(sibling_implementation_targets(
                &hit.display_name,
                header,
                siblings,
            ));
        hit.verification_targets
            .extend(interface_implementation_targets(
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

fn sibling_implementation_targets(
    display_name: &str,
    header: &VerifiedSourceFile,
    siblings: &[VerifiedSourceFile],
) -> Vec<SearchVerificationTargetDto> {
    if !display_name.contains("::") {
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
            let line = line_containing(&candidate.content, display_name)?;
            Some(target(
                "definition",
                candidate,
                line,
                display_name,
                "sibling implementation location for a C/C++ header hit",
            ))
        })
        .collect()
}

fn interface_implementation_targets(
    display_name: &str,
    header: &VerifiedSourceFile,
    siblings: &[VerifiedSourceFile],
) -> Vec<SearchVerificationTargetDto> {
    let Some((interface_name, member_name)) = split_qualified_member(display_name) else {
        return Vec::new();
    };
    if !abstract_header_declares_member(&header.content, interface_name, member_name) {
        return Vec::new();
    }

    let mut targets = Vec::new();
    for implementation_header in siblings
        .iter()
        .filter(|candidate| is_cxx_header_path(candidate.path.to_string_lossy().as_ref()))
    {
        let Some(class_name) = implementation_header
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
        else {
            continue;
        };
        if !header_declares_public_base(&implementation_header.content, class_name, interface_name)
        {
            continue;
        }
        let declaration_line = line_containing(
            &implementation_header.content,
            &format!("class {class_name}"),
        )
        .unwrap_or(1);
        targets.push(target(
            "declaration",
            implementation_header,
            declaration_line,
            class_name,
            "C/C++ implementation class declaration for an abstract interface hit",
        ));

        let definition_pattern = format!("{class_name}::{member_name}");
        if let Some((implementation, definition_line)) = siblings
            .iter()
            .filter(|candidate| {
                candidate.path.file_stem() == implementation_header.path.file_stem()
                    && is_cxx_implementation_path(&candidate.path)
            })
            .find_map(|candidate| {
                line_containing(&candidate.content, &definition_pattern)
                    .map(|line| (candidate, line))
            })
        {
            targets.push(target(
                "definition",
                implementation,
                definition_line,
                &definition_pattern,
                "C/C++ implementation method for an abstract interface hit",
            ));
        }
        if targets.len() >= 4 {
            break;
        }
    }
    targets
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

fn split_qualified_member(display_name: &str) -> Option<(&str, &str)> {
    let (owner, member) = display_name.rsplit_once("::")?;
    let owner = owner.rsplit("::").next()?.trim();
    let member = member
        .split_once('(')
        .map(|(prefix, _)| prefix)
        .unwrap_or(member)
        .trim();
    (!owner.is_empty() && !member.is_empty()).then_some((owner, member))
}

fn abstract_header_declares_member(content: &str, interface_name: &str, member_name: &str) -> bool {
    content.contains(&format!("class {interface_name}"))
        && content.contains(member_name)
        && content.contains("= 0")
}

fn header_declares_public_base(content: &str, class_name: &str, base_name: &str) -> bool {
    content.contains(&format!("class {class_name}"))
        && content.contains(&format!("public {base_name}"))
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
    use super::*;
    use codestory_contracts::api::{NodeId, NodeKind, SearchHitOrigin};
    use codestory_store::FileRole;

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
    fn abstract_interface_targets_come_from_verified_sibling_sources() {
        let project = tempfile::tempdir().expect("project");
        let sources = [
            (
                "src/StorageAccess.h",
                "class StorageAccess {\npublic:\n virtual TextAccess getFileContent() const = 0;\n};\n",
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

        assert!(hit.verification_targets.iter().any(|target| {
            target.file_path.ends_with("src/PersistentStorage.h") && target.role == "declaration"
        }));
        assert!(hit.verification_targets.iter().any(|target| {
            target.file_path.ends_with("src/PersistentStorage.cpp")
                && target.role == "definition"
                && target.line == 3
        }));
        assert!(
            !hit.verification_targets
                .iter()
                .any(|target| target.file_path.ends_with("src/StorageCache.cpp"))
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
