use super::super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::super::resolution::{StructuredCommandFailure, command_failure_envelope};
use super::affected_rendering::render_affected_markdown;
use crate::args::{AffectedChangeSource, AffectedCommand, AffectedStdinFormat};
use crate::output::{RenderedPublicOutput, emit_public_operation};
use crate::runtime::{RuntimeContext, ensure_index_ready, map_api_error};
#[cfg(test)]
use anyhow::bail;
use anyhow::{Context, Result};
use codestory_contracts::api::{
    AffectedAnalysisInput, AffectedAnalysisRequest, AffectedChangeKindDto, AffectedChangeRecordDto,
    CommandFailureEnvelope,
};
use codestory_workspace::{
    RepositoryChange, RepositoryChangeKind, RepositoryChangeScope, read_repository_changes,
};
use std::io::Read;

pub(in crate::app) fn run_affected(cmd: AffectedCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "affected")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "affected")?;
    let change_records =
        affected_change_records(&cmd).map_err(|error| affected_discovery_error(&cmd, error))?;
    let operation = runtime.run_observational_public_operation("affected", || {
        let output = runtime
            .browser
            .affected_analysis(AffectedAnalysisRequest {
                input: AffectedAnalysisInput::ChangeRecords(change_records.clone()),
                depth: Some(cmd.depth),
                filter: cmd.filter.clone(),
            })
            .map_err(map_api_error)?;
        let markdown = render_affected_markdown(&output);
        RenderedPublicOutput::structured(&output, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

pub(super) fn affected_change_records(
    cmd: &AffectedCommand,
) -> Result<Vec<AffectedChangeRecordDto>> {
    let mut records = cmd
        .paths
        .iter()
        .map(|path| affected_path_record(path, AffectedChangeKindDto::Unknown, "path"))
        .collect::<Vec<_>>();
    if cmd.stdin {
        let mut input = Vec::new();
        std::io::stdin()
            .read_to_end(&mut input)
            .context("Failed to read changed paths from stdin")?;
        let input = path_text_from_bytes(&input, "stdin")?;
        match cmd.stdin_format {
            AffectedStdinFormat::Path => {
                records.extend(input.lines().filter(|line| !line.is_empty()).map(|path| {
                    affected_path_record(path, AffectedChangeKindDto::Unknown, "stdin")
                }))
            }
            AffectedStdinFormat::NameStatus => {
                records.extend(parse_git_name_status_records(&input)?);
            }
        }
    }
    if !records.is_empty() {
        dedupe_affected_change_records(&mut records);
        return Ok(records);
    }
    let scope = match cmd.changes {
        AffectedChangeSource::Head => RepositoryChangeScope::Head,
        AffectedChangeSource::Staged => RepositoryChangeScope::Staged,
        AffectedChangeSource::Unstaged => RepositoryChangeScope::Unstaged,
        AffectedChangeSource::Untracked => RepositoryChangeScope::Untracked,
    };
    let mut records = read_repository_changes(&cmd.project.project, scope)
        .context("Failed to read repository changes")?
        .into_iter()
        .map(affected_repository_change_record)
        .collect::<Result<Vec<_>>>()?;
    dedupe_affected_change_records(&mut records);
    Ok(records)
}

fn affected_repository_change_record(change: RepositoryChange) -> Result<AffectedChangeRecordDto> {
    let (kind, status) = match change.kind {
        RepositoryChangeKind::Added => (AffectedChangeKindDto::Added, "A"),
        RepositoryChangeKind::Copied => (AffectedChangeKindDto::Copied, "C"),
        RepositoryChangeKind::Deleted => (AffectedChangeKindDto::Deleted, "D"),
        RepositoryChangeKind::Modified => (AffectedChangeKindDto::Modified, "M"),
        RepositoryChangeKind::Renamed => (AffectedChangeKindDto::Renamed, "R"),
        RepositoryChangeKind::TypeChanged => (AffectedChangeKindDto::Modified, "T"),
        RepositoryChangeKind::Unmerged => (AffectedChangeKindDto::Modified, "U"),
        RepositoryChangeKind::Untracked => (AffectedChangeKindDto::Untracked, "??"),
    };
    Ok(AffectedChangeRecordDto {
        path: path_text_from_bytes(&change.path, "repository_metadata")?,
        kind,
        status: status.to_string(),
        previous_path: change
            .previous_path
            .as_deref()
            .map(|path| path_text_from_bytes(path, "repository_metadata"))
            .transpose()?,
    })
}

#[derive(Debug)]
pub(in crate::app) struct UnsupportedNonUtf8Path {
    source: &'static str,
}

impl std::fmt::Display for UnsupportedNonUtf8Path {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "unsupported_non_utf8_path: {} returned a path that cannot be represented in UTF-8",
            self.source
        )
    }
}

impl std::error::Error for UnsupportedNonUtf8Path {}

pub(super) fn affected_discovery_error(
    cmd: &AffectedCommand,
    error: anyhow::Error,
) -> anyhow::Error {
    let Some(unsupported) = error.downcast_ref::<UnsupportedNonUtf8Path>() else {
        return error;
    };
    StructuredCommandFailure {
        envelope: unsupported_non_utf8_path_envelope(unsupported),
        output_file: cmd.output_file.clone(),
        markdown: None,
    }
    .into()
}

pub(in crate::app) fn unsupported_non_utf8_path_envelope(
    error: &UnsupportedNonUtf8Path,
) -> CommandFailureEnvelope {
    command_failure_envelope(
        "unsupported_non_utf8_path",
        "git_change_discovery",
        error.to_string(),
        serde_json::json!({"source": error.source}),
    )
}

#[cfg(test)]
fn nul_delimited_git_fields(input: &[u8]) -> Result<Vec<&[u8]>> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if input.last() != Some(&0) {
        bail!("git NUL-delimited path output is missing its terminator");
    }
    let fields = input[..input.len() - 1]
        .split(|byte| *byte == 0)
        .collect::<Vec<_>>();
    if fields.iter().any(|field| field.is_empty()) {
        bail!("git NUL-delimited path output contains an empty field");
    }
    Ok(fields)
}

