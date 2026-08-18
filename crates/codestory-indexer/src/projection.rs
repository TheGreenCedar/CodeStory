//! Callable and file-structural projection: what a re-index compares against.
//!
//! Split out of `lib.rs` under #1801. The projection machinery is a closed
//! subsystem — it turns a parsed file into the hashes that decide whether a
//! re-index is a no-op, a delta, or a full replace — and it is the part of the
//! indexer whose correctness is least visible from the outside, so it earns a
//! boundary of its own.
//!
//! Items are `pub(crate)` because `lib.rs` calls into them; a child module's
//! private items are not visible to its parent. Nothing here is public API
//! except the two signature tags, which `lib.rs` re-exports at their original
//! paths.

use super::*;

pub(crate) fn build_callable_projection_states(
    nodes: &[Node],
    edges: &[Edge],
    occurrences: &[Occurrence],
) -> Vec<CallableProjectionState> {
    let mut edges_by_source: HashMap<NodeId, Vec<&Edge>> = HashMap::new();
    for edge in edges {
        edges_by_source.entry(edge.source).or_default().push(edge);
    }

    let mut occurrences_by_file: HashMap<NodeId, Vec<&Occurrence>> = HashMap::new();
    for occurrence in occurrences {
        occurrences_by_file
            .entry(occurrence.location.file_node_id)
            .or_default()
            .push(occurrence);
    }
    // Sorted once per file so each callable can binary-search its own line
    // window instead of scanning every occurrence in the file: that scan was
    // 501M filter comparisons at 2 MB and 12.4B at 10 MB (#1820).
    for file_occurrences in occurrences_by_file.values_mut() {
        file_occurrences.sort_by_key(|occurrence| occurrence.location.start_line);
    }

    let node_by_id = nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<HashMap<_, _>>();
    let mut states = Vec::new();
    for node in nodes {
        if !matches!(
            node.kind,
            NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
        ) {
            continue;
        }
        let (Some(file_id), Some(start_line), Some(_start_col), Some(end_line)) = (
            node.file_node_id,
            node.start_line,
            node.start_col,
            node.end_line,
        ) else {
            continue;
        };
        let symbol_key = format!(
            "{}:{}",
            node.kind as i32,
            node.qualified_name
                .as_deref()
                .unwrap_or(node.serialized_name.as_str())
        );
        let signature_hash = callable_signature_hash(&symbol_key);

        let mut body_parts = vec![
            format!("extent:{start_line}:{end_line}"),
            format!("identity:{}", node.id.0),
        ];
        body_parts.extend(callable_edge_projection_parts(
            edges_by_source.get(&node.id),
        ));
        body_parts.extend(callable_occurrence_projection_parts(
            occurrences_by_file.get(&file_id),
            node,
            start_line,
            end_line,
        ));

        let normalized_signature = callable_normalized_signature(
            node,
            start_line,
            end_line,
            edges_by_source.get(&node.id),
            occurrences_by_file.get(&file_id),
        );

        states.push(CallableProjectionState {
            file_id: file_id.0,
            symbol_key,
            node_id: node.id,
            signature_hash,
            normalized_signature: Some(normalized_signature),
            body_hash: hash_parts(body_parts.iter().map(String::as_str)),
            start_line,
            end_line,
        });
    }

    if let Some(file_node) = nodes.iter().find(|node| node.kind == NodeKind::FILE) {
        let repaired_callables = CallableProjectionExtents::from_states(&states);
        let fence = structural_projection_fence(
            file_node.id,
            nodes,
            edges,
            occurrences,
            &node_by_id,
            &repaired_callables,
        );
        states.push(CallableProjectionState {
            file_id: file_node.id.0,
            symbol_key: FILE_STRUCTURAL_SYMBOL_KEY.to_string(),
            node_id: file_node.id,
            // On a callable row this column is the identity the delta path
            // cannot repair; on the file row it means the same thing for the
            // unowned population, which is why the identity lives here rather
            // than in a second row: a second row keyed to the same `node_id`
            // would double every `callable_projection_state` join an annotation
            // lookup makes against the FILE node.
            signature_hash: fence.identity,
            // The file structural row is not a callable, so it carries no
            // normalized signature and can never satisfy a rebind probe.
            normalized_signature: None,
            body_hash: fence.detector,
            start_line: 1,
            end_line: file_node.end_line.unwrap_or(1),
        });
    }

    states.sort_by(|lhs, rhs| lhs.symbol_key.cmp(&rhs.symbol_key));
    states
}

