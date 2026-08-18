//! Runtime-only packet candidates that keep graph proof beside public search hits.

use codestory_agent::packet_flow_requirements::{
    FlowRequirement, flow_requirement_call_receipt_is_valid,
};
use codestory_agent::packet_proof_atoms::{
    CallsiteMarkerPattern, FlowProofFormula, FlowProofOutcome, PacketProofEvidence,
    ProofEndpointPattern, ProofFactPattern, ProofRole, TypedRelationPattern,
    VerifiedTypedRelationReceipt, match_flow_requirements,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentCitationDto, EdgeId, EdgeKind, GraphArtifactDto, GraphResponse, NodeKind,
    SearchHit,
};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::rc::Rc;

const PACKET_CANDIDATE_SELECTION_VIEW_ID: &str = "packet-search-provenance";
const PACKET_CANDIDATE_SELECTION_VIEW_ID_PREFIX: &str = "packet-search-provenance-";
const PACKET_CANDIDATE_GRAPH_EDGE_LIMIT: usize = 20;
const PACKET_CITATION_EDGE_LIMIT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PacketGraphDirection {
    Outgoing,
    Incoming,
}

/// One bounded hydration trail's coverage facts, recorded runtime-side so the
/// proof-evidence extras builder can construct honest `TrailCoverage::Scanned`
/// records (R2, binding rule 7).
///
/// `coverage_edge_ids` is the NARROWED completeness set (F3 finding 3): not
/// every enumerated edge, but exactly the edges the scan's rule-7 coverage
/// claim needs to remain sound when they leave the evidence — the enumerated
/// edges of absence-subject kinds (the absence facts' subjects; a hidden one
/// could refute the absence) plus, for depth-2 scans, the enumerated MEMBER
/// edges (the deeper-rooted arm's membership witnesses). Truncation covers
/// reach, so incidental enumerated edges of other kinds (e.g. IMPORT context
/// lost to a graph cap) never void the coverage. The extras builder refuses
/// the scan when any recorded id is missing from the live evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacketCandidateTrailScan {
    /// Trail root, in DTO node-id form (the numeric id as a string).
    pub(crate) root: String,
    pub(crate) direction: PacketGraphDirection,
    pub(crate) depth: u32,
    /// The trail's complete edge filter — rule 7's traversal edge-kind set.
    pub(crate) edge_kinds: Vec<EdgeKind>,
    /// True when the trail hit its node cap before completing.
    pub(crate) truncated: bool,
    /// DTO ids of the enumerated edges the coverage claim depends on (see
    /// the struct docs). Every one of them must be live for the scan to be
    /// attached.
    pub(crate) coverage_edge_ids: Vec<EdgeId>,
}

/// Which widened hydration trails the active packet's task-class formulas
/// justify (R2). Derived exclusively from the formulas' typed-relation and
/// absence patterns — never from names, paths, or prompt tokens — and empty
/// for task classes without formula-bearing requirements, so Legacy packets
/// and plain searches hydrate exactly as before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PacketAtomHydrationSpec {
    /// Per root node kind: the atom-required edge kinds that get one separate
    /// bounded POST-PASS trail each (per direction), so a widened kind can
    /// never evict the CALL edges other atoms need. Never run on the sidecar
    /// stage clock (F3 REVISE).
    pub(crate) rooted: Vec<(NodeKind, Vec<EdgeKind>)>,
    /// FILE-rooted structural hydration in the POST-PASS: one depth-2 trail
    /// per direction with the uniform `[MEMBER, USAGE, IMPORT]` filter, so a
    /// single coverage record carries both the absent kind and the MEMBER
    /// witness rule 7's deeper-rooted arm requires. This flag also enables
    /// the cheap depth-1 `[MEMBER, IMPORT]` identity trails the in-loop R6
    /// promotion consumes mid-pass (the C bootstrap chain's needs) — the
    /// ONLY widened hydration allowed on the stage clock.
    pub(crate) file_structural: bool,
    /// Edge kinds named by the formulas' absence facts — the subjects whose
    /// enumerated edges belong in every scan's narrowed coverage set.
    pub(crate) absence_kinds: Vec<EdgeKind>,
    /// The typed-relation patterns of the active formulas whose edge kind is
    /// cross-container / identity-bearing (rev 5.4:
    /// [`PACKET_CROSS_CONTAINER_PROMOTION_KINDS`] only — membership/usage
    /// patterns never drive admission): the R6 promotion need-gate matches
    /// hydrated edges against these to decide which endpoint identities a
    /// still-unproven atom actually REQUIRES. Empty for all-Legacy packets
    /// AND for formulas naming no cross-container kind (the M family), which
    /// makes promotion inert for both.
    pub(crate) promotion_patterns: Vec<PacketPromotionPattern>,
    /// EVERY typed-relation pattern of the active formulas, with the same
    /// requirement/role provenance — a strict superset of
    /// `promotion_patterns`. These drive the need-set's PRIORITY ORDER only
    /// (gate 6: atom-role multiplicity), never its membership: an identity
    /// still joins the set exclusively through a cross-container match
    /// (rev 5.4), and only a cross-container role can open a promotion slot.
    /// Ordering by role multiplicity is what separates an identity that
    /// occupies several role positions of the requirement group — the one
    /// that can complete a GROUP-consistent proof — from a lone endpoint.
    pub(crate) role_scoring_patterns: Vec<PacketPromotionPattern>,
    /// The packet's active proof formulas, deduplicated by identity — the
    /// group matcher's input at the query-boundary retirement checkpoint
    /// (round 5.5 item 2b). Empty exactly when the packet has no
    /// formula-bearing requirement.
    pub(crate) formulas: Vec<PacketProofFormulaRef>,
}

/// One cross-container promotion pattern plus the provenance the R6
/// need-gate needs beyond the pattern itself (round 5.5 item 2):
///
/// * `requirement` — the flow requirement whose material atom carries the
///   pattern. This is the unit the query-boundary group checkpoint retires:
///   once the requirement's atoms discharge group-consistently against the
///   accumulated typed receipts, its patterns stop driving admission.
/// * `source_roles` / `target_roles` — the formula ROLES the pattern's
///   endpoints name (an `AnyOfRoles` guard names all of its alternatives).
///   These are the per-query promotion SLOTS: at most one promotion per
///   role per query, so the bound is derived from the atoms and never from a
///   constant — A yields {Builder, ConfigType} (2), C yields {Entrypoint,
///   VarsSource, BaseSource, AnimSource} (4), and M/all-Legacy yield none at
///   all, which is why they stay structurally inert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacketPromotionPattern {
    pub(crate) requirement: &'static str,
    pub(crate) pattern: &'static TypedRelationPattern,
    pub(crate) source_roles: Vec<ProofRole>,
    pub(crate) target_roles: Vec<ProofRole>,
}

impl PacketPromotionPattern {
    /// The roles one endpoint of this pattern names.
    fn roles_for(&self, endpoint: PacketPatternEndpoint) -> &[ProofRole] {
        match endpoint {
            PacketPatternEndpoint::Source => &self.source_roles,
            PacketPatternEndpoint::Target => &self.target_roles,
        }
    }
}

/// Which end of a typed-relation pattern an identity was bound at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PacketPatternEndpoint {
    Source,
    Target,
}

impl PacketPatternEndpoint {
    fn label(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Target => "target",
        }
    }
}

impl Deref for PacketPromotionPattern {
    type Target = TypedRelationPattern;

    fn deref(&self) -> &Self::Target {
        self.pattern
    }
}

/// A `&'static FlowProofFormula` with IDENTITY equality. The formula type is
/// a const table with no `PartialEq`, and the hydration spec keeps its
/// structural equality derive, so the reference is compared by pointer —
/// which is also exactly the dedup key the spec builder uses.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PacketProofFormulaRef(pub(crate) &'static FlowProofFormula);

impl PartialEq for PacketProofFormulaRef {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for PacketProofFormulaRef {}

impl PacketAtomHydrationSpec {
    pub(crate) fn is_empty(&self) -> bool {
        self.rooted.is_empty() && !self.file_structural
    }

    pub(crate) fn kinds_for_root(&self, kind: NodeKind) -> &[EdgeKind] {
        self.rooted
            .iter()
            .find(|(rooted_kind, _)| *rooted_kind == kind)
            .map(|(_, kinds)| kinds.as_slice())
            .unwrap_or(&[])
    }

    /// The IN-LOOP identity trail kinds for one non-FILE root: the root's
    /// atom-required kinds intersected with the CROSS-CONTAINER promotion
    /// kinds (rev 5.4 corollary, tightened after gate 5c): the in-loop
    /// identity trails exist solely to feed the R6 need-set, and rev 5.4
    /// admits only IMPORT/TYPE_USAGE matches — so trailing any other kind
    /// in-loop is pure stage-clock waste, AND a needless kind's fanout
    /// shares the trail accessor's edge budget (`max_nodes × 3`): on the
    /// animate entrypoint the combined [MEMBER, IMPORT] fanout (99+99)
    /// crossed that budget and the accessor's break-after-root retained ZERO
    /// edges, contributing nothing (gate 5c root cause). An A-family spec
    /// yields `[TYPE_USAGE]` on CLASS/STRUCT roots; M-family (CALL only)
    /// yields nothing. Cost bound: at most 1 extra depth-1 single-kind trail
    /// per direction per candidate for the shipped formulas, under the same
    /// 65-node cap.
    pub(crate) fn identity_trail_kinds_for_root(&self, kind: NodeKind) -> Vec<EdgeKind> {
        self.kinds_for_root(kind)
            .iter()
            .copied()
            .filter(|kind| PACKET_CROSS_CONTAINER_PROMOTION_KINDS.contains(kind))
            .collect()
    }

    /// Every distinct ROLE the cross-container promotion patterns name, in
    /// `ProofRole` order — the per-query promotion slots (round 5.5 item
    /// 2a). Atom-derived, never a constant: the A family yields
    /// {Builder, ConfigType} (2 slots), the C family {Entrypoint, VarsSource,
    /// BaseSource, AnimSource} (4), and the M family and all-Legacy packets
    /// yield NONE — with no cross-container pattern there is no slot, so
    /// their admission cannot even express a promotion.
    pub(crate) fn promotion_role_slots(&self) -> Vec<ProofRole> {
        let mut roles: Vec<ProofRole> = Vec::new();
        for pattern in &self.promotion_patterns {
            for role in pattern
                .source_roles
                .iter()
                .chain(pattern.target_roles.iter())
            {
                if !roles.contains(role) {
                    roles.push(*role);
                }
            }
        }
        roles.sort();
        roles
    }

