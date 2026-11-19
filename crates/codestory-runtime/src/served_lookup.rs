//! Lookups that verify only what they serve.
//!
//! `find`, `refs` and `read` start from live source text. Discovery honors
//! ignore rules and text matching runs in parallel.
//! Every printed line comes from bytes read during the call. Index facts
//! (enclosing definitions, declaration ranges, resolved references) decorate a
//! file only when those same bytes hash to the `content_hash` recorded for it
//! in the pinned complete core publication. A file that changed since
//! indexing, or was never indexed, is still served as plain text and marked.
//! Hashing the displayed files cannot verify every input to a binding.
//! Relationship rows describe how the pinned index resolved those files.
//!
//! `find` and `refs` show each hit inside its enclosing declaration, or inside
//! a few lines of context where the index has none to offer, so the reply
//! carries the code a reader would open next. A line budget bounds that; hits
//! past it stay one line each.
//!
//! `refs` then prints the declarations each definition uses, and `read` says
//! where they are. Those are the targets of the outgoing edges the index
//! resolved with certainty, each one only as a declaration in a file whose
//! live bytes the index still describes; a guessed resolution names nothing.
//!
//! Nothing here refreshes or activates an index, checks whole-tree freshness,
//! or writes: the store is opened observationally and a missing or unreadable
//! publication degrades the reply to text only.

use codestory_contracts::api::ApiError;
use codestory_contracts::graph::{
    Edge, EdgeKind, NodeId, NodeKind, OccurrenceKind, ResolutionCertainty, TrailCallerScope,
};
use codestory_store::{StorageError, Store};
use codestory_workspace::ProjectRelativePathResolution;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

mod investigation;
mod mention_witnesses;
mod source_candidates;
pub use investigation::{changed_relationships, find_implementations};

/// Hit lines one `find` or `refs` reply lists before it switches to counts.
pub const DEFAULT_LIMIT: usize = 150;
/// Lines one `find` or `refs` reply spends on declaration bodies and context,
/// counting their `==` headers and `...` gap marks.
pub const DEFAULT_BODY_LINES: usize = 600;
pub const DEFAULT_MAX_BYTES: usize = 7_500;
pub const DEFAULT_READ_MAX_BYTES: usize = 16_000;

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const BINARY_PROBE_BYTES: usize = 1024;
const MAX_CODE_CHARS: usize = 200;
const READ_LINE_CAP: usize = 300;
const APPROXIMATE_LINE_CAP: usize = 80;
/// A declaration up to this long is shown whole; a longer one is windowed.
const WHOLE_DECLARATION_LINES: u32 = 150;
/// Lines shown on each side of a hit inside a windowed declaration.
const DECLARATION_WINDOW: u32 = 12;
/// Lines shown on each side of a hit that has no verified declaration.
const TEXT_WINDOW: u32 = 3;
/// `refs` leads with the declarations of the name when there are this few.
const DEFINITION_BODIES: usize = 3;
/// Declarations `refs` shows after the references for each definition's uses.
const USED_DECLARATIONS: usize = 10;
/// Rows `read` lists under a declaration for the declarations it uses.
const USED_POINTERS: usize = 15;
/// A used declaration up to this long is printed whole; a longer one is one
/// row that says where to read it.
const WHOLE_USED_LINES: u32 = 80;
/// Percent of the budget the definitions left that the references may spend
/// when the used declarations want the rest.
const REFERENCE_SHARE: usize = 60;
/// `read <path>` prints a file up to this long whole, and outlines a longer one.
const WHOLE_FILE_LINES: u32 = 400;
/// Hit files past the listed ones that `refs` still checks for definitions,
/// so a small `--limit` does not hide where the name is defined.
const DEFINITION_SEARCH_FILES: usize = 1_000;
const NO_INDEX: &str = "no index; text matches only";
const APPROXIMATE: &str = " (not from index; approximate)";
const PINNED_USES: &str = "uses: (pinned index, matching endpoint files)";
/// Words that introduce a declaration, for the text-only range fallback.
const DECLARATION_WORDS: &str = "abstract async class const def define enum export extern final fn \
    fun func function impl inline interface let macro_rules! mod module namespace object override \
    package private proc protected pub public record static struct sub trait type typedef union \
    val var virtual void";

#[derive(serde::Serialize)]
pub struct LookupReply {
    pub text: String,
    /// Indicates a missing publication or drift observed in files this lookup checked.
    /// A false value gives no freshness guarantee for other binding inputs.
    pub refresh_recommended: bool,
}

fn lookup_reply(
    out: String,
    max_bytes: usize,
    index: Option<&Index<'_>>,
    follow_ups: &[Option<&str>],
) -> LookupReply {
    LookupReply {
        text: bound_reply(out, max_bytes, follow_ups),
        refresh_recommended: index.is_none_or(|index| index.refresh_recommended),
    }
}

/// Literal, case-sensitive text search over the live tree; any term matches.
///
/// With no terms, lists the live paths selected by `file_globs`. `body_lines`
/// is the budget for declaration bodies and context; `None` lists one line
/// per hit.
pub fn find(
    root: &Path,
    storage_path: &Path,
    terms: &[String],
    file_globs: &[String],
    limit: usize,
    body_lines: Option<usize>,
) -> Result<String, ApiError> {
    find_bounded(
        root,
        storage_path,
        terms,
        file_globs,
        limit,
        body_lines,
        0,
        false,
    )
    .map(|reply| reply.text)
}

#[allow(clippy::too_many_arguments)]
pub fn find_bounded(
    root: &Path,
    storage_path: &Path,
    terms: &[String],
    file_globs: &[String],
    limit: usize,
    body_lines: Option<usize>,
    max_bytes: usize,
    lookup_status: bool,
) -> Result<LookupReply, ApiError> {
    if terms
        .iter()
        .any(|term| term.is_empty() || term.contains(['\n', '\r']))
    {
        return Err(ApiError::invalid_argument(
            "find terms must be non-empty single-line text",
        ));
    }
    if terms.is_empty() {
        if file_globs.is_empty() {
            return Err(ApiError::invalid_argument(
                "find needs a TERM or a --file-glob",
            ));
        }
        let paths = live_paths(root, file_globs)?;
        let mut out = format!("{} files\n", paths.len());
        for path in paths.iter().take(limit) {
            let _ = writeln!(out, "{path}");
        }
        if paths.len() > limit {
            let _ = writeln!(out, "{} more files", paths.len() - limit);
        }
        if !lookup_status {
            return Ok(lookup_reply(out, max_bytes, None, &[]));
        }
        return Ok(with_pinned_index(storage_path, |store| {
            let mut index = store.map(Index::new);
            if let Some(index) = &mut index {
                for path in paths.iter().take(limit) {
                    if let Some((file, _)) = open_live(root, root.join(path)) {
                        index.serve(&file);
                    }
                }
            }
            lookup_reply(out, max_bytes, index.as_ref(), &[])
        }));
    }
    let needles = terms.iter().map(String::as_bytes).collect::<Vec<_>>();
    let files = scan_live_tree(root, file_globs, &needles, false, limit)?;
    Ok(with_pinned_index(storage_path, |store| {
        let mut index = store.map(Index::new);
        let mut out = String::new();
        if index.is_none() {
            let _ = writeln!(out, "{NO_INDEX}");
        }
        let total = files.iter().map(|file| file.hit_lines.len()).sum::<usize>();
        let _ = writeln!(out, "{total} matches in {} files", files.len());
        let out = render_hit_listing(out, &files, limit, index.as_mut(), body_lines, max_bytes);
        lookup_reply(out, max_bytes, index.as_ref(), &[])
    }))
}

/// Every whole-word mention of `name` (or of the last segment of
/// `Qualifier::Name` / `Qualifier.Name`), with the definitions the index
/// records for it in files whose live bytes it still describes. `body_lines`
/// is the budget for declaration bodies and context; `None` lists one line
/// per hit. With a budget and at most three definitions, the declarations
/// each definition uses follow the references.
pub fn refs(
    root: &Path,
    storage_path: &Path,
    name: &str,
    limit: usize,
    body_lines: Option<usize>,
) -> Result<String, ApiError> {
    refs_batch(
        root,
        storage_path,
        &[name.into()],
        &[],
        limit,
        body_lines,
        0,
    )
    .map(|reply| reply.text)
}

pub fn refs_batch(
    root: &Path,
    storage_path: &Path,
    selectors: &[String],
    file_globs: &[String],
    limit: usize,
    body_lines: Option<usize>,
    max_bytes: usize,
) -> Result<LookupReply, ApiError> {
    if selectors.is_empty() {
        return Err(ApiError::invalid_argument("refs needs at least one target"));
    }
    let scope = lookup_scope(root, file_globs)?;
    if let [name] = selectors
        && relationship_selector(root, name).is_none()
    {
        let wanted = ref_name(name)?;
        let files = scan_live_tree(
            root,
            file_globs,
            &[wanted[wanted.len() - 1].as_bytes()],
            true,
            limit,
        )?;
        return Ok(with_pinned_index(storage_path, |store| {
            let mut index = store.map(|store| Index::scoped(store, scope.as_ref()));
            let out = match &mut index {
                Some(index) => refs_with_index(
                    root, index, name, &wanted, &files, limit, body_lines, max_bytes,
                ),
                None => {
                    let total = files.iter().map(|file| file.hit_lines.len()).sum::<usize>();
                    let out = format!("{NO_INDEX}\n{total} references in {} files\n", files.len());
                    render_hit_listing(out, &files, limit, None, body_lines, max_bytes)
                }
            };
            lookup_reply(out, max_bytes, index.as_ref(), &[])
        }));
    }
    relationship_reply(
        root,
        storage_path,
        selectors,
        file_globs,
        scope.as_ref(),
        limit,
        body_lines,
        max_bytes,
    )
}

