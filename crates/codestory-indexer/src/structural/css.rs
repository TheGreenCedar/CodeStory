//! Structural CSS collector (stage 3c of the typed-obligation-proof program).
//!
//! Receipts emitted here follow P3 of the stage-0 proof-formula contract:
//! (i) every `@import` statement is a `NodeKind::MODULE` structural node with
//! an exact source span, plus a file→file IMPORT edge whose effective target
//! is the canonical file node id of the importer-directory-resolved path;
//! (ii) custom-property VARIABLE nodes mint only at declaration sites (`--x:`
//! in property position), never for `var(--x)` usages; (iii) selector CONSTANT
//! nodes mint only in selector position — never decimals, never comment or
//! string text; (iv) `@keyframes` declarations are `NodeKind::FUNCTION` nodes
//! with exact spans; (v) two USAGE families — selector→keyframe (from
//! `animation-name:` / the `animation:` shorthand) and selector→custom-property
//! (from `var(--x)`) — resolved directly when the declaration is in the same
//! file, and through the structural resolution job in
//! `resolution/pipeline.rs` when it is not; (vi) every minted node carries a
//! `MEMBER(file → node)` edge.
//!
//! Fail-closed shape, recorded: a reference the collector cannot resolve
//! in-file targets a synthetic `NodeKind::UNKNOWN` placeholder
//! (`css:var-ref:` / `css:keyframes-ref:` / `css:import-ref:`) that is not a
//! structural unit and can never look like a resolved VARIABLE / FUNCTION /
//! FILE target. The IMPORT edge deliberately does NOT point at a locally
//! minted FILE node with the canonical file id: store nodes upsert
//! last-write-wins by id, so a foreign placeholder FILE row could clobber the
//! real file node minted when the target is indexed. The canonical file id
//! arrives on `resolved_target` via the resolution job instead, and only when
//! that FILE node actually exists.

use crate::intermediate_storage::IntermediateStorage;
use codestory_contracts::graph::{NodeId, NodeKind};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use super::common::{
    StructuralSourceSpan, push_import_edge, push_member_edge, push_structural_node,
    push_synthetic_structural_node, push_usage_edge,
};

/// Keywords that can never be a keyframe name reference.
const CSS_WIDE_KEYWORDS: &[&str] = &[
    "none",
    "initial",
    "inherit",
    "unset",
    "revert",
    "revert-layer",
];

/// At-rules whose block contains nested rules rather than declarations.
const GROUPING_AT_RULES: &[&str] = &[
    "media",
    "supports",
    "container",
    "layer",
    "scope",
    "document",
];

pub(crate) fn collect_css_entities(
    path: &Path,
    source: &str,
    file_id: NodeId,
    storage: &mut IntermediateStorage,
    line_offset: u32,
    first_line_byte_offset: usize,
) {
    let map = SourceMap::new(source, line_offset, first_line_byte_offset);
    let (no_comments, structure) = blank_comments_and_strings(source);
    let parsed = parse_css(&structure);

    let mut selector_nodes: HashMap<String, NodeId> = HashMap::new();
    let mut var_nodes: HashMap<String, NodeId> = HashMap::new();
    let mut keyframe_nodes: HashMap<String, NodeId> = HashMap::new();
    let mut placeholder_nodes: HashMap<String, NodeId> = HashMap::new();
    let mut import_targets_seen: HashSet<NodeId> = HashSet::new();

    // Pass 1: declarations, selectors, and imports, in source order. Usages
    // wait for pass 2 so a keyframe or custom property declared after its
    // first use still resolves in-file (two-pass requirement).
    for record in &parsed.records {
        match record {
            CssRecord::Import {
                stmt_start,
                stmt_end,
            } => {
                let statement_text = &source[*stmt_start..=*stmt_end];
                let literal = extract_import_literal(&no_comments, *stmt_start, *stmt_end);
                let canonical =
                    format!("css:import:{}", literal.as_deref().unwrap_or("unresolved"));
                let span = map.span(*stmt_start, *stmt_end);
                let node_id = push_structural_node(
                    storage,
                    file_id,
                    NodeKind::MODULE,
                    statement_text,
                    &canonical,
                    span,
                );
                push_member_edge(storage, file_id, file_id, node_id, span.start_line);
                let Some(resolved) = literal
                    .as_deref()
                    .and_then(|literal| resolve_relative_import(path, literal))
                else {
                    // External, absolute, escaping, or empty literal: keep the
                    // statement node, refuse the edge (fail closed).
                    continue;
                };
                let display = resolved.to_string_lossy().to_string();
                let ref_canonical = format!("css:import-ref:{display}");
                let placeholder = match placeholder_nodes.get(&ref_canonical) {
                    Some(existing) => *existing,
                    None => {
                        let minted = push_synthetic_structural_node(
                            storage,
                            file_id,
                            NodeKind::UNKNOWN,
                            &display,
                            &ref_canonical,
                        );
                        placeholder_nodes.insert(ref_canonical, minted);
                        minted
                    }
                };
                if import_targets_seen.insert(placeholder) {
                    push_import_edge(storage, file_id, file_id, placeholder, span.start_line);
                }
            }
            CssRecord::Keyframes {
                name,
                at_start,
                name_end,
            } => {
                if keyframe_nodes.contains_key(name) {
                    continue;
                }
                let canonical = format!("css:keyframes:{name}");
                let span = map.span(*at_start, *name_end);
                let node_id = push_structural_node(
                    storage,
                    file_id,
                    NodeKind::FUNCTION,
                    name,
                    &canonical,
                    span,
                );
                push_member_edge(storage, file_id, file_id, node_id, span.start_line);
                keyframe_nodes.insert(name.clone(), node_id);
            }
            CssRecord::Rule { index } => {
                for token in &parsed.rules[*index] {
                    let canonical = token.canonical();
                    if selector_nodes.contains_key(&canonical) {
                        continue;
                    }
                    let span = map.span(token.name_start, token.name_start + token.name.len() - 1);
                    let node_id = push_structural_node(
                        storage,
                        file_id,
                        NodeKind::CONSTANT,
                        &token.name,
                        &canonical,
                        span,
                    );
                    push_member_edge(storage, file_id, file_id, node_id, span.start_line);
                    selector_nodes.insert(canonical, node_id);
                }
            }
            CssRecord::VarDeclaration { name, start } => {
                if var_nodes.contains_key(name) {
                    continue;
                }
                let canonical = format!("css:var:{name}");
                let span = map.span(*start, *start + name.len() - 1);
                let node_id = push_structural_node(
                    storage,
                    file_id,
                    NodeKind::VARIABLE,
                    name,
                    &canonical,
                    span,
                );
                push_member_edge(storage, file_id, file_id, node_id, span.start_line);
                var_nodes.insert(name.clone(), node_id);
            }
            CssRecord::Usage { .. } => {}
        }
    }

    // Pass 2: usage edges, now that every in-file declaration is minted.
    let mut usage_edges_seen: HashSet<(NodeId, NodeId)> = HashSet::new();
    for record in &parsed.records {
        let CssRecord::Usage {
            family,
            name,
            rule,
            at,
        } = record
        else {
            continue;
        };
        let Some(rule_index) = rule else {
            continue;
        };
        let tokens = &parsed.rules[*rule_index];
        if tokens.is_empty() {
            // A rule whose prelude minted no selector node (`:root`, `body`)
            // has no structural source to hang the usage on; dropping the
            // edge loses evidence but never fabricates any.
            continue;
        }
        let target = match family {
            UsageFamily::CustomProperty => var_nodes.get(name).copied().unwrap_or_else(|| {
                unresolved_reference_placeholder(
                    storage,
                    file_id,
                    &mut placeholder_nodes,
                    "css:var-ref",
                    name,
                )
            }),
            UsageFamily::Keyframe => keyframe_nodes.get(name).copied().unwrap_or_else(|| {
                unresolved_reference_placeholder(
                    storage,
                    file_id,
                    &mut placeholder_nodes,
                    "css:keyframes-ref",
                    name,
                )
            }),
        };
        let (line, _) = map.locate(*at);
        for token in tokens {
            let Some(owner) = selector_nodes.get(&token.canonical()).copied() else {
                continue;
            };
            if usage_edges_seen.insert((owner, target)) {
                push_usage_edge(storage, file_id, owner, target, line);
            }
        }
    }
}

