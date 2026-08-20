//! Typed proof atoms and the pure bounded role-unification matcher (stage 1).
//!
//! This module is the leaf model for typed-obligation proofs: it defines the
//! const proof formulas for the three flow groups that get real atoms (M for
//! `LOG_HANDLER_FLOW`, A for `MAPPER_PLAN_FLOW`, C for `CSS_ANIMATION_FLOW`),
//! the runtime receipt types those formulas are checked against, and the
//! deterministic matcher that discharges atoms. It imports contracts types
//! only — never a planning sibling — and stages 2-4 wire it to requirements,
//! retrieval retention, and verification.
//!
//! The binding rules come from the stage-0 formulas contract (revision 5.1):
//! receipts are the only discharge inputs; all cross-receipt joins are
//! node-identity joins; containment arithmetic uses same-receipt numbers or an
//! atom-anchored window; provenance never decides discharge (the conversion
//! from [`SupportUnitDto`] drops `query`); callsite identity is read only
//! after shape validation; certainty gates are attributed per edge kind;
//! absence facts require an untruncated covering scan whose traversal kinds
//! are known — unknown coverage fails closed; one receipt may discharge
//! several atoms; and every unmatched required atom fails closed.

use codestory_contracts::api::{
    AgentCitationDto, EdgeId, EdgeKind, GraphEdgeDto, NodeId, NodeKind, SupportUnitDto,
    SupportUnitKindDto,
};
use std::cmp::Reverse;
use std::collections::BTreeMap;

/// The certainty value the rule-6 gate requires on CALL and TYPE_USAGE
/// receipts.
const CERTAINTY_CERTAIN: &str = "certain";

/// Total candidate-receipt considerations the matcher may spend on one match
/// before it aborts. The search is depth-first over a statically sized fact
/// list, so receipt iteration is the only unbounded dimension.
///
/// Rationale for 4096: a real packet's evidence set is tens of receipts and
/// the largest formula carries 13 facts, so honest matches finish in well
/// under a thousand steps; only adversarial or runaway backtracking reaches
/// the cap, and that is reported as [`FlowProofOutcome::Aborted`] —
/// fail-closed for discharge, observable for telemetry.
const MATCH_STEP_LIMIT: usize = 4096;

/// Strict prefix of the combined loop marker
/// `receiver-binding:loop-element@{start}-{end}`.
const LOOP_ELEMENT_MARKER_PREFIX: &str = "receiver-binding:loop-element@";

/// Prefix of the receiver-owner classification marker.
const RECEIVER_OWNER_MARKER_PREFIX: &str = "receiver-owner:";

/// Prefix shared by the `syntax:{lang}-call` and `syntax:{lang}-new` markers.
const SYNTAX_MARKER_PREFIX: &str = "syntax:";

/// Stable identity of one proof atom inside the three formula groups.
///
/// R4-anchored source receipts record the discharging atom by this id — that
/// mark is what the rule-3(b) containment arm reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProofAtomId {
    /// M1a — carrier range for the flow owner (`logger_event`).
    M1a,
    /// M1b — object-construction CALL from the flow owner (`logger_event`).
    M1b,
    /// M2 — loop-bound dispatch containment (`handler_processing`).
    M2,
    /// M3 — receiver-annotated dispatch CALL (`handler_processing`).
    M3,
    /// A1 — configuration TYPE_USAGE (`mapper_config`).
    A1,
    /// A2 — carrier range for the configuration type (`mapper_config`).
    A2,
    /// A3 — plan-owner CALL onto a builder member (`mapper_execution`).
    A3,
    /// A4 — carrier range for the builder (`mapper_execution`).
    A4,
    /// A5 — plan-builder admissibility of the configuration source
    /// (`mapper_config`): the Builder role must bind a type that OWNS a
    /// method.
    A5,
    /// C1 — verified import-bearing entrypoint (`css_animation_entrypoint`).
    C1,
    /// C2 — imported variable declaration (`css_animation_structure`).
    C2,
    /// C3 — base selector var usage plus keyframe absence
    /// (`css_animation_structure`).
    C3,
    /// C4 — imported keyframe declaration and its selector usage
    /// (`css_animation_structure`).
    C4,
}

/// A role a formula binds to a node id (rule 2: roles bind node identities,
/// never names, paths, labels, or callsite segment values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProofRole {
    /// M — the owner both logging atoms and the carrier range join on.
    FlowOwner,
    /// A — the plan owner whose dispatch CALL A3 binds.
    PlanOwner,
    /// A — the builder joined across A1, A5's membership constraint, A3's
    /// MEMBER edge, and A4.
    Builder,
    /// A — the builder method (`Mb` in the formulas contract): A3's CALL
    /// target and MEMBER target.
    BuilderMethod,
    /// A — the configuration type joined across A1 and A2.
    ConfigType,
    /// C — the entrypoint stylesheet's canonical file node.
    Entrypoint,
    /// C — the imported variables source file.
    VarsSource,
    /// C — the imported base source file.
    BaseSource,
    /// C — the imported animation source file.
    AnimSource,
    /// C — the MODULE-kind import-statement structural node that is MEMBER of
    /// the entrypoint (C1's anchored line carrier, contract rev 5.1).
    ImportStatement,
    /// C — the custom-property VARIABLE node joined across C2 and C3.
    VarNode,
    /// C — the base selector CONSTANT node (`Sb` in the formulas contract).
    BaseSelector,
    /// C — the animation selector CONSTANT node (`Sa` in the formulas
    /// contract).
    AnimationSelector,
    /// C — the keyframe FUNCTION node declared in the animation source.
    KeyframeNode,
}

/// How one endpoint of a fact pattern constrains (or binds) a node id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofEndpointPattern {
    /// Unify the endpoint's node id with a role: bind it when the role is
    /// free, require equality when it is already bound.
    Role(ProofRole),
    /// The endpoint must equal a node id one of the listed roles is ALREADY
    /// bound to. Never binds; with none of the listed roles bound it fails
    /// closed — which makes it a guard under per-requirement matching
    /// (contract rev 5.2): a subset evaluated without its siblings' bindings
    /// cannot satisfy this endpoint.
    AnyOfRoles(&'static [ProofRole]),
    /// The endpoint is unconstrained.
    Any,
}

/// A classification marker requirement on a CALL receipt's callsite identity.
///
/// Markers are the order-agnostic `|`-segments after the canonical first
/// segment (rule 5). They restrict a single receipt's admissibility and are
/// never joined across receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallsiteMarkerPattern {
    /// A `syntax:{lang}-call` segment with a non-empty language part.
    SyntaxCall,
    /// A `syntax:{lang}-new` segment with a non-empty language part (the P1b
    /// construction receipt).
    SyntaxNew,
    /// A `receiver-owner:{owner}` segment with a non-empty value. The value
    /// is classification only; it never binds or joins.
    ReceiverOwner,
    /// A strictly parsed `receiver-binding:loop-element@{start}-{end}`
    /// segment whose range contains the shape-validated canonical callsite
    /// line (same-receipt arithmetic, rule 3(a)). Any malformed loop-element
    /// segment fails the receipt closed.
    LoopElementContainsCallsiteLine,
}

/// Pattern over one [`VerifiedTypedRelationReceipt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypedRelationPattern {
    /// Required edge kind. The rule-6 certainty gate is attributed from this
    /// kind by the matcher, never spelled per spec.
    pub kind: EdgeKind,
    /// Constraint on the effective source node id.
    pub source: ProofEndpointPattern,
    /// Constraint on the effective target node id.
    pub target: ProofEndpointPattern,
    /// Required effective-target node kind. For structural edges this is the
    /// admissibility proxy rule 6 names.
    pub target_kind: Option<NodeKind>,
    /// Callsite marker requirements (CALL receipts only in the shipped
    /// formulas).
    pub markers: &'static [CallsiteMarkerPattern],
    /// Require the effective target to differ from the effective source
    /// (M3's no-self-call clause).
    pub target_distinct_from_source: bool,
}

/// Pattern over one [`VerifiedSourceAspectReceipt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceAspectPattern {
    /// Required aspect kind.
    pub kind: SourceAspectKind,
    /// Constraint on the reread range's symbol id. A role constraint fails
    /// closed when the receipt carries no symbol id.
    pub symbol: ProofEndpointPattern,
    /// Require the receipt to carry this atom's R4 anchor mark (rule 3(b)).
    pub require_atom_anchor: bool,
}

/// An absence fact: no edge of `kind` from the node bound to `source` reaches
/// a node of `forbidden_target_kind`, proven over an untruncated covering
/// scan with known traversal kinds (rule 7). An edge of that kind whose
/// target kind is unknown also fails the absence closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbsentTypedRelationPattern {
    /// Role bound to the node whose outgoing edges the fact is about. It must
    /// already be bound by an earlier fact; an unbound role fails closed.
    pub source: ProofRole,
    /// The outgoing edge kind the scan must cover.
    pub kind: EdgeKind,
    /// The target node kind whose presence refutes the absence.
    pub forbidden_target_kind: NodeKind,
}

/// A rule-3(b) containment fact: a receipt-carried line (the `start_line` of
/// a source-aspect receipt of the named kind whose symbol is bound to
/// `line_symbol`) lies inside a window owned by `window_owner` that carries
/// this atom's anchor mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchoredContainmentPattern {
    /// Aspect kind the line-carrying receipt must have — the kind the atom
    /// names for its verified range.
    pub kind: SourceAspectKind,
    /// Role bound to the node whose declaration line is being covered. Must
    /// already be bound by an earlier fact of the same atom.
    pub line_symbol: ProofRole,
    /// Role bound to the citation that owns the anchored window. Must already
    /// be bound.
    pub window_owner: ProofRole,
}

/// One fact a proof atom requires. An atom discharges only when every one of
/// its facts matches (rule 8: sharing receipts across atoms is allowed, an
/// atom never discharges with a fact unmatched).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofFactPattern {
    /// A typed-relation receipt must match the pattern.
    TypedRelation(TypedRelationPattern),
    /// A source-aspect receipt must match the pattern.
    SourceAspect(SourceAspectPattern),
    /// An absence fact over a covering scan.
    AbsentTypedRelation(AbsentTypedRelationPattern),
    /// An anchored cross-receipt line containment fact.
    AnchoredLineContainment(AnchoredContainmentPattern),
}

/// One proof atom: its id, the requirement it is materially required by, and
/// the facts that discharge it.
#[derive(Debug, Clone, Copy)]
pub struct ProofAtomSpec {
    /// Stable atom id.
    pub id: ProofAtomId,
    /// The flow requirement id this atom is materially required by. Stage 2
    /// wires requirements to atoms; this module never imports them.
    pub requirement: &'static str,
    /// The facts that must all match, in evaluation order. Facts that read
    /// bound roles (absence, containment, `AnyOfRoles` endpoints) come after
    /// the facts — or, across atoms, the atoms — that bind them.
    pub facts: &'static [ProofFactPattern],
}

/// One formula group: the atoms and the role-distinctness constraints they
/// share. Role unification spans the whole group.
#[derive(Debug, Clone, Copy)]
pub struct FlowProofFormula {
    /// Atoms in evaluation order. Atoms whose facts read roles bound by other
    /// atoms come after them.
    pub atoms: &'static [ProofAtomSpec],
    /// Sets of roles whose bound node ids must be pairwise distinct. Roles a
    /// partial match left unbound are not compared.
    pub distinct_roles: &'static [&'static [ProofRole]],
}

impl FlowProofFormula {
    /// The formula's requirement ids, in declaration order (first appearance
    /// among [`FlowProofFormula::atoms`]).
    pub fn requirements(&self) -> Vec<&'static str> {
        let mut requirements = Vec::new();
        for atom in self.atoms {
            if !requirements.contains(&atom.requirement) {
                requirements.push(atom.requirement);
            }
        }
        requirements
    }

    /// The ids of the atoms materially required by `requirement`, in formula
    /// order. Empty when the formula names no such requirement — and an empty
    /// atom list never proves anything.
    pub fn atoms_for(&self, requirement: &str) -> Vec<ProofAtomId> {
        self.atoms
            .iter()
            .filter(|atom| atom.requirement == requirement)
            .map(|atom| atom.id)
            .collect()
    }

    /// The [`RequirementConstraintStrength`] of `requirement`.
    ///
    /// A pure function of the FORMULA: it reads atom specs only and never
    /// touches an evidence set, so the fallback order it induces is
    /// derivation-time knowledge and is identical on every packet.
    fn constraint_strength(&self, requirement: &str) -> RequirementConstraintStrength {
        let declaration_index = self
            .requirements()
            .iter()
            .position(|id| *id == requirement)
            .unwrap_or(usize::MAX);
        let mut typed_relation_facts = 0;
        let mut total_facts = 0;
        let mut bound_roles: Vec<ProofRole> = Vec::new();
        for atom in self
            .atoms
            .iter()
            .filter(|atom| atom.requirement == requirement)
        {
            for fact in atom.facts {
                total_facts += 1;
                match fact {
                    ProofFactPattern::TypedRelation(pattern) => {
                        typed_relation_facts += 1;
                        note_bound_role(&mut bound_roles, pattern.source);
                        note_bound_role(&mut bound_roles, pattern.target);
                    }
                    ProofFactPattern::SourceAspect(pattern) => {
                        note_bound_role(&mut bound_roles, pattern.symbol);
                    }
                    // Absence and containment facts read roles that other
                    // facts already bound and never bind one themselves, so
                    // they count toward the total only.
                    ProofFactPattern::AbsentTypedRelation(_)
                    | ProofFactPattern::AnchoredLineContainment(_) => {}
                }
            }
        }
        RequirementConstraintStrength {
            typed_relation_facts,
            bound_role_positions: bound_roles.len(),
            total_facts,
            declaration_index,
        }
    }

    /// The formula's requirement ids ordered most-constrained-first — the
    /// evaluation order of the per-requirement fallback in
    /// [`match_flow_requirements`]. Total and stable by construction: see
    /// [`RequirementConstraintStrength`].
    fn requirements_by_constraint_strength(&self) -> Vec<&'static str> {
        let mut requirements = self.requirements();
        requirements
            .sort_by_key(|requirement| self.constraint_strength(requirement).ordering_key());
        requirements
    }
}

/// How strongly one requirement's atoms constrain the group's role
/// assignment, as a property of the formula alone.
///
/// The per-requirement fallback in [`match_flow_requirements`] evaluates
/// requirements in decreasing strength — the most-constrained-variable
/// heuristic of constraint solving — instead of declaration order: the
/// requirement whose atoms discriminate hardest binds the roles it shares
/// with its siblings first, so a weakly constrained sibling inherits correct
/// bindings instead of dictating arbitrary ones from the first shape that
/// happens to match.
///
/// The components, in decreasing precedence:
/// 1. `typed_relation_facts` — required typed-relation facts. One typed
///    relation pins an edge kind, its certainty gate, up to two endpoints and
///    (where the formula names one) a target node kind against the live
///    graph; a carrier range only pins a symbol id to some reread receipt.
/// 2. `bound_role_positions` — distinct roles the requirement's atoms can
///    BIND. [`ProofEndpointPattern::AnyOfRoles`] endpoints, absence sources
///    and containment roles never bind, so they never inflate this.
/// 3. `total_facts` — every required fact, which is where absence and
///    containment guards count.
/// 4. `declaration_index` — position in [`FlowProofFormula::requirements`],
///    the final tie-break that makes the ordering total and stable.
///
/// Nothing here reads the evidence set: the same formula orders its
/// requirements identically on every packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RequirementConstraintStrength {
    /// Count of [`ProofFactPattern::TypedRelation`] facts.
    typed_relation_facts: usize,
    /// Count of distinct roles the requirement's facts can bind.
    bound_role_positions: usize,
    /// Count of all required facts.
    total_facts: usize,
    /// Position among the formula's requirements in declaration order.
    declaration_index: usize,
}

