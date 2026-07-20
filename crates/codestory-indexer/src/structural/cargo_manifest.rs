use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{NodeId, NodeKind};
use std::path::Path;

use super::common::{StructuralSourceSpan, push_member_edge, push_structural_node};

pub(crate) fn collect_cargo_manifest_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) {
    let path_key = path.to_string_lossy().replace('\\', "/");
    let mut section = Section::Other;
    let mut package_id = None;
    let mut in_workspace_members = false;

    for (line_idx, line_text) in source.lines().enumerate() {
        let line = line_idx.saturating_add(1).try_into().unwrap_or(u32::MAX);
        let code_text = strip_toml_comment(line_text);
        let trimmed = code_text.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some(next) = section_header(trimmed) {
            section = next;
            in_workspace_members = false;
            continue;
        }

        if section == Section::Workspace
            && (in_workspace_members || toml_key(trimmed) == Some("members"))
        {
            in_workspace_members = !code_text.contains(']');
            for (member, col) in quoted_values(code_text) {
                let member_id = push_structural_node(
                    storage,
                    file_id,
                    NodeKind::MODULE,
                    &member,
                    &format!("cargo-manifest:workspace-member:{path_key}:{member}"),
                    StructuralSourceSpan::token(line, col.saturating_sub(1) as usize, member.len()),
                );
                push_member_edge(storage, file_id, file_id, member_id, line);
            }
            continue;
        }

        if section == Section::Package && toml_key(trimmed) == Some("name") {
            if let Some((name, col)) = first_quoted_value(code_text) {
                let id = push_structural_node(
                    storage,
                    file_id,
                    NodeKind::PACKAGE,
                    &name,
                    &format!("cargo-manifest:package:{path_key}:{name}"),
                    StructuralSourceSpan::token(line, col.saturating_sub(1) as usize, name.len()),
                );
                push_member_edge(storage, file_id, file_id, id, line);
                package_id = Some(id);
            }
            continue;
        }

        if section.is_dependency_table()
            && let Some((dependency, col)) = dependency_key(code_text)
        {
            let dep_id = push_structural_node(
                storage,
                file_id,
                NodeKind::ANNOTATION,
                &dependency,
                &format!(
                    "cargo-manifest:dependency:{path_key}:{}:{dependency}",
                    section.name()
                ),
                StructuralSourceSpan::token(line, col.saturating_sub(1) as usize, dependency.len()),
            );
            push_member_edge(
                storage,
                file_id,
                package_id.unwrap_or(file_id),
                dep_id,
                line,
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Workspace,
    Package,
    Dependencies,
    DevDependencies,
    BuildDependencies,
    Other,
}

impl Section {
    fn is_dependency_table(self) -> bool {
        matches!(
            self,
            Self::Dependencies | Self::DevDependencies | Self::BuildDependencies
        )
    }

    fn name(self) -> &'static str {
        match self {
            Self::Dependencies => "dependencies",
            Self::DevDependencies => "dev-dependencies",
            Self::BuildDependencies => "build-dependencies",
            _ => "manifest",
        }
    }
}

fn section_header(trimmed: &str) -> Option<Section> {
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    Some(match inner {
        "workspace" => Section::Workspace,
        "package" => Section::Package,
        "dependencies" => Section::Dependencies,
        "dev-dependencies" => Section::DevDependencies,
        "build-dependencies" => Section::BuildDependencies,
        _ => Section::Other,
    })
}

fn strip_toml_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Some('"'), '\\') => escaped = true,
            (Some(current), value) if value == current => quote = None,
            (None, '"' | '\'') => quote = Some(ch),
            (None, '#') => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn toml_key(trimmed: &str) -> Option<&str> {
    trimmed.split_once('=')?.0.split_whitespace().next()
}

fn dependency_key(line: &str) -> Option<(String, u32)> {
    let (raw_key, _) = line.split_once('=')?;
    let key = raw_key.trim();
    if key.is_empty() || key.contains('.') {
        return None;
    }
    let key = key.trim_matches('"').trim_matches('\'');
    let start = raw_key.find(key)?;
    Some((
        key.to_string(),
        start.saturating_add(1).try_into().unwrap_or(u32::MAX),
    ))
}

fn first_quoted_value(line: &str) -> Option<(String, u32)> {
    quoted_values(line).into_iter().next()
}

fn quoted_values(line: &str) -> Vec<(String, u32)> {
    let mut values = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if !matches!(bytes[index], b'"' | b'\'') {
            index += 1;
            continue;
        }
        let quote = bytes[index];
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end] != quote {
            end += 1;
        }
        if end < bytes.len() {
            values.push((
                line[start..end].to_string(),
                start.saturating_add(1).try_into().unwrap_or(u32::MAX),
            ));
            index = end + 1;
        } else {
            break;
        }
    }
    values
}
