use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{NodeId, NodeKind};
use std::collections::HashMap;
use std::path::Path;

use super::common::{
    push_annotation_usage_edge, push_member_edge, push_structural_node, push_type_usage_edge,
    push_usage_edge,
};

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
        if let Some((schema, name)) = parse_qualified_name_after_keyword(line_text, "CREATE TABLE")
        {
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
                line_number,
                1,
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
        } else if let Some((schema, name)) =
            parse_qualified_name_after_keyword(line_text, "CREATE VIEW")
        {
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
                line_number,
                1,
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
                let canonical = format!("sql:index:{schema}.{table}.{index_name}");
                let node_id = push_structural_node(
                    storage,
                    file_id,
                    NodeKind::ANNOTATION,
                    &index_name,
                    &canonical,
                    line_number,
                    1,
                );
                push_annotation_usage_edge(storage, file_id, node_id, table_id, line_number);
            }
        } else if let Some((schema, name)) =
            parse_qualified_name_after_keyword(line_text, "CREATE FUNCTION")
                .or_else(|| parse_qualified_name_after_keyword(line_text, "CREATE PROCEDURE"))
        {
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
                line_number,
                1,
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
            return name;
        }
        if upper.starts_with("SET SEARCH_PATH ")
            && let Some(name) = next_ident(line)
        {
            return name;
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
            let canonical = format!("sql:schema:{name}");
            let node_id = push_structural_node(
                storage,
                file_id,
                NodeKind::NAMESPACE,
                &name,
                &canonical,
                line_number,
                1,
            );
            push_member_edge(storage, file_id, file_id, node_id, line_number);
            schemas.insert(name, node_id);
        }
    }
    schemas
}

fn default_schema_node(file_id: NodeId, storage: &mut IntermediateStorage, schema: &str) -> NodeId {
    let canonical = format!("sql:schema:{schema}");
    push_structural_node(
        storage,
        file_id,
        NodeKind::NAMESPACE,
        schema,
        &canonical,
        1,
        1,
    )
}

fn parse_qualified_name_after_keyword(line: &str, keyword: &str) -> Option<(String, String)> {
    let lower = line.to_ascii_lowercase();
    let keyword_lower = keyword.to_ascii_lowercase();
    let idx = lower.find(&keyword_lower)?;
    let rest = line[idx + keyword.len()..].trim();
    let rest = rest.trim_start_matches("IF NOT EXISTS").trim();
    let (schema, name) = split_qualified_ident(rest)?;
    Some((schema, name))
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
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let first = trimmed.chars().next()?;
    if first == '"' || first == '\'' || first == '`' {
        let end = trimmed[1..].find(first)?;
        let ident = &trimmed[1..1 + end];
        return (!ident.is_empty()).then(|| ident.to_string());
    }
    let end = trimmed
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
        .unwrap_or(trimmed.len());
    if end == 0 {
        return None;
    }
    Some(trimmed[..end].to_string())
}

fn next_ident(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let upper = trimmed.to_ascii_uppercase();
    if let Some(rest) = upper.strip_prefix("CREATE SCHEMA ") {
        return take_sql_ident(trimmed[trimmed.len() - rest.len()..].trim());
    }
    if let Some(rest) = upper.strip_prefix("CREATE DATABASE ") {
        return take_sql_ident(trimmed[trimmed.len() - rest.len()..].trim());
    }
    if let Some(rest) = upper.strip_prefix("SET SEARCH_PATH TO ") {
        return take_sql_ident(trimmed[trimmed.len() - rest.len()..].trim());
    }
    if let Some(rest) = upper.strip_prefix("SET SEARCH_PATH ") {
        return take_sql_ident(trimmed[trimmed.len() - rest.len()..].trim());
    }
    None
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
    let body = &line[start + 1..end];
    for part in body.split(',') {
        let part = part.trim();
        if part.is_empty() || part.to_ascii_uppercase().starts_with("CONSTRAINT") {
            continue;
        }
        let col = part
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches('"');
        if col.is_empty() {
            continue;
        }
        let canonical = format!("sql:column:{schema}.{table}.{col}");
        let node_id = push_structural_node(
            storage,
            file_id,
            NodeKind::FIELD,
            col,
            &canonical,
            line_no,
            1,
        );
        push_member_edge(storage, file_id, table_id, node_id, line_no);
    }
}

fn parse_create_index(line: &str) -> Option<(String, String, String)> {
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

fn next_token_after(line: &str, keyword: &str) -> Option<String> {
    let upper = line.to_ascii_uppercase();
    let idx = upper.find(&keyword.to_ascii_uppercase())?;
    let rest = line[idx + keyword.len()..].trim();
    take_sql_ident(rest)
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