/// Recognize explicit file selectors without interpreting Rust-style qualified names as paths.
fn relationship_selector<'a>(root: &Path, selector: &'a str) -> Option<(&'a str, Target<'a>)> {
    let (path, target) = parse_selector(selector)?;
    match target {
        Target::Line(_) | Target::Range(..) => Some((path, target)),
        Target::Symbol(_)
            if path.contains(['/', '\\'])
                || Path::new(path).extension().is_some()
                || root.join(path).is_file() =>
        {
            Some((path, target))
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn relationship_reply(
    root: &Path,
    storage_path: &Path,
    selectors: &[String],
    file_globs: &[String],
    scope: Option<&ignore::overrides::Override>,
    limit: usize,
    body_lines: Option<usize>,
    max_bytes: usize,
) -> Result<LookupReply, ApiError> {
    let mut names = BTreeSet::new();
    for selector in selectors {
        if relationship_selector(root, selector).is_none() {
            let wanted = ref_name(selector)?;
            names.insert(wanted[wanted.len() - 1]);
        }
    }
    // A request made solely of concrete selectors never walks the repository.
    let mut hits = BTreeMap::<&str, Vec<LiveFile>>::new();
    if !names.is_empty() {
        let paths = live_walk(root, file_globs)?.collect::<Vec<_>>();
        let matched = paths
            .par_iter()
            .map_init(Vec::new, |bytes, path| {
                if !read_live_text(path, bytes) {
                    return Vec::new();
                }
                names
                    .iter()
                    .filter_map(|&name| {
                        match_live_file(root, path, bytes, &[name.as_bytes()], true, limit)
                            .map(|file| (name, file))
                    })
                    .collect()
            })
            .flatten()
            .collect::<Vec<_>>();
        for (name, file) in matched {
            hits.entry(name).or_default().push(file);
        }
        for files in hits.values_mut() {
            files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
        }
    }
    Ok(with_pinned_index(storage_path, |store| {
        let mut index = store.map(|store| Index::scoped(store, scope));
        let mut roots = Vec::new();
        let mut seen = HashSet::new();
        let mut out = String::new();
        let mut remaining = limit;
        let mut bodies = RelationshipBodies {
            remaining: body_lines.unwrap_or(0),
            printed: Printed::default(),
            max_bytes,
            follow_ups: RelationshipFollowUps::default(),
        };
        for selector in selectors {
            let _ = writeln!(out, "== {}", printable(selector));
            let found = if let Some((path, target)) = relationship_selector(root, selector) {
                relationship_file_target(
                    &mut out,
                    root,
                    index.as_mut(),
                    scope,
                    path,
                    target,
                    &mut bodies,
                )
            } else {
                let wanted = name_segments(selector);
                let files = hits
                    .get(wanted[wanted.len() - 1])
                    .map_or(&[][..], Vec::as_slice);
                if let Some(file) = files.first()
                    && let Some(&line) = file.hit_lines.first()
                {
                    bodies.follow_ups.remember_source(&file.path, line);
                }
                relationship_name_target(&mut out, index.as_mut(), files, &wanted, &mut remaining)
            };
            if let Some(id) = found
                && seen.insert(id)
            {
                roots.push(id);
            }
        }
        if let Some(index) = &mut index {
            list_relationships(&mut out, root, index, &roots, limit, &mut bodies);
        }
        let follow_ups = [
            bodies.follow_ups.omitted.as_deref(),
            bodies.follow_ups.unresolved.as_deref(),
            bodies.follow_ups.source.as_deref(),
        ];
        lookup_reply(out, max_bytes, index.as_ref(), &follow_ups)
    }))
}

struct RelationshipBodies {
    remaining: usize,
    printed: Printed,
    max_bytes: usize,
    follow_ups: RelationshipFollowUps,
}

#[derive(Default)]
struct RelationshipFollowUps {
    omitted: Option<String>,
    unresolved: Option<String>,
    source: Option<String>,
}

impl RelationshipFollowUps {
    fn remember_source(&mut self, path: &str, line: u32) {
        self.source
            .get_or_insert_with(|| format!("follow-up: read {path}:{line}\n"));
    }
}

fn relationship_file_target(
    out: &mut String,
    root: &Path,
    index: Option<&mut Index<'_>>,
    scope: Option<&ignore::overrides::Override>,
    path: &str,
    target: Target<'_>,
    bodies: &mut RelationshipBodies,
) -> Option<NodeId> {
    let (absolute, relative, missing) =
        match codestory_workspace::resolve_project_relative_path(root, Path::new(path)) {
            Ok(ProjectRelativePathResolution::Existing { absolute, relative }) => {
                (absolute, relative, false)
            }
            Ok(ProjectRelativePathResolution::Missing { absolute, relative }) => {
                (absolute, relative, true)
            }
            _ => {
                out.push_str("relationships unavailable: file missing or outside project\n");
                return None;
            }
        };
    if !in_scope(scope, &relative) {
        out.push_str("file excluded by --file-glob\n");
        return None;
    }
    if missing {
        if let Some(index) = index {
            index.observe_missing_path(&absolute);
        }
        out.push_str("relationships unavailable: file missing\n");
        return None;
    }
    let Some((file, bytes)) = open_live(root, absolute) else {
        out.push_str("relationships unavailable: file unreadable or exceeds source limit\n");
        return None;
    };
    let text = String::from_utf8_lossy(&bytes);
    let (mark, definitions) = index.map_or(("", None), |index| index.serve(&file));
    let candidates = definitions
        .as_ref()
        .map_or_else(Vec::new, |defs| match target {
            Target::Line(line) => {
                let mut scopes = defs
                    .iter()
                    .filter(|def| is_scope_kind(def.kind) && (def.start..=def.end).contains(&line))
                    .collect::<Vec<_>>();
                scopes.sort_by_key(|def| (Reverse(def.start), def.end));
                if let Some(first) = scopes.first().copied() {
                    scopes.retain(|def| (def.start, def.end) == (first.start, first.end));
                }
                scopes
            }
            Target::Symbol(symbol) => {
                let wanted = name_segments(symbol);
                defs.iter()
                    .filter(|def| {
                        !wanted.is_empty()
                            && def.answers_to(&wanted)
                            && !is_local_variable(def, defs)
                    })
                    .collect()
            }
            _ => Vec::new(),
        });
    if let [def] = candidates.as_slice() {
        let _ = writeln!(
            out,
            "{}:{}-{} {}",
            file.path, def.start, def.end, def.qualified
        );
        return Some(def.id);
    }
    if candidates.len() > 1 {
        out.push_str("relationships unavailable: ambiguous declaration\n");
        for def in candidates.iter().take(DEFINITION_BODIES) {
            let _ = writeln!(
                out,
                "{}:{}-{} {}",
                file.path, def.start, def.end, def.qualified
            );
        }
        return None;
    }
    let _ = writeln!(
        out,
        "relationships unavailable: no verified declaration{mark}"
    );
    bodies.live_selection(out, &file.path, &text, target);
    None
}

impl RelationshipBodies {
    fn live_selection(&mut self, out: &mut String, path: &str, text: &str, target: Target<'_>) {
        let lines = text.lines().collect::<Vec<_>>();
        let count = lines.len() as u32;
        let (mut first, mut last, anchor) = match target {
            Target::Line(line) if (1..=count).contains(&line) => {
                let (first, last) = approximate_line_window(line, count);
                (first, last, line)
            }
            Target::Symbol(symbol) => {
                let Some((first, last)) = approximate_declaration(&lines, &name_segments(symbol))
                else {
                    return;
                };
                (first, last, first)
            }
            Target::Range(first, last) if first >= 1 && first <= last && first <= count => {
                (first, last.min(count), first)
            }
            _ => return,
        };
        self.follow_ups.remember_source(path, anchor);
        let mut budget = self.remaining.clamp(1, APPROXIMATE_LINE_CAP);
        if self.remaining == 0 {
            last = anchor;
        }
        if (anchor - first) as usize >= budget {
            first = anchor;
        }
        let initial_budget = budget;
        let gaps = self.printed.unprinted_ranges(path, first, last);
        let mut page = Page {
            path,
            lines: &lines,
            printed: &mut self.printed,
            max_bytes: self.max_bytes,
        };
        for (first, last) in gaps {
            if budget == 0 {
                let _ = writeln!(out, "... more live source; read {path}:{first}-{last}");
                break;
            }
            page.block(
                out,
                &mut budget,
                first,
                last,
                "",
                " (live source; approximate)",
            );
        }
        self.remaining = self.remaining.saturating_sub(initial_budget - budget);
    }
}

fn relationship_name_target(
    out: &mut String,
    mut index: Option<&mut Index<'_>>,
    files: &[LiveFile],
    wanted: &[&str],
    remaining: &mut usize,
) -> Option<NodeId> {
    let count = files.iter().map(|file| file.hit_lines.len()).sum::<usize>();
    let _ = writeln!(
        out,
        "{count} whole-word matching lines in {} files",
        files.len()
    );
    let mut candidates = Vec::new();
    let mut unverified = files.len().saturating_sub(DEFINITION_SEARCH_FILES);
    for file in files.iter().take(DEFINITION_SEARCH_FILES) {
        let defs = index.as_deref_mut().and_then(|index| index.serve(file).1);
        if let Some(defs) = defs {
            candidates.extend(
                defs.iter()
                    .filter(|def| def.answers_to(wanted) && !is_local_variable(def, &defs))
                    .map(|def| {
                        (
                            def.id,
                            &file.path,
                            def.start,
                            def.end,
                            def.qualified.clone(),
                        )
                    }),
            );
        } else {
            unverified += 1;
        }
    }
    if unverified == 0
        && let [(id, path, start, end, name)] = candidates.as_slice()
    {
        let _ = writeln!(out, "{path}:{start}-{end} {name}");
        return Some(*id);
    }
    let reason = if candidates.len() > 1 {
        "ambiguous declaration"
    } else if unverified != 0 {
        "matching files lack verified index facts"
    } else {
        "no verified declaration"
    };
    let _ = writeln!(out, "relationships unavailable: {reason}");
    for file in files {
        let shown = (*remaining).min(file.kept.len());
        for (line, code) in &file.kept[..shown] {
            let _ = writeln!(out, "{}:{line}: {code} (live text)", file.path);
        }
        *remaining -= shown;
    }
    None
}

fn list_relationships(
    out: &mut String,
    root: &Path,
    index: &mut Index<'_>,
    roots: &[NodeId],
    limit: usize,
    bodies: &mut RelationshipBodies,
) {
    use std::collections::VecDeque;

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    enum Follow {
        Both,
        Callers,
        Callees,
    }
    let mut opened = Vec::new();
    let mut opened_ids = HashSet::new();
    let mut queue = VecDeque::new();
    for &id in roots {
        if let Some(definition) = index.locate(root, id) {
            opened_ids.insert(id);
            opened.push(definition);
            queue.push_back((id, Follow::Both));
        }
    }
    let mut seen_edges = HashSet::new();
    let mut seen_sites = HashSet::new();
    let mut visited = HashSet::new();
    let mut sites = 0;
    let mut unavailable = 0;
    let mut excluded = 0;
    'traverse: while let Some((id, follow)) = queue.pop_front() {
        if visited.contains(&(id, Follow::Both)) || !visited.insert((id, follow)) {
            continue;
        }
        let mut edges = Vec::new();
        if follow != Follow::Callers {
            edges = index
                .store
                .get_outgoing_edges_for_node_id(
                    id,
                    &[EdgeKind::CALL],
                    TrailCallerScope::IncludeTestsAndBenches,
                    true,
                )
                .unwrap_or_else(|_| {
                    unavailable += 1;
                    Vec::new()
                });
        }
        if follow != Follow::Callees {
            edges.extend(
                index
                    .store
                    .get_incoming_edges_for_node_id(
                        id,
                        &[EdgeKind::CALL],
                        TrailCallerScope::IncludeTestsAndBenches,
                        true,
                    )
                    .unwrap_or_else(|_| {
                        unavailable += 1;
                        Vec::new()
                    }),
            );
        }
        edges.sort_by_key(|edge| (edge.effective_source() != id, edge.line, edge.id.0));
        for edge in edges {
            if !seen_edges.insert(edge.id) {
                continue;
            }
            let source_id = edge.effective_source();
            let target_id = edge.effective_target();
            if index.relationship_outside_scope(root, &edge) {
                excluded += 1;
                continue;
            }
            let Some((source, target, line)) = index.verified_call(root, &edge) else {
                unavailable += 1;
                if bodies.follow_ups.unresolved.is_none() {
                    let caller_site = edge.line.and_then(|line| {
                        opened
                            .iter()
                            .find(|decl| {
                                decl.definition().id == source_id && decl.owns_callsite(&edge, line)
                            })
                            .map(|decl| (decl.path(), line))
                    });
                    let hint = caller_site.or_else(|| {
                        opened
                            .iter()
                            .find(|decl| decl.definition().id == id)
                            .map(|decl| (decl.path(), decl.first))
                    });
                    bodies.follow_ups.unresolved = hint.map(|(path, line)| {
                        format!("follow-up (unresolved binding): read {path}:{line}\n")
                    });
                }
                continue;
            };
            if !seen_sites.insert((source.home.id, line, source_id, target_id)) {
                continue;
            }
            if sites == limit {
                bodies.follow_ups.omitted = Some(format!(
                    "follow-up (omitted site): read {}:{line}\n",
                    source.path()
                ));
                break 'traverse;
            }
            bodies.follow_ups.remember_source(source.path(), line);
            if sites == 0 {
                out.push_str(
                    "pinned index call bindings (caller and callee files match the index). Other binding inputs may have changed.\n",
                );
            }
            let _ = writeln!(
                out,
                "  {}:{line} {} -> {}",
                source.path(),
                source.definition().qualified,
                target.definition().qualified
            );
            sites += 1;
            if follow == Follow::Both {
                // Continue in the same direction without reopening the caller's siblings.
                if source_id == id {
                    queue.push_back((target_id, Follow::Callees));
                } else {
                    queue.push_back((source_id, Follow::Callers));
                }
            }
            for (id, endpoint) in [(source_id, source), (target_id, target)] {
                if opened_ids.insert(id) {
                    opened.push(endpoint);
                }
            }
        }
    }
    if unavailable != 0 {
        let _ = writeln!(
            out,
            "{unavailable} call edge(s) lack verified source at an endpoint or a certain binding in the pinned index"
        );
        if bodies.follow_ups.unresolved.is_none()
            && let Some(decl) = opened.first()
        {
            bodies.follow_ups.unresolved = Some(format!(
                "follow-up (unresolved binding): read {}:{}\n",
                decl.path(),
                decl.first
            ));
        }
        if let Some(follow_up) = &bodies.follow_ups.unresolved {
            out.push_str(follow_up);
        }
    }
    if excluded != 0 {
        let _ = writeln!(out, "{excluded} call edge(s) excluded by --file-glob");
    }
    if let Some(follow_up) = &bodies.follow_ups.omitted {
        out.push_str("additional call sites omitted from pinned index (--limit)\n");
        out.push_str(follow_up);
    }
    for decl in &opened {
        bodies.follow_ups.remember_source(decl.path(), decl.first);
        let lines = decl.home.text.lines().collect::<Vec<_>>();
        let first = decl.first;
        let last = decl.last;
        let qualified = &decl.definition().qualified;
        let path = decl.path();
        let cost = (last + 1 - first) as usize;
        if cost > bodies.remaining || cost > WHOLE_USED_LINES as usize || !decl.note.is_empty() {
            let _ = write!(out, "{}", decl.pointer());
            continue;
        }
        for (start, end) in bodies.printed.unprinted_ranges(path, first, last) {
            let _ = writeln!(out, "== {path}:{start}-{end} {qualified}");
            for row in start..=end {
                let _ = writeln!(out, "{row}: {}", bounded(lines[row as usize - 1]));
            }
            bodies.printed.add(path, start, end);
            bodies.remaining -= (end + 1 - start) as usize;
        }
    }
}

fn ref_name(name: &str) -> Result<Vec<&str>, ApiError> {
    let wanted = name_segments(name);
    if wanted
        .last()
        .is_some_and(|last| last.bytes().all(is_word_byte))
    {
        return Ok(wanted);
    }
    Err(ApiError::invalid_argument(
        "refs needs a name, qualified name, path::Symbol or path:LINE",
    ))
}

/// Complete declarations for a batch of selectors: `path::Symbol`,
/// `path:LINE` or `path:A-B`. A bare `path` is the whole file when it is
/// short, else an outline of its declarations. A declaration read by symbol
/// or line is followed by `uses:` rows that say where the declarations it
/// uses are. A selector that cannot be served gets its own `==` line and
/// never fails the batch.
pub fn read(root: &Path, storage_path: &Path, selectors: &[String]) -> Result<String, ApiError> {
    read_bounded(root, storage_path, selectors, 0).map(|reply| reply.text)
}

pub fn read_bounded(
    root: &Path,
    storage_path: &Path,
    selectors: &[String],
    max_bytes: usize,
) -> Result<LookupReply, ApiError> {
    if selectors.is_empty() {
        return Err(ApiError::invalid_argument(
            "read needs at least one selector",
        ));
    }
    Ok(with_pinned_index(storage_path, |store| {
        let mut index = store.map(Index::new);
        let mut out = String::new();
        let mut printed = Printed::default();
        for selector in selectors {
            if max_bytes != 0 && out.len() >= max_bytes {
                out.push_str("... selectors omitted (--max-bytes)\n");
                break;
            }
            read_selector(
                &mut out,
                root,
                index.as_mut(),
                &mut printed,
                selector,
                max_bytes,
            );
        }
        lookup_reply(out, max_bytes, index.as_ref(), &[])
    }))
}

fn bound_reply(mut out: String, max_bytes: usize, follow_ups: &[Option<&str>]) -> String {
    const OMITTED: &str = "... output omitted (--max-bytes)\n";
    if max_bytes == 0 || out.len() <= max_bytes {
        return out;
    }
    if max_bytes < OMITTED.len() {
        return "...\n".get(..max_bytes.min(4)).unwrap_or("").into();
    }
    let space = max_bytes - OMITTED.len();
    let follow_up = follow_ups
        .iter()
        .copied()
        .flatten()
        .find(|hint| hint.len() <= space)
        .unwrap_or("");
    let prefix_end = out.as_bytes()[..space - follow_up.len()]
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |last_line| last_line + 1);
    out.truncate(prefix_end);
    if !follow_up.is_empty()
        && !out
            .lines()
            .any(|line| line == follow_up.trim_end_matches('\n'))
    {
        out.push_str(follow_up);
    }
    out.push_str(OMITTED);
    out
}

/// Run `serve` against the pinned complete core publication, or with `None`
/// when there is none to read.
///
/// The observational open resolves the published generation through the
/// logical storage path and holds its read lease; it never repairs, migrates,
/// or creates SQLite sidecars beside a sealed generation.
fn with_pinned_index<T>(storage_path: &Path, serve: impl FnOnce(Option<&Store>) -> T) -> T {
    let Ok(storage) = Store::open_observational(storage_path) else {
        return serve(None);
    };
    let Ok(snapshot) = storage.read_snapshot() else {
        return serve(None);
    };
    let complete = matches!(
        snapshot.storage().get_complete_index_publication(),
        Ok(Some(_))
    );
    serve(complete.then(|| snapshot.storage()))
}

// ---------------------------------------------------------------------------
// Live text
// ---------------------------------------------------------------------------

/// One live file with matching lines, cut from a single read of its bytes.
struct LiveFile {
    /// Project-relative, `/`-separated.
    path: String,
    absolute: PathBuf,
    /// Hash of exactly the bytes `kept` was cut from.
    sha256: String,
    /// Every matching line number, ascending.
    hit_lines: Vec<u32>,
    /// Text of the first matching lines, up to the reply limit.
    kept: Vec<(u32, String)>,
}

impl LiveFile {
    fn rows(&self) -> impl Iterator<Item = Row<'_>> {
        self.kept.iter().map(|(line, code)| Row {
            line: *line,
            code,
            note: String::new(),
        })
    }
}

