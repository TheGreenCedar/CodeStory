//! Exact physical source units selected from a supplied Python, Rust or Go buffer.
//!
//! This boundary does not read files or produce graph evidence. Callers own
//! source identity and any subsequent admission, deduplication or rendering.

use crate::cancellation::CancellationToken;
use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::ops::ControlFlow;
use tree_sitter::{Node, ParseOptions, Parser};

/// One shared allowance across supplied buffers. No product limits are chosen here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUnitBudget {
    pub remaining_source_bytes: usize,
    pub remaining_work: usize,
    pub max_depth: usize,
}

/// A half-open UTF-8 byte extent covering complete physical lines, including
/// their existing terminators. One-based line endpoints are inclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUnit {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    /// Complete source AST kind, or `ancestor_header_context`.
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceUnitGap {
    InvalidMatchLine,
    NoEnclosingUnit,
    InlineAncestorHeader,
    AncestorHeaderTooLarge,
    AncestorDepthExceeded,
}

/// One entry per requested physical line, preserving input order and duplicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUnitSelection {
    pub match_line: usize,
    pub units: Vec<SourceUnit>,
    pub gaps: Vec<SourceUnitGap>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceUnitError {
    ParserUnavailable,
    UnsupportedSyntax,
    MalformedSource,
    SourceByteBudgetExceeded,
    WorkBudgetExceeded,
    DepthExceeded,
    Cancelled,
}

/// Compile exact physical source units for a supported file extension.
/// Named Python callable/class and Rust function/impl/trait/module owners use
/// the smallest enclosing supported extent. Go supplies only named function and
/// method owners with a body, including immediately attached comments.
/// At most two ancestor headers are
/// partial context, each bounded to 1024 bytes. No macro expansion or graph
/// semantics are inferred. Source/work allowances are shared across calls.
pub fn compile_source_units(
    extension: &str,
    source: &str,
    match_lines: &[usize],
    budget: &mut SourceUnitBudget,
    cancellation: &CancellationToken,
) -> Result<Vec<SourceUnitSelection>, SourceUnitError> {
    if !matches!(extension, "py" | "rs" | "go") {
        return Err(SourceUnitError::UnsupportedSyntax);
    }
    if cancellation.is_cancelled() {
        return Err(SourceUnitError::Cancelled);
    }
    if source.len() > budget.remaining_source_bytes {
        return Err(SourceUnitError::SourceByteBudgetExceeded);
    }
    budget.remaining_source_bytes -= source.len();
    let mut work = Work {
        budget,
        cancellation,
    };
    // Scan physical terminators once. A final LF does not invent an empty line.
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    if !source.is_empty() {
        starts.push(0);
    }
    for (offset, byte) in source.bytes().enumerate() {
        work.charge(1)?;
        if byte == b'\n' {
            ends.push(offset + 1);
            if offset + 1 < source.len() {
                starts.push(offset + 1);
            }
        }
    }
    if ends.len() < starts.len() {
        ends.push(source.len());
    }

    let mut parser = Parser::new();
    let language =
        crate::get_language_for_ext(extension).ok_or(SourceUnitError::ParserUnavailable)?;
    parser
        .set_language(&language.language)
        .map_err(|_| SourceUnitError::ParserUnavailable)?;
    // Both lexer input and native parser progress debit this same allowance.
    // Tree-sitter's progress callback supplies cooperative cancellation during
    // parsing; these are bounded work units, not an instruction/wall-clock meter.
    let (tree, parse_error) = {
        let shared = RefCell::new(&mut work);
        let error = Cell::new(None);
        let mut input = |offset: usize, _| {
            let bytes = source.as_bytes();
            let chunk = bytes
                .get(offset..offset.saturating_add(1024).min(bytes.len()))
                .unwrap_or_default();
            if let Err(reason) = shared.borrow_mut().charge(chunk.len().max(1)) {
                error.set(Some(reason));
                return &[][..];
            }
            chunk
        };
        let mut progress = |_: &tree_sitter::ParseState| {
            if error.get().is_some() {
                return ControlFlow::Break(());
            }
            match shared.borrow_mut().charge(1) {
                Ok(()) => ControlFlow::Continue(()),
                Err(reason) => {
                    error.set(Some(reason));
                    ControlFlow::Break(())
                }
            }
        };
        let tree = parser.parse_with_options(
            &mut input,
            None,
            Some(ParseOptions::new().progress_callback(&mut progress)),
        );
        (tree, error.get())
    };
    if let Some(error) = parse_error {
        return Err(error);
    }
    let tree = tree.ok_or(SourceUnitError::ParserUnavailable)?;
    if tree.root_node().has_error() {
        return Err(SourceUnitError::MalformedSource);
    }

    let braced = matches!(extension, "rs" | "go");
    let candidates = if braced {
        collect_braced_candidates(tree.root_node(), source, extension == "go", &mut work)?
    } else {
        collect_candidates(tree.root_node(), &mut work)?
    };
    let by_line = index_lines(&candidates, starts.len(), braced, &mut work)?;
    let mut selections = Vec::new();
    for &line in match_lines {
        work.charge(1)?;
        let mut selection = SourceUnitSelection {
            match_line: line,
            units: Vec::new(),
            gaps: Vec::new(),
        };
        if line == 0 || line > starts.len() {
            selection.gaps.push(SourceUnitGap::InvalidMatchLine);
        } else if let Some(owner) = by_line[line - 1] {
            let candidate = &candidates[owner];
            work.charge(1)?;
            selection.units.push(unit(
                candidate.start,
                candidate.end,
                candidate.kind,
                &starts,
                &ends,
            ));
            let mut parent = candidate.parent;
            for _ in 0..2 {
                let Some(id) = parent else {
                    break;
                };
                work.charge(1)?;
                let ancestor = &candidates[id];
                let header_end = ancestor
                    .first_body_line
                    .ok_or(SourceUnitError::UnsupportedSyntax)?
                    - 1;
                if header_end < ancestor.declaration_line {
                    selection.gaps.push(SourceUnitGap::InlineAncestorHeader);
                } else if ends[header_end - 1] - starts[ancestor.start - 1] > 1024 {
                    selection.gaps.push(SourceUnitGap::AncestorHeaderTooLarge);
                } else {
                    selection.units.insert(
                        0,
                        unit(
                            ancestor.start,
                            header_end,
                            "ancestor_header_context",
                            &starts,
                            &ends,
                        ),
                    );
                }
                parent = ancestor.parent;
            }
            if parent.is_some() {
                selection.gaps.push(SourceUnitGap::AncestorDepthExceeded);
            }
        } else {
            selection.gaps.push(SourceUnitGap::NoEnclosingUnit);
        }
        selections.push(selection);
    }
    work.charge(0)?;
    Ok(selections)
}

