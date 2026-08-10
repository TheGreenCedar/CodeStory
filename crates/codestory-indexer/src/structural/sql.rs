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

struct TableDefinition {
    schema: String,
    name: String,
    node_id: NodeId,
    header_offset: usize,
}

struct PendingForeignKey {
    owner_table_id: NodeId,
    owner_key: String,
    referenced_key: String,
    line_number: u32,
    start: usize,
    len: usize,
}

/// Blank out SQL comments while preserving every byte offset.
///
/// The collector is line-oriented and records exact byte spans, so comments are
/// overwritten with spaces rather than removed: `-- CREATE TABLE old_users (id
/// INT);` used to mint a real table node with a MEMBER edge and inline column
/// fields, and prose like `-- create table statements below` used to mint a
/// table called `statements` (CR-011). Quoting is tracked so a `--` or `/*`
/// inside a string literal or a quoted identifier is left alone, and block
/// comments carry their state across lines.
fn mask_sql_comments(source: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Scan {
        Code,
        Quoted(u8),
        BracketQuoted,
        BlockComment,
    }

    let bytes = source.as_bytes();
    // Every byte of a masked run is overwritten, so a multi-byte character
    // inside a comment becomes that many spaces and the result stays valid
    // UTF-8 at exactly the original length.
    let mut output = bytes.to_vec();
    let mut state = Scan::Code;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        match state {
            Scan::Code => match (byte, next) {
                (b'-', Some(b'-')) => {
                    while index < bytes.len() && bytes[index] != b'\n' {
                        output[index] = b' ';
                        index += 1;
                    }
                }
                (b'/', Some(b'*')) => {
                    state = Scan::BlockComment;
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 2;
                }
                (b'\'', _) | (b'"', _) | (b'`', _) => {
                    state = Scan::Quoted(byte);
                    index += 1;
                }
                (b'[', _) => {
                    state = Scan::BracketQuoted;
                    index += 1;
                }
                _ => index += 1,
            },
            Scan::Quoted(quote) => {
                if byte == quote {
                    state = Scan::Code;
                }
                index += 1;
            }
            Scan::BracketQuoted => {
                if byte == b']' {
                    if next == Some(b']') {
                        index += 1;
                    } else {
                        state = Scan::Code;
                    }
                }
                index += 1;
            }
            Scan::BlockComment => {
                if byte == b'*' && next == Some(b'/') {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    state = Scan::Code;
                    index += 2;
                } else {
                    if byte != b'\n' && byte != b'\r' {
                        output[index] = b' ';
                    }
                    index += 1;
                }
            }
        }
    }
    String::from_utf8(output).unwrap_or_else(|_| source.to_string())
}