fn unresolved_reference_placeholder(
    storage: &mut IntermediateStorage,
    file_id: NodeId,
    placeholder_nodes: &mut HashMap<String, NodeId>,
    prefix: &str,
    name: &str,
) -> NodeId {
    let canonical = format!("{prefix}:{name}");
    if let Some(existing) = placeholder_nodes.get(&canonical) {
        return *existing;
    }
    let minted =
        push_synthetic_structural_node(storage, file_id, NodeKind::UNKNOWN, name, &canonical);
    placeholder_nodes.insert(canonical, minted);
    minted
}

/// Resolve an `@import` literal against the importer's directory, lexically.
///
/// Indexer-owned path resolution (the agent-side path-reconstruction ban does
/// not apply here). External URLs, absolute paths, empty literals, and `..`
/// walks that escape the importer's tree all fail closed to `None`.
fn resolve_relative_import(importer: &Path, literal: &str) -> Option<PathBuf> {
    if literal.is_empty()
        || literal.starts_with("//")
        || literal.contains("://")
        || literal.starts_with("data:")
    {
        return None;
    }
    let literal_path = Path::new(literal);
    if literal_path.is_absolute() {
        return None;
    }
    let mut resolved = importer.parent()?.to_path_buf();
    for component in literal_path.components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(resolved)
}

/// Extract the path literal of one `@import` statement from comment-blanked
/// (but string-preserving) source bytes.
fn extract_import_literal(
    no_comments: &[u8],
    stmt_start: usize,
    stmt_end: usize,
) -> Option<String> {
    let text = &no_comments[stmt_start..stmt_end];
    let rest = text.get("@import".len()..)?;
    let rest = trim_ascii_start(rest);
    match rest.first()? {
        b'"' | b'\'' => {
            let quote = rest[0];
            let end = rest[1..].iter().position(|byte| *byte == quote)?;
            Some(String::from_utf8_lossy(&rest[1..1 + end]).into_owned())
        }
        _ if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case(b"url(") => {
            let inner = &rest[4..];
            let end = inner.iter().position(|byte| *byte == b')')?;
            let mut value = trim_ascii(&inner[..end]);
            if value.len() >= 2
                && (value[0] == b'"' || value[0] == b'\'')
                && value[value.len() - 1] == value[0]
            {
                value = &value[1..value.len() - 1];
            }
            Some(String::from_utf8_lossy(value).into_owned())
        }
        _ => None,
    }
}

fn trim_ascii_start(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    &bytes[start..]
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let bytes = trim_ascii_start(bytes);
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(0);
    &bytes[..end]
}

/// Maps local byte offsets to global (line, zero-based column) coordinates,
/// honoring the embedded-`<style>` line/column offsets the HTML path passes.
struct SourceMap {
    line_starts: Vec<usize>,
    line_offset: u32,
    first_line_byte_offset: usize,
}

impl SourceMap {
    fn new(source: &str, line_offset: u32, first_line_byte_offset: usize) -> Self {
        let mut line_starts = vec![0usize];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        Self {
            line_starts,
            line_offset,
            first_line_byte_offset,
        }
    }

    fn locate(&self, byte: usize) -> (u32, usize) {
        let line_idx = self.line_starts.partition_point(|start| *start <= byte) - 1;
        let mut column = byte - self.line_starts[line_idx];
        if line_idx == 0 {
            column += self.first_line_byte_offset;
        }
        let line = (line_idx as u32).saturating_add(self.line_offset);
        (line, column)
    }

    fn span(&self, start: usize, end_inclusive: usize) -> StructuralSourceSpan {
        let (start_line, start_col) = self.locate(start);
        let (end_line, end_col) = self.locate(end_inclusive);
        StructuralSourceSpan {
            start_line,
            start_col: start_col.saturating_add(1).try_into().unwrap_or(u32::MAX),
            end_line,
            end_col: end_col.saturating_add(1).try_into().unwrap_or(u32::MAX),
        }
    }
}