struct Work<'a> {
    budget: &'a mut SourceUnitBudget,
    cancellation: &'a CancellationToken,
}

impl Work<'_> {
    fn charge(&mut self, amount: usize) -> Result<(), SourceUnitError> {
        if self.cancellation.is_cancelled() {
            return Err(SourceUnitError::Cancelled);
        }
        if amount > self.budget.remaining_work {
            self.budget.remaining_work = 0;
            return Err(SourceUnitError::WorkBudgetExceeded);
        }
        self.budget.remaining_work -= amount;
        Ok(())
    }
}

struct Candidate {
    start: usize,
    end: usize,
    declaration_line: usize,
    first_body_line: Option<usize>,
    body_id: Option<usize>,
    parent: Option<usize>,
    callable: bool,
    top_level: bool,
    /// A complete physical envelope cannot contain another root declaration.
    admissible: bool,
    kind: &'static str,
}

struct Frame<'tree> {
    node: Node<'tree>,
    candidate: Option<usize>,
    active_owner: Option<usize>,
    body_owner: Option<usize>,
    top_level_wrapper: bool,
    last_code_line: Option<usize>,
}

/// One iterative entry/exit traversal, including punctuation. Comment leaves
/// contribute no AST endpoint, unlike tree-sitter block/declaration ranges.
fn collect_candidates(
    root: Node<'_>,
    work: &mut Work<'_>,
) -> Result<Vec<Candidate>, SourceUnitError> {
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut frames: Vec<Frame<'_>> = Vec::new();
    let mut cursor = root.walk();
    'walk: loop {
        work.charge(1)?;
        if frames.len() >= work.budget.max_depth {
            return Err(SourceUnitError::DepthExceeded);
        }
        let node = cursor.node();
        if node.is_error() || node.is_missing() {
            return Err(SourceUnitError::MalformedSource);
        }
        let kind = node.kind();
        let callable = matches!(kind, "function_definition" | "class_definition");
        let parent = frames.last();
        let parent_owner = parent.and_then(|f| f.active_owner);
        let top_level =
            parent.is_some_and(|f| f.node.kind() == "module" || (f.top_level_wrapper && callable));
        if let Some(owner) = parent.and_then(|f| f.body_owner)
            && kind != "comment"
            && candidates[owner].first_body_line.is_none()
        {
            let statement = if kind == "decorated_definition" {
                node.child_by_field_name("definition")
                    .ok_or(SourceUnitError::UnsupportedSyntax)?
            } else {
                node
            };
            candidates[owner].first_body_line = Some(statement.start_position().row + 1);
        }
        let candidate = if callable
            || (top_level && node.is_named() && !matches!(kind, "comment" | "decorated_definition"))
        {
            let start =
                if callable && parent.is_some_and(|f| f.node.kind() == "decorated_definition") {
                    parent.unwrap().node.start_position().row + 1
                } else {
                    node.start_position().row + 1
                };
            // DFS declaration order is also physical-start order, including
            // decorated wrappers. No sorting or per-hit tree search is needed.
            if candidates.last().is_some_and(|c| c.start > start) {
                return Err(SourceUnitError::UnsupportedSyntax);
            }
            let id = candidates.len();
            candidates.push(Candidate {
                start,
                end: start,
                declaration_line: node.start_position().row + 1,
                first_body_line: None,
                body_id: node.child_by_field_name("body").map(|n| n.id()),
                parent: parent_owner,
                callable,
                top_level,
                admissible: true,
                kind: python_kind(node)?,
            });
            Some(id)
        } else {
            None
        };
        if kind == "except_group_clause"
            && let Some(id) = parent.and_then(|f| f.candidate)
        {
            candidates[id].kind = "complete_TryStar";
        }
        let body_owner = if kind == "block" {
            parent_owner.filter(|&id| candidates[id].body_id == Some(node.id()))
        } else {
            None
        };
        let position = node.end_position();
        let last_code_line =
            (node.child_count() == 0 && kind != "comment" && node.end_byte() > node.start_byte())
                .then_some(position.row + usize::from(position.column != 0));
        frames.push(Frame {
            node,
            candidate,
            active_owner: if callable { candidate } else { parent_owner },
            body_owner,
            top_level_wrapper: kind == "decorated_definition" && top_level,
            last_code_line,
        });
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            work.charge(1)?;
            let finished = frames.pop().ok_or(SourceUnitError::MalformedSource)?;
            if let Some(id) = finished.candidate {
                candidates[id].end = finished
                    .last_code_line
                    .ok_or(SourceUnitError::MalformedSource)?;
            }
            if let Some(parent) = frames.last_mut() {
                parent.last_code_line = parent.last_code_line.max(finished.last_code_line);
            }
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    Ok(candidates)
}

