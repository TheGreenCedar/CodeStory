use crate::support::clamp_i64_to_u32;
use crate::{AppController, path_resolution};
use codestory_contracts::api::{
    ApiError, ReadFileTextRequest, ReadFileTextResponse, WriteFileResponse, WriteFileTextRequest,
};
use std::path::PathBuf;

/// Source bytes for a symbol-identified node, verified against the pinned
/// core publication. `content` is the single buffer every renderer must use:
/// the path was checked for project containment, read once, and hashed against
/// the indexed SHA-256, so no caller may reopen `path` afterwards.
pub(crate) struct VerifiedNodeSource {
    pub(crate) file_id: i64,
    pub(crate) start_line: u32,
    pub(crate) end_line: Option<u32>,
    pub(crate) path: PathBuf,
    pub(crate) content: String,
    pub(crate) content_sha256: String,
    /// Publication the verified bytes were bound to. `None` only for an
    /// unpinned caller whose storage declares no complete publication (a state
    /// unreachable through the public operation wrappers, which pin one).
    pub(crate) generation_id: Option<String>,
    pub(crate) run_id: Option<String>,
    pub(crate) project_id: String,
}

/// A file node names its own indexed file; other nodes require an explicit
/// association. Never infer a file from an orphan symbol's numeric identity.
pub(crate) fn source_file_id(
    node: &codestory_contracts::graph::Node,
) -> Option<codestory_contracts::graph::NodeId> {
    if node.kind == codestory_contracts::graph::NodeKind::FILE {
        Some(node.id)
    } else {
        node.file_node_id
    }
}

impl AppController {
    /// Resolve a public node identity to source bytes that provably match the
    /// indexed content hash under the currently pinned core publication.
    /// Returns `source_stale` when the file exists but no longer matches the
    /// indexed bytes, and `source_unavailable` for every other failure.
    pub(crate) fn verified_node_source(
        &self,
        node: &codestory_contracts::api::NodeDetailsDto,
    ) -> Result<VerifiedNodeSource, ApiError> {
        let unavailable = || {
            ApiError::new(
                "source_unavailable",
                "Focused source could not be bound to the pinned file.",
            )
        };
        let project_root = self.require_project_root()?;
        let storage = self.open_storage_read_only()?;
        // Public operations pin one complete publication for their whole read;
        // an unpinned caller binds to the complete publication the same
        // storage handle declares, which is the pin's own record under a pin.
        let publication = self
            .active_core_publication()
            .or_else(|| storage.get_complete_index_publication().ok().flatten());
        let id = node.id.0.parse::<i64>().map_err(|_| unavailable())?;
        let stored_node = storage
            .get_node(codestory_contracts::graph::NodeId(id))
            .ok()
            .flatten()
            .ok_or_else(unavailable)?;
        let file_id = source_file_id(&stored_node).ok_or_else(unavailable)?.0;
        let file = storage
            .get_file_by_id(file_id)
            .ok()
            .flatten()
            .ok_or_else(unavailable)?;
        let line = stored_node
            .start_line
            .filter(|line| *line > 0)
            .ok_or_else(unavailable)?;
        if node.start_line != Some(line) {
            return Err(unavailable());
        }
        let source = crate::search_evidence::verified_file_checked(
            &storage,
            Some(&project_root),
            &file,
        )
        .map_err(|error| match error {
            crate::search_evidence::VerifiedFileError::Stale => ApiError::new(
                "source_stale",
                format!(
                    "Indexed source for {} changed on disk after publication; refresh the index before reading symbol source.",
                    file.path.display()
                ),
            ),
            crate::search_evidence::VerifiedFileError::Unavailable => unavailable(),
        })?;
        Ok(VerifiedNodeSource {
            file_id,
            start_line: line,
            end_line: stored_node.end_line,
            path: source.path,
            content: source.content,
            content_sha256: source.content_sha256,
            generation_id: publication
                .as_ref()
                .map(|publication| publication.generation_id.clone()),
            run_id: publication
                .as_ref()
                .map(|publication| publication.run_id.clone()),
            project_id: codestory_workspace::project_identity_v3(&project_root).project_id,
        })
    }

    pub(crate) fn focused_source_context(
        &self,
        node: &codestory_contracts::api::NodeDetailsDto,
        maximum_bytes: usize,
        truncation_suffix: &str,
    ) -> Result<crate::FocusedSourceContext, ApiError> {
        let unavailable = || {
            ApiError::new(
                "source_unavailable",
                "Focused source could not be bound to the pinned file.",
            )
        };
        let source = self.verified_node_source(node)?;
        if source.generation_id.is_none() || source.run_id.is_none() {
            return Err(unavailable());
        }
        let line = source.start_line;
        let window =
            crate::snippets::bounded_source_window(&source.content, line, 6, maximum_bytes)
                .ok_or_else(unavailable)?;
        let rendered = crate::snippets::bounded_markdown_snippet_from_text(
            &source.content,
            line,
            6,
            maximum_bytes,
            truncation_suffix,
        )
        .map_err(|_| unavailable())?;
        let path = source.path.to_string_lossy().into_owned();
        Ok(crate::FocusedSourceContext {
            path: path.clone(),
            line,
            snippet: rendered.markdown,
            truncated: rendered.truncated,
            evidence: codestory_contracts::api::FocusedSourceEvidenceDto {
                node_id: node.id.clone(),
                file_id: source.file_id,
                path,
                project_id: source.project_id.clone(),
                core_generation_id: source.generation_id.clone().unwrap_or_default(),
                core_run_id: source.run_id.clone().unwrap_or_default(),
                content_sha256: source.content_sha256.clone(),
                start_line: window.start_line,
                end_line: window.end_line,
                excerpt: window.text,
                truncated: window.truncated,
            },
        })
    }