/// Honor ignore rules and omit hidden files and symlink entries.
fn live_walk(
    root: &Path,
    file_globs: &[String],
) -> Result<impl Iterator<Item = PathBuf>, ApiError> {
    let mut walk = ignore::WalkBuilder::new(root);
    walk.add_custom_ignore_filename(".rgignore");
    if let Some(scope) = lookup_scope(root, file_globs)? {
        walk.overrides(scope);
    }
    Ok(walk.build().filter_map(|entry| {
        let entry = entry.ok()?;
        entry.file_type()?.is_file().then(|| entry.into_path())
    }))
}

fn lookup_scope(
    root: &Path,
    file_globs: &[String],
) -> Result<Option<ignore::overrides::Override>, ApiError> {
    if file_globs.is_empty() {
        return Ok(None);
    }
    let mut scope = ignore::overrides::OverrideBuilder::new(root);
    for glob in file_globs {
        scope
            .add(glob)
            .map_err(|error| ApiError::invalid_argument(error.to_string()))?;
    }
    scope
        .build()
        .map(Some)
        .map_err(|error| ApiError::invalid_argument(error.to_string()))
}

fn in_scope(scope: Option<&ignore::overrides::Override>, path: &Path) -> bool {
    scope.is_none_or(|scope| !scope.matched(path, false).is_ignore())
}

fn live_paths(root: &Path, file_globs: &[String]) -> Result<Vec<String>, ApiError> {
    let mut paths = live_walk(root, file_globs)?
        .map(|path| project_path(root, &path))
        .collect::<Vec<_>>();
    paths.sort_unstable();
    Ok(paths)
}

fn scan_live_tree(
    root: &Path,
    file_globs: &[String],
    needles: &[&[u8]],
    whole_word: bool,
    keep: usize,
) -> Result<Vec<LiveFile>, ApiError> {
    let paths = live_walk(root, file_globs)?.collect::<Vec<_>>();
    let mut files = paths
        .par_iter()
        .map_init(Vec::new, |bytes, path| {
            if !read_live_text(path, bytes) {
                return None;
            }
            match_live_file(root, path, bytes, needles, whole_word, keep)
        })
        .flatten()
        .collect::<Vec<_>>();
    files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

/// Read a searchable text file into `bytes`. Files larger than 4 MB or with a
/// NUL in their first kilobyte are skipped. Structural text must also match
/// the collector's strict source contract.
fn read_live_text(path: &Path, bytes: &mut Vec<u8>) -> bool {
    bytes.clear();
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    if !file
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_FILE_BYTES)
        || file.take(MAX_FILE_BYTES + 1).read_to_end(bytes).is_err()
    {
        return false;
    }
    bytes.len() as u64 <= MAX_FILE_BYTES
        && !bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0)
        && (!codestory_indexer::structural::is_structural_candidate_path(path)
            || codestory_indexer::structural::is_structural_source_text(bytes))
}

fn match_live_file(
    root: &Path,
    path: &Path,
    bytes: &[u8],
    needles: &[&[u8]],
    whole_word: bool,
    keep: usize,
) -> Option<LiveFile> {
    let mut offsets = Vec::new();
    for needle in needles {
        let mut from = 0;
        while let Some(at) = find_from(bytes, needle, from) {
            let end = at + needle.len();
            if whole_word && !(is_word_start(bytes, at) && is_word_end(bytes, end)) {
                from = at + 1;
                continue;
            }
            offsets.push(at);
            // One row per line: resume after this line.
            from = line_end(bytes, end) + 1;
        }
    }
    if offsets.is_empty() {
        return None;
    }
    offsets.sort_unstable();
    let mut hit_lines = Vec::new();
    let mut kept = Vec::new();
    let (mut line, mut counted) = (1_u32, 0_usize);
    for at in offsets {
        line += bytes[counted..at]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count() as u32;
        counted = at;
        if hit_lines.last() == Some(&line) {
            continue;
        }
        hit_lines.push(line);
        if kept.len() < keep {
            let start = bytes[..at]
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |newline| newline + 1);
            kept.push((line, code_cell(&bytes[start..line_end(bytes, at)])));
        }
    }
    Some(LiveFile {
        path: project_path(root, path),
        absolute: path.to_path_buf(),
        sha256: sha256_hex(bytes),
        hit_lines,
        kept,
    })
}

fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let (first, rest) = needle.split_first()?;
    let last_start = haystack.len().checked_sub(needle.len())?;
    let mut at = from;
    while at <= last_start {
        at += haystack[at..=last_start]
            .iter()
            .position(|byte| byte == first)?;
        if haystack[at + 1..at + needle.len()] == *rest {
            return Some(at);
        }
        at += 1;
    }
    None
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |newline| from + newline)
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_word_start(bytes: &[u8], at: usize) -> bool {
    at == 0 || !is_word_byte(bytes[at - 1])
}

fn is_word_end(bytes: &[u8], end: usize) -> bool {
    bytes.get(end).is_none_or(|byte| !is_word_byte(*byte))
}

