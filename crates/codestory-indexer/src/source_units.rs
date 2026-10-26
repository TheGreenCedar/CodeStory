//! Exact physical source units selected from a supplied Python buffer.
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
    /// Python AST-compatible complete node kind, or `ancestor_header_context`.
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

/// Select the smallest complete Python callable/class containing each hit,
/// otherwise the sole containing top-level statement. Decorators belong to
/// their declaration. At most two nearest callable/class ancestor headers are
/// returned in outer-to-inner order, each bounded to the established 1024 bytes.
///
/// Work and source-byte allowances are shared across calls, never reset here.
/// Parse, traversal and request processing must be cooperatively bounded; an
/// unavailable, malformed, cancelled or exhausted compilation returns no units.
pub fn compile_python_source_units(
    source: &str,
    match_lines: &[usize],
    budget: &mut SourceUnitBudget,
    cancellation: &CancellationToken,
) -> Result<Vec<SourceUnitSelection>, SourceUnitError> {
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
    let language = crate::get_language_for_ext("py").ok_or(SourceUnitError::ParserUnavailable)?;
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

    let candidates = collect_candidates(tree.root_node(), &mut work)?;
    let by_line = index_lines(&candidates, starts.len(), &mut work)?;
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
        if let Some(owner) = parent.and_then(|f| f.body_owner) {
            if kind != "comment" && candidates[owner].first_body_line.is_none() {
                let statement = if kind == "decorated_definition" {
                    node.child_by_field_name("definition")
                        .ok_or(SourceUnitError::UnsupportedSyntax)?
                } else {
                    node
                };
                candidates[owner].first_body_line = Some(statement.start_position().row + 1);
            }
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
                kind: python_kind(node)?,
            });
            Some(id)
        } else {
            None
        };
        if kind == "except_group_clause" {
            if let Some(id) = parent.and_then(|f| f.candidate) {
                candidates[id].kind = "complete_TryStar";
            }
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
    work: &mut Work<'_>,
) -> Result<Vec<Option<usize>>, SourceUnitError> {
    let mut by_line = Vec::new();
    let mut callables = BinaryHeap::new();
    let mut statements: VecDeque<usize> = VecDeque::new();
    let mut next = 0;
    for line in 1..=lines {
        work.charge(1)?;
        while next < candidates.len() && candidates[next].start <= line {
            work.charge(1)?;
            let candidate = &candidates[next];
            if candidate.callable {
                work.charge(heap_work(callables.len()))?;
                callables.push(Reverse((
                    candidate.end - candidate.start,
                    next,
                    candidate.end,
                )));
            }
            if candidate.top_level {
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
        by_line.push(owner);
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