/// Blank comment interiors in both views and string interiors in the
/// structure view, preserving byte offsets and newlines throughout.
///
/// Returns `(no_comments, structure)`: import literals are read from the
/// first (strings intact), every structural decision from the second, so
/// commented-out or quoted text can never mint a node or an edge.
fn blank_comments_and_strings(source: &str) -> (Vec<u8>, Vec<u8>) {
    let bytes = source.as_bytes();
    let mut no_comments = bytes.to_vec();
    let mut structure = bytes.to_vec();
    enum State {
        Normal,
        Comment,
        Str(u8),
    }
    let mut state = State::Normal;
    let mut i = 0usize;
    while i < bytes.len() {
        match state {
            State::Normal => {
                if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    no_comments[i] = b' ';
                    structure[i] = b' ';
                    no_comments[i + 1] = b' ';
                    structure[i + 1] = b' ';
                    state = State::Comment;
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' || bytes[i] == b'\'' {
                    state = State::Str(bytes[i]);
                }
                i += 1;
            }
            State::Comment => {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    no_comments[i] = b' ';
                    structure[i] = b' ';
                    no_comments[i + 1] = b' ';
                    structure[i + 1] = b' ';
                    state = State::Normal;
                    i += 2;
                    continue;
                }
                if bytes[i] != b'\n' {
                    no_comments[i] = b' ';
                    structure[i] = b' ';
                }
                i += 1;
            }
            State::Str(quote) => {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    structure[i] = b' ';
                    if bytes[i + 1] != b'\n' {
                        structure[i + 1] = b' ';
                    }
                    i += 2;
                    continue;
                }
                if bytes[i] == quote || bytes[i] == b'\n' {
                    // CSS error recovery ends an unterminated string at the
                    // newline; the closing quote itself stays visible.
                    state = State::Normal;
                    i += 1;
                    continue;
                }
                structure[i] = b' ';
                i += 1;
            }
        }
    }
    (no_comments, structure)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SelectorKind {
    Class,
    Id,
}

struct SelectorToken {
    kind: SelectorKind,
    name: String,
    name_start: usize,
}

impl SelectorToken {
    fn canonical(&self) -> String {
        match self.kind {
            SelectorKind::Class => format!("css:class:{}", self.name),
            SelectorKind::Id => format!("css:id:{}", self.name),
        }
    }
}

enum UsageFamily {
    CustomProperty,
    Keyframe,
}

enum CssRecord {
    Import {
        stmt_start: usize,
        /// Byte index of the terminating `;` (span-inclusive end).
        stmt_end: usize,
    },
    Keyframes {
        name: String,
        at_start: usize,
        name_end: usize,
    },
    Rule {
        index: usize,
    },
    VarDeclaration {
        name: String,
        start: usize,
    },
    Usage {
        family: UsageFamily,
        name: String,
        rule: Option<usize>,
        at: usize,
    },
}

struct ParsedCss {
    records: Vec<CssRecord>,
    rules: Vec<Vec<SelectorToken>>,
}

enum Ctx {
    RuleList,
    Declarations { rule: Option<usize> },
}