pub(crate) fn collect_sql_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
) {
    let masked = mask_sql_comments(source);
    let source = masked.as_str();
    let lines = source.lines().collect::<Vec<_>>();
    let line_offsets = source_line_offsets(source);
    let default_schema = infer_default_schema(source);
    let schema_nodes = collect_schemas(source, file_id, storage, &default_schema);
    let mut tables: HashMap<String, Vec<NodeId>> = HashMap::new();
    let mut table_definitions = Vec::new();

    // Collect table headers first so a foreign key can resolve a table declared
    // later in the same schema file. Structural relationships remain local to a
    // file; we never guess across files or from an unresolved name.
    for (line_idx, line_text) in lines.iter().enumerate() {
        let line_number = line_idx as u32 + 1;
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
            let table_key = format!("{schema}.{name}");
            tables.entry(table_key).or_default().push(node_id);
            table_definitions.push(TableDefinition {
                schema,
                name,
                node_id,
                header_offset: line_offsets[line_idx]
                    .saturating_add(start)
                    .saturating_add(len),
            });
        }
    }

    let mut pending_foreign_keys = Vec::new();
    for table in &table_definitions {
        collect_table_body(
            source,
            &line_offsets,
            table,
            file_id,
            storage,
            &mut pending_foreign_keys,
        );
    }
    collect_alter_table_foreign_keys(&lines, &tables, &mut pending_foreign_keys);

    for foreign_key in pending_foreign_keys {
        let canonical = format!(
            "sql:foreign_key:{}:{}",
            foreign_key.owner_key, foreign_key.referenced_key
        );
        let foreign_key_id = push_structural_node(
            storage,
            file_id,
            NodeKind::ANNOTATION,
            "FOREIGN KEY",
            &canonical,
            StructuralSourceSpan::token(
                foreign_key.line_number,
                foreign_key.start,
                foreign_key.len,
            ),
        );
        push_member_edge(
            storage,
            file_id,
            foreign_key.owner_table_id,
            foreign_key_id,
            foreign_key.line_number,
        );
        if let Some(referenced_table_id) = unique_table_id(&tables, &foreign_key.referenced_key) {
            push_annotation_usage_edge(
                storage,
                file_id,
                foreign_key_id,
                referenced_table_id,
                foreign_key.line_number,
            );
        }
    }

    for (line_idx, line_text) in lines.iter().enumerate() {
        let line_number = line_idx as u32 + 1;
        let upper = line_text.trim().to_ascii_uppercase();
        if upper.starts_with("CREATE SCHEMA ")
            || upper.starts_with("CREATE DATABASE ")
            || parse_qualified_name_after_keyword(line_text, "CREATE TABLE").is_some()
        {
            continue;
        }
        if let Some(object) = parse_qualified_name_after_keyword(line_text, "CREATE VIEW") {
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
            if let Some(base) = parse_view_base_table(line_text)
                && let Some(base_id) = unique_table_id(&tables, &base)
            {
                push_type_usage_edge(storage, file_id, node_id, base_id, line_number);
            }
        } else if let Some((schema, table, index_name)) = parse_create_index(line_text) {
            if let Some(table_id) = unique_table_id(&tables, &format!("{schema}.{table}")) {
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
                if let Some(table_id) = unique_table_id(&tables, &table_key) {
                    push_usage_edge(storage, file_id, node_id, table_id, line_number);
                }
            }
        }
    }

    let _ = path;
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

/// Byte offsets on this line at which a SQL statement can begin.
///
/// One offset for the first non-whitespace byte, and one after every `;`.
/// Anything else on the line is inside a statement.
fn statement_start_offsets(line: &str) -> Vec<usize> {
    let mut offsets = vec![skip_ascii_whitespace(line, 0)];
    for (index, byte) in line.as_bytes().iter().enumerate() {
        if *byte == b';' {
            offsets.push(skip_ascii_whitespace(line, index + 1));
        }
    }
    offsets.retain(|offset| *offset < line.len());
    offsets
}

/// Locate `keyword` only where a statement starts.
///
/// The lookup used to be an unanchored `find`, so any line mentioning the
/// keyword produced a schema object: a string literal holding dynamic DDL, a
/// `COMMENT ON` body, a continuation line quoting the phrase (CR-011). Comment
/// masking removes one source of those; anchoring removes the rest. `CREATE
/// INDEX` is unaffected — `parse_create_index` was already anchored separately.
fn parse_qualified_name_after_keyword(line: &str, keyword: &str) -> Option<LocatedQualifiedName> {
    let lower = line.to_ascii_lowercase();
    let keyword_lower = keyword.to_ascii_lowercase();
    let idx = statement_start_offsets(line).into_iter().find(|offset| {
        lower[*offset..].starts_with(&keyword_lower)
            && line
                .as_bytes()
                .get(offset + keyword.len())
                .is_none_or(|byte| byte.is_ascii_whitespace() || *byte == b'(')
    })?;
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
    let trimmed_start = line.trim_start();
    let upper = trimmed_start.to_ascii_uppercase();
    for keyword in [
        "CREATE SCHEMA",
        "CREATE DATABASE",
        "SET SEARCH_PATH TO",
        "SET SEARCH_PATH",
    ] {
        if upper.starts_with(keyword) {
            let leading = line.len().saturating_sub(trimmed_start.len());
            return located_sql_identifier(line, leading + keyword.len());
        }
    }
    None
}