struct BracedFrame<'tree> {
    node: Node<'tree>,
    owner: Option<usize>,
    /// Outer attributes/doc comments awaiting the next item in this scope.
    envelope: Option<usize>,
    /// Go comment adjacency is tracked once per direct sibling, without scanning
    /// prior source or walking ancestors for every comment/observation.
    envelope_end: Option<usize>,
    last_direct_end: Option<usize>,
    /// Last noncomment root declaration and its optional named-owner candidate.
    /// Noncallable package/import/type/var siblings participate equally.
    last_declaration: Option<(usize, Option<usize>)>,
}

/// Rust and Go own their complete closing punctuation, unlike Python's last-statement
/// endpoint. Carry pending outer envelopes and the nearest named owner through
/// one traversal; token trees and closures never create fabricated functions.
fn collect_braced_candidates(
    root: Node<'_>,
    source: &str,
    go: bool,
    work: &mut Work<'_>,
) -> Result<Vec<Candidate>, SourceUnitError> {
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut frames: Vec<BracedFrame<'_>> = Vec::new();
    let mut cursor = root.walk();
    'walk: loop {
        work.charge(1)?;
        if frames.len() >= work.budget.max_depth {
            return Err(SourceUnitError::DepthExceeded);
        }
        let node = cursor.node();
        if node.is_error() || node.is_missing() {
            return Err(SourceUnitError::MalformedSource);
        }
        let kind = node.kind();
        let parent_owner = frames.last().and_then(|frame| frame.owner);
        let mut start = node.start_position().row + 1;
        let mut admissible = true;
        let mut root_declaration_end = None;
        if go {
            if let Some(parent) = frames.last_mut()
                && parent.node.kind() == "source_file"
            {
                work.charge(1)?;
                let position = node.end_position();
                let end = position.row + usize::from(position.column != 0);
                if kind == "comment" {
                    // A trailing comment on a previous declaration's physical
                    // row belongs to that row, not the following callable.
                    if parent.last_direct_end.is_none_or(|last| last < start) {
                        if parent.envelope_end != start.checked_sub(1) {
                            parent.envelope = Some(start);
                        }
                        parent.envelope_end = Some(end);
                    } else {
                        parent.envelope = None;
                        parent.envelope_end = None;
                    }
                } else if node.is_named() {
                    work.charge(1)?;
                    // AST siblings can be byte-disjoint but share a physical
                    // row. Invalidate the entire neighboring owner envelopes,
                    // including earlier body/doc hits, for ALL declarations.
                    if let Some((last_end, previous_owner)) = parent.last_declaration
                        && last_end >= start
                    {
                        admissible = false;
                        if let Some(id) = previous_owner {
                            candidates[id].admissible = false;
                        }
                    }
                    root_declaration_end = Some(end);
                    if parent.envelope_end == start.checked_sub(1) {
                        start = parent.envelope.take().unwrap_or(start);
                    } else {
                        parent.envelope = None;
                    }
                    parent.envelope_end = None;
                }
                if node.end_byte() > node.start_byte() {
                    parent.last_direct_end = Some(parent.last_direct_end.unwrap_or(0).max(end));
                }
            }
        } else if let Some(parent) = frames.last_mut()
            && matches!(
                parent.node.kind(),
                "source_file" | "declaration_list" | "block"
            )
        {
            work.charge(1)?;
            let prefix = source
                .as_bytes()
                .get(node.start_byte()..)
                .unwrap_or_default();
            let outer_doc = (prefix.starts_with(b"///") && !prefix.starts_with(b"////"))
                || (prefix.starts_with(b"/**") && !prefix.starts_with(b"/***"));
            match kind {
                "attribute_item" => {
                    parent.envelope.get_or_insert(start);
                }
                "line_comment" | "block_comment" if outer_doc => {
                    parent.envelope.get_or_insert(start);
                }
                "line_comment" | "block_comment"
                    if !prefix.starts_with(b"//!") && !prefix.starts_with(b"/*!") => {}
                _ => {
                    start = parent.envelope.take().unwrap_or(start);
                }
            }
        }
        let unit_kind = if go {
            go_kind(kind).filter(|_| node.child_by_field_name("body").is_some())
        } else {
            rust_kind(kind)
        };
        let candidate = if let Some(unit_kind) = unit_kind {
            if candidates
                .last()
                .is_some_and(|candidate| candidate.start > start)
            {
                return Err(SourceUnitError::UnsupportedSyntax);
            }
            let body = node.child_by_field_name("body");
            let position = node.end_position();
            let id = candidates.len();
            candidates.push(Candidate {
                start,
                end: position.row + usize::from(position.column != 0),
                declaration_line: node.start_position().row + 1,
                // Rust partial headers include the opening brace, including
                // multiline generic/where clauses, but none of the body items.
                first_body_line: body.map(|body| body.start_position().row + 2),
                body_id: body.map(|body| body.id()),
                parent: parent_owner,
                callable: true,
                top_level: parent_owner.is_none(),
                admissible,
                kind: unit_kind,
            });
            Some(id)
        } else {
            None
        };
        if let Some(end) = root_declaration_end {
            let parent = frames.last_mut().ok_or(SourceUnitError::MalformedSource)?;
            parent.last_declaration = Some((end, candidate));
        }
        frames.push(BracedFrame {
            node,
            owner: candidate.or(parent_owner),
            envelope: None,
            envelope_end: None,
            last_direct_end: None,
            last_declaration: None,
        });
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            work.charge(1)?;
            frames.pop().ok_or(SourceUnitError::MalformedSource)?;
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    Ok(candidates)
}

