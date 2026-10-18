//! Bounded source-navigation bindings, separate from exact call resolution.
use crate::cache::CachedResolutionFile;
use codestory_contracts::graph::{
    Node, NodeId, NodeKind, PublicExportAlias, PublicExportAliasStatus,
};
use codestory_store::{IndexPublicationRecord, Store};
use codestory_workspace::workspace_path_identity;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use tree_sitter::{Node as SyntaxNode, Tree};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum ExportTarget {
    Local(NodeId),
    Relative {
        module: String,
        name: String,
        import_line: Option<u32>,
    },
    Unresolved(PublicExportAliasStatus),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ExportInput {
    pub name: String,
    pub local_name: String,
    pub line: u32,
    pub column: u32,
    pub direct: bool,
    pub target: ExportTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ExportFileInput {
    // Coverage can be partial even when individual bindings have complete proof.
    // CachedResolutionFile separately owns parser and source integrity.
    pub complete: bool,
    pub bindings: Vec<ExportInput>,
}

fn text<'a>(node: SyntaxNode<'_>, source: &'a str) -> Option<&'a str> {
    node.utf8_text(source.as_bytes()).ok()
}

// Occupancy needs the actual spelling even when this form cannot resolve a
// target. Reuse JSON's decoder for its string subset; do not guess other JS
// escapes or escaped identifiers. Callers quarantine names still unknown.
fn known_binding_name(node: SyntaxNode<'_>, source: &str) -> Option<String> {
    let value = text(node, source)?;
    match node.kind() {
        "identifier" | "type_identifier" | "shorthand_property_identifier_pattern" => {
            (!value.contains('\\')).then(|| value.to_string())
        }
        "string" if value.starts_with('"') => serde_json::from_str(value).ok(),
        "string" => {
            let value = value.strip_prefix('\'')?.strip_suffix('\'')?;
            (!value.contains(['\\', '\n', '\r'])).then(|| value.to_string())
        }
        _ => None,
    }
}

fn has_token(node: SyntaxNode<'_>, token: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind() == token)
}

fn module_text(node: SyntaxNode<'_>, source: &str) -> Option<String> {
    let value = text(node, source)?;
    let value = value.get(1..value.len().checked_sub(1)?)?;
    (!value.contains('\\') && (value.starts_with("./") || value.starts_with("../")))
        .then(|| value.to_string())
}

fn declaration(node: SyntaxNode<'_>) -> Option<SyntaxNode<'_>> {
    let node = if node.kind() == "export_statement" {
        if has_token(node, "default") {
            return None;
        }
        node.child_by_field_name("declaration")?
    } else {
        node
    };
    matches!(node.kind(), "class_declaration" | "function_declaration").then_some(node)
}

fn declaration_target(
    node: SyntaxNode<'_>,
    source: &str,
    nodes: &[Node],
) -> Option<(String, ExportTarget)> {
    let name_node = node.child_by_field_name("name")?;
    let name = text(name_node, source)?.to_string();
    let anchor = if node.kind() == "class_declaration" {
        name_node
    } else {
        node
    };
    let kind = if node.kind() == "class_declaration" {
        NodeKind::CLASS
    } else {
        NodeKind::FUNCTION
    };
    let matches = nodes
        .iter()
        .filter(|candidate| {
            candidate.kind == kind
                && candidate.start_line == Some(anchor.start_position().row as u32 + 1)
                && candidate.start_col == Some(anchor.start_position().column as u32 + 1)
                && candidate.serialized_name.rsplit("::").next() == Some(name.as_str())
        })
        .collect::<Vec<_>>();
    let target = if let [target] = matches.as_slice() {
        ExportTarget::Local(target.id)
    } else {
        ExportTarget::Unresolved(PublicExportAliasStatus::Ambiguous)
    };
    Some((name, target))
}

fn module_writes(node: SyntaxNode<'_>, source: &str, writes: &mut HashSet<String>) {
    if matches!(
        node.kind(),
        "function_declaration"
            | "function_expression"
            | "arrow_function"
            | "class_declaration"
            | "method_definition"
    ) {
        return;
    }
    if matches!(
        node.kind(),
        "assignment_expression" | "augmented_assignment_expression" | "update_expression"
    ) {
        if let Some(left) = node
            .child_by_field_name("left")
            .or_else(|| node.child_by_field_name("argument"))
            && left.kind() == "identifier"
            && let Some(name) = text(left, source)
        {
            writes.insert(name.to_string());
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        module_writes(child, source, writes);
    }
}

// Record occupied names without giving unsupported declarations target authority.
// Unwrap declaration wrappers only; namespace/function bodies are not module scope.
fn unsupported_declaration_names(node: SyntaxNode<'_>) -> Vec<SyntaxNode<'_>> {
    let mut names = Vec::new();
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "ambient_declaration"
            | "lexical_declaration"
            | "variable_declaration"
            | "array_pattern"
            | "object_pattern"
            | "rest_pattern" => {
                let mut walk = node.walk();
                pending.extend(node.named_children(&mut walk));
            }
            "namespace_export" => {
                let mut walk = node.walk();
                names.extend(
                    node.named_children(&mut walk)
                        .filter(|name| matches!(name.kind(), "identifier" | "string")),
                );
            }
            "variable_declarator" => pending.extend(node.child_by_field_name("name")),
            "assignment_pattern" | "object_assignment_pattern" => {
                pending.extend(node.child_by_field_name("left"));
            }
            "pair_pattern" => pending.extend(node.child_by_field_name("value")),
            "identifier" | "type_identifier" | "shorthand_property_identifier_pattern" => {
                names.push(node);
            }
            _ => {
                if let Some(mut name) = node.child_by_field_name("name") {
                    while matches!(name.kind(), "nested_identifier" | "member_expression") {
                        let Some(object) = name.child_by_field_name("object") else {
                            break;
                        };
                        name = object;
                    }
                    if matches!(name.kind(), "identifier" | "type_identifier") {
                        names.push(name);
                    }
                }
            }
        }
    }
    names
}

pub(crate) fn collect(tree: &Tree, source: &str, nodes: &[Node]) -> ExportFileInput {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let statements = root.named_children(&mut cursor).collect::<Vec<_>>();
    let mut bindings = HashMap::<String, Vec<ExportTarget>>::new();
    let mut writes = HashSet::new();
    module_writes(root, source, &mut writes);
    let mut unknown_local_name = writes.iter().any(|name| name.contains('\\'));
    for statement in &statements {
        if let Some(decl) = declaration(*statement)
            && let Some((name, target)) = declaration_target(decl, source, nodes)
        {
            if decl
                .child_by_field_name("name")
                .and_then(|name| known_binding_name(name, source))
                .is_some()
            {
                bindings.entry(name).or_default().push(target);
            } else {
                unknown_local_name = true;
            }
        }
        if statement.kind() != "import_statement" {
            if declaration(*statement).is_none() {
                let raw = statement
                    .child_by_field_name("declaration")
                    .unwrap_or(*statement);
                for name in unsupported_declaration_names(raw) {
                    if let Some(name) = known_binding_name(name, source) {
                        bindings
                            .entry(name)
                            .or_default()
                            .push(ExportTarget::Unresolved(
                                PublicExportAliasStatus::Unsupported,
                            ));
                    } else {
                        unknown_local_name = true;
                    }
                }
            }
            continue;
        }
        let module = statement
            .child_by_field_name("source")
            .and_then(|node| module_text(node, source));
        let mut walk = statement.walk();
        let clause = statement
            .named_children(&mut walk)
            .find(|node| node.kind() == "import_clause");
        let Some(clause) = clause else { continue };
        let mut walk = clause.walk();
        let entries = clause
            .named_children(&mut walk)
            .filter(|node| node.kind() != "comment")
            .collect::<Vec<_>>();
        let supported_clause = entries.len() == 1
            && entries[0].kind() == "named_imports"
            && !has_token(*statement, "type");
        for entry in entries {
            if entry.kind() == "named_imports" {
                let mut walk = entry.walk();
                for specifier in entry.named_children(&mut walk) {
                    if specifier.kind() != "import_specifier" {
                        continue;
                    }
                    let Some(name_node) = specifier.child_by_field_name("name") else {
                        continue;
                    };
                    let local = specifier.child_by_field_name("alias").unwrap_or(name_node);
                    let Some(local_name) = known_binding_name(local, source) else {
                        unknown_local_name = true;
                        continue;
                    };
                    let target = if supported_clause
                        && !has_token(specifier, "type")
                        && name_node.kind() == "identifier"
                        && local.kind() == "identifier"
                    {
                        module.as_ref().map_or(
                            ExportTarget::Unresolved(PublicExportAliasStatus::Unsupported),
                            |module| ExportTarget::Relative {
                                module: module.clone(),
                                name: text(name_node, source).unwrap().to_string(),
                                import_line: Some(statement.start_position().row as u32 + 1),
                            },
                        )
                    } else {
                        ExportTarget::Unresolved(PublicExportAliasStatus::Unsupported)
                    };
                    bindings.entry(local_name).or_default().push(target);
                }
            } else {
                let name = if entry.kind() == "identifier" {
                    Some(entry)
                } else {
                    let mut walk = entry.walk();
                    entry
                        .named_children(&mut walk)
                        .find(|node| node.kind() == "identifier")
                };
                if let Some(name) = name {
                    if let Some(name) = known_binding_name(name, source) {
                        bindings
                            .entry(name)
                            .or_default()
                            .push(ExportTarget::Unresolved(
                                PublicExportAliasStatus::Unsupported,
                            ));
                    } else {
                        unknown_local_name = true;
                    }
                }
            }
        }
    }
    let mut complete = !root.has_error();
    let mut exports = Vec::new();
    let mut unknown_public_name = false;
    for statement in statements {
        if statement.kind() != "export_statement" {
            continue;
        }
        if let Some(decl) = declaration(statement)
            && let Some((name, target)) = declaration_target(decl, source, nodes)
        {
            if decl
                .child_by_field_name("name")
                .and_then(|name| known_binding_name(name, source))
                .is_none()
            {
                complete = false;
                unknown_public_name = true;
                continue;
            }
            let target = if writes.contains(&name) {
                ExportTarget::Unresolved(PublicExportAliasStatus::Unsupported)
            } else if unknown_local_name
                || bindings
                    .get(&name)
                    .is_none_or(|bindings| bindings.len() != 1)
            {
                ExportTarget::Unresolved(PublicExportAliasStatus::Ambiguous)
            } else {
                target
            };
            exports.push(ExportInput {
                name: name.clone(),
                local_name: name,
                line: decl.start_position().row as u32 + 1,
                column: decl.start_position().column as u32 + 1,
                direct: true,
                target,
            });
            continue;
        }
        let mut walk = statement.walk();
        let clause = statement
            .named_children(&mut walk)
            .find(|node| node.kind() == "export_clause");
        let Some(clause) = clause else {
            complete = false;
            // An explicit unsupported declaration still occupies its exported
            // spelling. Plain stars have no explicit binding. Defaults occupy
            // "default", never their local declaration name.
            if has_token(statement, "default") {
                exports.push(ExportInput {
                    name: "default".into(),
                    local_name: "default".into(),
                    line: statement.start_position().row as u32 + 1,
                    column: statement.start_position().column as u32 + 1,
                    direct: false,
                    target: ExportTarget::Unresolved(PublicExportAliasStatus::Unsupported),
                });
            } else {
                let mut walk = statement.walk();
                let raw = statement.child_by_field_name("declaration").or_else(|| {
                    statement
                        .named_children(&mut walk)
                        .find(|child| child.kind() == "namespace_export")
                });
                if let Some(raw) = raw {
                    for name in unsupported_declaration_names(raw) {
                        if let Some(value) = known_binding_name(name, source) {
                            // Empty quoted names cannot collide with a supported
                            // declaration, and the persisted contract is nonempty.
                            if value.is_empty() {
                                continue;
                            }
                            exports.push(ExportInput {
                                name: value.clone(),
                                local_name: value,
                                line: name.start_position().row as u32 + 1,
                                column: name.start_position().column as u32 + 1,
                                direct: false,
                                target: ExportTarget::Unresolved(
                                    PublicExportAliasStatus::Unsupported,
                                ),
                            });
                        } else {
                            unknown_public_name = true;
                        }
                    }
                }
            }
            continue;
        };
        let module = statement.child_by_field_name("source");
        let mut walk = clause.walk();
        for specifier in clause.named_children(&mut walk) {
            if specifier.kind() != "export_specifier" {
                continue;
            }
            let Some(local_node) = specifier.child_by_field_name("name") else {
                complete = false;
                unknown_public_name = true;
                continue;
            };
            let public_node = specifier.child_by_field_name("alias").unwrap_or(local_node);
            let Some(public) = known_binding_name(public_node, source) else {
                complete = false;
                unknown_public_name = true;
                continue;
            };
            if public.is_empty() {
                complete = false;
                continue;
            }
            let local = text(local_node, source).unwrap_or_default();
            let supported_names = local_node.kind() == "identifier"
                && public_node.kind() == "identifier"
                && known_binding_name(local_node, source).is_some();
            let target = if !supported_names {
                complete = false;
                ExportTarget::Unresolved(PublicExportAliasStatus::Unsupported)
            } else if has_token(statement, "type") || has_token(specifier, "type") {
                ExportTarget::Unresolved(PublicExportAliasStatus::Unsupported)
            } else if let Some(module) = module {
                module_text(module, source).map_or(
                    ExportTarget::Unresolved(PublicExportAliasStatus::Unsupported),
                    |module| ExportTarget::Relative {
                        module,
                        name: local.to_string(),
                        import_line: None,
                    },
                )
            } else if writes.contains(local) {
                ExportTarget::Unresolved(PublicExportAliasStatus::Unsupported)
            } else if unknown_local_name {
                ExportTarget::Unresolved(PublicExportAliasStatus::Ambiguous)
            } else {
                match bindings.get(local).map(Vec::as_slice) {
                    Some([target]) => target.clone(),
                    Some(_) => ExportTarget::Unresolved(PublicExportAliasStatus::Ambiguous),
                    None => ExportTarget::Unresolved(PublicExportAliasStatus::MissingBinding),
                }
            };
            exports.push(ExportInput {
                name: public,
                local_name: local.to_string(),
                line: public_node.start_position().row as u32 + 1,
                column: public_node.start_position().column as u32 + 1,
                direct: false,
                target,
            });
        }
    }
    let mut counts = HashMap::<String, usize>::new();
    for export in &exports {
        *counts.entry(export.name.clone()).or_default() += 1;
    }
    for export in &mut exports {
        if unknown_public_name || counts[&export.name] != 1 {
            export.target = ExportTarget::Unresolved(PublicExportAliasStatus::Ambiguous);
        }
    }
    ExportFileInput {
        complete,
        bindings: exports,
    }
}

pub(crate) struct Source<'a> {
    pub path: &'a Path,
    pub file: &'a CachedResolutionFile,
}

fn normalized(path: &Path) -> Option<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return None;
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    Some(result)
}

fn relative_target<'a, 'b>(
    source: &Source<'_>,
    module: &str,
    records: &'a [Source<'b>],
) -> Result<&'a Source<'b>, PublicExportAliasStatus> {
    let base = normalized(
        &source
            .path
            .parent()
            .ok_or(PublicExportAliasStatus::Unsupported)?
            .join(module),
    )
    .ok_or(PublicExportAliasStatus::Unsupported)?;
    let mut boundary = source
        .path
        .parent()
        .ok_or(PublicExportAliasStatus::Unsupported)?
        .to_path_buf();
    while records
        .iter()
        .any(|record| !record.path.starts_with(&boundary))
    {
        if !boundary.pop() {
            return Err(PublicExportAliasStatus::Unsupported);
        }
    }
    if !base.starts_with(&boundary) {
        return Err(PublicExportAliasStatus::Unsupported);
    }
    let mut candidates = vec![base.clone()];
    match base.extension().and_then(|extension| extension.to_str()) {
        None => {
            candidates.clear();
            for extension in ["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"] {
                candidates.push(base.with_extension(extension));
            }
        }
        Some("js") if matches!(source.file.language.as_str(), "typescript" | "tsx") => {
            candidates.push(base.with_extension("ts"));
            candidates.push(base.with_extension("tsx"));
        }
        Some("mjs") if source.file.language == "typescript" => {
            candidates.push(base.with_extension("mts"))
        }
        Some("cjs") if source.file.language == "typescript" => {
            candidates.push(base.with_extension("cts"))
        }
        Some("ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs") => {}
        _ => return Err(PublicExportAliasStatus::Unsupported),
    }
    let mut matches = Vec::new();
    for candidate in candidates {
        let metadata = match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(PublicExportAliasStatus::Incomplete),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(PublicExportAliasStatus::Unsupported);
        }
        let identity =
            workspace_path_identity(&candidate).map_err(|_| PublicExportAliasStatus::Incomplete)?;
        let known = records
            .iter()
            .filter(|record| workspace_path_identity(record.path).ok().as_ref() == Some(&identity))
            .collect::<Vec<_>>();
        let [record] = known.as_slice() else {
            return Err(PublicExportAliasStatus::Incomplete);
        };
        if !matches
            .iter()
            .any(|other: &&Source<'_>| other.file.file_id == record.file.file_id)
        {
            matches.push(*record);
        }
    }
    match matches.as_slice() {
        [record] => Ok(*record),
        [] => Err(PublicExportAliasStatus::MissingBinding),
        _ => Err(PublicExportAliasStatus::Ambiguous),
    }
}