/// Tag for a normalized signature whose body projected at least one part.
pub const CALLABLE_SHAPE_SIGNATURE_TAG: &str = "shape";
/// Tag for a normalized signature with no body evidence behind it.
pub const CALLABLE_OUTLINE_SIGNATURE_TAG: &str = "outline";

/// Position- and name-independent shape of one callable.
///
/// `signature_hash` is a *change detector*: it binds the symbol's own name and
/// its exact start position, which is exactly what incremental projection
/// wants and exactly what annotation rebinding must not use. Rebinding has to
/// recognise the same code under a new name, or in a new file, so this hash
/// deliberately drops three things:
///
/// - the symbol's own name, so a pure rename keeps its signature;
/// - the owning file, so a pure move keeps its signature;
/// - every absolute position, so a position-shifting edit keeps its signature.
///
/// What remains is the callable's shape: its kind, its line extent, and its
/// body projection expressed relative to its own start.
///
/// The result is tagged, because those two ingredients are not equally strong.
/// A callable whose body projects no edges and no occurrences — a one-line
/// accessor, a constant-returning stub — has only its kind and its line count
/// left, and every other stub of the same length shares them. That is an
/// `outline`: honest as a *consistency check* against evidence that already
/// identifies the symbol, worthless as an identifier on its own. A body that
/// projected something is a `shape`. Consumers that infer identity purely from
/// the signature must insist on a `shape`; the alternative is handing a
/// bookmark on a deleted stub to whichever stub happens to survive.
///
/// Distinct callables with identical shapes still collide, which is correct:
/// the rebind ladder treats a collision as an ambiguous match and refuses to
/// guess.
pub(crate) fn callable_normalized_signature(
    node: &Node,
    start_line: u32,
    end_line: u32,
    source_edges: Option<&Vec<&Edge>>,
    file_occurrences: Option<&Vec<&Occurrence>>,
) -> String {
    let mut body_parts = callable_relative_edge_parts(source_edges, start_line);
    body_parts.extend(callable_relative_occurrence_parts(
        file_occurrences,
        node,
        start_line,
        end_line,
    ));
    let tag = if body_parts.is_empty() {
        CALLABLE_OUTLINE_SIGNATURE_TAG
    } else {
        CALLABLE_SHAPE_SIGNATURE_TAG
    };
    let mut parts = vec![
        format!("kind={}", node.kind as i32),
        format!("extent={}", end_line.saturating_sub(start_line)),
    ];
    parts.extend(body_parts);
    let hash = hash_parts(parts.iter().map(String::as_str));
    format!("{tag}:{hash}")
}

/// Outgoing body edges keyed by callee identity and by line *within* the body.
///
/// `callsite_identity` is deliberately excluded: it embeds the owning file node
/// id and the absolute line, so including it would reintroduce exactly the
/// position and file dependence this hash exists to shed.
pub(crate) fn callable_relative_edge_parts(
    source_edges: Option<&Vec<&Edge>>,
    start_line: u32,
) -> Vec<String> {
    let Some(source_edges) = source_edges else {
        return Vec::new();
    };
    let mut edge_parts = source_edges
        .iter()
        .filter(|edge| !is_structural_projection_edge(edge.kind))
        .map(|edge| {
            format!(
                "e:{}:{}:{}",
                edge.kind as i32,
                edge.target.0,
                edge.line.unwrap_or(start_line).saturating_sub(start_line)
            )
        })
        .collect::<Vec<_>>();
    edge_parts.sort();
    edge_parts
}

