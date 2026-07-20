use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{NodeId, NodeKind};
use std::collections::HashMap;
use std::path::Path;

use super::common::{
    StructuralSourceSpan, push_annotation_usage_edge, push_member_edge, push_structural_node,
    push_synthetic_structural_node, push_type_usage_edge, push_usage_edge,
};

struct LocatedSqlIdentifier {
    value: String,
    start: usize,
    len: usize,
}

struct LocatedQualifiedName {
    schema: String,
    name: String,
    start: usize,
    len: usize,
}

pub(crate) fn collect_sql_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) {
    let default_schema = infer_default_schema(source);
    let schema_nodes = collect_schemas(source, file_id, storage, &default_schema);
    let mut tables: HashMap<String, NodeId> = HashMap::new();
    let mut views: HashMap<String, NodeId> = HashMap::new();

    for (line_idx, line_text) in source.lines().enumerate() {
        let line_number = line_idx as u32 + 1;
        let upper = line_text.trim().to_ascii_uppercase();
        if upper.starts_with("CREATE SCHEMA ") || upper.starts_with("CREATE DATABASE ") {
            continue;
        }
        if let Some(object) = parse_qualified_name_after_keyword(line_text, "CREATE TABLE") {
            let LocatedQualifiedName {
                schema,
                name,
                start,
                len,
            } = object;
            let schema_id = schema_nodes
                .get(&schema)
                .copied()
                .unwrap_or_else(|| default_schema_node(file_id, storage, &schema));
            let canonical = format!("sql:table:{schema}.{name}");
            let node_id = push_structural_node(
                storage,
                file_id,
                NodeKind::CLASS,
                &format!("{schema}.{name}"),
                &canonical,
                StructuralSourceSpan::token(line_number, start, len),
            );
            push_member_edge(storage, file_id, schema_id, node_id, line_number);
            tables.insert(format!("{schema}.{name}"), node_id);
            collect_inline_columns(
                line_text,
                file_id,
                storage,
                &schema,
                &name,
                node_id,
                line_number,
            );
        } else if let Some(object) = parse_qualified_name_after_keyword(line_text, "CREATE VIEW") {
            let LocatedQualifiedName {
                schema,
                name,
                start,
                len,
            } = object;
            let schema_id = schema_nodes
                .get(&schema)
                .copied()
                .unwrap_or_else(|| default_schema_node(file_id, storage, &schema));
            let canonical = format!("sql:view:{schema}.{name}");
            let node_id = push_structural_node(
                storage,
                file_id,
                NodeKind::CLASS,
                &format!("{schema}.{name}"),
                &canonical,
                StructuralSourceSpan::token(line_number, start, len),
            );
            push_member_edge(storage, file_id, schema_id, node_id, line_number);
            views.insert(format!("{schema}.{name}"), node_id);
            if let Some(base) = parse_view_base_table(line_text)
                && let Some(base_id) = tables.get(&base).copied()
            {
                push_type_usage_edge(storage, file_id, node_id, base_id, line_number);
            }
        } else if let Some((schema, table, index_name)) = parse_create_index(line_text) {
            if let Some(table_id) = tables.get(&format!("{schema}.{table}")).copied() {
                let canonical = format!("sql:index:{schema}.{table}.{}", index_name.value);
                let node_id = push_structural_node(
                    storage,
                    file_id,
                    NodeKind::ANNOTATION,
                    &index_name.value,
                    &canonical,
                    StructuralSourceSpan::token(line_number, index_name.start, index_name.len),
                );
                push_annotation_usage_edge(storage, file_id, node_id, table_id, line_number);
            }
        } else if let Some(object) =
            parse_qualified_name_after_keyword(line_text, "CREATE FUNCTION")
                .or_else(|| parse_qualified_name_after_keyword(line_text, "CREATE PROCEDURE"))
        {
            let LocatedQualifiedName {
                schema,
                name,
                start,
                len,
            } = object;
            let schema_id = schema_nodes
                .get(&schema)
                .copied()
                .unwrap_or_else(|| default_schema_node(file_id, storage, &schema));
            let canonical = format!("sql:func:{schema}.{name}");
            let node_id = push_structural_node(
                storage,
                file_id,
                NodeKind::FUNCTION,
                &format!("{schema}.{name}"),
                &canonical,
                StructuralSourceSpan::token(line_number, start, len),
            );
            push_member_edge(storage, file_id, schema_id, node_id, line_number);
            for table_key in referenced_tables(line_text, &schema) {
                if let Some(table_id) = tables.get(&table_key).copied() {
                    push_usage_edge(storage, file_id, node_id, table_id, line_number);
                }
            }
        }
    }

    let _ = (path, views);
}

