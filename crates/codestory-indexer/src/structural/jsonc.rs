use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{NodeId, NodeKind};
use jsonc_parser::ast::Value;
use jsonc_parser::common::Ranged;
use jsonc_parser::{CollectOptions, ParseOptions, parse_to_ast};
use std::path::Path;

use super::StructuralCollectionError;
use super::blanking::byte_offset_line_col;
use super::common::{StructuralSourceSpan, push_member_edge, push_structural_node};

pub(crate) fn collect_typescript_config_jsonc_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) -> Result<(), StructuralCollectionError> {
    let options = ParseOptions {
        allow_comments: true,
        allow_loose_object_property_names: false,
        allow_trailing_commas: true,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    };
    let parsed = parse_to_ast(source, &CollectOptions::default(), &options).map_err(|error| {
        StructuralCollectionError::Malformed(format!(
            "invalid TypeScript config JSONC syntax: {error}"
        ))
    })?;
    let value = parsed.value.ok_or_else(|| {
        StructuralCollectionError::Malformed(
            "invalid TypeScript config JSONC syntax: expected one JSON value".to_string(),
        )
    })?;
    let mut ordinal = 0usize;
    collect_value_keys(path, source, file_id, storage, &value, &mut ordinal);
    Ok(())
}

fn collect_value_keys(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    value: &Value<'_>,
    ordinal: &mut usize,
) {
    match value {
        Value::Object(object) => {
            for property in &object.properties {
                *ordinal += 1;
                let range = property.name.range();
                let (line, column) = byte_offset_line_col(source, range.start);
                let node_id = push_structural_node(
                    storage,
                    file_id,
                    NodeKind::ANNOTATION,
                    property.name.as_str(),
                    &format!(
                        "structural-jsonc:{}:object-key:{}:{}:{}",
                        path.to_string_lossy().replace('\\', "/"),
                        *ordinal,
                        line,
                        column
                    ),
                    StructuralSourceSpan::token(
                        line,
                        column.saturating_sub(1) as usize,
                        range.end.saturating_sub(range.start),
                    ),
                );
                push_member_edge(storage, file_id, file_id, node_id, line);
                collect_value_keys(path, source, file_id, storage, &property.value, ordinal);
            }
        }
        Value::Array(array) => {
            for element in &array.elements {
                collect_value_keys(path, source, file_id, storage, element, ordinal);
            }
        }
        Value::StringLit(_)
        | Value::NumberLit(_)
        | Value::BooleanLit(_)
        | Value::NullKeyword(_) => {}
    }
}