pub(crate) fn rematerialize(
    store: &Store,
    publication: &IndexPublicationRecord,
    records: &[Source<'_>],
) -> anyhow::Result<()> {
    let mut rows = Vec::new();
    let mut complete = true;
    for source in records.iter().filter(|record| {
        matches!(
            record.file.language.as_str(),
            "typescript" | "tsx" | "javascript"
        )
    }) {
        let Some(input) = &source.file.public_exports else {
            complete = false;
            continue;
        };
        complete &= input.complete && source.file.complete && source.file.lookup_input_complete;
        for binding in &input.bindings {
            let mut row = PublicExportAlias {
                public_name: binding.name.clone(),
                local_name: binding.local_name.clone(),
                export_file_id: source.file.file_id,
                export_source_sha256: source.file.source_sha256.clone(),
                export_line: binding.line,
                export_column: binding.column,
                import_line: None,
                module_specifier: None,
                target_node_id: None,
                target_file_id: None,
                target_source_sha256: None,
                status: PublicExportAliasStatus::Incomplete,
            };
            let target = if !source.file.complete || !source.file.lookup_input_complete {
                Err(PublicExportAliasStatus::Incomplete)
            } else {
                match &binding.target {
                    ExportTarget::Local(id) => Ok((*id, source)),
                    ExportTarget::Unresolved(reason) => Err(*reason),
                    ExportTarget::Relative {
                        module,
                        name,
                        import_line,
                    } => {
                        row.import_line = *import_line;
                        row.module_specifier = Some(module.clone());
                        relative_target(source, module, records).and_then(|target| {
                            let input = target
                                .file
                                .public_exports
                                .as_ref()
                                .ok_or(PublicExportAliasStatus::Incomplete)?;
                            if !target.file.complete || !target.file.lookup_input_complete {
                                return Err(PublicExportAliasStatus::Incomplete);
                            }
                            let bindings = input
                                .bindings
                                .iter()
                                .filter(|binding| binding.direct && binding.name == *name)
                                .collect::<Vec<_>>();
                            match bindings.as_slice() {
                                [binding] => match binding.target {
                                    ExportTarget::Local(id) => Ok((id, target)),
                                    _ => Err(PublicExportAliasStatus::Ambiguous),
                                },
                                [] => Err(PublicExportAliasStatus::MissingBinding),
                                _ => Err(PublicExportAliasStatus::Ambiguous),
                            }
                        })
                    }
                }
            };
            match target {
                Ok((id, target)) => {
                    row.status = PublicExportAliasStatus::Bound;
                    row.target_node_id = Some(id);
                    row.target_file_id = Some(target.file.file_id);
                    row.target_source_sha256 = Some(target.file.source_sha256.clone());
                }
                Err(status) => row.status = status,
            }
            rows.push(row);
        }
    }
    store.replace_public_export_aliases(publication, &rows, complete)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn extracted(source: &str) -> ExportFileInput {
        let config = crate::get_language_for_ext("ts").unwrap();
        let indexed =
            crate::index_file(Path::new("/source/module.ts"), source, &config, None, None).unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&config.language).unwrap();
        collect(&parser.parse(source, None).unwrap(), source, &indexed.nodes)
    }
    #[test]
    fn module_binding_scope_and_unsupported_exports_never_invent_targets() {
        let source =
            "class _Owner {}\nfunction nested(){ class _Owner {} }\nexport { _Owner as Public };\n";
        let input = extracted(source);
        assert!(input.complete);
        assert!(matches!(input.bindings[0].target, ExportTarget::Local(_)));
        for (source, status) in [
            (
                "export { Missing as Public };",
                PublicExportAliasStatus::MissingBinding,
            ),
            (
                "class _Owner {}\n_Owner = other;\nexport { _Owner as Public };",
                PublicExportAliasStatus::Unsupported,
            ),
            (
                "import _Owner from './other';\nexport { _Owner as Public };",
                PublicExportAliasStatus::Unsupported,
            ),
            (
                "import * as _Owner from './other';\nexport { _Owner as Public };",
                PublicExportAliasStatus::Unsupported,
            ),
            (
                "import type { _Owner } from './other';\nexport { _Owner as Public };",
                PublicExportAliasStatus::Unsupported,
            ),
            (
                "class _Owner {}\nexport { _Owner as Public, _Owner as Public };",
                PublicExportAliasStatus::Ambiguous,
            ),
            (
                "export class _Owner {}\nclass _Owner {}",
                PublicExportAliasStatus::Ambiguous,
            ),
            (
                "class _Owner {}\nconst _Owner = 1;\nexport { _Owner as Public };",
                PublicExportAliasStatus::Ambiguous,
            ),
            (
                "class _Owner {}\nexport { _Owner as default };\nexport default 1;",
                PublicExportAliasStatus::Ambiguous,
            ),
        ] {
            let input = extracted(source);
            assert!(
                input
                    .bindings
                    .iter()
                    .all(|binding| binding.target == ExportTarget::Unresolved(status)),
                "{source}: {input:?}"
            );
        }
        assert!(extracted("class HiddenBrush {}\n").bindings.is_empty());
        assert!(
            !extracted("class _Owner {}\nexport { _Owner as Public };\nfunction broken( {")
                .complete
        );
    }
}
