//! Pure supplied-buffer local Go slice compiler. No file, graph or runtime owner.
//!
//! Its inputs reuse `SourceUnitBudget`, `CancellationToken` and an exact
//! half-open UTF-8 expression byte range; failures reuse `SourceUnitError`.

use super::SourceUnit;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoSliceRole {
    Criterion,
    Binding,
    Definition,
    Control,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoSliceWitness {
    pub role: GoSliceRole,
    /// Exact dependency-role bytes; physical row context may display siblings.
    pub range: Range<usize>,
    pub unit: SourceUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoSliceBoundaryKind {
    ParameterInput,
    PackageInput,
    OpaqueCall,
    IndirectWrite,
    ClosureWrite,
    UnsupportedConstruct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoSliceBoundary {
    /// Exact supplied-buffer bytes of the input use or opaque operation.
    pub range: Range<usize>,
    pub kind: GoSliceBoundaryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoSliceGap {
    InvalidCriterion,
    UnsupportedConstruct,
    LoopCarriedState,
    UnknownMutation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoExpressionSlice {
    pub criterion: Range<usize>,
    pub witnesses: Vec<GoSliceWitness>,
    pub boundaries: Vec<GoSliceBoundary>,
    pub gaps: Vec<GoSliceGap>,
    /// Only supported local dependencies in a fully scanned function. Never
    /// execution, external-call semantics, feasibility or global absence proof.
    pub complete_local: bool,
}

use super::{
    CancellationToken, SourceUnitBudget, SourceUnitError, Work, heap_work, parse_source,
    physical_lines,
};
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

type FactId = usize;

enum EffectScope {
    None,
    // Monotone may-exposure, independent of dependency DAG membership.
    Exposure(Vec<FactId>),
    Write {
        bindings: Vec<FactId>,
        content: bool,
        cycle: bool,
    },
    // Unmodeled execution/control, rather than an invented binding write.
    Global,
}

struct Fact {
    range: Range<usize>,
    role: Option<GoSliceRole>,
    boundary: Option<GoSliceBoundaryKind>,
    gap: Option<GoSliceGap>,
    scope: EffectScope,
    parents: Vec<FactId>,
}

#[derive(Clone, Default)]
struct Value<'tree> {
    roots: Vec<FactId>,
    effects: Vec<FactId>,
    closure_writes: Vec<Node<'tree>>,
}

#[derive(Clone)]
struct Variable<'tree> {
    binding: FactId,
    // Effects older than a dominating local write cannot rewrite that version.
    // Joins take the earliest cutoff across all reaching branches.
    written_at: FactId,
    value: Value<'tree>,
}

impl Value<'_> {
    fn cost(&self) -> usize {
        1usize
            .saturating_add(self.roots.len())
            .saturating_add(self.effects.len())
            .saturating_add(self.closure_writes.len())
    }
}

#[derive(Clone, Default)]
struct Environment<'tree> {
    scopes: Vec<BTreeMap<String, Variable<'tree>>>,
    // A conservative cached upper bound for copying, visiting or dropping the
    // current environment. Updates and popped scopes never refund this weight.
    // Computing a charge must itself be constant work, not an uncharged walk.
    weight: usize,
}

impl<'tree> Environment<'tree> {
    fn lookup(&self, name: &str) -> Option<&Variable<'tree>> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    fn scope_of(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rposition(|scope| scope.contains_key(name))
    }

    fn cost(&self) -> usize {
        self.weight
    }

    fn push(&mut self) {
        self.weight = self.weight.saturating_add(1);
        self.scopes.push(BTreeMap::new());
    }

    // Callers debit the old environment and new entry before this operation.
    fn insert(&mut self, scope: usize, name: String, variable: Variable<'tree>) {
        self.weight = self
            .weight
            .saturating_add(name.len())
            .saturating_add(1)
            .saturating_add(variable.value.cost());
        self.scopes[scope].insert(name, variable);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Transfer {
    Return,
    Continue,
    Break,
}

#[derive(Clone, Default)]
struct Flow<'tree> {
    env: Environment<'tree>,
    controls: Vec<FactId>,
    effects: Vec<FactId>,
    transfer: Option<(Transfer, FactId)>,
}

struct Slicer<'source, 'work, 'budget, 'tree> {
    source: &'source str,
    work: &'work mut Work<'budget>,
    target: Node<'tree>,
    starts: Vec<usize>,
    ends: Vec<usize>,
    facts: Vec<Fact>,
    selected: Option<FactId>,
}

/// Pure, local, normal-flow dependency witnesses. Calls are opaque; in
/// particular, their spelling never proves termination or library behavior.
/// Unsupported relevant effects retain a source-located gap. The experiment is
/// supplied-buffer only; runtime owns authenticated reads and public projection.
pub fn compile_go_expression_slice(
    source: &str,
    criterion: Range<usize>,
    budget: &mut SourceUnitBudget,
    cancellation: &CancellationToken,
) -> Result<GoExpressionSlice, SourceUnitError> {
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
    let (starts, ends) = physical_lines(source, &mut work)?;
    let tree = parse_source("go", source, &mut work)?;
    // One bounded full-tree pass validates depth and finds an exact expression;
    // a text occurrence in a comment, literal fragment or declaration is not a
    // reason to retarget to its nearest expression.
    let valid_range = criterion.start < criterion.end
        && criterion.end <= source.len()
        && source.is_char_boundary(criterion.start)
        && source.is_char_boundary(criterion.end);
    let mut found = None;
    let mut backward_control = Vec::new();
    let mut stack = vec![(tree.root_node(), 0usize, None, false)];
    while let Some((node, depth, owner, in_literal)) = stack.pop() {
        work.charge(1)?;
        if depth >= work.budget.max_depth {
            return Err(SourceUnitError::DepthExceeded);
        }
        let owner = if matches!(node.kind(), "function_declaration" | "method_declaration") {
            Some(node)
        } else {
            owner
        };
        let in_literal = in_literal || node.kind() == "func_literal";
        if matches!(node.kind(), "goto_statement" | "labeled_statement") {
            backward_control.push(node);
        }
        if valid_range
            && node.start_byte() == criterion.start
            && node.end_byte() == criterion.end
            && expression(node.kind())
            && let Some(owner) = owner
            && owner
                .child_by_field_name("body")
                .is_some_and(|body| contains(body, node))
        {
            found = Some((node, owner, in_literal));
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            work.charge(1)?;
            stack.push((child, depth + 1, owner, in_literal));
        }
    }
    let Some((target, owner, in_literal)) = found else {
        work.charge(1)?;
        return Ok(GoExpressionSlice {
            criterion,
            witnesses: Vec::new(),
            boundaries: Vec::new(),
            gaps: vec![GoSliceGap::InvalidCriterion],
            complete_local: false,
        });
    };
    let mut slicer = Slicer {
        source,
        work: &mut work,
        target,
        starts,
        ends,
        facts: Vec::new(),
        selected: None,
    };
    let mut flow = Flow::default();
    flow.env.push();
    // Syntax after the criterion is normally irrelevant, but unsupported
    // jumps can revisit it after later writes. The full-tree pass above must
    // fence those constructs before prefix evaluation can certify locality.
    for node in backward_control {
        slicer.work.charge(1)?;
        if contains(owner, node) {
            let gap = slicer.fact(
                node.byte_range(),
                None,
                Some(GoSliceBoundaryKind::UnsupportedConstruct),
                Some(GoSliceGap::UnsupportedConstruct),
                Vec::new(),
            )?;
            slicer.facts[gap].scope = EffectScope::Global;
            flow.effects.push(gap);
        }
    }
    for field in ["receiver", "parameters"] {
        if let Some(parameters) = owner.child_by_field_name(field) {
            slicer.parameters(parameters, &mut flow, 0)?;
        }
    }
    if let Some(results) = owner.child_by_field_name("result") {
        if results.kind() == "parameter_list" {
            // Named results are local bindings with implicit initialization and
            // naked-return behavior; this first local model does not infer it.
            let gap = slicer.fact(
                results.byte_range(),
                None,
                Some(GoSliceBoundaryKind::UnsupportedConstruct),
                Some(GoSliceGap::UnsupportedConstruct),
                Vec::new(),
            )?;
            slicer.facts[gap].scope = EffectScope::Global;
            flow.effects.push(gap);
        }
    }
    if in_literal {
        let gap = slicer.fact(
            target.byte_range(),
            None,
            Some(GoSliceBoundaryKind::UnsupportedConstruct),
            Some(GoSliceGap::UnsupportedConstruct),
            Vec::new(),
        )?;
        slicer.capture(
            target,
            Value {
                roots: vec![gap],
                ..Value::default()
            },
            &[],
            &[],
        )?;
    } else if let Some(body) = owner.child_by_field_name("body") {
        slicer.statements(body, &mut flow, 0)?;
    }
    if slicer.selected.is_none() {
        // Exact syntax that this local evaluator cannot visit (for example a
        // write location or unreachable suffix) remains explicit, not fuzzy.
        let gap = slicer.fact(
            target.byte_range(),
            None,
            Some(GoSliceBoundaryKind::UnsupportedConstruct),
            Some(GoSliceGap::UnsupportedConstruct),
            Vec::new(),
        )?;
        slicer.capture(
            target,
            Value {
                roots: vec![gap],
                ..Value::default()
            },
            &[],
            &[],
        )?;
    }
    slicer.work.charge(
        flow.env
            .cost()
            .saturating_add(flow.controls.len())
            .saturating_add(flow.effects.len()),
    )?;
    drop(flow);
    slicer.finish(criterion)
}

fn contains(outer: Node<'_>, inner: Node<'_>) -> bool {
    outer.start_byte() <= inner.start_byte() && inner.end_byte() <= outer.end_byte()
}

fn expression(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "binary_expression"
            | "call_expression"
            | "composite_literal"
            | "false"
            | "true"
            | "float_literal"
            | "imaginary_literal"
            | "int_literal"
            | "nil"
            | "iota"
            | "func_literal"
            | "index_expression"
            | "interpreted_string_literal"
            | "raw_string_literal"
            | "rune_literal"
            | "parenthesized_expression"
            | "selector_expression"
            | "slice_expression"
            | "type_assertion_expression"
            | "type_conversion_expression"
            | "type_instantiation_expression"
            | "unary_expression"
    )
}