fn infer_default_schema(source: &str) -> String {
    for line in source.lines() {
        let upper = line.trim().to_ascii_uppercase();
        if upper.starts_with("CREATE SCHEMA ")
            && let Some(name) = next_ident(line)
        {
            return name.value;
        }
        if upper.starts_with("SET SEARCH_PATH ")
            && let Some(name) = next_ident(line)
        {
            return name.value;
        }
    }
    "public".to_string()
}

fn collect_schemas(
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    default_schema: &str,
) -> HashMap<String, NodeId> {
    let mut schemas = HashMap::new();
    schemas.insert(
        default_schema.to_string(),
        default_schema_node(file_id, storage, default_schema),
    );
    for (line_idx, line_text) in source.lines().enumerate() {
        let line_number = line_idx as u32 + 1;
        let upper = line_text.trim().to_ascii_uppercase();
        if upper.starts_with("CREATE SCHEMA ")
            && let Some(name) = next_ident(line_text)
        {
            let canonical = format!("sql:schema:{}", name.value);
            let node_id = push_structural_node(
                storage,
                file_id,
                NodeKind::NAMESPACE,
                &name.value,
                &canonical,
                StructuralSourceSpan::token(line_number, name.start, name.len),
            );
            push_member_edge(storage, file_id, file_id, node_id, line_number);
            schemas.insert(name.value, node_id);
        }
    }
    schemas
}

fn default_schema_node(file_id: NodeId, storage: &mut IntermediateStorage, schema: &str) -> NodeId {
    let canonical = format!("sql:schema:{schema}");
    push_synthetic_structural_node(storage, file_id, NodeKind::NAMESPACE, schema, &canonical)
}

fn parse_qualified_name_after_keyword(line: &str, keyword: &str) -> Option<LocatedQualifiedName> {
    let lower = line.to_ascii_lowercase();
    let keyword_lower = keyword.to_ascii_lowercase();
    let idx = lower.find(&keyword_lower)?;
    let mut start = skip_ascii_whitespace(line, idx + keyword.len());
    if line[start..]
        .to_ascii_uppercase()
        .starts_with("IF NOT EXISTS")
    {
        start = skip_ascii_whitespace(line, start + "IF NOT EXISTS".len());
    }
    let identifier = located_sql_identifier(line, start)?;
    let (schema, name) = split_qualified_ident(&identifier.value)?;
    Some(LocatedQualifiedName {
        schema,
        name,
        start: identifier.start,
        len: identifier.len,
    })
}

fn split_qualified_ident(text: &str) -> Option<(String, String)> {
    let token = take_sql_ident(text)?;
    if let Some((schema, name)) = token.split_once('.') {
        Some((schema.to_string(), name.to_string()))
    } else {
        Some(("public".to_string(), token.to_string()))
    }
}

fn take_sql_ident(text: &str) -> Option<String> {
    located_sql_identifier(text, 0).map(|identifier| identifier.value)
}

fn next_ident(line: &str) -> Option<LocatedSqlIdentifier> {
    let trimmed = line.trim();
    let upper = trimmed.to_ascii_uppercase();
    for keyword in [
        "CREATE SCHEMA",
        "CREATE DATABASE",
        "SET SEARCH_PATH TO",
        "SET SEARCH_PATH",
    ] {
        if upper.starts_with(keyword) {
            let leading = line.len().saturating_sub(trimmed.len());
            return located_sql_identifier(line, leading + keyword.len());
        }
    }
    None
}

fn located_sql_identifier(line: &str, from: usize) -> Option<LocatedSqlIdentifier> {
    let start = skip_ascii_whitespace(line, from);
    let rest = line.get(start..)?;
    let first = rest.chars().next()?;
    if matches!(first, '"' | '\'' | '`') {
        let inner_start = start + first.len_utf8();
        let end = line[inner_start..].find(first)? + inner_start;
        return (end > inner_start).then(|| LocatedSqlIdentifier {
            value: line[inner_start..end].to_string(),
            start: inner_start,
            len: end - inner_start,
        });
    }
    let len = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
        .unwrap_or(rest.len());
    (len > 0).then(|| LocatedSqlIdentifier {
        value: rest[..len].to_string(),
        start,
        len,
    })
}

fn skip_ascii_whitespace(line: &str, mut index: usize) -> usize {
    while line
        .as_bytes()
        .get(index)
        .is_some_and(u8::is_ascii_whitespace)
    {
        index += 1;
    }
    index
}