fn located_sql_identifier(line: &str, from: usize) -> Option<LocatedSqlIdentifier> {
    let start = skip_ascii_whitespace(line, from);
    let rest = line.get(start..)?;
    let first = rest.chars().next()?;
    if matches!(first, '"' | '\'' | '`' | '[') {
        let inner_start = start + first.len_utf8();
        let close = if first == '[' { ']' } else { first };
        let end = line[inner_start..].find(close)? + inner_start;
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

fn source_line_offsets(source: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    offsets.extend(
        source
            .bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
    );
    offsets
}

fn source_location_for_offset(line_offsets: &[usize], offset: usize) -> (u32, usize) {
    let line_index = line_offsets.partition_point(|line_start| *line_start <= offset) - 1;
    (
        line_index as u32 + 1,
        offset.saturating_sub(line_offsets[line_index]),
    )
}

fn unique_table_id(tables: &HashMap<String, Vec<NodeId>>, key: &str) -> Option<NodeId> {
    let table_ids = tables.get(key)?;
    (table_ids.len() == 1).then_some(table_ids[0])
}

fn collect_table_body(
    source: &str,
    line_offsets: &[usize],
    table: &TableDefinition,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    pending_foreign_keys: &mut Vec<PendingForeignKey>,
) {
    let Some((body_start, body_end)) = table_body_range(source, table.header_offset) else {
        return;
    };
    let owner_key = format!("{}.{}", table.schema, table.name);
    for (segment_start, segment_end) in table_body_segments(source, body_start, body_end) {
        let segment = &source[segment_start..segment_end];
        if let Some(foreign_key) = parse_foreign_key(segment, &table.schema) {
            let foreign_offset = segment_start.saturating_add(foreign_key.start);
            let (line_number, start) = source_location_for_offset(line_offsets, foreign_offset);
            pending_foreign_keys.push(PendingForeignKey {
                owner_table_id: table.node_id,
                owner_key: owner_key.clone(),
                referenced_key: foreign_key.referenced_key,
                line_number,
                start,
                len: foreign_key.len,
            });
            continue;
        }
        let Some(identifier) = located_sql_identifier(segment, 0) else {
            continue;
        };
        if sql_table_body_keyword(&identifier.value) {
            continue;
        }
        let col = identifier.value;
        let column_offset = segment_start.saturating_add(identifier.start);
        let (line_number, start) = source_location_for_offset(line_offsets, column_offset);
        let canonical = format!("sql:column:{}.{}.{col}", table.schema, table.name);
        let node_id = push_structural_node(
            storage,
            file_id,
            NodeKind::FIELD,
            &col,
            &canonical,
            StructuralSourceSpan::token(line_number, start, identifier.len),
        );
        push_member_edge(storage, file_id, table.node_id, node_id, line_number);
    }
}

fn table_body_range(source: &str, header_offset: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut state = SqlScanState::Code;
    let mut opened = false;
    let mut depth = 0usize;
    let mut body_start = None;
    let mut index = header_offset;
    while index < bytes.len() {
        match state {
            SqlScanState::Code => match bytes[index] {
                b'\'' => state = SqlScanState::Quoted(b'\''),
                b'"' => state = SqlScanState::Quoted(b'"'),
                b'`' => state = SqlScanState::Quoted(b'`'),
                b'[' => state = SqlScanState::BracketQuoted,
                b'(' => {
                    if !opened {
                        body_start = Some(index + 1);
                    }
                    opened = true;
                    depth = depth.saturating_add(1);
                }
                b')' if opened => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return body_start.map(|body_start| (body_start, index));
                    }
                }
                b';' if !opened => return None,
                _ => {}
            },
            SqlScanState::Quoted(quote) => {
                if bytes[index] == quote {
                    if bytes.get(index + 1) == Some(&quote) {
                        index += 1;
                    } else {
                        state = SqlScanState::Code;
                    }
                }
            }
            SqlScanState::BracketQuoted => {
                if bytes[index] == b']' {
                    if bytes.get(index + 1) == Some(&b']') {
                        index += 1;
                    } else {
                        state = SqlScanState::Code;
                    }
                }
            }
        }
        index += 1;
    }
    None
}