impl RequirementConstraintStrength {
    /// Sort key: ascending in this key is decreasing in constraint strength,
    /// with declaration order breaking every remaining tie, so the induced
    /// ordering of a formula's requirements is total and stable.
    fn ordering_key(self) -> (Reverse<usize>, Reverse<usize>, Reverse<usize>, usize) {
        (
            Reverse(self.typed_relation_facts),
            Reverse(self.bound_role_positions),
            Reverse(self.total_facts),
            self.declaration_index,
        )
    }
}

/// Records the role an endpoint pattern can bind, if any. Only
/// [`ProofEndpointPattern::Role`] ever binds — `AnyOfRoles` requires an
/// already-bound role and `Any` constrains nothing.
fn note_bound_role(roles: &mut Vec<ProofRole>, endpoint: ProofEndpointPattern) {
    if let ProofEndpointPattern::Role(role) = endpoint
        && !roles.contains(&role)
    {
        roles.push(role);
    }
}

/// The proof authority a flow requirement carries.
#[derive(Debug, Clone, Copy)]
pub enum FlowProofSpec {
    /// Today's discharge path, preserved exactly. Legacy requirements never
    /// emit atom-verification receipts.
    Legacy,
    /// The requirement is proven exclusively by the referenced formula's
    /// atoms.
    Atoms(&'static FlowProofFormula),
}

impl FlowProofSpec {
    /// The formula this spec carries; `None` for [`FlowProofSpec::Legacy`],
    /// which has no atoms to verify and therefore can never emit an atom
    /// receipt.
    pub const fn formula(self) -> Option<&'static FlowProofFormula> {
        match self {
            Self::Legacy => None,
            Self::Atoms(formula) => Some(formula),
        }
    }
}

/// Classification of a verified source-aspect receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAspectKind {
    /// A source range the runtime actually reread for a retained,
    /// sufficiency-eligible carrier citation.
    VerifiedCarrierRange,
    /// Reserved for construction-site receipts (P1b); no shipped formula
    /// reads it yet, and every shipped pattern rejects it.
    ConstructionSite,
}

/// Direction of a bounded hydration trail relative to its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailDirection {
    /// The trail followed edges out of the root.
    Outgoing,
    /// The trail followed edges into the root.
    Incoming,
}

/// Per-trail coverage metadata (rule 7).
///
/// The conversion from a live graph edge defaults to
/// [`TrailCoverage::Unknown`]: a caller must opt in with real trail metadata
/// (via [`VerifiedTypedRelationReceipt::with_coverage`] and by listing the
/// scan in [`PacketProofEvidence::trail_scans`]) before any absence fact can
/// discharge. Unknown coverage fails rule 7 closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrailCoverage {
    /// No trail metadata was recorded for this receipt. Never covers
    /// anything: absence facts fail closed on it.
    Unknown,
    /// One bounded trail's coverage record.
    Scanned {
        /// The trail's root node id.
        root: NodeId,
        /// Every edge kind the trail's filter traversed. Rule 7's
        /// deeper-rooted arm requires MEMBER among these in the SAME record
        /// that certifies the absent kind: a single-kind trail rooted at a
        /// file enumerates nothing and must not certify absences over
        /// members it never visited.
        traversal_kinds: Vec<EdgeKind>,
        /// The hydrated direction.
        direction: TrailDirection,
        /// Trail depth. Depth 2 or deeper reaches the root's members' own
        /// edges, which is what C3's covering-scan arm reads.
        depth: u32,
        /// True when the trail hit its node cap. A truncated scan never
        /// supports an absence fact.
        truncated: bool,
    },
}

/// A source range the runtime actually reread, reduced to its
/// receipt-bearing fields. Carries no query, rank, prompt, or provenance
/// field by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSourceAspectReceipt {
    /// Aspect classification.
    pub kind: SourceAspectKind,
    /// Node id of the retained, sufficiency-eligible owner citation.
    pub owner: NodeId,
    /// The reread range's symbol id, when the range is tied to a node.
    pub symbol_id: Option<NodeId>,
    /// First line of the reread range.
    pub start_line: Option<u32>,
    /// Last line of the reread range.
    pub end_line: Option<u32>,
    /// The atom that anchored this window under R4, when the verification
    /// pass anchored it for one. Rule 3(b) reads exactly this mark.
    pub atom_anchor: Option<ProofAtomId>,
}

impl VerifiedSourceAspectReceipt {
    /// Builds a [`SourceAspectKind::VerifiedCarrierRange`] receipt from a
    /// reread `SourceRange` support unit and its retained owner citation.
    ///
    /// Returns `None` unless the unit is a `SourceRange` with a non-empty
    /// snippet and the owner citation is sufficiency-eligible (rule 1). Only
    /// receipt-bearing fields are read: the unit's `query` and the citation's
    /// score and retrieval provenance are dropped at this boundary (rule 4).
    pub fn from_source_range_unit(
        unit: &SupportUnitDto,
        owner: &AgentCitationDto,
        atom_anchor: Option<ProofAtomId>,
    ) -> Option<Self> {
        if unit.kind != SupportUnitKindDto::SourceRange {
            return None;
        }
        if unit
            .snippet
            .as_deref()
            .is_none_or(|snippet| snippet.trim().is_empty())
        {
            return None;
        }
        if owner.eligible_for_sufficiency != Some(true) {
            return None;
        }
        Some(Self {
            kind: SourceAspectKind::VerifiedCarrierRange,
            owner: owner.node_id.clone(),
            symbol_id: unit.symbol_id.clone().map(NodeId),
            start_line: unit.start_line,
            end_line: unit.end_line,
            atom_anchor,
        })
    }
}

/// An edge present in the live answer graphs at finalize time. Carries no
/// query, rank, prompt, or provenance field by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedTypedRelationReceipt {
    /// The edge id, reported back per discharged atom.
    pub edge_id: EdgeId,
    /// Edge kind.
    pub kind: EdgeKind,
    /// Effective source node id (resolved endpoints are applied before DTO
    /// construction).
    pub source: NodeId,
    /// Effective target node id.
    pub target: NodeId,
    /// The effective target node's kind, when the graph carries the node —
    /// the rule-6 admissibility proxy for structural edges.
    pub target_kind: Option<NodeKind>,
    /// Producer/resolver certainty (`certain`, `probable`, `uncertain`).
    pub certainty: Option<String>,
    /// The raw callsite identity; the matcher shape-validates before reading.
    pub callsite_identity: Option<String>,
    /// Coverage record of the trail that hydrated this edge. Defaults to
    /// [`TrailCoverage::Unknown`] at conversion; absence facts require an
    /// explicit opt-in via [`VerifiedTypedRelationReceipt::with_coverage`].
    pub coverage: TrailCoverage,
}

impl VerifiedTypedRelationReceipt {
    /// Builds the receipt from a live graph edge plus its effective target's
    /// node kind. Coverage defaults to [`TrailCoverage::Unknown`], which
    /// fails every absence fact closed until the caller opts in with real
    /// trail metadata.
    ///
    /// Only receipt-bearing fields are read: the edge's `confidence` score
    /// and `candidate_targets` are dropped at this boundary.
    pub fn from_graph_edge(edge: &GraphEdgeDto, target_kind: Option<NodeKind>) -> Self {
        Self {
            edge_id: edge.id.clone(),
            kind: edge.kind,
            source: edge.source.clone(),
            target: edge.target.clone(),
            target_kind,
            certainty: edge.certainty.clone(),
            callsite_identity: edge.callsite_identity.clone(),
            coverage: TrailCoverage::Unknown,
        }
    }

    /// Attaches real trail coverage metadata — the explicit opt-in rule 7
    /// requires before this receipt can witness an absence fact.
    pub fn with_coverage(mut self, coverage: TrailCoverage) -> Self {
        self.coverage = coverage;
        self
    }
}

/// Everything the matcher may consult: verified receipts and the coverage
/// records of every bounded scan, including scans that produced no edges
/// (absence facts need those). No query, rank, prompt, or provenance field
/// exists here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PacketProofEvidence {
    /// Verified reread source ranges.
    pub source_aspects: Vec<VerifiedSourceAspectReceipt>,
    /// Verified live graph edges. Callers must include every edge the listed
    /// scans enumerated; absence facts read this set against `trail_scans`.
    pub typed_relations: Vec<VerifiedTypedRelationReceipt>,
    /// Coverage records of every bounded scan run for this packet.
    /// [`TrailCoverage::Unknown`] entries never cover anything.
    pub trail_scans: Vec<TrailCoverage>,
}

/// The exact receipt identities one discharged fact used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DischargedFact {
    /// A typed-relation receipt discharged the fact.
    TypedRelation {
        /// The discharging edge.
        edge_id: EdgeId,
        /// Its effective source node id.
        source: NodeId,
        /// Its effective target node id.
        target: NodeId,
    },
    /// A source-aspect receipt discharged the fact.
    SourceAspect {
        /// The owning citation's node id (the carrier identity).
        owner: NodeId,
        /// The reread range's symbol id.
        symbol_id: Option<NodeId>,
        /// First line of the range.
        start_line: Option<u32>,
        /// Last line of the range.
        end_line: Option<u32>,
    },
    /// An absence fact was proven over this covering scan.
    CoveredAbsence {
        /// Root of the covering scan.
        root: NodeId,
        /// Edge kind whose absence the scan covered.
        edge_kind: EdgeKind,
        /// Depth of the covering scan.
        depth: u32,
    },
    /// An anchored containment fact held.
    AnchoredLineContainment {
        /// The receipt-carried line that was covered.
        line: u32,
        /// The anchored window's owner citation node id.
        window_owner: NodeId,
        /// First line of the anchored window.
        window_start_line: u32,
        /// Last line of the anchored window.
        window_end_line: u32,
    },
}

/// One discharged atom with the receipts it used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DischargedAtom {
    /// The atom.
    pub atom: ProofAtomId,
    /// The requirement it is materially required by.
    pub requirement: &'static str,
    /// The discharged facts, in the spec's fact order.
    pub facts: Vec<DischargedFact>,
}

/// A successful match: the group-wide role bindings and every discharged atom
/// with its exact receipt identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedFlowProof {
    /// Node id bound to each role the match used.
    pub bindings: BTreeMap<ProofRole, NodeId>,
    /// Discharged atoms in formula order.
    pub atoms: Vec<DischargedAtom>,
}

/// The verdict of one matcher invocation.
///
/// [`FlowProofOutcome::Aborted`] is fail-closed for discharge exactly like
/// [`FlowProofOutcome::Unproven`] — no proof exists either way — but stays
/// observable so callers can distinguish "the evidence misses or refutes the
/// formula" from "the search hit its step bound".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowProofOutcome {
    /// Every required atom discharged under one role assignment.
    Proved(VerifiedFlowProof),
    /// Some required atom cannot discharge, a distinctness constraint failed,
    /// or the atom filter was empty or named an atom the formula does not
    /// contain. Fail closed (rule 9).
    Unproven,
    /// The step bound (`MATCH_STEP_LIMIT`, 4096 receipt considerations) was
    /// exhausted before the search completed. Never a proof; observable for
    /// telemetry.
    Aborted,
}

impl FlowProofOutcome {
    /// The proof, when this outcome is [`FlowProofOutcome::Proved`].
    pub fn proof(&self) -> Option<&VerifiedFlowProof> {
        match self {
            Self::Proved(proof) => Some(proof),
            Self::Unproven | Self::Aborted => None,
        }
    }
}

/// Matches every atom of the formula against the evidence under one
/// group-wide role assignment. Any atom that cannot discharge — or a failed
/// distinctness constraint — yields [`FlowProofOutcome::Unproven`] (fail
/// closed, rule 9); an exhausted step bound yields
/// [`FlowProofOutcome::Aborted`].
pub fn match_flow_proof(
    formula: &FlowProofFormula,
    evidence: &PacketProofEvidence,
) -> FlowProofOutcome {
    let all = formula.atoms.iter().map(|atom| atom.id).collect::<Vec<_>>();
    match_required_atoms(formula, &all, evidence)
}

/// Matches only the listed atoms of the formula, in formula order, under one
/// shared role assignment. An empty list or an atom id the formula does not
/// contain is [`FlowProofOutcome::Unproven`] (fail closed). Distinctness
/// constraints apply to the roles the partial match bound.
pub fn match_required_atoms(
    formula: &FlowProofFormula,
    required: &[ProofAtomId],
    evidence: &PacketProofEvidence,
) -> FlowProofOutcome {
    match_atoms_with_bindings(formula, required, evidence, BTreeMap::new())
}

/// Per-requirement verdicts under one group-wide assignment.
///
/// The full group is tried first: when it matches, every requirement is
/// proven under that single assignment and reported with its own atoms and
/// the shared bindings. Otherwise each requirement's atom subset is matched
/// on its own, MOST-CONSTRAINED-FIRST — ordered by how many typed-relation
/// facts it requires, then by how many distinct roles its atoms can bind,
/// then by its total fact count, with declaration order as the final
/// tie-break — and with role bindings required to stay consistent with every
/// earlier successful subset: a role one requirement bound constrains its
/// siblings, so per-requirement verdicts never fork the assignment.
///
/// Evaluating in constraint-strength order rather than declaration order is
/// what keeps a weakly constrained requirement from capturing a shared role
/// on an arbitrary match and starving its stronger sibling; the ordering is a
/// property of the formula alone, so it is the same on every packet. It never
/// manufactures a verdict: each Proved verdict is still a real discharge of
/// that requirement's own atoms, under bindings that some subset actually
/// proved, and a failed or aborted subset still carries nothing forward.
/// Verdicts are REPORTED in declaration order regardless of the order they
/// were evaluated in — the evaluation order is a solving strategy, not part
/// of the result contract.
///
/// A full-group abort falls through to the per-requirement pass, where any
/// abort is reported on the requirement that hit it.
pub fn match_flow_requirements(
    formula: &FlowProofFormula,
    evidence: &PacketProofEvidence,
) -> Vec<(&'static str, FlowProofOutcome)> {
    let requirements = formula.requirements();
    if let FlowProofOutcome::Proved(proof) = match_flow_proof(formula, evidence) {
        return requirements
            .into_iter()
            .map(|requirement| {
                let atoms = proof
                    .atoms
                    .iter()
                    .filter(|atom| atom.requirement == requirement)
                    .cloned()
                    .collect();
                (
                    requirement,
                    FlowProofOutcome::Proved(VerifiedFlowProof {
                        bindings: proof.bindings.clone(),
                        atoms,
                    }),
                )
            })
            .collect();
    }
    let mut carried = BTreeMap::new();
    let mut by_declaration: Vec<Option<FlowProofOutcome>> = vec![None; requirements.len()];
    for requirement in formula.requirements_by_constraint_strength() {
        let outcome = match_atoms_with_bindings(
            formula,
            &formula.atoms_for(requirement),
            evidence,
            carried.clone(),
        );
        if let FlowProofOutcome::Proved(proof) = &outcome {
            carried = proof.bindings.clone();
        }
        if let Some(slot) = requirements
            .iter()
            .position(|id| *id == requirement)
            .and_then(|index| by_declaration.get_mut(index))
        {
            *slot = Some(outcome);
        }
    }
    requirements
        .into_iter()
        .zip(by_declaration)
        // Unreachable: the strength ordering is a permutation of the
        // declaration order, so every slot is filled. Fail closed anyway.
        .map(|(requirement, outcome)| (requirement, outcome.unwrap_or(FlowProofOutcome::Unproven)))
        .collect()
}

