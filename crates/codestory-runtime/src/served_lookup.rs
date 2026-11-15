//! Lookups that verify only what they serve.
//!
//! `find`, `refs` and `read` start from live source text: one parallel pass
//! over the files ripgrep would search, then work proportional to the hits.
//! Every printed line comes from bytes read during the call. Index facts
//! (enclosing definitions, declaration ranges, resolved references) decorate a
//! file only when those same bytes hash to the `content_hash` recorded for it
//! in the pinned complete core publication. A file that changed since
//! indexing, or was never indexed, is still served as plain text and marked.
//!
//! `find` and `refs` show each hit inside its enclosing declaration, or inside
//! a few lines of context where the index has none to offer, so the reply
//! carries the code a reader would open next. A line budget bounds that; hits
//! past it stay one line each.
//!
//! Nothing here refreshes or activates an index, checks whole-tree freshness,
//! or writes: the store is opened observationally and a missing or unreadable
//! publication degrades the reply to text only.

use codestory_contracts::api::ApiError;
use codestory_contracts::graph::{
    EdgeKind, NodeId, NodeKind, OccurrenceKind, ResolutionCertainty, TrailCallerScope,
};
use codestory_store::{StorageError, Store};
use codestory_workspace::ProjectRelativePathResolution;
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, PoisonError};

/// Hit lines one `find` or `refs` reply lists before it switches to counts.
pub const DEFAULT_LIMIT: usize = 150;
/// Lines one `find` or `refs` reply spends on declaration bodies and context,
/// counting their `==` headers and `...` gap marks.
pub const DEFAULT_BODY_LINES: usize = 220;

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const BINARY_PROBE_BYTES: usize = 1024;
const MAX_CODE_CHARS: usize = 200;
const READ_LINE_CAP: usize = 300;
const APPROXIMATE_LINE_CAP: usize = 80;
/// A declaration up to this long is shown whole; a longer one is windowed.
const WHOLE_DECLARATION_LINES: u32 = 60;
/// Lines shown on each side of a hit inside a windowed declaration.
const DECLARATION_WINDOW: u32 = 6;
/// Lines shown on each side of a hit that has no verified declaration.
const TEXT_WINDOW: u32 = 3;
/// `refs` leads with the declarations of the name when there are this few.
const DEFINITION_BODIES: usize = 3;
/// `read <path>` prints a file up to this long whole, and outlines a longer one.
const WHOLE_FILE_LINES: u32 = 250;
/// Hit files past the listed ones that `refs` still checks for definitions,
/// so a small `--limit` does not hide where the name is defined.
const DEFINITION_SEARCH_FILES: usize = 1_000;
const NO_INDEX: &str = "no index; text matches only";
const APPROXIMATE: &str = " (not from index; approximate)";
/// Words that introduce a declaration, for the text-only range fallback.
const DECLARATION_WORDS: &str = "abstract async class const def define enum export extern final fn \
    fun func function impl inline interface let macro_rules! mod module namespace object override \
    package private proc protected pub public record static struct sub trait type typedef union \
    val var virtual void";

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
        return Ok(out);
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
        let mut bodies = body_lines.map(Bodies::new);
        list_hits(&mut out, &files, limit, index.as_mut(), bodies.as_mut());
        finish_bodies(&mut out, bodies);
        out
    }))
}

