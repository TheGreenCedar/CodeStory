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