/// Body occurrences expressed relative to the callable's own start line.
pub(crate) fn callable_relative_occurrence_parts(
    file_occurrences: Option<&Vec<&Occurrence>>,
    node: &Node,
    start_line: u32,
    end_line: u32,
) -> Vec<String> {
    let Some(file_occurrences) = file_occurrences else {
        return Vec::new();
    };
    // `file_occurrences` is sorted by start line, so everything this callable
    // can own is one contiguous window. The lower bound is the first
    // occurrence starting at or after the callable; the upper bound is the
    // first starting past its end, which is sound because an occurrence never
    // ends before it starts — anything starting past `end_line` must also end
    // past it and so can never satisfy the predicate.
    let first =
        file_occurrences.partition_point(|occurrence| occurrence.location.start_line < start_line);
    let past_last =
        file_occurrences.partition_point(|occurrence| occurrence.location.start_line <= end_line);
    let mut occurrence_parts = file_occurrences[first..past_last]
        .iter()
        .filter(|occurrence| {
            occurrence_belongs_to_callable_body(occurrence, node, start_line, end_line)
        })
        .map(|occurrence| {
            format!(
                "o:{}:{}:{}:{}:{}:{}",
                occurrence.element_id,
                occurrence.kind as i32,
                occurrence.location.start_line.saturating_sub(start_line),
                occurrence.location.start_col,
                occurrence.location.end_line.saturating_sub(start_line),
                occurrence.location.end_col
            )
        })
        .collect::<Vec<_>>();
    occurrence_parts.sort();
    occurrence_parts
}

/// Stamp distinguishing one callable-projection identity format from the next.
///
/// A stored row whose stamp differs was produced by a different definition of
/// "changed", so it cannot be compared field by field. Bumping the stamp is the
/// declared way to force exactly one full re-projection wave per file and then
/// return to incremental updates; it is the only reason `signature_hash` ever
/// changes for a symbol that kept its key.
pub(crate) const CALLABLE_PROJECTION_FORMAT_STAMP: &str = "callable-projection-format:2";

/// Identity of one projection row: the state the delta path cannot repair.
///
/// `signature_hash` used to bind `start_line` and `start_col`, so inserting a
/// line above any function flipped it and routed the whole file to
/// `FullReplace` (CR-008) — which deletes the file's nodes and, with them,
/// everything anchored to those nodes. Position is not identity: it is
/// projection *content*, carried in `body_hash` and in the row's own
/// `start_line`/`end_line` columns, both of which the delta path rewrites.
/// What remains here is the symbol key and the format stamp.
pub(crate) fn callable_signature_hash(symbol_key: &str) -> i64 {
    hash_parts([CALLABLE_PROJECTION_FORMAT_STAMP, symbol_key])
}

/// The edge kinds the caller-scoped delta cleanup rewrites.
///
/// `Store::delete_projection_for_callers` deletes exactly the call and usage
/// edges a changed caller sources in its own file. Anything else the caller
/// sources survives that cleanup, so it belongs to the file-structural fence
/// instead — this predicate is the single definition all three sites read.
pub(crate) fn edge_is_caller_scoped_repairable(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::CALL | EdgeKind::USAGE)
}