fn word_offset(line: &str, word: &str) -> Option<usize> {
    let (bytes, needle) = (line.as_bytes(), word.as_bytes());
    let mut from = 0;
    while let Some(at) = find_from(bytes, needle, from) {
        if is_word_start(bytes, at) && is_word_end(bytes, at + needle.len()) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn project_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

/// Keep terminal control bytes in source out of the reply.
fn printable(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_control() && ch != '\t' {
                char::REPLACEMENT_CHARACTER
            } else {
                ch
            }
        })
        .collect()
}

/// One listed code line: trimmed and bounded.
fn code_cell(line: &[u8]) -> String {
    bounded(String::from_utf8_lossy(line).trim())
}

fn bounded(text: &str) -> String {
    match text.char_indices().nth(MAX_CODE_CHARS) {
        Some((cut, _)) => format!("{}…", printable(&text[..cut])),
        None => printable(text),
    }
}

/// One read of a live file: its bytes, and their hash under the path a reply
/// shows for it.
fn open_live(root: &Path, absolute: PathBuf) -> Option<(LiveFile, Vec<u8>)> {
    if !std::fs::symlink_metadata(&absolute).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    if !read_live_text(&absolute, &mut bytes) {
        return None;
    }
    let file = LiveFile {
        path: project_path(root, &absolute),
        absolute,
        sha256: sha256_hex(&bytes),
        hit_lines: Vec::new(),
        kept: Vec::new(),
    };
    Some((file, bytes))
}

/// The text of `file`, provided its bytes are still the ones its hits and
/// its index verification were cut from.
fn live_text(file: &LiveFile) -> Option<String> {
    let mut bytes = Vec::new();
    (read_live_text(&file.absolute, &mut bytes) && sha256_hex(&bytes) == file.sha256)
        .then(|| String::from_utf8_lossy(&bytes).into_owned())
}

// ---------------------------------------------------------------------------
// Index facts for verified files
// ---------------------------------------------------------------------------

/// A definition the index records in one file.
struct Definition {
    id: NodeId,
    kind: NodeKind,
    name: String,
    qualified: String,
    start: u32,
    end: u32,
}

impl Definition {
    /// Whether this definition's name ends with the wanted name segments.
    fn answers_to(&self, wanted: &[&str]) -> bool {
        name_segments(&self.name).ends_with(wanted)
            || name_segments(&self.qualified).ends_with(wanted)
    }
}

type Served = (&'static str, Option<Rc<Vec<Definition>>>);

/// A project file read once whose bytes the index describes.
struct Verified {
    id: NodeId,
    file: LiveFile,
    text: String,
    definitions: Rc<Vec<Definition>>,
}

/// A definition in a verified file, over the lines it spans in the live text.
struct Located {
    home: Rc<Verified>,
    /// Its position among the definitions of `home`.
    slot: usize,
    first: u32,
    last: u32,
    /// Says so when `last` is not from the index.
    note: &'static str,
}

impl Located {
    fn definition(&self) -> &Definition {
        &self.home.definitions[self.slot]
    }

    fn path(&self) -> &str {
        &self.home.file.path
    }

    fn owns_callsite(&self, edge: &Edge, line: u32) -> bool {
        let def = self.definition();
        edge.file_node_id == Some(self.home.id)
            && (def.start..=def.end.min(self.last)).contains(&line)
    }

    /// One row that says where to read it.
    fn pointer(&self) -> String {
        let definition = self.definition();
        format!(
            "  {}:{}-{} {} {}\n",
            self.path(),
            self.first,
            self.last,
            kind_label(definition.kind),
            definition.qualified
        )
    }
}

/// Index facts, handed out per file and only for bytes the index describes.
struct Index<'a> {
    store: &'a Store,
    scope: Option<&'a ignore::overrides::Override>,
    refresh_recommended: bool,
    /// What was served for a path, and the hash of the bytes it was served for.
    served: HashMap<PathBuf, (String, Served)>,
    /// Files opened by their index id, `None` for one that is not verified.
    opened: HashMap<i64, Option<Rc<Verified>>>,
}

impl<'a> Index<'a> {
    fn verified_call(&mut self, root: &Path, edge: &Edge) -> Option<(Located, Located, u32)> {
        let line = edge.line.filter(|&line| line > 0)?;
        if edge.kind != EdgeKind::CALL
            || edge.file_node_id.is_none()
            || !is_certain(edge)
            || !edge.candidate_targets.is_empty()
        {
            return None;
        }
        let caller = self.locate(root, edge.effective_source())?;
        if !caller.owns_callsite(edge, line) {
            return None;
        }
        Some((caller, self.locate(root, edge.effective_target())?, line))
    }

    fn new(store: &'a Store) -> Self {
        Self::scoped(store, None)
    }

    fn scoped(store: &'a Store, scope: Option<&'a ignore::overrides::Override>) -> Self {
        Self {
            store,
            scope,
            refresh_recommended: false,
            served: HashMap::new(),
            opened: HashMap::new(),
        }
    }

    /// The path resolver already confirmed this missing path is inside the project.
    fn observe_missing_path(&mut self, absolute: &Path) {
        self.refresh_recommended |= self
            .store
            .get_file_by_path(absolute)
            .is_ok_and(|row| row.is_some());
    }

    fn relationship_outside_scope(&self, root: &Path, edge: &Edge) -> bool {
        let Some(scope) = self.scope else {
            return false;
        };
        let file_excluded = |id: NodeId| {
            self.store
                .get_file_by_id(id.0)
                .ok()
                .flatten()
                .is_some_and(|file| {
                    file.path
                        .strip_prefix(root)
                        .is_ok_and(|path| !in_scope(Some(scope), path))
                })
        };
        edge.file_node_id.is_some_and(file_excluded)
            || [edge.effective_source(), edge.effective_target()]
                .into_iter()
                .any(|id| {
                    self.store
                        .get_node(id)
                        .ok()
                        .flatten()
                        .and_then(|node| node.file_node_id)
                        .is_some_and(file_excluded)
                })
    }

    /// The file the index records as `file_id`, when it is inside the project
    /// and its live bytes hash to the indexed `content_hash`.
    fn open(&mut self, root: &Path, file_id: i64) -> Option<Rc<Verified>> {
        if let Some(opened) = self.opened.get(&file_id) {
            return opened.clone();
        }
        let opened = self.verify(root, file_id).map(Rc::new);
        self.opened.insert(file_id, opened.clone());
        opened
    }

    fn verify(&mut self, root: &Path, file_id: i64) -> Option<Verified> {
        let absolute = self.store.get_file_by_id(file_id).ok()??.path;
        if !in_scope(self.scope, absolute.strip_prefix(root).ok()?) {
            return None;
        }
        let Ok(ProjectRelativePathResolution::Existing { absolute, relative }) =
            codestory_workspace::resolve_project_relative_path(root, &absolute)
        else {
            self.refresh_recommended = true;
            return None;
        };
        if !in_scope(self.scope, &relative) {
            return None;
        }
        let Some((file, bytes)) = open_live(root, absolute) else {
            self.refresh_recommended = true;
            return None;
        };
        let definitions = self.serve(&file).1?;
        Some(Verified {
            id: NodeId(file_id),
            text: String::from_utf8_lossy(&bytes).into_owned(),
            file,
            definitions,
        })
    }

    /// The declaration of `id`, when the index records one in a verified file.
    fn locate(&mut self, root: &Path, id: NodeId) -> Option<Located> {
        let file_id = self.store.get_node(id).ok()??.file_node_id?;
        let home = self.open(root, file_id.0)?;
        let slot = home
            .definitions
            .iter()
            .position(|definition| definition.id == id)?;
        let (first, last, note) = declared_range(
            &home.text.lines().collect::<Vec<_>>(),
            &home.definitions[slot],
        )?;
        Some(Located {
            home,
            slot,
            first,
            last,
            note,
        })
    }

    /// The header mark for `file` and, only when the bytes just read hash to
    /// the indexed `content_hash`, the definitions recorded for it.
    fn serve(&mut self, file: &LiveFile) -> Served {
        if let Some((sha256, served)) = self.served.get(&file.absolute)
            && *sha256 == file.sha256
        {
            return served.clone();
        }
        let served = match self.store.get_file_by_path(&file.absolute) {
            Ok(None) => (" (not indexed)", None),
            Ok(Some(row)) => match self.store.get_file_content_hash(row.id) {
                Ok(Some(hash)) if hash == file.sha256 => {
                    match definitions_in(self.store, row.id, &row.language) {
                        Ok(definitions) => ("", Some(Rc::new(definitions))),
                        Err(_) => (" (index unreadable)", None),
                    }
                }
                Ok(_) => (" (changed since indexing)", None),
                Err(_) => (" (index unreadable)", None),
            },
            Err(_) => (" (index unreadable)", None),
        };
        self.refresh_recommended |= served.1.is_none();
        self.served
            .insert(file.absolute.clone(), (file.sha256.clone(), served.clone()));
        served
    }
}

fn definitions_in(
    store: &Store,
    file_id: i64,
    language: &str,
) -> Result<Vec<Definition>, StorageError> {
    let mut ids = store
        .get_occurrences_for_file(NodeId(file_id))?
        .into_iter()
        .filter(|occurrence| {
            matches!(
                occurrence.kind,
                OccurrenceKind::DEFINITION
                    | OccurrenceKind::DECLARATION
                    | OccurrenceKind::MACRO_DEFINITION
            )
        })
        .map(|occurrence| NodeId(occurrence.element_id))
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    let mut definitions = store
        .get_nodes_by_ids(&ids)?
        .into_values()
        .filter_map(|node| {
            let (start, end) = (node.start_line?, node.end_line?);
            let named = !matches!(
                node.kind,
                NodeKind::UNKNOWN
                    | NodeKind::FILE
                    | NodeKind::BUILTIN_TYPE
                    | NodeKind::TYPE_PARAMETER
            );
            // Rust extraction represents import bindings as MODULE nodes carrying IMPORT edges.
            // Their live source can be a text hit, but it is not a module or symbol definition.
            let declaration = language != "rust"
                || node.kind != NodeKind::MODULE
                || store
                    .get_outgoing_edges_for_node_id(
                        node.id,
                        &[EdgeKind::IMPORT],
                        TrailCallerScope::IncludeTestsAndBenches,
                        true,
                    )
                    .is_ok_and(|imports| imports.is_empty());
            let here = node.file_node_id == Some(NodeId(file_id));
            (named && declaration && here && start >= 1 && end >= start).then(|| Definition {
                id: node.id,
                kind: node.kind,
                qualified: node
                    .qualified_name
                    .unwrap_or_else(|| node.serialized_name.clone()),
                name: node.serialized_name,
                start,
                end,
            })
        })
        .collect::<Vec<_>>();
    definitions.sort_by(|left, right| {
        (left.start, right.end, &left.name).cmp(&(right.start, left.end, &right.name))
    });
    Ok(definitions)
}

fn is_scope_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::UNION
            | NodeKind::ENUM
            | NodeKind::TYPEDEF
            | NodeKind::FUNCTION
            | NodeKind::METHOD
            | NodeKind::MACRO
    )
}

fn is_callable_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    )
}

/// The innermost function/method/class-like definition whose range holds `line`.
fn enclosing_scope(definitions: &[Definition], line: u32) -> Option<&Definition> {
    definitions
        .iter()
        .filter(|definition| {
            is_scope_kind(definition.kind) && definition.start <= line && line <= definition.end
        })
        .max_by_key(|definition| (definition.start, std::cmp::Reverse(definition.end)))
}

fn kind_label(kind: NodeKind) -> String {
    format!("{kind:?}").to_ascii_lowercase()
}