fn go_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "function_declaration" => Some("complete_function_declaration"),
        "method_declaration" => Some("complete_method_declaration"),
        _ => None,
    }
}

fn rust_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function_item" => "complete_function_item",
        "function_signature_item" => "complete_function_signature_item",
        "impl_item" => "complete_impl_item",
        "trait_item" => "complete_trait_item",
        "mod_item" => "complete_mod_item",
        _ => return None,
    })
}

fn python_kind(node: Node<'_>) -> Result<&'static str, SourceUnitError> {
    let asynchronous = node.child(0).is_some_and(|n| n.kind() == "async");
    Ok(match node.kind() {
        "function_definition" if asynchronous => "complete_AsyncFunctionDef",
        "function_definition" => "complete_FunctionDef",
        "class_definition" => "complete_ClassDef",
        "import_statement" => "complete_Import",
        "import_from_statement" | "future_import_statement" => "complete_ImportFrom",
        "expression_statement" => match node
            .named_child(0)
            .map(|n| (n.kind(), n.child_by_field_name("type").is_some()))
        {
            Some(("assignment", true)) => "complete_AnnAssign",
            Some(("assignment", false)) => "complete_Assign",
            Some(("augmented_assignment", _)) => "complete_AugAssign",
            _ => "complete_Expr",
        },
        "if_statement" => "complete_If",
        "for_statement" if asynchronous => "complete_AsyncFor",
        "for_statement" => "complete_For",
        "while_statement" => "complete_While",
        "with_statement" if asynchronous => "complete_AsyncWith",
        "with_statement" => "complete_With",
        "try_statement" => "complete_Try",
        "match_statement" => "complete_Match",
        "assert_statement" => "complete_Assert",
        "break_statement" => "complete_Break",
        "continue_statement" => "complete_Continue",
        "delete_statement" => "complete_Delete",
        "global_statement" => "complete_Global",
        "nonlocal_statement" => "complete_Nonlocal",
        "pass_statement" => "complete_Pass",
        "raise_statement" => "complete_Raise",
        "return_statement" => "complete_Return",
        "type_alias_statement" => "complete_TypeAlias",
        _ => return Err(SourceUnitError::UnsupportedSyntax),
    })
}

/// Sweep physical lines once. The heap chooses the smallest callable extent;
/// top-level overlap counts preserve the prototype's ambiguous same-line refusal.
fn index_lines(
    candidates: &[Candidate],
    lines: usize,
    braced: bool,
    work: &mut Work<'_>,
) -> Result<Vec<Option<usize>>, SourceUnitError> {
    let mut ambiguous = vec![false; lines];
    if braced {
        // Disjoint sibling items can share one physical row. A row-only match
        // cannot choose either. This is one candidate pass, never per-hit work.
        let mut previous: Vec<Option<usize>> = vec![None; candidates.len() + 1];
        for (id, candidate) in candidates.iter().enumerate() {
            work.charge(1)?;
            let scope = candidate.parent.map_or(0, |parent| parent + 1);
            if let Some(prior) = previous[scope]
                && candidates[prior].end >= candidate.start
            {
                ambiguous[candidate.start - 1] = true;
            }
            if let Some(parent) = candidate.parent {
                if candidate.start == candidates[parent].declaration_line {
                    ambiguous[candidate.start - 1] = true;
                }
                if candidate.end == candidates[parent].end {
                    ambiguous[candidate.end - 1] = true;
                }
            }
            previous[scope] = Some(id);
        }
    }
    let mut by_line = Vec::new();
    let mut callables = BinaryHeap::new();
    let mut statements: VecDeque<usize> = VecDeque::new();
    let mut next = 0;
    for line in 1..=lines {
        work.charge(1)?;
        while next < candidates.len() && candidates[next].start <= line {
            work.charge(1)?;
            let candidate = &candidates[next];
            if candidate.callable && candidate.admissible {
                work.charge(heap_work(callables.len()))?;
                callables.push(Reverse((
                    candidate.end - candidate.start,
                    next,
                    candidate.end,
                )));
            }
            if candidate.top_level && candidate.admissible {
                statements.push_back(next);
            }
            next += 1;
        }
        while callables
            .peek()
            .is_some_and(|Reverse((_, _, end))| *end < line)
        {
            work.charge(heap_work(callables.len()))?;
            callables.pop();
        }
        while statements
            .front()
            .is_some_and(|&id| candidates[id].end < line)
        {
            work.charge(1)?;
            statements.pop_front();
        }
        let owner = callables
            .peek()
            .map(|Reverse((_, id, _))| *id)
            .or_else(|| (statements.len() == 1).then(|| statements[0]));
        by_line.push(if ambiguous[line - 1] { None } else { owner });
    }
    Ok(by_line)
}

fn heap_work(size: usize) -> usize {
    (usize::BITS - size.saturating_add(1).leading_zeros()) as usize + 1
}

fn unit(start: usize, end: usize, kind: &str, starts: &[usize], ends: &[usize]) -> SourceUnit {
    SourceUnit {
        start_byte: starts[start - 1],
        end_byte: ends[end - 1],
        start_line: start,
        end_line: end,
        kind: kind.to_owned(),
    }
}

#[cfg(test)]
#[path = "source_units_go_tests.rs"]
mod go_tests;