/// Every edge the delta cleanup would remove for this caller.
///
/// A caller whose outgoing edges changed is only safe to repair incrementally
/// if the cleanup deletes the same edges this hash counted; an edge the hash
/// counts but the cleanup skips is an edge whose stale row survives the repair.
pub(crate) fn callable_edge_projection_parts(source_edges: Option<&Vec<&Edge>>) -> Vec<String> {
    let Some(source_edges) = source_edges else {
        return Vec::new();
    };
    let mut edge_parts = source_edges
        .iter()
        .filter(|edge| edge_is_caller_scoped_repairable(edge.kind))
        .map(|edge| {
            format!(
                "{}:{}:{}:{}",
                edge.kind as i32,
                edge.target.0,
                edge.line.unwrap_or(0),
                edge.callsite_identity.as_deref().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    edge_parts.sort();
    edge_parts
}

/// The edge kinds that describe a symbol's place in the file's structure
/// rather than the work its body does.
///
/// Distinct from `edge_is_caller_scoped_repairable`, and the two must not be
/// collapsed into one another: that predicate names the edges the delta
/// cleanup rewrites, so it fences incremental repair, while this one excludes
/// structure from a callable's *shape*, so it feeds the normalized signature
/// the annotation rebind ladder matches on. They answer different questions
/// and are free to disagree about a kind that is neither CALL nor USAGE.
pub(crate) fn is_structural_projection_edge(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::MEMBER | EdgeKind::INHERITANCE | EdgeKind::IMPORT | EdgeKind::OVERRIDE
    )
}

pub(crate) fn callable_occurrence_projection_parts(
    file_occurrences: Option<&Vec<&Occurrence>>,
    node: &Node,
    start_line: u32,
    end_line: u32,
) -> Vec<String> {
    let Some(file_occurrences) = file_occurrences else {
        return Vec::new();
    };
    let mut occurrence_parts = file_occurrences
        .iter()
        .filter(|occurrence| {
            occurrence_belongs_to_callable_body(occurrence, node, start_line, end_line)
        })
        .map(|occurrence| {
            format!(
                "{}:{}:{}:{}:{}:{}",
                occurrence.element_id,
                occurrence.kind as i32,
                occurrence.location.start_line,
                occurrence.location.start_col,
                occurrence.location.end_line,
                occurrence.location.end_col
            )
        })
        .collect::<Vec<_>>();
    occurrence_parts.sort();
    occurrence_parts
}

pub(crate) fn occurrence_belongs_to_callable_body(
    occurrence: &Occurrence,
    node: &Node,
    start_line: u32,
    end_line: u32,
) -> bool {
    occurrence.location.start_line >= start_line
        && occurrence.location.end_line <= end_line
        && occurrence.element_id != node.id.0
}

/// Everything in a file that no callable row owns.
///
/// The delta path repairs exactly two things: the rows of the callers it was
/// given, and the node table (upserted by id, so positions self-heal for any
/// node whose id is stable). This hash is the fence around the rest, and it is
/// deliberately built as the complement of what
/// `Store::delete_projection_for_callers` deletes:
///
/// - **node identity** for every node in the file. Some collectors still mint
///   ids from a line and column, so a shifted declaration becomes a *different*
///   node; a delta would insert the new row and leave the old one behind.
///   Hashing the id makes any identity churn a full replacement, which is the
///   only repair that removes the abandoned rows.
/// - **unowned edges**, with their line. An edge whose source is not a callable
///   of this file is never deleted by the caller-scoped cleanup, and edge rows
///   are insert-or-ignore, so a moved edge would keep its old line forever.
/// - **unowned occurrences**, with their span. Occurrence rows carry no id at
///   all, so a moved occurrence that is not deleted becomes a duplicate row
///   rather than an updated one.
///
/// Callables contribute only their identity here: their positions, edges, and
/// body occurrences live in their own rows, which the delta path rewrites.
/// The callables that actually have a projection row, and their extents.
///
/// Ownership must be read off the rows that exist, not off the node kinds: a
/// callable the projection skipped (no recorded column, say) repairs nothing,
/// so everything inside it still belongs to the file fence.
pub(crate) struct CallableProjectionExtents {
    node_ids: HashSet<NodeId>,
    /// Extents sorted by start line, with the running maximum end line, so a
    /// containment probe can stop before scanning every callable in the file.
    sorted_extents: Vec<(u32, u32)>,
    prefix_max_end: Vec<u32>,
}

impl CallableProjectionExtents {
    fn from_states(states: &[CallableProjectionState]) -> Self {
        let node_ids = states.iter().map(|state| state.node_id).collect();
        let mut sorted_extents = states
            .iter()
            .map(|state| (state.start_line, state.end_line))
            .collect::<Vec<_>>();
        sorted_extents.sort_unstable();
        let mut prefix_max_end = Vec::with_capacity(sorted_extents.len());
        let mut running_max = 0;
        for (_, end_line) in &sorted_extents {
            running_max = running_max.max(*end_line);
            prefix_max_end.push(running_max);
        }
        Self {
            node_ids,
            sorted_extents,
            prefix_max_end,
        }
    }

    fn owns_node(&self, node_id: NodeId) -> bool {
        self.node_ids.contains(&node_id)
    }

    /// Mirror of the occurrence predicate in `delete_projection_for_callers`:
    /// an occurrence is repairable when it is a callable's own definition or
    /// falls inside a callable's recorded extent.
    fn owns_occurrence(&self, occurrence: &Occurrence) -> bool {
        if self.node_ids.contains(&NodeId(occurrence.element_id)) {
            return true;
        }
        let candidates = self
            .sorted_extents
            .partition_point(|(start_line, _)| *start_line <= occurrence.location.start_line);
        if candidates == 0 || self.prefix_max_end[candidates - 1] < occurrence.location.end_line {
            return false;
        }
        self.sorted_extents[..candidates]
            .iter()
            .any(|(_, end_line)| *end_line >= occurrence.location.end_line)
    }
}

/// The two numbers the file-structural row carries.
pub(crate) struct StructuralProjectionFence {
    /// `body_hash`: the fence's change detector, and a **frozen wire format**.
    /// A store written by an earlier release compares against this value
    /// directly, so altering one part string would re-replace — and so strip
    /// the annotations from — every file in every existing database.
    detector: i64,
    /// `signature_hash`: the same population with the repairable positions
    /// taken out, which is what lets a pure shift be repaired in place instead
    /// of read as churn.
    identity: i64,
}

/// Stamp distinguishing one file-structural *identity* format from the next.
///
/// Embeds `CALLABLE_PROJECTION_FORMAT_STAMP`, so a callable-format bump
/// invalidates this one too.
pub(crate) const FILE_STRUCTURAL_IDENTITY_FORMAT_STAMP: &str = "file-structural-identity-format:1";

/// The one unowned row class the reposition repair cannot fix.
///
/// `Store::delete_unowned_projection_for_file` removes the file's unowned edge
/// rows by `file_node_id`, because that is the only column tying an edge row to
/// the parse that will re-emit it. An unowned edge recorded against a
/// *different* file — or against no file at all — is therefore never reached by
/// that repair, and edge rows are insert-or-ignore, so a moved one would keep
/// its old line forever. Those edges keep their line inside the identity hash,
/// which routes them to `FullReplace`: the only cleanup that does reach them.
pub(crate) fn edge_position_is_repairable_by_file(edge: &Edge, file_id: NodeId) -> bool {
    edge.file_node_id == Some(file_id)
}

pub(crate) fn structural_projection_fence(
    file_id: NodeId,
    nodes: &[Node],
    edges: &[Edge],
    occurrences: &[Occurrence],
    node_by_id: &HashMap<NodeId, &Node>,
    repaired_callables: &CallableProjectionExtents,
) -> StructuralProjectionFence {
    let mut parts = Vec::new();
    let mut identity_parts = Vec::new();

    for node in nodes {
        if node.id == file_id {
            continue;
        }
        let qualified_name = node
            .qualified_name
            .as_deref()
            .unwrap_or(node.serialized_name.as_str());
        let role = if is_callable_kind(node.kind) {
            "callable"
        } else {
            "node"
        };
        let part = format!("{role}:{}:{qualified_name}:{}", node.kind as i32, node.id.0);
        identity_parts.push(part.clone());
        parts.push(part);
    }

    for edge in edges {
        if edge_is_caller_scoped_repairable(edge.kind) && repaired_callables.owns_node(edge.source)
        {
            continue;
        }
        let endpoint_name = |id: &NodeId| {
            node_by_id
                .get(id)
                .map(|node| {
                    node.qualified_name
                        .as_deref()
                        .unwrap_or(node.serialized_name.as_str())
                })
                .unwrap_or_default()
        };
        let source_name = endpoint_name(&edge.source);
        let target_name = endpoint_name(&edge.target);
        let kind = edge.kind as i32;
        let line = edge.line.unwrap_or(0);
        parts.push(format!("edge:{kind}:{source_name}:{target_name}:{line}"));
        identity_parts.push(if edge_position_is_repairable_by_file(edge, file_id) {
            format!("edge:{kind}:{source_name}:{target_name}")
        } else {
            format!("unrepairable-edge:{kind}:{source_name}:{target_name}:{line}")
        });
    }

    for occurrence in occurrences {
        if occurrence.location.file_node_id != file_id {
            continue;
        }
        if repaired_callables.owns_occurrence(occurrence) {
            continue;
        }
        let element = occurrence.element_id;
        let kind = occurrence.kind as i32;
        parts.push(format!(
            "occurrence:{element}:{kind}:{}:{}:{}:{}",
            occurrence.location.start_line,
            occurrence.location.start_col,
            occurrence.location.end_line,
            occurrence.location.end_col
        ));
        identity_parts.push(format!("occurrence:{element}:{kind}"));
    }

    parts.sort();
    identity_parts.sort();
    StructuralProjectionFence {
        detector: hash_parts(parts.iter().map(String::as_str)),
        identity: hash_parts(
            [
                FILE_STRUCTURAL_IDENTITY_FORMAT_STAMP,
                CALLABLE_PROJECTION_FORMAT_STAMP,
            ]
            .into_iter()
            .chain(identity_parts.iter().map(String::as_str)),
        ),
    }
}

/// How the file-structural fence classifies one refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileStructuralVerdict {
    /// The fence saw nothing move and nothing change.
    Unchanged,
    /// Only repairable positions moved.
    Repositioned,
    /// The unowned population itself changed, or there is no identity evidence.
    Replaced,
}

/// Read the stored file-structural row against the one just projected.
///
/// The upgrade from a store that predates the identity hash needs no migration
/// and triggers no re-projection wave. Those rows hold
/// `callable_signature_hash(FILE_STRUCTURAL_SYMBOL_KEY)` — one constant, the
/// same for every file — which is not equal to any identity this function is
/// handed, so a legacy row is read as churn and behaves exactly as it did
/// before: `NoChanges` while the change detector agrees, `FullReplace` the
/// moment it does not. The first edit to a file re-stamps it with a real
/// identity, and only from then on can it be repositioned.
pub(crate) fn classify_file_structural_fence(
    existing: &CallableProjectionState,
    current: &CallableProjectionState,
) -> FileStructuralVerdict {
    if current.body_hash == existing.body_hash {
        return FileStructuralVerdict::Unchanged;
    }
    if existing.signature_hash != current.signature_hash {
        return FileStructuralVerdict::Replaced;
    }
    FileStructuralVerdict::Repositioned
}

pub(crate) fn classify_projection_update(
    existing: &[CallableProjectionState],
    current: &[CallableProjectionState],
) -> ProjectionUpdateMode {
    if existing.is_empty() {
        return ProjectionUpdateMode::InsertFresh;
    }
    if current.is_empty() {
        return ProjectionUpdateMode::FullReplace;
    }

    let existing_by_key = existing
        .iter()
        .map(|state| (state.symbol_key.as_str(), state))
        .collect::<HashMap<_, _>>();
    let current_by_key = current
        .iter()
        .map(|state| (state.symbol_key.as_str(), state))
        .collect::<HashMap<_, _>>();

    if existing_by_key.len() != current_by_key.len() {
        return ProjectionUpdateMode::FullReplace;
    }
    if existing_by_key
        .keys()
        .any(|symbol_key| !current_by_key.contains_key(symbol_key))
    {
        return ProjectionUpdateMode::FullReplace;
    }

    let mut changed_callers = Vec::new();
    let mut fence = FileStructuralVerdict::Unchanged;
    for current_state in current {
        let Some(existing_state) = existing_by_key.get(current_state.symbol_key.as_str()) else {
            return ProjectionUpdateMode::FullReplace;
        };
        if current_state.symbol_key == FILE_STRUCTURAL_SYMBOL_KEY {
            fence = classify_file_structural_fence(existing_state, current_state);
            if fence == FileStructuralVerdict::Replaced {
                return ProjectionUpdateMode::FullReplace;
            }
            continue;
        }
        if current_state.signature_hash != existing_state.signature_hash {
            return ProjectionUpdateMode::FullReplace;
        }
        if current_state.body_hash != existing_state.body_hash {
            changed_callers.push(current_state.node_id);
        }
    }

    match fence {
        FileStructuralVerdict::Replaced => ProjectionUpdateMode::FullReplace,
        FileStructuralVerdict::Repositioned => {
            ProjectionUpdateMode::RepositionUnowned { changed_callers }
        }
        FileStructuralVerdict::Unchanged if changed_callers.is_empty() => {
            ProjectionUpdateMode::NoChanges
        }
        FileStructuralVerdict::Unchanged => ProjectionUpdateMode::Delta { changed_callers },
    }
}