/// The lines a definition spans: the range the index records, within the
/// live text. A range that covers only the line opening a block (some
/// languages index a class by its name alone) gets its end from the text,
/// and the note says so.
fn declared_range(lines: &[&str], definition: &Definition) -> Option<(u32, u32, &'static str)> {
    let count = lines.len() as u32;
    if count == 0 {
        return None;
    }
    let (first, mut last) = (definition.start.min(count), definition.end.min(count));
    let mut note = "";
    if first == last && opens_block(lines[first as usize - 1]) {
        last = block_end(lines, first as usize - 1) as u32 + 1;
        if last > first {
            note = " (end not from index; approximate)";
        }
    }
    Some((first, last, note))
}

fn name_segments(name: &str) -> Vec<&str> {
    name.split("::")
        .flat_map(|part| part.split('.'))
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// The declarations that `owner` and the definitions nested in it reference,
/// in order of first use and `most` at the most: the targets of the outgoing
/// edges the index resolved with certainty, each one only as a declaration in
/// a verified file. A guessed resolution names nothing. `owner` itself, what
/// is nested in it, namespaces, and what `shown` reports are left out.
fn used_by(
    index: &mut Index<'_>,
    root: &Path,
    owner: &Located,
    most: usize,
    shown: impl Fn(&Located) -> bool,
) -> Vec<Located> {
    let nested = |first: u32, last: u32| owner.first <= first && last <= owner.last;
    // Where each edge is in the text, or else the definition it starts from.
    let mut sites = Vec::new();
    for source in owner
        .home
        .definitions
        .iter()
        .filter(|source| nested(source.start, source.end))
    {
        let edges = index
            .store
            .get_outgoing_edges_for_node_id(
                source.id,
                &[],
                TrailCallerScope::IncludeTestsAndBenches,
                true,
            )
            .unwrap_or_default();
        for edge in edges {
            if edge.kind != EdgeKind::MEMBER && is_certain(&edge) {
                sites.push((
                    edge.line.unwrap_or(source.start),
                    edge.id.0,
                    edge.effective_target(),
                ));
            }
        }
    }
    sites.sort_unstable();
    let mut seen = HashSet::new();
    let mut used = Vec::new();
    for (_, _, target) in sites {
        if used.len() == most {
            break;
        }
        if seen.insert(target)
            && let Some(target) = index.locate(root, target)
            && !(Rc::ptr_eq(&target.home, &owner.home) && nested(target.first, target.last))
            && !matches!(
                target.definition().kind,
                NodeKind::MODULE | NodeKind::NAMESPACE | NodeKind::PACKAGE
            )
            && !shown(&target)
        {
            used.push(target);
        }
    }
    used
}

/// Whether the index is certain of the resolution `edge` records.
fn is_certain(edge: &Edge) -> bool {
    edge.certainty
        .or_else(|| ResolutionCertainty::from_confidence(edge.confidence))
        == Some(ResolutionCertainty::Certain)
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

struct Row<'a> {
    line: u32,
    code: &'a str,
    note: String,
}

/// One hit as one line: `<line> [enclosing definition] <code><note>`.
fn list_row(out: &mut String, row: &Row<'_>, definitions: Option<&[Definition]>) {
    let _ = write!(out, "  {}", row.line);
    if let Some(scope) = definitions.and_then(|definitions| enclosing_scope(definitions, row.line))
    {
        let _ = write!(out, " [{}]", scope.name);
    }
    if !row.code.is_empty() {
        let _ = write!(out, " {}", row.code);
    }
    let _ = writeln!(out, "{}", row.note);
}

/// List one file's rows under its header while the reply limit lasts; rows
/// past the limit are counted, never dropped. With `bodies`, a listed row is
/// shown inside its declaration or a window of context while the budget
/// lasts.
fn list_file(
    out: &mut String,
    remaining: &mut usize,
    file: &LiveFile,
    (mark, definitions): Served,
    rows: &[Row<'_>],
    unlisted: usize,
    bodies: Option<&mut Bodies>,
) {
    let path = &file.path;
    let definitions = definitions.as_deref().map(Vec::as_slice);
    let shown = rows.len().min(*remaining);
    *remaining -= shown;
    if shown > 0 {
        let _ = writeln!(out, "{path}{mark}");
    }
    let mut listed = &rows[..shown];
    if let Some(bodies) = bodies {
        listed = list_bodies(out, bodies, file, definitions, listed);
        bodies.overflowed |= !listed.is_empty();
    }
    for row in listed {
        list_row(out, row, definitions);
    }
    let more = rows.len() - shown + unlisted;
    if more > 0 {
        let _ = writeln!(out, "{path}: {more} more");
    }
}

/// List text hits as they were found, with index facts for the files `index`
/// still describes.
fn list_hits(
    out: &mut String,
    files: &[LiveFile],
    limit: usize,
    mut index: Option<&mut Index<'_>>,
    mut bodies: Option<&mut Bodies>,
) {
    let mut remaining = limit;
    for file in files {
        let served = match index.as_deref_mut() {
            Some(index) if remaining > 0 => index.serve(file),
            _ => ("", None),
        };
        let rows = file.rows().collect::<Vec<_>>();
        list_file(
            out,
            &mut remaining,
            file,
            served,
            &rows,
            file.hit_lines.len() - rows.len(),
            bodies.as_deref_mut(),
        );
    }
}

fn render_hit_listing(
    heading: String,
    files: &[LiveFile],
    limit: usize,
    mut index: Option<&mut Index<'_>>,
    body_lines: Option<usize>,
    max_bytes: usize,
) -> String {
    prefer_hit_locations(body_lines, max_bytes, |body_lines| {
        let mut out = heading.clone();
        let mut bodies = body_lines.map(Bodies::new);
        list_hits(
            &mut out,
            files,
            limit,
            index.as_deref_mut(),
            bodies.as_mut(),
        );
        finish_bodies(&mut out, bodies);
        out
    })
}

fn prefer_hit_locations(
    body_lines: Option<usize>,
    max_bytes: usize,
    mut render: impl FnMut(Option<usize>) -> String,
) -> String {
    let full = render(body_lines);
    if body_lines.is_none() || max_bytes == 0 || full.len() <= max_bytes {
        return full;
    }
    let mut compact = render(None);
    if compact.len().saturating_add(MORE_BODIES.len()) <= max_bytes {
        compact.push_str(MORE_BODIES);
    }
    compact
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/// Line ranges one reply has printed, by the path it shows for their file.
#[derive(Default)]
struct Printed(HashMap<String, Vec<(u32, u32)>>);

impl Printed {
    fn has_line(&self, path: &str, line: u32) -> bool {
        self.0.get(path).is_some_and(|ranges| {
            ranges
                .iter()
                .any(|&(first, last)| (first..=last).contains(&line))
        })
    }

    fn add(&mut self, path: &str, first: u32, last: u32) {
        self.0
            .entry(path.to_owned())
            .or_default()
            .push((first, last));
    }

    fn unprinted_ranges(&self, path: &str, mut first: u32, last: u32) -> Vec<(u32, u32)> {
        let mut gaps = Vec::new();
        while first <= last {
            while first <= last && self.has_line(path, first) {
                first += 1;
            }
            let mut next = first;
            while next <= last && !self.has_line(path, next) {
                next += 1;
            }
            if next > first {
                gaps.push((first, next - 1));
            }
            first = next;
        }
        gaps
    }

    /// Whether one printed range shows all the lines of `declaration`.
    fn holds(&self, declaration: &Located) -> bool {
        self.0.get(declaration.path()).is_some_and(|ranges| {
            ranges
                .iter()
                .any(|(first, last)| *first <= declaration.first && declaration.last <= *last)
        })
    }
}

/// What one reply may still spend on declaration bodies and context.
struct Bodies {
    /// Lines left, counting `==` headers and `...` gap marks.
    budget: usize,
    /// Declarations already printed, so that none is printed twice.
    declared: HashSet<NodeId>,
    /// The lines printed for them and for context.
    printed: Printed,
    /// Whether a hit or a used declaration stayed one row because its body
    /// did not fit.
    overflowed: bool,
}

impl Bodies {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            declared: HashSet::new(),
            printed: Printed::default(),
            overflowed: false,
        }
    }

    /// Whether the reply already shows `declaration`, whole or windowed.
    fn shows(&self, declaration: &Located) -> bool {
        self.declared.contains(&declaration.definition().id) || self.printed.holds(declaration)
    }
}

const MORE_BODIES: &str = "more: read <path>::<Symbol> or <path>:A-B\n";

/// Say how to read the bodies that did not fit.
fn finish_bodies(out: &mut String, bodies: Option<Bodies>) {
    if bodies.is_some_and(|bodies| bodies.overflowed) {
        out.push_str(MORE_BODIES);
    }
}

/// A definition, the lines it spans in the live text, and a note when the
/// last of them is not from the index.
type Declared<'a> = (&'a Definition, u32, u32, &'static str);

/// One body under a file header: a declaration, whole or windowed around its
/// hits, or a plain window of context.
struct Block<'a> {
    declaration: Option<Declared<'a>>,
    /// Line ranges to print, ascending and apart.
    windows: Vec<(u32, u32)>,
    /// The hit lines it shows, ascending.
    hits: Vec<u32>,
}

fn window_around(line: u32, radius: u32, low: u32, high: u32) -> (u32, u32) {
    let line = line.clamp(low, high);
    (
        line.saturating_sub(radius).max(low),
        (line + radius).min(high),
    )
}

/// Add a window that starts no earlier than the last one, merging the two
/// when they touch.
fn add_window(windows: &mut Vec<(u32, u32)>, window: (u32, u32)) {
    match windows.last_mut() {
        Some(last) if window.0 <= last.1 + 1 => last.1 = last.1.max(window.1),
        _ => windows.push(window),
    }
}

/// Group ascending hit lines into the bodies that show them, ordered by each
/// body's first hit. A hit belongs to its innermost declaration, or to the
/// widest declaration printed whole that holds it, so a nested declaration
/// is not repeated beside the one that contains it.
fn plan_bodies<'a>(
    lines: &[&str],
    definitions: Option<&'a [Definition]>,
    hits: &[u32],
) -> Vec<Block<'a>> {
    let count = lines.len() as u32;
    let scopes = hits
        .iter()
        .map(|line| {
            let scope = enclosing_scope(definitions?, *line)?;
            let (first, last, note) = declared_range(lines, scope)?;
            Some((scope, first, last, note))
        })
        .collect::<Vec<Option<Declared<'a>>>>();
    let whole = |scope: &Declared<'_>| scope.2 - scope.1 < WHOLE_DECLARATION_LINES;
    let mut blocks = Vec::<Block<'a>>::new();
    for (line, scope) in hits.iter().copied().zip(&scopes) {
        let owner = scopes
            .iter()
            .flatten()
            .filter(|scope| whole(scope) && (scope.1..=scope.2).contains(&line))
            .min_by_key(|scope| (scope.1, Reverse(scope.2)))
            .or(scope.as_ref())
            .copied();
        let window = match owner {
            Some(owner) if whole(&owner) => (owner.1, owner.2),
            Some((_, first, last, _)) => window_around(line, DECLARATION_WINDOW, first, last),
            None => window_around(line, TEXT_WINDOW, 1, count),
        };
        let known = blocks
            .iter_mut()
            .rev()
            .find(|block| match (block.declaration, owner) {
                (Some(left), Some(right)) => left.0.id == right.0.id,
                (None, None) => block
                    .windows
                    .last()
                    .is_some_and(|last| window.0 <= last.1 + 1),
                _ => false,
            });
        match known {
            Some(block) => {
                block.hits.push(line);
                add_window(&mut block.windows, window);
            }
            None => blocks.push(Block {
                declaration: owner,
                windows: vec![window],
                hits: vec![line],
            }),
        }
    }
    blocks
}