    pub(crate) fn focused_source_matches_current(
        &self,
        source: &codestory_contracts::api::FocusedSourceEvidenceDto,
        target_path: Option<&str>,
    ) -> bool {
        let Some(publication) = self.active_core_publication() else {
            return false;
        };
        let Ok(project_root) = self.require_project_root() else {
            return false;
        };
        if publication.generation_id != source.core_generation_id
            || publication.run_id != source.core_run_id
            || codestory_workspace::project_identity_v3(&project_root).project_id
                != source.project_id
        {
            return false;
        }
        let Ok(storage) = self.open_storage_read_only() else {
            return false;
        };
        let Ok(id) = source.node_id.0.parse::<i64>() else {
            return false;
        };
        let Some(node) = storage
            .get_node(codestory_contracts::graph::NodeId(id))
            .ok()
            .flatten()
        else {
            return false;
        };
        if source_file_id(&node).map(|id| id.0) != Some(source.file_id) {
            return false;
        }
        let Some(file) = storage.get_file_by_id(source.file_id).ok().flatten() else {
            return false;
        };
        let Some(focus_line) = node.start_line.filter(|line| *line > 0) else {
            return false;
        };
        if source.start_line != focus_line.saturating_sub(6).max(1)
            || source.end_line > focus_line.saturating_add(6)
        {
            return false;
        }
        let path = if file.path.is_absolute() {
            file.path
        } else {
            project_root.join(file.path)
        };
        if let Some(target_path) = target_path {
            let target_path = std::path::Path::new(target_path);
            let target_path = if target_path.is_absolute() {
                target_path.to_path_buf()
            } else {
                project_root.join(target_path)
            };
            if !codestory_workspace::same_workspace_path(&path, &target_path) {
                return false;
            }
        }
        codestory_workspace::same_workspace_path(&path, std::path::Path::new(&source.path))
            && storage
                .get_file_content_hash(source.file_id)
                .ok()
                .flatten()
                .as_deref()
                == Some(&source.content_sha256)
    }

    pub(crate) fn resolve_project_file_path(
        &self,
        path: &str,
        allow_missing_leaf: bool,
    ) -> Result<PathBuf, ApiError> {
        path_resolution::resolve_project_file_path(self, path, allow_missing_leaf)
    }

    pub fn read_file_text(
        &self,
        req: ReadFileTextRequest,
    ) -> Result<ReadFileTextResponse, ApiError> {
        let candidate = self.resolve_project_file_path(&req.path, false)?;

        let text = std::fs::read_to_string(&candidate).map_err(|e| {
            ApiError::internal(format!("Failed to read file {}: {e}", candidate.display()))
        })?;

        Ok(ReadFileTextResponse {
            path: candidate.to_string_lossy().to_string(),
            text,
        })
    }

    /// One open/scan for every window of the same contained file. This reads
    /// current workspace bytes; it does not mint a symbol or indexed digest.
    pub(crate) fn path_source_windows(
        &self,
        path: &str,
        requests: &[(u32, u32)],
        maximum_bytes: usize,
    ) -> Result<(String, Vec<crate::snippets::PathSourceWindow>), ApiError> {
        let candidate = self.resolve_project_file_path(path, false)?;
        let root = self
            .require_project_root()?
            .canonicalize()
            .map_err(|error| ApiError::internal(error.to_string()))?;
        let relative = candidate
            .strip_prefix(&root)
            .map_err(|_| ApiError::invalid_argument("Source path escaped the project"))?;
        let relative = relative
            .to_str()
            .ok_or_else(|| ApiError::invalid_argument("Source path is not UTF-8"))?
            .replace(std::path::MAIN_SEPARATOR, "/");
        let file = open_regular_source_file_with(&candidate, |path| std::fs::File::open(path))
            .map_err(|error| ApiError::new("source_unavailable", error.to_string()))?;
        let windows = crate::snippets::read_path_source_windows(
            std::io::BufReader::new(file),
            requests,
            maximum_bytes,
        )
        .map_err(|error| ApiError::new("source_unavailable", error.to_string()))?;
        Ok((relative, windows))
    }

    pub fn write_file_text(
        &self,
        req: WriteFileTextRequest,
    ) -> Result<WriteFileResponse, ApiError> {
        let candidate = self.resolve_project_file_path(&req.path, true)?;
        std::fs::write(&candidate, &req.text).map_err(|e| {
            ApiError::internal(format!("Failed to write file {}: {e}", candidate.display()))
        })?;

        Ok(WriteFileResponse {
            bytes_written: clamp_i64_to_u32(req.text.len() as i64),
        })
    }
}

/// Reject known nonregular paths before opening (notably FIFOs), then verify
/// the opened handle as well. This is not a race-free anchored-open contract:
/// ordinary replacement between metadata/open remains outside that guarantee.
/// The opener seam lets a finite test prove ordering without a blocking FIFO.
pub(crate) fn open_regular_source_file_with(
    path: &std::path::Path,
    open: impl FnOnce(&std::path::Path) -> std::io::Result<std::fs::File>,
) -> std::io::Result<std::fs::File> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Source path must name a regular file",
        ));
    }
    let file = open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Opened source handle must be a regular file",
        ));
    }
    Ok(file)
}