/// Every whole-word mention of `name` (or of the last segment of
/// `Qualifier::Name` / `Qualifier.Name`), with the definitions the index
/// records for it in files whose live bytes it still describes. `body_lines`
/// is the budget for declaration bodies and context; `None` lists one line
/// per hit.
pub fn refs(
    root: &Path,
    storage_path: &Path,
    name: &str,
    limit: usize,
    body_lines: Option<usize>,
) -> Result<String, ApiError> {
    let wanted = name_segments(name);
    let Some(last) = wanted
        .last()
        .copied()
        .filter(|last| last.bytes().all(is_word_byte))
    else {
        return Err(ApiError::invalid_argument(
            "refs needs an identifier, Qualifier::Name or Qualifier.Name",
        ));
    };
    let files = scan_live_tree(root, &[], &[last.as_bytes()], true, limit)?;
    Ok(with_pinned_index(storage_path, |store| {
        let mut bodies = body_lines.map(Bodies::new);
        let mut out = match store {
            Some(store) => {
                refs_with_index(root, store, name, &wanted, &files, limit, bodies.as_mut())
            }
            None => {
                let total = files.iter().map(|file| file.hit_lines.len()).sum::<usize>();
                let mut out = format!("{NO_INDEX}\n{total} references in {} files\n", files.len());
                list_hits(&mut out, &files, limit, None, bodies.as_mut());
                out
            }
        };
        finish_bodies(&mut out, bodies);
        out
    }))
}

/// Complete declarations for a batch of selectors: `path::Symbol`,
/// `path:LINE` or `path:A-B`. A bare `path` is the whole file when it is
/// short, else an outline of its declarations. A selector that cannot be
/// served gets its own `==` line and never fails the batch.
pub fn read(root: &Path, storage_path: &Path, selectors: &[String]) -> Result<String, ApiError> {
    if selectors.is_empty() {
        return Err(ApiError::invalid_argument(
            "read needs at least one selector",
        ));
    }
    Ok(with_pinned_index(storage_path, |store| {
        let mut index = store.map(Index::new);
        let mut out = String::new();
        for selector in selectors {
            read_selector(&mut out, root, index.as_mut(), selector);
        }
        out
    }))
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

/// The files ripgrep searches by default: ignore files honoured, hidden files
/// skipped, symlinks not followed. Globs select like `rg -g`.
fn live_walk(root: &Path, file_globs: &[String]) -> Result<ignore::WalkParallel, ApiError> {
    let mut walk = ignore::WalkBuilder::new(root);
    walk.add_custom_ignore_filename(".rgignore");
    if !file_globs.is_empty() {
        let mut globs = ignore::overrides::OverrideBuilder::new(root);
        for glob in file_globs {
            globs
                .add(glob)
                .map_err(|error| ApiError::invalid_argument(error.to_string()))?;
        }
        walk.overrides(
            globs
                .build()
                .map_err(|error| ApiError::invalid_argument(error.to_string()))?,
        );
    }
    Ok(walk.build_parallel())
}

fn live_paths(root: &Path, file_globs: &[String]) -> Result<Vec<String>, ApiError> {
    let found = Mutex::new(Vec::new());
    let sink = &found;
    live_walk(root, file_globs)?.run(|| {
        Box::new(move |entry| {
            if let Ok(entry) = entry
                && entry.file_type().is_some_and(|kind| kind.is_file())
            {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(project_path(root, entry.path()));
            }
            ignore::WalkState::Continue
        })
    });
    let mut paths = found.into_inner().unwrap_or_else(PoisonError::into_inner);
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
    let found = Mutex::new(Vec::new());
    let sink = &found;
    live_walk(root, file_globs)?.run(|| {
        let mut bytes = Vec::new();
        Box::new(move |entry| {
            if let Ok(entry) = entry
                && entry.file_type().is_some_and(|kind| kind.is_file())
                && read_live_text(entry.path(), &mut bytes)
                && let Some(file) =
                    match_live_file(root, entry.path(), &bytes, needles, whole_word, keep)
            {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(file);
            }
            ignore::WalkState::Continue
        })
    });
    let mut files = found.into_inner().unwrap_or_else(PoisonError::into_inner);
    files.sort_unstable_by(|left: &LiveFile, right| left.path.cmp(&right.path));
    Ok(files)
}

/// Read a searchable text file into `bytes`. Files larger than 4 MB or with a
/// NUL in their first kilobyte are not text this lookup serves.
fn read_live_text(path: &Path, bytes: &mut Vec<u8>) -> bool {
    bytes.clear();
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    if !file
        .metadata()
        .is_ok_and(|metadata| metadata.len() <= MAX_FILE_BYTES)
        || file.take(MAX_FILE_BYTES + 1).read_to_end(bytes).is_err()
    {
        return false;
    }
    bytes.len() as u64 <= MAX_FILE_BYTES
        && !bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0)
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

/// Index facts, handed out per file and only for bytes the index describes.
struct Index<'a> {
    store: &'a Store,
    /// What was served for a path, and the hash of the bytes it was served for.
    served: HashMap<PathBuf, (String, Served)>,
}