/// Print `block` of the file shown as `path` window by window while the
/// budget lasts and return the last line printed, 0 when nothing fit. `tail`
/// is the body line printed just above under the same file header, if any.
/// Hit lines are the keys of `notes` and end in `>`; every other line ends
/// in `:`.
fn print_body(
    out: &mut String,
    bodies: &mut Bodies,
    path: &str,
    lines: &[&str],
    block: &Block<'_>,
    notes: &BTreeMap<u32, &str>,
    tail: u32,
) -> u32 {
    if block
        .declaration
        .is_some_and(|(definition, ..)| bodies.declared.contains(&definition.id))
    {
        return 0;
    }
    let mut end = 0;
    for (first, last) in block.windows.iter().copied() {
        let title = block.declaration.filter(|_| end == 0);
        let gap = match block.declaration {
            Some(_) => end != 0,
            None => tail != 0 && tail + 1 != first,
        };
        let cost = (last + 1 - first) as usize + usize::from(title.is_some()) + usize::from(gap);
        if cost > bodies.budget {
            break;
        }
        bodies.budget -= cost;
        bodies.printed.add(path, first, last);
        if let Some((definition, first, last, note)) = title {
            bodies.declared.insert(definition.id);
            let _ = writeln!(out, "  == {} {first}-{last}{note}", definition.qualified);
        }
        if gap {
            out.push_str("  ...\n");
        }
        for number in first..=last {
            let (sign, note) = match notes.get(&number) {
                Some(note) => ('>', *note),
                None => (':', ""),
            };
            let _ = write!(out, "  {number}{sign}");
            let code = bounded(lines[number as usize - 1].trim_end());
            if !code.is_empty() {
                let _ = write!(out, " {code}");
            }
            let _ = writeln!(out, "{note}");
        }
        end = last;
    }
    end
}

/// Show one file's listed rows inside their declarations, or inside windows
/// of context where the index offers none, and return the rows left to list
/// one line each. A row whose body does not fit the budget is listed that
/// way right after what was printed of its declaration.
fn list_bodies<'r, 'c>(
    out: &mut String,
    bodies: &mut Bodies,
    file: &LiveFile,
    definitions: Option<&[Definition]>,
    rows: &'r [Row<'c>],
) -> &'r [Row<'c>] {
    // A body spends a line on each hit it shows, so this bounds the planning.
    let (fitting, rest) = rows.split_at(rows.len().min(bodies.budget));
    let Some(text) = (!fitting.is_empty())
        .then(|| live_text(file))
        .flatten()
        .filter(|text| !text.is_empty())
    else {
        return rows;
    };
    let lines = text.lines().collect::<Vec<_>>();
    let by_line = fitting
        .iter()
        .map(|row| (row.line, row))
        .collect::<BTreeMap<_, _>>();
    let notes = fitting
        .iter()
        .map(|row| (row.line, row.note.as_str()))
        .collect::<BTreeMap<_, _>>();
    let hits = by_line.keys().copied().collect::<Vec<_>>();
    let mut tail = 0;
    for block in plan_bodies(&lines, definitions, &hits) {
        let end = print_body(out, bodies, &file.path, &lines, &block, &notes, tail);
        tail = end;
        for row in block
            .hits
            .iter()
            .filter(|line| **line > end)
            .filter_map(|line| by_line.get(line))
        {
            list_row(out, row, definitions);
            (bodies.overflowed, tail) = (true, 0);
        }
    }
    rest
}

/// Print a definition's declaration under its header line: whole, or windowed
/// around its own line and the hits it directly holds. Returns the line
/// ranges printed, whose hits need no listing below.
fn definition_body(
    out: &mut String,
    bodies: &mut Bodies,
    file: &LiveFile,
    definitions: &[Definition],
    (definition, own_line): (&Definition, u32),
    hits: &[(u32, String)],
) -> Vec<(u32, u32)> {
    let Some(text) = live_text(file) else {
        return Vec::new();
    };
    let lines = text.lines().collect::<Vec<_>>();
    let Some((first, last, note)) = declared_range(&lines, definition) else {
        return Vec::new();
    };
    let mut windows = Vec::new();
    if last - first < WHOLE_DECLARATION_LINES {
        windows.push((first, last));
    } else {
        let held = hits.iter().map(|(line, _)| *line).filter(|line| {
            *line > own_line
                && enclosing_scope(definitions, *line)
                    .is_some_and(|scope| scope.id == definition.id)
        });
        for line in std::iter::once(own_line).chain(held) {
            add_window(
                &mut windows,
                window_around(line, DECLARATION_WINDOW, first, last),
            );
        }
    }
    let block = Block {
        declaration: Some((definition, first, last, note)),
        windows,
        hits: Vec::new(),
    };
    let notes = hits
        .iter()
        .map(|(line, note)| (*line, note.as_str()))
        .collect();
    let end = print_body(out, bodies, &file.path, &lines, &block, &notes, 0);
    let mut printed = block.windows;
    printed.retain(|window| window.1 <= end);
    printed
}

/// After the references: the declarations `owner` uses that the reply does
/// not show yet. Each is printed whole under its file while it is short and
/// the budget lasts; otherwise one row says where to read it.
fn list_used(out: &mut String, bodies: &mut Bodies, owner: &str, used: &[Located]) {
    let (mut titled, mut header) = (false, None);
    for target in used {
        if bodies.shows(target) {
            continue;
        }
        if !titled {
            let _ = writeln!(out, "{owner} {PINNED_USES}");
            titled = true;
        }
        let mut body = String::new();
        if target.last - target.first < WHOLE_USED_LINES {
            let block = Block {
                declaration: Some((target.definition(), target.first, target.last, target.note)),
                windows: vec![(target.first, target.last)],
                hits: Vec::new(),
            };
            let lines = target.home.text.lines().collect::<Vec<_>>();
            let notes = BTreeMap::new();
            print_body(&mut body, bodies, target.path(), &lines, &block, &notes, 0);
            bodies.overflowed |= body.is_empty();
        }
        if body.is_empty() {
            out.push_str(&target.pointer());
            header = None;
            continue;
        }
        if header != Some(target.path()) {
            let _ = writeln!(out, "{}", target.path());
            header = Some(target.path());
        }
        out.push_str(&body);
    }
}

// ---------------------------------------------------------------------------
// refs
// ---------------------------------------------------------------------------

/// A definition named like the query, found in a verified hit file.
struct Candidate {
    /// Position of its file among the hit files.
    file: usize,
    /// The hit line that carries its name.
    line: u32,
    id: NodeId,
    kind: NodeKind,
    qualified: String,
    /// Whether it is one of the definitions the query asks about.
    wanted: bool,
}