/// Shared engine: matches the listed atoms in formula order with the role
/// assignment seeded from `bindings`.
fn match_atoms_with_bindings(
    formula: &FlowProofFormula,
    required: &[ProofAtomId],
    evidence: &PacketProofEvidence,
    bindings: BTreeMap<ProofRole, NodeId>,
) -> FlowProofOutcome {
    if required.is_empty() {
        return FlowProofOutcome::Unproven;
    }
    if required
        .iter()
        .any(|id| !formula.atoms.iter().any(|atom| atom.id == *id))
    {
        return FlowProofOutcome::Unproven;
    }
    let selected = formula
        .atoms
        .iter()
        .filter(|atom| required.contains(&atom.id))
        .copied()
        .collect::<Vec<_>>();
    let mut solver = Solver {
        evidence,
        bindings,
        facts: Vec::new(),
        steps: 0,
        aborted: false,
    };
    let solved = solver.solve(&selected, formula.distinct_roles, 0, 0);
    if solver.aborted {
        return FlowProofOutcome::Aborted;
    }
    if !solved {
        return FlowProofOutcome::Unproven;
    }
    let mut atoms = selected
        .iter()
        .map(|atom| DischargedAtom {
            atom: atom.id,
            requirement: atom.requirement,
            facts: Vec::new(),
        })
        .collect::<Vec<_>>();
    for (atom_index, fact) in solver.facts {
        atoms[atom_index].facts.push(fact);
    }
    FlowProofOutcome::Proved(VerifiedFlowProof {
        bindings: solver.bindings,
        atoms,
    })
}

/// Depth-first backtracking search over the selected atoms' facts.
///
/// Recursion depth is bounded by the static fact count of the formula; the
/// receipt iteration inside each fact is bounded by [`MATCH_STEP_LIMIT`]
/// consumed steps, after which the whole match aborts.
struct Solver<'evidence> {
    evidence: &'evidence PacketProofEvidence,
    bindings: BTreeMap<ProofRole, NodeId>,
    facts: Vec<(usize, DischargedFact)>,
    steps: usize,
    aborted: bool,
}

impl Solver<'_> {
    fn step(&mut self) -> bool {
        self.steps += 1;
        if self.steps > MATCH_STEP_LIMIT {
            self.aborted = true;
        }
        !self.aborted
    }

    fn bind_endpoint(
        &mut self,
        pattern: ProofEndpointPattern,
        value: &NodeId,
        newly_bound: &mut Vec<ProofRole>,
    ) -> bool {
        match pattern {
            ProofEndpointPattern::Any => true,
            ProofEndpointPattern::Role(role) => match self.bindings.get(&role) {
                Some(bound) => bound == value,
                None => {
                    self.bindings.insert(role, value.clone());
                    newly_bound.push(role);
                    true
                }
            },
            ProofEndpointPattern::AnyOfRoles(roles) => roles
                .iter()
                .any(|role| self.bindings.get(role) == Some(value)),
        }
    }

    fn unbind(&mut self, newly_bound: &[ProofRole]) {
        for role in newly_bound {
            self.bindings.remove(role);
        }
    }

    fn solve(
        &mut self,
        atoms: &[ProofAtomSpec],
        distinct: &[&[ProofRole]],
        atom_index: usize,
        fact_index: usize,
    ) -> bool {
        if self.aborted {
            return false;
        }
        let Some(atom) = atoms.get(atom_index) else {
            return distinct_bindings_hold(distinct, &self.bindings);
        };
        let Some(fact) = atom.facts.get(fact_index) else {
            return self.solve(atoms, distinct, atom_index + 1, 0);
        };
        match fact {
            ProofFactPattern::TypedRelation(pattern) => {
                self.solve_typed_relation(atoms, distinct, atom_index, fact_index, pattern)
            }
            ProofFactPattern::SourceAspect(pattern) => {
                self.solve_source_aspect(atoms, distinct, atom_index, fact_index, pattern, atom.id)
            }
            ProofFactPattern::AbsentTypedRelation(pattern) => {
                let Some(discharged) = self.absence_discharge(pattern) else {
                    return false;
                };
                self.facts.push((atom_index, discharged));
                if self.solve(atoms, distinct, atom_index, fact_index + 1) {
                    return true;
                }
                self.facts.pop();
                false
            }
            ProofFactPattern::AnchoredLineContainment(pattern) => {
                let Some(discharged) = self.containment_discharge(pattern, atom.id) else {
                    return false;
                };
                self.facts.push((atom_index, discharged));
                if self.solve(atoms, distinct, atom_index, fact_index + 1) {
                    return true;
                }
                self.facts.pop();
                false
            }
        }
    }

    fn solve_typed_relation(
        &mut self,
        atoms: &[ProofAtomSpec],
        distinct: &[&[ProofRole]],
        atom_index: usize,
        fact_index: usize,
        pattern: &TypedRelationPattern,
    ) -> bool {
        let evidence = self.evidence;
        for receipt in &evidence.typed_relations {
            if !self.step() {
                return false;
            }
            if !typed_relation_admissible(pattern, receipt) {
                continue;
            }
            let mut newly_bound = Vec::new();
            let bound = self.bind_endpoint(pattern.source, &receipt.source, &mut newly_bound)
                && self.bind_endpoint(pattern.target, &receipt.target, &mut newly_bound);
            if bound {
                self.facts.push((
                    atom_index,
                    DischargedFact::TypedRelation {
                        edge_id: receipt.edge_id.clone(),
                        source: receipt.source.clone(),
                        target: receipt.target.clone(),
                    },
                ));
                if self.solve(atoms, distinct, atom_index, fact_index + 1) {
                    return true;
                }
                self.facts.pop();
            }
            self.unbind(&newly_bound);
            if self.aborted {
                return false;
            }
        }
        false
    }

    fn solve_source_aspect(
        &mut self,
        atoms: &[ProofAtomSpec],
        distinct: &[&[ProofRole]],
        atom_index: usize,
        fact_index: usize,
        pattern: &SourceAspectPattern,
        atom_id: ProofAtomId,
    ) -> bool {
        let evidence = self.evidence;
        for receipt in &evidence.source_aspects {
            if !self.step() {
                return false;
            }
            if receipt.kind != pattern.kind {
                continue;
            }
            if pattern.require_atom_anchor && receipt.atom_anchor != Some(atom_id) {
                continue;
            }
            let mut newly_bound = Vec::new();
            let bound = match pattern.symbol {
                ProofEndpointPattern::Any => true,
                constrained => receipt.symbol_id.clone().is_some_and(|symbol| {
                    self.bind_endpoint(constrained, &symbol, &mut newly_bound)
                }),
            };
            if bound {
                self.facts.push((
                    atom_index,
                    DischargedFact::SourceAspect {
                        owner: receipt.owner.clone(),
                        symbol_id: receipt.symbol_id.clone(),
                        start_line: receipt.start_line,
                        end_line: receipt.end_line,
                    },
                ));
                if self.solve(atoms, distinct, atom_index, fact_index + 1) {
                    return true;
                }
                self.facts.pop();
            }
            self.unbind(&newly_bound);
            if self.aborted {
                return false;
            }
        }
        false
    }

    /// Rule 7: the absence holds only when no admissible-or-unknown edge
    /// contradicts it AND an untruncated outgoing scan with known traversal
    /// kinds provably covers the node's outgoing set of that kind — either
    /// rooted at the node itself, or a depth≥2 scan whose OWN traversal kinds
    /// include MEMBER plus an in-evidence MEMBER(root → node) witness
    /// hydrated untruncated from that root. Unknown coverage never covers.
    /// Creates no bindings, so the first covering scan (input order) is the
    /// deterministic witness.
    fn absence_discharge(
        &mut self,
        pattern: &AbsentTypedRelationPattern,
    ) -> Option<DischargedFact> {
        let evidence = self.evidence;
        let source = self.bindings.get(&pattern.source)?.clone();
        for receipt in &evidence.typed_relations {
            if !self.step() {
                return None;
            }
            if receipt.kind == pattern.kind && receipt.source == source {
                match receipt.target_kind {
                    Some(kind) if kind != pattern.forbidden_target_kind => {}
                    // A forbidden-kind target refutes the absence; an unknown
                    // target kind fails it closed.
                    _ => return None,
                }
            }
        }
        for scan in &evidence.trail_scans {
            if !self.step() {
                return None;
            }
            let TrailCoverage::Scanned {
                root,
                traversal_kinds,
                direction,
                depth,
                truncated,
            } = scan
            else {
                continue;
            };
            if *truncated
                || *direction != TrailDirection::Outgoing
                || !traversal_kinds.contains(&pattern.kind)
            {
                continue;
            }
            let covers = *root == source
                || (*depth >= 2
                    && traversal_kinds.contains(&EdgeKind::MEMBER)
                    && member_reached_from_root(evidence, root, &source));
            if covers {
                return Some(DischargedFact::CoveredAbsence {
                    root: root.clone(),
                    edge_kind: pattern.kind,
                    depth: *depth,
                });
            }
        }
        None
    }

    /// Rule 3(b): a receipt-carried line inside an atom-anchored window. The
    /// line carrier must be a source-aspect receipt of the kind the atom
    /// names. Creates no bindings; the first satisfying receipt pair in input
    /// order is the deterministic witness. The line carrier and the window
    /// may be the same receipt (rule 3(a) same-receipt arithmetic).
    fn containment_discharge(
        &mut self,
        pattern: &AnchoredContainmentPattern,
        atom_id: ProofAtomId,
    ) -> Option<DischargedFact> {
        let evidence = self.evidence;
        let line_node = self.bindings.get(&pattern.line_symbol)?.clone();
        let window_owner = self.bindings.get(&pattern.window_owner)?.clone();
        for line_receipt in &evidence.source_aspects {
            if !self.step() {
                return None;
            }
            if line_receipt.kind != pattern.kind {
                continue;
            }
            if line_receipt.symbol_id.as_ref() != Some(&line_node) {
                continue;
            }
            let Some(line) = line_receipt.start_line else {
                continue;
            };
            for window in &evidence.source_aspects {
                if !self.step() {
                    return None;
                }
                if window.owner != window_owner || window.atom_anchor != Some(atom_id) {
                    continue;
                }
                let (Some(window_start_line), Some(window_end_line)) =
                    (window.start_line, window.end_line)
                else {
                    continue;
                };
                if window_start_line <= line && line <= window_end_line {
                    return Some(DischargedFact::AnchoredLineContainment {
                        line,
                        window_owner: window.owner.clone(),
                        window_start_line,
                        window_end_line,
                    });
                }
            }
        }
        None
    }
}

/// True when every distinctness set's bound roles carry pairwise-distinct
/// node ids.
fn distinct_bindings_hold(
    distinct: &[&[ProofRole]],
    bindings: &BTreeMap<ProofRole, NodeId>,
) -> bool {
    distinct.iter().all(|set| {
        let bound = set
            .iter()
            .filter_map(|role| bindings.get(role))
            .collect::<Vec<_>>();
        bound
            .iter()
            .enumerate()
            .all(|(index, id)| bound[index + 1..].iter().all(|other| other != id))
    })
}

/// True when a MEMBER edge from `root` to `node` is in evidence and was
/// itself hydrated by an untruncated trail rooted at `root` — the witness
/// that a deeper-rooted scan actually reached `node` (rule 7's covering arm).
/// A witness with [`TrailCoverage::Unknown`] never counts.
fn member_reached_from_root(evidence: &PacketProofEvidence, root: &NodeId, node: &NodeId) -> bool {
    evidence.typed_relations.iter().any(|receipt| {
        receipt.kind == EdgeKind::MEMBER
            && receipt.source == *root
            && receipt.target == *node
            && matches!(
                &receipt.coverage,
                TrailCoverage::Scanned { root: coverage_root, truncated: false, .. }
                    if coverage_root == root
            )
    })
}

/// Single-receipt admissibility checks that never touch role bindings.
fn typed_relation_admissible(
    pattern: &TypedRelationPattern,
    receipt: &VerifiedTypedRelationReceipt,
) -> bool {
    receipt.kind == pattern.kind
        && certainty_gate_passes(receipt.kind, receipt.certainty.as_deref())
        && pattern
            .target_kind
            .is_none_or(|kind| receipt.target_kind == Some(kind))
        && (!pattern.target_distinct_from_source || receipt.source != receipt.target)
        && markers_satisfied(pattern.markers, receipt.callsite_identity.as_deref())
}

/// Rule 6, attributed per kind: CALL and TYPE_USAGE receipts must carry
/// `certain`; structural MEMBER, USAGE, and IMPORT edges are excluded from
/// the gate (their admissibility is read through the effective target's node
/// kind). Any other kind is gated conservatively, since no shipped formula
/// names one.
fn certainty_gate_passes(kind: EdgeKind, certainty: Option<&str>) -> bool {
    match kind {
        EdgeKind::MEMBER | EdgeKind::USAGE | EdgeKind::IMPORT => true,
        _ => certainty == Some(CERTAINTY_CERTAIN),
    }
}

/// True when every required marker is satisfied by the callsite identity. A
/// non-empty requirement list with no identity fails closed.
fn markers_satisfied(markers: &[CallsiteMarkerPattern], identity: Option<&str>) -> bool {
    if markers.is_empty() {
        return true;
    }
    let Some(identity) = identity else {
        return false;
    };
    markers.iter().all(|marker| match marker {
        CallsiteMarkerPattern::SyntaxCall => syntax_marker_present(identity, "-call"),
        CallsiteMarkerPattern::SyntaxNew => syntax_marker_present(identity, "-new"),
        CallsiteMarkerPattern::ReceiverOwner => marker_segments(identity).any(|segment| {
            segment
                .strip_prefix(RECEIVER_OWNER_MARKER_PREFIX)
                .is_some_and(|value| !value.is_empty())
        }),
        CallsiteMarkerPattern::LoopElementContainsCallsiteLine => {
            loop_element_containment_holds(identity)
        }
    })
}

/// The order-agnostic marker segments: everything after the canonical first
/// `|`-segment (rule 5).
fn marker_segments(identity: &str) -> impl Iterator<Item = &str> {
    identity.split('|').skip(1)
}

/// True when a `syntax:{lang}{suffix}` marker with a non-empty language part
/// is present.
fn syntax_marker_present(identity: &str, suffix: &str) -> bool {
    marker_segments(identity).any(|segment| {
        segment
            .strip_prefix(SYNTAX_MARKER_PREFIX)
            .and_then(|rest| rest.strip_suffix(suffix))
            .is_some_and(|language| !language.is_empty())
    })
}

/// Shape-validates the canonical first segment (`file:line:col:target`, four
/// non-empty `:`-fields, numeric line and column) and returns the canonical
/// line. A malformed first segment yields `None`, and every
/// containment-dependent check fails closed (rule 5).
fn canonical_callsite_line(identity: &str) -> Option<u32> {
    let first = identity.split('|').next()?;
    let fields = first.split(':').collect::<Vec<_>>();
    if fields.len() != 4 || fields.iter().any(|field| field.is_empty()) {
        return None;
    }
    let line = parse_ascii_u32(fields[1])?;
    parse_ascii_u32(fields[2])?;
    Some(line)
}

/// Strict all-digits `u32` parse; anything else is `None`.
fn parse_ascii_u32(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>().ok()
}