    /// The edge kinds the active formulas' typed-relation and absence facts
    /// name. This is the input restriction of the retirement checkpoint
    /// (round 5.5 item 2b): a receipt of any other kind can never discharge
    /// an atom, so keeping it would only cost the matcher steps.
    pub(crate) fn formula_receipt_kinds(&self) -> Vec<EdgeKind> {
        let mut kinds: Vec<EdgeKind> = Vec::new();
        for formula in &self.formulas {
            for atom in formula.0.atoms {
                for fact in atom.facts {
                    let kind = match fact {
                        ProofFactPattern::TypedRelation(pattern) => pattern.kind,
                        ProofFactPattern::AbsentTypedRelation(pattern) => pattern.kind,
                        ProofFactPattern::SourceAspect(_)
                        | ProofFactPattern::AnchoredLineContainment(_) => continue,
                    };
                    if !kinds.contains(&kind) {
                        kinds.push(kind);
                    }
                }
            }
        }
        kinds
    }
}

/// The uniform edge filter of the POST-PASS FILE-rooted structural trails (R2).
pub(crate) const PACKET_FILE_STRUCTURAL_TRAIL_KINDS: [EdgeKind; 3] =
    [EdgeKind::MEMBER, EdgeKind::USAGE, EdgeKind::IMPORT];

/// The depth-1 identity-establishing filter the IN-LOOP hydration runs for
/// FILE roots when the C-family spec is active — exactly
/// `PACKET_FILE_STRUCTURAL_TRAIL_KINDS ∩
/// PACKET_CROSS_CONTAINER_PROMOTION_KINDS`. MEMBER was removed after gate 5c:
/// its matches feed nothing under rev 5.4, and its fanout shared the trail
/// accessor's edge budget with IMPORT — on a 99-import entrypoint the
/// combined fetch crossed `max_nodes × 3` and the accessor retained zero
/// edges, silencing the whole import closure. With IMPORT alone the
/// entrypoint's 99-target trail truncates at the node cap but RETAINS its
/// first ~64 import edges, whose identities contribute (truncation bars
/// absence claims, never positive identity receipts — rule 7).
pub(crate) const PACKET_FILE_IDENTITY_TRAIL_KINDS: [EdgeKind; 1] = [EdgeKind::IMPORT];

/// The CROSS-CONTAINER / identity-bearing kinds whose patterns may feed the
/// R6 promotion need-set (contract rev 5.4, after round-4 telemetry showed
/// generic role-to-role MEMBER/USAGE patterns flooding the need-set with
/// every hydrated container's members — 84-91% of admissions became
/// promotions): IMPORT and TYPE_USAGE endpoints name retrieval-underranked
/// containers (files, types) that admission exists to rescue. Membership and
/// usage kinds discharge atoms as receipts but never drive admission — their
/// carriers are the containers themselves, which either resolve naturally or
/// arrive through the cross-container promotions.
pub(crate) const PACKET_CROSS_CONTAINER_PROMOTION_KINDS: [EdgeKind; 2] =
    [EdgeKind::IMPORT, EdgeKind::TYPE_USAGE];

/// Upper bound on the typed receipts the query-boundary retirement
/// checkpoint accumulates. A real packet's checkpoint input is tens of
/// receipts (the formulas' fact kinds, hydrated in-loop and deduplicated by
/// edge id); the cap only bounds an adversarial fanout so the matcher's own
/// step limit is never the thing that stops us. Overflow is fail-closed and
/// deterministic: the first receipts in accumulation order are kept, so the
/// checkpoint can only under-retire, never over-retire.
const PACKET_CHECKPOINT_RECEIPT_LIMIT: usize = 256;

/// Derives the widened hydration spec from the packet's flow requirements.
/// Only the edge kinds the task class's formula atoms actually name get
/// trails, bounded per the contract (≤3 kinds × 2 directions per candidate
/// for the shipped formulas).
pub(crate) fn packet_atom_hydration_spec(
    flow_requirements: &[FlowRequirement],
) -> PacketAtomHydrationSpec {
    let mut atom_kinds: Vec<EdgeKind> = Vec::new();
    let mut absence_kinds: Vec<EdgeKind> = Vec::new();
    let mut promotion_patterns: Vec<PacketPromotionPattern> = Vec::new();
    let mut role_scoring_patterns: Vec<PacketPromotionPattern> = Vec::new();
    let mut formulas: Vec<PacketProofFormulaRef> = Vec::new();
    let push_kind = |kind: EdgeKind, kinds: &mut Vec<EdgeKind>| {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    };
    for requirement in flow_requirements {
        let Some(formula) = requirement.proof.formula() else {
            continue;
        };
        if !formulas
            .iter()
            .any(|existing| std::ptr::eq(existing.0, formula))
        {
            formulas.push(PacketProofFormulaRef(formula));
        }
        for atom in formula.atoms {
            for fact in atom.facts {
                match fact {
                    ProofFactPattern::TypedRelation(pattern) => {
                        push_kind(pattern.kind, &mut atom_kinds);
                        if role_scoring_patterns
                            .iter()
                            .any(|existing| std::ptr::eq(existing.pattern, pattern))
                        {
                            continue;
                        }
                        let entry = PacketPromotionPattern {
                            requirement: atom.requirement,
                            pattern,
                            source_roles: promotion_endpoint_roles(pattern.source),
                            target_roles: promotion_endpoint_roles(pattern.target),
                        };
                        // Membership stays cross-container-only (rev 5.4);
                        // every typed pattern additionally feeds the
                        // multiplicity SCORE that orders the set.
                        if PACKET_CROSS_CONTAINER_PROMOTION_KINDS.contains(&pattern.kind) {
                            promotion_patterns.push(entry.clone());
                        }
                        role_scoring_patterns.push(entry);
                    }
                    ProofFactPattern::AbsentTypedRelation(pattern) => {
                        push_kind(pattern.kind, &mut atom_kinds);
                        push_kind(pattern.kind, &mut absence_kinds);
                    }
                    ProofFactPattern::SourceAspect(_)
                    | ProofFactPattern::AnchoredLineContainment(_) => {}
                }
            }
        }
    }
    if atom_kinds.is_empty() {
        return PacketAtomHydrationSpec::default();
    }
    let intersect = |allowed: &[EdgeKind]| {
        allowed
            .iter()
            .copied()
            .filter(|kind| atom_kinds.contains(kind))
            .collect::<Vec<_>>()
    };
    // The root-kind table restates the contract's R2 enumeration (CLASS,
    // FILE, structural CONSTANT/VARIABLE/FUNCTION, MODULE) as edge-kind
    // budgets per root family. CALL on behavioral roots is today's hydration
    // and stays outside this spec.
    let class_kinds = intersect(&[EdgeKind::TYPE_USAGE, EdgeKind::MEMBER, EdgeKind::CALL]);
    let behavioral_kinds = intersect(&[EdgeKind::TYPE_USAGE, EdgeKind::MEMBER, EdgeKind::USAGE]);
    let structural_kinds = intersect(&[EdgeKind::MEMBER, EdgeKind::USAGE]);
    let mut rooted = Vec::new();
    for (kinds, roots) in [
        (class_kinds, &[NodeKind::CLASS, NodeKind::STRUCT][..]),
        (
            behavioral_kinds,
            &[NodeKind::FUNCTION, NodeKind::METHOD, NodeKind::MACRO][..],
        ),
        (
            structural_kinds,
            &[NodeKind::CONSTANT, NodeKind::VARIABLE, NodeKind::MODULE][..],
        ),
    ] {
        if kinds.is_empty() {
            continue;
        }
        for root in roots {
            rooted.push((*root, kinds.clone()));
        }
    }
    PacketAtomHydrationSpec {
        rooted,
        // FILE-rooted structural trails serve file-to-file structure: they
        // run only when the formulas name IMPORT (the C-family signature).
        // MEMBER alone (the A formulas) is served by the owner-rooted trails
        // above — per the contract, A3's MEMBER edge arrives via a Builder-
        // or method-rooted trail, never via file hydration.
        file_structural: atom_kinds.contains(&EdgeKind::IMPORT),
        absence_kinds,
        promotion_patterns,
        role_scoring_patterns,
        formulas,
    }
}

/// Thread-scoped state for one packet operation's proof plumbing: the widened
/// hydration spec (read by candidate hydration), the trail-scan ledger keyed
/// by graph artifact id (written at candidate-graph merge, drained by the
/// orchestrator's proof-evidence extras builder), and the R6 promotion
/// need-set shared across EVERY sidecar query of the packet (gate round 2,
/// finding 1: the bootstrap chain establishes identities while resolving one
/// query's candidates and must promote candidates in OTHER queries' windows
/// — per-call state killed the chain at link one).
///
/// PROMOTION IS ATOM-NEED-GATED (contract rev 5.3, after gate round 3 showed
/// unfiltered identity promotion mass-displacing base-order evidence) and
/// CROSS-CONTAINER-RESTRICTED (rev 5.4, after round-4 telemetry showed
/// generic MEMBER/USAGE role-to-role patterns flooding the set): an identity
/// joins the need-set ONLY when it is a ROLE-CONSTRAINED endpoint of a
/// hydrated edge that matches a still-unproven material atom's IMPORT or
/// TYPE_USAGE pattern (checked with the R1(c) mirror against the
/// pre-filtered `promotion_patterns`). An identity that merely exists — an exact in-loop
/// resolution, an endpoint of a non-matching edge, an `Any`-endpoint of a
/// matching edge — never promotes. With no active formula-bearing
/// requirements the pattern list is empty, the need-set stays empty, and
/// promotion is INERT: admission is bit-identical to pre-R6 behavior.
/// "Still-unproven" is exact at admission time: the finalize matcher has not
/// run during retrieval, so every material atom of the active formulas is
/// honestly unproven while candidates are being admitted.
///
/// Cross-query ordering note (adjudicated): the batch order is fixed, so
/// queries resolved AFTER an identity was established benefit from it while
/// earlier queries cannot retroactively re-admit — an acceptable, fully
/// deterministic asymmetry.
///
/// Round 5.5 item 2 adds two bounds on top of the need-gate, both
/// atom-derived:
///
/// * (a) PER-ROLE PER-QUERY PROMOTION SLOTS — at most one promotion per
///   formula ROLE per sidecar query, the roles being the endpoints of the
///   cross-container patterns (A: Builder/ConfigType = 2; C: Entrypoint plus
///   the three source roles = 4; M and all-Legacy: none, so they cannot even
///   express a promotion and stay bit-identical). See
///   [`PacketProofSession::free_promotion_role`].
/// * (b) QUERY-BOUNDARY GROUP-CHECKPOINTED RETIREMENT — after each query the
///   public group matcher runs over the accumulated typed receipts and
///   retires the requirements it proves, silencing their promotion patterns.
///   See [`PacketProofSession::checkpoint_group_retirement`].
///
/// Plain searches and Legacy packets never install a session, so their
/// behavior is unchanged (the resolution loop falls back to a throwaway
/// per-call session whose empty pattern list keeps promotion inert).
#[derive(Debug, Default)]
pub(crate) struct PacketProofSession {
    pub(crate) hydration: PacketAtomHydrationSpec,
    artifact_scans: RefCell<Vec<(String, Vec<PacketCandidateTrailScan>)>>,
    atom_needed_node_ids: RefCell<HashSet<i64>>,
    /// Which (role, requirement) attributions put each need-set identity
    /// there — the per-query promotion slots (round 5.5 item 2a) and the
    /// retirement linkage (item 2b) read exactly this map. Recorded on every
    /// pattern match, not only the first, so an identity that several roles
    /// need can be admitted through whichever slot is still free.
    atom_needed_roles: RefCell<HashMap<i64, Vec<PacketNeedRoleAttribution>>>,
    /// Typed receipts accumulated in-loop, deduplicated by edge id and
    /// restricted to the formulas' fact kinds — the retirement checkpoint's
    /// only input.
    checkpoint_receipts: RefCell<Vec<VerifiedTypedRelationReceipt>>,
    checkpoint_receipt_ids: RefCell<HashSet<String>>,
    /// Requirements whose atoms the group matcher has already discharged
    /// against the accumulated receipts. Grows monotonically; a retired
    /// requirement's promotion patterns stop driving admission.
    retired_requirements: RefCell<Vec<&'static str>>,
    /// [`PacketAtomHydrationSpec::formula_receipt_kinds`], computed once.
    receipt_kinds: Vec<EdgeKind>,
    file_identity_cache: RefCell<HashMap<String, Option<i64>>>,
    /// Env-gated R6 observability (gate round 4): recorded only when the
    /// step-trace artifact is armed, drained into the `r6_session` section of
    /// the developer step trace — NEVER into `retrieval_trace`.
    trace_enabled: bool,
    need_set_trace: RefCell<Vec<PacketNeedSetTraceEntry>>,
    resolved_node_ids: RefCell<HashSet<i64>>,
    query_admissions: RefCell<Vec<PacketQueryAdmissionTrace>>,
    hydration_trace: RefCell<Vec<PacketIdentityHydrationTrace>>,
}

/// Which pattern endpoint added one node id to the promotion need-set.
#[derive(Debug, Clone)]
struct PacketNeedSetTraceEntry {
    node_id: i64,
    pattern_kind: EdgeKind,
    endpoint: &'static str,
    roles: Vec<ProofRole>,
}

/// One (requirement, role) position an identity occupies in the active
/// formulas. The requirement is what retirement silences; the count of
/// distinct un-retired attributions is the identity's PRIORITY (gate 6 —
/// atom-role multiplicity).
///
/// `slot_eligible` separates the two jobs an attribution can do: only a
/// CROSS-CONTAINER attribution (rev 5.4) may open a promotion slot, while
/// every attribution — including the membership/usage and CALL positions —
/// counts toward the priority score. Ordering the need-set can never add a
/// member to it, so rev 5.4's membership restriction is untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PacketNeedRoleAttribution {
    role: ProofRole,
    requirement: &'static str,
    slot_eligible: bool,
}

/// One resolution call's admission decisions (env-gated).
#[derive(Debug, Clone, Default)]
pub(crate) struct PacketQueryAdmissionTrace {
    pub(crate) query_index: usize,
    /// (node id, admitted via promotion).
    pub(crate) admitted: Vec<(String, bool)>,
    /// The per-role promotion slots this query consumed, in consumption
    /// order (round 5.5 item 2a).
    pub(crate) promotion_roles_used: Vec<ProofRole>,
    /// Un-attempted remainder at query end: (promotion identity if
    /// derivable, whether it was in the need-set when the query ended,
    /// whether it still had a free promotion SLOT then — round 5.5 item 2a
    /// separates slot exhaustion from resolution-budget exhaustion).
    pub(crate) unattempted: Vec<(Option<i64>, bool, bool)>,
}

/// One identity-trail hydration's contribution (env-gated).
#[derive(Debug, Clone)]
struct PacketIdentityHydrationTrace {
    root: String,
    edge_count: usize,
    needed_added: Vec<i64>,
}

impl PacketProofSession {
    pub(crate) fn new(hydration: PacketAtomHydrationSpec) -> Self {
        let receipt_kinds = hydration.formula_receipt_kinds();
        Self {
            hydration,
            artifact_scans: RefCell::new(Vec::new()),
            atom_needed_node_ids: RefCell::new(HashSet::new()),
            atom_needed_roles: RefCell::new(HashMap::new()),
            checkpoint_receipts: RefCell::new(Vec::new()),
            checkpoint_receipt_ids: RefCell::new(HashSet::new()),
            retired_requirements: RefCell::new(Vec::new()),
            receipt_kinds,
            file_identity_cache: RefCell::new(HashMap::new()),
            trace_enabled: crate::agent::trace_export::packet_step_trace_armed(),
            need_set_trace: RefCell::new(Vec::new()),
            resolved_node_ids: RefCell::new(HashSet::new()),
            query_admissions: RefCell::new(Vec::new()),
            hydration_trace: RefCell::new(Vec::new()),
        }
    }

    /// Whether the env-gated R6 trace is armed — callers must skip any
    /// recording work with a non-trivial cost (identity derivation for
    /// un-attempted remainders) when it is off, so production admission pays
    /// nothing for observability.
    pub(crate) fn trace_enabled(&self) -> bool {
        self.trace_enabled
    }

    /// Test-only: arm the R6 trace without touching the process environment.
    #[cfg(test)]
    pub(crate) fn with_trace_enabled(mut self) -> Self {
        self.trace_enabled = true;
        self
    }

    /// Feeds one hydrated candidate graph into the promotion need-set AND
    /// into the retirement checkpoint's receipt set: every
    /// edge is matched against the active formulas' typed-relation patterns
    /// (the R1(c) mirror — single-receipt classification), and each endpoint
    /// whose corresponding pattern endpoint is role-constrained joins the
    /// set. An edge matching no pattern, and the `Any` endpoints of matching
    /// edges (e.g. M3's unconstrained dispatch target), contribute nothing —
    /// which is what keeps M-shard and all-Legacy admission displacement-free
    /// (rev 5.3).
    pub(crate) fn record_atom_needed_identities(&self, graph: &GraphResponse) {
        if self.hydration.promotion_patterns.is_empty() {
            return;
        }
        let node_kinds = graph
            .nodes
            .iter()
            .map(|node| (node.id.0.as_str(), node.kind))
            .collect::<HashMap<_, _>>();
        let mut needed = self.atom_needed_node_ids.borrow_mut();
        let mut roles = self.atom_needed_roles.borrow_mut();
        let mut added_here: Vec<i64> = Vec::new();
        let mut edge_count = 0usize;
        for edge in &graph.edges {
            edge_count += 1;
            self.accumulate_checkpoint_receipt(edge, &node_kinds);
            // One pass over EVERY typed pattern. A cross-container match is
            // the only thing that adds a member (rev 5.4) or opens a slot;
            // every other match only records the role position the identity
            // occupies, which is what the multiplicity priority counts.
            for pattern in &self.hydration.role_scoring_patterns {
                if !edge_matches_typed_relation_pattern(pattern, edge, &node_kinds) {
                    continue;
                }
                let slot_eligible = PACKET_CROSS_CONTAINER_PROMOTION_KINDS.contains(&pattern.kind);
                for (endpoint, raw) in [
                    (PacketPatternEndpoint::Source, &edge.source.0),
                    (PacketPatternEndpoint::Target, &edge.target.0),
                ] {
                    let endpoint_pattern = match endpoint {
                        PacketPatternEndpoint::Source => pattern.source,
                        PacketPatternEndpoint::Target => pattern.target,
                    };
                    if !promotion_endpoint_is_role_constrained(endpoint_pattern) {
                        continue;
                    }
                    let Ok(node_id) = raw.parse::<i64>() else {
                        continue;
                    };
                    let endpoint_roles = pattern.roles_for(endpoint);
                    // Attribution is recorded on EVERY match, not only the
                    // first: an identity several roles need must stay
                    // admissible through whichever of its slots is free, and
                    // the priority score is exactly this multiplicity.
                    let attributions = roles.entry(node_id).or_default();
                    for role in endpoint_roles {
                        let attribution = PacketNeedRoleAttribution {
                            role: *role,
                            requirement: pattern.requirement,
                            slot_eligible,
                        };
                        if !attributions.contains(&attribution) {
                            attributions.push(attribution);
                        }
                    }
                    if slot_eligible && needed.insert(node_id) {
                        added_here.push(node_id);
                        self.need_set_trace
                            .borrow_mut()
                            .push(PacketNeedSetTraceEntry {
                                node_id,
                                pattern_kind: pattern.kind,
                                endpoint: endpoint.label(),
                                roles: endpoint_roles.to_vec(),
                            });
                    }
                }
            }
        }
        if self.trace_enabled {
            self.hydration_trace
                .borrow_mut()
                .push(PacketIdentityHydrationTrace {
                    root: graph.center_id.0.clone(),
                    edge_count,
                    needed_added: added_here,
                });
        }
    }

    /// Accumulates one hydrated edge as a typed receipt for the retirement
    /// checkpoint, restricted to the formulas' fact kinds and deduplicated
    /// by edge id. The receipt is built through the SAME public constructor
    /// finalize uses, so the checkpoint reads exactly what the proof layer
    /// would read.
    fn accumulate_checkpoint_receipt(
        &self,
        edge: &codestory_contracts::api::GraphEdgeDto,
        node_kinds: &HashMap<&str, NodeKind>,
    ) {
        if !self.receipt_kinds.contains(&edge.kind) {
            return;
        }
        let mut receipts = self.checkpoint_receipts.borrow_mut();
        if receipts.len() >= PACKET_CHECKPOINT_RECEIPT_LIMIT {
            return;
        }
        if !self
            .checkpoint_receipt_ids
            .borrow_mut()
            .insert(edge.id.0.clone())
        {
            return;
        }
        receipts.push(VerifiedTypedRelationReceipt::from_graph_edge(
            edge,
            node_kinds.get(edge.target.0.as_str()).copied(),
        ));
    }

    /// QUERY-BOUNDARY GROUP-CHECKPOINTED RETIREMENT (round 5.5 item 2b).
    ///
    /// Runs the PUBLIC group matcher over the typed receipts accumulated
    /// in-loop so far and retires every requirement it reports proven: that
    /// requirement's promotion patterns stop driving admission, because the
    /// need they encode is already met.
    ///
    /// The correctness argument, stated so it survives edits: RETIREMENT IS
    /// EXACTLY AS STRICT AS THE PROOF LAYER ITSELF — it is the proof layer,
    /// called on a subset of the evidence finalize will see. So admission is
    /// never stricter than proof: a requirement that will not discharge at
    /// finalize cannot retire here and keeps hunting. Mid-retrieval the
    /// evidence carries no anchored windows and no coverage records, so
    /// source-aspect, anchored-containment, and absence facts all fail
    /// closed by construction, and only structurally satisfiable typed atoms
    /// can ever count — which is why the shipped A and C requirements (each
    /// carrying a carrier-range atom) keep hunting until the true chain
    /// binds, exactly as adjudicated in round 5.5.
    ///
    /// Properties: monotone (the retired set only grows), deterministic (no
    /// timers, no wall clock, receipts in accumulation order), and FAIL
    /// CLOSED — a [`FlowProofOutcome::Aborted`] checkpoint retires NOTHING.
    /// Retirement silences promotion only; base-order admission continues
    /// unchanged.
    pub(crate) fn checkpoint_group_retirement(&self) {
        if self.hydration.promotion_patterns.is_empty() {
            return;
        }
        {
            let retired = self.retired_requirements.borrow();
            if self
                .hydration
                .promotion_patterns
                .iter()
                .all(|pattern| retired.contains(&pattern.requirement))
            {
                return;
            }
        }
        let evidence = PacketProofEvidence {
            source_aspects: Vec::new(),
            typed_relations: self.checkpoint_receipts.borrow().clone(),
            trail_scans: Vec::new(),
        };
        if evidence.typed_relations.is_empty() {
            return;
        }
        let mut retired = self.retired_requirements.borrow_mut();
        for formula in &self.hydration.formulas {
            for requirement in
                retired_requirements_from_outcomes(&match_flow_requirements(formula.0, &evidence))
            {
                if !retired.contains(&requirement) {
                    retired.push(requirement);
                }
            }
        }
    }