#[allow(clippy::too_many_arguments)]
fn refs_with_index(
    root: &Path,
    index: &mut Index<'_>,
    name: &str,
    wanted: &[&str],
    files: &[LiveFile],
    limit: usize,
    body_lines: Option<usize>,
    max_bytes: usize,
) -> String {
    let store = index.store;
    let last = wanted[wanted.len() - 1];

    // Definitions come from the index, but only through hit files it still
    // describes: a declaration line always mentions the name.
    let mut listed_files = 0;
    let mut listed_lines = 0;
    while listed_files < files.len() && listed_lines < limit {
        listed_lines += files[listed_files].hit_lines.len();
        listed_files += 1;
    }
    let searched = files.len().min(listed_files + DEFINITION_SEARCH_FILES);
    let mut candidates = Vec::new();
    for (position, file) in files[..searched].iter().enumerate() {
        let Some(definitions) = index.serve(file).1 else {
            continue;
        };
        for definition in definitions.iter() {
            if name_segments(&definition.name).last() != Some(&last)
                || is_local_variable(definition, &definitions)
            {
                continue;
            }
            candidates.push(Candidate {
                file: position,
                line: file
                    .hit_lines
                    .iter()
                    .copied()
                    .find(|line| (definition.start..=definition.end).contains(line))
                    .unwrap_or(definition.start),
                id: definition.id,
                kind: definition.kind,
                qualified: definition.qualified.clone(),
                wanted: definition.answers_to(wanted),
            });
        }
    }
    let reported = candidates
        .iter()
        .filter(|candidate| candidate.wanted)
        .count();
    // A qualifier narrows the reply only when the index knows that definition.
    let narrowing = reported > 0 && reported < candidates.len();

    // What the index ties to each hit line: the definition declared there and
    // the definitions a reference there resolved to with certainty. A guessed
    // resolution never adds an arrow or omits a hit.
    let positions = files
        .iter()
        .enumerate()
        .map(|(position, file)| (file.absolute.as_path(), position))
        .collect::<HashMap<_, _>>();
    let mut defined_at = HashMap::<(usize, u32), Vec<usize>>::new();
    for (slot, candidate) in candidates.iter().enumerate() {
        defined_at
            .entry((candidate.file, candidate.line))
            .or_default()
            .push(slot);
    }
    let mut resolved_at = HashMap::<(usize, u32), Vec<usize>>::new();
    let mut index_only = BTreeMap::<i64, BTreeSet<u32>>::new();
    let mut verified_hit_files = HashMap::<i64, Option<usize>>::new();
    for (slot, candidate) in candidates.iter().enumerate() {
        for ((file_id, line), certain) in reference_sites(store, candidate.id) {
            let position = *verified_hit_files.entry(file_id).or_insert_with(|| {
                let position =
                    *positions.get(store.get_file_by_id(file_id).ok()??.path.as_path())?;
                (store.get_file_content_hash(file_id).ok()?? == files[position].sha256)
                    .then_some(position)
            });
            let hit =
                position.filter(|position| files[*position].hit_lines.binary_search(&line).is_ok());
            match hit {
                Some(position) if certain => {
                    resolved_at.entry((position, line)).or_default().push(slot);
                }
                None if candidate.wanted && certain => {
                    index_only.entry(file_id).or_default().insert(line);
                }
                _ => {}
            }
        }
    }
    let claims = |position: usize, line: u32| {
        defined_at
            .get(&(position, line))
            .into_iter()
            .chain(resolved_at.get(&(position, line)))
            .flatten()
            .map(|slot| &candidates[*slot])
    };
    let omitted = |position: usize, line: u32| {
        narrowing
            && claims(position, line).next().is_some()
            && claims(position, line).all(|candidate| !candidate.wanted)
    };
    let defines = |position: usize, line: u32| {
        defined_at
            .get(&(position, line))
            .is_some_and(|slots| slots.iter().any(|slot| candidates[*slot].wanted))
    };
    // With several definitions, say which one the index resolved a line to
    // when it resolved it to exactly one.
    let note = |position: usize, line: u32| {
        let mut targets = resolved_at
            .get(&(position, line))
            .into_iter()
            .flatten()
            .map(|slot| &candidates[*slot])
            .filter(|candidate| candidate.wanted);
        match targets.next() {
            Some(target)
                if reported > 1
                    && !defines(position, line)
                    && targets.all(|other| other.id == target.id) =>
            {
                format!(
                    " -> {}:{} (pinned index)",
                    files[target.file].path, target.line
                )
            }
            _ => String::new(),
        }
    };

    // Reference lines the index records in verified files that the text
    // search did not find, read live like every other line.
    let index_only = index_only
        .into_iter()
        .filter_map(|(file_id, lines)| index_only_file(root, index, file_id, &lines))
        .filter(|file| {
            positions
                .get(file.absolute.as_path())
                .is_none_or(|position| files[*position].sha256 == file.sha256)
        })
        .collect::<Vec<_>>();
    let mut listing = files
        .iter()
        .enumerate()
        .map(|(position, file)| (file, Some(position), None))
        .collect::<Vec<_>>();
    for file in &index_only {
        match positions.get(file.absolute.as_path()) {
            Some(position) => listing[*position].2 = Some(file),
            None => listing.push((file, None, Some(file))),
        }
    }
    listing.sort_by(|left, right| left.0.path.cmp(&right.0.path));

    let (mut references, mut reference_files, mut omitted_hits) = (0, 0, 0);
    for (_, position, index_only) in &listing {
        let mut count = index_only.map_or(0, |file| file.kept.len());
        if let Some(position) = *position {
            for line in &files[position].hit_lines {
                if omitted(position, *line) {
                    omitted_hits += 1;
                } else if !defines(position, *line) {
                    count += 1;
                }
            }
        }
        references += count;
        reference_files += usize::from(count > 0);
    }

    // Both layouts use the same admitted references and the cached file observations.
    prefer_hit_locations(body_lines, max_bytes, |body_lines| {
        let mut body_state = body_lines.map(Bodies::new);
        let mut bodies = body_state.as_mut();
        let mut out = format!("{name}: {reported} definition(s)");
        if searched < files.len() {
            let _ = write!(
                out,
                " in the first {searched} of {} files with hits",
                files.len()
            );
        }
        out.push('\n');
        // Hit lines already shown, marked, inside a definition printed up here.
        let mut above = HashMap::<usize, Vec<(u32, u32)>>::new();
        for candidate in candidates
            .iter()
            .filter(|candidate| candidate.wanted)
            .take(limit)
        {
            let file = &files[candidate.file];
            let _ = writeln!(
                out,
                "  {}:{} {} {}",
                file.path,
                candidate.line,
                kind_label(candidate.kind),
                candidate.qualified
            );
            if reported <= DEFINITION_BODIES
            && let Some(bodies) = bodies.as_deref_mut()
            && let Some(definitions) = index.serve(file).1
            && let Some(definition) = definitions
                .iter()
                .find(|definition| definition.id == candidate.id)
            // A constructor inside a class printed whole is already shown.
            && !above.get(&candidate.file).is_some_and(|shown| {
                shown
                    .iter()
                    .any(|(first, last)| *first <= definition.start && definition.end <= *last)
            }) {
                let hits = file
                    .hit_lines
                    .iter()
                    .filter(|line| !omitted(candidate.file, **line))
                    .map(|line| (*line, note(candidate.file, *line)))
                    .collect::<Vec<_>>();
                above
                    .entry(candidate.file)
                    .or_default()
                    .extend(definition_body(
                        &mut out,
                        bodies,
                        file,
                        &definitions,
                        (definition, candidate.line),
                        &hits,
                    ));
            }
        }
        if reported > limit {
            let _ = writeln!(out, "  ... {} more definitions", reported - limit);
        }
        // What each definition uses is printed after the references. Both spend
        // what the definitions left of the budget: the references their share of
        // it, and more when the used declarations do not want all of the rest.
        let mut used = Vec::new();
        let mut reserved = 0;
        if reported <= DEFINITION_BODIES
            && let Some(bodies) = bodies.as_deref_mut()
        {
            for candidate in candidates
                .iter()
                .filter(|candidate| candidate.wanted)
                .take(limit)
            {
                if let Some(owner) = index.locate(root, candidate.id) {
                    let targets = used_by(index, root, &owner, USED_DECLARATIONS, |target| {
                        bodies.shows(target)
                    });
                    used.push((candidate.qualified.as_str(), targets));
                }
            }
            let wanted = used
                .iter()
                .flat_map(|(_, targets)| targets)
                .filter(|target| target.last - target.first < WHOLE_USED_LINES)
                .map(|target| (target.last - target.first) as usize + 2)
                .sum::<usize>();
            reserved = wanted.min(bodies.budget - bodies.budget * REFERENCE_SHARE / 100);
            bodies.budget -= reserved;
        }
        let summary_start = out.len();
        let _ = writeln!(out, "{references} references in {reference_files} files");
        let summary = summary_start..out.len();

        let hidden = |position: usize, line: u32| {
            omitted(position, line)
                || above.get(&position).is_some_and(|shown| {
                    shown
                        .iter()
                        .any(|(first, last)| (*first..=*last).contains(&line))
                })
        };
        let mut remaining = limit;
        for (file, position, index_only) in &listing {
            let served = if remaining > 0 {
                index.serve(file)
            } else {
                ("", None)
            };
            let mut rows = Vec::new();
            let mut total = index_only.map_or(0, |file| file.kept.len());
            if let Some(position) = *position {
                total += files[position]
                    .hit_lines
                    .iter()
                    .filter(|line| !hidden(position, **line))
                    .count();
                for (line, code) in &files[position].kept {
                    if !hidden(position, *line) {
                        rows.push(Row {
                            line: *line,
                            code,
                            note: note(position, *line),
                        });
                    }
                }
            }
            if let Some(index_only) = index_only {
                rows.extend(index_only.kept.iter().map(|(line, code)| Row {
                    line: *line,
                    code,
                    note: " (via index)".to_owned(),
                }));
                rows.sort_by_key(|row| row.line);
            }
            list_file(
                &mut out,
                &mut remaining,
                file,
                served,
                &rows,
                total - rows.len(),
                bodies.as_deref_mut(),
            );
        }
        if omitted_hits > 0 {
            let _ = writeln!(
                out,
                "{omitted_hits} hit(s) omitted: the pinned index resolved them to other definitions of {last}"
            );
        }
        if let Some(bodies) = bodies {
            bodies.budget += reserved;
            for (owner, targets) in &used {
                list_used(&mut out, bodies, owner, targets);
            }
        }
        if max_bytes != 0 && out.len() > max_bytes {
            let heading = out.find('\n').map_or(0, |end| end + 1);
            let count = out[summary.clone()].to_owned();
            out.replace_range(summary, "");
            out.insert_str(heading, &count);
        }
        finish_bodies(&mut out, body_state);
        out
    })
}

/// A plain variable declared inside a callable is not a definition worth a
/// header line; module- and class-level ones are.
fn is_local_variable(definition: &Definition, all: &[Definition]) -> bool {
    definition.kind == NodeKind::VARIABLE
        && all.iter().any(|scope| {
            is_callable_kind(scope.kind)
                && scope.id != definition.id
                && scope.start <= definition.start
                && definition.end <= scope.end
        })
}

/// `(file id, line)` of every reference the index records to `definition`,
/// from incoming edges of any certainty, and whether the index is certain of
/// that resolution.
///
/// An edge's own line is where its expression starts. When the edge points at
/// a per-site placeholder, the placeholder's occurrence is the name token
/// itself, which is the line a reader looks for in a call split over lines.
fn reference_sites(store: &Store, definition: NodeId) -> BTreeMap<(i64, u32), bool> {
    let Ok(mut edges) = store.get_incoming_edges_for_node_id(
        definition,
        &[],
        TrailCallerScope::IncludeTestsAndBenches,
        true,
    ) else {
        return BTreeMap::new();
    };
    edges.retain(|edge| edge.kind != EdgeKind::MEMBER);
    let placeholders = edges
        .iter()
        .map(|edge| edge.target)
        .filter(|target| *target != definition)
        .collect::<Vec<_>>();
    let tokens = store
        .get_occurrences_for_node_ids(&placeholders)
        .unwrap_or_default();
    let mut sites = BTreeMap::new();
    for edge in &edges {
        let certain = edge
            .certainty
            .or_else(|| ResolutionCertainty::from_confidence(edge.confidence))
            == Some(ResolutionCertainty::Certain);
        let mut named = tokens
            .get(&edge.target)
            .into_iter()
            .flatten()
            .filter(|occurrence| {
                matches!(
                    occurrence.kind,
                    OccurrenceKind::REFERENCE | OccurrenceKind::MACRO_REFERENCE
                ) && edge
                    .file_node_id
                    .is_none_or(|file| file == occurrence.location.file_node_id)
            })
            .map(|occurrence| {
                (
                    occurrence.location.file_node_id.0,
                    occurrence.location.start_line,
                )
            })
            .collect::<Vec<_>>();
        if let Some(line) = edge.line {
            // A placeholder shared by several sites: keep this edge's token.
            named = named
                .into_iter()
                .filter(|(_, token_line)| *token_line >= line)
                .min_by_key(|(_, token_line)| *token_line)
                .into_iter()
                .collect();
            if named.is_empty()
                && let Some(file) = edge.file_node_id
            {
                named.push((file.0, line));
            }
        }
        for site in named {
            *sites.entry(site).or_default() |= certain;
        }
    }
    sites
}

/// Live text of index-recorded lines in one file, provided the file is inside
/// the project and its current bytes are the ones the index describes.
fn index_only_file(
    root: &Path,
    index: &mut Index<'_>,
    file_id: i64,
    lines: &BTreeSet<u32>,
) -> Option<LiveFile> {
    let verified = index.open(root, file_id)?;
    let text = verified
        .text
        .as_bytes()
        .split(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    let kept = lines
        .iter()
        .filter_map(|line| {
            Some((
                *line,
                code_cell(text.get((*line as usize).checked_sub(1)?)?),
            ))
        })
        .filter(|(_, code)| !code.is_empty())
        .collect::<Vec<_>>();
    (!kept.is_empty()).then(|| LiveFile {
        path: verified.file.path.clone(),
        absolute: verified.file.absolute.clone(),
        sha256: verified.file.sha256.clone(),
        hit_lines: kept.iter().map(|(line, _)| *line).collect(),
        kept,
    })
}

// ---------------------------------------------------------------------------
// read
// ---------------------------------------------------------------------------

enum Target<'a> {
    Symbol(&'a str),
    Line(u32),
    Range(u32, u32),
    File,
}

/// A selector with no `:` is a path.
fn parse_selector(selector: &str) -> Option<(&str, Target<'_>)> {
    if let Some((path, symbol)) = selector.split_once("::") {
        return (!path.is_empty() && !symbol.is_empty()).then_some((path, Target::Symbol(symbol)));
    }
    let Some((path, lines)) = selector.rsplit_once(':') else {
        return (!selector.is_empty()).then_some((selector, Target::File));
    };
    if path.is_empty() {
        return None;
    }
    if let Ok(line) = lines.parse() {
        return Some((path, Target::Line(line)));
    }
    let (first, last) = lines.split_once('-')?;
    Some((path, Target::Range(first.parse().ok()?, last.parse().ok()?)))
}

/// The live lines of one file being read.
struct Page<'a> {
    path: &'a str,
    lines: &'a [&'a str],
    /// The lines the reply has printed so far.
    printed: &'a mut Printed,
    max_bytes: usize,
}