fn table_body_segments(source: &str, body_start: usize, body_end: usize) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut state = SqlScanState::Code;
    let mut depth = 0usize;
    let mut segment_start = body_start;
    let mut segments = Vec::new();
    let mut index = body_start;
    while index < body_end {
        match state {
            SqlScanState::Code => match bytes[index] {
                b'\'' => state = SqlScanState::Quoted(b'\''),
                b'"' => state = SqlScanState::Quoted(b'"'),
                b'`' => state = SqlScanState::Quoted(b'`'),
                b'[' => state = SqlScanState::BracketQuoted,
                b'(' => depth = depth.saturating_add(1),
                b')' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    segments.push((segment_start, index));
                    segment_start = index + 1;
                }
                _ => {}
            },
            SqlScanState::Quoted(quote) => {
                if bytes[index] == quote {
                    if bytes.get(index + 1) == Some(&quote) {
                        index += 1;
                    } else {
                        state = SqlScanState::Code;
                    }
                }
            }
            SqlScanState::BracketQuoted => {
                if bytes[index] == b']' {
                    if bytes.get(index + 1) == Some(&b']') {
                        index += 1;
                    } else {
                        state = SqlScanState::Code;
                    }
                }
            }
        }
        index += 1;
    }
    segments.push((segment_start, body_end));
    segments
}

#[derive(Clone, Copy)]
enum SqlScanState {
    Code,
    Quoted(u8),
    BracketQuoted,
}

fn sql_table_body_keyword(identifier: &str) -> bool {
    matches!(
        identifier.to_ascii_uppercase().as_str(),
        "CONSTRAINT" | "FOREIGN" | "PRIMARY" | "UNIQUE" | "CHECK" | "KEY" | "INDEX"
    )
}

struct ParsedForeignKey {
    referenced_key: String,
    start: usize,
    len: usize,
}

fn parse_foreign_key(line: &str, default_schema: &str) -> Option<ParsedForeignKey> {
    let mut cursor = skip_ascii_whitespace(line, 0);
    if starts_sql_keyword(line, cursor, "CONSTRAINT") {
        cursor = skip_ascii_whitespace(line, cursor + "CONSTRAINT".len());
        let constraint = located_sql_identifier(line, cursor)?;
        cursor = skip_ascii_whitespace(line, constraint.start + constraint.len);
        while matches!(
            line.as_bytes().get(cursor),
            Some(b']' | b'"' | b'\'' | b'`')
        ) {
            cursor = skip_ascii_whitespace(line, cursor.saturating_add(1));
        }
    }
    if !starts_sql_keyword(line, cursor, "FOREIGN") {
        return None;
    }
    let foreign_start = cursor;
    cursor = skip_ascii_whitespace(line, cursor + "FOREIGN".len());
    if !starts_sql_keyword(line, cursor, "KEY") {
        return None;
    }
    cursor = skip_ascii_whitespace(line, cursor + "KEY".len());
    if line.as_bytes().get(cursor) != Some(&b'(') {
        return None;
    }
    let local_end = line[cursor + 1..].find(')')? + cursor + 1;
    cursor = skip_ascii_whitespace(line, local_end + 1);
    if !starts_sql_keyword(line, cursor, "REFERENCES") {
        return None;
    }
    cursor = skip_ascii_whitespace(line, cursor + "REFERENCES".len());
    let referenced = located_sql_identifier(line, cursor)?;
    let (schema, name) = split_qualified_ident(&referenced.value)?;
    let schema = if schema == "public" && !referenced.value.contains('.') {
        default_schema.to_string()
    } else {
        schema
    };
    Some(ParsedForeignKey {
        referenced_key: format!("{schema}.{name}"),
        start: foreign_start,
        len: "FOREIGN KEY".len(),
    })
}

fn collect_alter_table_foreign_keys(
    lines: &[&str],
    tables: &HashMap<String, Vec<NodeId>>,
    pending_foreign_keys: &mut Vec<PendingForeignKey>,
) {
    let mut alter_table: Option<(NodeId, String, String)> = None;
    for (line_index, line) in lines.iter().enumerate() {
        if let Some(object) = parse_qualified_name_after_keyword(line, "ALTER TABLE") {
            let owner_key = format!("{}.{}", object.schema, object.name);
            alter_table = unique_table_id(tables, &owner_key)
                .map(|node_id| (node_id, owner_key, object.schema));
        }
        if let Some((owner_table_id, owner_key, schema)) = alter_table.as_ref()
            && let Some(foreign_key) = parse_foreign_key(line, schema)
        {
            pending_foreign_keys.push(PendingForeignKey {
                owner_table_id: *owner_table_id,
                owner_key: owner_key.clone(),
                referenced_key: foreign_key.referenced_key,
                line_number: line_index as u32 + 1,
                start: foreign_key.start,
                len: foreign_key.len,
            });
        }
        if alter_table.is_some() && line.contains(';') {
            alter_table = None;
        }
    }
}