impl<'source, 'work, 'budget, 'tree> Slicer<'source, 'work, 'budget, 'tree> {
    fn step(&mut self, depth: usize) -> Result<(), SourceUnitError> {
        self.work.charge(1)?;
        if depth >= self.work.budget.max_depth {
            return Err(SourceUnitError::DepthExceeded);
        }
        Ok(())
    }

    fn text(&self, node: Node<'tree>) -> &'source str {
        &self.source[node.byte_range()]
    }

    fn children(&mut self, node: Node<'tree>) -> Result<Vec<Node<'tree>>, SourceUnitError> {
        let mut children = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.work.charge(1)?;
            children.push(child);
        }
        Ok(children)
    }

    fn ids(&mut self, mut ids: Vec<FactId>) -> Result<Vec<FactId>, SourceUnitError> {
        self.work
            .charge(ids.len().saturating_mul(heap_work(ids.len())))?;
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    fn fact(
        &mut self,
        range: Range<usize>,
        role: Option<GoSliceRole>,
        boundary: Option<GoSliceBoundaryKind>,
        gap: Option<GoSliceGap>,
        parents: Vec<FactId>,
    ) -> Result<FactId, SourceUnitError> {
        self.work.charge(1usize.saturating_add(parents.len()))?;
        let parents = self.ids(parents)?;
        let id = self.facts.len();
        self.facts.push(Fact {
            range,
            role,
            boundary,
            gap,
            scope: EffectScope::None,
            parents,
        });
        Ok(id)
    }

    fn copy_flow(&mut self, flow: &Flow<'tree>) -> Result<Flow<'tree>, SourceUnitError> {
        self.work.charge(
            flow.env
                .cost()
                .saturating_add(flow.controls.len())
                .saturating_add(flow.effects.len())
                .saturating_mul(2),
        )?;
        Ok(flow.clone())
    }

    fn copy_value(&mut self, value: &Value<'tree>) -> Result<Value<'tree>, SourceUnitError> {
        self.work.charge(value.cost().saturating_mul(2))?;
        Ok(value.clone())
    }

    fn copy_ids(&mut self, ids: &[FactId]) -> Result<Vec<FactId>, SourceUnitError> {
        self.work.charge(ids.len())?;
        Ok(ids.to_vec())
    }

    fn add_effects(
        &mut self,
        roots: &mut Vec<FactId>,
        effects: &[FactId],
        binding: Option<(FactId, FactId)>,
        reads_content: bool,
    ) -> Result<(), SourceUnitError> {
        self.work
            .charge(roots.len().saturating_add(effects.len()))?;
        for &id in effects {
            let retain = match &self.facts[id].scope {
                EffectScope::Global => true,
                EffectScope::Write {
                    bindings,
                    content,
                    cycle,
                } => {
                    self.work.charge(bindings.len())?;
                    (reads_content && *content)
                        || binding.is_some_and(|(target, cutoff)| {
                            (*cycle || id > cutoff) && bindings.contains(&target)
                        })
                }
                EffectScope::None | EffectScope::Exposure(_) => false,
            };
            if retain {
                roots.push(id);
            }
        }
        Ok(())
    }

    fn visible_bindings(
        &mut self,
        env: &Environment<'tree>,
    ) -> Result<Vec<FactId>, SourceUnitError> {
        self.work.charge(env.cost())?;
        let bindings = env
            .scopes
            .iter()
            .flat_map(|scope| scope.values().map(|v| v.binding))
            .collect();
        self.ids(bindings)
    }

    fn exposures(
        &mut self,
        effects: &[FactId],
    ) -> Result<(Vec<FactId>, Vec<FactId>), SourceUnitError> {
        self.work.charge(effects.len())?;
        let mut targets = Vec::new();
        let mut events = Vec::new();
        for &id in effects {
            if let EffectScope::Exposure(bindings) = &self.facts[id].scope {
                self.work.charge(
                    targets
                        .len()
                        .saturating_add(bindings.len())
                        .saturating_add(1),
                )?;
                targets.extend_from_slice(bindings);
                events.push(id);
            }
        }
        Ok((self.ids(targets)?, self.ids(events)?))
    }

    fn expose(
        &mut self,
        node: Node<'tree>,
        targets: Vec<FactId>,
    ) -> Result<Option<FactId>, SourceUnitError> {
        let targets = self.ids(targets)?;
        if targets.is_empty() {
            return Ok(None);
        }
        let parents = self.copy_ids(&targets)?;
        let event = self.fact(node.byte_range(), None, None, None, parents)?;
        self.facts[event].scope = EffectScope::Exposure(targets);
        Ok(Some(event))
    }

    // Only syntactically named storage is resolved. Dereferenced/unknown bases
    // stay conservative; this is not a points-to or new lexical-binding engine.
    fn local_base(
        &mut self,
        node: Node<'tree>,
        env: &Environment<'tree>,
        depth: usize,
    ) -> Result<Option<FactId>, SourceUnitError> {
        self.step(depth)?;
        self.work
            .charge(env.cost().saturating_add(node.child_count()))?;
        match node.kind() {
            "identifier" => Ok(env.lookup(self.text(node)).map(|v| v.binding)),
            "selector_expression" | "index_expression" | "slice_expression" => {
                let base = node.child_by_field_name("operand");
                if let Some(base) = base {
                    self.local_base(base, env, depth + 1)
                } else {
                    Ok(None)
                }
            }
            "parenthesized_expression" => {
                let children = self.children(node)?;
                if let Some(&base) = children.first() {
                    self.local_base(base, env, depth + 1)
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn capture(
        &mut self,
        node: Node<'tree>,
        mut value: Value<'tree>,
        controls: &[FactId],
        effects: &[FactId],
    ) -> Result<(), SourceUnitError> {
        self.work
            .charge(value.cost().saturating_add(controls.len()))?;
        value.roots.extend_from_slice(controls);
        self.add_effects(&mut value.roots, effects, None, false)?;
        self.add_effects(&mut value.roots, &value.effects, None, false)?;
        let criterion = self.fact(
            node.byte_range(),
            Some(GoSliceRole::Criterion),
            None,
            None,
            value.roots,
        )?;
        self.selected = Some(criterion);
        Ok(())
    }

    /// All statement-level consumers commit evaluated effects here before
    /// inspecting or discarding a value. Stored values do not own those effects
    /// a second time; the monotone flow ledger also covers blank results.
    fn eval_in_flow(
        &mut self,
        node: Node<'tree>,
        flow: &mut Flow<'tree>,
        depth: usize,
    ) -> Result<Value<'tree>, SourceUnitError> {
        let mut value = self.eval(node, &flow.env, &flow.controls, &flow.effects, depth)?;
        self.work
            .charge(flow.effects.len().saturating_add(value.effects.len()))?;
        flow.effects.append(&mut value.effects);
        flow.effects = self.ids(std::mem::take(&mut flow.effects))?;
        Ok(value)
    }

    fn parameters(
        &mut self,
        node: Node<'tree>,
        flow: &mut Flow<'tree>,
        depth: usize,
    ) -> Result<(), SourceUnitError> {
        self.step(depth)?;
        self.work.charge(node.child_count())?;
        if matches!(
            node.kind(),
            "parameter_declaration" | "variadic_parameter_declaration"
        ) {
            // A supplied file has no closed package/type binding proof, even
            // for predeclared spellings. Inputs remain opaque; content effects
            // are conservative for every call, not inferred from type text.
            let mut cursor = node.walk();
            let mut names = Vec::new();
            for name in node.children_by_field_name("name", &mut cursor) {
                self.work.charge(1)?;
                names.push(name);
            }
            for name in names {
                self.work.charge(name.byte_range().len())?;
                let spelling = self.text(name).to_owned();
                let input = self.fact(
                    name.byte_range(),
                    None,
                    Some(GoSliceBoundaryKind::ParameterInput),
                    None,
                    Vec::new(),
                )?;
                let variable = Variable {
                    binding: input,
                    written_at: input,
                    value: Value {
                        roots: vec![input],
                        ..Value::default()
                    },
                };
                self.work.charge(
                    flow.env
                        .cost()
                        .saturating_add(spelling.len())
                        .saturating_add(variable.value.cost()),
                )?;
                flow.env.insert(0, spelling, variable);
            }
        } else {
            for child in self.children(node)? {
                self.parameters(child, flow, depth + 1)?;
            }
        }
        Ok(())
    }

    fn eval(
        &mut self,
        node: Node<'tree>,
        env: &Environment<'tree>,
        controls: &[FactId],
        effects: &[FactId],
        depth: usize,
    ) -> Result<Value<'tree>, SourceUnitError> {
        self.step(depth)?;
        if self.selected.is_some() {
            return Ok(Value::default());
        }
        self.work
            .charge(env.cost().saturating_add(node.child_count()))?;
        let mut value = match node.kind() {
            "identifier" => {
                self.work.charge(env.cost())?;
                if let Some(variable) = env.lookup(self.text(node)) {
                    let mut value = self.copy_value(&variable.value)?;
                    self.add_effects(
                        &mut value.roots,
                        effects,
                        Some((variable.binding, variable.written_at)),
                        false,
                    )?;
                    value
                } else if self.text(node) == "_" {
                    Value::default()
                } else {
                    Value {
                        roots: vec![self.fact(
                            node.byte_range(),
                            None,
                            Some(GoSliceBoundaryKind::PackageInput),
                            None,
                            Vec::new(),
                        )?],
                        ..Value::default()
                    }
                }
            }
            "int_literal"
            | "float_literal"
            | "imaginary_literal"
            | "rune_literal"
            | "interpreted_string_literal"
            | "raw_string_literal"
            | "true"
            | "false"
            | "nil" => Value::default(),
            "call_expression" => self.call(node, env, controls, effects, depth + 1)?,
            "binary_expression"
                if node
                    .child_by_field_name("operator")
                    .is_some_and(|op| matches!(self.text(op), "&&" | "||")) =>
            {
                self.short_circuit(node, env, controls, effects, depth + 1)?
            }
            "func_literal" => {
                let mut writes = Vec::new();
                let mut stack = vec![(node, depth + 1)];
                while let Some((part, depth)) = stack.pop() {
                    self.step(depth)?;
                    if matches!(
                        part.kind(),
                        "assignment_statement" | "inc_statement" | "dec_statement"
                    ) {
                        writes.push(part);
                    }
                    for child in self.children(part)? {
                        stack.push((child, depth + 1));
                    }
                }
                let targets = self.visible_bindings(env)?;
                let mut captured = Value {
                    closure_writes: writes,
                    ..Value::default()
                };
                if let Some(event) = self.expose(node, targets)? {
                    captured.effects.push(event);
                }
                captured
            }
            "selector_expression" => {
                if let Some(operand) = node.child_by_field_name("operand") {
                    if operand.kind() == "identifier" && env.lookup(self.text(operand)).is_none() {
                        Value {
                            roots: vec![self.fact(
                                node.byte_range(),
                                None,
                                Some(GoSliceBoundaryKind::PackageInput),
                                None,
                                Vec::new(),
                            )?],
                            ..Value::default()
                        }
                    } else {
                        let mut value = self.eval(operand, env, controls, effects, depth + 1)?;
                        // Without type/method proof this selector may be a saved
                        // method value retaining an implicit receiver address.
                        if let Some(binding) = self.local_base(operand, env, depth + 1)? {
                            if let Some(event) = self.expose(node, vec![binding])? {
                                value.effects.push(event);
                            }
                        }
                        value
                    }
                } else {
                    Value::default()
                }
            }
            "keyed_element" => self.eval_keyed_element(node, env, controls, effects, depth + 1)?,
            "composite_literal" => {
                let mut value = self.eval_children(node, env, controls, effects, depth + 1)?;
                let mut parents = self.copy_ids(&value.roots)?;
                self.work
                    .charge(parents.len().saturating_add(controls.len()))?;
                parents.extend_from_slice(controls);
                let gap = self.fact(
                    node.byte_range(),
                    None,
                    Some(GoSliceBoundaryKind::UnsupportedConstruct),
                    Some(GoSliceGap::UnsupportedConstruct),
                    parents,
                )?;
                value.roots.push(gap);
                value
            }
            "type_identifier"
            | "qualified_type"
            | "struct_type"
            | "field_declaration_list"
            | "field_declaration"
            | "pointer_type"
            | "slice_type"
            | "array_type"
            | "implicit_length_array_type"
            | "function_type"
            | "interface_type" => Value::default(),
            _ => {
                let supported = matches!(
                    node.kind(),
                    "binary_expression"
                        | "unary_expression"
                        | "parenthesized_expression"
                        | "index_expression"
                        | "slice_expression"
                        | "expression_list"
                        | "argument_list"
                        | "literal_value"
                        | "literal_element"
                );
                let mut combined = self.eval_children(node, env, controls, effects, depth + 1)?;
                let operator = node.child_by_field_name("operator").map(|op| self.text(op));
                if node.kind() == "unary_expression" && operator == Some("&") {
                    let children = self.children(node)?;
                    let target = if let Some(&operand) = children.first() {
                        self.local_base(operand, env, depth + 1)?
                    } else {
                        None
                    };
                    if let Some(target) = target {
                        if let Some(event) = self.expose(node, vec![target])? {
                            combined.effects.push(event);
                        }
                    } else if children
                        .first()
                        .is_some_and(|n| n.kind() == "unary_expression")
                    {
                        let targets = self.visible_bindings(env)?;
                        if let Some(event) = self.expose(node, targets)? {
                            combined.effects.push(event);
                        }
                    }
                }
                let receive = node.kind() == "unary_expression" && operator == Some("<-");
                let dereference = node.kind() == "unary_expression" && operator == Some("*");
                if !supported || receive || dereference {
                    let parents = self.copy_ids(&combined.roots)?;
                    let gap = self.fact(
                        node.byte_range(),
                        None,
                        Some(GoSliceBoundaryKind::UnsupportedConstruct),
                        Some(GoSliceGap::UnsupportedConstruct),
                        parents,
                    )?;
                    combined.roots.push(gap);
                    if !supported || receive {
                        self.facts[gap].scope = EffectScope::Global;
                        combined.effects.push(gap);
                    }
                }
                combined
            }
        };
        // Content uncertainty enters the dependency DAG at an actual memory
        // read. A bare reference still has its reaching local binding; this
        // does not certify its pointee contents or any opaque callee behavior.
        let reads_content = matches!(
            node.kind(),
            "index_expression" | "slice_expression" | "selector_expression"
        ) || (node.kind() == "unary_expression"
            && node
                .child_by_field_name("operator")
                .is_some_and(|op| self.text(op) == "*"));
        if reads_content {
            self.add_effects(&mut value.roots, effects, None, true)?;
            self.add_effects(&mut value.roots, &value.effects, None, true)?;
        }
        if self.selected.is_none() && node.id() == self.target.id() {
            // A function literal's body is not a local value/flow proof.
            if node.kind() == "func_literal" {
                let gap = self.fact(
                    node.byte_range(),
                    None,
                    Some(GoSliceBoundaryKind::UnsupportedConstruct),
                    Some(GoSliceGap::UnsupportedConstruct),
                    Vec::new(),
                )?;
                value.roots.push(gap);
            }
            let mut copied = self.copy_value(&value)?;
            if node.kind() == "call_expression" {
                // Argument evaluation is already in the call's roots. Effects
                // OF this selected opaque call are outgoing, not incoming.
                copied.effects.clear();
            }
            self.capture(node, copied, controls, effects)?;
        }
        Ok(value)
    }

    fn eval_keyed_element(
        &mut self,
        node: Node<'tree>,
        env: &Environment<'tree>,
        controls: &[FactId],
        effects: &[FactId],
        depth: usize,
    ) -> Result<Value<'tree>, SourceUnitError> {
        self.step(depth)?;
        self.work.charge(node.child_count())?;
        let key = node
            .child_by_field_name("key")
            .ok_or(SourceUnitError::UnsupportedSyntax)?;
        let value = node
            .child_by_field_name("value")
            .ok_or(SourceUnitError::UnsupportedSyntax)?;
        let mut simple_key = key;
        let mut key_depth = depth;
        while simple_key.kind() == "literal_element" {
            self.step(key_depth)?;
            key_depth += 1;
            let children = self.children(simple_key)?;
            if children.len() != 1 {
                break;
            }
            simple_key = children[0];
        }
        // Only a direct syntactic map_type proves a map key here. A nested
        // untyped literal must not inherit an enclosing map's key semantics.
        let mut explicit_map = false;
        // Parent lookup may traverse ancestry inside the parser; debit the
        // already-validated depth bound before each native lookup.
        self.work.charge(self.work.budget.max_depth)?;
        if let Some(body) = node.parent() {
            self.work.charge(body.child_count().saturating_add(1))?;
            if body.kind() == "literal_value" {
                self.work.charge(self.work.budget.max_depth)?;
                if let Some(literal) = body.parent() {
                    self.work.charge(literal.child_count().saturating_add(1))?;
                    explicit_map = literal.kind() == "composite_literal"
                        && literal
                            .child_by_field_name("body")
                            .is_some_and(|b| b.id() == body.id())
                        && literal
                            .child_by_field_name("type")
                            .is_some_and(|t| t.kind() == "map_type");
                }
            }
        }
        if simple_key.kind() != "identifier" || explicit_map {
            // Complex keys are real evaluated expressions: retain their calls,
            // nested addresses/captures and ordered value evaluation unchanged.
            return self.eval_children(node, env, controls, effects, depth + 1);
        }
        // An identifier in a named/otherwise unresolved composite might be a
        // field label or a value key. Do not look up a local/package binding as
        // though the supplied buffer established the named type's meaning.
        let parents = self.copy_ids(controls)?;
        let gap = self.fact(
            key.byte_range(),
            None,
            Some(GoSliceBoundaryKind::UnsupportedConstruct),
            Some(GoSliceGap::UnsupportedConstruct),
            parents,
        )?;
        let mut result = Value {
            roots: vec![gap],
            ..Value::default()
        };
        if contains(key, self.target) {
            let selected = self.copy_value(&result)?;
            self.capture(self.target, selected, controls, effects)?;
            return Ok(result);
        }
        let evaluated = self.eval(value, env, controls, effects, depth + 1)?;
        self.combine(&mut result, evaluated)?;
        Ok(result)
    }

    fn eval_children(
        &mut self,
        node: Node<'tree>,
        env: &Environment<'tree>,
        controls: &[FactId],
        effects: &[FactId],
        depth: usize,
    ) -> Result<Value<'tree>, SourceUnitError> {
        self.step(depth)?;
        let mut combined = Value::default();
        let mut preceding = self.copy_ids(effects)?;
        for child in self.children(node)? {
            if self.selected.is_some() {
                break;
            }
            let value = self.eval(child, env, controls, &preceding, depth + 1)?;
            self.work
                .charge(preceding.len().saturating_add(value.effects.len()))?;
            preceding.extend_from_slice(&value.effects);
            self.combine(&mut combined, value)?;
        }
        Ok(combined)
    }

    fn short_circuit(
        &mut self,
        node: Node<'tree>,
        env: &Environment<'tree>,
        controls: &[FactId],
        effects: &[FactId],
        depth: usize,
    ) -> Result<Value<'tree>, SourceUnitError> {
        self.step(depth)?;
        let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            let parents = self.copy_ids(controls)?;
            let gap = self.fact(
                node.byte_range(),
                None,
                Some(GoSliceBoundaryKind::UnsupportedConstruct),
                Some(GoSliceGap::UnsupportedConstruct),
                parents,
            )?;
            return Ok(Value {
                roots: vec![gap],
                ..Value::default()
            });
        };
        let mut value = self.eval(left, env, controls, effects, depth + 1)?;
        if self.selected.is_some() {
            return Ok(value);
        }
        let mut parents = self.copy_ids(&value.roots)?;
        self.work.charge(
            parents
                .len()
                .saturating_add(value.effects.len())
                .saturating_add(controls.len()),
        )?;
        parents.extend_from_slice(&value.effects);
        parents.extend_from_slice(controls);
        // The exact prefix includes && or ||, preserving the condition's sense.
        let guard = self.fact(
            left.start_byte()..right.start_byte(),
            Some(GoSliceRole::Control),
            None,
            None,
            parents,
        )?;
        let mut conditional = self.copy_ids(controls)?;
        conditional.push(guard);
        let mut preceding = self.copy_ids(effects)?;
        self.work
            .charge(preceding.len().saturating_add(value.effects.len()))?;
        preceding.extend_from_slice(&value.effects);
        let right_value = self.eval(right, env, &conditional, &preceding, depth + 1)?;
        self.combine(&mut value, right_value)?;
        value.roots.push(guard);
        Ok(value)
    }

    fn combine(
        &mut self,
        into: &mut Value<'tree>,
        other: Value<'tree>,
    ) -> Result<(), SourceUnitError> {
        self.work.charge(into.cost().saturating_add(other.cost()))?;
        into.roots.extend(other.roots);
        into.effects.extend(other.effects);
        into.closure_writes.extend(other.closure_writes);
        Ok(())
    }

    fn call(
        &mut self,
        node: Node<'tree>,
        env: &Environment<'tree>,
        controls: &[FactId],
        effects: &[FactId],
        depth: usize,
    ) -> Result<Value<'tree>, SourceUnitError> {
        self.step(depth)?;
        self.work
            .charge(env.cost().saturating_add(node.child_count()))?;
        let mut arguments = Value::default();
        if let Some(function) = node.child_by_field_name("function") {
            if function.kind() == "identifier" {
                if env.lookup(self.text(function)).is_some() {
                    self.combine_eval(&mut arguments, function, env, controls, effects, depth + 1)?;
                }
            } else if function.kind() == "selector_expression" {
                if let Some(receiver) = function.child_by_field_name("operand")
                    && !(receiver.kind() == "identifier"
                        && env.lookup(self.text(receiver)).is_none())
                {
                    self.combine_eval(&mut arguments, function, env, controls, effects, depth + 1)?;
                }
            } else {
                self.combine_eval(&mut arguments, function, env, controls, effects, depth + 1)?;
            }
        }
        let mut preceding = self.copy_ids(effects)?;
        self.work
            .charge(preceding.len().saturating_add(arguments.effects.len()))?;
        preceding.extend_from_slice(&arguments.effects);
        if let Some(list) = node.child_by_field_name("arguments") {
            for argument in self.children(list)? {
                if self.selected.is_some() {
                    return Ok(arguments);
                }
                let value = self.eval(argument, env, controls, &preceding, depth + 1)?;
                self.work
                    .charge(preceding.len().saturating_add(value.effects.len()))?;
                preceding.extend_from_slice(&value.effects);
                self.combine(&mut arguments, value)?;
            }
        }
        if self.selected.is_some() {
            return Ok(arguments);
        }
        let mut dependencies = self.ids(arguments.roots)?;
        self.work.charge(
            dependencies
                .len()
                .saturating_add(controls.len())
                .saturating_add(arguments.effects.len()),
        )?;
        dependencies.extend_from_slice(controls);
        dependencies.extend_from_slice(&arguments.effects);
        self.work.charge(dependencies.len())?;
        let call = self.fact(
            node.byte_range(),
            None,
            Some(GoSliceBoundaryKind::OpaqueCall),
            None,
            dependencies.clone(),
        )?;
        let mut mutations = arguments.effects;
        let mut exposure_input = self.copy_ids(effects)?;
        self.work
            .charge(exposure_input.len().saturating_add(mutations.len()))?;
        exposure_input.extend_from_slice(&mutations);
        let (targets, events) = self.exposures(&exposure_input)?;
        // Even a zero-argument opaque call may affect shared storage. Its
        // local binding targets still require independently retained exposure.
        {
            let mut parents = self.copy_ids(&dependencies)?;
            self.work
                .charge(parents.len().saturating_add(events.len()))?;
            parents.extend_from_slice(&events);
            let effect = self.fact(
                node.byte_range(),
                None,
                Some(GoSliceBoundaryKind::IndirectWrite),
                Some(GoSliceGap::UnknownMutation),
                parents,
            )?;
            let bindings = self.copy_ids(&targets)?;
            self.facts[effect].scope = EffectScope::Write {
                bindings,
                content: true,
                cycle: false,
            };
            mutations.push(effect);
        }
        for write in arguments.closure_writes {
            let parents = self.copy_ids(&dependencies)?;
            let effect = self.fact(
                write.byte_range(),
                None,
                Some(GoSliceBoundaryKind::ClosureWrite),
                Some(GoSliceGap::UnknownMutation),
                parents,
            )?;
            let bindings = self.copy_ids(&targets)?;
            self.facts[effect].scope = EffectScope::Write {
                bindings,
                content: true,
                cycle: false,
            };
            mutations.push(effect);
        }
        // The returned value remains an opaque callee boundary, including
        // unmodeled hidden inputs/reads. Do not infer external independence
        // from local completeness. A later syntactic content read joins the
        // retained ledger, including this call's outgoing content uncertainty.
        Ok(Value {
            roots: vec![call],
            effects: mutations,
            closure_writes: Vec::new(),
        })
    }

    fn combine_eval(
        &mut self,
        value: &mut Value<'tree>,
        node: Node<'tree>,
        env: &Environment<'tree>,
        controls: &[FactId],
        effects: &[FactId],
        depth: usize,
    ) -> Result<(), SourceUnitError> {
        let next = self.eval(node, env, controls, effects, depth)?;
        self.combine(value, next)
    }

    fn block(
        &mut self,
        node: Node<'tree>,
        flow: &mut Flow<'tree>,
        depth: usize,
    ) -> Result<(), SourceUnitError> {
        self.step(depth)?;
        flow.env.push();
        self.statements(node, flow, depth + 1)?;
        self.work.charge(flow.env.cost())?;
        flow.env.scopes.pop();
        Ok(())
    }

    fn statements(
        &mut self,
        node: Node<'tree>,
        flow: &mut Flow<'tree>,
        depth: usize,
    ) -> Result<(), SourceUnitError> {
        self.step(depth)?;
        for child in self.children(node)? {
            self.statement(child, flow, depth + 1)?;
            if self.selected.is_some() || flow.transfer.is_some() {
                break;
            }
        }
        Ok(())
    }

    fn statement(
        &mut self,
        node: Node<'tree>,
        flow: &mut Flow<'tree>,
        depth: usize,
    ) -> Result<(), SourceUnitError> {
        self.step(depth)?;
        if self.selected.is_some() || flow.transfer.is_some() {
            return Ok(());
        }
        self.work.charge(node.child_count())?;
        match node.kind() {
            "statement_list" => {
                for child in self.children(node)? {
                    self.statement(child, flow, depth + 1)?;
                    if self.selected.is_some() || flow.transfer.is_some() {
                        break;
                    }
                }
            }
            "block" => self.block(node, flow, depth + 1)?,
            "short_var_declaration" | "assignment_statement" => {
                self.assignment(node, flow, depth + 1)?
            }
            "var_declaration" => {
                for child in self.children(node)? {
                    if child.kind() == "var_spec" {
                        self.var_spec(node, child, flow, depth + 1)?;
                    } else if child.kind() == "var_spec_list" {
                        for spec in self.children(child)? {
                            self.var_spec(spec, spec, flow, depth + 1)?;
                        }
                    }
                }
            }
            "if_statement" => self.branch(node, flow, depth + 1)?,
            "for_statement" => self.loop_statement(node, flow, depth + 1)?,
            "return_statement" | "continue_statement" | "break_statement" => {
                if node.kind() == "return_statement" && contains(node, self.target) {
                    for child in self.children(node)? {
                        let _ = self.eval_in_flow(child, flow, depth + 1)?;
                    }
                }
                let transfer = match node.kind() {
                    "return_statement" => Transfer::Return,
                    "continue_statement" => Transfer::Continue,
                    _ => Transfer::Break,
                };
                let parents = self.copy_ids(&flow.controls)?;
                let id = self.fact(
                    node.byte_range(),
                    Some(GoSliceRole::Control),
                    None,
                    None,
                    parents,
                )?;
                flow.transfer = Some((transfer, id));
            }
            "expression_statement" => {
                for child in self.children(node)? {
                    let _ = self.eval_in_flow(child, flow, depth + 1)?;
                }
            }
            "inc_statement" | "dec_statement" => {
                let children = self.children(node)?;
                if let Some(name) = children
                    .first()
                    .copied()
                    .filter(|n| n.kind() == "identifier")
                {
                    let value = self.eval_in_flow(name, flow, depth + 1)?;
                    self.write(node, name, value, false, flow)?;
                } else {
                    self.unsupported(node, flow, GoSliceGap::UnknownMutation)?;
                }
            }
            "comment" | "empty_statement" => (),
            _ => self.unsupported(node, flow, GoSliceGap::UnsupportedConstruct)?,
        }
        Ok(())
    }

    fn unsupported(
        &mut self,
        node: Node<'tree>,
        flow: &mut Flow<'tree>,
        gap: GoSliceGap,
    ) -> Result<(), SourceUnitError> {
        let parents = self.copy_ids(&flow.controls)?;
        let id = self.fact(
            node.byte_range(),
            None,
            Some(GoSliceBoundaryKind::UnsupportedConstruct),
            Some(gap),
            parents,
        )?;
        self.facts[id].scope = EffectScope::Global;
        flow.effects.push(id);
        if contains(node, self.target) && self.selected.is_none() {
            self.capture(
                self.target,
                Value {
                    roots: vec![id],
                    ..Value::default()
                },
                &flow.controls,
                &flow.effects,
            )?;
        }
        Ok(())
    }

    fn var_spec(
        &mut self,
        declaration: Node<'tree>,
        spec: Node<'tree>,
        flow: &mut Flow<'tree>,
        depth: usize,
    ) -> Result<(), SourceUnitError> {
        self.step(depth)?;
        self.work.charge(spec.child_count())?;
        let mut cursor = spec.walk();
        let mut names = Vec::new();
        for name in spec.children_by_field_name("name", &mut cursor) {
            self.work.charge(1)?;
            names.push(name);
        }
        let values = if let Some(list) = spec.child_by_field_name("value") {
            self.children(list)?
        } else {
            Vec::new()
        };
        let mut evaluated = Vec::new();
        for value in values {
            evaluated.push(self.eval_in_flow(value, flow, depth + 1)?);
        }
        let unsupported = !evaluated.is_empty() && names.len() != evaluated.len();
        let arity_gap = if unsupported {
            Some(self.fact(
                declaration.byte_range(),
                None,
                Some(GoSliceBoundaryKind::UnsupportedConstruct),
                Some(GoSliceGap::UnsupportedConstruct),
                Vec::new(),
            )?)
        } else {
            None
        };
        for (index, name) in names.into_iter().enumerate() {
            let mut value = if evaluated.is_empty() {
                Value::default()
            } else if let Some(value) = evaluated.get(index) {
                self.copy_value(value)?
            } else {
                Value::default()
            };
            if let Some(gap) = arity_gap {
                value.roots.push(gap);
            }
            self.write(declaration, name, value, true, flow)?;
        }
        Ok(())
    }

    fn assignment(
        &mut self,
        node: Node<'tree>,
        flow: &mut Flow<'tree>,
        depth: usize,
    ) -> Result<(), SourceUnitError> {
        let Some(left) = node.child_by_field_name("left") else {
            return self.unsupported(node, flow, GoSliceGap::UnsupportedConstruct);
        };
        let Some(right) = node.child_by_field_name("right") else {
            return self.unsupported(node, flow, GoSliceGap::UnsupportedConstruct);
        };
        let names = self.children(left)?;
        let expressions = self.children(right)?;
        let mut values = Vec::new();
        for expression in expressions {
            values.push(self.eval_in_flow(expression, flow, depth + 1)?);
        }
        if self.selected.is_some() {
            return Ok(());
        }
        self.work.charge(names.len())?;
        if names.iter().any(|name| name.kind() != "identifier") {
            let mut deps = self.copy_ids(&flow.controls)?;
            for value in values {
                self.work
                    .charge(deps.len().saturating_add(value.roots.len()))?;
                deps.extend(value.roots);
            }
            let id = self.fact(
                node.byte_range(),
                None,
                Some(GoSliceBoundaryKind::IndirectWrite),
                Some(GoSliceGap::UnknownMutation),
                deps,
            )?;
            let (mut targets, _) = self.exposures(&flow.effects)?;
            for name in names {
                if let Some(base) = self.local_base(name, &flow.env, depth + 1)? {
                    targets.push(base);
                } else if name.kind() == "unary_expression" {
                    let unknown = self.visible_bindings(&flow.env)?;
                    self.work
                        .charge(targets.len().saturating_add(unknown.len()))?;
                    targets.extend(unknown);
                }
            }
            let bindings = self.ids(targets)?;
            self.facts[id].scope = EffectScope::Write {
                bindings,
                content: true,
                cycle: false,
            };
            flow.effects.push(id);
            return Ok(());
        }
        let tuple_unknown = names.len() != values.len();
        let mut all = Value::default();
        if tuple_unknown {
            for value in &values {
                let copied = self.copy_value(value)?;
                self.combine(&mut all, copied)?;
            }
            let parents = self.copy_ids(&all.roots)?;
            let id = self.fact(
                node.byte_range(),
                None,
                Some(GoSliceBoundaryKind::UnsupportedConstruct),
                Some(GoSliceGap::UnsupportedConstruct),
                parents,
            )?;
            all.roots.push(id);
        }
        let short = node.kind() == "short_var_declaration";
        let compound = !short
            && node
                .child_by_field_name("operator")
                .is_some_and(|op| self.text(op) != "=");
        for (index, name) in names.into_iter().enumerate() {
            let mut value = if tuple_unknown {
                self.copy_value(&all)?
            } else {
                self.copy_value(&values[index])?
            };
            if compound {
                let previous = self.eval_in_flow(name, flow, depth + 1)?;
                self.combine(&mut value, previous)?;
            }
            // RHS snapshots are already evaluated before any simultaneous write.
            self.write(node, name, value, short, flow)?;
        }
        Ok(())
    }

    fn write(
        &mut self,
        node: Node<'tree>,
        name: Node<'tree>,
        mut value: Value<'tree>,
        short: bool,
        flow: &mut Flow<'tree>,
    ) -> Result<(), SourceUnitError> {
        let spelling = self.text(name);
        if spelling == "_" {
            return Ok(());
        }
        self.work.charge(
            flow.env
                .cost()
                .saturating_mul(2)
                .saturating_add(spelling.len()),
        )?;
        let current = flow.env.scopes.len() - 1;
        let prior_scope = flow.env.scope_of(spelling);
        if !short && prior_scope.is_none() {
            let id = self.fact(
                node.byte_range(),
                None,
                Some(GoSliceBoundaryKind::IndirectWrite),
                Some(GoSliceGap::UnknownMutation),
                value.roots,
            )?;
            let (bindings, _) = self.exposures(&flow.effects)?;
            self.facts[id].scope = EffectScope::Write {
                bindings,
                content: true,
                cycle: false,
            };
            flow.effects.push(id);
            return Ok(());
        }
        let scope = if short {
            current
        } else {
            prior_scope.unwrap_or(current)
        };
        let existing = flow.env.scopes[scope].get(spelling).map(|var| var.binding);
        let binding = if let Some(binding) = existing {
            binding
        } else {
            self.fact(
                node.byte_range(),
                Some(GoSliceRole::Binding),
                None,
                None,
                Vec::new(),
            )?
        };
        self.work.charge(
            value
                .roots
                .len()
                .saturating_add(flow.controls.len())
                .saturating_add(value.effects.len()),
        )?;
        value.roots.extend_from_slice(&flow.controls);
        value.roots.extend_from_slice(&value.effects);
        value.roots.push(binding);
        let definition = self.fact(
            node.byte_range(),
            Some(GoSliceRole::Definition),
            None,
            None,
            value.roots,
        )?;
        self.work
            .charge(flow.effects.len().saturating_add(value.effects.len()))?;
        flow.effects.extend_from_slice(&value.effects);
        value.roots = vec![definition];
        self.work.charge(
            flow.env
                .cost()
                .saturating_add(spelling.len())
                .saturating_add(value.cost()),
        )?;
        flow.env.insert(
            scope,
            spelling.to_owned(),
            Variable {
                binding,
                written_at: definition,
                value,
            },
        );
        Ok(())
    }

    fn guard(
        &mut self,
        node: Node<'tree>,
        body: Node<'tree>,
        value: Value<'tree>,
        flow: &Flow<'tree>,
    ) -> Result<FactId, SourceUnitError> {
        let mut deps = value.roots;
        self.work.charge(
            deps.len()
                .saturating_add(flow.controls.len())
                .saturating_add(value.effects.len()),
        )?;
        deps.extend_from_slice(&flow.controls);
        deps.extend(value.effects);
        self.fact(
            node.start_byte()..body.start_byte() + 1,
            Some(GoSliceRole::Control),
            None,
            None,
            deps,
        )
    }

    fn branch(
        &mut self,
        node: Node<'tree>,
        flow: &mut Flow<'tree>,
        depth: usize,
    ) -> Result<(), SourceUnitError> {
        self.step(depth)?;
        flow.env.push();
        if let Some(initializer) = node.child_by_field_name("initializer") {
            self.statement(initializer, flow, depth + 1)?;
        }
        let Some(condition) = node.child_by_field_name("condition") else {
            return self.unsupported(node, flow, GoSliceGap::UnsupportedConstruct);
        };
        let Some(body) = node.child_by_field_name("consequence") else {
            return self.unsupported(node, flow, GoSliceGap::UnsupportedConstruct);
        };
        let value = self.eval_in_flow(condition, flow, depth + 1)?;
        if self.selected.is_some() {
            return Ok(());
        }
        let guard = self.guard(node, body, value, flow)?;
        let mut yes = self.copy_flow(flow)?;
        yes.controls.push(guard);
        self.block(body, &mut yes, depth + 1)?;
        if self.selected.is_some() {
            return Ok(());
        }
        let mut no = self.copy_flow(flow)?;
        no.controls.push(guard);
        if let Some(alternative) = node.child_by_field_name("alternative") {
            if alternative.kind() == "block" {
                self.block(alternative, &mut no, depth + 1)?;
            } else {
                self.statement(alternative, &mut no, depth + 1)?;
            }
        }
        if self.selected.is_some() {
            return Ok(());
        }
        self.join(flow, yes, no, Some(guard))?;
        self.work.charge(flow.env.cost())?;
        flow.env.scopes.pop();
        Ok(())
    }

    fn join(
        &mut self,
        flow: &mut Flow<'tree>,
        yes: Flow<'tree>,
        no: Flow<'tree>,
        guard: Option<FactId>,
    ) -> Result<(), SourceUnitError> {
        self.work.charge(
            flow.env
                .cost()
                .saturating_add(yes.env.cost())
                .saturating_add(no.env.cost())
                .saturating_add(flow.effects.len())
                .saturating_add(yes.effects.len())
                .saturating_add(no.effects.len())
                .saturating_add(flow.controls.len())
                .saturating_add(yes.controls.len())
                .saturating_add(no.controls.len()),
        )?;
        if let Some((_, transfer)) = yes.transfer {
            if no.transfer.is_none() {
                self.work.charge(no.env.cost())?;
                flow.env = no.env;
                flow.effects.extend(yes.effects);
                flow.effects.extend(no.effects);
                if let Some(guard) = guard {
                    flow.controls.push(guard);
                }
                flow.controls.push(transfer);
                return Ok(());
            }
        }
        if let Some((_, transfer)) = no.transfer {
            if yes.transfer.is_none() {
                self.work.charge(yes.env.cost())?;
                flow.env = yes.env;
                flow.effects.extend(yes.effects);
                flow.effects.extend(no.effects);
                if let Some(guard) = guard {
                    flow.controls.push(guard);
                }
                flow.controls.push(transfer);
                return Ok(());
            }
        }
        let mut added_weight = 0usize;
        for (scope_index, scope) in flow.env.scopes.iter_mut().enumerate() {
            let lookup_work = heap_work(scope.len()).saturating_mul(2);
            for (name, variable) in scope {
                self.work
                    .charge(name.len().saturating_add(1).saturating_mul(lookup_work))?;
                if let (Some(a), Some(b)) = (
                    yes.env.scopes[scope_index].get(name),
                    no.env.scopes[scope_index].get(name),
                ) {
                    let mut value = self.copy_value(&a.value)?;
                    let other = self.copy_value(&b.value)?;
                    self.combine(&mut value, other)?;
                    value.roots = self.ids(value.roots)?;
                    value.effects = self.ids(value.effects)?;
                    added_weight = added_weight.saturating_add(value.cost());
                    variable.written_at = a.written_at.min(b.written_at);
                    variable.value = value;
                }
            }
        }
        flow.env.weight = flow.env.weight.saturating_add(added_weight);
        self.work
            .charge(yes.effects.len().saturating_add(no.effects.len()))?;
        flow.effects.extend(yes.effects);
        flow.effects.extend(no.effects);
        flow.effects = self.ids(std::mem::take(&mut flow.effects))?;
        if yes.transfer.is_some() && no.transfer.is_some() {
            flow.transfer = yes.transfer;
        }
        Ok(())
    }

    fn summarize_loop(
        &mut self,
        node: Node<'tree>,
        initializer: Option<Node<'tree>>,
        env: &Environment<'tree>,
        effects: &[FactId],
        depth: usize,
    ) -> Result<Vec<FactId>, SourceUnitError> {
        let (mut exposed, mut exposure_events) = self.exposures(effects)?;
        let visible = self.visible_bindings(env)?;
        let mut pending = Vec::new();
        let mut calls = false;
        let mut unknown_control = false;
        let mut stack = vec![(node, depth)];
        while let Some((part, at_depth)) = stack.pop() {
            self.step(at_depth)?;
            if initializer.is_some_and(|initial| part.id() == initial.id()) {
                continue;
            }
            self.work
                .charge(part.child_count().saturating_add(env.cost()))?;
            let mut writes = Vec::new();
            let mut captures = Vec::new();
            match part.kind() {
                "call_expression" => calls = true,
                "assignment_statement" | "short_var_declaration" => {
                    if let Some(left) = part.child_by_field_name("left") {
                        for name in self.children(left)? {
                            if let Some(base) = self.local_base(name, env, at_depth + 1)? {
                                writes.push(base);
                            } else if name.kind() != "identifier" {
                                self.work
                                    .charge(writes.len().saturating_add(visible.len()))?;
                                writes.extend_from_slice(&visible);
                            }
                        }
                    } else {
                        unknown_control = true;
                    }
                }
                "inc_statement" | "dec_statement" => {
                    if let Some(&name) = self.children(part)?.first() {
                        if let Some(base) = self.local_base(name, env, at_depth + 1)? {
                            writes.push(base);
                        } else {
                            self.work
                                .charge(writes.len().saturating_add(visible.len()))?;
                            writes.extend_from_slice(&visible);
                        }
                    }
                }
                "unary_expression" => {
                    let operator = part.child_by_field_name("operator").map(|op| self.text(op));
                    if operator == Some("&") {
                        if let Some(&operand) = self.children(part)?.first() {
                            if let Some(base) = self.local_base(operand, env, at_depth + 1)? {
                                captures.push(base);
                            } else if operand.kind() == "unary_expression" {
                                self.work
                                    .charge(captures.len().saturating_add(visible.len()))?;
                                captures.extend_from_slice(&visible);
                            }
                        }
                    } else if operator == Some("<-") {
                        unknown_control = true;
                    }
                }
                "selector_expression" => {
                    if let Some(operand) = part.child_by_field_name("operand") {
                        if let Some(base) = self.local_base(operand, env, at_depth + 1)? {
                            captures.push(base);
                        }
                    }
                }
                "func_literal" => {
                    self.work.charge(visible.len())?;
                    captures.extend_from_slice(&visible);
                }
                "for_statement" if part.id() != node.id() => unknown_control = true,
                "range_clause"
                | "select_statement"
                | "expression_switch_statement"
                | "type_switch_statement"
                | "goto_statement"
                | "labeled_statement"
                | "go_statement"
                | "defer_statement"
                | "send_statement" => unknown_control = true,
                other => {
                    let supported_expression = matches!(
                        other,
                        "binary_expression"
                            | "parenthesized_expression"
                            | "index_expression"
                            | "slice_expression"
                            | "selector_expression"
                            | "call_expression"
                    );
                    if (other.ends_with("_expression") && !supported_expression)
                        || (other.ends_with("_statement")
                            && !matches!(
                                other,
                                "if_statement"
                                    | "return_statement"
                                    | "continue_statement"
                                    | "break_statement"
                                    | "empty_statement"
                                    | "expression_statement"
                                    | "for_statement"
                            ))
                    {
                        unknown_control = true;
                    }
                }
            }
            if !captures.is_empty() {
                let targets = self.ids(captures)?;
                self.work
                    .charge(exposed.len().saturating_add(targets.len()))?;
                exposed.extend_from_slice(&targets);
                if let Some(event) = self.expose(part, targets)? {
                    exposure_events.push(event);
                    pending.push(event);
                }
            }
            if !writes.is_empty() {
                let bindings = self.ids(writes)?;
                // Header/post mutations explain their carried control input
                // locally. Body-written bindings deliberately retain the full
                // cycle fallback: this experiment does not solve backedges or
                // prove a dominating reset for every reaching iteration.
                let header = node
                    .child_by_field_name("body")
                    .is_some_and(|body| part.end_byte() <= body.start_byte());
                let range = if header {
                    part.byte_range()
                } else {
                    node.byte_range()
                };
                let gap = self.fact(
                    range,
                    None,
                    Some(GoSliceBoundaryKind::UnsupportedConstruct),
                    Some(GoSliceGap::LoopCarriedState),
                    Vec::new(),
                )?;
                self.facts[gap].scope = EffectScope::Write {
                    bindings,
                    content: false,
                    cycle: true,
                };
                pending.push(gap);
            }
            for child in self.children(part)? {
                stack.push((child, at_depth + 1));
            }
        }
        // Aggregate, not a source-order simulation: a late exposure can enable
        // an earlier zero-argument operation on a subsequent cycle.
        let exposed = self.ids(exposed)?;
        if calls {
            let parents = self.ids(exposure_events)?;
            let gap = self.fact(
                node.byte_range(),
                None,
                Some(GoSliceBoundaryKind::UnsupportedConstruct),
                Some(GoSliceGap::LoopCarriedState),
                parents,
            )?;
            self.facts[gap].scope = EffectScope::Write {
                bindings: exposed,
                content: true,
                cycle: true,
            };
            pending.push(gap);
        }
        if unknown_control {
            let gap = self.fact(
                node.byte_range(),
                None,
                Some(GoSliceBoundaryKind::UnsupportedConstruct),
                Some(GoSliceGap::LoopCarriedState),
                Vec::new(),
            )?;
            self.facts[gap].scope = EffectScope::Global;
            pending.push(gap);
        }
        self.ids(pending)
    }

    fn loop_statement(
        &mut self,
        node: Node<'tree>,
        flow: &mut Flow<'tree>,
        depth: usize,
    ) -> Result<(), SourceUnitError> {
        self.step(depth)?;
        flow.env.push();
        let Some(body) = node.child_by_field_name("body") else {
            return self.unsupported(node, flow, GoSliceGap::UnsupportedConstruct);
        };
        let mut initializer = None;
        let mut test = None;
        for child in self.children(node)? {
            if child.id() == body.id() {
                continue;
            }
            if child.kind() == "for_clause" {
                initializer = child.child_by_field_name("initializer");
                if let Some(initial) = initializer {
                    self.statement(initial, flow, depth + 1)?;
                }
                test = child.child_by_field_name("condition");
            } else if child.kind() == "range_clause" {
                test = child.child_by_field_name("right");
                self.unsupported(child, flow, GoSliceGap::UnsupportedConstruct)?;
            } else {
                test = Some(child);
            }
        }
        let pending =
            self.summarize_loop(node, initializer, &flow.env, &flow.effects, depth + 1)?;
        self.work
            .charge(flow.effects.len().saturating_add(pending.len()))?;
        flow.effects.extend(pending);
        flow.effects = self.ids(std::mem::take(&mut flow.effects))?;
        let condition = if let Some(test) = test {
            self.eval_in_flow(test, flow, depth + 1)?
        } else {
            Value::default()
        };
        let guard = self.guard(node, body, condition, flow)?;
        let mut iteration = self.copy_flow(flow)?;
        iteration.controls.push(guard);
        self.block(body, &mut iteration, depth + 1)?;
        if self.selected.is_some() {
            return Ok(());
        }
        // Keep zero-iteration and entered paths. The may-summary, not a fixed
        // point or an overwritten global gap, accounts for repeated-state risk.
        iteration.transfer = None;
        let untouched = self.copy_flow(flow)?;
        self.join(flow, iteration, untouched, None)?;
        self.work.charge(flow.env.cost())?;
        flow.env.scopes.pop();
        Ok(())
    }

    fn finish(self, criterion: Range<usize>) -> Result<GoExpressionSlice, SourceUnitError> {
        self.work.charge(self.facts.len())?;
        let mut seen = vec![false; self.facts.len()];
        let mut stack = self.selected.into_iter().collect::<Vec<_>>();
        let mut result = GoExpressionSlice {
            criterion,
            witnesses: Vec::new(),
            boundaries: Vec::new(),
            gaps: Vec::new(),
            complete_local: true,
        };
        let mut witness_keys = BTreeSet::new();
        let mut boundary_keys = BTreeSet::new();
        while let Some(id) = stack.pop() {
            self.work.charge(1)?;
            if seen[id] {
                continue;
            }
            seen[id] = true;
            let fact = &self.facts[id];
            self.work
                .charge(stack.len().saturating_add(fact.parents.len()))?;
            stack.extend_from_slice(&fact.parents);
            if let Some(role) = fact.role {
                self.work.charge(heap_work(witness_keys.len()))?;
                if witness_keys.insert((fact.range.start, fact.range.end, role as u8)) {
                    self.work.charge(
                        heap_work(self.starts.len())
                            .saturating_mul(2)
                            .saturating_add("go_slice_physical_context".len()),
                    )?;
                    let first = self
                        .starts
                        .partition_point(|&start| start <= fact.range.start)
                        - 1;
                    let last = self.starts.partition_point(|&start| start < fact.range.end) - 1;
                    result.witnesses.push(GoSliceWitness {
                        role,
                        range: fact.range.clone(),
                        unit: SourceUnit {
                            start_byte: self.starts[first],
                            end_byte: self.ends[last],
                            start_line: first + 1,
                            end_line: last + 1,
                            kind: "go_slice_physical_context".to_owned(),
                        },
                    });
                }
            }
            if let Some(kind) = fact.boundary {
                self.work.charge(heap_work(boundary_keys.len()))?;
                if boundary_keys.insert((fact.range.start, fact.range.end, kind as u8)) {
                    result.boundaries.push(GoSliceBoundary {
                        range: fact.range.clone(),
                        kind,
                    });
                }
            }
            if let Some(gap) = fact.gap {
                self.work.charge(result.gaps.len().saturating_add(1))?;
                if !result.gaps.contains(&gap) {
                    result.gaps.push(gap);
                }
                result.complete_local = false;
            }
        }
        self.work.charge(
            result
                .witnesses
                .len()
                .saturating_mul(heap_work(result.witnesses.len()))
                .saturating_add(
                    result
                        .boundaries
                        .len()
                        .saturating_mul(heap_work(result.boundaries.len())),
                ),
        )?;
        result
            .witnesses
            .sort_by_key(|w| (w.range.start, w.range.end, w.role as u8));
        result
            .boundaries
            .sort_by_key(|b| (b.range.start, b.range.end, b.kind as u8));
        self.work.charge(0)?;
        Ok(result)
    }
}