impl Page<'_> {
    /// Print `first..=last` under a header, up to `budget` lines, and name the
    /// range that continues when the budget cuts it.
    fn block(
        &mut self,
        out: &mut String,
        budget: &mut usize,
        first: u32,
        last: u32,
        title: &str,
        note: &str,
    ) {
        let path = self.path;
        let _ = writeln!(out, "== {path}:{first}-{last}{title}{note}");
        let planned = ((last + 1 - first) as usize).min(*budget);
        let (first, last) = (first as usize, last as usize);
        let mut shown = 0;
        for (offset, code) in self.lines[first - 1..][..planned].iter().enumerate() {
            if self.max_bytes != 0 && out.len() + code.len() + 16 > self.max_bytes {
                out.push_str("... source omitted (--max-bytes)\n");
                break;
            }
            let _ = write!(out, "{}:", first + offset);
            if !code.is_empty() {
                let _ = write!(out, " {}", printable(code));
            }
            out.push('\n');
            shown += 1;
        }
        if shown > 0 {
            self.printed
                .add(path, first as u32, (first + shown - 1) as u32);
        }
        *budget -= shown;
        let next = first + shown;
        if next <= last {
            let _ = writeln!(
                out,
                "... {} more lines; read {path}:{next}-{last}",
                last + 1 - next
            );
        }
    }

    /// Print a definition, starting near the requested line if the budget would omit it.
    fn declaration(
        &mut self,
        out: &mut String,
        budget: &mut usize,
        definition: &Definition,
        requested_line: Option<u32>,
    ) {
        if let Some((first, last, note)) = declared_range(self.lines, definition) {
            let first = match requested_line {
                Some(line) if (line - first) as usize >= *budget => {
                    line.saturating_sub(DECLARATION_WINDOW).max(first)
                }
                _ => first,
            };
            let title = format!(" {}", definition.qualified);
            self.block(out, budget, first, last, &title, note);
        }
    }

    /// Where the declarations of a long file are: one row per declaration in
    /// source order, nested ones indented. Rows come from the index when it
    /// describes these bytes, else from the lines that start a declaration
    /// and open a block. Either way a local binding is not a row. A file with
    /// nothing to outline is read from the top.
    fn outline(
        &mut self,
        out: &mut String,
        budget: &mut usize,
        definitions: Option<&[Definition]>,
    ) {
        let (path, lines) = (self.path, self.lines);
        let mut rows = match definitions {
            Some(definitions) => definitions
                .iter()
                .filter(|definition| !is_local_variable(definition, definitions))
                .filter_map(|definition| {
                    let (first, last, _) = declared_range(lines, definition)?;
                    let label = format!("{} {}", kind_label(definition.kind), definition.qualified);
                    Some((first, last, label))
                })
                .collect::<Vec<_>>(),
            None => (0..lines.len())
                .filter(|at| starts_declaration(lines[*at]) && opens_block(lines[*at]))
                .filter(|at| !lines[*at].trim_start().starts_with("let "))
                .map(|at| (at, block_end(lines, at)))
                .filter(|(at, end)| end > at)
                .map(|(at, end)| (at as u32 + 1, end as u32 + 1, bounded(lines[at].trim())))
                .collect(),
        };
        if rows.is_empty() {
            return self.block(out, budget, 1, lines.len() as u32, "", "");
        }
        rows.sort_by_key(|(first, last, _)| (*first, Reverse(*last)));
        let note = if definitions.is_some() {
            ""
        } else {
            APPROXIMATE
        };
        let _ = writeln!(out, "== {path} ({} lines; outline){note}", lines.len());
        // Last lines of the declarations that hold the row being printed.
        let mut open = Vec::new();
        for (first, last, label) in rows.iter().take(*budget) {
            while open.last().is_some_and(|end| end < first) {
                open.pop();
            }
            let _ = writeln!(out, "{}{first}-{last} {label}", "  ".repeat(open.len()));
            open.push(*last);
        }
        if rows.len() > *budget {
            let _ = writeln!(out, "... {} more declarations", rows.len() - *budget);
        }
        let _ = writeln!(out, "read {path}::<Symbol> or {path}:A-B for bodies");
    }
}

/// Under a declaration just read: where the declarations it uses are, one
/// row each, leaving out what the reply has already printed.
fn list_used_pointers(
    out: &mut String,
    root: &Path,
    index: Option<&mut Index<'_>>,
    printed: &Printed,
    definition: &Definition,
) {
    let Some(index) = index else {
        return;
    };
    let Some(owner) = index.locate(root, definition.id) else {
        return;
    };
    let used = used_by(index, root, &owner, USED_POINTERS, |target| {
        printed.holds(target)
    });
    if !used.is_empty() {
        let _ = writeln!(out, "{PINNED_USES}");
    }
    for target in &used {
        out.push_str(&target.pointer());
    }
}

fn approximate_line_window(line: u32, count: u32) -> (u32, u32) {
    (line.saturating_sub(20).max(1), (line + 40).min(count))
}

fn read_selector(
    out: &mut String,
    root: &Path,
    mut index: Option<&mut Index<'_>>,
    printed: &mut Printed,
    selector: &str,
    max_bytes: usize,
) {
    let Some((path, target)) = parse_selector(selector) else {
        let _ = writeln!(
            out,
            "== {selector}: expected path, path::Symbol, path:LINE or path:A-B"
        );
        return;
    };
    let absolute = match codestory_workspace::resolve_project_relative_path(root, Path::new(path)) {
        Ok(ProjectRelativePathResolution::Existing { absolute, .. }) => absolute,
        Ok(ProjectRelativePathResolution::Missing { absolute, .. }) => {
            if let Some(index) = index {
                index.observe_missing_path(&absolute);
            }
            let _ = writeln!(out, "== {selector}: no file {path}");
            return;
        }
        Ok(ProjectRelativePathResolution::OutOfProject) => {
            let _ = writeln!(out, "== {selector}: {path} is outside the project");
            return;
        }
        _ => {
            let _ = writeln!(out, "== {selector}: no file {path}");
            return;
        }
    };
    let Some((file, bytes)) = open_live(root, absolute) else {
        let _ = writeln!(
            out,
            "== {selector}: {path} is unreadable, binary or larger than 4 MB"
        );
        return;
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines = text.lines().collect::<Vec<_>>();
    let count = lines.len() as u32;
    let mut page = Page {
        path: &file.path,
        lines: &lines,
        printed,
        max_bytes,
    };
    let definitions = index.as_deref_mut().and_then(|index| index.serve(&file).1);
    let mut budget = READ_LINE_CAP;
    let mut approximate_budget = APPROXIMATE_LINE_CAP;
    let outside = |out: &mut String| {
        let _ = writeln!(out, "== {selector}: {} has {count} lines", file.path);
    };
    match target {
        Target::Range(first, last) if first >= 1 && first <= last && first <= count => {
            page.block(out, &mut budget, first, last.min(count), "", "");
        }
        Target::Line(line) if line >= 1 && line <= count => {
            match definitions
                .as_deref()
                .and_then(|definitions| enclosing_scope(definitions, line))
            {
                Some(definition) => {
                    page.declaration(out, &mut budget, definition, Some(line));
                    list_used_pointers(out, root, index, page.printed, definition);
                }
                None => {
                    let (first, last) = approximate_line_window(line, count);
                    page.block(out, &mut approximate_budget, first, last, "", APPROXIMATE);
                }
            }
        }
        Target::File if count > WHOLE_FILE_LINES => {
            page.outline(out, &mut budget, definitions.as_deref().map(Vec::as_slice));
        }
        Target::File if count >= 1 => page.block(out, &mut (count as usize), 1, count, "", ""),
        Target::Range(..) | Target::Line(_) | Target::File => outside(out),
        Target::Symbol(symbol) => {
            let wanted = name_segments(symbol);
            match definitions.as_deref() {
                Some(definitions) => {
                    let mut found = definitions
                        .iter()
                        .filter(|definition| {
                            definition.name == symbol || definition.qualified == symbol
                        })
                        .collect::<Vec<_>>();
                    if found.is_empty() && !wanted.is_empty() {
                        found = definitions
                            .iter()
                            .filter(|definition| definition.answers_to(&wanted))
                            .collect();
                    }
                    if found
                        .iter()
                        .any(|definition| is_scope_kind(definition.kind))
                    {
                        found.retain(|definition| is_scope_kind(definition.kind));
                    }
                    for definition in &found {
                        page.declaration(out, &mut budget, definition, None);
                        list_used_pointers(
                            out,
                            root,
                            index.as_deref_mut(),
                            page.printed,
                            definition,
                        );
                    }
                    if found.is_empty() {
                        let mut known = definitions
                            .iter()
                            .filter(|definition| is_scope_kind(definition.kind))
                            .map(|definition| definition.name.as_str())
                            .collect::<Vec<_>>();
                        known.dedup();
                        let more = known.len().saturating_sub(20);
                        known.truncate(20);
                        let _ = write!(
                            out,
                            "== {selector}: no symbol {symbol} in {}; definitions there: {}",
                            file.path,
                            known.join(", ")
                        );
                        if more > 0 {
                            let _ = write!(out, ", ... {more} more");
                        }
                        out.push('\n');
                    }
                }
                None => match approximate_declaration(&lines, &wanted) {
                    Some((first, last)) => page.block(
                        out,
                        &mut approximate_budget,
                        first,
                        last,
                        &format!(" {symbol}"),
                        APPROXIMATE,
                    ),
                    None => {
                        let _ = writeln!(
                            out,
                            "== {selector}: no symbol {symbol} in {} (not from index; searched the live text)",
                            file.path
                        );
                    }
                },
            }
        }
    }
}

/// Without index facts: the declaration line that mentions the last name
/// segment, through the end of the block it opens.
fn approximate_declaration(lines: &[&str], wanted: &[&str]) -> Option<(u32, u32)> {
    let (last, owners) = wanted.split_last()?;
    let mut from = 0;
    for owner in owners {
        if let Some(at) = declaration_line(lines, owner, from) {
            from = at + 1;
        }
    }
    let at = declaration_line(lines, last, from).or_else(|| declaration_line(lines, last, 0))?;
    Some((at as u32 + 1, block_end(lines, at) as u32 + 1))
}

/// The first line at or after `from` that declares `word`, else the first
/// that mentions it as a whole word.
fn declaration_line(lines: &[&str], word: &str, from: usize) -> Option<usize> {
    let mentions = || {
        lines
            .iter()
            .enumerate()
            .skip(from)
            .filter_map(|(at, line)| Some((at, &line[..word_offset(line, word)?])))
    };
    mentions()
        .find(|(_, before)| {
            // The word right before the name, past a `(receiver)` group.
            let mut before = before.trim_end();
            if before.ends_with(')')
                && let Some(open) = before.rfind('(')
            {
                before = before[..open].trim_end();
            }
            before
                .rsplit(|ch: char| !is_declaration_char(ch))
                .next()
                .is_some_and(is_declaration_word)
        })
        .or_else(|| mentions().next())
        .map(|(at, _)| at)
}

fn is_declaration_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '!'
}

fn is_declaration_word(token: &str) -> bool {
    DECLARATION_WORDS.split(' ').any(|word| word == token)
}

/// Whether a line starts a declaration by the rule `declaration_line` uses:
/// declaration words, each past a `(receiver)` group, lead up to a name.
fn starts_declaration(line: &str) -> bool {
    let mut rest = line.trim_start();
    let mut declares = false;
    loop {
        let end = rest
            .find(|ch| !is_declaration_char(ch))
            .unwrap_or(rest.len());
        if !is_declaration_word(&rest[..end]) {
            return declares && end > 0;
        }
        declares = true;
        rest = rest[end..].trim_start();
        if rest.starts_with('(')
            && let Some(close) = rest.find(')')
        {
            rest = rest[close + 1..].trim_start();
        }
    }
}

fn indent(line: &str) -> usize {
    line.chars()
        .take_while(|ch| ch.is_whitespace())
        .map(|ch| if ch == '\t' { 4 } else { 1 })
        .sum()
}

fn opens_block(line: &str) -> bool {
    line.trim_end().ends_with(['{', '(', '[', ':', ',', '='])
}

/// The last line (0-based) of the block that starts at `start`: everything
/// indented deeper, plus the closing line at the starting indent.
fn block_end(lines: &[&str], start: usize) -> usize {
    let depth = indent(lines[start]);
    let mut end = start;
    for (at, line) in lines.iter().enumerate().skip(start + 1) {
        let code = line.trim();
        if code.is_empty() {
            continue;
        }
        if indent(line) > depth {
            end = at;
            continue;
        }
        let closes = indent(line) == depth && (code.starts_with(['}', ')', ']']) || code == "end");
        if closes {
            end = at;
        }
        // `) -> T {` and `} else {` close one part and open the next.
        if !(closes && opens_block(code)) {
            break;
        }
    }
    end
}