    /// The requirements retired so far, in retirement order.
    pub(crate) fn retired_requirements(&self) -> Vec<&'static str> {
        self.retired_requirements.borrow().clone()
    }

    /// Whether promotion can still displace anything: there is at least one
    /// atom-needed identity AND at least one un-retired promotion pattern.
    /// With no patterns at all (M family, all-Legacy) this is permanently
    /// false and admission is bit-identical to pre-R6 behavior.
    pub(crate) fn promotion_is_active(&self) -> bool {
        if self.hydration.promotion_patterns.is_empty() || !self.has_atom_needed_identities() {
            return false;
        }
        let retired = self.retired_requirements.borrow();
        self.hydration
            .promotion_patterns
            .iter()
            .any(|pattern| !retired.contains(&pattern.requirement))
    }

    /// The promotion SLOT one identity may be admitted through this query,
    /// or `None` when it has none free (round 5.5 item 2a): the lowest
    /// `ProofRole` among the identity's un-retired attributions that this
    /// query has not spent yet. Deterministic by `ProofRole` order; an
    /// identity whose every attributed role is spent or retired simply waits
    /// for the next query, and base-order admission continues meanwhile.
    pub(crate) fn free_promotion_role(
        &self,
        node_id: i64,
        spent_this_query: &[ProofRole],
    ) -> Option<ProofRole> {
        // Membership first: the roles map also carries SCORING-only
        // attributions (non-cross-container role positions), which order the
        // need-set but must never let a non-member in.
        if !self.atom_needed_node_ids.borrow().contains(&node_id) {
            return None;
        }
        let attributions = self.atom_needed_roles.borrow();
        let entries = attributions.get(&node_id)?;
        let retired = self.retired_requirements.borrow();
        entries
            .iter()
            .filter(|entry| entry.slot_eligible && !retired.contains(&entry.requirement))
            .map(|entry| entry.role)
            .filter(|role| !spent_this_query.contains(role))
            .min()
    }

    /// NEED-SET PRIORITY BY ATOM-ROLE MULTIPLICITY (gate 6): the number of
    /// distinct un-retired (requirement, role) positions this identity
    /// occupies in the active formulas.
    ///
    /// Why multiplicity is the right order, stated so it survives edits: the
    /// need-gate exists to find a GROUP-consistent proof, and an identity
    /// that occupies several role positions of the requirement group is the
    /// one that can complete one. A builder type that is the TYPE_USAGE
    /// source of the configuration atom AND the owner position of the
    /// execution atom scores above a lone configuration TARGET, which
    /// occupies exactly one position and can only ever discharge half a
    /// group. Gate 6 measured the failure this repairs: 294 equally-needed
    /// TYPE_USAGE identities, so the slots went to whatever base order
    /// happened to surface first and the true chain was never admitted.
    ///
    /// Atom-derived and deterministic end to end: the score is a count over
    /// the formulas' own patterns — no vocabulary, no query tokens, no
    /// file positions, no repo-specific constants, no fixed counts. It
    /// changes WHICH candidate fills a slot, never how many exist.
    pub(crate) fn promotion_priority(&self, node_id: i64) -> usize {
        let attributions = self.atom_needed_roles.borrow();
        let Some(entries) = attributions.get(&node_id) else {
            return 0;
        };
        let retired = self.retired_requirements.borrow();
        entries
            .iter()
            .filter(|entry| !retired.contains(&entry.requirement))
            .count()
    }

    /// Records one resolution call's admission decisions (env-gated) plus
    /// the admitted node ids for later `already_attempted` attribution.
    pub(crate) fn record_query_admissions(&self, trace: PacketQueryAdmissionTrace) {
        if !self.trace_enabled {
            return;
        }
        {
            let mut resolved = self.resolved_node_ids.borrow_mut();
            for (node_id, _) in &trace.admitted {
                if let Ok(id) = node_id.parse::<i64>() {
                    resolved.insert(id);
                }
            }
        }
        self.query_admissions.borrow_mut().push(trace);
    }

    /// The next query index for admission tracing (env-gated).
    pub(crate) fn next_query_index(&self) -> usize {
        self.query_admissions.borrow().len()
    }

    /// The `r6_session` section of the developer step trace: the final
    /// need-set with the pattern endpoint that added each id, per-query
    /// admission decisions with a derived why-not for the un-attempted
    /// remainder, and the identity-trail hydration summary per root.
    pub(crate) fn r6_trace_json(&self) -> serde_json::Value {
        let needed = self.atom_needed_node_ids.borrow();
        let resolved = self.resolved_node_ids.borrow();
        let attributions = self.atom_needed_roles.borrow();
        let need_set = self
            .need_set_trace
            .borrow()
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "node_id": entry.node_id,
                    "pattern_kind": format!("{:?}", entry.pattern_kind),
                    "endpoint": entry.endpoint,
                    "roles": entry
                        .roles
                        .iter()
                        .map(|role| format!("{role:?}"))
                        .collect::<Vec<_>>(),
                    // Gate 6: the multiplicity priority that orders the set,
                    // with the exact role positions it counts — read this to
                    // see whether a chain identity outranks a lone endpoint.
                    "priority": self.promotion_priority(entry.node_id),
                    "role_positions": attributions
                        .get(&entry.node_id)
                        .map(|entries| {
                            entries
                                .iter()
                                .map(|attribution| {
                                    format!(
                                        "{}:{:?}{}",
                                        attribution.requirement,
                                        attribution.role,
                                        if attribution.slot_eligible { "" } else { "*" }
                                    )
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>();
        drop(attributions);
        let query_admissions = self
            .query_admissions
            .borrow()
            .iter()
            .map(|trace| {
                let admitted = trace
                    .admitted
                    .iter()
                    .map(|(node_id, promoted)| {
                        serde_json::json!({ "node_id": node_id, "promoted": promoted })
                    })
                    .collect::<Vec<_>>();
                let unattempted = trace
                    .unattempted
                    .iter()
                    .map(|(identity, needed_at_query_end, slot_free_at_query_end)| {
                        let why_not = match identity {
                            None => "no_identity",
                            Some(id) if resolved.contains(id) => "already_attempted",
                            Some(_) if *needed_at_query_end && !*slot_free_at_query_end => {
                                "slot_exhausted"
                            }
                            Some(_) if *needed_at_query_end => "budget_exhausted",
                            Some(id) if needed.contains(id) => "query_ordering",
                            Some(_) => "not_in_need_set",
                        };
                        serde_json::json!({ "identity": identity, "why_not": why_not })
                    })
                    .collect::<Vec<_>>();
                serde_json::json!({
                    "query_index": trace.query_index,
                    "admitted": admitted,
                    "unattempted": unattempted,
                    "promotion_roles_used": trace
                        .promotion_roles_used
                        .iter()
                        .map(|role| format!("{role:?}"))
                        .collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        let identity_hydrations = self
            .hydration_trace
            .borrow()
            .iter()
            .map(|trace| {
                serde_json::json!({
                    "root": trace.root,
                    "edge_count": trace.edge_count,
                    "needed_added": trace.needed_added,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "promotion_pattern_count": self.hydration.promotion_patterns.len(),
            "promotion_role_slots": self
                .hydration
                .promotion_role_slots()
                .iter()
                .map(|role| format!("{role:?}"))
                .collect::<Vec<_>>(),
            "retired_requirements": self.retired_requirements(),
            "checkpoint_receipt_count": self.checkpoint_receipts.borrow().len(),
            "need_set": need_set,
            "query_admissions": query_admissions,
            "identity_hydrations": identity_hydrations,
        })
    }

    pub(crate) fn has_atom_needed_identities(&self) -> bool {
        !self.atom_needed_node_ids.borrow().is_empty()
    }

    pub(crate) fn identity_is_atom_needed(&self, node_id: i64) -> bool {
        self.atom_needed_node_ids.borrow().contains(&node_id)
    }

    /// Cross-query cache of file-shaped promotion identities, keyed by the
    /// candidate's normalized repo-relative path — the same declared-path
    /// derivation, spared re-running per query on large pools (stage-clock
    /// hygiene, gate round 2 finding 4).
    pub(crate) fn cached_file_identity(
        &self,
        rel_path: &str,
        derive: impl FnOnce() -> Option<i64>,
    ) -> Option<i64> {
        if let Some(cached) = self.file_identity_cache.borrow().get(rel_path) {
            return *cached;
        }
        let derived = derive();
        self.file_identity_cache
            .borrow_mut()
            .insert(rel_path.to_string(), derived);
        derived
    }

    /// Records the scans behind one merged candidate artifact. First write
    /// wins: the artifact id is immutable lineage of the original bounded
    /// view, so a replay of the same candidate carries the same scans.
    pub(crate) fn record_artifact_scans(
        &self,
        artifact_id: &str,
        scans: &[PacketCandidateTrailScan],
    ) {
        if scans.is_empty() {
            return;
        }
        let mut ledger = self.artifact_scans.borrow_mut();
        if ledger.iter().any(|(existing, _)| existing == artifact_id) {
            return;
        }
        ledger.push((artifact_id.to_string(), scans.to_vec()));
    }

    pub(crate) fn artifact_scans(&self) -> Vec<(String, Vec<PacketCandidateTrailScan>)> {
        self.artifact_scans.borrow().clone()
    }
}

thread_local! {
    static ACTIVE_PACKET_PROOF_SESSION: RefCell<Option<Rc<PacketProofSession>>> =
        const { RefCell::new(None) };
}

pub(crate) fn active_packet_proof_session() -> Option<Rc<PacketProofSession>> {
    ACTIVE_PACKET_PROOF_SESSION.with(|active| active.borrow().clone())
}

pub(crate) struct PacketProofSessionGuard {
    previous: Option<Rc<PacketProofSession>>,
}

impl Drop for PacketProofSessionGuard {
    fn drop(&mut self) {
        ACTIVE_PACKET_PROOF_SESSION.with(|active| {
            active.replace(self.previous.take());
        });
    }
}

/// Installs the packet proof session for the current thread until the guard
/// drops (same scoped pattern as the pinned-retrieval read).
pub(crate) fn install_packet_proof_session(
    session: Rc<PacketProofSession>,
) -> PacketProofSessionGuard {
    let previous = ACTIVE_PACKET_PROOF_SESSION.with(|active| active.replace(Some(session)));
    PacketProofSessionGuard { previous }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacketGraphEdgeProvenance {
    pub(crate) edge_id: EdgeId,
    pub(crate) direction: PacketGraphDirection,
    pub(crate) hop: u32,
    pub(crate) producers: Vec<String>,
    pub(crate) certainty: Option<String>,
}

/// A packet-only search result. Public search DTOs stay unchanged while exact graph proof remains
/// attached until the packet citation and graph artifact are assembled.
#[derive(Debug, Clone)]
pub(crate) struct PacketSearchHit {
    pub(crate) hit: SearchHit,
    pub(crate) graph_provenance: Vec<PacketGraphEdgeProvenance>,
    pub(crate) graph: Option<GraphResponse>,
    /// Coverage records of the bounded trails that hydrated this candidate's
    /// graph (R2). Empty outside an active packet proof session.
    pub(crate) trail_scans: Vec<PacketCandidateTrailScan>,
}

impl PacketSearchHit {
    #[cfg(test)]
    pub(crate) fn without_graph(hit: SearchHit) -> Self {
        Self {
            hit,
            graph_provenance: Vec::new(),
            graph: None,
            trail_scans: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn citation(&self, include_evidence: bool) -> AgentCitationDto {
        self.citation_for_requirements(include_evidence, &[])
    }

    pub(crate) fn citation_for_requirements(
        &self,
        include_evidence: bool,
        flow_requirements: &[FlowRequirement],
    ) -> AgentCitationDto {
        let citation = codestory_agent::citation::to_citation_from_hit(
            &self.hit,
            None,
            None,
            include_evidence,
        );
        self.citation_for_requirements_from_base(citation, include_evidence, flow_requirements)
    }

    fn citation_for_requirements_from_base(
        &self,
        mut citation: AgentCitationDto,
        include_evidence: bool,
        flow_requirements: &[FlowRequirement],
    ) -> AgentCitationDto {
        if include_evidence && self.hit.resolvable {
            let proof_edge_ids = self.proof_edge_ids_for_requirements(&citation, flow_requirements);
            citation.evidence_edge_ids = self.selected_edge_ids_for_requirements(
                &citation,
                flow_requirements,
                PACKET_CITATION_EDGE_LIMIT,
            );
            if !proof_edge_ids.is_empty()
                && citation.evidence_tier
                    == Some(codestory_contracts::api::PacketEvidenceTierDto::DenseSemantic)
                && self.hit.resolvable
                && citation.file_path.is_some()
                && citation.line.is_some()
            {
                // The candidate is no longer dense-only: the exact carrier now owns a strict,
                // receiver-aware parser receipt. Publish that stronger lane atomically so a
                // duplicate dense anchor cannot keep the carrier ineligible.
                citation.evidence_tier =
                    Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph);
                citation.evidence_producer = Some("core_incident_call".to_string());
                citation.eligible_for_sufficiency = Some(true);
                if let Some(breakdown) = citation.retrieval_score_breakdown.as_mut() {
                    breakdown.graph = breakdown.graph.max(breakdown.total);
                    breakdown.tier_cap = None;
                    breakdown.dampening.retain(|reason| reason != "dense_only");
                    if !breakdown
                        .provenance
                        .iter()
                        .any(|producer| producer == "core_incident_call")
                    {
                        breakdown.provenance.push("core_incident_call".to_string());
                    }
                    breakdown.final_rank_reason =
                        Some("receiver-aware parser CALL receipt".to_string());
                }
            }
        }
        citation
    }

    pub(crate) fn has_proof_call_provenance_for_requirement(
        &self,
        citation: &AgentCitationDto,
        requirement: &FlowRequirement,
    ) -> bool {
        !self
            .proof_edge_ids_for_requirement(citation, requirement)
            .is_empty()
    }

    #[cfg(test)]
    pub(crate) fn has_proof_call_provenance(&self) -> bool {
        let Some(graph) = self.graph.as_ref() else {
            return false;
        };
        let provenance_ids = self
            .graph_provenance
            .iter()
            .map(|provenance| &provenance.edge_id)
            .collect::<HashSet<_>>();
        graph.edges.iter().any(|edge| {
            provenance_ids.contains(&edge.id)
                && edge.kind == EdgeKind::CALL
                && (edge.certainty.as_deref() == Some("certain")
                    || (edge.certainty.is_none()
                        && edge.confidence.is_none()
                        && edge.callsite_identity.as_deref().is_some_and(|identity| {
                            identity.contains("|receiver-owner:")
                                && identity.split('|').any(|segment| {
                                    segment.starts_with("syntax:") && segment.ends_with("-call")
                                })
                        })))
        })
    }

    pub(crate) fn proof_edge_ids_for_requirements(
        &self,
        citation: &AgentCitationDto,
        flow_requirements: &[FlowRequirement],
    ) -> Vec<EdgeId> {
        let mut selected = Vec::new();
        for requirement in flow_requirements
            .iter()
            .filter(|requirement| packet_requirement_applies_to_citation(requirement, citation))
        {
            if let Some(edge_id) = self
                .proof_edge_ids_for_requirement(citation, requirement)
                .into_iter()
                .next()
                && !selected.contains(&edge_id)
            {
                selected.push(edge_id);
            }
        }
        selected
    }

    fn selected_edge_ids_for_requirements(
        &self,
        citation: &AgentCitationDto,
        flow_requirements: &[FlowRequirement],
        limit: usize,
    ) -> Vec<EdgeId> {
        let mut selected = self.proof_edge_ids_for_requirements(citation, flow_requirements);
        let selected_set = selected.iter().cloned().collect::<HashSet<_>>();
        let Some(graph) = self.graph.as_ref() else {
            return selected;
        };
        let admissible_call_ids = flow_requirements
            .iter()
            .filter(|requirement| packet_requirement_applies_to_citation(requirement, citation))
            .flat_map(|requirement| {
                self.proof_edge_ids_for_requirement(citation, requirement)
                    .into_iter()
            })
            .collect::<HashSet<_>>();
        let has_applicable_call_requirement = flow_requirements.iter().any(|requirement| {
            // R1(c): a formula naming a CALL typed-relation pattern guards
            // CALL context exactly as a legacy call-boundary requirement
            // does — only pattern-admissible CALL edges may ride the
            // citation's evidence list.
            if let Some(formula) = requirement.proof.formula() {
                return formula_requirement_typed_relation_patterns(formula, requirement.id)
                    .iter()
                    .any(|pattern| pattern.kind == EdgeKind::CALL);
            }
            requirement.evidence.citation_proves(citation)
                && (requirement
                    .evidence
                    .call_boundary_target(citation)
                    .is_some()
                    || requirement
                        .evidence
                        .ordered_call_boundary(citation)
                        .is_some())
        });
        // R1(c), extended to every atom-named kind: an edge whose kind a
        // formula pattern names must be pattern-admissible to ride this
        // citation's evidence list — an uncertain TYPE_USAGE or a wrong-kind
        // member stays graph context, exactly as owner-invalid CALLs do.
        let formula_pattern_kinds = flow_requirements
            .iter()
            .filter_map(|requirement| {
                requirement
                    .proof
                    .formula()
                    .map(|formula| (formula, requirement.id))
            })
            .flat_map(|(formula, requirement_id)| {
                formula_requirement_typed_relation_patterns(formula, requirement_id)
                    .into_iter()
                    .map(|pattern| pattern.kind)
            })
            .collect::<HashSet<_>>();
        let graph_edges = graph
            .edges
            .iter()
            .map(|edge| (&edge.id, edge))
            .collect::<std::collections::HashMap<_, _>>();
        let mut context = self
            .graph_provenance
            .iter()
            .map(|provenance| provenance.edge_id.clone())
            .filter(|edge_id| {
                graph_edges.get(edge_id).is_some_and(|edge| {
                    !selected_set.contains(edge_id)
                        && (edge.kind != EdgeKind::CALL
                            || !has_applicable_call_requirement
                            || admissible_call_ids.contains(edge_id))
                        && (!formula_pattern_kinds.contains(&edge.kind)
                            || admissible_call_ids.contains(edge_id))
                })
            })
            .collect::<Vec<_>>();
        context.sort_by(|left, right| left.0.cmp(&right.0));
        context.dedup();
        selected.extend(context);
        selected.dedup();
        selected.truncate(limit);
        selected
    }

    fn graph_for_requirements(
        &self,
        citation: &AgentCitationDto,
        flow_requirements: &[FlowRequirement],
    ) -> Option<GraphResponse> {
        let graph = self.graph.as_ref()?;
        let mut selected_edge_ids =
            self.proof_edge_ids_for_requirements(citation, flow_requirements);
        let selected_set = selected_edge_ids.iter().cloned().collect::<HashSet<_>>();
        let graph_edge_ids = graph
            .edges
            .iter()
            .map(|edge| &edge.id)
            .collect::<HashSet<_>>();
        let mut context = self
            .graph_provenance
            .iter()
            .map(|provenance| provenance.edge_id.clone())
            .filter(|edge_id| graph_edge_ids.contains(edge_id) && !selected_set.contains(edge_id))
            .collect::<Vec<_>>();
        context.sort_by(|left, right| left.0.cmp(&right.0));
        context.dedup();
        selected_edge_ids.extend(context);
        selected_edge_ids.dedup();
        selected_edge_ids.truncate(PACKET_CANDIDATE_GRAPH_EDGE_LIMIT);
        let selected_order = selected_edge_ids
            .iter()
            .enumerate()
            .map(|(index, edge_id)| (edge_id, index))
            .collect::<std::collections::HashMap<_, _>>();
        let mut edges = graph
            .edges
            .iter()
            .filter(|edge| selected_order.contains_key(&edge.id))
            .cloned()
            .collect::<Vec<_>>();
        edges.sort_by_key(|edge| selected_order[&edge.id]);
        if edges.is_empty() {
            return None;
        }

        let retained_node_ids = edges
            .iter()
            .flat_map(|edge| [edge.source.clone(), edge.target.clone()])
            .chain(std::iter::once(graph.center_id.clone()))
            .collect::<HashSet<_>>();
        let nodes = graph
            .nodes
            .iter()
            .filter(|node| retained_node_ids.contains(&node.id))
            .cloned()
            .collect::<Vec<_>>();
        let candidate_omitted = graph.edges.len().saturating_sub(edges.len());
        Some(GraphResponse {
            center_id: graph.center_id.clone(),
            nodes,
            edges,
            truncated: graph.truncated || candidate_omitted > 0,
            omitted_edge_count: graph
                .omitted_edge_count
                .saturating_add(u32::try_from(candidate_omitted).unwrap_or(u32::MAX)),
            canonical_layout: None,
        })
    }

    fn proof_edge_ids_for_requirement(
        &self,
        citation: &AgentCitationDto,
        requirement: &FlowRequirement,
    ) -> Vec<EdgeId> {
        let Some(graph) = self.graph.as_ref() else {
            return Vec::new();
        };
        let provenance_ids = self
            .graph_provenance
            .iter()
            .map(|provenance| &provenance.edge_id)
            .collect::<HashSet<_>>();
        // R1(c): formula-bearing requirements select edges by their
        // FlowProofSpec typed-relation patterns — single-receipt
        // classification facts only (edge kind, certainty gate, effective
        // target kind, callsite markers). `citation_proves` and the legacy
        // receipt validator serve Legacy requirements exclusively.
        if let Some(formula) = requirement.proof.formula() {
            let node_kinds = graph
                .nodes
                .iter()
                .map(|node| (node.id.0.as_str(), node.kind))
                .collect::<std::collections::HashMap<_, _>>();
            let patterns = formula_requirement_typed_relation_patterns(formula, requirement.id);
            if patterns.is_empty() {
                return Vec::new();
            }
            let mut matches = graph
                .edges
                .iter()
                .filter(|edge| provenance_ids.contains(&edge.id))
                .filter(|edge| edge.source == citation.node_id || edge.target == citation.node_id)
                .filter(|edge| {
                    patterns.iter().any(|pattern| {
                        edge_matches_typed_relation_pattern(pattern, edge, &node_kinds)
                    })
                })
                .map(|edge| edge.id.clone())
                .collect::<Vec<_>>();
            matches.sort_by(|left, right| left.0.cmp(&right.0));
            matches.dedup();
            return matches;
        }
        let mut matches = graph
            .edges
            .iter()
            .filter(|edge| provenance_ids.contains(&edge.id))
            .filter(|edge| {
                receipt_neighbor(graph, citation, edge).is_some_and(|(label, kind)| {
                    flow_requirement_call_receipt_is_valid(requirement, citation, edge, label, kind)
                })
            })
            .map(|edge| edge.id.clone())
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| left.0.cmp(&right.0));
        matches.dedup();
        matches
    }
}

/// Whether a flow requirement participates in this citation's edge selection.
/// Formula-bearing requirements always apply — their admissibility lives in
/// the typed-relation patterns, never in the `citation_proves` vocabulary
/// gate (R1(b,c)). Legacy requirements keep the vocabulary gate exactly.
fn packet_requirement_applies_to_citation(
    requirement: &FlowRequirement,
    citation: &AgentCitationDto,
) -> bool {
    requirement.proof.formula().is_some() || requirement.evidence.citation_proves(citation)
}

/// The typed-relation patterns of the atoms materially required by
/// `requirement_id`, in formula order.
fn formula_requirement_typed_relation_patterns(
    formula: &'static FlowProofFormula,
    requirement_id: &str,
) -> Vec<&'static TypedRelationPattern> {
    formula
        .atoms
        .iter()
        .filter(|atom| atom.requirement == requirement_id)
        .flat_map(|atom| atom.facts.iter())
        .filter_map(|fact| match fact {
            ProofFactPattern::TypedRelation(pattern) => Some(pattern),
            ProofFactPattern::SourceAspect(_)
            | ProofFactPattern::AbsentTypedRelation(_)
            | ProofFactPattern::AnchoredLineContainment(_) => None,
        })
        .collect()
}

/// Single-receipt admissibility of one live graph edge against one
/// typed-relation pattern — a candidate-level mirror of the matcher's
/// `typed_relation_admissible` (packet_proof_atoms), kept semantically
/// identical and pinned by a parity test through the public matcher API:
/// required kind, the rule-6 certainty gate attributed per kind (CALL and
/// TYPE_USAGE need `certain`; structural MEMBER/USAGE/IMPORT are exempt),
/// the effective target's node kind where the pattern names one, the
/// no-self-call clause, and shape-validated callsite markers. Role bindings
/// are the group matcher's job and are deliberately NOT checked here —
/// selection admits receipts, unification proves.
fn edge_matches_typed_relation_pattern(
    pattern: &TypedRelationPattern,
    edge: &codestory_contracts::api::GraphEdgeDto,
    node_kinds: &std::collections::HashMap<&str, NodeKind>,
) -> bool {
    edge.kind == pattern.kind
        && edge_certainty_gate_passes(edge.kind, edge.certainty.as_deref())
        && pattern
            .target_kind
            .is_none_or(|kind| node_kinds.get(edge.target.0.as_str()).copied() == Some(kind))
        && (!pattern.target_distinct_from_source || edge.source != edge.target)
        && edge_markers_satisfied(pattern.markers, edge.callsite_identity.as_deref())
}

/// Whether a pattern endpoint constrains a ROLE identity (rev 5.3 promotion
/// need-gate): `Role` and `AnyOfRoles` endpoints bind or guard identities the
/// formulas join on, so a matching edge's node id there is atom-needed; an
/// `Any` endpoint (e.g. M3's dispatch target) requires nothing.
fn promotion_endpoint_is_role_constrained(endpoint: ProofEndpointPattern) -> bool {
    !matches!(endpoint, ProofEndpointPattern::Any)
}

/// The formula ROLES one pattern endpoint names — the promotion slots that
/// endpoint's identities may be admitted through (round 5.5 item 2a). An
/// `AnyOfRoles` guard names every alternative it may hold; an `Any` endpoint
/// names none, which is why M3's unconstrained dispatch target can never
/// open a slot.
fn promotion_endpoint_roles(endpoint: ProofEndpointPattern) -> Vec<ProofRole> {
    match endpoint {
        ProofEndpointPattern::Role(role) => vec![role],
        ProofEndpointPattern::AnyOfRoles(roles) => roles.to_vec(),
        ProofEndpointPattern::Any => Vec::new(),
    }
}

/// The retirement decision over one group-matcher run (round 5.5 item 2b):
/// ONLY a `Proved` verdict retires. `Unproven` means the need is still live,
/// and `Aborted` means the search hit its step bound before it could answer
/// — neither is a proof, so neither may silence the need-gate.
fn retired_requirements_from_outcomes(
    outcomes: &[(&'static str, FlowProofOutcome)],
) -> Vec<&'static str> {
    outcomes
        .iter()
        .filter(|(_, outcome)| matches!(outcome, FlowProofOutcome::Proved(_)))
        .map(|(requirement, _)| *requirement)
        .collect()
}

/// Rule 6 attributed per kind, mirroring the matcher: structural MEMBER,
/// USAGE, and IMPORT edges are exempt; every other kind requires `certain`.
fn edge_certainty_gate_passes(kind: EdgeKind, certainty: Option<&str>) -> bool {
    match kind {
        EdgeKind::MEMBER | EdgeKind::USAGE | EdgeKind::IMPORT => true,
        _ => certainty == Some("certain"),
    }
}

/// Marker satisfaction, mirroring the matcher: a non-empty requirement list
/// with no identity fails closed; markers are the order-agnostic segments
/// after the canonical first segment.
fn edge_markers_satisfied(markers: &[CallsiteMarkerPattern], identity: Option<&str>) -> bool {
    if markers.is_empty() {
        return true;
    }
    let Some(identity) = identity else {
        return false;
    };
    markers.iter().all(|marker| match marker {
        CallsiteMarkerPattern::SyntaxCall => edge_syntax_marker_present(identity, "-call"),
        CallsiteMarkerPattern::SyntaxNew => edge_syntax_marker_present(identity, "-new"),
        CallsiteMarkerPattern::ReceiverOwner => edge_marker_segments(identity).any(|segment| {
            segment
                .strip_prefix("receiver-owner:")
                .is_some_and(|value| !value.is_empty())
        }),
        CallsiteMarkerPattern::LoopElementContainsCallsiteLine => {
            edge_loop_element_containment_holds(identity)
        }
    })
}

fn edge_marker_segments(identity: &str) -> impl Iterator<Item = &str> {
    identity.split('|').skip(1)
}

fn edge_syntax_marker_present(identity: &str, suffix: &str) -> bool {
    edge_marker_segments(identity).any(|segment| {
        segment
            .strip_prefix("syntax:")
            .and_then(|rest| rest.strip_suffix(suffix))
            .is_some_and(|language| !language.is_empty())
    })
}

/// Shape-validated canonical first segment (`file:line:col:target`, rule 5).
fn edge_canonical_callsite_line(identity: &str) -> Option<u32> {
    let first = identity.split('|').next()?;
    let fields = first.split(':').collect::<Vec<_>>();
    if fields.len() != 4 || fields.iter().any(|field| field.is_empty()) {
        return None;
    }
    let line = edge_parse_ascii_u32(fields[1])?;
    edge_parse_ascii_u32(fields[2])?;
    Some(line)
}

fn edge_parse_ascii_u32(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>().ok()
}

fn edge_loop_element_containment_holds(identity: &str) -> bool {
    let Some(line) = edge_canonical_callsite_line(identity) else {
        return false;
    };
    let mut contained = false;
    for segment in edge_marker_segments(identity) {
        let Some(range) = segment.strip_prefix("receiver-binding:loop-element@") else {
            continue;
        };
        let Some((start_text, end_text)) = range.split_once('-') else {
            return false;
        };
        let (Some(start), Some(end)) = (
            edge_parse_ascii_u32(start_text),
            edge_parse_ascii_u32(end_text),
        ) else {
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

fn receipt_neighbor<'a>(
    graph: &'a GraphResponse,
    citation: &AgentCitationDto,
    edge: &codestory_contracts::api::GraphEdgeDto,
) -> Option<(&'a str, codestory_contracts::api::NodeKind)> {
    let neighbor_id = if edge.source == citation.node_id {
        &edge.target
    } else if edge.target == citation.node_id {
        &edge.source
    } else {
        return None;
    };
    graph
        .nodes
        .iter()
        .find(|node| node.id == *neighbor_id)
        .map(|node| (node.label.as_str(), node.kind))
}

impl Deref for PacketSearchHit {
    type Target = SearchHit;

    fn deref(&self) -> &Self::Target {
        &self.hit
    }
}

/// Preserve one capped candidate view as one graph artifact. `GraphResponse::omitted_edge_count`
/// is artifact-local: it describes only the bounded source view fingerprinted into this artifact
/// and must not be summed across candidate artifacts. The ID is stable lineage for that original
/// view; a later output cap may remove a known retained edge and increment the local count without
/// changing lineage. Keeping overlapping views separate avoids inventing union arithmetic for
/// opaque omissions whose edge identities are unavailable.
#[cfg(test)]
pub(crate) fn merge_packet_candidate_graph(answer: &mut AgentAnswerDto, hit: &PacketSearchHit) {
    merge_packet_candidate_graph_for_requirements(answer, hit, &[]);
}

pub(crate) fn merge_packet_candidate_graph_for_requirements(
    answer: &mut AgentAnswerDto,
    hit: &PacketSearchHit,
    flow_requirements: &[FlowRequirement],
) {
    let citation = hit.citation_for_requirements(true, flow_requirements);
    let Some(candidate_graph) = hit.graph_for_requirements(&citation, flow_requirements) else {
        return;
    };
    let artifact_id = packet_candidate_selection_view_id(&candidate_graph);
    // R2: tie this candidate's trail scans to the immutable artifact lineage
    // so the proof-evidence extras builder can construct honest coverage
    // records after the caps run. First write wins with the lineage.
    if let Some(session) = active_packet_proof_session() {
        session.record_artifact_scans(&artifact_id, &hit.trail_scans);
    }
    if !answer.graphs.iter().any(|artifact| match artifact {
        GraphArtifactDto::Uml { id, .. } | GraphArtifactDto::Mermaid { id, .. } => {
            id == &artifact_id
        }
    }) {
        answer.graphs.push(GraphArtifactDto::Uml {
            id: artifact_id.clone(),
            title: "Packet search graph provenance".to_string(),
            graph: candidate_graph,
        });
    }
    if !answer.subgraph_ids.contains(&artifact_id) {
        answer.subgraph_ids.push(artifact_id);
    }
}

/// Immutable identity of the original bounded selection view, computed before any downstream
/// presentation cap. This is lineage, not a checksum of the graph's current serialized rows: a
/// later output cap may remove known rows and increase that view's omission count while the ID
/// remains stable. Replaying the same candidate therefore finds the existing lineage and must not
/// restore optional rows deliberately removed to meet the packet budget.
fn packet_candidate_selection_view_id(graph: &GraphResponse) -> String {
    let mut edge_ids = graph
        .edges
        .iter()
        .map(|edge| edge.id.0.as_str())
        .collect::<Vec<_>>();
    edge_ids.sort_unstable();

    let mut digest = Sha256::new();
    hash_graph_id_component(&mut digest, "immutable-candidate-selection-view-v1");
    hash_graph_id_component(&mut digest, &graph.center_id.0);
    for edge_id in edge_ids {
        hash_graph_id_component(&mut digest, edge_id);
    }
    digest.update([u8::from(graph.truncated)]);
    digest.update(graph.omitted_edge_count.to_le_bytes());
    let fingerprint = digest.finalize();
    format!("{PACKET_CANDIDATE_SELECTION_VIEW_ID}-{fingerprint:x}")
}

pub(crate) fn is_packet_candidate_selection_view_id(id: &str) -> bool {
    id.strip_prefix(PACKET_CANDIDATE_SELECTION_VIEW_ID_PREFIX)
        .is_some_and(|fingerprint| {
            fingerprint.len() == 64 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn hash_graph_id_component(digest: &mut Sha256, value: &str) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_le_bytes());
    digest.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::packet_budget::cap_packet_graph_edges_for_test;
    use codestory_agent::packet_flow_requirements::packet_flow_requirements_for_terms;
    use codestory_agent::packet_terms::packet_probe_terms;
    use codestory_contracts::api::{
        AgentRetrievalTraceDto, GraphEdgeDto, GraphNodeDto, NodeId, NodeKind,
        PacketEvidenceResolutionDto, PacketEvidenceTierDto, PacketTaskClassDto, SearchHitOrigin,
    };

    fn answer() -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "answer".into(),
            prompt: "prompt".into(),
            summary: "summary".into(),
            freshness: None,
            sections: Vec::new(),
            citations: Vec::new(),
            subgraph_ids: Vec::new(),
            retrieval_version: "sidecar".into(),
            graphs: Vec::new(),
            source_coverage: Vec::new(),
            retrieval_trace: AgentRetrievalTraceDto {
                request_id: "r".into(),
                retrieval_publication: None,
                resolved_profile: codestory_contracts::api::AgentRetrievalPresetDto::Architecture,
                policy_mode: codestory_contracts::api::AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 0,
                sla_target_ms: None,
                sla_missed: false,
                semantic_fallback_count: 0,
                semantic_fallbacks: Vec::new(),
                semantic_stage_timeout_zero_hits: 0,
                semantic_abstained_count: 0,
                annotations: Vec::new(),
                packet_claim_profile_telemetry: None,
                source_freshness_telemetry: None,
                steps: Vec::new(),
                packet_sidecar_diagnostics: Vec::new(),
                retrieval_shadow: None,
            },
        }
    }

    fn packet_hit(edge_id: &str) -> PacketSearchHit {
        let node_id = NodeId("2".into());
        PacketSearchHit {
            trail_scans: Vec::new(),
            hit: SearchHit {
                node_id: node_id.clone(),
                display_name: "Session.send".into(),
                kind: NodeKind::METHOD,
                file_path: Some("requests/sessions.py".into()),
                line: Some(1),
                score: 0.8,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                match_quality: None,
                evidence_tier: None,
                evidence_producer: None,
                resolution_status: None,
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: None,
                source_excerpt: None,
                verification_targets: Vec::new(),
                score_breakdown: None,
            },
            graph_provenance: vec![PacketGraphEdgeProvenance {
                edge_id: EdgeId(edge_id.into()),
                direction: PacketGraphDirection::Incoming,
                hop: 1,
                producers: vec!["scip_graph_projection".into()],
                certainty: Some("certain".into()),
            }],
            graph: Some(GraphResponse {
                center_id: node_id.clone(),
                nodes: [("1", "Session.request"), ("2", "Session.send")]
                    .into_iter()
                    .map(|(id, label)| GraphNodeDto {
                        id: NodeId(id.into()),
                        label: label.into(),
                        kind: NodeKind::METHOD,
                        depth: u32::from(id != "2"),
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: None,
                        qualified_name: None,
                        member_access: None,
                    })
                    .collect(),
                edges: vec![GraphEdgeDto {
                    id: EdgeId(edge_id.into()),
                    source: NodeId("1".into()),
                    target: node_id,
                    kind: EdgeKind::CALL,
                    confidence: Some(1.0),
                    certainty: Some("certain".into()),
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            }),
        }
    }

    fn server_requirement(id: &str) -> FlowRequirement {
        let terms = packet_probe_terms(
            "Trace how a server application registers middleware, handles a request, and sends the response.",
        );
        packet_flow_requirements_for_terms(&terms, PacketTaskClassDto::RouteTracing)
            .into_iter()
            .find(|requirement| requirement.id == id)
            .unwrap_or_else(|| panic!("missing server requirement {id}"))
    }

    fn boundary_hit(
        carrier: &str,
        target_label: &str,
        callsite_identity: Option<&str>,
        certainty: Option<&str>,
        outgoing: bool,
    ) -> PacketSearchHit {
        let center_id = NodeId("carrier".into());
        let neighbor_id = NodeId("neighbor".into());
        let (source, target) = if outgoing {
            (center_id.clone(), neighbor_id.clone())
        } else {
            (neighbor_id.clone(), center_id.clone())
        };
        PacketSearchHit {
            trail_scans: Vec::new(),
            hit: SearchHit {
                node_id: center_id.clone(),
                display_name: carrier.into(),
                kind: NodeKind::METHOD,
                file_path: Some("src/server.js".into()),
                line: Some(10),
                score: 0.8,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                match_quality: None,
                evidence_tier: Some(PacketEvidenceTierDto::LexicalSource),
                evidence_producer: Some("symbol_doc".into()),
                resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: Some(true),
                source_excerpt: None,
                verification_targets: Vec::new(),
                score_breakdown: None,
            },
            graph_provenance: vec![PacketGraphEdgeProvenance {
                edge_id: EdgeId("boundary".into()),
                direction: if outgoing {
                    PacketGraphDirection::Outgoing
                } else {
                    PacketGraphDirection::Incoming
                },
                hop: 1,
                producers: vec!["core_incident_call".into()],
                certainty: certainty.map(str::to_string),
            }],
            graph: Some(GraphResponse {
                center_id: center_id.clone(),
                nodes: vec![
                    GraphNodeDto {
                        id: center_id,
                        label: carrier.into(),
                        kind: NodeKind::METHOD,
                        depth: 0,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("src/server.js".into()),
                        qualified_name: Some(carrier.into()),
                        member_access: None,
                    },
                    GraphNodeDto {
                        id: neighbor_id,
                        label: target_label.into(),
                        kind: if certainty == Some("certain") {
                            NodeKind::METHOD
                        } else {
                            NodeKind::UNKNOWN
                        },
                        depth: 1,
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("src/server.js".into()),
                        qualified_name: None,
                        member_access: None,
                    },
                ],
                edges: vec![GraphEdgeDto {
                    id: EdgeId("boundary".into()),
                    source,
                    target,
                    kind: EdgeKind::CALL,
                    confidence: certainty.map(|_| 1.0),
                    certainty: certainty.map(str::to_string),
                    callsite_identity: callsite_identity.map(str::to_string),
                    candidate_targets: Vec::new(),
                }],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            }),
        }
    }

    fn overlapping_candidate_hit(
        center: &str,
        edge_specs: &[(&str, &str, &str)],
        omitted_edge_count: u32,
    ) -> PacketSearchHit {
        let center_id = NodeId(center.into());
        let mut node_ids = edge_specs
            .iter()
            .flat_map(|(_, source, target)| [*source, *target])
            .collect::<Vec<_>>();
        node_ids.sort_unstable();
        node_ids.dedup();
        let edges = edge_specs
            .iter()
            .map(|(id, source, target)| GraphEdgeDto {
                id: EdgeId((*id).into()),
                source: NodeId((*source).into()),
                target: NodeId((*target).into()),
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".into()),
                callsite_identity: None,
                candidate_targets: Vec::new(),
            })
            .collect::<Vec<_>>();
        PacketSearchHit {
            trail_scans: Vec::new(),
            hit: SearchHit {
                node_id: center_id.clone(),
                display_name: center.into(),
                kind: NodeKind::METHOD,
                file_path: Some("src/overlap.js".into()),
                line: Some(1),
                score: 0.8,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                match_quality: None,
                evidence_tier: Some(PacketEvidenceTierDto::ResolvedGraph),
                evidence_producer: Some("core_incident_call".into()),
                resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: Some(true),
                source_excerpt: None,
                verification_targets: Vec::new(),
                score_breakdown: None,
            },
            graph_provenance: edges
                .iter()
                .map(|edge| PacketGraphEdgeProvenance {
                    edge_id: edge.id.clone(),
                    direction: if edge.source == center_id {
                        PacketGraphDirection::Outgoing
                    } else {
                        PacketGraphDirection::Incoming
                    },
                    hop: 1,
                    producers: vec!["core_incident_call".into()],
                    certainty: edge.certainty.clone(),
                })
                .collect(),
            graph: Some(GraphResponse {
                center_id: center_id.clone(),
                nodes: node_ids
                    .into_iter()
                    .map(|id| GraphNodeDto {
                        id: NodeId(id.into()),
                        label: id.into(),
                        kind: NodeKind::METHOD,
                        depth: u32::from(id != center),
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: Some("src/overlap.js".into()),
                        qualified_name: Some(id.into()),
                        member_access: None,
                    })
                    .collect(),
                edges,
                truncated: omitted_edge_count > 0,
                omitted_edge_count,
                canonical_layout: None,
            }),
        }
    }

    fn mapper_requirement(id: &str) -> FlowRequirement {
        let terms =
            packet_probe_terms("How does the mapper build its configuration and execution plan?");
        packet_flow_requirements_for_terms(&terms, PacketTaskClassDto::ArchitectureExplanation)
            .into_iter()
            .find(|requirement| requirement.id == id)
            .unwrap_or_else(|| panic!("missing mapper requirement {id}"))
    }

    fn typed_edge(
        id: &str,
        source: &str,
        target: &str,
        kind: EdgeKind,
        certainty: Option<&str>,
        callsite_identity: Option<&str>,
    ) -> GraphEdgeDto {
        GraphEdgeDto {
            id: EdgeId(id.into()),
            source: NodeId(source.into()),
            target: NodeId(target.into()),
            kind,
            confidence: None,
            certainty: certainty.map(str::to_string),
            callsite_identity: callsite_identity.map(str::to_string),
            candidate_targets: Vec::new(),
        }
    }

    fn typed_hit(
        center: &str,
        nodes: &[(&str, NodeKind)],
        edges: Vec<GraphEdgeDto>,
    ) -> PacketSearchHit {
        let graph_provenance = edges
            .iter()
            .map(|edge| PacketGraphEdgeProvenance {
                edge_id: edge.id.clone(),
                direction: if edge.source.0 == center {
                    PacketGraphDirection::Outgoing
                } else {
                    PacketGraphDirection::Incoming
                },
                hop: 1,
                producers: vec!["atom_trail_hydration".into()],
                certainty: edge.certainty.clone(),
            })
            .collect();
        PacketSearchHit {
            trail_scans: Vec::new(),
            hit: SearchHit {
                node_id: NodeId(center.into()),
                display_name: "Widget".into(),
                kind: NodeKind::CLASS,
                file_path: Some("src/widget.cs".into()),
                line: Some(4),
                score: 0.7,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                match_quality: None,
                evidence_tier: Some(PacketEvidenceTierDto::ResolvedGraph),
                evidence_producer: Some("atom_trail_hydration".into()),
                resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: Some(true),
                source_excerpt: None,
                verification_targets: Vec::new(),
                score_breakdown: None,
            },
            graph_provenance,
            graph: Some(GraphResponse {
                center_id: NodeId(center.into()),
                nodes: nodes
                    .iter()
                    .map(|(id, kind)| GraphNodeDto {
                        id: NodeId((*id).into()),
                        label: (*id).into(),
                        kind: *kind,
                        depth: u32::from(*id != center),
                        label_policy: None,
                        badge_visible_members: None,
                        badge_total_members: None,
                        merged_symbol_examples: Vec::new(),
                        file_path: None,
                        qualified_name: None,
                        member_access: None,
                    })
                    .collect(),
                edges,
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            }),
        }
    }

    /// R1(b,c): formula-bearing requirements select edges by their atom
    /// typed-relation patterns — negative first: an uncertain TYPE_USAGE
    /// receipt is never selected; the vocabulary-free citation then proves
    /// the certain receipt is selected without `citation_proves`.
    #[test]
    fn formula_requirements_select_edges_by_atom_patterns_not_vocabulary() {
        let requirement = mapper_requirement("mapper_config");
        assert!(
            requirement.proof.formula().is_some(),
            "mapper_config must be formula-bearing"
        );
        // The carrier's display name is deliberately outside every mapper
        // vocabulary list, so any selection can only come from the patterns.
        assert!(!requirement.evidence.citation_proves(
            &codestory_agent::citation::to_citation_from_hit(
                &typed_hit("builder-1", &[("builder-1", NodeKind::CLASS)], Vec::new()).hit,
                None,
                None,
                true,
            )
        ));

        let uncertain = typed_hit(
            "builder-1",
            &[
                ("builder-1", NodeKind::CLASS),
                ("config-1", NodeKind::CLASS),
            ],
            vec![typed_edge(
                "uses-config",
                "builder-1",
                "config-1",
                EdgeKind::TYPE_USAGE,
                None,
                None,
            )],
        );
        let citation =
            uncertain.citation_for_requirements(true, std::slice::from_ref(&requirement));
        assert!(
            citation.evidence_edge_ids.is_empty(),
            "the rule-6 certainty gate must fail an uncertain TYPE_USAGE receipt closed"
        );

        let mut certain = uncertain.clone();
        certain.graph.as_mut().expect("graph").edges[0].certainty = Some("certain".into());
        certain.graph_provenance[0].certainty = Some("certain".into());
        let citation = certain.citation_for_requirements(true, std::slice::from_ref(&requirement));
        assert_eq!(
            citation.evidence_edge_ids,
            [EdgeId("uses-config".into())],
            "a certain TYPE_USAGE receipt is the mapper_config atom pattern"
        );

        // mapper_execution's MEMBER pattern names METHOD as the effective
        // target kind: a FIELD member never satisfies it, a METHOD does.
        let execution = mapper_requirement("mapper_execution");
        let field_member = typed_hit(
            "builder-1",
            &[
                ("builder-1", NodeKind::CLASS),
                ("helper-1", NodeKind::FIELD),
            ],
            vec![typed_edge(
                "member-edge",
                "builder-1",
                "helper-1",
                EdgeKind::MEMBER,
                None,
                None,
            )],
        );
        let citation =
            field_member.citation_for_requirements(true, std::slice::from_ref(&execution));
        assert!(citation.evidence_edge_ids.is_empty());
        let mut method_member = field_member.clone();
        method_member.graph.as_mut().expect("graph").nodes[1].kind = NodeKind::METHOD;
        let citation =
            method_member.citation_for_requirements(true, std::slice::from_ref(&execution));
        assert_eq!(citation.evidence_edge_ids, [EdgeId("member-edge".into())]);
    }

    /// Parity pin: the candidate-level pattern mirror agrees with the public
    /// matcher on every single-fact atom shape, so the two admissibility
    /// paths cannot drift apart silently.
    #[test]
    fn edge_pattern_mirror_agrees_with_the_atom_matcher() {
        use codestory_agent::packet_proof_atoms::{
            FlowProofOutcome, LOG_HANDLER_FLOW_PROOF, MAPPER_PLAN_FLOW_PROOF, PacketProofEvidence,
            ProofAtomId, VerifiedTypedRelationReceipt, match_required_atoms,
        };

        let single_fact_pattern = |formula: &'static FlowProofFormula,
                                   atom_id: ProofAtomId|
         -> &'static TypedRelationPattern {
            let atom = formula
                .atoms
                .iter()
                .find(|atom| atom.id == atom_id)
                .expect("atom");
            assert_eq!(atom.facts.len(), 1, "parity requires single-fact atoms");
            match &atom.facts[0] {
                ProofFactPattern::TypedRelation(pattern) => pattern,
                other => panic!("expected typed-relation fact, got {other:?}"),
            }
        };

        let call =
            |certainty: Option<&str>, identity: Option<&str>, target_kind, self_call: bool| {
                let target = if self_call { "owner-1" } else { "handler-1" };
                (
                    typed_edge(
                        "edge-1",
                        "owner-1",
                        target,
                        EdgeKind::CALL,
                        certainty,
                        identity,
                    ),
                    [("owner-1", NodeKind::METHOD), (target, target_kind)]
                        .into_iter()
                        .collect::<std::collections::HashMap<_, _>>(),
                )
            };
        let m3_identity = "app/log.php:10:5:handle|syntax:php-call|receiver-owner:handler";
        let cases: Vec<(
            &'static FlowProofFormula,
            ProofAtomId,
            GraphEdgeDto,
            std::collections::HashMap<&str, NodeKind>,
        )> = vec![
            // M3 positive and each negative clause.
            {
                let (edge, kinds) =
                    call(Some("certain"), Some(m3_identity), NodeKind::METHOD, false);
                (&LOG_HANDLER_FLOW_PROOF, ProofAtomId::M3, edge, kinds)
            },
            {
                let (edge, kinds) = call(None, Some(m3_identity), NodeKind::METHOD, false);
                (&LOG_HANDLER_FLOW_PROOF, ProofAtomId::M3, edge, kinds)
            },
            {
                let (edge, kinds) = call(
                    Some("certain"),
                    Some("app/log.php:10:5:handle|syntax:php-call"),
                    NodeKind::METHOD,
                    false,
                );
                (&LOG_HANDLER_FLOW_PROOF, ProofAtomId::M3, edge, kinds)
            },
            {
                let (edge, kinds) =
                    call(Some("certain"), Some(m3_identity), NodeKind::CLASS, false);
                (&LOG_HANDLER_FLOW_PROOF, ProofAtomId::M3, edge, kinds)
            },
            {
                let (edge, kinds) =
                    call(Some("certain"), Some(m3_identity), NodeKind::METHOD, true);
                (&LOG_HANDLER_FLOW_PROOF, ProofAtomId::M3, edge, kinds)
            },
            // M1b: construction marker, target unconstrained.
            {
                let (edge, kinds) = call(
                    Some("certain"),
                    Some("app/log.php:10:5:Handler|syntax:php-new"),
                    NodeKind::CLASS,
                    false,
                );
                (&LOG_HANDLER_FLOW_PROOF, ProofAtomId::M1b, edge, kinds)
            },
            {
                let (edge, kinds) = call(
                    Some("certain"),
                    Some("app/log.php:10:5:Handler|syntax:-new"),
                    NodeKind::CLASS,
                    false,
                );
                (&LOG_HANDLER_FLOW_PROOF, ProofAtomId::M1b, edge, kinds)
            },
            // M2: loop-element containment — contained, outside, malformed
            // range, malformed canonical segment.
            {
                let (edge, kinds) = call(
                    Some("certain"),
                    Some(
                        "app/log.php:10:5:handle|syntax:php-call|receiver-owner:h|receiver-binding:loop-element@8-14",
                    ),
                    NodeKind::METHOD,
                    false,
                );
                (&LOG_HANDLER_FLOW_PROOF, ProofAtomId::M2, edge, kinds)
            },
            {
                let (edge, kinds) = call(
                    Some("certain"),
                    Some(
                        "app/log.php:20:5:handle|syntax:php-call|receiver-owner:h|receiver-binding:loop-element@8-14",
                    ),
                    NodeKind::METHOD,
                    false,
                );
                (&LOG_HANDLER_FLOW_PROOF, ProofAtomId::M2, edge, kinds)
            },
            {
                let (edge, kinds) = call(
                    Some("certain"),
                    Some(
                        "app/log.php:10:5:handle|syntax:php-call|receiver-owner:h|receiver-binding:loop-element@14-8",
                    ),
                    NodeKind::METHOD,
                    false,
                );
                (&LOG_HANDLER_FLOW_PROOF, ProofAtomId::M2, edge, kinds)
            },
            {
                let (edge, kinds) = call(
                    Some("certain"),
                    Some(
                        "app/log.php:x:5:handle|syntax:php-call|receiver-owner:h|receiver-binding:loop-element@8-14",
                    ),
                    NodeKind::METHOD,
                    false,
                );
                (&LOG_HANDLER_FLOW_PROOF, ProofAtomId::M2, edge, kinds)
            },
            // A1: certainty-gated TYPE_USAGE.
            {
                let edge = typed_edge(
                    "edge-1",
                    "builder-1",
                    "config-1",
                    EdgeKind::TYPE_USAGE,
                    Some("certain"),
                    None,
                );
                let kinds = [
                    ("builder-1", NodeKind::CLASS),
                    ("config-1", NodeKind::CLASS),
                ]
                .into_iter()
                .collect();
                (&MAPPER_PLAN_FLOW_PROOF, ProofAtomId::A1, edge, kinds)
            },
            {
                let edge = typed_edge(
                    "edge-1",
                    "builder-1",
                    "config-1",
                    EdgeKind::TYPE_USAGE,
                    Some("probable"),
                    None,
                );
                let kinds = [
                    ("builder-1", NodeKind::CLASS),
                    ("config-1", NodeKind::CLASS),
                ]
                .into_iter()
                .collect();
                (&MAPPER_PLAN_FLOW_PROOF, ProofAtomId::A1, edge, kinds)
            },
        ];

        let mut positive = 0usize;
        for (formula, atom_id, edge, kinds) in cases {
            let pattern = single_fact_pattern(formula, atom_id);
            let mirror = edge_matches_typed_relation_pattern(pattern, &edge, &kinds);
            let receipt = VerifiedTypedRelationReceipt::from_graph_edge(
                &edge,
                kinds.get(edge.target.0.as_str()).copied(),
            );
            let evidence = PacketProofEvidence {
                typed_relations: vec![receipt],
                ..PacketProofEvidence::default()
            };
            let matcher = matches!(
                match_required_atoms(formula, &[atom_id], &evidence),
                FlowProofOutcome::Proved(_)
            );
            assert_eq!(
                mirror, matcher,
                "mirror and matcher disagree on {atom_id:?} for {edge:?}"
            );
            positive += usize::from(mirror);
        }
        assert!(positive >= 4, "the battery must include real positives");

        // Ride-along (F3 finding 10): C-formula patterns live in multi-fact
        // atoms, so parity uses a scaffold — the atom's OTHER facts are held
        // by fixed receipts (plus the anchored source receipt the atom
        // requires) and only the receipt under test varies. Variations stay
        // role-consistent on their endpoints by construction: the mirror is
        // single-receipt classification and role unification is deliberately
        // the group matcher's job, so a role-inconsistent edge is outside
        // the parity contract.
        use codestory_agent::packet_proof_atoms::{
            CSS_ANIMATION_FLOW_PROOF, SourceAspectKind, VerifiedSourceAspectReceipt,
        };
        let c_pattern =
            |atom_id: ProofAtomId, fact_index: usize| -> &'static TypedRelationPattern {
                let atom = CSS_ANIMATION_FLOW_PROOF
                    .atoms
                    .iter()
                    .find(|atom| atom.id == atom_id)
                    .expect("atom");
                match &atom.facts[fact_index] {
                    ProofFactPattern::TypedRelation(pattern) => pattern,
                    other => panic!("expected typed-relation fact, got {other:?}"),
                }
            };
        let anchored = |node: &str, atom: ProofAtomId| VerifiedSourceAspectReceipt {
            kind: SourceAspectKind::VerifiedCarrierRange,
            owner: NodeId(node.into()),
            symbol_id: Some(NodeId(node.into())),
            start_line: Some(3),
            end_line: Some(3),
            atom_anchor: Some(atom),
        };
        let css_kinds: std::collections::HashMap<&str, NodeKind> = [
            ("entry", NodeKind::FILE),
            ("vars", NodeKind::FILE),
            ("anim", NodeKind::FILE),
            ("var-node", NodeKind::VARIABLE),
            ("kf", NodeKind::FUNCTION),
            ("sa", NodeKind::CONSTANT),
        ]
        .into_iter()
        .collect();
        let as_receipt =
            |edge: &GraphEdgeDto, kinds: &std::collections::HashMap<&str, NodeKind>| {
                VerifiedTypedRelationReceipt::from_graph_edge(
                    edge,
                    kinds.get(edge.target.0.as_str()).copied(),
                )
            };
        // (atom, fact index of the pattern under test, scaffold edges,
        // anchored receipts, edge under test with an optional node-kind
        // override for its target)
        let member_vars_var = typed_edge("m-var", "vars", "var-node", EdgeKind::MEMBER, None, None);
        let import_entry_vars = typed_edge("i-vars", "entry", "vars", EdgeKind::IMPORT, None, None);
        let import_entry_anim = typed_edge("i-anim", "entry", "anim", EdgeKind::IMPORT, None, None);
        let member_anim_kf = typed_edge("m-kf", "anim", "kf", EdgeKind::MEMBER, None, None);
        let member_anim_sa = typed_edge("m-sa", "anim", "sa", EdgeKind::MEMBER, None, None);
        let usage_sa_kf = typed_edge("u-kf", "sa", "kf", EdgeKind::USAGE, None, None);
        let c_cases: Vec<(
            ProofAtomId,
            usize,
            Vec<&GraphEdgeDto>,
            Vec<VerifiedSourceAspectReceipt>,
            GraphEdgeDto,
            Option<(&str, NodeKind)>,
        )> = vec![
            // C2 IMPORT pattern: FILE target passes (uncertain is exempt —
            // the structural certainty pin), a VARIABLE target fails.
            (
                ProofAtomId::C2,
                0,
                vec![&member_vars_var],
                vec![anchored("var-node", ProofAtomId::C2)],
                import_entry_vars.clone(),
                None,
            ),
            (
                ProofAtomId::C2,
                0,
                vec![&member_vars_var],
                vec![anchored("var-node", ProofAtomId::C2)],
                import_entry_vars.clone(),
                Some(("vars", NodeKind::VARIABLE)),
            ),
            // C2 MEMBER pattern: VARIABLE target passes, CONSTANT fails.
            (
                ProofAtomId::C2,
                1,
                vec![&import_entry_vars],
                vec![anchored("var-node", ProofAtomId::C2)],
                member_vars_var.clone(),
                None,
            ),
            (
                ProofAtomId::C2,
                1,
                vec![&import_entry_vars],
                vec![anchored("var-node", ProofAtomId::C2)],
                member_vars_var.clone(),
                Some(("var-node", NodeKind::CONSTANT)),
            ),
            // C4 USAGE pattern: FUNCTION target passes (uncertain exempt),
            // a CONSTANT-reported target fails.
            (
                ProofAtomId::C4,
                4,
                vec![&import_entry_anim, &member_anim_kf, &member_anim_sa],
                vec![anchored("kf", ProofAtomId::C4)],
                usage_sa_kf.clone(),
                None,
            ),
            (
                ProofAtomId::C4,
                4,
                vec![&import_entry_anim, &member_anim_kf, &member_anim_sa],
                vec![anchored("kf", ProofAtomId::C4)],
                usage_sa_kf.clone(),
                Some(("kf", NodeKind::CONSTANT)),
            ),
        ];
        let mut c_positive = 0usize;
        for (atom_id, fact_index, scaffold, anchors, edge, kind_override) in c_cases {
            let mut kinds = css_kinds.clone();
            if let Some((node, kind)) = kind_override {
                kinds.insert(node, kind);
            }
            let pattern = c_pattern(atom_id, fact_index);
            let mirror = edge_matches_typed_relation_pattern(pattern, &edge, &kinds);
            let mut typed_relations = scaffold
                .iter()
                .map(|scaffold_edge| as_receipt(scaffold_edge, &kinds))
                .collect::<Vec<_>>();
            typed_relations.push(as_receipt(&edge, &kinds));
            let evidence = PacketProofEvidence {
                typed_relations,
                source_aspects: anchors,
                ..PacketProofEvidence::default()
            };
            let matcher = matches!(
                match_required_atoms(&CSS_ANIMATION_FLOW_PROOF, &[atom_id], &evidence),
                FlowProofOutcome::Proved(_)
            );
            assert_eq!(
                mirror, matcher,
                "mirror and matcher disagree on {atom_id:?} fact {fact_index} for {edge:?}"
            );
            c_positive += usize::from(mirror);
        }
        assert_eq!(
            c_positive, 3,
            "each C pattern under test must have exactly one passing variation"
        );
    }

    /// R2: the hydration spec derives exclusively from the formula atoms'
    /// edge kinds — Legacy-only requirement sets stay empty, and no kind is
    /// widened that the task class's atoms do not name.
    #[test]
    fn hydration_spec_is_derived_from_formula_atom_kinds_only() {
        let server = packet_flow_requirements_for_terms(
            &packet_probe_terms(
                "Trace how a server application registers middleware, handles a request, and sends the response.",
            ),
            PacketTaskClassDto::RouteTracing,
        );
        let legacy_spec = packet_atom_hydration_spec(&server);
        assert!(
            legacy_spec.is_empty(),
            "Legacy-only requirements must not widen hydration"
        );
        assert!(
            legacy_spec.promotion_patterns.is_empty(),
            "Legacy-only requirements must derive no promotion patterns (rev 5.3 inertness)"
        );

        let mapper = packet_flow_requirements_for_terms(
            &packet_probe_terms("How does the mapper build its configuration and execution plan?"),
            PacketTaskClassDto::ArchitectureExplanation,
        );
        let spec = packet_atom_hydration_spec(&mapper);
        assert!(!spec.file_structural, "A formulas never name FILE trails");
        assert!(
            spec.kinds_for_root(NodeKind::CLASS)
                .contains(&EdgeKind::TYPE_USAGE)
        );
        assert!(
            spec.kinds_for_root(NodeKind::CLASS)
                .contains(&EdgeKind::MEMBER)
        );
        assert!(
            !spec
                .kinds_for_root(NodeKind::VARIABLE)
                .contains(&EdgeKind::USAGE),
            "no atom names USAGE for the mapper task"
        );
        assert!(
            spec.absence_kinds.is_empty(),
            "the A formulas carry no absence facts"
        );
        assert_eq!(
            spec.identity_trail_kinds_for_root(NodeKind::CLASS),
            vec![EdgeKind::TYPE_USAGE],
            "A-family CLASS roots run TYPE_USAGE identity trails only (gate 5c: \
             MEMBER feeds nothing under rev 5.4 and its fanout shares the \
             trail edge budget)"
        );
        assert!(
            spec.promotion_patterns
                .iter()
                .any(|pattern| pattern.kind == EdgeKind::TYPE_USAGE),
            "the A formulas' TYPE_USAGE pattern feeds the need-gate"
        );
        assert!(
            spec.promotion_patterns
                .iter()
                .all(|pattern| pattern.kind == EdgeKind::TYPE_USAGE),
            "rev 5.4: A3's CALL and MEMBER patterns never drive admission"
        );

        let css = packet_flow_requirements_for_terms(
            &packet_probe_terms(
                "Trace how the css animation keyframes and custom property variables are declared and used by the base selectors in the imported stylesheets.",
            ),
            PacketTaskClassDto::ArchitectureExplanation,
        );
        let spec = packet_atom_hydration_spec(&css);
        assert!(
            spec.file_structural,
            "C formulas name MEMBER/USAGE/IMPORT, so FILE roots hydrate structurally"
        );
        assert_eq!(
            spec.absence_kinds,
            vec![EdgeKind::USAGE],
            "C3's absence subject is the only absence kind"
        );
        assert!(
            spec.identity_trail_kinds_for_root(NodeKind::CONSTANT)
                .is_empty(),
            "structural roots run no in-loop identity trails (gate 5c): \
             MEMBER/USAGE feed nothing under rev 5.4"
        );
        assert!(
            !spec.promotion_patterns.is_empty()
                && spec
                    .promotion_patterns
                    .iter()
                    .all(|pattern| pattern.kind == EdgeKind::IMPORT),
            "rev 5.4: only the C IMPORT patterns drive admission — never MEMBER/USAGE"
        );
        assert!(
            spec.kinds_for_root(NodeKind::VARIABLE)
                .contains(&EdgeKind::USAGE)
        );
        assert!(
            spec.kinds_for_root(NodeKind::CONSTANT)
                .contains(&EdgeKind::MEMBER)
        );
        assert!(
            !spec
                .kinds_for_root(NodeKind::CLASS)
                .contains(&EdgeKind::TYPE_USAGE),
            "no C atom names TYPE_USAGE"
        );
    }

    /// The session ledger keys scans by artifact lineage: the first merge
    /// records them, an exact replay does not duplicate them, and sessions
    /// never leak outside their guard.
    #[test]
    fn merge_records_trail_scans_into_the_active_session_once() {
        let mut hit = packet_hit("edge-1");
        hit.trail_scans = vec![PacketCandidateTrailScan {
            root: "2".into(),
            direction: PacketGraphDirection::Outgoing,
            depth: 1,
            edge_kinds: vec![EdgeKind::CALL],
            truncated: false,
            coverage_edge_ids: vec![EdgeId("edge-1".into())],
        }];
        let session = Rc::new(PacketProofSession::new(PacketAtomHydrationSpec::default()));
        {
            let _guard = install_packet_proof_session(Rc::clone(&session));
            let mut answer = answer();
            merge_packet_candidate_graph(&mut answer, &hit);
            merge_packet_candidate_graph(&mut answer, &hit);
        }
        let ledger = session.artifact_scans();
        assert_eq!(ledger.len(), 1, "replays must not duplicate scan records");
        assert_eq!(ledger[0].1, hit.trail_scans);
        assert!(
            active_packet_proof_session().is_none(),
            "the guard must uninstall the session"
        );

        // Without a session nothing is recorded anywhere.
        let unscoped = Rc::new(PacketProofSession::new(PacketAtomHydrationSpec::default()));
        let mut answer = answer();
        merge_packet_candidate_graph(&mut answer, &hit);
        assert!(unscoped.artifact_scans().is_empty());
    }

    /// Round 5.5 item 2a: the per-query promotion SLOTS are the distinct
    /// role endpoints of the formulas' cross-container patterns — derived
    /// from the atoms, never a constant. A yields two (Builder, ConfigType),
    /// C yields four (Entrypoint plus the three source roles), and the M
    /// family and all-Legacy packets yield NONE, which is what makes their
    /// admission structurally unable to promote.
    #[test]
    fn promotion_role_slots_are_derived_from_the_cross_container_atom_endpoints() {
        let legacy = packet_atom_hydration_spec(&packet_flow_requirements_for_terms(
            &packet_probe_terms(
                "Trace how a server application registers middleware, handles a request, and sends the response.",
            ),
            PacketTaskClassDto::RouteTracing,
        ));
        assert!(
            legacy.promotion_role_slots().is_empty(),
            "all-Legacy packets have no slot at all — promotion cannot be expressed"
        );

        let m = packet_atom_hydration_spec(&packet_flow_requirements_for_terms(
            &packet_probe_terms(
                "Trace how the logger creates a log record and dispatches it to each handler for processing.",
            ),
            PacketTaskClassDto::ArchitectureExplanation,
        ));
        assert!(
            m.promotion_role_slots().is_empty(),
            "the M formulas name only CALL — no cross-container pattern, no slot"
        );

        let a = packet_atom_hydration_spec(&packet_flow_requirements_for_terms(
            &packet_probe_terms("How does the mapper build its configuration and execution plan?"),
            PacketTaskClassDto::ArchitectureExplanation,
        ));
        assert_eq!(
            a.promotion_role_slots(),
            vec![ProofRole::Builder, ProofRole::ConfigType],
            "A1's TYPE_USAGE endpoints are the A-shard's two slots"
        );
        assert!(
            a.promotion_patterns
                .iter()
                .all(|pattern| pattern.requirement == "mapper_config"),
            "the A promotion pattern belongs to the requirement retirement retires"
        );

        let c = packet_atom_hydration_spec(&packet_flow_requirements_for_terms(
            &packet_probe_terms(
                "Trace how the css animation keyframes and custom property variables are declared and used by the base selectors in the imported stylesheets.",
            ),
            PacketTaskClassDto::ArchitectureExplanation,
        ));
        assert_eq!(
            c.promotion_role_slots(),
            vec![
                ProofRole::Entrypoint,
                ProofRole::VarsSource,
                ProofRole::BaseSource,
                ProofRole::AnimSource,
            ],
            "the C IMPORT patterns name the entrypoint plus the three source roles"
        );
        assert_eq!(
            c.formula_receipt_kinds(),
            vec![EdgeKind::IMPORT, EdgeKind::MEMBER, EdgeKind::USAGE],
            "the retirement checkpoint reads only the formulas' fact kinds"
        );
    }

    /// Round 5.5 item 2b, fail-closed core: ONLY a `Proved` verdict retires.
    /// An `Aborted` checkpoint — the matcher's step bound, not an answer —
    /// retires NOTHING, and neither does `Unproven`.
    #[test]
    fn an_aborted_or_unproven_checkpoint_retires_nothing() {
        let proved =
            FlowProofOutcome::Proved(codestory_agent::packet_proof_atoms::VerifiedFlowProof {
                bindings: std::collections::BTreeMap::new(),
                atoms: Vec::new(),
            });
        assert_eq!(
            retired_requirements_from_outcomes(&[
                ("aborted_requirement", FlowProofOutcome::Aborted),
                ("unproven_requirement", FlowProofOutcome::Unproven),
                ("proved_requirement", proved),
            ]),
            vec!["proved_requirement"],
            "an aborted or unproven verdict is not a proof and may not silence the need-gate"
        );
        assert!(
            retired_requirements_from_outcomes(&[
                ("a", FlowProofOutcome::Aborted),
                ("b", FlowProofOutcome::Unproven),
            ])
            .is_empty(),
            "no proof, no retirement"
        );
    }

    /// Gate 6 — NEED-SET PRIORITY BY ATOM-ROLE MULTIPLICITY. The score is
    /// the count of distinct (requirement, role) positions an identity
    /// occupies, so a chain identity standing in two positions of the
    /// requirement group outranks a lone endpoint. Non-cross-container
    /// positions (A3's CALL and MEMBER roles) COUNT toward the score but
    /// still add no member and open no slot — ordering the need-set can
    /// never widen it (rev 5.4 membership restriction, held).
    #[test]
    fn promotion_priority_counts_distinct_requirement_role_positions() {
        let mapper = packet_flow_requirements_for_terms(
            &packet_probe_terms("How does the mapper build its configuration and execution plan?"),
            PacketTaskClassDto::ArchitectureExplanation,
        );
        let spec = packet_atom_hydration_spec(&mapper);
        assert!(
            spec.role_scoring_patterns.len() > spec.promotion_patterns.len(),
            "scoring reads every typed pattern; membership reads only the cross-container ones"
        );
        let session = PacketProofSession::new(spec);
        let node = |id: &str, kind: NodeKind| GraphNodeDto {
            id: NodeId(id.into()),
            label: id.into(),
            kind,
            depth: 1,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: None,
            qualified_name: None,
            member_access: None,
        };
        session.record_atom_needed_identities(&GraphResponse {
            center_id: NodeId("50".into()),
            nodes: vec![
                node("50", NodeKind::CLASS),
                node("51", NodeKind::CLASS),
                node("52", NodeKind::CLASS),
                node("60", NodeKind::METHOD),
                node("61", NodeKind::METHOD),
            ],
            edges: vec![
                // 50 stands in BOTH role positions of the config atom.
                typed_edge(
                    "t1",
                    "50",
                    "51",
                    EdgeKind::TYPE_USAGE,
                    Some("certain"),
                    None,
                ),
                typed_edge(
                    "t2",
                    "51",
                    "50",
                    EdgeKind::TYPE_USAGE,
                    Some("certain"),
                    None,
                ),
                // 52 is a lone target.
                typed_edge(
                    "t3",
                    "50",
                    "52",
                    EdgeKind::TYPE_USAGE,
                    Some("certain"),
                    None,
                ),
                // A3's CALL pattern: scoring provenance only.
                typed_edge("c1", "61", "60", EdgeKind::CALL, Some("certain"), None),
            ],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        });

        assert_eq!(
            session.promotion_priority(50),
            2,
            "an identity in two role positions of the group outranks a lone endpoint"
        );
        assert_eq!(session.promotion_priority(51), 2);
        assert_eq!(
            session.promotion_priority(52),
            1,
            "a lone configuration target occupies exactly one position"
        );
        assert_eq!(
            session.promotion_priority(999),
            0,
            "an identity nothing needs scores zero"
        );

        // The CALL endpoints score — and stay out of the need-set entirely.
        for call_endpoint in [60, 61] {
            assert!(
                session.promotion_priority(call_endpoint) >= 1,
                "a CALL role position counts toward the score: {call_endpoint}"
            );
            assert!(
                !session.identity_is_atom_needed(call_endpoint),
                "rev 5.4: a non-cross-container match adds no member: {call_endpoint}"
            );
            assert_eq!(
                session.free_promotion_role(call_endpoint, &[]),
                None,
                "a scoring-only position opens no promotion slot: {call_endpoint}"
            );
        }
        assert_eq!(
            session.hydration.promotion_role_slots(),
            vec![ProofRole::Builder, ProofRole::ConfigType],
            "the slot count is unchanged by scoring — still the cross-container endpoints"
        );
    }

    /// Round 5.5 item 2a: one identity is admissible through each of its
    /// attributed roles exactly once per query, in `ProofRole` order, and an
    /// identity with no free role yields no promotion at all — which is what
    /// leaves base-order admission untouched.
    #[test]
    fn a_promotion_slot_is_spent_once_per_role_per_query() {
        let css = packet_flow_requirements_for_terms(
            &packet_probe_terms(
                "Trace how the css animation keyframes and custom property variables are declared and used by the base selectors in the imported stylesheets.",
            ),
            PacketTaskClassDto::ArchitectureExplanation,
        );
        let session = PacketProofSession::new(packet_atom_hydration_spec(&css));
        session.record_atom_needed_identities(&GraphResponse {
            center_id: NodeId("10".into()),
            nodes: [10, 11]
                .iter()
                .map(|id| GraphNodeDto {
                    id: NodeId(id.to_string()),
                    label: id.to_string(),
                    kind: NodeKind::FILE,
                    depth: 1,
                    label_policy: None,
                    badge_visible_members: None,
                    badge_total_members: None,
                    merged_symbol_examples: Vec::new(),
                    file_path: None,
                    qualified_name: None,
                    member_access: None,
                })
                .collect(),
            edges: vec![typed_edge("i", "10", "11", EdgeKind::IMPORT, None, None)],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        });

        // The IMPORT source carries exactly the entrypoint slot.
        assert_eq!(
            session.free_promotion_role(10, &[]),
            Some(ProofRole::Entrypoint)
        );
        assert_eq!(
            session.free_promotion_role(10, &[ProofRole::Entrypoint]),
            None,
            "the entrypoint slot is spent for the rest of the query"
        );
        // The IMPORT target carries the three source-file slots and hands
        // them out in ProofRole order, deterministically.
        let mut spent = Vec::new();
        for expected in [
            ProofRole::VarsSource,
            ProofRole::BaseSource,
            ProofRole::AnimSource,
        ] {
            let role = session
                .free_promotion_role(11, &spent)
                .expect("a source slot must remain");
            assert_eq!(role, expected);
            spent.push(role);
        }
        assert_eq!(
            session.free_promotion_role(11, &spent),
            None,
            "a fourth promotion of a target identity has no slot left this query"
        );
        assert_eq!(
            session.free_promotion_role(999, &[]),
            None,
            "an identity nothing needs never has a slot"
        );
    }

    /// Rev 5.4 negative (round-4 flood): hydrated edges matching
    /// role-constrained MEMBER and USAGE patterns add NOTHING to the
    /// promotion need-set — only cross-container IMPORT/TYPE_USAGE matches
    /// do.
    #[test]
    fn member_and_usage_pattern_matches_never_join_the_need_set() {
        let graph_of = |nodes: &[(&str, NodeKind)], edges: Vec<GraphEdgeDto>| GraphResponse {
            center_id: NodeId(nodes[0].0.into()),
            nodes: nodes
                .iter()
                .map(|(id, kind)| GraphNodeDto {
                    id: NodeId((*id).into()),
                    label: (*id).into(),
                    kind: *kind,
                    depth: 1,
                    label_policy: None,
                    badge_visible_members: None,
                    badge_total_members: None,
                    merged_symbol_examples: Vec::new(),
                    file_path: None,
                    qualified_name: None,
                    member_access: None,
                })
                .collect(),
            edges,
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        // C family: MEMBER file→CONSTANT and USAGE CONSTANT→VARIABLE match
        // C3's role-to-role patterns exactly — the round-4 flood shape —
        // while IMPORT file→file is the only admissible feed.
        let css = packet_flow_requirements_for_terms(
            &packet_probe_terms(
                "Trace how the css animation keyframes and custom property variables are declared and used by the base selectors in the imported stylesheets.",
            ),
            PacketTaskClassDto::ArchitectureExplanation,
        );
        let session = PacketProofSession::new(packet_atom_hydration_spec(&css));
        session.record_atom_needed_identities(&graph_of(
            &[
                ("10", NodeKind::FILE),
                ("11", NodeKind::FILE),
                ("30", NodeKind::CONSTANT),
                ("40", NodeKind::VARIABLE),
            ],
            vec![
                typed_edge("m", "10", "30", EdgeKind::MEMBER, None, None),
                typed_edge("u", "30", "40", EdgeKind::USAGE, None, None),
                typed_edge("i", "10", "11", EdgeKind::IMPORT, None, None),
            ],
        ));
        for flooded in [30, 40] {
            assert!(
                !session.identity_is_atom_needed(flooded),
                "MEMBER/USAGE pattern matches must add nothing (rev 5.4): {flooded}"
            );
        }
        for container in [10, 11] {
            assert!(
                session.identity_is_atom_needed(container),
                "the IMPORT endpoints are the admissible containers: {container}"
            );
        }

        // A family: certain CALL onto a METHOD and MEMBER class→METHOD match
        // A3's role-to-role patterns — never admitted; the certain
        // TYPE_USAGE edge admits both type endpoints.
        let mapper = packet_flow_requirements_for_terms(
            &packet_probe_terms("How does the mapper build its configuration and execution plan?"),
            PacketTaskClassDto::ArchitectureExplanation,
        );
        let session = PacketProofSession::new(packet_atom_hydration_spec(&mapper));
        session.record_atom_needed_identities(&graph_of(
            &[
                ("50", NodeKind::CLASS),
                ("51", NodeKind::CLASS),
                ("60", NodeKind::METHOD),
                ("61", NodeKind::METHOD),
            ],
            vec![
                typed_edge("c", "61", "60", EdgeKind::CALL, Some("certain"), None),
                typed_edge("m", "50", "60", EdgeKind::MEMBER, None, None),
                typed_edge("t", "50", "51", EdgeKind::TYPE_USAGE, Some("certain"), None),
            ],
        ));
        for flooded in [60, 61] {
            assert!(
                !session.identity_is_atom_needed(flooded),
                "CALL/MEMBER matches must add nothing (rev 5.4): {flooded}"
            );
        }
        for type_endpoint in [50, 51] {
            assert!(
                session.identity_is_atom_needed(type_endpoint),
                "the TYPE_USAGE endpoints are the admissible types: {type_endpoint}"
            );
        }
    }

    #[test]
    fn citation_and_graph_keep_exact_packet_candidate_provenance() {
        let hit = packet_hit("edge-1");
        let citation = hit.citation(true);
        assert_eq!(citation.evidence_edge_ids, [EdgeId("edge-1".into())]);
        assert!(hit.has_proof_call_provenance());

        let mut answer = answer();
        merge_packet_candidate_graph(&mut answer, &hit);
        merge_packet_candidate_graph(&mut answer, &hit);
        let GraphArtifactDto::Uml { id, graph, .. } = &answer.graphs[0] else {
            panic!("expected UML graph");
        };
        assert_eq!(answer.graphs.len(), 1, "exact replay must be idempotent");
        assert_eq!(graph.edges.len(), 1);
        assert!(id.starts_with(PACKET_CANDIDATE_SELECTION_VIEW_ID));
        assert_eq!(answer.subgraph_ids, std::slice::from_ref(id));
    }

    #[test]
    fn syntax_only_call_proof_requires_the_requirement_receiver_owner() {
        for (requirement_id, carrier, target, receiver_owner) in [
            ("request_dispatch", "app.handle", "handle", "app.router"),
            ("request_entrypoint", "app.route", "route", "app.router"),
            ("request_terminal", "res.send", "end", "res"),
            ("request_terminal", "reply.send", "finish", "reply"),
        ] {
            let requirement = server_requirement(requirement_id);
            let identity = format!(
                "src/server.js:10:1:20|syntax:js-member-call|receiver-owner:{receiver_owner}"
            );
            let hit = boundary_hit(carrier, target, Some(&identity), None, true);
            let citation = hit.citation_for_requirements(true, &[requirement]);
            let graph = hit.graph.as_ref().expect("graph");
            let edge = &graph.edges[0];
            let (neighbor_label, neighbor_kind) =
                receipt_neighbor(graph, &citation, edge).expect("neighbor");
            assert!(
                flow_requirement_call_receipt_is_valid(
                    &requirement,
                    &citation,
                    edge,
                    neighbor_label,
                    neighbor_kind,
                ),
                "{receiver_owner}.{target} must prove {requirement_id}"
            );
            assert!(hit.has_proof_call_provenance_for_requirement(&citation, &requirement));
            assert_eq!(citation.evidence_edge_ids[0], EdgeId("boundary".into()));
        }

        for (requirement_id, carrier, target, receiver_owner) in [
            ("request_entrypoint", "app.use", "use", "Metrics"),
            ("request_dispatch", "app.handle", "handle", "Telemetry"),
            ("request_terminal", "res.send", "end", "Telemetry"),
            ("request_terminal", "res.send", "write", "Cache"),
        ] {
            let requirement = server_requirement(requirement_id);
            let identity = format!(
                "src/server.js:10:1:20|syntax:js-member-call|receiver-owner:{receiver_owner}"
            );
            let hit = boundary_hit(carrier, target, Some(&identity), None, true);
            let citation = hit.citation_for_requirements(true, &[requirement]);
            assert!(
                !hit.has_proof_call_provenance_for_requirement(&citation, &requirement),
                "{receiver_owner}.{target} must not prove {requirement_id}"
            );
            assert!(
                citation.evidence_edge_ids.is_empty(),
                "owner-invalid unresolved CALLs must not leak back as citation context"
            );
        }
    }

    #[test]
    fn dense_only_carrier_promotes_only_with_a_strict_requirement_proof() {
        let requirement = server_requirement("request_entrypoint");
        let mut lawful = boundary_hit(
            "app.route",
            "route",
            Some("src/server.js:10|syntax:js-member-call|receiver-owner:app.router"),
            None,
            true,
        );
        lawful.hit.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
        lawful.hit.evidence_producer = Some("dense_anchor".into());
        lawful.hit.eligible_for_sufficiency = Some(false);
        lawful.hit.score_breakdown = Some(codestory_contracts::api::RetrievalScoreBreakdownDto {
            lexical: 0.0,
            semantic: 0.8,
            graph: 0.0,
            total: 0.8,
            tier_cap: Some(0.4),
            boosts: Vec::new(),
            dampening: vec!["dense_only".into()],
            final_rank_reason: Some("dense anchor".into()),
            provenance: vec!["dense_anchor".into()],
        });
        assert!(
            lawful
                .proof_edge_ids_for_requirement(
                    &codestory_agent::citation::to_citation_from_hit(
                        &lawful.hit,
                        None,
                        None,
                        true,
                    ),
                    &requirement,
                )
                .contains(&EdgeId("boundary".into()))
        );
        let base = codestory_agent::citation::to_citation_from_hit(&lawful.hit, None, None, true);
        let promoted = lawful.citation_for_requirements_from_base(
            base,
            true,
            std::slice::from_ref(&requirement),
        );
        assert_eq!(
            promoted.evidence_tier,
            Some(PacketEvidenceTierDto::ResolvedGraph)
        );
        assert_eq!(
            promoted.evidence_producer.as_deref(),
            Some("core_incident_call")
        );
        assert_eq!(promoted.eligible_for_sufficiency, Some(true));
        assert_eq!(promoted.evidence_edge_ids, [EdgeId("boundary".into())]);
        let breakdown = promoted
            .retrieval_score_breakdown
            .as_ref()
            .expect("promoted score breakdown");
        assert_eq!(breakdown.graph, 0.8);
        assert_eq!(breakdown.tier_cap, None);
        assert!(
            !breakdown
                .dampening
                .iter()
                .any(|reason| reason == "dense_only")
        );
        assert!(
            breakdown
                .provenance
                .iter()
                .any(|producer| producer == "core_incident_call")
        );

        let mut explicit_probable =
            boundary_hit("app.route", "route", None, Some("probable"), true);
        explicit_probable.graph.as_mut().expect("graph").edges[0].confidence = None;
        let negative_shapes = [
            boundary_hit(
                "app.route",
                "route",
                Some("src/server.js:10|syntax:js-member-call|receiver-owner:metrics"),
                None,
                true,
            ),
            boundary_hit(
                "app.route",
                "record",
                Some("src/server.js:10|syntax:js-member-call|receiver-owner:app.router"),
                None,
                true,
            ),
            explicit_probable,
            boundary_hit(
                "app.route",
                "route",
                Some("src/server.js:10|syntax:js-member-call|receiver-owner:app.router"),
                None,
                false,
            ),
            boundary_hit("app.route", "route", None, None, true),
        ];
        for mut negative in negative_shapes {
            negative.hit.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
            negative.hit.evidence_producer = Some("dense_anchor".into());
            negative.hit.eligible_for_sufficiency = Some(false);
            let base =
                codestory_agent::citation::to_citation_from_hit(&negative.hit, None, None, true);
            let citation = negative.citation_for_requirements_from_base(
                base,
                true,
                std::slice::from_ref(&requirement),
            );
            assert_eq!(
                citation.evidence_tier,
                Some(PacketEvidenceTierDto::DenseSemantic)
            );
            assert_eq!(citation.evidence_producer.as_deref(), Some("dense_anchor"));
            assert_eq!(citation.eligible_for_sufficiency, Some(false));
            assert!(citation.evidence_edge_ids.is_empty());
        }

        let mut confidence_only = boundary_hit(
            "app.route",
            "route",
            Some("src/server.js:10|syntax:js-member-call|receiver-owner:metrics"),
            None,
            true,
        );
        confidence_only.hit.evidence_tier = Some(PacketEvidenceTierDto::DenseSemantic);
        confidence_only.hit.evidence_producer = Some("dense_anchor".into());
        confidence_only.hit.eligible_for_sufficiency = Some(false);
        confidence_only.graph.as_mut().expect("graph").edges[0].confidence = Some(1.0);
        let base =
            codestory_agent::citation::to_citation_from_hit(&confidence_only.hit, None, None, true);
        let citation = confidence_only.citation_for_requirements_from_base(
            base,
            true,
            std::slice::from_ref(&requirement),
        );
        assert_eq!(
            citation.evidence_tier,
            Some(PacketEvidenceTierDto::DenseSemantic)
        );
        assert_eq!(citation.eligible_for_sufficiency, Some(false));
        assert!(citation.evidence_edge_ids.is_empty());
    }

    #[test]
    fn certain_target_keeps_target_predicate_while_invalid_edges_fail_closed() {
        let requirement = server_requirement("request_dispatch");
        let certain = boundary_hit("app.handle", "Router.handle", None, Some("certain"), true);
        let citation = certain.citation_for_requirements(true, &[requirement]);
        assert!(certain.has_proof_call_provenance_for_requirement(&citation, &requirement));

        let mut resolved = boundary_hit("app.handle", "Router.handle", None, None, true);
        resolved.graph.as_mut().expect("graph").nodes[1].kind = NodeKind::METHOD;
        let resolved_citation = resolved.citation_for_requirements(true, &[requirement]);
        assert!(
            resolved.has_proof_call_provenance_for_requirement(&resolved_citation, &requirement)
        );

        let incoming = boundary_hit("app.handle", "Router.handle", None, Some("certain"), false);
        let incoming_citation = incoming.citation_for_requirements(true, &[requirement]);
        assert!(
            !incoming.has_proof_call_provenance_for_requirement(&incoming_citation, &requirement)
        );

        let wrong_target = boundary_hit(
            "app.handle",
            "telemetry.record",
            None,
            Some("certain"),
            true,
        );
        let wrong_citation = wrong_target.citation_for_requirements(true, &[requirement]);
        assert!(
            !wrong_target.has_proof_call_provenance_for_requirement(&wrong_citation, &requirement)
        );

        let mut speculative =
            boundary_hit("app.handle", "Router.handle", None, Some("probable"), true);
        speculative.graph.as_mut().expect("graph").edges[0].confidence = Some(0.7);
        let speculative_citation = speculative.citation_for_requirements(true, &[requirement]);
        assert!(
            !speculative
                .has_proof_call_provenance_for_requirement(&speculative_citation, &requirement)
        );

        let no_callsite = boundary_hit("app.handle", "handle", None, None, true);
        let no_callsite_citation = no_callsite.citation_for_requirements(true, &[requirement]);
        assert!(
            !no_callsite
                .has_proof_call_provenance_for_requirement(&no_callsite_citation, &requirement)
        );
        assert!(no_callsite_citation.evidence_edge_ids.is_empty());
    }

    #[test]
    fn more_than_twenty_incoming_edges_cannot_evict_a_lawful_outgoing_boundary() {
        let center_id = NodeId("response-send".into());
        let end_id = NodeId("response-end".into());
        let wrong_id = NodeId("response-buffer".into());
        let caller_id = NodeId("response-json".into());
        let mut nodes = [
            (center_id.clone(), "response.send"),
            (end_id.clone(), "end"),
            (wrong_id.clone(), "buffer"),
            (caller_id.clone(), "response.json"),
        ]
        .into_iter()
        .map(|(id, label)| GraphNodeDto {
            id,
            label: label.into(),
            kind: NodeKind::METHOD,
            depth: 1,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: None,
            qualified_name: None,
            member_access: None,
        })
        .collect::<Vec<_>>();
        nodes[0].depth = 0;
        nodes[1].kind = NodeKind::UNKNOWN;

        let mut edges = (0..24)
            .map(|index| GraphEdgeDto {
                id: EdgeId(format!("context-{index:02}")),
                source: caller_id.clone(),
                target: center_id.clone(),
                kind: EdgeKind::CALL,
                confidence: None,
                certainty: None,
                callsite_identity: Some(format!("server.js:{}|syntax:js-member-call", index + 1)),
                candidate_targets: Vec::new(),
            })
            .collect::<Vec<_>>();
        edges.extend([
            GraphEdgeDto {
                id: EdgeId("incoming-only".into()),
                source: caller_id.clone(),
                target: center_id.clone(),
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".into()),
                callsite_identity: Some("server.js:20|syntax:js-member-call".into()),
                candidate_targets: Vec::new(),
            },
            GraphEdgeDto {
                id: EdgeId("speculative-end".into()),
                source: center_id.clone(),
                target: end_id.clone(),
                kind: EdgeKind::CALL,
                confidence: Some(0.7),
                certainty: Some("probable".into()),
                callsite_identity: Some("server.js:21|syntax:js-member-call".into()),
                candidate_targets: Vec::new(),
            },
            GraphEdgeDto {
                id: EdgeId("unbound-end".into()),
                source: center_id.clone(),
                target: end_id.clone(),
                kind: EdgeKind::CALL,
                confidence: None,
                certainty: None,
                callsite_identity: None,
                candidate_targets: Vec::new(),
            },
            GraphEdgeDto {
                id: EdgeId("zz-proof-end".into()),
                source: center_id.clone(),
                target: end_id,
                kind: EdgeKind::CALL,
                confidence: None,
                certainty: None,
                callsite_identity: Some(
                    "server.js:23|syntax:js-member-call|receiver-owner:res".into(),
                ),
                candidate_targets: Vec::new(),
            },
        ]);
        let graph_provenance = edges
            .iter()
            .map(|edge| PacketGraphEdgeProvenance {
                edge_id: edge.id.clone(),
                direction: if edge.source == center_id {
                    PacketGraphDirection::Outgoing
                } else {
                    PacketGraphDirection::Incoming
                },
                hop: 1,
                producers: vec!["core_incident_call".into()],
                certainty: edge.certainty.clone(),
            })
            .collect();
        let hit = PacketSearchHit {
            trail_scans: Vec::new(),
            hit: SearchHit {
                node_id: center_id.clone(),
                display_name: "response.send".into(),
                kind: NodeKind::METHOD,
                file_path: Some("src/response.js".into()),
                line: Some(10),
                score: 0.8,
                origin: SearchHitOrigin::IndexedSymbol,
                target: None,
                resolvable: true,
                match_quality: None,
                evidence_tier: Some(PacketEvidenceTierDto::LexicalSource),
                evidence_producer: Some("symbol_doc".into()),
                resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
                loss_reason: None,
                coverage_role: None,
                eligible_for_sufficiency: Some(true),
                source_excerpt: None,
                verification_targets: Vec::new(),
                score_breakdown: None,
            },
            graph_provenance,
            graph: Some(GraphResponse {
                center_id,
                nodes,
                edges,
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            }),
        };
        let terms = packet_probe_terms(
            "Trace how a server application registers middleware, handles a request, and sends the response.",
        );
        let requirements =
            packet_flow_requirements_for_terms(&terms, PacketTaskClassDto::RouteTracing);
        let terminal = requirements
            .iter()
            .find(|requirement| requirement.id == "request_terminal")
            .expect("terminal requirement");
        let citation = hit.citation_for_requirements(true, &requirements);

        assert_eq!(citation.evidence_edge_ids[0], EdgeId("zz-proof-end".into()));
        assert_eq!(
            citation.evidence_edge_ids.len(),
            1,
            "unrelated CALL context stays graph-only"
        );
        assert!(hit.has_proof_call_provenance_for_requirement(&citation, terminal));

        let mut capped = answer();
        merge_packet_candidate_graph_for_requirements(&mut capped, &hit, &requirements);
        let GraphArtifactDto::Uml { graph, .. } = &capped.graphs[0] else {
            panic!("expected candidate graph");
        };
        assert_eq!(graph.edges.len(), PACKET_CANDIDATE_GRAPH_EDGE_LIMIT);
        assert_eq!(graph.edges[0].id, EdgeId("zz-proof-end".into()));
        assert!(graph.truncated);
        assert_eq!(graph.omitted_edge_count, 8);

        let mut negative = hit.clone();
        negative
            .graph
            .as_mut()
            .expect("graph")
            .edges
            .retain(|edge| edge.id != EdgeId("zz-proof-end".into()));
        assert!(!negative.has_proof_call_provenance_for_requirement(&citation, terminal));
    }

    #[test]
    fn lawful_outgoing_target_after_old_cutoff_is_selected_and_merge_keeps_omissions() {
        let requirement = server_requirement("request_terminal");
        let mut hit = boundary_hit(
            "res.send",
            "end",
            Some("src/server.js:50:1:20|syntax:js-member-call|receiver-owner:res"),
            None,
            true,
        );
        hit.graph_provenance[0].edge_id = EdgeId("zz-proof-end".into());
        let graph = hit.graph.as_mut().expect("graph");
        graph.edges[0].id = EdgeId("zz-proof-end".into());
        let wrong_id = NodeId("wrong".into());
        graph.nodes.push(GraphNodeDto {
            id: wrong_id.clone(),
            label: "observe".into(),
            kind: NodeKind::UNKNOWN,
            depth: 1,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: Some("src/server.js".into()),
            qualified_name: None,
            member_access: None,
        });
        for index in 0..25 {
            let edge_id = EdgeId(format!("context-{index:02}"));
            graph.edges.push(GraphEdgeDto {
                id: edge_id.clone(),
                source: NodeId("carrier".into()),
                target: wrong_id.clone(),
                kind: EdgeKind::CALL,
                confidence: None,
                certainty: None,
                callsite_identity: Some(format!(
                    "src/server.js:{}:1:20|syntax:js-member-call|receiver-owner:metrics",
                    index + 1
                )),
                candidate_targets: Vec::new(),
            });
            hit.graph_provenance.push(PacketGraphEdgeProvenance {
                edge_id,
                direction: PacketGraphDirection::Outgoing,
                hop: 1,
                producers: vec!["core_incident_call".into()],
                certainty: None,
            });
        }
        graph.truncated = true;
        graph.omitted_edge_count = 7;

        let citation = hit.citation_for_requirements(true, &[requirement]);
        assert_eq!(citation.evidence_edge_ids[0], EdgeId("zz-proof-end".into()));
        assert!(hit.has_proof_call_provenance_for_requirement(&citation, &requirement));

        let mut merged = answer();
        merge_packet_candidate_graph_for_requirements(&mut merged, &hit, &[requirement]);
        merge_packet_candidate_graph_for_requirements(&mut merged, &hit, &[requirement]);
        let GraphArtifactDto::Uml {
            id: selection_view_id,
            graph,
            ..
        } = &merged.graphs[0]
        else {
            panic!("expected merged candidate graph");
        };
        assert_eq!(graph.edges.len(), PACKET_CANDIDATE_GRAPH_EDGE_LIMIT);
        assert_eq!(graph.edges[0].id, EdgeId("zz-proof-end".into()));
        assert!(graph.truncated);
        assert_eq!(graph.omitted_edge_count, 13);

        let immutable_selection_view_id = selection_view_id.clone();
        let mut downstream_capped = merged.clone();
        assert!(cap_packet_graph_edges_for_test(
            &mut downstream_capped,
            1,
            &[EdgeId("zz-proof-end".into())],
        ));
        let capped_snapshot = serde_json::to_value(&downstream_capped).expect("capped answer");
        let GraphArtifactDto::Uml { id, graph, .. } = &downstream_capped.graphs[0] else {
            panic!("expected capped selection view");
        };
        assert_eq!(id, &immutable_selection_view_id);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].id, EdgeId("zz-proof-end".into()));
        assert!(graph.truncated);
        assert_eq!(graph.omitted_edge_count, 32);

        // Replaying the same source candidate after presentation capping finds the immutable
        // selection-view lineage and must not restore its budget-dropped optional rows.
        merge_packet_candidate_graph_for_requirements(&mut downstream_capped, &hit, &[requirement]);
        assert_eq!(
            serde_json::to_value(&downstream_capped).expect("replayed answer"),
            capped_snapshot
        );

        let candidate_graph = hit
            .graph_for_requirements(&citation, &[requirement])
            .expect("capped candidate graph");
        let mut preexisting_graph = candidate_graph.clone();
        preexisting_graph.truncated = false;
        preexisting_graph.omitted_edge_count = 0;
        let mut duplicate_owner = answer();
        duplicate_owner.graphs.push(GraphArtifactDto::Uml {
            id: "existing-neighborhood".into(),
            title: "Existing neighborhood".into(),
            graph: preexisting_graph,
        });
        merge_packet_candidate_graph_for_requirements(&mut duplicate_owner, &hit, &[requirement]);
        merge_packet_candidate_graph_for_requirements(&mut duplicate_owner, &hit, &[requirement]);
        assert_eq!(duplicate_owner.graphs.len(), 2);
        let GraphArtifactDto::Uml { id, graph, .. } = &duplicate_owner.graphs[0] else {
            panic!("expected existing graph");
        };
        assert_eq!(id, "existing-neighborhood");
        assert_eq!(graph.edges.len(), PACKET_CANDIDATE_GRAPH_EDGE_LIMIT);
        assert!(!graph.truncated);
        assert_eq!(graph.omitted_edge_count, 0);
        let GraphArtifactDto::Uml {
            id: candidate_id,
            graph: preserved,
            ..
        } = &duplicate_owner.graphs[1]
        else {
            panic!("expected candidate-local graph");
        };
        assert!(preserved.truncated);
        assert_eq!(
            preserved.omitted_edge_count,
            candidate_graph.omitted_edge_count
        );
        assert_eq!(
            duplicate_owner.subgraph_ids,
            std::slice::from_ref(candidate_id)
        );
    }

    #[test]
    fn overlapping_candidate_omissions_remain_artifact_local_and_replay_is_idempotent() {
        // A retains {a,b} and omits {c}; B retains {b,c} and omits {a}. The retained union is
        // complete, but opaque counts cannot prove whether the hidden identities overlap. Keep
        // the two bounded views separate instead of publishing a false aggregate omission of 2.
        let first = overlapping_candidate_hit(
            "candidate-a",
            &[
                ("a", "caller-a", "candidate-a"),
                ("b", "candidate-a", "candidate-b"),
            ],
            1,
        );
        let second = overlapping_candidate_hit(
            "candidate-b",
            &[
                ("b", "candidate-a", "candidate-b"),
                ("c", "candidate-b", "target-c"),
            ],
            1,
        );
        let complete = overlapping_candidate_hit(
            "candidate-complete",
            &[("complete", "candidate-complete", "target-complete")],
            0,
        );

        let mut merged = answer();
        for hit in [&first, &second, &first, &second, &complete, &complete] {
            merge_packet_candidate_graph(&mut merged, hit);
        }

        assert_eq!(
            merged.graphs.len(),
            3,
            "exact replays must not add artifacts"
        );
        assert_eq!(merged.subgraph_ids.len(), 3);
        assert_eq!(merged.subgraph_ids.iter().collect::<HashSet<_>>().len(), 3);

        let mut overlapping_views = merged
            .graphs
            .iter()
            .filter_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. }
                    if graph.center_id.0 == "candidate-a" || graph.center_id.0 == "candidate-b" =>
                {
                    let mut ids = graph
                        .edges
                        .iter()
                        .map(|edge| edge.id.0.as_str())
                        .collect::<Vec<_>>();
                    ids.sort_unstable();
                    Some((graph.center_id.0.as_str(), ids, graph))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        overlapping_views.sort_by_key(|(center, _, _)| *center);
        assert_eq!(overlapping_views.len(), 2);
        assert_eq!(overlapping_views[0].1, ["a", "b"]);
        assert_eq!(overlapping_views[1].1, ["b", "c"]);
        for (_, _, graph) in &overlapping_views {
            assert!(graph.truncated, "one edge remains omitted from this view");
            assert_eq!(graph.omitted_edge_count, 1);
        }
        let retained_union = overlapping_views
            .iter()
            .flat_map(|(_, ids, _)| ids.iter().copied())
            .collect::<HashSet<_>>();
        assert_eq!(retained_union, HashSet::from(["a", "b", "c"]));
        assert!(
            overlapping_views
                .iter()
                .all(|(_, _, graph)| graph.omitted_edge_count != 2),
            "no artifact may claim a synthetic aggregate omission"
        );

        let complete_graph = merged
            .graphs
            .iter()
            .find_map(|artifact| match artifact {
                GraphArtifactDto::Uml { graph, .. }
                    if graph.center_id.0 == "candidate-complete" =>
                {
                    Some(graph)
                }
                _ => None,
            })
            .expect("complete candidate view");
        assert!(!complete_graph.truncated);
        assert_eq!(complete_graph.omitted_edge_count, 0);
    }
}
