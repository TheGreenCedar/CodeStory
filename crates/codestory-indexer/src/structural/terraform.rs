use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{NodeId, NodeKind};
use std::ops::ControlFlow;
use std::path::Path;
use tree_sitter::{Node as SyntaxNode, ParseOptions, ParseState, Parser, Point, Tree};

use super::StructuralCollectionError;
use super::byte_offset_line_col;
use super::common::{StructuralSourceSpan, push_member_edge, push_structural_node};

const MIN_PARSE_PROGRESS_BUDGET: usize = 16_384;
const PARSE_PROGRESS_CALLS_PER_SOURCE_BYTE: usize = 64;
const MIN_CST_WALK_BUDGET: usize = 1_024;
const CST_NODES_PER_SOURCE_BYTE: usize = 16;

pub(crate) fn collect_terraform_entities(
    _path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) -> Result<(), StructuralCollectionError> {
    let tree = parse_terraform(source)?;
    if tree.root_node().has_error() {
        return Err(StructuralCollectionError::Malformed(
            "invalid Terraform syntax".to_string(),
        ));
    }

    let walk_budget = source
        .len()
        .saturating_mul(CST_NODES_PER_SOURCE_BYTE)
        .max(MIN_CST_WALK_BUDGET);
    let mut visited = 0usize;
    let mut ordinal = 0usize;
    walk_named_nodes(tree.root_node(), |node| {
        visited = visited.saturating_add(1);
        if visited > walk_budget {
            return Err(StructuralCollectionError::Malformed(format!(
                "Terraform syntax tree exceeds the {walk_budget}-node traversal limit"
            )));
        }

        let anchor = match node.kind() {
            "block" => block_header_anchor(source, node)
                .map(|(name, span)| (NodeKind::MODULE, "block", name, span)),
            "attribute" => direct_identifier(node).and_then(|identifier| {
                source_anchor(source, identifier)
                    .map(|(name, span)| (NodeKind::ANNOTATION, "assignment", name, span))
            }),
            "object_elem" => simple_object_key(node).and_then(|identifier| {
                source_anchor(source, identifier)
                    .map(|(name, span)| (NodeKind::ANNOTATION, "object-key", name, span))
            }),
            _ => None,
        };

        if let Some((kind, anchor_kind, name, span)) = anchor {
            ordinal = ordinal.saturating_add(1);
            let node_id = push_structural_node(
                storage,
                file_id,
                kind,
                name,
                &format!("terraform:{anchor_kind}:{ordinal}:{name}"),
                span,
            );
            push_member_edge(storage, file_id, file_id, node_id, span.start_line);
        }
        Ok(())
    })
}

fn parse_terraform(source: &str) -> Result<Tree, StructuralCollectionError> {
    let parse_budget = source
        .len()
        .saturating_mul(PARSE_PROGRESS_CALLS_PER_SOURCE_BYTE)
        .max(MIN_PARSE_PROGRESS_BUDGET);
    parse_terraform_with_progress_budget(source, parse_budget)
}

pub(super) fn parse_terraform_with_progress_budget(
    source: &str,
    parse_budget: usize,
) -> Result<Tree, StructuralCollectionError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_hcl::LANGUAGE.into())
        .map_err(|error| {
            StructuralCollectionError::Malformed(format!(
                "Terraform structural parser configuration failed: {error}"
            ))
        })?;

    let bytes = source.as_bytes();
    let mut progress_calls = 0usize;
    let mut progress = |_: &ParseState| {
        progress_calls = progress_calls.saturating_add(1);
        if progress_calls > parse_budget {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut progress);
    let mut read = |offset: usize, _position: Point| bytes.get(offset..).unwrap_or_default();
    parser
        .parse_with_options(&mut read, None, Some(options))
        .ok_or_else(|| {
            StructuralCollectionError::Malformed(format!(
                "Terraform parsing exceeded the {parse_budget}-step progress limit"
            ))
        })
}

fn walk_named_nodes<'tree, F>(
    root: SyntaxNode<'tree>,
    mut visit: F,
) -> Result<(), StructuralCollectionError>
where
    F: FnMut(SyntaxNode<'tree>) -> Result<(), StructuralCollectionError>,
{
    visit(root)?;
    let root_id = root.id();
    let mut cursor = root.walk();
    if !cursor.goto_first_child() {
        return Ok(());
    }
    loop {
        let node = cursor.node();
        if node.is_named() {
            visit(node)?;
            if cursor.goto_first_child() {
                continue;
            }
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() || cursor.node().id() == root_id {
                return Ok(());
            }
        }
    }
}

fn block_header_anchor<'source>(
    source: &'source str,
    block: SyntaxNode<'_>,
) -> Option<(&'source str, StructuralSourceSpan)> {
    let mut first = None;
    let mut last = None;
    let mut found_block_start = false;
    let mut cursor = block.walk();
    for child in block.named_children(&mut cursor) {
        match child.kind() {
            "block_start" => {
                found_block_start = true;
                break;
            }
            "identifier" | "string_lit" => {
                if first.is_none() {
                    first = Some(child);
                }
                last = Some(child);
            }
            _ => {}
        }
    }
    if !found_block_start {
        return None;
    }
    let start = first?.start_byte();
    let end = last?.end_byte();
    let name = source.get(start..end)?;
    Some((name, source_span(source, start, end)?))
}

fn direct_identifier(node: SyntaxNode<'_>) -> Option<SyntaxNode<'_>> {
    direct_named_child(node, "identifier")
}

fn simple_object_key(object_element: SyntaxNode<'_>) -> Option<SyntaxNode<'_>> {
    let key = object_element.child_by_field_name("key")?;
    if key.kind() != "expression" || key.named_child_count() != 1 {
        return None;
    }
    let variable = key.named_child(0)?;
    if variable.kind() != "variable_expr" || variable.named_child_count() != 1 {
        return None;
    }
    let identifier = variable.named_child(0)?;
    (identifier.kind() == "identifier").then_some(identifier)
}

fn direct_named_child<'tree>(node: SyntaxNode<'tree>, kind: &str) -> Option<SyntaxNode<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn source_anchor<'source>(
    source: &'source str,
    node: SyntaxNode<'_>,
) -> Option<(&'source str, StructuralSourceSpan)> {
    let name = source.get(node.byte_range())?;
    Some((
        name,
        source_span(source, node.start_byte(), node.end_byte())?,
    ))
}

fn source_span(source: &str, start: usize, end: usize) -> Option<StructuralSourceSpan> {
    if start >= end || end > source.len() {
        return None;
    }
    let (start_line, start_col) = byte_offset_line_col(source, start);
    let (end_line, end_col) = byte_offset_line_col(source, end.saturating_sub(1));
    Some(StructuralSourceSpan {
        start_line,
        start_col,
        end_line,
        end_col,
    })
}