fn parse_css(structure: &[u8]) -> ParsedCss {
    let mut parsed = ParsedCss {
        records: Vec::new(),
        rules: Vec::new(),
    };
    let mut ctx: Vec<Ctx> = vec![Ctx::RuleList];
    let mut seg_start = 0usize;
    let mut i = 0usize;
    while i < structure.len() {
        match structure[i] {
            b'{' => {
                let next = match ctx.last().expect("context stack is never empty") {
                    Ctx::RuleList => open_rule_list_block(structure, seg_start, i, &mut parsed),
                    // Nested blocks inside a declaration block (modern CSS
                    // nesting) are not attributed to any selector: fail
                    // closed rather than mis-anchor a usage.
                    Ctx::Declarations { .. } => Ctx::Declarations { rule: None },
                };
                ctx.push(next);
                seg_start = i + 1;
            }
            b'}' => {
                if let Ctx::Declarations { rule } =
                    ctx.last().expect("context stack is never empty")
                {
                    process_declaration(structure, seg_start, i, *rule, &mut parsed.records);
                }
                if ctx.len() > 1 {
                    ctx.pop();
                }
                seg_start = i + 1;
            }
            b';' => {
                match ctx.last().expect("context stack is never empty") {
                    Ctx::RuleList => {
                        maybe_record_import(structure, seg_start, i, &mut parsed.records)
                    }
                    Ctx::Declarations { rule } => {
                        process_declaration(structure, seg_start, i, *rule, &mut parsed.records)
                    }
                }
                seg_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parsed
}

fn open_rule_list_block(
    structure: &[u8],
    seg_start: usize,
    brace: usize,
    parsed: &mut ParsedCss,
) -> Ctx {
    let Some(prelude_start) =
        (seg_start..brace).find(|index| !structure[*index].is_ascii_whitespace())
    else {
        return Ctx::Declarations { rule: None };
    };
    if structure[prelude_start] == b'@' {
        let ident_end = ident_run_end(structure, prelude_start + 1);
        let at_ident =
            String::from_utf8_lossy(&structure[prelude_start + 1..ident_end]).to_ascii_lowercase();
        let base = strip_vendor_prefix(&at_ident);
        if base == "keyframes" {
            let name_start =
                (ident_end..brace).find(|index| !structure[*index].is_ascii_whitespace());
            if let Some(name_start) = name_start {
                let name_end = ident_run_end(structure, name_start);
                if name_end > name_start {
                    let name =
                        String::from_utf8_lossy(&structure[name_start..name_end]).into_owned();
                    if is_countable_ident(name.as_bytes()) {
                        parsed.records.push(CssRecord::Keyframes {
                            name,
                            at_start: prelude_start,
                            name_end: name_end - 1,
                        });
                    }
                }
            }
            // Keyframe step preludes (`from`, `to`, `62.5%`) sit in this
            // nested list and mint nothing.
            return Ctx::RuleList;
        }
        if GROUPING_AT_RULES.contains(&base) {
            return Ctx::RuleList;
        }
        // `@font-face`, `@page`, `@property`, ...: declaration blocks with no
        // selector owner.
        return Ctx::Declarations { rule: None };
    }
    let tokens = scan_selector_tokens(structure, prelude_start, brace);
    let index = parsed.rules.len();
    parsed.rules.push(tokens);
    parsed.records.push(CssRecord::Rule { index });
    Ctx::Declarations { rule: Some(index) }
}

fn maybe_record_import(
    structure: &[u8],
    seg_start: usize,
    semicolon: usize,
    records: &mut Vec<CssRecord>,
) {
    let Some(prelude_start) =
        (seg_start..semicolon).find(|index| !structure[*index].is_ascii_whitespace())
    else {
        return;
    };
    let prelude = &structure[prelude_start..semicolon];
    if prelude.len() < "@import".len()
        || !prelude[.."@import".len()].eq_ignore_ascii_case(b"@import")
    {
        return;
    }
    if prelude
        .get("@import".len())
        .is_some_and(|byte| is_ident_byte(*byte))
    {
        return;
    }
    records.push(CssRecord::Import {
        stmt_start: prelude_start,
        stmt_end: semicolon,
    });
}

fn process_declaration(
    structure: &[u8],
    start: usize,
    end: usize,
    rule: Option<usize>,
    records: &mut Vec<CssRecord>,
) {
    let Some(colon) = (start..end).find(|index| structure[*index] == b':') else {
        return;
    };
    let Some(prop_start) = (start..colon).find(|index| !structure[*index].is_ascii_whitespace())
    else {
        return;
    };
    let prop_end = (prop_start..colon)
        .rev()
        .find(|index| !structure[*index].is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(prop_start);
    let property = &structure[prop_start..prop_end];

    scan_var_usages(structure, colon + 1, end, rule, records);

    if property.len() > 2 && property.starts_with(b"--") {
        if property.iter().all(|byte| is_ident_byte(*byte)) {
            records.push(CssRecord::VarDeclaration {
                name: String::from_utf8_lossy(property).into_owned(),
                start: prop_start,
            });
        }
        return;
    }

    let normalized =
        strip_vendor_prefix(&String::from_utf8_lossy(property).to_ascii_lowercase()).to_string();
    let excluded: &[&str] = match normalized.as_str() {
        "animation-name" => CSS_WIDE_KEYWORDS,
        "animation" => ANIMATION_SHORTHAND_KEYWORDS,
        _ => return,
    };
    for (name, at) in scan_value_idents(structure, colon + 1, end) {
        if excluded.contains(&name.to_ascii_lowercase().as_str()) {
            continue;
        }
        records.push(CssRecord::Usage {
            family: UsageFamily::Keyframe,
            name,
            rule,
            at,
        });
    }
}

/// Value idents reserved by the `animation` shorthand grammar, plus the
/// CSS-wide keywords: none of these can be a keyframe name reference.
const ANIMATION_SHORTHAND_KEYWORDS: &[&str] = &[
    "linear",
    "ease",
    "ease-in",
    "ease-out",
    "ease-in-out",
    "step-start",
    "step-end",
    "steps",
    "cubic-bezier",
    "infinite",
    "normal",
    "reverse",
    "alternate",
    "alternate-reverse",
    "forwards",
    "backwards",
    "both",
    "running",
    "paused",
    "important",
    "auto",
    "scroll",
    "view",
    "allow-discrete",
    "none",
    "initial",
    "inherit",
    "unset",
    "revert",
    "revert-layer",
];

fn scan_var_usages(
    structure: &[u8],
    start: usize,
    end: usize,
    rule: Option<usize>,
    records: &mut Vec<CssRecord>,
) {
    let mut i = start;
    while i + 4 <= end {
        if !structure[i..i + 3].eq_ignore_ascii_case(b"var") || structure[i + 3] != b'(' {
            i += 1;
            continue;
        }
        if i > start && is_ident_byte(structure[i - 1]) {
            i += 1;
            continue;
        }
        let mut cursor = i + 4;
        while cursor < end && structure[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor + 2 <= end && structure[cursor..cursor + 2] == *b"--" {
            let name_end = ident_run_end(structure, cursor + 2);
            if name_end > cursor + 2 {
                records.push(CssRecord::Usage {
                    family: UsageFamily::CustomProperty,
                    name: String::from_utf8_lossy(&structure[cursor..name_end]).into_owned(),
                    rule,
                    at: cursor,
                });
                i = name_end;
                continue;
            }
        }
        i += 4;
    }
}

fn scan_value_idents(structure: &[u8], start: usize, end: usize) -> Vec<(String, usize)> {
    let mut idents = Vec::new();
    let mut i = start;
    while i < end {
        if !is_ident_byte(structure[i]) {
            i += 1;
            continue;
        }
        let run_start = i;
        let run_end = ident_run_end(structure, i).min(end);
        i = run_end;
        let run = &structure[run_start..run_end];
        // Function names (`var(`, `steps(`, `cubic-bezier(`) are not
        // keyframe references; neither are times/numbers nor custom-property
        // idents.
        if structure.get(run_end) == Some(&b'(') {
            continue;
        }
        if !is_countable_ident(run) || run.starts_with(b"--") {
            continue;
        }
        idents.push((String::from_utf8_lossy(run).into_owned(), run_start));
    }
    idents
}

fn scan_selector_tokens(structure: &[u8], start: usize, end: usize) -> Vec<SelectorToken> {
    let mut tokens = Vec::new();
    let mut i = start;
    while i < end {
        let kind = match structure[i] {
            b'.' => Some(SelectorKind::Class),
            b'#' => Some(SelectorKind::Id),
            _ => None,
        };
        let Some(kind) = kind else {
            i += 1;
            continue;
        };
        let name_start = i + 1;
        let name_end = ident_run_end(structure, name_start).min(end);
        let run = &structure[name_start..name_end];
        if run.is_empty() || !is_countable_ident(run) {
            i = name_start;
            continue;
        }
        tokens.push(SelectorToken {
            kind,
            name: String::from_utf8_lossy(run).into_owned(),
            name_start,
        });
        i = name_end;
    }
    tokens
}

fn ident_run_end(structure: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < structure.len() && is_ident_byte(structure[end]) {
        end += 1;
    }
    end
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// A CSS ident that can name something: never digit-leading (`.5s`, `62`),
/// never a lone `-`, never `-` followed by a digit (`-2s`).
fn is_countable_ident(run: &[u8]) -> bool {
    match run.first() {
        None => false,
        Some(byte) if byte.is_ascii_digit() => false,
        Some(b'-') => match run.get(1) {
            None => false,
            Some(byte) => !byte.is_ascii_digit(),
        },
        Some(_) => true,
    }
}

fn strip_vendor_prefix(ident: &str) -> &str {
    let Some(rest) = ident.strip_prefix('-') else {
        return ident;
    };
    match rest.find('-') {
        Some(index) => &rest[index + 1..],
        None => ident,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intermediate_storage::IntermediateStorage;
    use crate::structural::{
        MAX_STRUCTURAL_UNITS_PER_FILE, StructuralCollectionError, index_structural_source,
    };
    use codestory_contracts::graph::{EdgeKind, Node, NodeId, NodeKind};
    use std::path::Path;

    fn collect(path: &str, source: &str) -> IntermediateStorage {
        let mut storage = IntermediateStorage::default();
        collect_css_entities(Path::new(path), source, NodeId(42), &mut storage, 1, 0);
        storage
    }

    fn find_node<'a>(storage: &'a IntermediateStorage, canonical: &str) -> Option<&'a Node> {
        storage
            .nodes
            .iter()
            .find(|node| node.canonical_id.as_deref() == Some(canonical))
    }

    fn node_by_id(storage: &IntermediateStorage, id: NodeId) -> &Node {
        storage
            .nodes
            .iter()
            .find(|node| node.id == id)
            .expect("edge endpoint node")
    }

    fn usage_targets_of(storage: &IntermediateStorage, source_canonical: &str) -> Vec<String> {
        let source = find_node(storage, source_canonical).expect("usage source node");
        let mut targets = storage
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::USAGE && edge.source == source.id)
            .map(|edge| {
                node_by_id(storage, edge.target)
                    .canonical_id
                    .clone()
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        targets.sort();
        targets
    }

    #[test]
    fn collects_class_id_and_custom_property() {
        let storage = collect(
            "styles.css",
            ".btn { --primary: blue; color: var(--primary); }\n#app { }",
        );
        let kinds: Vec<_> = storage.nodes.iter().map(|n| n.kind).collect();
        assert!(kinds.contains(&NodeKind::CONSTANT));
        assert!(kinds.contains(&NodeKind::VARIABLE));
        assert!(storage.edges.iter().any(|e| e.kind == EdgeKind::MEMBER));
    }

    #[test]
    fn custom_properties_mint_only_at_declaration_sites() {
        let storage = collect(
            "styles.css",
            ".card { color: var(--accent); }\n:root { --accent: blue; }",
        );
        let var_nodes: Vec<_> = storage
            .nodes
            .iter()
            .filter(|node| {
                node.canonical_id
                    .as_deref()
                    .is_some_and(|canonical| canonical.starts_with("css:var:"))
            })
            .collect();
        assert_eq!(var_nodes.len(), 1, "only the declaration site mints");
        let declaration = var_nodes[0];
        assert_eq!(declaration.kind, NodeKind::VARIABLE);
        assert_eq!(
            declaration.canonical_id.as_deref(),
            Some("css:var:--accent")
        );
        assert_eq!(
            declaration.start_line,
            Some(2),
            "declared line, not the usage line"
        );

        // The usage resolves in-file even though the declaration comes later.
        assert_eq!(
            usage_targets_of(&storage, "css:class:card"),
            vec!["css:var:--accent".to_string()]
        );
        assert!(
            find_node(&storage, "css:var-ref:--accent").is_none(),
            "an in-file-resolved usage must not mint a placeholder"
        );
    }

    #[test]
    fn selector_constants_mint_only_in_selector_position() {
        let storage = collect(
            "styles.css",
            "div.card, #app { transition: all .5s; margin: .75em; }\n/* .ghost and #phantom hide in a comment */\n.real { content: \".dot #hash\"; }\n[href$=\".css\"] { color: red; }\n",
        );
        for expected in ["css:class:card", "css:id:app", "css:class:real"] {
            let node = find_node(&storage, expected).expect(expected);
            assert_eq!(node.kind, NodeKind::CONSTANT);
        }
        for forbidden in [
            "css:class:ghost",
            "css:id:phantom",
            "css:class:dot",
            "css:id:hash",
            "css:class:css",
            "css:class:5s",
            "css:class:75em",
        ] {
            assert!(
                find_node(&storage, forbidden).is_none(),
                "{forbidden} is not selector-position source"
            );
        }
    }

    #[test]
    fn keyframes_mint_function_nodes_with_exact_spans() {
        let storage = collect(
            "styles.css",
            "@keyframes bounce {\n  from { transform: none; }\n  62.5% { opacity: .5; }\n}\n@-webkit-keyframes bounce { to { opacity: 1; } }\n",
        );
        let keyframes: Vec<_> = storage
            .nodes
            .iter()
            .filter(|node| node.canonical_id.as_deref() == Some("css:keyframes:bounce"))
            .collect();
        assert_eq!(keyframes.len(), 1, "vendor duplicate must not double-mint");
        let node = keyframes[0];
        assert_eq!(node.kind, NodeKind::FUNCTION);
        assert_eq!(node.start_line, Some(1));
        assert_eq!(node.start_col, Some(1));
        assert_eq!(node.end_col, Some("@keyframes bounce".len() as u32));
        assert!(
            storage
                .edges
                .iter()
                .any(|edge| edge.kind == EdgeKind::MEMBER && edge.target == node.id),
            "every new node carries MEMBER(file -> node)"
        );
        assert!(
            !storage
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::CONSTANT),
            "keyframe step preludes (from/to/62.5%) mint no selectors"
        );
    }

    #[test]
    fn same_file_usage_families_resolve_directly() {
        let storage = collect(
            "styles.css",
            ".spinner { animation: spin 2s linear infinite; }\n.pulse { animation-name: pulsing, spin; }\n.themed { color: var(--ink); }\n:root { --ink: black; }\n@keyframes spin { to { transform: rotate(1turn); } }\n@keyframes pulsing { to { opacity: 0; } }\n",
        );
        assert_eq!(
            usage_targets_of(&storage, "css:class:spinner"),
            vec!["css:keyframes:spin".to_string()]
        );
        assert_eq!(
            usage_targets_of(&storage, "css:class:pulse"),
            vec![
                "css:keyframes:pulsing".to_string(),
                "css:keyframes:spin".to_string(),
            ]
        );
        assert_eq!(
            usage_targets_of(&storage, "css:class:themed"),
            vec!["css:var:--ink".to_string()]
        );
        assert!(
            !storage.nodes.iter().any(|node| {
                node.canonical_id
                    .as_deref()
                    .is_some_and(|canonical| canonical.contains("-ref:"))
            }),
            "fully in-file references need no placeholders"
        );
        for keyword in ["linear", "infinite"] {
            assert!(
                !storage
                    .nodes
                    .iter()
                    .any(|node| node.serialized_name == keyword),
                "animation keyword `{keyword}` must not become a reference"
            );
        }
    }

    #[test]
    fn import_statements_mint_module_nodes_and_importer_resolved_edges() {
        let storage = collect(
            "site/nested/entry.css",
            "@import './theme/_vars.css';\n@import \"../shared/base.css\";\n@import './theme/_vars.css';\n",
        );
        let modules: Vec<_> = storage
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::MODULE)
            .collect();
        assert_eq!(modules.len(), 3, "every import statement is a MODULE node");
        let first = modules
            .iter()
            .find(|node| node.start_line == Some(1))
            .expect("first import node");
        assert_eq!(first.serialized_name, "@import './theme/_vars.css';");
        assert_eq!(first.start_col, Some(1));
        assert_eq!(
            first.end_col,
            Some("@import './theme/_vars.css';".len() as u32)
        );
        assert!(
            storage
                .edges
                .iter()
                .any(|edge| edge.kind == EdgeKind::MEMBER && edge.target == first.id),
            "import statement nodes carry MEMBER(file -> node)"
        );

        let vars_ref = find_node(&storage, "css:import-ref:site/nested/theme/_vars.css")
            .expect("importer-directory-resolved vars placeholder");
        assert_eq!(vars_ref.kind, NodeKind::UNKNOWN);
        assert_eq!(
            vars_ref.qualified_name.as_deref(),
            Some("site/nested/theme/_vars.css")
        );
        let base_ref = find_node(&storage, "css:import-ref:site/shared/base.css")
            .expect("parent-dir-resolved base placeholder");

        let import_edges: Vec<_> = storage
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::IMPORT)
            .collect();
        assert_eq!(import_edges.len(), 2, "duplicate imports deduplicate");
        for edge in &import_edges {
            assert_eq!(edge.source, NodeId(42), "IMPORT edges are file-sourced");
        }
        assert!(import_edges.iter().any(|edge| edge.target == vars_ref.id));
        assert!(import_edges.iter().any(|edge| edge.target == base_ref.id));

        // The stage-3c repair: no synthetic `file:{literal}` targets, no
        // fabricated FILE nodes, and the importer path is actually consumed.
        assert!(
            !storage.nodes.iter().any(|node| {
                node.kind == NodeKind::FILE
                    || node
                        .canonical_id
                        .as_deref()
                        .is_some_and(|canonical| canonical.starts_with("file:"))
            }),
            "the collector must not fabricate FILE nodes for import targets"
        );
        assert!(
            !storage.structural_unit_node_ids.contains(&vars_ref.id),
            "placeholders are not structural units and carry no fabricated span"
        );
        assert_eq!(vars_ref.start_line, None);
    }

    #[test]
    fn hostile_imports_and_references_fail_closed() {
        let storage = collect(
            "site/entry.css",
            "/* @import 'commented.css'; */\n.decoy { background: url('@import \"stringed.css\";'); }\n@import 'https://cdn.example/x.css';\n@import url(//cdn.example/y.css);\n@import '/abs/z.css';\n@import '';\n@import '../../escape.css';\n.user { color: var(--undeclared); animation-name: ghost; }\n",
        );
        assert_eq!(
            storage
                .edges
                .iter()
                .filter(|edge| edge.kind == EdgeKind::IMPORT)
                .count(),
            0,
            "external, absolute, empty, and escaping literals never edge"
        );
        assert_eq!(
            storage
                .nodes
                .iter()
                .filter(|node| node.kind == NodeKind::MODULE)
                .count(),
            5,
            "real import statements mint nodes; commented/quoted text does not"
        );
        assert!(
            find_node(&storage, "css:var:--undeclared").is_none(),
            "a usage-minted custom-property node cannot exist (P3.ii)"
        );
        let var_ref = find_node(&storage, "css:var-ref:--undeclared")
            .expect("undeclared var reference placeholder");
        assert_eq!(var_ref.kind, NodeKind::UNKNOWN);
        let keyframe_ref = find_node(&storage, "css:keyframes-ref:ghost")
            .expect("undeclared keyframe reference placeholder");
        assert_eq!(keyframe_ref.kind, NodeKind::UNKNOWN);
        assert_eq!(
            usage_targets_of(&storage, "css:class:user"),
            vec![
                "css:keyframes-ref:ghost".to_string(),
                "css:var-ref:--undeclared".to_string(),
            ]
        );
        for placeholder in [var_ref, keyframe_ref] {
            assert!(!storage.structural_unit_node_ids.contains(&placeholder.id));
            assert_eq!(placeholder.start_line, None);
        }
    }

    #[test]
    fn near_cap_css_unit_count_stays_fail_closed_at_the_limit() {
        let mut source = String::new();
        for index in 0..MAX_STRUCTURAL_UNITS_PER_FILE {
            source.push_str(&format!(".c{index}{{color:red}}\n"));
        }
        let at_cap = index_structural_source(Path::new("big.css"), &source)
            .expect("exactly the cap must collect");
        assert_eq!(
            at_cap.structural_unit_node_ids.len(),
            MAX_STRUCTURAL_UNITS_PER_FILE
        );

        source.push_str(".overflow{color:red}\n");
        match index_structural_source(Path::new("big.css"), &source) {
            Err(StructuralCollectionError::UnitLimit {
                observed_unit_count,
                structural_unit_cap,
            }) => {
                assert_eq!(
                    observed_unit_count,
                    MAX_STRUCTURAL_UNITS_PER_FILE as u64 + 1
                );
                assert_eq!(structural_unit_cap, MAX_STRUCTURAL_UNITS_PER_FILE as u64);
            }
            Err(other) => panic!("one unit over the cap must fail closed, got {other:?}"),
            Ok(_) => panic!("one unit over the cap must fail closed, got a collection"),
        }
    }

    #[test]
    fn cross_file_references_resolve_through_the_import_closure() -> anyhow::Result<()> {
        use codestory_contracts::events::EventBus;
        use codestory_store::Store as Storage;
        use codestory_workspace::RefreshInfo;

        let dir = tempfile::tempdir()?;
        let entry = dir.path().join("site/entry.css");
        let vars = dir.path().join("site/theme/vars.css");
        let spin = dir.path().join("site/anim/spin.css");
        std::fs::create_dir_all(entry.parent().unwrap())?;
        std::fs::create_dir_all(vars.parent().unwrap())?;
        std::fs::create_dir_all(spin.parent().unwrap())?;
        std::fs::write(
            &entry,
            "@import 'theme/vars.css';\n@import 'anim/spin.css';\n.app { color: var(--accent); animation-name: spin; background: var(--missing); }\n",
        )?;
        std::fs::write(&vars, ":root { --accent: teal; }\n")?;
        std::fs::write(
            &spin,
            "@keyframes spin { to { transform: rotate(1turn); } }\n.spin { animation-name: spin; }\n",
        )?;

        let mut storage = Storage::new_in_memory()?;
        let indexer = crate::WorkspaceIndexer::new(dir.path().to_path_buf());
        indexer.run_incremental(
            &mut storage,
            &RefreshInfo {
                mode: codestory_workspace::BuildMode::Incremental,
                files_to_index: vec![entry.clone(), vars.clone(), spin.clone()],
                files_to_remove: Vec::new(),
                existing_file_ids: std::collections::HashMap::new(),
            },
            &EventBus::new(),
            None,
        )?;

        let nodes = storage.get_nodes()?;
        let edges = storage.get_edges()?;
        let entry_file_id = crate::WorkspaceIndexer::canonical_file_node_id_for_path(&entry);
        let vars_file_id = crate::WorkspaceIndexer::canonical_file_node_id_for_path(&vars);
        let spin_file_id = crate::WorkspaceIndexer::canonical_file_node_id_for_path(&spin);

        let by_canonical = |canonical: &str| {
            nodes
                .iter()
                .find(|node| node.canonical_id.as_deref() == Some(canonical))
                .unwrap_or_else(|| panic!("missing node {canonical}"))
        };
        let accent = by_canonical("css:var:--accent");
        assert_eq!(accent.kind, NodeKind::VARIABLE);
        assert_eq!(accent.file_node_id, Some(NodeId(vars_file_id)));
        let keyframe = by_canonical("css:keyframes:spin");
        assert_eq!(keyframe.kind, NodeKind::FUNCTION);
        assert_eq!(keyframe.file_node_id, Some(NodeId(spin_file_id)));
        let app = nodes
            .iter()
            .find(|node| {
                node.canonical_id.as_deref() == Some("css:class:app")
                    && node.file_node_id == Some(NodeId(entry_file_id))
            })
            .expect("entry selector node");

        // P3.i: both entrypoint IMPORT edges reach real canonical file ids.
        let entry_imports: Vec<_> = edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::IMPORT && edge.source == NodeId(entry_file_id))
            .collect();
        assert_eq!(entry_imports.len(), 2);
        let resolved_targets: std::collections::HashSet<_> = entry_imports
            .iter()
            .map(|edge| edge.resolved_target.expect("import resolves to a file"))
            .collect();
        assert_eq!(
            resolved_targets,
            std::collections::HashSet::from([NodeId(vars_file_id), NodeId(spin_file_id)])
        );

        // P3.v cross-file: selector -> custom property and selector -> keyframe
        // resolve through the import closure to declaration nodes.
        assert!(
            edges.iter().any(|edge| {
                edge.kind == EdgeKind::USAGE
                    && edge.source == app.id
                    && edge.resolved_target == Some(accent.id)
            }),
            "var usage must resolve into the imported vars file"
        );
        assert!(
            edges.iter().any(|edge| {
                edge.kind == EdgeKind::USAGE
                    && edge.source == app.id
                    && edge.resolved_target == Some(keyframe.id)
            }),
            "keyframe usage must resolve into the imported animation file"
        );

        // Fail closed: an undeclared var keeps its UNKNOWN placeholder target.
        let missing_ref = by_canonical("css:var-ref:--missing");
        assert_eq!(missing_ref.kind, NodeKind::UNKNOWN);
        let missing_edge = edges
            .iter()
            .find(|edge| {
                edge.kind == EdgeKind::USAGE
                    && edge.source == app.id
                    && edge.target == missing_ref.id
            })
            .expect("undeclared var usage edge");
        assert_eq!(missing_edge.resolved_target, None);

        // Same-file resolution stays direct: .spin -> spin keyframe.
        let spin_selector = nodes
            .iter()
            .find(|node| {
                node.canonical_id.as_deref() == Some("css:class:spin")
                    && node.file_node_id == Some(NodeId(spin_file_id))
            })
            .expect("spin selector node");
        assert!(
            edges.iter().any(|edge| {
                edge.kind == EdgeKind::USAGE
                    && edge.source == spin_selector.id
                    && edge.target == keyframe.id
            }),
            "same-file keyframe usage targets the declaration directly"
        );
        Ok(())
    }

    #[test]
    fn edited_css_file_reindexes_without_stale_import_or_usage_state() -> anyhow::Result<()> {
        use codestory_contracts::events::EventBus;
        use codestory_store::Store as Storage;
        use codestory_workspace::RefreshInfo;

        let dir = tempfile::tempdir()?;
        let entry = dir.path().join("entry.css");
        let first = dir.path().join("a.css");
        let second = dir.path().join("b.css");
        std::fs::write(&entry, "@import 'a.css';\n.app { color: var(--x); }\n")?;
        std::fs::write(&first, ":root { --x: red; }\n")?;
        std::fs::write(&second, ":root { --x: green; }\n")?;

        let mut storage = Storage::new_in_memory()?;
        let indexer = crate::WorkspaceIndexer::new(dir.path().to_path_buf());
        let bus = EventBus::new();
        let refresh = |files: Vec<std::path::PathBuf>| RefreshInfo {
            mode: codestory_workspace::BuildMode::Incremental,
            files_to_index: files,
            files_to_remove: Vec::new(),
            existing_file_ids: std::collections::HashMap::new(),
        };
        indexer.run_incremental(
            &mut storage,
            &refresh(vec![entry.clone(), first.clone(), second.clone()]),
            &bus,
            None,
        )?;

        let entry_file_id = crate::WorkspaceIndexer::canonical_file_node_id_for_path(&entry);
        let first_file_id = crate::WorkspaceIndexer::canonical_file_node_id_for_path(&first);
        let second_file_id = crate::WorkspaceIndexer::canonical_file_node_id_for_path(&second);
        let entry_imports = |edges: &[codestory_contracts::graph::Edge]| {
            edges
                .iter()
                .filter(|edge| {
                    edge.kind == EdgeKind::IMPORT && edge.source == NodeId(entry_file_id)
                })
                .map(|edge| edge.resolved_target)
                .collect::<Vec<_>>()
        };
        let edges = storage.get_edges()?;
        assert_eq!(entry_imports(&edges), vec![Some(NodeId(first_file_id))]);
        let nodes = storage.get_nodes()?;
        let declaration_in = |nodes: &[Node], file_id: i64| {
            nodes
                .iter()
                .find(|node| {
                    node.canonical_id.as_deref() == Some("css:var:--x")
                        && node.file_node_id == Some(NodeId(file_id))
                })
                .map(|node| node.id)
                .expect("--x declaration node")
        };
        let first_declaration = declaration_in(&nodes, first_file_id);
        assert!(
            edges.iter().any(|edge| edge.kind == EdgeKind::USAGE
                && edge.resolved_target == Some(first_declaration)),
            "initial var usage resolves into a.css"
        );

        // Edit the importer: the import moves from a.css to b.css. The
        // USAGE->IMPORT kind change puts these edges outside the caller-scoped
        // delta-repair class, so the structural FullReplace fence must remove
        // the stale rows and the re-run must re-resolve against b.css only.
        std::fs::write(&entry, "@import 'b.css';\n.app { color: var(--x); }\n")?;
        indexer.run_incremental(&mut storage, &refresh(vec![entry.clone()]), &bus, None)?;

        let edges = storage.get_edges()?;
        let nodes = storage.get_nodes()?;
        assert_eq!(
            entry_imports(&edges),
            vec![Some(NodeId(second_file_id))],
            "exactly one IMPORT survives the edit and it points at b.css"
        );
        let second_declaration = declaration_in(&nodes, second_file_id);
        let app = nodes
            .iter()
            .find(|node| {
                node.canonical_id.as_deref() == Some("css:class:app")
                    && node.file_node_id == Some(NodeId(entry_file_id))
            })
            .expect("entry selector node after edit");
        let usage = edges
            .iter()
            .find(|edge| edge.kind == EdgeKind::USAGE && edge.source == app.id)
            .expect("var usage edge after edit");
        assert_eq!(
            usage.resolved_target,
            Some(second_declaration),
            "the usage re-resolves against the new import closure"
        );
        let first_declaration = declaration_in(&nodes, first_file_id);
        assert!(
            !edges.iter().any(|edge| edge.kind == EdgeKind::USAGE
                && edge.source == app.id
                && edge.resolved_target == Some(first_declaration)),
            "no stale resolution into the no-longer-imported a.css survives"
        );
        Ok(())
    }

    #[test]
    fn sibling_sheets_resolve_vars_through_the_shared_importer_component() -> anyhow::Result<()> {
        use codestory_contracts::events::EventBus;
        use codestory_store::Store as Storage;
        use codestory_workspace::RefreshInfo;

        // The animate.css topology that surfaced the component requirement:
        // `_base.css` imports NOTHING and reaches `_vars.css` only through
        // their shared importer (`_base.css` <- animate.css -> `_vars.css`).
        // Custom properties resolve through the page cascade, so the
        // resolution walk must traverse IMPORT edges in both directions.
        let dir = tempfile::tempdir()?;
        let entry = dir.path().join("source/animate.css");
        let vars = dir.path().join("source/_vars.css");
        let base = dir.path().join("source/_base.css");
        let bounce = dir.path().join("source/anim/bounce.css");
        std::fs::create_dir_all(entry.parent().unwrap())?;
        std::fs::create_dir_all(bounce.parent().unwrap())?;
        std::fs::write(
            &entry,
            "@import '_vars.css';\n@import '_base.css';\n@import 'anim/bounce.css';\n",
        )?;
        std::fs::write(&vars, ":root { --animate-duration: 1s; }\n")?;
        std::fs::write(
            &base,
            ".animated { animation-duration: var(--animate-duration); animation-fill-mode: both; }\n",
        )?;
        std::fs::write(
            &bounce,
            "@keyframes bounce { to { opacity: 1; } }\n.bounce { animation-name: bounce; animation-duration: var(--animate-duration); }\n",
        )?;

        let mut storage = Storage::new_in_memory()?;
        let indexer = crate::WorkspaceIndexer::new(dir.path().to_path_buf());
        indexer.run_incremental(
            &mut storage,
            &RefreshInfo {
                mode: codestory_workspace::BuildMode::Incremental,
                files_to_index: vec![entry.clone(), vars.clone(), base.clone(), bounce.clone()],
                files_to_remove: Vec::new(),
                existing_file_ids: std::collections::HashMap::new(),
            },
            &EventBus::new(),
            None,
        )?;

        let nodes = storage.get_nodes()?;
        let edges = storage.get_edges()?;
        let vars_file_id = crate::WorkspaceIndexer::canonical_file_node_id_for_path(&vars);
        let base_file_id = crate::WorkspaceIndexer::canonical_file_node_id_for_path(&base);
        let bounce_file_id = crate::WorkspaceIndexer::canonical_file_node_id_for_path(&bounce);

        let duration = nodes
            .iter()
            .find(|node| {
                node.canonical_id.as_deref() == Some("css:var:--animate-duration")
                    && node.file_node_id == Some(NodeId(vars_file_id))
            })
            .expect("--animate-duration declaration in _vars.css");
        assert_eq!(duration.kind, NodeKind::VARIABLE);

        // The C3 join fact: the sibling base sheet's selector usage resolves
        // to the _vars.css declaration even though _base.css imports nothing.
        let animated = nodes
            .iter()
            .find(|node| {
                node.canonical_id.as_deref() == Some("css:class:animated")
                    && node.file_node_id == Some(NodeId(base_file_id))
            })
            .expect("_base.css selector node");
        let sibling_usage = edges
            .iter()
            .find(|edge| edge.kind == EdgeKind::USAGE && edge.source == animated.id)
            .expect("sibling var usage edge");
        assert_eq!(
            sibling_usage.resolved_target,
            Some(duration.id),
            "a sibling sheet's var() must resolve through the shared importer"
        );
        assert_eq!(
            sibling_usage.certainty,
            Some(codestory_contracts::graph::ResolutionCertainty::Certain)
        );

        // Two-hop component: the animation sheet's var() reaches _vars.css
        // through the importer as well.
        let bounce_selector = nodes
            .iter()
            .find(|node| {
                node.canonical_id.as_deref() == Some("css:class:bounce")
                    && node.file_node_id == Some(NodeId(bounce_file_id))
            })
            .expect("bounce selector node");
        assert!(
            edges.iter().any(|edge| {
                edge.kind == EdgeKind::USAGE
                    && edge.source == bounce_selector.id
                    && edge.resolved_target == Some(duration.id)
            }),
            "the animation sheet's var() resolves into _vars.css too"
        );

        // The C3 discriminator stays intact: the base selector has no
        // USAGE edge whose effective target is FUNCTION-kind, while the
        // animation selector has exactly its keyframe usage.
        let node_kind_of = |id: NodeId| {
            nodes
                .iter()
                .find(|node| node.id == id)
                .map(|node| node.kind)
        };
        assert!(
            !edges.iter().any(|edge| {
                edge.kind == EdgeKind::USAGE
                    && edge.source == animated.id
                    && node_kind_of(edge.resolved_target.unwrap_or(edge.target))
                        == Some(NodeKind::FUNCTION)
            }),
            "the base selector must keep zero USAGE->FUNCTION edges"
        );
        assert!(
            edges.iter().any(|edge| {
                edge.kind == EdgeKind::USAGE
                    && edge.source == bounce_selector.id
                    && node_kind_of(edge.resolved_target.unwrap_or(edge.target))
                        == Some(NodeKind::FUNCTION)
            }),
            "the animation selector carries its USAGE->FUNCTION edge"
        );
        Ok(())
    }
}
