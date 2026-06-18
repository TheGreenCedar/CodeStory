use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{NodeId, NodeKind};
use std::path::Path;

use super::common::{push_member_edge, push_structural_node};

pub(crate) fn collect_github_actions_workflow_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) {
    let path_key = path.to_string_lossy().replace('\\', "/");
    let (workflow_name, workflow_line, workflow_col) =
        workflow_name(source).unwrap_or_else(|| fallback_workflow_name(path));
    let workflow_id = push_structural_node(
        storage,
        file_id,
        NodeKind::MODULE,
        &workflow_name,
        &format!("github-actions:workflow:{path_key}"),
        workflow_line,
        workflow_col,
    );
    push_member_edge(storage, file_id, file_id, workflow_id, workflow_line);

    collect_jobs_and_steps(&path_key, source, file_id, storage, workflow_id);
}

fn collect_jobs_and_steps(
    path_key: &str,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    workflow_id: NodeId,
) {
    let mut in_jobs = false;
    let mut job_indent = None;
    let mut current_job: Option<(usize, NodeId, String)> = None;
    let mut in_steps = false;
    let mut steps_indent = 0usize;
    let mut step_index = 0usize;

    for (line_idx, line_text) in source.lines().enumerate() {
        let line = line_idx.saturating_add(1).try_into().unwrap_or(u32::MAX);
        let trimmed = line_text.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = leading_spaces(line_text);

        if indent == 0 && trimmed == "jobs:" {
            in_jobs = true;
            current_job = None;
            in_steps = false;
            continue;
        }
        if !in_jobs {
            continue;
        }
        if indent == 0 {
            break;
        }

        if let Some(expected_indent) = job_indent
            && indent == expected_indent
            && !trimmed.starts_with('-')
            && let Some(job_name) = yaml_mapping_key(trimmed)
        {
            let job_id = push_structural_node(
                storage,
                file_id,
                NodeKind::FUNCTION,
                &job_name,
                &format!("github-actions:job:{path_key}:{job_name}"),
                line,
                indent.saturating_add(1).try_into().unwrap_or(u32::MAX),
            );
            push_member_edge(storage, file_id, workflow_id, job_id, line);
            current_job = Some((indent, job_id, job_name));
            in_steps = false;
            step_index = 0;
            continue;
        }

        if job_indent.is_none()
            && indent > 0
            && !trimmed.starts_with('-')
            && let Some(job_name) = yaml_mapping_key(trimmed)
        {
            job_indent = Some(indent);
            let job_id = push_structural_node(
                storage,
                file_id,
                NodeKind::FUNCTION,
                &job_name,
                &format!("github-actions:job:{path_key}:{job_name}"),
                line,
                indent.saturating_add(1).try_into().unwrap_or(u32::MAX),
            );
            push_member_edge(storage, file_id, workflow_id, job_id, line);
            current_job = Some((indent, job_id, job_name));
            step_index = 0;
            continue;
        }

        let Some((current_job_indent, job_id, job_name)) = current_job.as_ref() else {
            continue;
        };
        if indent <= *current_job_indent {
            in_steps = false;
            continue;
        }
        if trimmed == "steps:" {
            in_steps = true;
            steps_indent = indent;
            step_index = 0;
            continue;
        }
        if in_steps
            && indent > steps_indent
            && let Some((step_label, step_col)) = step_label(line_text)
        {
            step_index = step_index.saturating_add(1);
            let name = format!("{job_name} step {step_index}: {step_label}");
            let step_id = push_structural_node(
                storage,
                file_id,
                NodeKind::ANNOTATION,
                &name,
                &format!("github-actions:step:{path_key}:{job_name}:{step_index}"),
                line,
                step_col,
            );
            push_member_edge(storage, file_id, *job_id, step_id, line);
        }
    }
}

fn workflow_name(source: &str) -> Option<(String, u32, u32)> {
    for (line_idx, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if leading_spaces(line) == 0
            && let Some(rest) = trimmed.strip_prefix("name:")
        {
            let value = clean_yaml_scalar(rest.trim());
            if !value.is_empty() {
                let col = line.find(&value).unwrap_or(0).saturating_add(1);
                return Some((
                    value,
                    line_idx.saturating_add(1).try_into().unwrap_or(u32::MAX),
                    col.try_into().unwrap_or(u32::MAX),
                ));
            }
        }
    }
    None
}

fn fallback_workflow_name(path: &Path) -> (String, u32, u32) {
    (
        path.file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("workflow")
            .to_string(),
        1,
        1,
    )
}

fn yaml_mapping_key(trimmed_line: &str) -> Option<String> {
    let (key, _) = trimmed_line.split_once(':')?;
    let key = key.trim();
    if key.is_empty() || key.contains(' ') || !key.chars().all(github_actions_key_char) {
        return None;
    }
    Some(key.to_string())
}

fn step_label(line: &str) -> Option<(String, u32)> {
    let indent = leading_spaces(line);
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("- ")?;
    for key in ["name:", "uses:", "run:"] {
        if let Some(value) = rest.trim_start().strip_prefix(key) {
            let value = clean_yaml_scalar(value.trim());
            let label = if value.is_empty() {
                key.trim_end_matches(':').to_string()
            } else {
                value
            };
            return Some((
                label,
                indent.saturating_add(3).try_into().unwrap_or(u32::MAX),
            ));
        }
    }
    None
}

fn clean_yaml_scalar(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|ch| *ch == ' ').count()
}

fn github_actions_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')
}
