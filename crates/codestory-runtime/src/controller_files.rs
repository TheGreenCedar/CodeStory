use crate::snippets::{
    BoundedSnippet, BoundedSnippetRangeOptions, bounded_markdown_snippet_from_path,
    bounded_markdown_snippet_range_from_path,
};
use crate::support::clamp_i64_to_u32;
use crate::{AppController, path_resolution};
use codestory_contracts::api::{
    ApiError, ReadFileTextRequest, ReadFileTextResponse, WriteFileResponse, WriteFileTextRequest,
};
use std::path::PathBuf;

impl AppController {
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
        let publication = self.active_core_publication().ok_or_else(unavailable)?;
        let project_root = self.require_project_root()?;
        let storage = self.open_storage_read_only()?;
        let id = node.id.0.parse::<i64>().map_err(|_| unavailable())?;
        let stored_node = storage
            .get_node(codestory_contracts::graph::NodeId(id))
            .ok()
            .flatten()
            .ok_or_else(unavailable)?;
        let file_id = stored_node.file_node_id.ok_or_else(unavailable)?.0;
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
        let source = crate::search_evidence::verified_file(&storage, Some(&project_root), &file)
            .ok_or_else(unavailable)?;
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
                file_id,
                path,
                project_id: codestory_workspace::project_identity_v3(&project_root).project_id,
                core_generation_id: publication.generation_id,
                core_run_id: publication.run_id,
                content_sha256: source.content_sha256,
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
        if node.file_node_id.map(|id| id.0) != Some(source.file_id) {
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

    pub(crate) fn bounded_file_snippet(
        &self,
        path: &str,
        line: u32,
        context_lines: usize,
        max_bytes: usize,
        truncation_suffix: &str,
    ) -> Result<(String, BoundedSnippet), ApiError> {
        let candidate = self.resolve_project_file_path(path, false)?;
        let snippet = bounded_markdown_snippet_from_path(
            &candidate,
            line,
            context_lines,
            max_bytes,
            truncation_suffix,
        )
        .map_err(|e| {
            ApiError::internal(format!("Failed to read file {}: {e}", candidate.display()))
        })?;

        Ok((candidate.to_string_lossy().to_string(), snippet))
    }

    pub(crate) fn bounded_file_snippet_range(
        &self,
        path: &str,
        options: BoundedSnippetRangeOptions<'_>,
    ) -> Result<(String, BoundedSnippet), ApiError> {
        let candidate = self.resolve_project_file_path(path, false)?;
        let snippet = bounded_markdown_snippet_range_from_path(
            &candidate,
            options.focus_line,
            options.start_line,
            options.end_line,
            options.context_lines,
            options.max_bytes,
            options.truncation_suffix,
        )
        .map_err(|e| {
            ApiError::internal(format!("Failed to read file {}: {e}", candidate.display()))
        })?;

        Ok((candidate.to_string_lossy().to_string(), snippet))
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