/// True when the identity carries a strictly parsed
/// `receiver-binding:loop-element@{start}-{end}` marker whose range contains
/// the shape-validated canonical callsite line. Any malformed loop-element
/// segment fails the whole check closed.
fn loop_element_containment_holds(identity: &str) -> bool {
    let Some(line) = canonical_callsite_line(identity) else {
        return false;
    };
    let mut contained = false;
    for segment in marker_segments(identity) {
        let Some(range) = segment.strip_prefix(LOOP_ELEMENT_MARKER_PREFIX) else {
            continue;
        };
        let Some((start_text, end_text)) = range.split_once('-') else {
            return false;
        };
        let (Some(start), Some(end)) = (parse_ascii_u32(start_text), parse_ascii_u32(end_text))
        else {
            return false;
        };
        if start > end {
            return false;
        }
        if start <= line && line <= end {
            contained = true;
        }
    }
    contained
}

/// Endpoint shorthand for the const formula tables below.
const fn role(role: ProofRole) -> ProofEndpointPattern {
    ProofEndpointPattern::Role(role)
}

/// A marker-free typed-relation fact. The certainty gate is attributed from
/// `kind` at match time (rule 6); `target_kind` is the structural
/// admissibility proxy where the formula names one.
const fn edge_fact(
    kind: EdgeKind,
    source: ProofEndpointPattern,
    target: ProofEndpointPattern,
    target_kind: Option<NodeKind>,
) -> ProofFactPattern {
    ProofFactPattern::TypedRelation(TypedRelationPattern {
        kind,
        source,
        target,
        target_kind,
        markers: &[],
        target_distinct_from_source: false,
    })
}

/// A verified-carrier-range fact whose symbol binds `symbol`, optionally
/// requiring this atom's R4 anchor mark.
const fn carrier_range_fact(symbol: ProofRole, require_atom_anchor: bool) -> ProofFactPattern {
    ProofFactPattern::SourceAspect(SourceAspectPattern {
        kind: SourceAspectKind::VerifiedCarrierRange,
        symbol: ProofEndpointPattern::Role(symbol),
        require_atom_anchor,
    })
}

/// M3 — receiver-annotated dispatch CALL: certain, from the flow owner, onto
/// a resolved method distinct from the owner, carrying `syntax:{lang}-call`
/// and `receiver-owner:` classification segments.
const M3_DISPATCH_CALL: TypedRelationPattern = TypedRelationPattern {
    kind: EdgeKind::CALL,
    source: role(ProofRole::FlowOwner),
    target: ProofEndpointPattern::Any,
    target_kind: Some(NodeKind::METHOD),
    markers: &[
        CallsiteMarkerPattern::SyntaxCall,
        CallsiteMarkerPattern::ReceiverOwner,
    ],
    target_distinct_from_source: true,
};

/// M — logging flow (`LOG_HANDLER_FLOW`) proof formula.
///
/// M2 restates M3's edge shape and adds the combined loop marker plus its
/// same-receipt line containment, so the marker verifiably rides a
/// dispatch-shaped edge: joins stay node-identity-only and any edge
/// discharging M2 also satisfies M3's pattern. M1b's construction CALL keeps
/// its target unconstrained — "object construction" is the fact; no
/// vocabulary may name the type.
pub const LOG_HANDLER_FLOW_PROOF: FlowProofFormula = FlowProofFormula {
    atoms: &[
        ProofAtomSpec {
            id: ProofAtomId::M1a,
            requirement: "logger_event",
            facts: &[carrier_range_fact(ProofRole::FlowOwner, false)],
        },
        ProofAtomSpec {
            id: ProofAtomId::M1b,
            requirement: "logger_event",
            facts: &[ProofFactPattern::TypedRelation(TypedRelationPattern {
                target_kind: None,
                markers: &[CallsiteMarkerPattern::SyntaxNew],
                target_distinct_from_source: false,
                ..M3_DISPATCH_CALL
            })],
        },
        ProofAtomSpec {
            id: ProofAtomId::M2,
            requirement: "handler_processing",
            facts: &[ProofFactPattern::TypedRelation(TypedRelationPattern {
                markers: &[
                    CallsiteMarkerPattern::SyntaxCall,
                    CallsiteMarkerPattern::ReceiverOwner,
                    CallsiteMarkerPattern::LoopElementContainsCallsiteLine,
                ],
                ..M3_DISPATCH_CALL
            })],
        },
        ProofAtomSpec {
            id: ProofAtomId::M3,
            requirement: "handler_processing",
            facts: &[ProofFactPattern::TypedRelation(M3_DISPATCH_CALL)],
        },
    ],
    distinct_roles: &[],
};

/// A — mapping flow (`MAPPER_PLAN_FLOW`) proof formula. All joins are node-id
/// joins: `Builder` spans A1's source, A5's membership constraint, A3's
/// MEMBER edge, and A4's carrier range; `ConfigType` spans A1's target and
/// A2's carrier range.
///
/// A5 states STRUCTURALLY what the `Builder` role means, so `mapper_config`
/// cannot be satisfied by an arbitrary type-usage pair. A1 alone admits ANY
/// certain TYPE_USAGE edge, and on a large index that is thousands of equally
/// admissible pairs — a lone parameter type is as good a "builder" as the
/// type whose plan methods actually run, so the requirement proves on
/// whichever pair retrieval surfaced first and the bounded carrier slots then
/// protect that pair. A plan builder is not "a type that appears as the
/// source of a type-usage edge"; it is a type whose OWN methods get called.
/// A5 requires exactly the owning half of that shape — a MEMBER edge from
/// `Builder` onto a METHOD — with an `Any` target, so it constrains the role
/// without joining anything: it never binds a second role, never names a
/// method identity, and adds no vocabulary, name, path, or count.
///
/// SEPARATE ATOM, NOT A SECOND FACT INSIDE A1 (deliberate). Discharge is
/// identical either way — both are conjuncts of `mapper_config`, and a
/// missing MEMBER receipt fails the requirement closed in both shapes — so
/// the choice is decided by what the ATOM is the unit of everywhere else:
///
/// * The pre-cap carrier protection runs a weighted set cover over the atoms
///   a partial match verified, and a carrier's weight is the SET OF ATOMS it
///   covers. As its own atom, the member-bearing type covers {A1, A5} while
///   the configuration type covers {A1, A2}, so the type that carries the
///   builder shape competes for a bounded slot on equal footing instead of
///   trailing a config type by one atom. Folded into A1 it would cover {A1}
///   exactly as it does today, which would tighten what PROVES while leaving
///   what gets PROTECTED unchanged — and protection is the stage where the
///   defect actually reaches the answer.
/// * Per-requirement atom reporting and the R4 anchor planner both read atom
///   specs, so a distinct id makes "the builder is member-bearing" a
///   separately reported obligation rather than a silent extra clause on the
///   configuration edge.
/// * A1 stays a single-receipt atom, which keeps the candidate-level
///   single-receipt mirror a one-pattern-per-atom parity with this matcher
///   for the whole A family.
///
/// Fail-closed either way, and this is the safer direction: with no MEMBER
/// receipt retained for the bound `Builder`, `mapper_config` reports
/// Unproven instead of reporting a proof off an unrelated pair.
pub const MAPPER_PLAN_FLOW_PROOF: FlowProofFormula = FlowProofFormula {
    atoms: &[
        ProofAtomSpec {
            id: ProofAtomId::A1,
            requirement: "mapper_config",
            facts: &[edge_fact(
                EdgeKind::TYPE_USAGE,
                role(ProofRole::Builder),
                role(ProofRole::ConfigType),
                None,
            )],
        },
        ProofAtomSpec {
            id: ProofAtomId::A2,
            requirement: "mapper_config",
            facts: &[carrier_range_fact(ProofRole::ConfigType, false)],
        },
        ProofAtomSpec {
            id: ProofAtomId::A5,
            requirement: "mapper_config",
            facts: &[edge_fact(
                EdgeKind::MEMBER,
                role(ProofRole::Builder),
                ProofEndpointPattern::Any,
                Some(NodeKind::METHOD),
            )],
        },
        ProofAtomSpec {
            id: ProofAtomId::A3,
            requirement: "mapper_execution",
            facts: &[
                edge_fact(
                    EdgeKind::CALL,
                    role(ProofRole::PlanOwner),
                    role(ProofRole::BuilderMethod),
                    Some(NodeKind::METHOD),
                ),
                edge_fact(
                    EdgeKind::MEMBER,
                    role(ProofRole::Builder),
                    role(ProofRole::BuilderMethod),
                    Some(NodeKind::METHOD),
                ),
            ],
        },
        ProofAtomSpec {
            id: ProofAtomId::A4,
            requirement: "mapper_execution",
            facts: &[carrier_range_fact(ProofRole::Builder, false)],
        },
    ],
    distinct_roles: &[],
};