fn collect_inline_columns(
    line: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    schema: &str,
    table: &str,
    table_id: NodeId,
    line_no: u32,
) {
    if !line.contains('(') {
        return;
    }
    let Some(start) = line.find('(') else {
        return;
    };
    let Some(end) = line.rfind(')') else {
        return;
    };
    let body_start = start + 1;
    let body = &line[body_start..end];
    let mut part_start = 0usize;
    for raw_part in body.split(',') {
        let absolute_part_start = body_start + part_start;
        let Some(identifier) = located_sql_identifier(line, absolute_part_start) else {
            part_start = part_start.saturating_add(raw_part.len()).saturating_add(1);
            continue;
        };
        if identifier.value.eq_ignore_ascii_case("CONSTRAINT") {
            part_start = part_start.saturating_add(raw_part.len()).saturating_add(1);
            continue;
        }
        let col = identifier.value;
        let canonical = format!("sql:column:{schema}.{table}.{col}");
        let node_id = push_structural_node(
            storage,
            file_id,
            NodeKind::FIELD,
            &col,
            &canonical,
            StructuralSourceSpan::token(line_no, identifier.start, identifier.len),
        );
        push_member_edge(storage, file_id, table_id, node_id, line_no);
        part_start = part_start.saturating_add(raw_part.len()).saturating_add(1);
    }
}

fn parse_create_index(line: &str) -> Option<(String, String, LocatedSqlIdentifier)> {
    let upper = line.trim().to_ascii_uppercase();
    if !upper.starts_with("CREATE ") || !upper.contains(" INDEX ") {
        return None;
    }
    let index_name = next_token_after(line, "INDEX")?;
    let table_part = line.to_ascii_uppercase();
    let on_idx = table_part.find(" ON ")?;
    let table_ref = line[on_idx + 4..].trim();
    let (schema, table) = split_qualified_ident(table_ref)?;
    Some((schema, table, index_name))
}

fn next_token_after(line: &str, keyword: &str) -> Option<LocatedSqlIdentifier> {
    let upper = line.to_ascii_uppercase();
    let idx = upper.find(&keyword.to_ascii_uppercase())?;
    located_sql_identifier(line, idx + keyword.len())
}

fn parse_view_base_table(line: &str) -> Option<String> {
    let upper = line.to_ascii_uppercase();
    let idx = upper.find(" FROM ")?;
    let rest = line[idx + 6..].trim();
    let (schema, name) = split_qualified_ident(rest)?;
    Some(format!("{schema}.{name}"))
}

fn referenced_tables(line: &str, default_schema: &str) -> Vec<String> {
    let upper = line.to_ascii_uppercase();
    let mut tables = Vec::new();
    for keyword in [" FROM ", " JOIN ", " INTO ", " UPDATE "] {
        let mut search = 0usize;
        while let Some(rel) = upper[search..].find(keyword) {
            let idx = search + rel + keyword.len();
            let rest = line[idx..].trim();
            if let Some((schema, name)) = split_qualified_ident(rest) {
                let schema = if schema == "public" && !rest.contains('.') {
                    default_schema.to_string()
                } else {
                    schema
                };
                tables.push(format!("{schema}.{name}"));
            }
            search = idx + 1;
        }
    }
    tables
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intermediate_storage::IntermediateStorage;
    use codestory_contracts::graph::{EdgeKind, NodeKind};
    use std::path::Path;

    #[test]
    fn collects_schema_table_column_and_index() {
        let sql = r#"
CREATE SCHEMA app;
CREATE TABLE app.users (id INT PRIMARY KEY, email TEXT);
CREATE INDEX users_email_idx ON app.users (email);
"#;
        let mut storage = IntermediateStorage::default();
        let file_id = NodeId(7);
        collect_sql_entities(Path::new("schema.sql"), sql, file_id, &mut storage);
        assert!(storage.nodes.iter().any(|n| n.kind == NodeKind::NAMESPACE
            && n.canonical_id.as_deref() == Some("sql:schema:app")));
        assert!(
            storage
                .nodes
                .iter()
                .any(|n| n.canonical_id.as_deref() == Some("sql:table:app.users"))
        );
        assert!(storage.nodes.iter().any(|n| n.kind == NodeKind::FIELD));
        assert!(storage.edges.iter().any(|e| e.kind == EdgeKind::MEMBER));
        assert!(
            storage
                .edges
                .iter()
                .any(|e| e.kind == EdgeKind::ANNOTATION_USAGE)
        );
    }
}