impl<'a> Index<'a> {
    fn new(store: &'a Store) -> Self {
        Self {
            store,
            served: HashMap::new(),
        }
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
                Ok(Some(hash)) if hash == file.sha256 => match definitions_in(self.store, row.id) {
                    Ok(definitions) => ("", Some(Rc::new(definitions))),
                    Err(_) => (" (index unreadable)", None),
                },
                Ok(_) => (" (changed since indexing)", None),
                Err(_) => (" (index unreadable)", None),
            },
            Err(_) => (" (index unreadable)", None),
        };
        self.served
            .insert(file.absolute.clone(), (file.sha256.clone(), served.clone()));
        served
    }
}

fn definitions_in(store: &Store, file_id: i64) -> Result<Vec<Definition>, StorageError> {
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
            let here = node.file_node_id == Some(NodeId(file_id));
            (named && here && start >= 1 && end >= start).then(|| Definition {
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

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/// What one reply may still spend on declaration bodies and context.
struct Bodies {
    /// Lines left, counting `==` headers and `...` gap marks.
    budget: usize,
    /// Declarations already printed, so that none is printed twice.
    declared: HashSet<NodeId>,
    /// Whether a hit stayed a one-line row because its body did not fit.
    overflowed: bool,
}

impl Bodies {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            declared: HashSet::new(),
            overflowed: false,
        }
    }
}

/// Say how to read the bodies that did not fit.
fn finish_bodies(out: &mut String, bodies: Option<Bodies>) {
    if bodies.is_some_and(|bodies| bodies.overflowed) {
        out.push_str("more: read <path>::<Symbol> or <path>:A-B\n");
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

/// Print `block` window by window while the budget lasts and return the last
/// line printed, 0 when nothing fit. `tail` is the body line printed just
/// above under the same file header, if any. Hit lines are the keys of
/// `notes` and end in `>`; every other line ends in `:`.
fn print_body(
    out: &mut String,
    bodies: &mut Bodies,
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
        let end = print_body(out, bodies, &lines, &block, &notes, tail);
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
    let end = print_body(out, bodies, &lines, &block, &notes, 0);
    let mut printed = block.windows;
    printed.retain(|window| window.1 <= end);
    printed
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

fn refs_with_index(
    root: &Path,
    store: &Store,
    name: &str,
    wanted: &[&str],
    files: &[LiveFile],
    limit: usize,
    mut bodies: Option<&mut Bodies>,
) -> String {
    let last = wanted[wanted.len() - 1];
    let mut index = Index::new(store);

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
                None if candidate.wanted => {
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
                format!(" -> {}:{}", files[target.file].path, target.line)
            }
            _ => String::new(),
        }
    };

    // Reference lines the index records in verified files that the text
    // search did not find, read live like every other line.
    let index_only = index_only
        .into_iter()
        .filter_map(|(file_id, lines)| index_only_file(root, store, file_id, &lines))
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
            })
        {
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
    let _ = writeln!(out, "{references} references in {reference_files} files");

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
            "{omitted_hits} hit(s) omitted: the index resolved them to other definitions of {last}"
        );
    }
    out
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
    store: &Store,
    file_id: i64,
    lines: &BTreeSet<u32>,
) -> Option<LiveFile> {
    let absolute = store.get_file_by_id(file_id).ok()??.path;
    absolute.strip_prefix(root).ok()?;
    let mut bytes = Vec::new();
    if !read_live_text(&absolute, &mut bytes) {
        return None;
    }
    let sha256 = sha256_hex(&bytes);
    if store.get_file_content_hash(file_id).ok()?? != sha256 {
        return None;
    }
    let text = bytes.split(|byte| *byte == b'\n').collect::<Vec<_>>();
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
        path: project_path(root, &absolute),
        absolute,
        sha256,
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
}