pub(super) fn path_text_from_bytes(bytes: &[u8], source: &'static str) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| anyhow::Error::new(UnsupportedNonUtf8Path { source }))
}

#[cfg(test)]
pub(in crate::app) fn parse_git_name_status_records_z(
    input: &[u8],
) -> Result<Vec<AffectedChangeRecordDto>> {
    let fields = nul_delimited_git_fields(input)?;
    let mut records = Vec::new();
    let mut index = 0;
    while index < fields.len() {
        let status = std::str::from_utf8(fields[index])
            .context("git name-status status is not valid UTF-8")?;
        index += 1;
        let kind = affected_change_kind_from_status(status);
        let previous_path = if matches!(
            kind,
            AffectedChangeKindDto::Renamed | AffectedChangeKindDto::Copied
        ) {
            let field = fields
                .get(index)
                .context("git name-status rename/copy record is missing the previous path")?;
            index += 1;
            Some(path_text_from_bytes(field, "git_name_status")?)
        } else {
            None
        };
        let field = fields
            .get(index)
            .context("git name-status record is missing the path")?;
        index += 1;
        records.push(AffectedChangeRecordDto {
            path: path_text_from_bytes(field, "git_name_status")?,
            kind,
            status: status.to_string(),
            previous_path,
        });
    }
    Ok(records)
}

pub(super) fn parse_git_name_status_records(input: &str) -> Result<Vec<AffectedChangeRecordDto>> {
    input
        .lines()
        .filter(|line| !line.is_empty())
        .map(parse_git_name_status_record)
        .collect()
}

pub(super) fn parse_git_name_status_record(line: &str) -> Result<AffectedChangeRecordDto> {
    let parts = line.split('\t').collect::<Vec<_>>();
    if parts.len() == 1 {
        return Ok(affected_path_record(
            parts[0],
            AffectedChangeKindDto::Unknown,
            "path",
        ));
    }
    let status = parts[0];
    let kind = affected_change_kind_from_status(status);
    let (previous_path, path) = if matches!(
        kind,
        AffectedChangeKindDto::Renamed | AffectedChangeKindDto::Copied
    ) {
        let previous = parts
            .get(1)
            .copied()
            .filter(|path| !path.is_empty())
            .context("git name-status rename/copy row is missing the previous path")?;
        let current = parts
            .get(2)
            .copied()
            .filter(|path| !path.is_empty())
            .context("git name-status rename/copy row is missing the current path")?;
        (Some(previous.to_string()), current)
    } else {
        let path = parts
            .get(1)
            .copied()
            .filter(|path| !path.is_empty())
            .context("git name-status row is missing the path")?;
        (None, path)
    };
    Ok(AffectedChangeRecordDto {
        path: path.to_string(),
        kind,
        status: status.to_string(),
        previous_path,
    })
}

pub(in crate::app) fn affected_path_record(
    path: &str,
    kind: AffectedChangeKindDto,
    status: &str,
) -> AffectedChangeRecordDto {
    AffectedChangeRecordDto {
        path: path.to_string(),
        kind,
        status: status.to_string(),
        previous_path: None,
    }
}

pub(super) fn affected_change_kind_from_status(status: &str) -> AffectedChangeKindDto {
    match status.chars().next().unwrap_or_default() {
        'A' => AffectedChangeKindDto::Added,
        'M' | 'T' | 'U' => AffectedChangeKindDto::Modified,
        'D' => AffectedChangeKindDto::Deleted,
        'R' => AffectedChangeKindDto::Renamed,
        'C' => AffectedChangeKindDto::Copied,
        '?' => AffectedChangeKindDto::Untracked,
        _ => AffectedChangeKindDto::Unknown,
    }
}

pub(super) fn dedupe_affected_change_records(records: &mut Vec<AffectedChangeRecordDto>) {
    records.retain(|record| !record.path.is_empty());
    records.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.previous_path.cmp(&right.previous_path))
            .then(left.status.cmp(&right.status))
    });
    records.dedup_by(|left, right| {
        left.path == right.path
            && left.previous_path == right.previous_path
            && left.status == right.status
    });
}