/// C — CSS animation flow (`CSS_ANIMATION_FLOW`) proof formula.
///
/// C1 sits last so its bound-roles-only IMPORT guard and its containment
/// read the roles C2-C4 established. Per contract rev 5.1/5.2, C1 is a
/// verified entrypoint range covering the declaration line of a MODULE-kind
/// member — the P3.i import-statement node — PLUS an IMPORT edge from the
/// entrypoint onto an ALREADY-BOUND source-file role. The guard is not a
/// tautology: under per-requirement matching, a failed structure closure
/// leaves those roles unbound and `css_animation_entrypoint` then fails
/// closed instead of binding `Entrypoint` to an arbitrary
/// MODULE-member-bearing file. (The per-edge statement-to-IMPORT linkage
/// stays dropped — no producer fact connects a statement node to its
/// file-to-file IMPORT edge.) The three source files are pairwise distinct
/// by decision.
pub const CSS_ANIMATION_FLOW_PROOF: FlowProofFormula = FlowProofFormula {
    atoms: &[
        ProofAtomSpec {
            id: ProofAtomId::C2,
            requirement: "css_animation_structure",
            facts: &[
                edge_fact(
                    EdgeKind::IMPORT,
                    role(ProofRole::Entrypoint),
                    role(ProofRole::VarsSource),
                    Some(NodeKind::FILE),
                ),
                edge_fact(
                    EdgeKind::MEMBER,
                    role(ProofRole::VarsSource),
                    role(ProofRole::VarNode),
                    Some(NodeKind::VARIABLE),
                ),
                carrier_range_fact(ProofRole::VarNode, true),
            ],
        },
        ProofAtomSpec {
            id: ProofAtomId::C3,
            requirement: "css_animation_structure",
            facts: &[
                edge_fact(
                    EdgeKind::IMPORT,
                    role(ProofRole::Entrypoint),
                    role(ProofRole::BaseSource),
                    Some(NodeKind::FILE),
                ),
                edge_fact(
                    EdgeKind::MEMBER,
                    role(ProofRole::BaseSource),
                    role(ProofRole::BaseSelector),
                    Some(NodeKind::CONSTANT),
                ),
                edge_fact(
                    EdgeKind::USAGE,
                    role(ProofRole::BaseSelector),
                    role(ProofRole::VarNode),
                    Some(NodeKind::VARIABLE),
                ),
                ProofFactPattern::AbsentTypedRelation(AbsentTypedRelationPattern {
                    source: ProofRole::BaseSelector,
                    kind: EdgeKind::USAGE,
                    forbidden_target_kind: NodeKind::FUNCTION,
                }),
            ],
        },
        ProofAtomSpec {
            id: ProofAtomId::C4,
            requirement: "css_animation_structure",
            facts: &[
                edge_fact(
                    EdgeKind::IMPORT,
                    role(ProofRole::Entrypoint),
                    role(ProofRole::AnimSource),
                    Some(NodeKind::FILE),
                ),
                edge_fact(
                    EdgeKind::MEMBER,
                    role(ProofRole::AnimSource),
                    role(ProofRole::KeyframeNode),
                    Some(NodeKind::FUNCTION),
                ),
                carrier_range_fact(ProofRole::KeyframeNode, true),
                edge_fact(
                    EdgeKind::MEMBER,
                    role(ProofRole::AnimSource),
                    role(ProofRole::AnimationSelector),
                    Some(NodeKind::CONSTANT),
                ),
                edge_fact(
                    EdgeKind::USAGE,
                    role(ProofRole::AnimationSelector),
                    role(ProofRole::KeyframeNode),
                    Some(NodeKind::FUNCTION),
                ),
            ],
        },
        ProofAtomSpec {
            id: ProofAtomId::C1,
            requirement: "css_animation_entrypoint",
            facts: &[
                edge_fact(
                    EdgeKind::MEMBER,
                    role(ProofRole::Entrypoint),
                    role(ProofRole::ImportStatement),
                    Some(NodeKind::MODULE),
                ),
                // The rev 5.2 fail-closed guard: matches only when a source
                // role is already bound, so an entrypoint subset evaluated
                // after a failed structure closure cannot bind Entrypoint
                // freely. Structural certainty exemption applies (IMPORT).
                edge_fact(
                    EdgeKind::IMPORT,
                    role(ProofRole::Entrypoint),
                    ProofEndpointPattern::AnyOfRoles(&[
                        ProofRole::VarsSource,
                        ProofRole::BaseSource,
                        ProofRole::AnimSource,
                    ]),
                    Some(NodeKind::FILE),
                ),
                ProofFactPattern::AnchoredLineContainment(AnchoredContainmentPattern {
                    kind: SourceAspectKind::VerifiedCarrierRange,
                    line_symbol: ProofRole::ImportStatement,
                    window_owner: ProofRole::Entrypoint,
                }),
            ],
        },
    ],
    distinct_roles: &[&[
        ProofRole::VarsSource,
        ProofRole::BaseSource,
        ProofRole::AnimSource,
    ]],
};

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::SearchHitOrigin;

    fn node(id: &str) -> NodeId {
        NodeId(id.to_string())
    }

    fn scan(root: &str, kinds: &[EdgeKind], depth: u32, truncated: bool) -> TrailCoverage {
        TrailCoverage::Scanned {
            root: node(root),
            traversal_kinds: kinds.to_vec(),
            direction: TrailDirection::Outgoing,
            depth,
            truncated,
        }
    }

    fn proved(outcome: FlowProofOutcome) -> VerifiedFlowProof {
        match outcome {
            FlowProofOutcome::Proved(proof) => proof,
            other => panic!("expected a proof, got {other:?}"),
        }
    }

    fn certain_call(
        edge: &str,
        source: &str,
        target: &str,
        callsite: Option<&str>,
    ) -> VerifiedTypedRelationReceipt {
        VerifiedTypedRelationReceipt {
            edge_id: EdgeId(edge.to_string()),
            kind: EdgeKind::CALL,
            source: node(source),
            target: node(target),
            target_kind: Some(NodeKind::METHOD),
            certainty: Some("certain".to_string()),
            callsite_identity: callsite.map(str::to_string),
            coverage: TrailCoverage::Unknown,
        }
    }

    fn structural(
        edge: &str,
        kind: EdgeKind,
        source: &str,
        target: &str,
        target_kind: Option<NodeKind>,
    ) -> VerifiedTypedRelationReceipt {
        VerifiedTypedRelationReceipt {
            edge_id: EdgeId(edge.to_string()),
            kind,
            source: node(source),
            target: node(target),
            target_kind,
            certainty: None,
            callsite_identity: None,
            coverage: TrailCoverage::Unknown,
        }
    }

    fn aspect(
        owner: &str,
        symbol: Option<&str>,
        start_line: u32,
        end_line: u32,
        atom_anchor: Option<ProofAtomId>,
    ) -> VerifiedSourceAspectReceipt {
        VerifiedSourceAspectReceipt {
            kind: SourceAspectKind::VerifiedCarrierRange,
            owner: node(owner),
            symbol_id: symbol.map(node),
            start_line: Some(start_line),
            end_line: Some(end_line),
            atom_anchor,
        }
    }

    const DISPATCH_CALLSITE: &str = "src/owner_file.ext:42:8:process_item|syntax:lang-call|receiver-owner:collection_item|receiver-binding:loop-element@40-44";
    const CONSTRUCTION_CALLSITE: &str = "src/owner_file.ext:12:4:make_record|syntax:lang-new";

    fn m_evidence() -> PacketProofEvidence {
        PacketProofEvidence {
            source_aspects: vec![aspect(
                "cite:flow-owner",
                Some("node:flow-owner"),
                10,
                60,
                None,
            )],
            typed_relations: vec![
                certain_call(
                    "edge:construction",
                    "node:flow-owner",
                    "node:record-type",
                    Some(CONSTRUCTION_CALLSITE),
                ),
                certain_call(
                    "edge:dispatch",
                    "node:flow-owner",
                    "node:element-method",
                    Some(DISPATCH_CALLSITE),
                ),
            ],
            trail_scans: Vec::new(),
        }
    }

    fn a_evidence() -> PacketProofEvidence {
        let mut type_usage = certain_call(
            "edge:config-usage",
            "node:builder-type",
            "node:config-type",
            None,
        );
        type_usage.kind = EdgeKind::TYPE_USAGE;
        type_usage.target_kind = Some(NodeKind::CLASS);
        PacketProofEvidence {
            source_aspects: vec![
                aspect("cite:config", Some("node:config-type"), 5, 30, None),
                aspect("cite:builder", Some("node:builder-type"), 40, 90, None),
            ],
            typed_relations: vec![
                type_usage,
                certain_call(
                    "edge:plan-call",
                    "node:plan-owner",
                    "node:build-method",
                    None,
                ),
                structural(
                    "edge:builder-member",
                    EdgeKind::MEMBER,
                    "node:builder-type",
                    "node:build-method",
                    Some(NodeKind::METHOD),
                ),
            ],
            trail_scans: Vec::new(),
        }
    }

    fn c_evidence() -> PacketProofEvidence {
        PacketProofEvidence {
            source_aspects: vec![
                // C1's anchored window: owned by the entrypoint citation,
                // reread for the MODULE-kind import statement declared at
                // line 3. Serves as both line carrier and window (rule 3(a)).
                VerifiedSourceAspectReceipt {
                    kind: SourceAspectKind::VerifiedCarrierRange,
                    owner: node("file:entry"),
                    symbol_id: Some(node("node:import-stmt")),
                    start_line: Some(3),
                    end_line: Some(8),
                    atom_anchor: Some(ProofAtomId::C1),
                },
                aspect(
                    "cite:vars",
                    Some("node:var-decl"),
                    12,
                    14,
                    Some(ProofAtomId::C2),
                ),
                aspect(
                    "cite:motion",
                    Some("node:keyframe-decl"),
                    20,
                    32,
                    Some(ProofAtomId::C4),
                ),
            ],
            typed_relations: vec![
                structural(
                    "edge:import-vars",
                    EdgeKind::IMPORT,
                    "file:entry",
                    "file:vars",
                    Some(NodeKind::FILE),
                ),
                structural(
                    "edge:import-base",
                    EdgeKind::IMPORT,
                    "file:entry",
                    "file:base",
                    Some(NodeKind::FILE),
                ),
                structural(
                    "edge:import-motion",
                    EdgeKind::IMPORT,
                    "file:entry",
                    "file:motion",
                    Some(NodeKind::FILE),
                ),
                structural(
                    "edge:entry-import-stmt",
                    EdgeKind::MEMBER,
                    "file:entry",
                    "node:import-stmt",
                    Some(NodeKind::MODULE),
                ),
                structural(
                    "edge:vars-var",
                    EdgeKind::MEMBER,
                    "file:vars",
                    "node:var-decl",
                    Some(NodeKind::VARIABLE),
                ),
                // The MEMBER witness that the base file's depth-2 structural
                // trail actually reached the selector (rule 7).
                structural(
                    "edge:base-selector",
                    EdgeKind::MEMBER,
                    "file:base",
                    "node:base-selector",
                    Some(NodeKind::CONSTANT),
                )
                .with_coverage(scan(
                    "file:base",
                    &[EdgeKind::MEMBER, EdgeKind::USAGE],
                    2,
                    false,
                )),
                structural(
                    "edge:selector-var-usage",
                    EdgeKind::USAGE,
                    "node:base-selector",
                    "node:var-decl",
                    Some(NodeKind::VARIABLE),
                ),
                structural(
                    "edge:motion-keyframe",
                    EdgeKind::MEMBER,
                    "file:motion",
                    "node:keyframe-decl",
                    Some(NodeKind::FUNCTION),
                ),
                structural(
                    "edge:motion-selector",
                    EdgeKind::MEMBER,
                    "file:motion",
                    "node:motion-selector",
                    Some(NodeKind::CONSTANT),
                ),
                structural(
                    "edge:selector-keyframe-usage",
                    EdgeKind::USAGE,
                    "node:motion-selector",
                    "node:keyframe-decl",
                    Some(NodeKind::FUNCTION),
                ),
            ],
            trail_scans: vec![scan(
                "file:base",
                &[EdgeKind::MEMBER, EdgeKind::USAGE],
                2,
                false,
            )],
        }
    }

    fn citation(node_id: &str, eligible: Option<bool>) -> AgentCitationDto {
        AgentCitationDto {
            node_id: node(node_id),
            display_name: "synthetic_owner".to_string(),
            kind: NodeKind::FUNCTION,
            file_path: Some("src/synthetic_owner.ext".to_string()),
            line: Some(10),
            score: 1.0,
            origin: SearchHitOrigin::IndexedSymbol,
            target: None,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: eligible,
            source_excerpt: None,
        }
    }

    fn source_range_unit(
        symbol: Option<&str>,
        snippet: Option<&str>,
        query: Option<&str>,
    ) -> SupportUnitDto {
        SupportUnitDto {
            id: "source:synthetic:10".to_string(),
            kind: SupportUnitKindDto::SourceRange,
            summary: "source for synthetic_owner".to_string(),
            path: Some("src/synthetic_owner.ext".to_string()),
            symbol_id: symbol.map(str::to_string),
            start_line: Some(10),
            end_line: Some(20),
            snippet: snippet.map(str::to_string),
            edge_kind: None,
            from_symbol: None,
            to_symbol: None,
            query: query.map(str::to_string),
        }
    }

    fn dispatch_with_callsite(callsite: Option<&str>) -> PacketProofEvidence {
        let mut evidence = m_evidence();
        evidence.typed_relations[1].callsite_identity = callsite.map(str::to_string);
        evidence
    }

    // ------------------------------------------------------------------
    // Negative tests (the stage-0 contract's negative-test seed, in order).
    // ------------------------------------------------------------------

    #[test]
    fn name_or_path_token_candidates_with_no_receipts_prove_nothing_for_any_flow() {
        let empty = PacketProofEvidence::default();
        assert_eq!(
            match_flow_proof(&LOG_HANDLER_FLOW_PROOF, &empty),
            FlowProofOutcome::Unproven
        );
        assert_eq!(
            match_flow_proof(&MAPPER_PLAN_FLOW_PROOF, &empty),
            FlowProofOutcome::Unproven
        );
        assert_eq!(
            match_flow_proof(&CSS_ANIMATION_FLOW_PROOF, &empty),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn m3_fails_without_certain_certainty_on_the_call_receipt() {
        for certainty in [None, Some("probable"), Some("uncertain")] {
            let mut evidence = m_evidence();
            evidence.typed_relations[1].certainty = certainty.map(str::to_string);
            assert_eq!(
                match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::M3], &evidence),
                FlowProofOutcome::Unproven,
                "certainty {certainty:?} must not discharge M3"
            );
        }
    }

    #[test]
    fn a1_fails_without_certain_certainty_on_the_type_usage_receipt() {
        for certainty in [None, Some("probable"), Some("uncertain")] {
            let mut evidence = a_evidence();
            evidence.typed_relations[0].certainty = certainty.map(str::to_string);
            assert_eq!(
                match_required_atoms(&MAPPER_PLAN_FLOW_PROOF, &[ProofAtomId::A1], &evidence),
                FlowProofOutcome::Unproven,
                "certainty {certainty:?} must not discharge A1"
            );
        }
    }

    #[test]
    fn m3_fails_without_a_syntax_call_marker_on_a_placeholder_shaped_edge() {
        // A placeholder-fallback edge carries no `syntax:{lang}-call` segment.
        let evidence = dispatch_with_callsite(Some(
            "src/owner_file.ext:42:8:process_item|receiver-owner:collection_item",
        ));
        assert_eq!(
            match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::M3], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn m3_fails_without_a_receiver_owner_marker() {
        let evidence = dispatch_with_callsite(Some(
            "src/owner_file.ext:42:8:process_item|syntax:lang-call",
        ));
        assert_eq!(
            match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::M3], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn m3_fails_on_a_self_call() {
        let mut evidence = m_evidence();
        evidence.typed_relations[1].target = node("node:flow-owner");
        assert_eq!(
            match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::M3], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn m3_fails_on_an_unresolved_call_target() {
        let mut evidence = m_evidence();
        evidence.typed_relations[1].target_kind = None;
        assert_eq!(
            match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::M3], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn m1b_fails_without_a_syntax_new_marker() {
        let mut evidence = m_evidence();
        evidence.typed_relations[0].callsite_identity =
            Some("src/owner_file.ext:12:4:make_record|syntax:lang-call".to_string());
        assert_eq!(
            match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::M1b], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn m2_fails_when_the_loop_marker_is_absent() {
        let evidence = dispatch_with_callsite(Some(
            "src/owner_file.ext:42:8:process_item|syntax:lang-call|receiver-owner:collection_item",
        ));
        assert_eq!(
            match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::M2], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn m2_fails_when_the_loop_marker_is_malformed() {
        for marker in [
            "receiver-binding:loop-element@40",
            "receiver-binding:loop-element@40-x",
            "receiver-binding:loop-element@-44",
            "receiver-binding:loop-element@44-40",
            "receiver-binding:loop-element@40-44-junk",
            "receiver-binding:loop-element@",
        ] {
            let identity = format!(
                "src/owner_file.ext:42:8:process_item|syntax:lang-call|receiver-owner:collection_item|{marker}"
            );
            let evidence = dispatch_with_callsite(Some(identity.as_str()));
            assert_eq!(
                match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::M2], &evidence),
                FlowProofOutcome::Unproven,
                "malformed loop marker {marker:?} must not discharge M2"
            );
        }
    }

    #[test]
    fn m2_fails_when_the_canonical_first_segment_fails_shape_validation() {
        for first_segment in [
            // Three fields instead of four.
            "src/owner_file.ext:42:process_item",
            // Non-numeric line.
            "src/owner_file.ext:x42:8:process_item",
            // Empty column field.
            "src/owner_file.ext:42::process_item",
            // Bare-marker identity: the first segment is not canonical.
            "syntax:lang-call",
        ] {
            let identity = format!(
                "{first_segment}|syntax:lang-call|receiver-owner:collection_item|receiver-binding:loop-element@40-44"
            );
            let evidence = dispatch_with_callsite(Some(identity.as_str()));
            assert_eq!(
                match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::M2], &evidence),
                FlowProofOutcome::Unproven,
                "malformed canonical segment {first_segment:?} must not discharge M2"
            );
        }
    }

    #[test]
    fn m2_fails_when_the_callsite_line_is_outside_the_loop_range() {
        let evidence = dispatch_with_callsite(Some(
            "src/owner_file.ext:45:8:process_item|syntax:lang-call|receiver-owner:collection_item|receiver-binding:loop-element@40-44",
        ));
        assert_eq!(
            match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::M2], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn m1a_and_m3_owner_mismatch_fails() {
        let mut evidence = m_evidence();
        evidence.source_aspects =
            vec![aspect("cite:other", Some("node:other-owner"), 10, 60, None)];
        assert_eq!(
            match_required_atoms(
                &LOG_HANDLER_FLOW_PROOF,
                &[ProofAtomId::M1a, ProofAtomId::M3],
                &evidence
            ),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn a_joins_fail_on_differing_node_ids_even_when_receipts_are_otherwise_identical() {
        // Receipts carry no display names at all, so two builders that would
        // share one are distinguishable only by node id — and must not join.
        let mut evidence = a_evidence();
        evidence.typed_relations[2].source = node("node:builder-type-sibling");
        assert_eq!(
            match_required_atoms(
                &MAPPER_PLAN_FLOW_PROOF,
                &[ProofAtomId::A1, ProofAtomId::A3],
                &evidence
            ),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn a_lone_type_usage_pair_without_an_owned_method_no_longer_proves_the_config() {
        // The measured defect, reduced: one certain TYPE_USAGE edge and a
        // reread range for its target. That is everything A1 and A2 ask for,
        // and on a real index thousands of pairs have exactly this shape.
        // A5 refuses it because the source owns no method.
        let mut lone_usage = certain_call(
            "edge:lone-usage",
            "node:parameter-owner",
            "node:parameter-type",
            None,
        );
        lone_usage.kind = EdgeKind::TYPE_USAGE;
        lone_usage.target_kind = Some(NodeKind::CLASS);
        let evidence = PacketProofEvidence {
            source_aspects: vec![aspect(
                "cite:param",
                Some("node:parameter-type"),
                2,
                4,
                None,
            )],
            typed_relations: vec![lone_usage],
            trail_scans: Vec::new(),
        };
        assert_eq!(
            match_required_atoms(
                &MAPPER_PLAN_FLOW_PROOF,
                &MAPPER_PLAN_FLOW_PROOF.atoms_for("mapper_config"),
                &evidence
            ),
            FlowProofOutcome::Unproven
        );
        // The failure is A5's and nothing else's: the pre-fix atom pair over
        // the identical receipts still discharges.
        proved(match_required_atoms(
            &MAPPER_PLAN_FLOW_PROOF,
            &[ProofAtomId::A1, ProofAtomId::A2],
            &evidence,
        ));
    }

    #[test]
    fn a_plan_builder_shaped_source_still_proves_the_config_requirement() {
        // Same two receipts as above plus the one membership fact that makes
        // the source a plan builder: a MEMBER edge onto a method it owns.
        // No name, path, count, or second join is involved — the method
        // identity is never bound.
        let mut usage = certain_call(
            "edge:builder-usage",
            "node:owning-type",
            "node:config-type",
            None,
        );
        usage.kind = EdgeKind::TYPE_USAGE;
        usage.target_kind = Some(NodeKind::CLASS);
        let evidence = PacketProofEvidence {
            source_aspects: vec![aspect("cite:config", Some("node:config-type"), 2, 4, None)],
            typed_relations: vec![
                usage,
                structural(
                    "edge:owned-method",
                    EdgeKind::MEMBER,
                    "node:owning-type",
                    "node:owned-method",
                    Some(NodeKind::METHOD),
                ),
            ],
            trail_scans: Vec::new(),
        };
        let proof = proved(match_required_atoms(
            &MAPPER_PLAN_FLOW_PROOF,
            &MAPPER_PLAN_FLOW_PROOF.atoms_for("mapper_config"),
            &evidence,
        ));
        assert_eq!(
            proof.bindings.get(&ProofRole::Builder),
            Some(&node("node:owning-type"))
        );
        assert_eq!(
            proof.bindings.get(&ProofRole::BuilderMethod),
            None,
            "A5's Any target must never bind the method role"
        );
    }

    #[test]
    fn a5_rejects_a_member_edge_onto_a_non_method_and_onto_another_type() {
        // Two ways the shape can be counterfeited: a member that is not a
        // method, and a method owned by some OTHER type. Both fail closed.
        let mut usage = certain_call(
            "edge:builder-usage",
            "node:owning-type",
            "node:config-type",
            None,
        );
        usage.kind = EdgeKind::TYPE_USAGE;
        usage.target_kind = Some(NodeKind::CLASS);
        let base = PacketProofEvidence {
            source_aspects: vec![aspect("cite:config", Some("node:config-type"), 2, 4, None)],
            typed_relations: vec![usage],
            trail_scans: Vec::new(),
        };
        let config_atoms = MAPPER_PLAN_FLOW_PROOF.atoms_for("mapper_config");

        let mut field_member = base.clone();
        field_member.typed_relations.push(structural(
            "edge:owned-field",
            EdgeKind::MEMBER,
            "node:owning-type",
            "node:owned-field",
            Some(NodeKind::FIELD),
        ));
        assert_eq!(
            match_required_atoms(&MAPPER_PLAN_FLOW_PROOF, &config_atoms, &field_member),
            FlowProofOutcome::Unproven
        );

        let mut foreign_member = base;
        foreign_member.typed_relations.push(structural(
            "edge:foreign-method",
            EdgeKind::MEMBER,
            "node:unrelated-type",
            "node:owned-method",
            Some(NodeKind::METHOD),
        ));
        assert_eq!(
            match_required_atoms(&MAPPER_PLAN_FLOW_PROOF, &config_atoms, &foreign_member),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn a3_fails_without_the_member_edge() {
        let mut evidence = a_evidence();
        evidence.typed_relations.remove(2);
        assert_eq!(
            match_required_atoms(&MAPPER_PLAN_FLOW_PROOF, &[ProofAtomId::A3], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn removing_any_single_a_receipt_leaves_its_requirement_unproven() {
        let config_atoms = MAPPER_PLAN_FLOW_PROOF.atoms_for("mapper_config");
        let execution_atoms = MAPPER_PLAN_FLOW_PROOF.atoms_for("mapper_execution");
        assert_eq!(
            config_atoms,
            vec![ProofAtomId::A1, ProofAtomId::A2, ProofAtomId::A5]
        );
        assert_eq!(execution_atoms, vec![ProofAtomId::A3, ProofAtomId::A4]);

        let mut without_type_usage = a_evidence();
        without_type_usage.typed_relations.remove(0);
        assert_eq!(
            match_required_atoms(&MAPPER_PLAN_FLOW_PROOF, &config_atoms, &without_type_usage),
            FlowProofOutcome::Unproven
        );

        let mut without_config_range = a_evidence();
        without_config_range.source_aspects.remove(0);
        assert_eq!(
            match_required_atoms(
                &MAPPER_PLAN_FLOW_PROOF,
                &config_atoms,
                &without_config_range
            ),
            FlowProofOutcome::Unproven
        );

        // A5's membership receipt: without it the Builder role has no
        // owned method, so mapper_config fails closed even though its
        // TYPE_USAGE pair and configuration range are both intact.
        let mut without_builder_member = a_evidence();
        without_builder_member.typed_relations.remove(2);
        assert_eq!(
            match_required_atoms(
                &MAPPER_PLAN_FLOW_PROOF,
                &config_atoms,
                &without_builder_member
            ),
            FlowProofOutcome::Unproven
        );

        let mut without_plan_call = a_evidence();
        without_plan_call.typed_relations.remove(1);
        assert_eq!(
            match_required_atoms(
                &MAPPER_PLAN_FLOW_PROOF,
                &execution_atoms,
                &without_plan_call
            ),
            FlowProofOutcome::Unproven
        );

        let mut without_builder_range = a_evidence();
        without_builder_range.source_aspects.remove(1);
        assert_eq!(
            match_required_atoms(
                &MAPPER_PLAN_FLOW_PROOF,
                &execution_atoms,
                &without_builder_range
            ),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn segment_values_never_unify_roles_across_receipts() {
        // The carrier range names one node; the only dispatch-shaped edge
        // hangs off another. Both callsites share the identical
        // `receiver-owner:` value — which must not stand in for the missing
        // node-id join.
        let mut evidence = m_evidence();
        evidence.source_aspects =
            vec![aspect("cite:other", Some("node:other-owner"), 10, 60, None)];
        evidence.typed_relations.push(VerifiedTypedRelationReceipt {
            edge_id: EdgeId("edge:other-call".to_string()),
            kind: EdgeKind::CALL,
            source: node("node:other-owner"),
            target: node("node:element-method"),
            target_kind: Some(NodeKind::METHOD),
            certainty: Some("certain".to_string()),
            // Shares the receiver-owner VALUE but lacks the syntax marker.
            callsite_identity: Some(
                "src/other_file.ext:9:2:process_item|receiver-owner:collection_item".to_string(),
            ),
            coverage: TrailCoverage::Unknown,
        });
        assert_eq!(
            match_required_atoms(
                &LOG_HANDLER_FLOW_PROOF,
                &[ProofAtomId::M1a, ProofAtomId::M3],
                &evidence
            ),
            FlowProofOutcome::Unproven
        );

        // The mirrored positive: with the carrier range on the dispatching
        // node id, the same evidence discharges — node identity, never the
        // segment value, is what joins.
        let mut joined = evidence;
        joined.source_aspects = vec![aspect(
            "cite:flow-owner",
            Some("node:flow-owner"),
            10,
            60,
            None,
        )];
        proved(match_required_atoms(
            &LOG_HANDLER_FLOW_PROOF,
            &[ProofAtomId::M1a, ProofAtomId::M3],
            &joined,
        ));
    }

    #[test]
    fn c2_fails_for_a_non_imported_aspect_file() {
        let mut evidence = c_evidence();
        evidence
            .typed_relations
            .retain(|receipt| receipt.edge_id.0 != "edge:import-vars");
        assert_eq!(
            match_required_atoms(&CSS_ANIMATION_FLOW_PROOF, &[ProofAtomId::C2], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn pairwise_equal_source_file_bindings_fail_the_css_formula() {
        // Rebase every C3 receipt onto the vars file so BaseSource can only
        // bind the same id as VarsSource; distinctness must then fail the
        // whole formula.
        let mut evidence = c_evidence();
        evidence
            .typed_relations
            .retain(|receipt| receipt.edge_id.0 != "edge:import-base");
        for receipt in &mut evidence.typed_relations {
            if receipt.edge_id.0 == "edge:base-selector" {
                receipt.source = node("file:vars");
                receipt.coverage =
                    scan("file:vars", &[EdgeKind::MEMBER, EdgeKind::USAGE], 2, false);
            }
        }
        evidence.trail_scans = vec![scan(
            "file:vars",
            &[EdgeKind::MEMBER, EdgeKind::USAGE],
            2,
            false,
        )];
        assert_eq!(
            match_flow_proof(&CSS_ANIMATION_FLOW_PROOF, &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn an_unresolved_var_usage_target_never_discharges_c3() {
        // A usage-minted custom-property node cannot exist (P3.ii), so an
        // unresolved usage target surfaces as a missing target kind — and
        // must not discharge.
        let mut evidence = c_evidence();
        for receipt in &mut evidence.typed_relations {
            if receipt.edge_id.0 == "edge:selector-var-usage" {
                receipt.target_kind = None;
            }
        }
        assert_eq!(
            match_required_atoms(
                &CSS_ANIMATION_FLOW_PROOF,
                &[ProofAtomId::C2, ProofAtomId::C3],
                &evidence
            ),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn a_base_selector_with_any_function_kind_usage_fails_c3() {
        // The sibling-animation-file shape: a selector that also uses a
        // keyframe (FUNCTION-kind) node cannot bind the base-selector role.
        let mut evidence = c_evidence();
        evidence.typed_relations.push(structural(
            "edge:selector-keyframe-refutation",
            EdgeKind::USAGE,
            "node:base-selector",
            "node:keyframe-decl",
            Some(NodeKind::FUNCTION),
        ));
        assert_eq!(
            match_required_atoms(
                &CSS_ANIMATION_FLOW_PROOF,
                &[ProofAtomId::C2, ProofAtomId::C3],
                &evidence
            ),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn a_truncated_outgoing_usage_scan_fails_the_c3_absence_clause_closed() {
        let mut evidence = c_evidence();
        evidence.trail_scans = vec![scan(
            "file:base",
            &[EdgeKind::MEMBER, EdgeKind::USAGE],
            2,
            true,
        )];
        assert_eq!(
            match_required_atoms(
                &CSS_ANIMATION_FLOW_PROOF,
                &[ProofAtomId::C2, ProofAtomId::C3],
                &evidence
            ),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn a_usage_only_scan_with_an_external_member_witness_fails_the_c3_absence_clause() {
        // The F1 finding-1 shape: a USAGE-only depth-2 trail rooted at the
        // base file enumerates nothing (file nodes have no outgoing USAGE),
        // reports untruncated, and the MEMBER witness comes from a different
        // trail. MEMBER must be among the covering record's OWN traversal
        // kinds, so this fails closed.
        let mut evidence = c_evidence();
        evidence.trail_scans = vec![scan("file:base", &[EdgeKind::USAGE], 2, false)];
        assert_eq!(
            match_required_atoms(
                &CSS_ANIMATION_FLOW_PROOF,
                &[ProofAtomId::C2, ProofAtomId::C3],
                &evidence
            ),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn unknown_coverage_never_satisfies_an_absence_clause() {
        // The conversion-default Unknown on the MEMBER witness receipt makes
        // the deeper-rooted covering arm fail closed.
        let mut unknown_witness = c_evidence();
        for receipt in &mut unknown_witness.typed_relations {
            if receipt.edge_id.0 == "edge:base-selector" {
                receipt.coverage = TrailCoverage::Unknown;
            }
        }
        assert_eq!(
            match_required_atoms(
                &CSS_ANIMATION_FLOW_PROOF,
                &[ProofAtomId::C2, ProofAtomId::C3],
                &unknown_witness
            ),
            FlowProofOutcome::Unproven
        );

        // An Unknown entry in the scan list never serves as a covering scan.
        let mut unknown_scan = c_evidence();
        unknown_scan.trail_scans = vec![TrailCoverage::Unknown];
        assert_eq!(
            match_required_atoms(
                &CSS_ANIMATION_FLOW_PROOF,
                &[ProofAtomId::C2, ProofAtomId::C3],
                &unknown_scan
            ),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn a_keyframe_usage_into_a_non_imported_file_fails_c4() {
        let mut evidence = c_evidence();
        evidence
            .typed_relations
            .retain(|receipt| receipt.edge_id.0 != "edge:import-motion");
        assert_eq!(
            match_required_atoms(&CSS_ANIMATION_FLOW_PROOF, &[ProofAtomId::C4], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn containment_never_discharges_against_a_window_lacking_the_atom_anchor_mark() {
        // A prompt-chosen window carries no atom anchor: rule 3(b) rejects it.
        let mut unanchored = c_evidence();
        unanchored.source_aspects[0].atom_anchor = None;
        assert_eq!(
            match_flow_proof(&CSS_ANIMATION_FLOW_PROOF, &unanchored),
            FlowProofOutcome::Unproven
        );

        // A window anchored for a DIFFERENT atom does not satisfy C1 either.
        let mut misanchored = c_evidence();
        misanchored.source_aspects[0].atom_anchor = Some(ProofAtomId::C2);
        assert_eq!(
            match_flow_proof(&CSS_ANIMATION_FLOW_PROOF, &misanchored),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn c1_containment_rejects_a_line_carrier_of_the_wrong_aspect_kind() {
        // The line-carrying receipt must have the aspect kind the atom names
        // (VerifiedCarrierRange for C1); the reserved ConstructionSite
        // classification never stands in for it.
        let mut evidence = c_evidence();
        evidence.source_aspects[0].kind = SourceAspectKind::ConstructionSite;
        assert_eq!(
            match_flow_proof(&CSS_ANIMATION_FLOW_PROOF, &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn c1_requires_the_anchored_line_to_belong_to_a_module_kind_member() {
        // Contract rev 5.1: the covered declaration line must belong to a
        // MODULE-kind member of the entrypoint. A CONSTANT-kind member's line
        // no longer satisfies C1.
        let mut evidence = c_evidence();
        for receipt in &mut evidence.typed_relations {
            if receipt.edge_id.0 == "edge:entry-import-stmt" {
                receipt.target_kind = Some(NodeKind::CONSTANT);
            }
        }
        assert_eq!(
            match_flow_proof(&CSS_ANIMATION_FLOW_PROOF, &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn legacy_specs_carry_no_formula_so_no_atom_receipt_can_exist() {
        // Legacy preserves today's discharge path: there is no formula to
        // match, so no atom verification and no atom receipt can ever be
        // produced for a Legacy-bearing requirement. Nothing in this module
        // serializes, so the public packet JSON shape is untouched by
        // construction.
        assert!(FlowProofSpec::Legacy.formula().is_none());
        assert!(
            FlowProofSpec::Atoms(&LOG_HANDLER_FLOW_PROOF)
                .formula()
                .is_some()
        );
    }

    #[test]
    fn structural_member_receipts_discharge_without_certainty_evaluation() {
        // Rule 6 excludes structural edges from the certainty gate; their
        // admissibility is the effective target's node kind.
        let mut evidence = a_evidence();
        evidence.typed_relations[2].certainty = Some("uncertain".to_string());
        proved(match_required_atoms(
            &MAPPER_PLAN_FLOW_PROOF,
            &[ProofAtomId::A3],
            &evidence,
        ));
    }

    #[test]
    fn a_construction_site_receipt_is_representable_and_gated_out_of_carrier_range_atoms() {
        // The reserved P1b classification stays representable, and every
        // shipped carrier-range pattern rejects it.
        let mut evidence = m_evidence();
        evidence.source_aspects[0].kind = SourceAspectKind::ConstructionSite;
        assert_eq!(
            match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::M1a], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    #[test]
    fn the_matcher_reports_aborted_at_the_step_bound_instead_of_searching_unboundedly() {
        // Decoy carrier ranges and construction edges that never join force
        // the search over the step limit before it can reach the one viable
        // pair, which sits last in input order. The abort is observable and
        // fail-closed.
        let mut flooded = PacketProofEvidence::default();
        for index in 0..200 {
            flooded.source_aspects.push(aspect(
                &format!("cite:decoy-{index}"),
                Some(&format!("node:decoy-owner-{index}")),
                1,
                5,
                None,
            ));
        }
        for index in 0..100 {
            flooded.typed_relations.push(certain_call(
                &format!("edge:decoy-{index}"),
                "node:unjoined",
                &format!("node:decoy-target-{index}"),
                Some(CONSTRUCTION_CALLSITE),
            ));
        }
        flooded.source_aspects.push(aspect(
            "cite:flow-owner",
            Some("node:flow-owner"),
            10,
            60,
            None,
        ));
        flooded.typed_relations.push(certain_call(
            "edge:construction",
            "node:flow-owner",
            "node:record-type",
            Some(CONSTRUCTION_CALLSITE),
        ));
        assert_eq!(
            match_required_atoms(
                &LOG_HANDLER_FLOW_PROOF,
                &[ProofAtomId::M1a, ProofAtomId::M1b],
                &flooded
            ),
            FlowProofOutcome::Aborted
        );
        // The per-requirement pass reports the abort on the requirement that
        // hit it, retires and propagates nothing from it, and is unchanged by
        // the constraint-strength evaluation order.
        assert_eq!(
            match_flow_requirements(&LOG_HANDLER_FLOW_PROOF, &flooded),
            vec![
                ("logger_event", FlowProofOutcome::Aborted),
                ("handler_processing", FlowProofOutcome::Unproven),
            ]
        );
        assert_eq!(
            match_flow_requirements(&LOG_HANDLER_FLOW_PROOF, &flooded),
            declaration_order_requirements(&LOG_HANDLER_FLOW_PROOF, &flooded)
        );

        // The same shape below the bound discharges, so the failure above is
        // the cap, not the formula.
        let mut small = PacketProofEvidence::default();
        for index in 0..3 {
            small.source_aspects.push(aspect(
                &format!("cite:decoy-{index}"),
                Some(&format!("node:decoy-owner-{index}")),
                1,
                5,
                None,
            ));
        }
        small.source_aspects.push(aspect(
            "cite:flow-owner",
            Some("node:flow-owner"),
            10,
            60,
            None,
        ));
        small.typed_relations.push(certain_call(
            "edge:construction",
            "node:flow-owner",
            "node:record-type",
            Some(CONSTRUCTION_CALLSITE),
        ));
        proved(match_required_atoms(
            &LOG_HANDLER_FLOW_PROOF,
            &[ProofAtomId::M1a, ProofAtomId::M1b],
            &small,
        ));
    }

    #[test]
    fn atom_filters_fail_closed_when_empty_or_unknown() {
        let evidence = m_evidence();
        assert_eq!(
            match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[], &evidence),
            FlowProofOutcome::Unproven
        );
        assert_eq!(
            match_required_atoms(&LOG_HANDLER_FLOW_PROOF, &[ProofAtomId::A1], &evidence),
            FlowProofOutcome::Unproven
        );
    }

    // ------------------------------------------------------------------
    // Positive discharge shapes.
    // ------------------------------------------------------------------

    #[test]
    fn m_formula_discharges_construction_loop_and_dispatch_receipts() {
        let proof = proved(match_flow_proof(&LOG_HANDLER_FLOW_PROOF, &m_evidence()));
        assert_eq!(
            proof.bindings.get(&ProofRole::FlowOwner),
            Some(&node("node:flow-owner"))
        );
        assert_eq!(
            proof
                .atoms
                .iter()
                .map(|atom| (atom.atom, atom.requirement))
                .collect::<Vec<_>>(),
            vec![
                (ProofAtomId::M1a, "logger_event"),
                (ProofAtomId::M1b, "logger_event"),
                (ProofAtomId::M2, "handler_processing"),
                (ProofAtomId::M3, "handler_processing"),
            ]
        );
        // Rule 8: the single dispatch edge discharges both M2 and M3, each
        // against its own fully matched pattern.
        let dispatch_edge = DischargedFact::TypedRelation {
            edge_id: EdgeId("edge:dispatch".to_string()),
            source: node("node:flow-owner"),
            target: node("node:element-method"),
        };
        assert_eq!(proof.atoms[2].facts, vec![dispatch_edge.clone()]);
        assert_eq!(proof.atoms[3].facts, vec![dispatch_edge]);
    }

    #[test]
    fn a_formula_discharges_on_node_identity_joins_including_the_member_edge() {
        let proof = proved(match_flow_proof(&MAPPER_PLAN_FLOW_PROOF, &a_evidence()));
        assert_eq!(
            proof.bindings.get(&ProofRole::Builder),
            Some(&node("node:builder-type"))
        );
        assert_eq!(
            proof.bindings.get(&ProofRole::ConfigType),
            Some(&node("node:config-type"))
        );
        assert_eq!(
            proof.bindings.get(&ProofRole::PlanOwner),
            Some(&node("node:plan-owner"))
        );
        assert_eq!(
            proof.bindings.get(&ProofRole::BuilderMethod),
            Some(&node("node:build-method"))
        );
        assert_eq!(
            proof
                .atoms
                .iter()
                .map(|atom| (atom.atom, atom.requirement))
                .collect::<Vec<_>>(),
            vec![
                (ProofAtomId::A1, "mapper_config"),
                (ProofAtomId::A2, "mapper_config"),
                (ProofAtomId::A5, "mapper_config"),
                (ProofAtomId::A3, "mapper_execution"),
                (ProofAtomId::A4, "mapper_execution"),
            ]
        );
        let builder_member = DischargedFact::TypedRelation {
            edge_id: EdgeId("edge:builder-member".to_string()),
            source: node("node:builder-type"),
            target: node("node:build-method"),
        };
        // Rule 8: the one membership receipt discharges A5's admissibility
        // fact and A3's join, each against its own fully matched pattern.
        assert_eq!(proof.atoms[2].facts, vec![builder_member.clone()]);
        assert_eq!(proof.atoms[3].facts[1], builder_member);
    }

    #[test]
    fn c_formula_discharges_imports_members_usages_and_the_covering_absence_scan() {
        let proof = proved(match_flow_proof(&CSS_ANIMATION_FLOW_PROOF, &c_evidence()));
        assert_eq!(
            proof.bindings.get(&ProofRole::Entrypoint),
            Some(&node("file:entry"))
        );
        assert_eq!(
            proof.bindings.get(&ProofRole::KeyframeNode),
            Some(&node("node:keyframe-decl"))
        );
        // C3's absence clause names the covering untruncated scan it used.
        assert_eq!(
            proof.atoms[1].facts[3],
            DischargedFact::CoveredAbsence {
                root: node("file:base"),
                edge_kind: EdgeKind::USAGE,
                depth: 2,
            }
        );
        // C1 discharged with the anchored window covering the MODULE-kind
        // statement's declaration line (facts: MEMBER, the bound-roles
        // IMPORT guard, then the containment).
        assert_eq!(
            proof.atoms[3].facts[2],
            DischargedFact::AnchoredLineContainment {
                line: 3,
                window_owner: node("file:entry"),
                window_start_line: 3,
                window_end_line: 8,
            }
        );
    }

    // ------------------------------------------------------------------
    // Per-requirement matching semantics.
    // ------------------------------------------------------------------

    #[test]
    fn per_requirement_matching_returns_every_requirement_proven_when_the_full_group_matches() {
        let outcomes = match_flow_requirements(&LOG_HANDLER_FLOW_PROOF, &m_evidence());
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].0, "logger_event");
        assert_eq!(outcomes[1].0, "handler_processing");
        let logger = proved(outcomes[0].1.clone());
        let handler = proved(outcomes[1].1.clone());
        // One group-wide assignment: both requirements share the bindings.
        assert_eq!(logger.bindings, handler.bindings);
        assert_eq!(
            logger
                .atoms
                .iter()
                .map(|atom| atom.atom)
                .collect::<Vec<_>>(),
            vec![ProofAtomId::M1a, ProofAtomId::M1b]
        );
        assert_eq!(
            handler
                .atoms
                .iter()
                .map(|atom| atom.atom)
                .collect::<Vec<_>>(),
            vec![ProofAtomId::M2, ProofAtomId::M3]
        );
    }

    #[test]
    fn per_requirement_matching_carries_role_bindings_across_requirement_subsets() {
        // logger_event's receipts hang off one owner, the dispatch edge off
        // another, so the full group cannot match.
        let evidence = PacketProofEvidence {
            source_aspects: vec![aspect("cite:owner-x", Some("node:owner-x"), 10, 60, None)],
            typed_relations: vec![
                certain_call(
                    "edge:construction-x",
                    "node:owner-x",
                    "node:record-type",
                    Some(CONSTRUCTION_CALLSITE),
                ),
                certain_call(
                    "edge:dispatch-y",
                    "node:owner-y",
                    "node:element-method",
                    Some(DISPATCH_CALLSITE),
                ),
            ],
            trail_scans: Vec::new(),
        };
        assert_eq!(
            match_flow_proof(&LOG_HANDLER_FLOW_PROOF, &evidence),
            FlowProofOutcome::Unproven
        );
        // Standalone, logger_event would prove with FlowOwner bound to its
        // own owner...
        let standalone = proved(match_required_atoms(
            &LOG_HANDLER_FLOW_PROOF,
            &[ProofAtomId::M1a, ProofAtomId::M1b],
            &evidence,
        ));
        assert_eq!(
            standalone.bindings.get(&ProofRole::FlowOwner),
            Some(&node("node:owner-x"))
        );
        // ...but handler_processing is the more constrained requirement, so
        // the fallback runs it first and carries ITS FlowOwner binding into
        // the sibling subset, which then fails: verdicts never fork the
        // assignment.
        assert_eq!(
            LOG_HANDLER_FLOW_PROOF.requirements_by_constraint_strength(),
            vec!["handler_processing", "logger_event"]
        );
        let outcomes = match_flow_requirements(&LOG_HANDLER_FLOW_PROOF, &evidence);
        assert_eq!(outcomes[0], ("logger_event", FlowProofOutcome::Unproven));
        assert_eq!(outcomes[1].0, "handler_processing");
        let handler = proved(outcomes[1].1.clone());
        assert_eq!(
            handler.bindings.get(&ProofRole::FlowOwner),
            Some(&node("node:owner-y"))
        );
    }

    #[test]
    fn css_entrypoint_follows_the_structure_closure_through_the_per_requirement_split() {
        // C1's bound-roles-only IMPORT guard is only a guard while structure
        // is evaluated FIRST. Constraint strength must keep it there: nine
        // typed relations across C2-C4 against C1's two.
        assert_eq!(
            CSS_ANIMATION_FLOW_PROOF.requirements_by_constraint_strength(),
            vec!["css_animation_structure", "css_animation_entrypoint"],
            "the entrypoint guard fails closed only when structure binds the source roles first"
        );

        // Structure succeeds: css_animation_entrypoint is proven under the
        // same assignment and reports the carried Entrypoint binding.
        let outcomes = match_flow_requirements(&CSS_ANIMATION_FLOW_PROOF, &c_evidence());
        assert_eq!(outcomes[0].0, "css_animation_structure");
        assert_eq!(outcomes[1].0, "css_animation_entrypoint");
        proved(outcomes[0].1.clone());
        let entrypoint = proved(outcomes[1].1.clone());
        assert_eq!(
            entrypoint.bindings.get(&ProofRole::Entrypoint),
            Some(&node("file:entry"))
        );
        assert_eq!(
            entrypoint
                .atoms
                .iter()
                .map(|atom| atom.atom)
                .collect::<Vec<_>>(),
            vec![ProofAtomId::C1]
        );

        // Structure fails (truncated absence scan): the entrypoint subset
        // then evaluates with no carried bindings, and C1's bound-roles-only
        // IMPORT guard fails it closed even though its MODULE-kind member
        // and anchored window receipts are all present (the rev 5.2
        // false-positive shape).
        let mut broken_structure = c_evidence();
        broken_structure.trail_scans = vec![scan(
            "file:base",
            &[EdgeKind::MEMBER, EdgeKind::USAGE],
            2,
            true,
        )];
        let outcomes = match_flow_requirements(&CSS_ANIMATION_FLOW_PROOF, &broken_structure);
        assert_eq!(
            outcomes,
            vec![
                ("css_animation_structure", FlowProofOutcome::Unproven),
                ("css_animation_entrypoint", FlowProofOutcome::Unproven),
            ]
        );
    }

    #[test]
    fn per_requirement_matching_reports_one_requirement_proven_while_its_sibling_fails() {
        assert_eq!(
            MAPPER_PLAN_FLOW_PROOF.atoms_for("mapper_execution"),
            vec![ProofAtomId::A3, ProofAtomId::A4]
        );
        let mut evidence = a_evidence();
        // Remove A4's builder carrier range: mapper_execution — which
        // constraint strength runs FIRST — fails, and because a failed
        // subset carries nothing forward, mapper_config still proves on its
        // own bindings even though its sibling bound PlanOwner, Builder and
        // BuilderMethod on the way to failing. (Removing the MEMBER receipt
        // instead would now fail BOTH requirements, since A5 reads it too.)
        evidence.source_aspects.remove(1);
        let outcomes = match_flow_requirements(&MAPPER_PLAN_FLOW_PROOF, &evidence);
        assert_eq!(outcomes[0].0, "mapper_config");
        let config = proved(outcomes[0].1.clone());
        assert_eq!(
            config.bindings.get(&ProofRole::Builder),
            Some(&node("node:builder-type"))
        );
        assert_eq!(
            config.bindings.get(&ProofRole::ConfigType),
            Some(&node("node:config-type"))
        );
        assert_eq!(
            config.bindings.get(&ProofRole::PlanOwner),
            None,
            "the failed sibling's partial bindings must not leak into the proof"
        );
        assert_eq!(
            outcomes[1],
            ("mapper_execution", FlowProofOutcome::Unproven)
        );
    }

    // ------------------------------------------------------------------
    // Constraint-strength ordering of the per-requirement fallback.
    // ------------------------------------------------------------------

    /// The pre-fix fallback, reproduced verbatim as an equivalence oracle:
    /// full group first, then each requirement's subset in DECLARATION order
    /// with bindings carried from every earlier successful subset.
    fn declaration_order_requirements(
        formula: &FlowProofFormula,
        evidence: &PacketProofEvidence,
    ) -> Vec<(&'static str, FlowProofOutcome)> {
        let requirements = formula.requirements();
        if let FlowProofOutcome::Proved(proof) = match_flow_proof(formula, evidence) {
            return requirements
                .into_iter()
                .map(|requirement| {
                    let atoms = proof
                        .atoms
                        .iter()
                        .filter(|atom| atom.requirement == requirement)
                        .cloned()
                        .collect();
                    (
                        requirement,
                        FlowProofOutcome::Proved(VerifiedFlowProof {
                            bindings: proof.bindings.clone(),
                            atoms,
                        }),
                    )
                })
                .collect();
        }
        let mut carried = BTreeMap::new();
        let mut outcomes = Vec::new();
        for requirement in requirements {
            let outcome = match_atoms_with_bindings(
                formula,
                &formula.atoms_for(requirement),
                evidence,
                carried.clone(),
            );
            if let FlowProofOutcome::Proved(proof) = &outcome {
                carried = proof.bindings.clone();
            }
            outcomes.push((requirement, outcome));
        }
        outcomes
    }

    /// Two requirements over DISJOINT roles, declared weakest-first so the
    /// strength ordering really does reorder them. With no role shared,
    /// neither subset can constrain the other, so order cannot matter.
    const DISJOINT_ROLE_FORMULA: FlowProofFormula = FlowProofFormula {
        atoms: &[
            ProofAtomSpec {
                id: ProofAtomId::M1a,
                requirement: "logger_event",
                facts: &[carrier_range_fact(ProofRole::FlowOwner, false)],
            },
            ProofAtomSpec {
                id: ProofAtomId::A3,
                requirement: "mapper_execution",
                facts: &[
                    edge_fact(
                        EdgeKind::CALL,
                        role(ProofRole::PlanOwner),
                        role(ProofRole::BuilderMethod),
                        Some(NodeKind::METHOD),
                    ),
                    edge_fact(
                        EdgeKind::MEMBER,
                        role(ProofRole::Builder),
                        role(ProofRole::BuilderMethod),
                        Some(NodeKind::METHOD),
                    ),
                ],
            },
        ],
        distinct_roles: &[],
    };

    #[test]
    fn constraint_strength_orders_every_shipped_formula_most_constrained_first() {
        // M: handler_processing carries two typed relations (M2, M3);
        // logger_event carries one plus a carrier range.
        assert_eq!(
            LOG_HANDLER_FLOW_PROOF.requirements(),
            vec!["logger_event", "handler_processing"]
        );
        assert_eq!(
            LOG_HANDLER_FLOW_PROOF.requirements_by_constraint_strength(),
            vec!["handler_processing", "logger_event"]
        );
        assert_eq!(
            LOG_HANDLER_FLOW_PROOF.constraint_strength("handler_processing"),
            RequirementConstraintStrength {
                typed_relation_facts: 2,
                bound_role_positions: 1,
                total_facts: 2,
                declaration_index: 1,
            }
        );
        assert_eq!(
            LOG_HANDLER_FLOW_PROOF.constraint_strength("logger_event"),
            RequirementConstraintStrength {
                typed_relation_facts: 1,
                bound_role_positions: 1,
                total_facts: 2,
                declaration_index: 0,
            }
        );

        // A: mapper_execution carries A3's CALL plus its MEMBER join and
        // binds three roles; mapper_config carries the same number of typed
        // relations (A1's TYPE_USAGE and A5's membership constraint) but
        // binds one role fewer, so execution still leads.
        assert_eq!(
            MAPPER_PLAN_FLOW_PROOF.requirements(),
            vec!["mapper_config", "mapper_execution"]
        );
        assert_eq!(
            MAPPER_PLAN_FLOW_PROOF.requirements_by_constraint_strength(),
            vec!["mapper_execution", "mapper_config"]
        );
        assert_eq!(
            MAPPER_PLAN_FLOW_PROOF.constraint_strength("mapper_execution"),
            RequirementConstraintStrength {
                typed_relation_facts: 2,
                bound_role_positions: 3,
                total_facts: 3,
                declaration_index: 1,
            }
        );
        assert_eq!(
            MAPPER_PLAN_FLOW_PROOF.constraint_strength("mapper_config"),
            RequirementConstraintStrength {
                typed_relation_facts: 2,
                bound_role_positions: 2,
                total_facts: 3,
                declaration_index: 0,
            }
        );

        // C: structure keeps its declared lead, which is what makes C1's
        // bound-roles-only IMPORT endpoint a guard.
        assert_eq!(
            CSS_ANIMATION_FLOW_PROOF.requirements(),
            vec!["css_animation_structure", "css_animation_entrypoint"]
        );
        assert_eq!(
            CSS_ANIMATION_FLOW_PROOF.requirements_by_constraint_strength(),
            vec!["css_animation_structure", "css_animation_entrypoint"]
        );
        assert_eq!(
            CSS_ANIMATION_FLOW_PROOF.constraint_strength("css_animation_structure"),
            RequirementConstraintStrength {
                typed_relation_facts: 9,
                bound_role_positions: 8,
                total_facts: 12,
                declaration_index: 0,
            }
        );
        assert_eq!(
            CSS_ANIMATION_FLOW_PROOF.constraint_strength("css_animation_entrypoint"),
            RequirementConstraintStrength {
                // C1's AnyOfRoles IMPORT endpoint is a typed relation but
                // binds nothing, and its containment fact counts only in the
                // total.
                typed_relation_facts: 2,
                bound_role_positions: 2,
                total_facts: 3,
                declaration_index: 1,
            }
        );
    }

    #[test]
    fn constraint_strength_ordering_is_total_stable_and_evidence_independent() {
        for formula in [
            &LOG_HANDLER_FLOW_PROOF,
            &MAPPER_PLAN_FLOW_PROOF,
            &CSS_ANIMATION_FLOW_PROOF,
            &DISJOINT_ROLE_FORMULA,
        ] {
            let order = formula.requirements_by_constraint_strength();
            // A permutation of the declaration order: nothing is dropped or
            // duplicated.
            let mut sorted_declaration = formula.requirements();
            sorted_declaration.sort_unstable();
            let mut sorted_order = order.clone();
            sorted_order.sort_unstable();
            assert_eq!(sorted_declaration, sorted_order);
            // Stable: recomputing yields the identical sequence. The
            // computation takes no evidence argument at all, which is the
            // structural proof that it cannot vary per packet.
            assert_eq!(order, formula.requirements_by_constraint_strength());
            // Total: no two requirements tie on the full key, and the keys
            // are strictly increasing along the evaluation order.
            let keys = order
                .iter()
                .map(|requirement| formula.constraint_strength(requirement).ordering_key())
                .collect::<Vec<_>>();
            assert!(
                keys.windows(2).all(|pair| pair[0] < pair[1]),
                "{formula:?} ordering is not strict: {keys:?}"
            );
        }
    }

    #[test]
    fn the_weakly_constrained_requirement_no_longer_captures_a_shared_role() {
        // The live defect's shape: a lone TYPE_USAGE pair unrelated to the
        // plan chain (a bare parameter type, of which a real index has
        // thousands) sits beside the true plan-builder chain. Both A1 and A2
        // match it, and Builder/ConfigType are shared with mapper_execution.
        let mut decoy_usage = certain_call(
            "edge:decoy-usage",
            "node:decoy-owner",
            "node:decoy-config",
            None,
        );
        decoy_usage.kind = EdgeKind::TYPE_USAGE;
        decoy_usage.target_kind = Some(NodeKind::CLASS);
        let evidence = PacketProofEvidence {
            source_aspects: vec![
                aspect("cite:decoy", Some("node:decoy-config"), 2, 4, None),
                aspect("cite:builder", Some("node:plan-builder"), 40, 90, None),
            ],
            typed_relations: vec![
                decoy_usage,
                certain_call(
                    "edge:plan-call",
                    "node:plan-owner",
                    "node:build-method",
                    None,
                ),
                structural(
                    "edge:builder-member",
                    EdgeKind::MEMBER,
                    "node:plan-builder",
                    "node:build-method",
                    Some(NodeKind::METHOD),
                ),
            ],
            trail_scans: Vec::new(),
        };
        // Nothing carries a TYPE_USAGE out of the real plan builder, so the
        // full group cannot close and the per-requirement fallback runs.
        assert_eq!(
            match_flow_proof(&MAPPER_PLAN_FLOW_PROOF, &evidence),
            FlowProofOutcome::Unproven
        );

        // The pre-fix ATOM PAIR still matches the decoy — the ordering fix
        // never made A1/A2 discriminate — and the bindings it produces
        // starve the sibling.
        let decoy_first = proved(match_required_atoms(
            &MAPPER_PLAN_FLOW_PROOF,
            &[ProofAtomId::A1, ProofAtomId::A2],
            &evidence,
        ));
        assert_eq!(
            decoy_first.bindings.get(&ProofRole::Builder),
            Some(&node("node:decoy-owner"))
        );
        assert_eq!(
            match_atoms_with_bindings(
                &MAPPER_PLAN_FLOW_PROOF,
                &[ProofAtomId::A3, ProofAtomId::A4],
                &evidence,
                decoy_first.bindings.clone(),
            ),
            FlowProofOutcome::Unproven
        );
        // A5 is what removes the decoy from the requirement's reach: the
        // decoy source owns no method, so even the pre-fix DECLARATION order
        // — which evaluates mapper_config first, with no sibling bindings to
        // constrain it — now fails it closed and leaves the true chain free.
        let declaration_order = declaration_order_requirements(&MAPPER_PLAN_FLOW_PROOF, &evidence);
        assert_eq!(
            declaration_order[0],
            ("mapper_config", FlowProofOutcome::Unproven),
            "the decoy pair must no longer satisfy the configuration requirement"
        );
        assert_eq!(declaration_order[1].0, "mapper_execution");
        assert_eq!(
            proved(declaration_order[1].1.clone())
                .bindings
                .get(&ProofRole::Builder),
            Some(&node("node:plan-builder"))
        );

        // Most-constrained-first runs mapper_execution instead, and its
        // MEMBER join binds Builder to the real plan builder.
        let outcomes = match_flow_requirements(&MAPPER_PLAN_FLOW_PROOF, &evidence);
        assert_eq!(outcomes[0].0, "mapper_config");
        assert_eq!(outcomes[1].0, "mapper_execution");
        let execution = proved(outcomes[1].1.clone());
        assert_eq!(
            execution.bindings.get(&ProofRole::Builder),
            Some(&node("node:plan-builder"))
        );
        assert_eq!(
            execution
                .atoms
                .iter()
                .map(|atom| atom.atom)
                .collect::<Vec<_>>(),
            vec![ProofAtomId::A3, ProofAtomId::A4]
        );
        // mapper_config, evaluated second, now fails closed on the true
        // bindings instead of reporting a proof off the unrelated pair.
        assert_eq!(outcomes[0].1, FlowProofOutcome::Unproven);
    }

    #[test]
    fn reordering_changes_nothing_for_a_formula_whose_requirements_share_no_roles() {
        assert_eq!(
            DISJOINT_ROLE_FORMULA.requirements(),
            vec!["logger_event", "mapper_execution"]
        );
        assert_eq!(
            DISJOINT_ROLE_FORMULA.requirements_by_constraint_strength(),
            vec!["mapper_execution", "logger_event"],
            "the evaluation order must actually differ, or the check is vacuous"
        );

        let mut combined = a_evidence();
        combined.source_aspects.extend(m_evidence().source_aspects);
        combined
            .typed_relations
            .extend(m_evidence().typed_relations);
        let mut config_only = a_evidence();
        config_only.typed_relations.remove(2);
        let mut owner_only = m_evidence();
        owner_only.typed_relations.clear();
        for evidence in [
            PacketProofEvidence::default(),
            m_evidence(),
            a_evidence(),
            c_evidence(),
            combined,
            config_only,
            owner_only,
        ] {
            assert_eq!(
                match_flow_requirements(&DISJOINT_ROLE_FORMULA, &evidence),
                declaration_order_requirements(&DISJOINT_ROLE_FORMULA, &evidence),
                "disjoint roles: every verdict must match declaration-order behavior"
            );
        }
    }

    #[test]
    fn the_full_group_path_is_untouched_by_the_fallback_ordering() {
        for (formula, evidence) in [
            (&LOG_HANDLER_FLOW_PROOF, m_evidence()),
            (&MAPPER_PLAN_FLOW_PROOF, a_evidence()),
            (&CSS_ANIMATION_FLOW_PROOF, c_evidence()),
        ] {
            let group = proved(match_flow_proof(formula, &evidence));
            let outcomes = match_flow_requirements(formula, &evidence);
            assert_eq!(
                outcomes,
                declaration_order_requirements(formula, &evidence),
                "the full-group path never reaches the reordered fallback"
            );
            assert_eq!(
                outcomes
                    .iter()
                    .map(|(requirement, _)| *requirement)
                    .collect::<Vec<_>>(),
                formula.requirements()
            );
            for (requirement, outcome) in &outcomes {
                let proof = proved(outcome.clone());
                assert_eq!(
                    proof.bindings, group.bindings,
                    "{requirement} must report the one group-wide assignment"
                );
                assert_eq!(
                    proof.atoms.iter().map(|atom| atom.atom).collect::<Vec<_>>(),
                    formula.atoms_for(requirement)
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // Conversion boundaries and spec-type guarantees.
    // ------------------------------------------------------------------

    #[test]
    fn source_aspect_conversion_drops_query_and_requires_a_reread_eligible_receipt() {
        let owner = citation("node:flow-owner", Some(true));
        let with_query = source_range_unit(
            Some("node:flow-owner"),
            Some("fn synthetic_owner() {}"),
            Some("a prompt-derived query"),
        );
        let without_query = source_range_unit(
            Some("node:flow-owner"),
            Some("fn synthetic_owner() {}"),
            None,
        );
        assert_eq!(
            VerifiedSourceAspectReceipt::from_source_range_unit(&with_query, &owner, None),
            VerifiedSourceAspectReceipt::from_source_range_unit(&without_query, &owner, None),
            "the conversion must drop `query`: receipts from units differing only in it are equal"
        );

        let mut wrong_kind = source_range_unit(None, Some("fn synthetic_owner() {}"), None);
        wrong_kind.kind = SupportUnitKindDto::TypedGraphEdge;
        assert!(
            VerifiedSourceAspectReceipt::from_source_range_unit(&wrong_kind, &owner, None)
                .is_none()
        );

        let empty_snippet = source_range_unit(None, Some("   \n"), None);
        assert!(
            VerifiedSourceAspectReceipt::from_source_range_unit(&empty_snippet, &owner, None)
                .is_none()
        );
        let no_snippet = source_range_unit(None, None, None);
        assert!(
            VerifiedSourceAspectReceipt::from_source_range_unit(&no_snippet, &owner, None)
                .is_none()
        );

        let unit = source_range_unit(None, Some("fn synthetic_owner() {}"), None);
        for eligible in [None, Some(false)] {
            let ineligible = citation("node:flow-owner", eligible);
            assert!(
                VerifiedSourceAspectReceipt::from_source_range_unit(&unit, &ineligible, None)
                    .is_none(),
                "owner eligibility {eligible:?} must not yield a receipt"
            );
        }
    }

    #[test]
    fn typed_relation_conversion_defaults_to_unknown_coverage_and_drops_scores() {
        let edge = GraphEdgeDto {
            id: EdgeId("edge:dispatch".to_string()),
            source: node("node:flow-owner"),
            target: node("node:element-method"),
            kind: EdgeKind::CALL,
            confidence: Some(0.25),
            certainty: Some("certain".to_string()),
            callsite_identity: Some(DISPATCH_CALLSITE.to_string()),
            candidate_targets: vec![node("node:decoy-target-0")],
        };
        let mut scored_differently = edge.clone();
        scored_differently.confidence = Some(0.99);
        scored_differently.candidate_targets = Vec::new();
        let receipt = VerifiedTypedRelationReceipt::from_graph_edge(&edge, Some(NodeKind::METHOD));
        assert_eq!(
            receipt,
            VerifiedTypedRelationReceipt::from_graph_edge(
                &scored_differently,
                Some(NodeKind::METHOD)
            ),
            "confidence and candidate targets are not receipt-bearing fields"
        );
        assert_eq!(
            receipt.coverage,
            TrailCoverage::Unknown,
            "coverage must default to Unknown; absence facts need an explicit opt-in"
        );
        assert_eq!(receipt.certainty.as_deref(), Some("certain"));
        assert_eq!(
            receipt.callsite_identity.as_deref(),
            Some(DISPATCH_CALLSITE)
        );

        let opted_in = receipt.with_coverage(scan("node:flow-owner", &[EdgeKind::CALL], 1, false));
        assert_eq!(
            opted_in.coverage,
            scan("node:flow-owner", &[EdgeKind::CALL], 1, false)
        );
    }

    #[test]
    fn spec_types_reachable_from_flow_proof_spec_are_const_copy_and_static() {
        fn assert_spec_type<T: Copy + 'static>() {}
        assert_spec_type::<FlowProofSpec>();
        assert_spec_type::<FlowProofFormula>();
        assert_spec_type::<ProofAtomSpec>();
        assert_spec_type::<ProofFactPattern>();
        assert_spec_type::<TypedRelationPattern>();
        assert_spec_type::<SourceAspectPattern>();
        assert_spec_type::<AbsentTypedRelationPattern>();
        assert_spec_type::<AnchoredContainmentPattern>();
        assert_spec_type::<ProofEndpointPattern>();
        assert_spec_type::<CallsiteMarkerPattern>();
        assert_spec_type::<ProofRole>();
        assert_spec_type::<ProofAtomId>();
        assert_spec_type::<SourceAspectKind>();
        // The formulas themselves are const items, which is the
        // const-constructibility proof.
        const _: &FlowProofFormula = &LOG_HANDLER_FLOW_PROOF;
        const _: &FlowProofFormula = &MAPPER_PLAN_FLOW_PROOF;
        const _: &FlowProofFormula = &CSS_ANIMATION_FLOW_PROOF;
    }
}