impl Page<'_> {
    /// Print `first..=last` under a header, up to `budget` lines, and name the
    /// range that continues when the budget cuts it.
    fn block(
        &self,
        out: &mut String,
        budget: &mut usize,
        first: u32,
        last: u32,
        title: &str,
        note: &str,
    ) {
        let path = self.path;
        let _ = writeln!(out, "== {path}:{first}-{last}{title}{note}");
        let (first, last) = (first as usize, last as usize);
        let shown = (last + 1 - first).min(*budget);
        *budget -= shown;
        for (offset, code) in self.lines[first - 1..][..shown].iter().enumerate() {
            let _ = write!(out, "{}:", first + offset);
            if !code.is_empty() {
                let _ = write!(out, " {}", printable(code));
            }
            out.push('\n');
        }
        let next = first + shown;
        if next <= last {
            let _ = writeln!(
                out,
                "... {} more lines; read {path}:{next}-{last}",
                last + 1 - next
            );
        }
    }

    /// Print a definition over the lines it spans.
    fn declaration(&self, out: &mut String, budget: &mut usize, definition: &Definition) {
        if let Some((first, last, note)) = declared_range(self.lines, definition) {
            let title = format!(" {}", definition.qualified);
            self.block(out, budget, first, last, &title, note);
        }
    }

    /// Where the declarations of a long file are: one row per declaration in
    /// source order, nested ones indented. Rows come from the index when it
    /// describes these bytes, else from the lines that start a declaration
    /// and open a block. Either way a local binding is not a row. A file with
    /// nothing to outline is read from the top.
    fn outline(&self, out: &mut String, budget: &mut usize, definitions: Option<&[Definition]>) {
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

fn read_selector(out: &mut String, root: &Path, index: Option<&mut Index<'_>>, selector: &str) {
    let Some((path, target)) = parse_selector(selector) else {
        let _ = writeln!(
            out,
            "== {selector}: expected path, path::Symbol, path:LINE or path:A-B"
        );
        return;
    };
    let absolute = match codestory_workspace::resolve_project_relative_path(root, Path::new(path)) {
        Ok(ProjectRelativePathResolution::Existing { absolute, .. }) => absolute,
        Ok(ProjectRelativePathResolution::OutOfProject) => {
            let _ = writeln!(out, "== {selector}: {path} is outside the project");
            return;
        }
        _ => {
            let _ = writeln!(out, "== {selector}: no file {path}");
            return;
        }
    };
    let mut bytes = Vec::new();
    if !read_live_text(&absolute, &mut bytes) {
        let _ = writeln!(
            out,
            "== {selector}: {path} is unreadable, binary or larger than 4 MB"
        );
        return;
    }
    let file = LiveFile {
        path: project_path(root, &absolute),
        absolute,
        sha256: sha256_hex(&bytes),
        hit_lines: Vec::new(),
        kept: Vec::new(),
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines = text.lines().collect::<Vec<_>>();
    let count = lines.len() as u32;
    let page = Page {
        path: &file.path,
        lines: &lines,
    };
    let definitions = index.and_then(|index| index.serve(&file).1);
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
                Some(definition) => page.declaration(out, &mut budget, definition),
                None => page.block(
                    out,
                    &mut approximate_budget,
                    line.saturating_sub(20).max(1),
                    (line + 40).min(count),
                    "",
                    APPROXIMATE,
                ),
            }
        }
        Target::File if count > WHOLE_FILE_LINES => {
            page.outline(out, &mut budget, definitions.as_deref().map(Vec::as_slice));
        }
        Target::File if count >= 1 => page.block(out, &mut budget, 1, count, "", ""),
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
                        page.declaration(out, &mut budget, definition);
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