fn starts_sql_keyword(line: &str, start: usize, keyword: &str) -> bool {
    line.get(start..).is_some_and(|tail| {
        tail.get(..keyword.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(keyword))
            && tail
                .as_bytes()
                .get(keyword.len())
                .is_none_or(|byte| byte.is_ascii_whitespace() || *byte == b'(')
    })
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

    #[test]
    fn inline_table_columns_are_collected_as_distinct_members() {
        let storage = collect("CREATE TABLE app.entries (id INTEGER, label TEXT, owner_id INT);\n");
        let fields = storage
            .nodes
            .iter()
            .filter_map(|node| node.canonical_id.as_deref())
            .filter(|canonical| canonical.starts_with("sql:column:app.entries."))
            .collect::<Vec<_>>();
        assert_eq!(
            fields,
            vec![
                "sql:column:app.entries.id",
                "sql:column:app.entries.label",
                "sql:column:app.entries.owner_id",
            ]
        );
    }

    #[test]
    fn table_body_segments_keep_quoted_and_nested_commas_inside_one_field() {
        let storage = collect(
            "CREATE TABLE app.entries (\n\
             id INTEGER,\n\
             label TEXT DEFAULT 'north, south (archived)',\n\
             total NUMERIC(10, 2),\n\
             CHECK (length(label) > 0)\n\
             );\n",
        );
        let fields = storage
            .nodes
            .iter()
            .filter_map(|node| node.canonical_id.as_deref())
            .filter(|canonical| canonical.starts_with("sql:column:app.entries."))
            .collect::<Vec<_>>();
        assert_eq!(
            fields,
            vec![
                "sql:column:app.entries.id",
                "sql:column:app.entries.label",
                "sql:column:app.entries.total",
            ]
        );
    }

    #[test]
    fn duplicate_target_tables_leave_foreign_keys_unlinked() {
        let storage = collect(
            "CREATE TABLE app.target (id INTEGER);\n\
             CREATE TABLE app.target (id INTEGER);\n\
             CREATE TABLE app.source (target_id INTEGER, \
             FOREIGN KEY (target_id) REFERENCES app.target (id));\n",
        );
        let foreign_key = storage
            .nodes
            .iter()
            .find(|node| {
                node.canonical_id.as_deref() == Some("sql:foreign_key:app.source:app.target")
            })
            .expect("foreign-key evidence remains visible");
        assert!(
            !storage.edges.iter().any(|edge| {
                edge.kind == EdgeKind::ANNOTATION_USAGE && edge.source == foreign_key.id
            }),
            "an ambiguous target must not receive an arbitrary relationship edge"
        );
    }

    #[test]
    fn bracket_quoted_multiline_tables_and_foreign_keys_keep_exact_structural_spans() {
        let source = r#"
CREATE TABLE [ledger_entry]
(
    [entry_id] INTEGER PRIMARY KEY,
    [account_id] INTEGER NOT NULL,
    FOREIGN KEY ([account_id]) REFERENCES [account] ([account_id])
);
CREATE TABLE [account]
(
    [account_id] INTEGER PRIMARY KEY
);
"#;
        let storage = collect(source);
        let entry = storage
            .nodes
            .iter()
            .find(|node| node.canonical_id.as_deref() == Some("sql:table:public.ledger_entry"))
            .expect("bracket-quoted table");
        let account = storage
            .nodes
            .iter()
            .find(|node| node.canonical_id.as_deref() == Some("sql:table:public.account"))
            .expect("forward table");
        let field = storage
            .nodes
            .iter()
            .find(|node| {
                node.canonical_id.as_deref() == Some("sql:column:public.ledger_entry.account_id")
            })
            .expect("multiline field");
        let foreign_key = storage
            .nodes
            .iter()
            .find(|node| {
                node.kind == NodeKind::ANNOTATION
                    && node.canonical_id.as_deref()
                        == Some("sql:foreign_key:public.ledger_entry:public.account")
            })
            .expect("foreign-key annotation");

        let field_line = source.lines().nth(4).expect("field line");
        let field_start = field.start_col.expect("field start") as usize - 1;
        let field_end = field.end_col.expect("field end") as usize;
        assert_eq!(
            &field_line.as_bytes()[field_start..field_end],
            b"account_id"
        );

        let foreign_key_line = source.lines().nth(5).expect("foreign-key line");
        let foreign_key_start = foreign_key.start_col.expect("foreign-key start") as usize - 1;
        let foreign_key_end = foreign_key.end_col.expect("foreign-key end") as usize;
        assert_eq!(
            &foreign_key_line.as_bytes()[foreign_key_start..foreign_key_end],
            b"FOREIGN KEY"
        );
        assert!(storage.edges.iter().any(|edge| {
            edge.kind == EdgeKind::MEMBER
                && edge.source == entry.id
                && edge.target == foreign_key.id
        }));
        assert!(storage.edges.iter().any(|edge| {
            edge.kind == EdgeKind::ANNOTATION_USAGE
                && edge.source == foreign_key.id
                && edge.target == account.id
        }));
    }

    #[test]
    fn alter_table_foreign_key_connects_existing_tables() {
        let source = r#"
CREATE TABLE parent_item (id INTEGER PRIMARY KEY);
CREATE TABLE child_item (parent_id INTEGER NOT NULL);
ALTER TABLE child_item ADD CONSTRAINT child_parent_fk
    FOREIGN KEY (parent_id) REFERENCES parent_item (id);
"#;
        let storage = collect(source);
        let child = storage
            .nodes
            .iter()
            .find(|node| node.canonical_id.as_deref() == Some("sql:table:public.child_item"))
            .expect("child table");
        let parent = storage
            .nodes
            .iter()
            .find(|node| node.canonical_id.as_deref() == Some("sql:table:public.parent_item"))
            .expect("parent table");
        let foreign_key = storage
            .nodes
            .iter()
            .find(|node| {
                node.canonical_id.as_deref()
                    == Some("sql:foreign_key:public.child_item:public.parent_item")
            })
            .expect("alter-table foreign key");
        assert!(storage.edges.iter().any(|edge| {
            edge.kind == EdgeKind::MEMBER
                && edge.source == child.id
                && edge.target == foreign_key.id
        }));
        assert!(storage.edges.iter().any(|edge| {
            edge.kind == EdgeKind::ANNOTATION_USAGE
                && edge.source == foreign_key.id
                && edge.target == parent.id
        }));
    }

    #[test]
    fn commented_foreign_keys_do_not_mint_relationships() {
        let storage = collect(
            "CREATE TABLE source_item (\n\
             -- FOREIGN KEY (target_id) REFERENCES target_item (id)\n\
             target_id INTEGER\n\
             );\n\
             CREATE TABLE target_item (id INTEGER);\n",
        );
        assert!(
            !storage.nodes.iter().any(|node| {
                node.canonical_id
                    .as_deref()
                    .is_some_and(|canonical| canonical.starts_with("sql:foreign_key:"))
            }),
            "commented relationship must not be collected"
        );
    }

    #[test]
    fn indented_schema_database_and_search_path_identifiers_keep_exact_byte_spans() {
        for (line, expected) in [
            ("  CREATE SCHEMA app;  ", "app"),
            ("\tCREATE DATABASE warehouse;   ", "warehouse"),
            ("    SET SEARCH_PATH TO tenant, public;  ", "tenant"),
            (" SET SEARCH_PATH analytics, public; ", "analytics"),
        ] {
            let identifier = next_ident(line).expect("located SQL identifier");
            assert_eq!(identifier.value, expected);
            assert_eq!(
                &line.as_bytes()[identifier.start..identifier.start + identifier.len],
                expected.as_bytes()
            );
        }

        let source = "  CREATE SCHEMA app;  \nCREATE TABLE app.users (id INT);\n";
        let mut storage = IntermediateStorage::default();
        collect_sql_entities(Path::new("schema.sql"), source, NodeId(9), &mut storage);
        let schema = storage
            .nodes
            .iter()
            .find(|node| {
                node.canonical_id.as_deref() == Some("sql:schema:app") && node.start_col.is_some()
            })
            .expect("schema node");
        let start = schema.start_col.expect("schema start column") as usize - 1;
        let end = schema.end_col.expect("schema end column") as usize;
        assert_eq!(
            &source.lines().next().unwrap().as_bytes()[start..end],
            b"app"
        );
    }

    fn canonical_ids(storage: &IntermediateStorage) -> Vec<String> {
        let mut ids = storage
            .nodes
            .iter()
            .filter_map(|node| node.canonical_id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    fn collect(source: &str) -> IntermediateStorage {
        let mut storage = IntermediateStorage::default();
        collect_sql_entities(Path::new("schema.sql"), source, NodeId(7), &mut storage);
        storage
    }

    #[test]
    fn commented_out_and_quoted_ddl_produce_no_schema_objects() {
        let live = collect("CREATE TABLE app.users (id INT, email TEXT);\n");
        let live_ids = canonical_ids(&live);
        assert!(
            live_ids.contains(&"sql:table:app.users".to_string()),
            "live DDL must still be collected: {live_ids:?}"
        );
        assert!(
            live_ids.contains(&"sql:column:app.users.id".to_string()),
            "live inline columns must still be collected: {live_ids:?}"
        );

        let commented = collect(concat!(
            "-- CREATE TABLE app.old_users (id INT);\n",
            "-- create table statements below\n",
            "/* CREATE TABLE app.block_users (id INT);\n",
            "   CREATE VIEW app.block_view AS SELECT 1; */\n",
            "EXECUTE 'CREATE TABLE app.dynamic_users (id INT)';\n",
            "COMMENT ON TABLE app.users IS 'CREATE FUNCTION app.fake()';\n",
        ));
        let commented_ids = canonical_ids(&commented);
        for forbidden in [
            "sql:table:app.old_users",
            "sql:table:public.statements",
            "sql:table:app.block_users",
            "sql:view:app.block_view",
            "sql:table:app.dynamic_users",
            "sql:func:app.fake",
        ] {
            assert!(
                !commented_ids.contains(&forbidden.to_string()),
                "`{forbidden}` must not be minted from a comment or a string literal: \
                 {commented_ids:?}"
            );
        }
        assert!(
            !commented
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::FIELD),
            "no inline columns may be minted from commented-out DDL: {commented_ids:?}"
        );
    }

    #[test]
    fn a_trailing_comment_does_not_hide_the_statement_it_follows() {
        let storage = collect(
            "CREATE TABLE app.users (id INT); -- CREATE TABLE app.ghost (id INT);\n\
             CREATE TABLE app.orders (id INT); /* note */\n",
        );
        let ids = canonical_ids(&storage);
        assert!(ids.contains(&"sql:table:app.users".to_string()), "{ids:?}");
        assert!(ids.contains(&"sql:table:app.orders".to_string()), "{ids:?}");
        assert!(!ids.contains(&"sql:table:app.ghost".to_string()), "{ids:?}");
    }

    #[test]
    fn comment_masking_preserves_every_byte_offset() {
        let source = "CREATE TABLE app.users (id INT); -- naïve note\n/* ünicode */\n";
        let masked = mask_sql_comments(source);
        assert_eq!(masked.len(), source.len());
        assert_eq!(masked.lines().count(), source.lines().count());
        assert!(masked.starts_with("CREATE TABLE app.users (id INT);"));
        assert!(
            masked
                .lines()
                .next()
                .expect("first line")
                .trim_end()
                .ends_with(';'),
            "the comment tail must be blanked, not shortened: {masked:?}"
        );
        assert!(
            masked
                .lines()
                .nth(1)
                .expect("second line")
                .trim()
                .is_empty()
        );
    }

    #[test]
    fn a_quoted_comment_marker_is_not_a_comment() {
        let masked = mask_sql_comments("SELECT '-- not a comment' AS note; -- real\n");
        assert!(
            masked.contains("'-- not a comment'"),
            "string literals keep their contents: {masked:?}"
        );
        assert!(
            !masked.contains("real"),
            "the real comment is still blanked: {masked:?}"
        );
    }
}
