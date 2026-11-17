//! Lookups that verify only what they serve.
//!
//! `find`, `refs` and `read` start from live source text. Discovery honors
//! ignore rules and text matching runs in parallel.
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

/// Hit lines one `find` or `refs` reply lists before it switches to counts.
pub const DEFAULT_LIMIT: usize = 150;
/// Lines one `find` or `refs` reply spends on declaration bodies and context,
/// counting their `==` headers and `...` gap marks.
pub const DEFAULT_BODY_LINES: usize = 280;
/// Default byte ceiling for multi-file `find`, `refs`, and `slice` replies so
/// agent shell/MCP hosts never truncate mid-declaration.
pub const DEFAULT_MAX_BYTES: usize = 7_500;
/// Default byte ceiling for `read` replies so multi-selector reads and dense
/// files fit cleanly inside an agent turn unless `--max-bytes 0` is given.
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
    find_with_budget(
        root,
        storage_path,
        terms,
        file_globs,
        limit,
        body_lines,
        None,
    )
}

/// Like [`find`], with an optional byte cap (`Some(0)` disables byte capping).
pub fn find_with_budget(
    root: &Path,
    storage_path: &Path,
    terms: &[String],
    file_globs: &[String],
    limit: usize,
    body_lines: Option<usize>,
    max_bytes: Option<usize>,
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
    let ranked_query = RankedQuery::from_terms(terms);
    let scan = scan_live_tree_ranked(root, file_globs, &ranked_query, limit)?;
    let files = scan.files;
    let effective_max_bytes = resolve_max_bytes(max_bytes, body_lines.is_some() && files.len() > 3);
    Ok(with_pinned_index(storage_path, |store| {
        let mut index = store.map(Index::new);
        let mut out = String::new();
        if index.is_none() {
            let _ = writeln!(out, "{NO_INDEX}");
        }
        let total = files.iter().map(|file| file.hit_lines.len()).sum::<usize>();
        let _ = writeln!(out, "{total} matches in {} files", files.len());
        let mut bodies = body_lines.map(|b| Bodies::with_byte_cap(b, effective_max_bytes));
        let term_weights = (files.len() > 3 || ranked_query.terms.len() > 1)
            .then_some(scan.weighted_terms.as_slice());
        list_hits_ranked(
            &mut out,
            &files,
            limit,
            index.as_mut(),
            bodies.as_mut(),
            term_weights,
        );
        if files.len() > 3 && body_lines.is_some() {
            append_related_tests_footer(
                &mut out,
                root,
                &files,
                &scan.all_paths,
                &ranked_query.raw_terms,
                effective_max_bytes,
            );
        }
        finish_bodies(&mut out, bodies);
        out
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
    refs_with_budget_and_globs(root, storage_path, name, &[], limit, body_lines, None)
}

/// Like [`refs`], with an optional byte cap (`Some(0)` disables byte capping).
pub fn refs_with_budget(
    root: &Path,
    storage_path: &Path,
    name: &str,
    limit: usize,
    body_lines: Option<usize>,
    max_bytes: Option<usize>,
) -> Result<String, ApiError> {
    refs_with_budget_and_globs(root, storage_path, name, &[], limit, body_lines, max_bytes)
}

/// Like [`refs_with_budget`], restricted to paths matching `file_globs` when non-empty.
pub fn refs_with_budget_and_globs(
    root: &Path,
    storage_path: &Path,
    name: &str,
    file_globs: &[String],
    limit: usize,
    body_lines: Option<usize>,
    max_bytes: Option<usize>,
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
    let (files, all_paths) =
        scan_live_tree_with_paths(root, file_globs, &[last.as_bytes()], true, limit)?;
    let effective_max_bytes = resolve_max_bytes(max_bytes, body_lines.is_some() && files.len() > 3);
    Ok(with_pinned_index(storage_path, |store| {
        let mut bodies = body_lines.map(|b| Bodies::with_byte_cap(b, effective_max_bytes));
        let mut out = match store {
            Some(store) => refs_with_index(
                root,
                store,
                name,
                &wanted,
                &files,
                &all_paths,
                limit,
                bodies.as_mut(),
            ),
            None => {
                let total = files.iter().map(|file| file.hit_lines.len()).sum::<usize>();
                let mut out = format!("{NO_INDEX}\n{total} references in {} files\n", files.len());
                let mut ranked_files = files;
                if ranked_files.len() > 3 && bodies.is_some() {
                    ranked_files.sort_unstable_by(|left, right| {
                        (
                            path_rank_tier(&left.path),
                            Reverse(left.defines_query_symbol),
                            Reverse(left.hit_lines.len().min(8)),
                            &left.path,
                        )
                            .cmp(&(
                                path_rank_tier(&right.path),
                                Reverse(right.defines_query_symbol),
                                Reverse(right.hit_lines.len().min(8)),
                                &right.path,
                            ))
                    });
                }
                list_hits(&mut out, &ranked_files, limit, None, bodies.as_mut());
                if ranked_files.len() > 3 && bodies.is_some() {
                    append_related_tests_footer(
                        &mut out,
                        root,
                        &ranked_files,
                        &all_paths,
                        &[last.to_string()],
                        effective_max_bytes,
                    );
                }
                out
            }
        };
        finish_bodies(&mut out, bodies);
        out
    }))
}

fn resolve_max_bytes(explicit: Option<usize>, default_apply: bool) -> Option<usize> {
    match explicit {
        Some(0) => None,
        Some(n) => Some(n),
        None if default_apply => Some(DEFAULT_MAX_BYTES),
        None => None,
    }
}

/// Complete declarations for a batch of selectors: `path::Symbol`,
/// `path:LINE` or `path:A-B`. A bare `path` is the whole file when it is
/// short, else an outline of its declarations. A declaration read by symbol
/// or line is followed by `uses:` rows that say where the declarations it
/// uses are. A selector that cannot be served gets its own `==` line and
/// never fails the batch.
pub fn read(root: &Path, storage_path: &Path, selectors: &[String]) -> Result<String, ApiError> {
    read_with_budget(root, storage_path, selectors, None)
}

/// Read files, outlines, symbols or line ranges within an optional output byte cap.
pub fn read_with_budget(
    root: &Path,
    storage_path: &Path,
    selectors: &[String],
    max_bytes: Option<usize>,
) -> Result<String, ApiError> {
    if selectors.is_empty() {
        return Err(ApiError::invalid_argument(
            "read needs at least one selector",
        ));
    }
    let byte_cap = max_bytes.filter(|&cap| cap > 0);
    Ok(with_pinned_index(storage_path, |store| {
        let mut index = store.map(Index::new);
        let mut out = String::new();
        let mut printed = Printed::default();
        for (idx, selector) in selectors.iter().enumerate() {
            if idx > 0 && byte_cap.is_some_and(|cap| out.len() >= cap.saturating_sub(240)) {
                let remaining = &selectors[idx..];
                let preview = remaining
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                let suffix = if remaining.len() > 4 { " ..." } else { "" };
                let _ = writeln!(
                    out,
                    "... {} selector(s) omitted (--max-bytes {} reached); read {preview}{suffix}",
                    remaining.len(),
                    byte_cap.unwrap_or(0)
                );
                break;
            }
            read_selector(
                &mut out,
                root,
                index.as_mut(),
                &mut printed,
                selector,
                byte_cap,
            );
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
    /// Text of the matching lines, ordered by line number ascending.
    kept: Vec<(u32, String)>,
    /// Distinct query terms matched in this file.
    matched_terms: usize,
    /// Whether any matched line starts a declaration.
    defines_query_symbol: bool,
    /// IDF-weighted BM25F score (populated by ranked live scans).
    rank_score: f64,
    /// Per-term hit counts in `terms` order (for IDF aggregation).
    term_hits: Vec<u32>,
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

#[derive(Clone)]
struct QueryTermSpec {
    text: String,
    bytes: Vec<u8>,
    whole_word: bool,
    is_path_hint: bool,
    boost: f64,
}

#[derive(Clone)]
struct WeightedTerm {
    text: String,
    idf: f64,
    boost: f64,
}

struct RankedQuery {
    raw_terms: Vec<String>,
    terms: Vec<QueryTermSpec>,
}

impl RankedQuery {
    fn from_terms(raw: &[String]) -> Self {
        let mut specs = Vec::new();
        let mut seen = HashSet::new();
        for term in raw {
            let is_path_hint = term.contains('/') && !term.starts_with("--");
            if seen.insert(term.clone()) {
                specs.push(QueryTermSpec {
                    text: term.clone(),
                    bytes: term.as_bytes().to_vec(),
                    whole_word: false,
                    is_path_hint,
                    boost: if term.len() >= 6 { 1.4 } else { 1.0 },
                });
            }
            // Split qualified names (`Type.Method` / `Mod::Func`) into linked sub-tokens
            // when they are not filenames, so Go/Rust/Python methods match declarations.
            if !is_path_hint && (term.contains("::") || term.contains('.')) {
                let looks_like_file = matches!(
                    term.rsplit('.').next(),
                    Some(
                        "py" | "go"
                            | "rs"
                            | "ts"
                            | "tsx"
                            | "js"
                            | "jsx"
                            | "json"
                            | "jsonc"
                            | "yaml"
                            | "yml"
                            | "toml"
                            | "md"
                            | "txt"
                    )
                );
                if !looks_like_file {
                    for seg in name_segments(term) {
                        if seg.len() >= 3 && seen.insert(seg.to_string()) {
                            specs.push(QueryTermSpec {
                                text: seg.to_string(),
                                bytes: seg.as_bytes().to_vec(),
                                whole_word: true,
                                is_path_hint: false,
                                boost: 1.5,
                            });
                        }
                    }
                }
            }
        }
        Self {
            raw_terms: raw.to_vec(),
            terms: specs,
        }
    }
}

struct RankedScan {
    files: Vec<LiveFile>,
    all_paths: Vec<String>,
    weighted_terms: Vec<WeightedTerm>,
}

/// Honor ignore rules and omit hidden files and symlink entries.
fn live_walk(
    root: &Path,
    file_globs: &[String],
) -> Result<impl Iterator<Item = PathBuf>, ApiError> {
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
    Ok(walk.build().filter_map(|entry| {
        let entry = entry.ok()?;
        entry.file_type()?.is_file().then(|| entry.into_path())
    }))
}

fn live_paths(root: &Path, file_globs: &[String]) -> Result<Vec<String>, ApiError> {
    let mut paths = live_walk(root, file_globs)?
        .map(|path| project_path(root, &path))
        .collect::<Vec<_>>();
    paths.sort_unstable();
    Ok(paths)
}

fn path_rank_tier(path: &str) -> u8 {
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("vendor/")
        || lower.contains("/vendor/")
        || lower.starts_with("third_party/")
        || lower.contains("/third_party/")
        || lower.starts_with("node_modules/")
        || lower.contains("/node_modules/")
        || lower.starts_with("docs/")
        || lower.contains("/docs/")
        || lower.ends_with(".md")
        || lower.ends_with(".txt")
        || lower.ends_with(".rst")
        || lower.ends_with(".1")
        || lower.ends_with(".lock")
    {
        2
    } else if codestory_workspace::cochange::is_test_or_fixture_path(&lower) {
        1
    } else {
        0
    }
}

fn scan_live_tree_with_paths(
    root: &Path,
    file_globs: &[String],
    needles: &[&[u8]],
    whole_word: bool,
    keep: usize,
) -> Result<(Vec<LiveFile>, Vec<String>), ApiError> {
    let paths = live_walk(root, file_globs)?.collect::<Vec<_>>();
    let all_paths = paths
        .iter()
        .map(|p| project_path(root, p))
        .collect::<Vec<_>>();
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
    files.sort_unstable_by(|left, right| {
        (Reverse(left.matched_terms), &left.path).cmp(&(Reverse(right.matched_terms), &right.path))
    });
    Ok((files, all_paths))
}

fn scan_live_tree_ranked(
    root: &Path,
    file_globs: &[String],
    query: &RankedQuery,
    keep: usize,
) -> Result<RankedScan, ApiError> {
    let paths = live_walk(root, file_globs)?.collect::<Vec<_>>();
    let total_files = paths.len().max(1) as f64;
    let all_paths = paths
        .iter()
        .map(|p| project_path(root, p))
        .collect::<Vec<_>>();
    let raw_count = query.raw_terms.len();
    let keep_per_file = keep.max(512);
    let mut files = paths
        .par_iter()
        .map_init(Vec::new, |bytes, path| {
            if !read_live_text(path, bytes) {
                return None;
            }
            match_live_file_ranked(root, path, bytes, query, keep_per_file)
        })
        .flatten()
        .collect::<Vec<_>>();

    // For tiny test workspaces (<= 3 matched files), keep exact legacy behavior on raw_terms
    // if all hits came from raw_terms; only expand split-term hits when multiple files match
    // or a raw term had zero matches.
    let mut df = vec![0usize; query.terms.len()];
    for file in &files {
        for (idx, count) in file.term_hits.iter().enumerate() {
            if *count > 0 {
                df[idx] += 1;
            }
        }
    }
    let weighted_terms = query
        .terms
        .iter()
        .enumerate()
        .map(|(idx, spec)| {
            let n = df[idx] as f64;
            let idf = ((total_files - n + 0.5) / (n + 0.5) + 1.0).ln();
            WeightedTerm {
                text: spec.text.clone(),
                idf,
                boost: spec.boost,
            }
        })
        .collect::<Vec<_>>();

    // If raw terms already hit <= 3 files and we did not need split fallback, preserve exact hits
    // for tiny unit-test determinism; otherwise keep all ranked matches and score by BM25F + IDF.
    let raw_hit_files = files
        .iter()
        .filter(|f| f.term_hits.iter().take(raw_count).any(|&c| c > 0))
        .count();
    if raw_hit_files <= 3 && files.len() <= 3 && df.iter().take(raw_count).all(|&d| d > 0) {
        for f in &mut files {
            f.kept.truncate(keep);
        }
        files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
        return Ok(RankedScan {
            files,
            all_paths,
            weighted_terms,
        });
    }

    for file in &mut files {
        let lower_path = file.path.to_ascii_lowercase();
        let mut score = 0.0_f64;
        let mut raw_matched = 0usize;
        let mut split_matched = 0usize;
        let mut path_matched = 0usize;
        for (idx, (&cnt, wt)) in file.term_hits.iter().zip(&weighted_terms).enumerate() {
            let spec = &query.terms[idx];
            if cnt > 0 {
                let tf = cnt as f64;
                let bm25_tf = (2.2 * tf) / (tf + 1.2);
                score += wt.idf * wt.boost * bm25_tf;
                if idx < raw_count {
                    raw_matched += 1;
                } else {
                    split_matched += 1;
                }
            }
            let term_lower = spec.text.to_ascii_lowercase();
            if term_lower.len() >= 4 && lower_path.contains(&term_lower) {
                path_matched += 1;
                score += wt.idf * if spec.is_path_hint { 2.5 } else { 1.3 };
            }
        }
        if raw_matched >= 2 || (raw_matched + split_matched) >= 2 {
            score *= 1.0 + 0.25 * ((raw_matched + split_matched).min(5) as f64);
        }
        if file.defines_query_symbol {
            score += 4.5;
        }
        if path_matched > 0 {
            score += 3.0;
        }
        match path_rank_tier(&file.path) {
            0 => score += 2.0,
            1 => score += 0.2,
            _ => score *= 0.25,
        }
        file.rank_score = score;
    }

    files.sort_unstable_by(|left, right| {
        right
            .rank_score
            .total_cmp(&left.rank_score)
            .then_with(|| Reverse(left.matched_terms).cmp(&Reverse(right.matched_terms)))
            .then_with(|| left.path.cmp(&right.path))
    });

    Ok(RankedScan {
        files,
        all_paths,
        weighted_terms,
    })
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
    let mut matched_terms = 0usize;
    let mut term_hits = vec![0u32; needles.len()];
    for (idx, needle) in needles.iter().enumerate() {
        let mut from = 0;
        let mut count = 0u32;
        while let Some(at) = find_from(bytes, needle, from) {
            let end = at + needle.len();
            if whole_word && !(is_word_start(bytes, at) && is_word_end(bytes, end)) {
                from = at + 1;
                continue;
            }
            count = count.saturating_add(1);
            offsets.push(at);
            // One row per line: resume after this line.
            from = line_end(bytes, end) + 1;
        }
        term_hits[idx] = count;
        if count > 0 {
            matched_terms += 1;
        }
    }
    if offsets.is_empty() {
        return None;
    }
    offsets.sort_unstable();
    let mut hit_lines = Vec::new();
    let mut kept = Vec::new();
    let mut defines_query_symbol = false;
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
            let cell = code_cell(&bytes[start..line_end(bytes, at)]);
            if !defines_query_symbol && starts_declaration(&cell) {
                defines_query_symbol = true;
            }
            kept.push((line, cell));
        }
    }
    Some(LiveFile {
        path: project_path(root, path),
        absolute: path.to_path_buf(),
        sha256: sha256_hex(bytes),
        hit_lines,
        kept,
        matched_terms,
        defines_query_symbol,
        rank_score: 0.0,
        term_hits,
    })
}

fn match_live_file_ranked(
    root: &Path,
    path: &Path,
    bytes: &[u8],
    query: &RankedQuery,
    keep: usize,
) -> Option<LiveFile> {
    let rel = project_path(root, path);
    let rel_lower = rel.to_ascii_lowercase();
    let mut offsets_by_term: Vec<Vec<usize>> = vec![Vec::new(); query.terms.len()];
    let mut matched_terms = 0usize;
    let mut term_hits = vec![0u32; query.terms.len()];
    let mut any_hit = false;

    for (idx, spec) in query.terms.iter().enumerate() {
        let mut from = 0;
        let mut count = 0u32;
        while let Some(at) = find_from(bytes, &spec.bytes, from) {
            let end = at + spec.bytes.len();
            if spec.whole_word && !(is_word_start(bytes, at) && is_word_end(bytes, end)) {
                from = at + 1;
                continue;
            }
            count = count.saturating_add(1);
            if offsets_by_term[idx].len() < 128 {
                offsets_by_term[idx].push(at);
            }
            from = line_end(bytes, end) + 1;
        }
        if count == 0 && spec.is_path_hint && rel_lower.contains(&spec.text.to_ascii_lowercase()) {
            count = 1;
            offsets_by_term[idx].push(0);
        }
        term_hits[idx] = count;
        if count > 0 {
            matched_terms += 1;
            any_hit = true;
        }
    }
    if !any_hit {
        return None;
    }

    let mut all_offsets = offsets_by_term.into_iter().flatten().collect::<Vec<_>>();
    all_offsets.sort_unstable();

    let mut hit_lines = Vec::new();
    let mut kept = Vec::new();
    let mut defines_query_symbol = false;
    let (mut line, mut counted) = (1_u32, 0_usize);
    for at in all_offsets {
        if at > counted {
            line += bytes[counted..at]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count() as u32;
            counted = at;
        }
        if hit_lines.last() == Some(&line) {
            continue;
        }
        hit_lines.push(line);
        if kept.len() < keep {
            let start = bytes[..at]
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |newline| newline + 1);
            let cell = code_cell(&bytes[start..line_end(bytes, at)]);
            if !defines_query_symbol && starts_declaration(&cell) {
                defines_query_symbol = true;
            }
            kept.push((line, cell));
        }
    }
    Some(LiveFile {
        path: rel,
        absolute: path.to_path_buf(),
        sha256: sha256_hex(bytes),
        hit_lines,
        kept,
        matched_terms,
        defines_query_symbol,
        rank_score: 0.0,
        term_hits,
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
        matched_terms: 0,
        defines_query_symbol: false,
        rank_score: 0.0,
        term_hits: Vec::new(),
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
    /// What was served for a path, and the hash of the bytes it was served for.
    served: HashMap<PathBuf, (String, Served)>,
    /// Files opened by their index id, `None` for one that is not verified.
    opened: HashMap<i64, Option<Rc<Verified>>>,
}

impl<'a> Index<'a> {
    fn new(store: &'a Store) -> Self {
        Self {
            store,
            served: HashMap::new(),
            opened: HashMap::new(),
        }
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
        absolute.strip_prefix(root).ok()?;
        let (file, bytes) = open_live(root, absolute)?;
        let definitions = self.serve(&file).1?;
        Some(Verified {
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

/// The declarations that `owner` and the definitions nested in it reference,
/// in order of first use and `most` at the most: the targets of the outgoing
/// edges the index resolved with certainty, each one only as a declaration in
/// a verified file. When a symbol has no directly located certain targets,
/// falls back to verified import-binding targets, unambiguous same-file
/// declarations referenced by its body, or its enclosing class initializer.
fn used_by(
    index: &mut Index<'_>,
    root: &Path,
    owner: &Located,
    most: usize,
    allow_fallback: bool,
    shown: impl Fn(&Located) -> bool,
) -> Vec<Located> {
    let nested = |first: u32, last: u32| owner.first <= first && last <= owner.last;
    // Where each edge is in the text, or else the definition it starts from.
    let mut sites = Vec::new();
    let mut all_outgoing = Vec::new();
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
            if allow_fallback {
                all_outgoing.push((source.start, edge));
            }
        }
    }
    sites.sort_unstable();
    let mut seen = HashSet::new();
    let mut used = Vec::new();
    let mut had_any_certain_target = false;
    for (_, _, target) in &sites {
        if used.len() == most {
            break;
        }
        if seen.insert(*target)
            && let Some(target) = index.locate(root, *target)
            && !(Rc::ptr_eq(&target.home, &owner.home) && nested(target.first, target.last))
            && !matches!(
                target.definition().kind,
                NodeKind::MODULE | NodeKind::NAMESPACE | NodeKind::PACKAGE
            )
        {
            had_any_certain_target = true;
            if !shown(&target) {
                used.push(target);
            }
        }
    }
    if allow_fallback && used.is_empty() && !had_any_certain_target && most > 0 {
        fallback_used_by(
            index,
            root,
            owner,
            most,
            &all_outgoing,
            &mut seen,
            &mut used,
            &nested,
            &shown,
        );
    }
    used
}

#[allow(clippy::too_many_arguments)]
fn fallback_used_by(
    index: &mut Index<'_>,
    root: &Path,
    owner: &Located,
    most: usize,
    all_outgoing: &[(u32, Edge)],
    seen: &mut HashSet<NodeId>,
    used: &mut Vec<Located>,
    nested: &impl Fn(u32, u32) -> bool,
    shown: &impl Fn(&Located) -> bool,
) {
    let try_push = |used: &mut Vec<Located>, seen: &mut HashSet<NodeId>, target: Located| -> bool {
        if used.len() >= most {
            return false;
        }
        let def_id = target.definition().id;
        if !seen.insert(def_id)
            || (Rc::ptr_eq(&target.home, &owner.home) && nested(target.first, target.last))
            || matches!(
                target.definition().kind,
                NodeKind::MODULE
                    | NodeKind::NAMESPACE
                    | NodeKind::PACKAGE
                    | NodeKind::FIELD
                    | NodeKind::ENUM_CONSTANT
            )
            || shown(&target)
        {
            return false;
        }
        used.push(target);
        true
    };

    // 1. Follow Certain or Probable outgoing edges that target import binding nodes
    // or unambiguous Probable targets.
    let mut ordered_edges = all_outgoing
        .iter()
        .map(|(start, edge)| (edge.line.unwrap_or(*start), edge.id.0, edge))
        .collect::<Vec<_>>();
    ordered_edges.sort_by_key(|(line, id, _)| (*line, *id));

    let imported_homes = imported_verified_homes(index, root, owner);

    for (_, _, edge) in &ordered_edges {
        if used.len() >= most {
            return;
        }
        if edge.kind == EdgeKind::MEMBER {
            continue;
        }
        let effective = edge.effective_target();
        if is_certain(edge) {
            if let Some(located) = locate_via_imported_homes(index, &imported_homes, effective) {
                try_push(used, seen, located);
                continue;
            }
        } else if edge.callsite_identity.is_some() && edge.confidence.unwrap_or_default() >= 0.62 {
            if let Some(located) = index.locate(root, effective) {
                let short = name_segments(&located.definition().name)
                    .last()
                    .copied()
                    .unwrap_or(located.definition().name.as_str());
                let count = located
                    .home
                    .definitions
                    .iter()
                    .filter(|def| {
                        !is_local_variable(def, &located.home.definitions)
                            && name_segments(&def.name).last().copied() == Some(short)
                    })
                    .count();
                if count == 1 {
                    try_push(used, seen, located);
                    continue;
                }
            }
            if let Some(located) = locate_via_imported_homes(index, &imported_homes, effective) {
                try_push(used, seen, located);
                continue;
            }
        }
    }

    // 2. Same-file or imported declarations referenced by outgoing CALL / TYPE_USAGE / USAGE edges.
    for (_, _, edge) in &ordered_edges {
        if used.len() >= most {
            return;
        }
        if !matches!(
            edge.kind,
            EdgeKind::CALL | EdgeKind::TYPE_USAGE | EdgeKind::USAGE | EdgeKind::ANNOTATION_USAGE
        ) {
            continue;
        }
        let Some(target_node) = index.store.get_node(edge.target).ok().flatten() else {
            continue;
        };
        let Some(short) = name_segments(&target_node.serialized_name).last().copied() else {
            continue;
        };
        if let Some(located) = locate_unique_in_home(&owner.home, short, nested) {
            try_push(used, seen, located);
        } else {
            for home in &imported_homes {
                if let Some(located) = locate_unique_in_home(home, short, &|_, _| false) {
                    try_push(used, seen, located);
                    break;
                }
            }
        }
    }

    if !used.is_empty() {
        return;
    }

    // 3. Whole-word mentions of same-file non-local declarations in owner's source lines.
    let home_lines = owner.home.text.lines().collect::<Vec<_>>();
    let first_idx = (owner.first as usize).saturating_sub(1);
    let last_idx = (owner.last as usize).min(home_lines.len());
    if first_idx < last_idx {
        let body_slice = &home_lines[first_idx..last_idx];
        for (slot, def) in owner.home.definitions.iter().enumerate() {
            if used.len() >= most {
                break;
            }
            if nested(def.start, def.end) || is_local_variable(def, &owner.home.definitions) {
                continue;
            }
            let Some(short) = name_segments(&def.name).last().copied() else {
                continue;
            };
            if short.len() < 3 {
                continue;
            }
            let mentioned = body_slice
                .iter()
                .any(|line| word_offset(line, short).is_some());
            if mentioned && let Some((first, last, note)) = declared_range(&home_lines, def) {
                try_push(
                    used,
                    seen,
                    Located {
                        home: Rc::clone(&owner.home),
                        slot,
                        first,
                        last,
                        note,
                    },
                );
            }
        }
    }

    if !used.is_empty() {
        return;
    }

    // 4. If owner is a method inside an enclosing class/struct/interface, point to its
    // constructor (__init__ / new) or the enclosing type itself.
    if let Some((enclosing_slot, enclosing)) = owner
        .home
        .definitions
        .iter()
        .enumerate()
        .filter(|(_, def)| {
            matches!(
                def.kind,
                NodeKind::CLASS | NodeKind::STRUCT | NodeKind::INTERFACE | NodeKind::ENUM
            ) && def.start < owner.first
                && owner.last <= def.end
        })
        .max_by_key(|(_, def)| (def.start, Reverse(def.end)))
    {
        for (slot, def) in owner.home.definitions.iter().enumerate() {
            if def.start >= enclosing.start
                && def.end <= enclosing.end
                && !nested(def.start, def.end)
                && matches!(def.kind, NodeKind::METHOD | NodeKind::FUNCTION)
                && matches!(
                    name_segments(&def.name).last().copied(),
                    Some("__init__" | "new" | "init")
                )
                && let Some((first, last, note)) = declared_range(&home_lines, def)
                && try_push(
                    used,
                    seen,
                    Located {
                        home: Rc::clone(&owner.home),
                        slot,
                        first,
                        last,
                        note,
                    },
                )
            {
                return;
            }
        }
        if let Some((first, last, note)) = declared_range(&home_lines, enclosing) {
            let _ = try_push(
                used,
                seen,
                Located {
                    home: Rc::clone(&owner.home),
                    slot: enclosing_slot,
                    first,
                    last,
                    note,
                },
            );
        }
    }
}

fn imported_verified_homes(
    index: &mut Index<'_>,
    root: &Path,
    owner: &Located,
) -> Vec<Rc<Verified>> {
    let mut source_ids = Vec::new();
    if let Ok(Some(file_row)) = index.store.get_file_by_path(&owner.home.file.absolute) {
        source_ids.push(NodeId(file_row.id));
    }
    for def in owner.home.definitions.iter() {
        if matches!(
            def.kind,
            NodeKind::MODULE | NodeKind::NAMESPACE | NodeKind::PACKAGE
        ) || (owner.first <= def.start && def.end <= owner.last)
        {
            source_ids.push(def.id);
        }
    }
    source_ids.sort_unstable();
    source_ids.dedup();

    let mut file_ids = Vec::new();
    for source_id in source_ids {
        let edges = index
            .store
            .get_outgoing_edges_for_node_id(
                source_id,
                &[EdgeKind::IMPORT],
                TrailCallerScope::IncludeTestsAndBenches,
                true,
            )
            .unwrap_or_default();
        for edge in edges {
            let target_id = edge.effective_target();
            if let Ok(Some(node)) = index.store.get_node(target_id) {
                if let Some(fid) = node.file_node_id {
                    file_ids.push(fid.0);
                } else if node.kind == NodeKind::FILE {
                    file_ids.push(node.id.0);
                }
            }
        }
    }
    file_ids.sort_unstable();
    file_ids.dedup();
    file_ids
        .into_iter()
        .filter_map(|fid| index.open(root, fid))
        .filter(|home| !Rc::ptr_eq(home, &owner.home))
        .collect()
}

fn locate_via_imported_homes(
    index: &mut Index<'_>,
    imported_homes: &[Rc<Verified>],
    target_id: NodeId,
) -> Option<Located> {
    let target_node = index.store.get_node(target_id).ok()??;
    let short = name_segments(&target_node.serialized_name)
        .last()
        .copied()?;
    for home in imported_homes {
        if let Some(located) = locate_unique_in_home(home, short, &|_, _| false) {
            return Some(located);
        }
    }
    None
}

fn locate_unique_in_home(
    home: &Rc<Verified>,
    short_name: &str,
    excluded_span: &impl Fn(u32, u32) -> bool,
) -> Option<Located> {
    let mut matches = home
        .definitions
        .iter()
        .enumerate()
        .filter(|(_, def)| {
            !excluded_span(def.start, def.end)
                && !is_local_variable(def, &home.definitions)
                && !matches!(
                    def.kind,
                    NodeKind::MODULE | NodeKind::NAMESPACE | NodeKind::PACKAGE
                )
                && (def.name == short_name
                    || name_segments(&def.name).last().copied() == Some(short_name))
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        matches.retain(|(_, def)| is_scope_kind(def.kind));
    }
    let [(slot, def)] = matches.as_slice() else {
        return None;
    };
    let lines = home.text.lines().collect::<Vec<_>>();
    let (first, last, note) = declared_range(&lines, def)?;
    Some(Located {
        home: Rc::clone(home),
        slot: *slot,
        first,
        last,
        note,
    })
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
#[allow(clippy::too_many_arguments)]
fn list_file(
    out: &mut String,
    remaining: &mut usize,
    file: &LiveFile,
    (mark, definitions): Served,
    rows: &[Row<'_>],
    unlisted: usize,
    bodies: Option<&mut Bodies>,
    max_blocks_per_file: Option<usize>,
    term_weights: Option<&[WeightedTerm]>,
) {
    let path = &file.path;
    let definitions = definitions.as_deref().map(Vec::as_slice);
    let shown = rows.len().min(*remaining);
    *remaining -= shown;
    if shown > 0 {
        if let Some(bodies_ref) = bodies.as_deref()
            && bodies_ref.bytes_exhausted(out.len())
        {
            let total_more = rows.len() + unlisted;
            if total_more > 0 {
                let _ = writeln!(out, "{path}: {total_more} more");
            }
            return;
        }
        let _ = writeln!(out, "{path}{mark}");
    }
    let mut listed = &rows[..shown];
    let mut bodies = bodies;
    if let Some(bodies_ref) = bodies.as_deref_mut() {
        listed = list_bodies(
            out,
            bodies_ref,
            file,
            definitions,
            listed,
            max_blocks_per_file,
            term_weights,
        );
        bodies_ref.overflowed |= !listed.is_empty();
    }
    let mut skipped_by_bytes = 0usize;
    for row in listed {
        if let Some(bodies_ref) = bodies.as_deref_mut()
            && bodies_ref.bytes_exhausted(out.len())
        {
            bodies_ref.overflowed = true;
            skipped_by_bytes += 1;
            continue;
        }
        list_row(out, row, definitions);
    }
    let more = rows.len() - shown + unlisted + skipped_by_bytes;
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
    index: Option<&mut Index<'_>>,
    bodies: Option<&mut Bodies>,
) {
    list_hits_ranked(out, files, limit, index, bodies, None);
}

fn list_hits_ranked(
    out: &mut String,
    files: &[LiveFile],
    limit: usize,
    mut index: Option<&mut Index<'_>>,
    mut bodies: Option<&mut Bodies>,
    term_weights: Option<&[WeightedTerm]>,
) {
    let mut remaining = limit;
    let max_blocks_per_file = (files.len() > 3).then_some(3);
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
            max_blocks_per_file,
            term_weights,
        );
    }
}

fn append_related_tests_footer(
    out: &mut String,
    root: &Path,
    files: &[LiveFile],
    all_paths: &[String],
    terms: &[String],
    max_bytes: Option<usize>,
) {
    if let Some(cap) = max_bytes
        && out.len() + 120 >= cap
    {
        return;
    }
    let Some(top_prod) = files.iter().find(|f| path_rank_tier(&f.path) == 0) else {
        return;
    };
    let extra = terms.iter().map(String::as_str).collect::<Vec<_>>();
    let related = codestory_workspace::cochange::find_related_tests_and_fixtures_in_paths(
        root,
        &top_prod.path,
        Some(all_paths),
        &extra,
        4,
    );
    let shown_paths = files
        .iter()
        .take(5)
        .map(|f| f.path.as_str())
        .collect::<HashSet<_>>();
    let novel = related
        .into_iter()
        .filter(|r| !shown_paths.contains(r.path.as_str()))
        .collect::<Vec<_>>();
    if novel.is_empty() {
        return;
    }
    let mut block = String::from("related tests & fixtures:\n");
    for item in novel {
        let _ = writeln!(block, "  {}", item.path);
    }
    if max_bytes.is_none_or(|cap| out.len() + block.len() <= cap) {
        out.push_str(&block);
    }
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/// Line ranges one reply has printed, by the path it shows for their file.
#[derive(Default)]
struct Printed(HashMap<String, Vec<(u32, u32)>>);

impl Printed {
    fn add(&mut self, path: &str, first: u32, last: u32) {
        self.0
            .entry(path.to_owned())
            .or_default()
            .push((first, last));
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
    /// Optional total reply byte ceiling.
    max_bytes: Option<usize>,
    /// Declarations already printed, so that none is printed twice.
    declared: HashSet<NodeId>,
    /// The lines printed for them and for context.
    printed: Printed,
    /// Whether a hit or a used declaration stayed one row because its body
    /// did not fit.
    overflowed: bool,
}

impl Bodies {
    fn with_byte_cap(budget: usize, max_bytes: Option<usize>) -> Self {
        Self {
            budget,
            max_bytes,
            declared: HashSet::new(),
            printed: Printed::default(),
            overflowed: false,
        }
    }

    fn bytes_exhausted(&self, current_bytes: usize) -> bool {
        self.max_bytes.is_some_and(|cap| current_bytes >= cap)
    }

    fn fits_bytes(&self, current_bytes: usize, additional: usize) -> bool {
        self.max_bytes
            .is_none_or(|cap| current_bytes.saturating_add(additional) <= cap)
    }

    /// Whether the reply already shows `declaration`, whole or windowed.
    fn shows(&self, declaration: &Located) -> bool {
        self.declared.contains(&declaration.definition().id) || self.printed.holds(declaration)
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

fn score_block_with_terms(lines: &[&str], block: &Block<'_>, term_weights: &[WeightedTerm]) -> f64 {
    let mut score = 0.0_f64;
    let mut matched = vec![false; term_weights.len()];
    for &(first, last) in &block.windows {
        let s = (first as usize).saturating_sub(1);
        let e = (last as usize).min(lines.len());
        for line in &lines[s..e] {
            for (idx, wt) in term_weights.iter().enumerate() {
                if !matched[idx] && line.contains(&wt.text) {
                    matched[idx] = true;
                    score += wt.idf * wt.boost;
                }
            }
        }
    }
    if let Some((def, ..)) = block.declaration {
        for (idx, wt) in term_weights.iter().enumerate() {
            if def.name == wt.text || def.qualified == wt.text {
                score += wt.idf * 3.0;
            } else if def.name.contains(&wt.text) || def.qualified.contains(&wt.text) {
                score += wt.idf * 1.5;
            }
            let _ = idx;
        }
    }
    if let Some(&(first, _)) = block.windows.first()
        && let Some(first_line) = lines.get(first as usize - 1)
        && starts_declaration(first_line)
    {
        for wt in term_weights {
            if first_line.contains(&wt.text) {
                score += wt.idf * 1.5;
            }
        }
    }
    score
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
        let mut snippet = String::new();
        if let Some((definition, first, last, note)) = title {
            let _ = writeln!(
                snippet,
                "  == {} {first}-{last}{note}",
                definition.qualified
            );
        }
        if gap {
            snippet.push_str("  ...\n");
        }
        for number in first..=last {
            let (sign, note) = match notes.get(&number) {
                Some(note) => ('>', *note),
                None => (':', ""),
            };
            let _ = write!(snippet, "  {number}{sign}");
            let code = bounded(lines[number as usize - 1].trim_end());
            if !code.is_empty() {
                let _ = write!(snippet, " {code}");
            }
            let _ = writeln!(snippet, "{note}");
        }
        if !bodies.fits_bytes(out.len(), snippet.len()) {
            bodies.overflowed = true;
            break;
        }
        bodies.budget -= cost;
        bodies.printed.add(path, first, last);
        if let Some((definition, ..)) = title {
            bodies.declared.insert(definition.id);
        }
        out.push_str(&snippet);
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
    max_blocks_per_file: Option<usize>,
    term_weights: Option<&[WeightedTerm]>,
) -> &'r [Row<'c>] {
    // A body spends a line on each hit it shows, so this bounds the planning.
    let (fitting, rest) = if term_weights.is_some() {
        (rows, &rows[0..0])
    } else {
        rows.split_at(rows.len().min(bodies.budget))
    };
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
    let mut blocks = plan_bodies(&lines, definitions, &hits);
    if let Some(weights) = term_weights
        && blocks.len() > 1
    {
        let mut scored = blocks
            .into_iter()
            .enumerate()
            .map(|(idx, block)| {
                let s = score_block_with_terms(&lines, &block, weights);
                (s, idx, block)
            })
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        blocks = scored.into_iter().map(|(_, _, block)| block).collect();
    }
    let mut tail = 0;
    let mut printed_blocks = 0usize;
    let mut unprinted_rows = Vec::new();
    for block in &blocks {
        let cap_reached = max_blocks_per_file.is_some_and(|cap| printed_blocks >= cap);
        let end = if cap_reached || bodies.bytes_exhausted(out.len()) {
            0
        } else {
            let e = print_body(out, bodies, &file.path, &lines, block, &notes, tail);
            if e > 0 {
                printed_blocks += 1;
            }
            e
        };
        tail = if term_weights.is_some() { 0 } else { end };
        for row in block
            .hits
            .iter()
            .filter(|line| **line > end || (end == 0))
            .filter_map(|line| by_line.get(line))
        {
            if bodies.bytes_exhausted(out.len()) {
                bodies.overflowed = true;
                continue;
            }
            if term_weights.is_none() {
                list_row(out, row, definitions);
                (bodies.overflowed, tail) = (true, 0);
            } else {
                unprinted_rows.push(*row);
            }
        }
    }
    if term_weights.is_some() && !unprinted_rows.is_empty() && !bodies.bytes_exhausted(out.len()) {
        for row in unprinted_rows.into_iter().take(12) {
            if !bodies.fits_bytes(out.len(), 120) {
                bodies.overflowed = true;
                break;
            }
            list_row(out, row, definitions);
            bodies.overflowed = true;
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
            let _ = writeln!(out, "{owner} uses:");
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
    store: &Store,
    name: &str,
    wanted: &[&str],
    files: &[LiveFile],
    all_paths: &[String],
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
    // the definitions a reference there resolved to with certainty (or probable).
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
    let mut probable_resolved_at = HashMap::<(usize, u32), Vec<usize>>::new();
    let mut index_only = BTreeMap::<i64, BTreeSet<u32>>::new();
    let mut verified_hit_files = HashMap::<i64, Option<usize>>::new();
    for (slot, candidate) in candidates.iter().enumerate() {
        for ((file_id, line), (certain, probable)) in reference_sites(store, candidate.id) {
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
                Some(position) if probable => {
                    probable_resolved_at
                        .entry((position, line))
                        .or_default()
                        .push(slot);
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
        let mut certain_targets = resolved_at
            .get(&(position, line))
            .into_iter()
            .flatten()
            .map(|slot| &candidates[*slot])
            .filter(|candidate| candidate.wanted);
        if let Some(target) = certain_targets.next()
            && reported > 1
            && !defines(position, line)
            && certain_targets.all(|other| other.id == target.id)
        {
            return format!(" -> {}:{}", files[target.file].path, target.line);
        }
        let mut probable_targets = probable_resolved_at
            .get(&(position, line))
            .into_iter()
            .flatten()
            .map(|slot| &candidates[*slot])
            .filter(|candidate| candidate.wanted);
        if let Some(target) = probable_targets.next()
            && reported > 1
            && !defines(position, line)
            && probable_targets.all(|other| other.id == target.id)
        {
            return format!(" -> {}:{} (probable)", files[target.file].path, target.line);
        }
        String::new()
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
    let rank_files = bodies.is_some() && files.len() > 3;
    if rank_files {
        let wanted_files: HashSet<usize> = candidates
            .iter()
            .filter(|c| c.wanted)
            .map(|c| c.file)
            .collect();
        let certain_files: HashSet<usize> = resolved_at
            .iter()
            .filter(|(_, slots)| slots.iter().any(|s| candidates[*s].wanted))
            .map(|((pos, _), _)| *pos)
            .collect();
        let probable_files: HashSet<usize> = probable_resolved_at
            .iter()
            .filter(|(_, slots)| slots.iter().any(|s| candidates[*s].wanted))
            .map(|((pos, _), _)| *pos)
            .collect();
        listing.sort_by(|left, right| {
            let left_rank = (
                Reverse(left.1.is_some_and(|p| wanted_files.contains(&p))),
                Reverse(left.1.is_some_and(|p| certain_files.contains(&p))),
                Reverse(left.1.is_some_and(|p| probable_files.contains(&p))),
                path_rank_tier(&left.0.path),
                left.0.path.as_str(),
            );
            let right_rank = (
                Reverse(right.1.is_some_and(|p| wanted_files.contains(&p))),
                Reverse(right.1.is_some_and(|p| certain_files.contains(&p))),
                Reverse(right.1.is_some_and(|p| probable_files.contains(&p))),
                path_rank_tier(&right.0.path),
                right.0.path.as_str(),
            );
            left_rank.cmp(&right_rank)
        });
    } else {
        listing.sort_by(|left, right| left.0.path.cmp(&right.0.path));
    }

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

    let mut ordered_wanted = candidates
        .iter()
        .filter(|candidate| candidate.wanted)
        .collect::<Vec<_>>();
    if rank_files && ordered_wanted.len() > DEFINITION_BODIES {
        ordered_wanted.sort_by_key(|c| {
            (
                path_rank_tier(&files[c.file].path),
                &files[c.file].path,
                c.line,
            )
        });
    }
    let body_def_cap = if rank_files {
        DEFINITION_BODIES
    } else if reported <= DEFINITION_BODIES {
        reported
    } else {
        0
    };

    // Hit lines already shown, marked, inside a definition printed up here.
    let mut above = HashMap::<usize, Vec<(u32, u32)>>::new();
    let mut body_candidates = Vec::new();
    for (idx, candidate) in ordered_wanted.iter().take(limit).enumerate() {
        let file = &files[candidate.file];
        let _ = writeln!(
            out,
            "  {}:{} {} {}",
            file.path,
            candidate.line,
            kind_label(candidate.kind),
            candidate.qualified
        );
        if idx < body_def_cap
            && let Some(bodies) = bodies.as_deref_mut()
            && !bodies.bytes_exhausted(out.len())
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
            body_candidates.push(*candidate);
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
    if (reported <= DEFINITION_BODIES || rank_files)
        && let Some(bodies) = bodies.as_deref_mut()
    {
        for candidate in body_candidates {
            if let Some(owner) = index.locate(root, candidate.id) {
                let targets = used_by(
                    &mut index,
                    root,
                    &owner,
                    USED_DECLARATIONS,
                    true,
                    |target| bodies.shows(target),
                );
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
            (listing.len() > 3).then_some(3),
            None,
        );
    }
    if omitted_hits > 0 {
        let _ = writeln!(
            out,
            "{omitted_hits} hit(s) omitted: the index resolved them to other definitions of {last}"
        );
    }
    if let Some(bodies) = bodies {
        bodies.budget += reserved;
        for (owner, targets) in &used {
            list_used(&mut out, bodies, owner, targets);
        }
        if rank_files {
            append_related_tests_footer(
                &mut out,
                root,
                files,
                all_paths,
                &[last.to_string()],
                bodies.max_bytes,
            );
        }
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
/// from incoming edges of any certainty, returning `(certain, probable)`.
///
/// An edge's own line is where its expression starts. When the edge points at
/// a per-site placeholder, the placeholder's occurrence is the name token
/// itself, which is the line a reader looks for in a call split over lines.
fn reference_sites(store: &Store, definition: NodeId) -> BTreeMap<(i64, u32), (bool, bool)> {
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
    let mut sites: BTreeMap<(i64, u32), (bool, bool)> = BTreeMap::new();
    for edge in &edges {
        let certainty = edge
            .certainty
            .or_else(|| ResolutionCertainty::from_confidence(edge.confidence));
        let certain = certainty == Some(ResolutionCertainty::Certain);
        let probable = certainty == Some(ResolutionCertainty::Probable);
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
            let entry = sites.entry(site).or_default();
            entry.0 |= certain;
            entry.1 |= probable;
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
        matched_terms: 1,
        defines_query_symbol: false,
        rank_score: 0.0,
        term_hits: Vec::new(),
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
    byte_cap: Option<usize>,
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
        let limit = ((last + 1 - first) as usize).min(*budget);
        let (first_idx, last_idx) = (first as usize, last as usize);
        let mut shown = 0usize;
        for (offset, code) in self.lines[first_idx - 1..][..limit].iter().enumerate() {
            if shown >= 1
                && let Some(cap) = self.byte_cap
                && out.len() + code.len() + 64 > cap
            {
                break;
            }
            let _ = write!(out, "{}:", first_idx + offset);
            if !code.is_empty() {
                let _ = write!(out, " {}", printable(code));
            }
            out.push('\n');
            shown += 1;
        }
        if shown > 0 {
            self.printed.add(path, first, first + shown as u32 - 1);
        }
        *budget -= shown;
        let next = first_idx + shown;
        if next <= last_idx {
            let _ = writeln!(
                out,
                "... {} more lines; read {path}:{next}-{last_idx}",
                last_idx + 1 - next
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
        from_index: bool,
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
        let note = if from_index { "" } else { APPROXIMATE };
        let _ = writeln!(out, "== {path} ({} lines; outline){note}", lines.len());
        // Last lines of the declarations that hold the row being printed.
        let mut open = Vec::new();
        let mut emitted_rows = 0usize;
        for (first, last, label) in rows.iter().take(*budget) {
            if emitted_rows >= 4
                && let Some(cap) = self.byte_cap
                && out.len() + label.len() + 96 > cap.min(9_500)
            {
                break;
            }
            while open.last().is_some_and(|end| end < first) {
                open.pop();
            }
            let _ = writeln!(out, "{}{first}-{last} {label}", "  ".repeat(open.len()));
            open.push(*last);
            emitted_rows += 1;
        }
        if rows.len() > emitted_rows {
            let _ = writeln!(out, "... {} more declarations", rows.len() - emitted_rows);
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
    let mut used = used_by(index, root, &owner, USED_POINTERS, true, |target| {
        printed.holds(target)
    });
    let mut extra_fixture_rows = Vec::new();
    if root.join(".git").exists()
        && !codestory_workspace::cochange::is_test_or_fixture_path(owner.path())
        && used.len() < USED_POINTERS
    {
        let short_owner = name_segments(&owner.definition().name)
            .last()
            .copied()
            .unwrap_or(owner.definition().name.as_str())
            .to_ascii_lowercase();
        for related in
            codestory_workspace::cochange::find_related_tests_and_fixtures(root, owner.path(), 2)
        {
            if used.len() + extra_fixture_rows.len() >= USED_POINTERS || related.score < 0.85 {
                break;
            }
            if used.iter().any(|u| u.path() == related.path) {
                continue;
            }
            let abs = root.join(&related.path);
            if let Ok(Some(file_row)) = index.store.get_file_by_path(&abs)
                && let Some(test_home) = index.open(root, file_row.id)
            {
                let test_lines = test_home.text.lines().collect::<Vec<_>>();
                let chosen = test_home
                    .definitions
                    .iter()
                    .enumerate()
                    .filter(|(_, def)| is_scope_kind(def.kind))
                    .find(|(_, def)| def.name.to_ascii_lowercase().contains(&short_owner))
                    .or_else(|| {
                        test_home
                            .definitions
                            .iter()
                            .enumerate()
                            .find(|(_, def)| is_scope_kind(def.kind))
                    });
                if let Some((slot, def)) = chosen
                    && let Some((first, last, note)) = declared_range(&test_lines, def)
                {
                    let candidate = Located {
                        home: Rc::clone(&test_home),
                        slot,
                        first,
                        last,
                        note,
                    };
                    if !printed.holds(&candidate) {
                        used.push(candidate);
                    }
                    continue;
                }
            }
            if related.is_fixture
                && let Some((live_file, bytes)) = open_live(root, abs)
            {
                let line_count = String::from_utf8_lossy(&bytes).lines().count().max(1);
                extra_fixture_rows.push(format!(
                    "  {}:1-{} file {}\n",
                    live_file.path, line_count, live_file.path
                ));
            }
        }
    }
    if !used.is_empty() || !extra_fixture_rows.is_empty() {
        out.push_str("uses:\n");
    }
    for target in &used {
        out.push_str(&target.pointer());
    }
    for row in &extra_fixture_rows {
        out.push_str(row);
    }
}

fn read_selector(
    out: &mut String,
    root: &Path,
    mut index: Option<&mut Index<'_>>,
    printed: &mut Printed,
    selector: &str,
    byte_cap: Option<usize>,
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
        byte_cap,
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
                    if byte_cap.is_none_or(|cap| out.len() + 240 < cap) {
                        list_used_pointers(out, root, index, page.printed, definition);
                    }
                }
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
            page.outline(
                out,
                &mut budget,
                definitions.as_deref().map(Vec::as_slice),
                definitions.is_some(),
            );
        }
        Target::File if byte_cap.is_some_and(|cap| out.len() + bytes.len() > cap) => {
            if let Some(defs) = definitions.as_deref() {
                page.outline(out, &mut budget, Some(defs), true);
            } else if let Some(live) = live_ast_slice_facts(&file.path, &text)
                && live
                    .definitions
                    .iter()
                    .filter(|d| is_scope_kind(d.kind))
                    .count()
                    >= 2
            {
                page.outline(out, &mut budget, Some(live.definitions.as_slice()), false);
            } else {
                page.outline(out, &mut budget, None, false);
            }
        }
        Target::File if count >= 1 => {
            page.block(out, &mut (count as usize), 1, count, "", "");
        }
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
                    for (def_idx, definition) in found.iter().enumerate() {
                        if def_idx > 0
                            && byte_cap.is_some_and(|cap| out.len() >= cap.saturating_sub(240))
                        {
                            let _ = writeln!(
                                out,
                                "== {}::{}: omitted (--max-bytes {} reached; read {}::{})",
                                file.path,
                                definition.qualified,
                                byte_cap.unwrap_or(0),
                                file.path,
                                definition.qualified
                            );
                            break;
                        }
                        page.declaration(out, &mut budget, definition, None);
                        if byte_cap.is_none_or(|cap| out.len() + 240 < cap) {
                            list_used_pointers(
                                out,
                                root,
                                index.as_deref_mut(),
                                page.printed,
                                definition,
                            );
                        }
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
    // If a signature wraps across lines before opening its `{` or `:`, allow
    // up to 8 signature lines to reach the opening delimiter.
    let mut header_open = opens_block(lines[start]);
    for (at, line) in lines.iter().enumerate().skip(start + 1) {
        let code = line.trim();
        if code.is_empty() {
            continue;
        }
        if indent(line) > depth {
            end = at;
            if code.ends_with(['{', ':']) {
                header_open = true;
            }
            continue;
        }
        if !header_open && at - start <= 8 {
            end = at;
            if code.ends_with(['{', ':']) || code.starts_with('{') {
                header_open = true;
                continue;
            }
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

// ---------------------------------------------------------------------------
// edit
// ---------------------------------------------------------------------------

/// Replace a declaration or line range addressed by `selector` (`path::Symbol`,
/// `path:LINE` or `path:A-B`) with `replacement`, validating syntax with
/// Tree-sitter before atomically writing the file.
pub fn edit(
    root: &Path,
    storage_path: &Path,
    selector: &str,
    replacement: &str,
) -> Result<String, ApiError> {
    let Some((path, target)) = parse_selector(selector) else {
        return Err(ApiError::invalid_argument(format!(
            "{selector}: expected path::Symbol, path:LINE or path:A-B"
        )));
    };
    if matches!(target, Target::File) {
        return Err(ApiError::invalid_argument(format!(
            "{selector}: edit requires path::Symbol, path:LINE or path:A-B"
        )));
    }
    let absolute = match codestory_workspace::resolve_project_relative_path(root, Path::new(path)) {
        Ok(ProjectRelativePathResolution::Existing { absolute, .. }) => absolute,
        Ok(ProjectRelativePathResolution::OutOfProject) => {
            return Err(ApiError::invalid_argument(format!(
                "{selector}: {path} is outside the project"
            )));
        }
        _ => {
            return Err(ApiError::invalid_argument(format!(
                "{selector}: no file {path}"
            )));
        }
    };
    let Some((file, bytes)) = open_live(root, absolute) else {
        return Err(ApiError::invalid_argument(format!(
            "{selector}: {path} is unreadable, binary or larger than 4 MB"
        )));
    };
    let text = String::from_utf8(bytes).map_err(|_| {
        ApiError::invalid_argument(format!("{selector}: {path} is not valid UTF-8"))
    })?;
    let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let had_trailing_newline = text.ends_with('\n');
    let lines = text.lines().collect::<Vec<_>>();
    let count = lines.len() as u32;

    let (first, last, label) = with_pinned_index(storage_path, |store| {
        let mut index = store.map(Index::new);
        let definitions = index.as_mut().and_then(|idx| idx.serve(&file).1);
        match target {
            Target::Range(first, last) => {
                if first < 1 || first > last || last > count {
                    return Err(ApiError::invalid_argument(format!(
                        "{selector}: range {first}-{last} is outside {} ({count} lines)",
                        file.path
                    )));
                }
                Ok((first, last, format!("{}:{first}-{last}", file.path)))
            }
            Target::Line(line) => {
                if line < 1 || line > count {
                    return Err(ApiError::invalid_argument(format!(
                        "{selector}: line {line} is outside {} ({count} lines)",
                        file.path
                    )));
                }
                let Some(def) = definitions
                    .as_deref()
                    .and_then(|defs| enclosing_scope(defs, line))
                else {
                    return Err(ApiError::invalid_argument(format!(
                        "{selector}: no verified enclosing declaration at line {line} in {}",
                        file.path
                    )));
                };
                let Some((first, last, _)) = declared_range(&lines, def) else {
                    return Err(ApiError::invalid_argument(format!(
                        "{selector}: could not resolve declaration range in {}",
                        file.path
                    )));
                };
                Ok((first, last, format!("{}::{}", file.path, def.qualified)))
            }
            Target::Symbol(symbol) => {
                let wanted = name_segments(symbol);
                if let Some(defs) = definitions.as_deref() {
                    let mut found = defs
                        .iter()
                        .filter(|def| def.name == symbol || def.qualified == symbol)
                        .collect::<Vec<_>>();
                    if found.is_empty() && !wanted.is_empty() {
                        found = defs.iter().filter(|def| def.answers_to(&wanted)).collect();
                    }
                    if found.iter().any(|def| is_scope_kind(def.kind)) {
                        found.retain(|def| is_scope_kind(def.kind));
                    }
                    if found.len() > 1 {
                        let spans = found
                            .iter()
                            .map(|def| format!("{}:{}-{}", file.path, def.start, def.end))
                            .collect::<Vec<_>>()
                            .join(", ");
                        return Err(ApiError::invalid_argument(format!(
                            "{selector}: ambiguous symbol {symbol} in {}; disambiguate with one of: {spans}",
                            file.path
                        )));
                    }
                    if let [def] = found.as_slice() {
                        let Some((first, last, _)) = declared_range(&lines, def) else {
                            return Err(ApiError::invalid_argument(format!(
                                "{selector}: could not resolve declaration range for {symbol}"
                            )));
                        };
                        return Ok((first, last, format!("{}::{}", file.path, def.qualified)));
                    }
                }
                if let Some((first, last)) = approximate_declaration(&lines, &wanted) {
                    Ok((first, last, format!("{}::{symbol}", file.path)))
                } else {
                    Err(ApiError::invalid_argument(format!(
                        "{selector}: no symbol {symbol} in {}",
                        file.path
                    )))
                }
            }
            Target::File => unreachable!(),
        }
    })?;

    let trimmed_replacement = replacement
        .strip_suffix("\r\n")
        .or_else(|| replacement.strip_suffix('\n'))
        .unwrap_or(replacement);
    let mut owned_replacement_lines: Vec<String> =
        if trimmed_replacement.is_empty() && !replacement.is_empty() {
            vec![String::new()]
        } else if trimmed_replacement.is_empty() {
            Vec::new()
        } else {
            trimmed_replacement.lines().map(str::to_string).collect()
        };

    // Preserve base indentation when replacing an indented declaration (e.g. a
    // class method) with code supplied at column 0.
    if let Some(orig_first) = lines.get((first as usize).saturating_sub(1)) {
        let base_indent_len = orig_first
            .bytes()
            .take_while(|b| *b == b' ' || *b == b'\t')
            .count();
        let base_indent = &orig_first[..base_indent_len];
        if !base_indent.is_empty()
            && owned_replacement_lines
                .iter()
                .filter(|line| !line.trim().is_empty())
                .all(|line| !line.starts_with(base_indent) && !line.starts_with([' ', '\t']))
        {
            for line in &mut owned_replacement_lines {
                if !line.trim().is_empty() {
                    *line = format!("{base_indent}{line}");
                }
            }
        }
    }
    let replacement_lines: Vec<&str> = owned_replacement_lines.iter().map(String::as_str).collect();

    let mut new_lines = Vec::with_capacity(
        lines
            .len()
            .saturating_sub((last + 1 - first) as usize)
            .saturating_add(replacement_lines.len()),
    );
    new_lines.extend_from_slice(&lines[..(first as usize - 1)]);
    new_lines.extend_from_slice(&replacement_lines);
    new_lines.extend_from_slice(&lines[(last as usize)..]);

    let mut new_text = new_lines.join(line_ending);
    if had_trailing_newline && !new_text.is_empty() {
        new_text.push_str(line_ending);
    }

    if let Some(ext) = file.absolute.extension().and_then(|e| e.to_str()) {
        let orig_valid = codestory_indexer::validate_source_syntax_for_ext(ext, &text).is_ok();
        if orig_valid
            && let Err((err_line, err_col)) =
                codestory_indexer::validate_source_syntax_for_ext(ext, &new_text)
        {
            return Err(ApiError::invalid_argument(format!(
                "syntax error after edit in {} at line {err_line}:{err_col} (file not modified)",
                file.path
            )));
        }
    }

    codestory_workspace::atomic_file::write_bytes_atomic(
        &file.absolute,
        "cs-edit",
        new_text.as_bytes(),
    )
    .map_err(|error| ApiError::internal(format!("failed to write {}: {error}", file.path)))?;

    let new_last = if replacement_lines.is_empty() {
        first.saturating_sub(1)
    } else {
        first + replacement_lines.len() as u32 - 1
    };
    Ok(format!(
        "updated {label} (lines {first}..{new_last}); syntax ok\n"
    ))
}

// ---------------------------------------------------------------------------
// slice: single-call multi-region + 1-hop + test/fixture context assembly
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SliceTerm {
    text: String,
    lower: String,
    case_sensitive: bool,
    is_path_hint: bool,
    boost: f64,
}

fn is_slice_stopword(lower: &str) -> bool {
    matches!(
        lower,
        "this"
            | "that"
            | "with"
            | "from"
            | "into"
            | "about"
            | "what"
            | "which"
            | "where"
            | "when"
            | "explain"
            | "show"
            | "find"
            | "read"
            | "source"
            | "please"
            | "before"
            | "after"
            | "without"
            | "change"
            | "changes"
            | "changing"
            | "want"
            | "need"
            | "should"
            | "could"
            | "would"
            | "does"
            | "have"
            | "only"
            | "both"
            | "each"
            | "every"
            | "same"
            | "existing"
            | "current"
            | "behavior"
            | "exactly"
            | "expected"
            | "exact"
            | "file"
            | "files"
            | "line"
            | "lines"
            | "code"
            | "give"
            | "list"
            | "count"
            | "name"
            | "type"
            | "value"
            | "values"
            | "case"
            | "cases"
            | "first"
            | "second"
            | "other"
            | "keep"
            | "used"
            | "uses"
            | "using"
            | "make"
            | "makes"
            | "made"
            | "touch"
            | "also"
            | "even"
            | "though"
            | "already"
            | "still"
            | "there"
            | "their"
            | "they"
            | "them"
            | "then"
            | "than"
            | "were"
            | "been"
            | "will"
            | "through"
            | "between"
            | "under"
            | "over"
            | "such"
            | "more"
            | "most"
            | "many"
            | "some"
            | "none"
            | "include"
            | "including"
            | "return"
            | "returns"
            | "returned"
            | "compare"
            | "assume"
            | "follow"
            | "identify"
            | "report"
            | "check"
            | "checks"
            | "another"
            | "several"
            | "once"
            | "goes"
            | "today"
            | "plain"
            | "normal"
            | "small"
            | "smaller"
            | "separate"
            | "avoid"
            | "proposed"
            | "answer"
            | "instead"
            | "comes"
            | "tries"
            | "getting"
            | "having"
            | "taking"
            | "doing"
            | "needed"
            | "outside"
            | "whole"
            | "level"
            | "levels"
            | "creates"
            | "created"
            | "points"
            | "leave"
            | "leaving"
            | "alone"
            | "actually"
            | "really"
            | "simply"
            | "trying"
            | "consider"
            | "considering"
            | "planning"
            | "reviewing"
            | "reach"
            | "reaches"
            | "reaching"
            | "distinguish"
            | "inside"
            | "enough"
            | "whether"
            | "different"
            | "differently"
            | "expect"
            | "thing"
            | "touching"
            | "telling"
    )
}

fn path_matches_term(lower_path: &str, term_lower: &str) -> bool {
    if term_lower.len() >= 4 && lower_path.contains(term_lower) {
        return true;
    }
    if term_lower.len() >= 3 {
        return lower_path
            .split(['/', '_', '-', '.'])
            .any(|seg| seg == term_lower);
    }
    false
}

fn split_camel_case(ident: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let chars: Vec<char> = ident.chars().collect();
    let mut start = 0usize;
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let curr = chars[i];
        let next = chars.get(i + 1).copied();
        let boundary = (prev.is_ascii_lowercase() && curr.is_ascii_uppercase())
            || (prev.is_ascii_uppercase()
                && curr.is_ascii_uppercase()
                && next.is_some_and(|n| n.is_ascii_lowercase()))
            || (prev.is_ascii_digit() != curr.is_ascii_digit());
        if boundary && i > start {
            parts.push(chars[start..i].iter().collect());
            start = i;
        }
    }
    if start < chars.len() {
        parts.push(chars[start..].iter().collect());
    }
    parts
}

fn stem_code_words(lower: &str) -> Vec<String> {
    let mut out = Vec::new();
    if lower.len() < 5 || !lower.chars().all(|c| c.is_ascii_alphabetic()) {
        return out;
    }
    if let Some(stem) = lower.strip_suffix("ations")
        && stem.len() >= 4
    {
        out.push(stem.to_string());
        return out;
    }
    if let Some(stem) = lower.strip_suffix("ies")
        && stem.len() >= 3
    {
        out.push(format!("{stem}y"));
        if stem.len() >= 4 {
            out.push(stem.to_string());
        }
        return out;
    }
    for suffix in [
        "ation", "ition", "ment", "ness", "able", "ible", "ings", "ing", "ion", "ers", "er", "ed",
        "es",
    ] {
        if let Some(stem) = lower.strip_suffix(suffix)
            && stem.len() >= 4
        {
            out.push(stem.to_string());
            // Collapse doubled trailing consonant after -ing / -ed / -er (e.g. "skipped" -> "skip").
            let b = stem.as_bytes();
            if b.len() >= 4 && b[b.len() - 1] == b[b.len() - 2] {
                out.push(stem[..stem.len() - 1].to_string());
            }
            return out;
        }
    }
    if let Some(stem) = lower.strip_suffix('s')
        && stem.len() >= 4
        && !lower.ends_with("ss")
        && !lower.ends_with("us")
        && !lower.ends_with("is")
    {
        out.push(stem.to_string());
    }
    out
}

fn is_code_extension(lower: &str) -> bool {
    matches!(
        lower.rsplit('.').next(),
        Some(
            "rs" | "go"
                | "py"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
                | "java"
                | "kt"
                | "swift"
                | "c"
                | "cc"
                | "cpp"
                | "h"
                | "hpp"
                | "cs"
                | "rb"
                | "php"
                | "scala"
                | "sh"
        )
    )
}

fn slice_path_tier(path: &str) -> u8 {
    let lower = path.to_ascii_lowercase();
    if path_rank_tier(path) == 2
        || lower.contains("zz_generated")
        || lower.contains("_generated.")
        || lower.contains("/generated/")
        || lower.starts_with("generated/")
        || lower.ends_with(".d.ts")
        || lower.ends_with(".gen.ts")
        || lower.ends_with(".gen.tsx")
        || lower.ends_with(".gen.go")
        || lower.ends_with("_gen.go")
        || lower.ends_with("_gen.ts")
        || lower.ends_with("_gen.tsx")
        || lower.ends_with(".pb.go")
        || lower.ends_with("_pb2.py")
        || lower.ends_with(".min.js")
        || lower.ends_with(".sql")
        || lower.ends_with(".toml")
    {
        2
    } else if codestory_workspace::cochange::is_test_or_fixture_path(&lower) {
        if is_code_extension(&lower)
            || lower.contains("/testdata/")
            || lower.contains("/specs/")
            || lower.contains("/spec/")
            || lower.contains(".golden.")
            || lower.contains("/fixtures/")
            || lower.ends_with(".out")
            || lower.ends_with(".snap")
            || lower.ends_with(".jsonc")
            || lower.ends_with(".json")
        {
            1
        } else {
            2
        }
    } else if is_code_extension(&lower) {
        0
    } else {
        2
    }
}

fn is_slice_skipped_asset(
    rel_lower: &str,
    file_len: u64,
    wants_tests: bool,
    terms: &[SliceTerm],
) -> bool {
    let explicitly_named = terms.iter().any(|t| {
        (t.is_path_hint || t.boost >= 2.0) && t.lower.len() >= 5 && rel_lower.contains(&t.lower)
    });
    if explicitly_named {
        return false;
    }
    if rel_lower.contains("zz_generated")
        || rel_lower.contains("_generated.")
        || rel_lower.contains("/generated/")
        || rel_lower.starts_with("generated/")
        || rel_lower.starts_with("vendor/")
        || rel_lower.contains("/vendor/")
        || rel_lower.contains("/node_modules/")
        || rel_lower.ends_with(".d.ts")
        || rel_lower.ends_with(".gen.ts")
        || rel_lower.ends_with(".gen.tsx")
        || rel_lower.ends_with(".gen.go")
        || rel_lower.ends_with("_gen.go")
        || rel_lower.ends_with("_gen.ts")
        || rel_lower.ends_with("_gen.tsx")
        || rel_lower.ends_with(".pb.go")
        || rel_lower.ends_with("_pb2.py")
        || rel_lower.ends_with(".min.js")
        || rel_lower.ends_with(".map")
        || rel_lower.ends_with(".lock")
        || rel_lower.ends_with(".svg")
    {
        return true;
    }
    let is_fixture = rel_lower.contains("fixture")
        || rel_lower.contains("/testdata/")
        || rel_lower.contains("/specs/")
        || rel_lower.contains("/spec/")
        || rel_lower.contains(".golden.");
    let is_data = rel_lower.ends_with(".json")
        || rel_lower.ends_with(".jsonc")
        || rel_lower.ends_with(".yaml")
        || rel_lower.ends_with(".yml")
        || rel_lower.ends_with(".out")
        || rel_lower.ends_with(".snap");
    if is_data && (file_len > 32 * 1024 || (!wants_tests && !is_fixture)) {
        return true;
    }
    false
}

fn extract_slice_terms(question: &str) -> (Vec<SliceTerm>, bool) {
    let lower_q = question.to_ascii_lowercase();
    let wants_tests = lower_q.contains("test")
        || lower_q.contains("fixture")
        || lower_q.contains("golden")
        || lower_q.contains("regression")
        || lower_q.contains("spec");
    let mut terms: Vec<SliceTerm> = Vec::new();
    let mut seen = HashSet::<String>::new();

    let push_one = |raw: &str,
                    case_sensitive: bool,
                    boost: f64,
                    is_path_hint: bool,
                    terms: &mut Vec<SliceTerm>,
                    seen: &mut HashSet<String>| {
        if raw.len() < 2 || raw.len() > 120 || raw.chars().all(|c| c.is_ascii_digit()) {
            return;
        }
        let lower = raw.to_ascii_lowercase();
        if !case_sensitive && is_slice_stopword(&lower) {
            return;
        }
        let key = format!("{case_sensitive}:{lower}");
        if !seen.insert(key) {
            return;
        }
        terms.push(SliceTerm {
            text: raw.to_string(),
            lower,
            case_sensitive,
            is_path_hint,
            boost,
        });
    };

    let mut push_term = |raw: &str, case_sensitive: bool, boost: f64| {
        let trimmed = raw
            .trim_matches(|c: char| {
                matches!(
                    c,
                    '`' | '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';' | ':' | '?' | '!'
                )
            })
            .trim_end_matches("()");
        if trimmed.len() < 2 || trimmed.len() > 120 || trimmed.chars().all(|c| c.is_ascii_digit()) {
            return;
        }
        let lower = trimmed.to_ascii_lowercase();
        let is_path_hint = trimmed.contains('/') && !trimmed.starts_with("--");
        push_one(
            trimmed,
            case_sensitive,
            boost,
            is_path_hint,
            &mut terms,
            &mut seen,
        );

        if is_path_hint {
            for seg in trimmed
                .split('/')
                .filter(|s| s.len() >= 4 && *s != "src" && *s != "pkg")
            {
                push_one(seg, false, boost * 0.85, false, &mut terms, &mut seen);
            }
        }

        if trimmed.starts_with("--") && trimmed.len() >= 4 {
            let unflagged = trimmed.trim_start_matches('-');
            push_one(unflagged, false, boost * 0.9, false, &mut terms, &mut seen);
            if unflagged.contains('-') {
                let snake = unflagged.replace('-', "_");
                push_one(&snake, false, boost * 0.95, false, &mut terms, &mut seen);
            }
        } else if !is_path_hint && trimmed.contains('-') {
            let snake = trimmed.replace('-', "_");
            push_one(&snake, false, boost * 1.15, false, &mut terms, &mut seen);
            let compact = lower.replace('-', "");
            if compact.len() >= 6 {
                push_one(&compact, false, boost * 1.05, false, &mut terms, &mut seen);
            }
            for part in trimmed.split('-') {
                if part.len() >= 3 {
                    push_one(part, false, boost * 0.75, false, &mut terms, &mut seen);
                    for stem in stem_code_words(&part.to_ascii_lowercase()) {
                        push_one(&stem, false, boost * 0.65, false, &mut terms, &mut seen);
                    }
                }
            }
        }

        if !is_path_hint && (trimmed.contains("::") || trimmed.contains('.')) {
            let is_file_ext = matches!(
                lower.rsplit('.').next(),
                Some(
                    "py" | "go"
                        | "rs"
                        | "ts"
                        | "tsx"
                        | "js"
                        | "jsx"
                        | "json"
                        | "jsonc"
                        | "yaml"
                        | "yml"
                        | "toml"
                )
            );
            if !is_file_ext {
                for seg in name_segments(trimmed) {
                    if seg.len() >= 3 {
                        push_one(seg, true, boost * 0.95, false, &mut terms, &mut seen);
                        push_one(
                            &seg.to_ascii_lowercase(),
                            false,
                            boost * 0.7,
                            false,
                            &mut terms,
                            &mut seen,
                        );
                    }
                }
            }
        }

        let parts = split_camel_case(trimmed);
        if parts.len() >= 2 && !trimmed.contains(['_', '/', '.']) {
            // Match exported/unexported case variants (e.g. `disableDeletion` -> `DisableDeletion` & `disable_deletion`).
            push_one(&lower, false, boost * 0.78, false, &mut terms, &mut seen);
            let snake = parts
                .iter()
                .map(|p| p.to_ascii_lowercase())
                .collect::<Vec<_>>()
                .join("_");
            push_one(&snake, false, boost * 0.85, false, &mut terms, &mut seen);
            for part in parts {
                if part.len() >= 3 {
                    push_one(
                        &part.to_ascii_lowercase(),
                        false,
                        boost * 0.55,
                        false,
                        &mut terms,
                        &mut seen,
                    );
                }
            }
        }

        if trimmed.contains('_') && !is_path_hint {
            let compact = lower.replace('_', "");
            if compact.len() >= 6 {
                push_one(&compact, false, boost * 0.85, false, &mut terms, &mut seen);
            }
            for part in trimmed.split('_') {
                if part.len() >= 3 {
                    push_one(
                        &part.to_ascii_lowercase(),
                        false,
                        boost * 0.6,
                        false,
                        &mut terms,
                        &mut seen,
                    );
                }
            }
        }

        for stem in stem_code_words(&lower) {
            push_one(&stem, false, boost * 0.75, false, &mut terms, &mut seen);
        }
    };

    // 1. Inline backticked and double-quoted spans first (highest precision).
    let bytes = question.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' || bytes[i] == b'"' {
            let delim = bytes[i];
            if let Some(rel_end) = bytes[i + 1..].iter().position(|&b| b == delim) {
                let atom = &question[i + 1..i + 1 + rel_end];
                if !atom.contains('\n') && atom.len() >= 2 && atom.len() <= 100 {
                    for part in atom.split_whitespace() {
                        let is_code = part.contains(['_', '.', ':', '/', '-'])
                            || part.chars().any(|c| c.is_ascii_uppercase());
                        push_term(part, is_code, 2.2);
                    }
                }
                i += rel_end + 2;
                continue;
            }
        }
        i += 1;
    }

    // 2. Unquoted code-shaped tokens, flags, paths, and domain nouns.
    for raw_tok in question.split_whitespace() {
        let tok = raw_tok
            .trim_matches(|c: char| {
                matches!(
                    c,
                    '`' | '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';' | ':' | '?' | '!' | '.'
                )
            })
            .trim_end_matches("()");
        if tok.len() < 3 {
            continue;
        }
        let has_upper = tok.chars().any(|c| c.is_ascii_uppercase());
        let has_lower = tok.chars().any(|c| c.is_ascii_lowercase());
        let is_flag = tok.starts_with("--") && tok.len() >= 4;
        let is_path = tok.contains('/') && !tok.starts_with("http");
        let is_snake_or_qualified = tok.contains('_') || tok.contains("::");
        let is_camel = has_upper && has_lower && tok[1..].chars().any(|c| c.is_ascii_uppercase());
        let is_acronym = tok.len() >= 3
            && tok
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        if is_flag || is_path || is_snake_or_qualified || is_camel || is_acronym {
            push_term(tok, true, 1.8);
        } else if (tok.len() >= 4
            || (tok.len() == 3 && !is_slice_stopword(&tok.to_ascii_lowercase())))
            && tok.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            let boost = if tok.len() == 3 {
                0.75
            } else if has_upper && !question.starts_with(raw_tok) {
                1.2
            } else {
                0.85
            };
            push_term(tok, false, boost);
        }
    }

    terms.sort_by(|a, b| b.boost.total_cmp(&a.boost));
    terms.truncate(48);
    (terms, wants_tests)
}

fn declared_ident_on_line(line: &str) -> Option<&str> {
    let mut rest = line.trim_start();
    let mut declares = false;
    loop {
        let end = rest
            .find(|ch| !is_declaration_char(ch))
            .unwrap_or(rest.len());
        let word = &rest[..end];
        if !is_declaration_word(word) {
            return (declares && !word.is_empty()).then_some(word);
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

fn find_slice_term_lines(text: &str, terms: &[SliceTerm]) -> (Vec<u32>, Vec<u32>, u32, usize) {
    let mut hits_per_term = vec![0u32; terms.len()];
    let lower_text = text.to_ascii_lowercase();
    let mut active = Vec::new();
    for (idx, term) in terms.iter().enumerate() {
        let present = if term.case_sensitive {
            text.contains(&term.text)
        } else {
            lower_text.contains(&term.lower)
        };
        if present {
            active.push(idx);
        }
    }
    let mut line_count = 0usize;
    if active.is_empty() {
        line_count = text.lines().count();
        return (hits_per_term, Vec::new(), 0, line_count);
    }

    let mut exact_decl_hits = 0u32;
    let mut scored_lines: Vec<(f64, u32)> = Vec::new();
    for (line_idx, line) in text.lines().enumerate() {
        line_count = line_idx + 1;
        let trimmed = line.trim_start();
        let is_comment =
            trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*');
        let mut line_score = 0.0_f64;
        let mut lower_line: Option<String> = None;
        let decl_ident = if !is_comment {
            declared_ident_on_line(line)
        } else {
            None
        };
        let mut exact_decl_on_line = false;
        let mut decl_hit = false;
        for &idx in &active {
            let term = &terms[idx];
            let matched = if term.case_sensitive {
                line.contains(&term.text)
            } else {
                lower_line
                    .get_or_insert_with(|| line.to_ascii_lowercase())
                    .contains(&term.lower)
            };
            if matched {
                hits_per_term[idx] =
                    hits_per_term[idx].saturating_add(if is_comment { 0 } else { 1 });
                let w = if is_comment {
                    term.boost * 0.25
                } else {
                    term.boost
                };
                line_score += w;
                if let Some(dident) = decl_ident {
                    if term.boost >= 1.5
                        && (dident == term.text
                            || dident.eq_ignore_ascii_case(&term.lower)
                            || (term.lower.len() >= 6
                                && dident.to_ascii_lowercase().contains(&term.lower)))
                    {
                        exact_decl_on_line = true;
                    } else if term.boost >= 1.2 {
                        decl_hit = true;
                    }
                }
            }
        }
        // Still record pure-comment file hits only if code didn't match at all.
        if is_comment && line_score > 0.0 {
            for &idx in &active {
                if hits_per_term[idx] == 0 {
                    let term = &terms[idx];
                    let matched = if term.case_sensitive {
                        line.contains(&term.text)
                    } else {
                        lower_line
                            .as_deref()
                            .is_some_and(|l| l.contains(&term.lower))
                    };
                    if matched {
                        hits_per_term[idx] = 1;
                    }
                }
            }
        }
        if line_score > 0.0 {
            if exact_decl_on_line {
                exact_decl_hits = exact_decl_hits.saturating_add(1);
                line_score += 8.0;
            } else if decl_hit {
                line_score += 2.5;
            }
            scored_lines.push((line_score, line_idx as u32 + 1));
        }
    }

    scored_lines.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored_lines.truncate(160);
    let mut hit_lines = scored_lines
        .into_iter()
        .map(|(_, ln)| ln)
        .collect::<Vec<_>>();
    hit_lines.sort_unstable();
    (hits_per_term, hit_lines, exact_decl_hits, line_count)
}

struct SliceFileHit {
    path: String,
    term_hits: Vec<u32>,
    hit_lines: Vec<u32>,
    exact_decl_hits: u32,
    line_count: usize,
    score: f64,
}

struct SliceCandidateBlock {
    path: String,
    first: u32,
    last: u32,
    title: String,
    rendered: String,
    bytes: usize,
    term_mask: Vec<bool>,
    base_score: f64,
    file_score: f64,
    tier: u8,
    hop_bonus: f64,
    def_id: Option<NodeId>,
    is_whole_small_file: bool,
}

fn leading_doc_comment_start(lines: &[&str], start_line: u32) -> u32 {
    let mut s = start_line;
    for step in 1..=3u32 {
        let prev = start_line.saturating_sub(step);
        if prev == 0 {
            break;
        }
        let t = lines[prev as usize - 1].trim_start();
        if t.starts_with("//") || t.starts_with('#') {
            s = prev;
        } else {
            break;
        }
    }
    s
}

fn enclosing_live_range(lines: &[&str], hit_line: u32, max_lines: u32) -> (u32, u32) {
    let count = lines.len() as u32;
    if count <= max_lines {
        return (1, count);
    }
    let min_lines = 12.min(count);
    let idx = (hit_line as usize)
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1));
    let hit_indent = indent(lines[idx]);
    // Search upward up to 60 lines for the nearest enclosing declaration header.
    let min_up = idx.saturating_sub(60);
    let mut header = None;
    for up in (min_up..=idx).rev() {
        let trimmed = lines[up].trim();
        if trimmed.is_empty() {
            continue;
        }
        let up_indent = indent(lines[up]);
        if up_indent <= hit_indent && starts_declaration(lines[up]) {
            header = Some(up);
            break;
        }
    }
    if let Some(start_idx) = header {
        let end_idx = block_end(lines, start_idx).min(start_idx + max_lines as usize - 1);
        let first = start_idx as u32 + 1;
        let last = (end_idx as u32 + 1).max(hit_line).min(count);
        if (last + 1 - first) >= min_lines && (last + 1 - first) <= max_lines && last >= hit_line {
            return (first, last);
        }
        if (last + 1 - first) < min_lines {
            let ext_first = first.min(hit_line.saturating_sub(10)).max(1);
            let ext_last = (ext_first + min_lines - 1).min(count);
            return (ext_first, ext_last.max(hit_line));
        }
        let w = window_around(hit_line, max_lines / 2, first, count);
        return (w.0, w.1.min(w.0 + max_lines - 1));
    }
    let radius = (max_lines / 2).min(18).max(min_lines / 2);
    window_around(hit_line, radius, 1, count)
}

fn render_slice_block(path: &str, lines: &[&str], first: u32, last: u32, title: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "== {path}:{first}-{last}{title}");
    let first_u = (first as usize).max(1);
    let last_u = (last as usize).min(lines.len());
    for number in first_u..=last_u {
        let _ = write!(out, "{number}:");
        let code = bounded(lines[number - 1].trim_end());
        if !code.is_empty() {
            let _ = write!(out, " {code}");
        }
        out.push('\n');
    }
    out
}

struct LiveAstSliceFacts {
    definitions: Vec<Definition>,
    same_file_edges: HashMap<NodeId, Vec<NodeId>>,
}

const MAX_LIVE_AST_BYTES: usize = 256 * 1024;

fn live_ast_slice_facts(rel_path: &str, text: &str) -> Option<LiveAstSliceFacts> {
    if text.len() > MAX_LIVE_AST_BYTES {
        return None;
    }
    let ext = Path::new(rel_path).extension().and_then(|e| e.to_str())?;
    let lang_cfg = codestory_indexer::get_language_for_ext(ext)?;
    let result =
        codestory_indexer::index_file(Path::new(rel_path), text, &lang_cfg, None, None).ok()?;
    let mut definitions = result
        .nodes
        .iter()
        .filter_map(|node| {
            let (start, end) = (node.start_line?, node.end_line?);
            let keep = !matches!(
                node.kind,
                NodeKind::UNKNOWN
                    | NodeKind::FILE
                    | NodeKind::BUILTIN_TYPE
                    | NodeKind::TYPE_PARAMETER
                    | NodeKind::VARIABLE
            );
            (keep && start >= 1 && end >= start).then(|| Definition {
                id: node.id,
                kind: node.kind,
                qualified: node
                    .qualified_name
                    .clone()
                    .unwrap_or_else(|| node.serialized_name.clone()),
                name: node.serialized_name.clone(),
                start,
                end,
            })
        })
        .collect::<Vec<_>>();
    definitions.sort_by(|left, right| {
        (left.start, Reverse(left.end), &left.name).cmp(&(
            right.start,
            Reverse(right.end),
            &right.name,
        ))
    });
    let def_ids: HashSet<NodeId> = definitions.iter().map(|d| d.id).collect();
    let mut same_file_edges: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for edge in &result.edges {
        if !matches!(
            edge.kind,
            EdgeKind::CALL
                | EdgeKind::USAGE
                | EdgeKind::TYPE_USAGE
                | EdgeKind::INHERITANCE
                | EdgeKind::OVERRIDE
        ) {
            continue;
        }
        let src = edge.effective_source();
        let dst = edge.effective_target();
        if src != dst && def_ids.contains(&src) && def_ids.contains(&dst) {
            same_file_edges.entry(src).or_default().push(dst);
        }
    }
    Some(LiveAstSliceFacts {
        definitions,
        same_file_edges,
    })
}

fn line_is_non_code_flags(lines: &[&str]) -> Vec<bool> {
    let mut flags = Vec::with_capacity(lines.len());
    let mut in_triple_dq = false;
    let mut in_triple_sq = false;
    let mut in_block_comment = false;
    for &line in lines {
        let trimmed = line.trim();
        let mut non_code = trimmed.is_empty();
        if in_block_comment {
            non_code = true;
            if trimmed.contains("*/") {
                in_block_comment = false;
            }
        } else if in_triple_dq {
            non_code = true;
            if trimmed.contains("\"\"\"") {
                in_triple_dq = false;
            }
        } else if in_triple_sq {
            non_code = true;
            if trimmed.contains("'''") {
                in_triple_sq = false;
            }
        } else if trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || trimmed.starts_with('*')
            || trimmed.starts_with("import ")
            || trimmed.starts_with("from ")
            || trimmed.starts_with("use ")
            || trimmed.starts_with("package ")
        {
            non_code = true;
        } else if trimmed.starts_with("/*") {
            non_code = true;
            if !trimmed.contains("*/") {
                in_block_comment = true;
            }
        } else if let Some(rest) = trimmed.strip_prefix("\"\"\"") {
            non_code = true;
            if !rest.contains("\"\"\"") {
                in_triple_dq = true;
            }
        } else if let Some(rest) = trimmed.strip_prefix("'''") {
            non_code = true;
            if !rest.contains("'''") {
                in_triple_sq = true;
            }
        }
        flags.push(non_code);
    }
    flags
}

fn extract_cross_file_hints(
    blocks: &[&SliceCandidateBlock],
    seed_samples: &[(&str, &str)],
    all_paths: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut path_hints = Vec::new();
    let mut symbol_counts: HashMap<String, usize> = HashMap::new();
    let mut seen_paths = HashSet::new();
    let path_set: HashSet<&str> = all_paths.iter().map(String::as_str).collect();

    let mut push_path_hint = |raw: &str, base_dir: Option<&str>| {
        let rel = raw
            .trim()
            .trim_start_matches("./")
            .trim_start_matches("../");
        if rel.len() < 4 || rel.contains("://") {
            return;
        }
        if let Some(dir) = base_dir
            && !dir.is_empty()
        {
            let joined = format!("{dir}/{rel}");
            if path_set.contains(joined.as_str()) && seen_paths.insert(joined.clone()) {
                path_hints.push(joined);
                return;
            }
        }
        if path_set.contains(rel) && seen_paths.insert(rel.to_string()) {
            path_hints.push(rel.to_string());
            return;
        }
        // Strip external module prefixes (e.g. `github.com/org/repo/pkg/foo/bar` -> `pkg/foo/bar`).
        let parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() >= 3 && (parts[0].contains('.') || parts.len() >= 4) {
            let tail3 = parts[parts.len().saturating_sub(3)..].join("/");
            if tail3.len() >= 5 && seen_paths.insert(tail3.clone()) {
                path_hints.push(tail3);
            }
            let tail2 = parts[parts.len() - 2..].join("/");
            if tail2.len() >= 5 && seen_paths.insert(tail2.clone()) {
                path_hints.push(tail2);
            }
        } else if seen_paths.insert(rel.to_string()) {
            path_hints.push(rel.to_string());
        }
    };

    for &(rel_path, sample) in seed_samples {
        let base_dir = rel_path.rsplit_once('/').map(|(d, _)| d);
        for line in sample.lines().take(160) {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("from ")
                && let Some((mod_part, _)) = rest.split_once(" import ")
            {
                let rel = mod_part.trim().trim_start_matches('.').replace('.', "/");
                push_path_hint(&rel, None);
            } else if let Some(rest) = trimmed.strip_prefix("use ") {
                let cleaned = rest
                    .trim_start_matches("crate::")
                    .trim_start_matches("super::")
                    .split(['{', ';'])
                    .next()
                    .unwrap_or("")
                    .trim_end_matches("::")
                    .replace("::", "/");
                if let Some((parent, _)) = cleaned.rsplit_once('/') {
                    push_path_hint(parent, None);
                }
                push_path_hint(&cleaned, None);
            } else if let Some(rest) = trimmed
                .strip_prefix("mod ")
                .or_else(|| trimmed.strip_prefix("pub mod "))
                && let Some(mod_name) = rest.strip_suffix(';')
                && !mod_name.contains([' ', '{'])
            {
                push_path_hint(mod_name, base_dir);
            } else if (trimmed.starts_with("import ")
                || trimmed.starts_with('"')
                || trimmed.starts_with("export "))
                && let Some((_, quoted)) = trimmed.split_once('"')
                && let Some((pkg, _)) = quoted.split_once('"')
                && pkg.contains('/')
            {
                push_path_hint(pkg, base_dir);
            }
        }
    }

    for blk in blocks {
        let blk_dir = blk.path.rsplit_once('/').map(|(d, _)| d);
        for line in blk.rendered.lines().skip(1) {
            let code = line.split_once(':').map(|(_, rest)| rest).unwrap_or(line);
            // Follow explicit quoted fixture/file path strings inside code blocks.
            for delim in ['"', '\''] {
                let mut rest = code;
                while let Some((_, after)) = rest.split_once(delim) {
                    if let Some((lit, tail)) = after.split_once(delim) {
                        if (lit.contains('/') || lit.contains('.'))
                            && lit.len() >= 5
                            && lit.len() <= 120
                        {
                            push_path_hint(lit, blk_dir);
                        }
                        rest = tail;
                    } else {
                        break;
                    }
                }
            }
            let bytes = code.as_bytes();
            let mut i = 0usize;
            while i < bytes.len() {
                if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
                    let start = i;
                    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    let ident = &code[start..i];
                    let followed_by_call = code[i..].trim_start().starts_with('(');
                    let is_camel = ident.len() >= 6
                        && ident.bytes().next().is_some_and(|b| b.is_ascii_uppercase())
                        && ident.bytes().any(|b| b.is_ascii_lowercase());
                    let is_snake =
                        ident.len() >= 6 && ident.contains('_') && !ident.starts_with("__");
                    if ident.len() >= 5
                        && (followed_by_call || is_camel || is_snake)
                        && !is_slice_stopword(&ident.to_ascii_lowercase())
                        && !matches!(
                            ident,
                            "assert"
                                | "format"
                                | "println"
                                | "eprintln"
                                | "super"
                                | "String"
                                | "Option"
                                | "Result"
                                | "append"
                                | "extend"
                                | "clone"
                                | "unwrap"
                                | "filter"
                                | "sorted"
                                | "return"
                        )
                    {
                        *symbol_counts.entry(ident.to_string()).or_insert(0) +=
                            if followed_by_call { 2 } else { 1 };
                    }
                } else {
                    i += 1;
                }
            }
        }
    }
    let mut ranked_syms: Vec<(String, usize)> = symbol_counts.into_iter().collect();
    ranked_syms.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let symbols = ranked_syms.into_iter().map(|(s, _)| s).take(8).collect();
    (path_hints, symbols)
}

struct LiveHopQuery<'a> {
    root: &'a Path,
    all_paths: &'a [String],
    seed_set: &'a HashSet<&'a str>,
    path_hints: &'a [String],
    callee_symbols: &'a [String],
    terms: &'a [SliceTerm],
    wants_tests: bool,
    allow_disk_scan: bool,
    limit: usize,
}

fn discover_live_hop_paths(query: &LiveHopQuery<'_>) -> Vec<(String, String, bool)> {
    let LiveHopQuery {
        root,
        all_paths,
        seed_set,
        path_hints,
        callee_symbols,
        terms,
        wants_tests,
        allow_disk_scan,
        limit,
    } = *query;
    let mut found: Vec<(String, String, bool)> = Vec::new();
    let mut seen_found = HashSet::new();

    // Rank path hints so explicit fixtures/testdata and query-term-matching modules beat generic imports.
    let mut scored_hints: Vec<(i32, &String)> = path_hints
        .iter()
        .map(|h| {
            let hl = h.to_ascii_lowercase();
            let is_fix = hl.contains("/testdata/")
                || hl.contains("/fixtures/")
                || hl.contains("/specs/")
                || hl.contains(".golden.")
                || hl.ends_with(".json")
                || hl.ends_with(".jsonc")
                || hl.ends_with(".out");
            let mut s: i32 = if is_fix { 50 } else { 0 };
            for t in terms {
                if path_matches_term(&hl, &t.lower) {
                    s += if t.boost >= 1.5 { 20 } else { 10 };
                }
            }
            (s, h)
        })
        .collect();
    scored_hints.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));

    // 1. Direct path tail / quoted fixture matching for imported modules (zero full-tree disk read).
    let import_cap = (limit / 2).max(3);
    for (_, hint) in scored_hints {
        if found.len() >= import_cap {
            break;
        }
        let allow_fixture = hint.contains("/testdata/")
            || hint.contains("/fixtures/")
            || hint.contains("/specs/")
            || hint.contains(".golden.")
            || hint.ends_with(".json")
            || hint.ends_with(".jsonc")
            || hint.ends_with(".out");
        let needle_file = format!("/{hint}.");
        let needle_dir = format!("/{hint}/");
        for p in all_paths {
            if seed_set.contains(p.as_str()) {
                continue;
            }
            let tier = slice_path_tier(p);
            if !(tier == 0 || allow_fixture || (wants_tests && tier == 1)) {
                continue;
            }
            if (p == hint
                || p.ends_with(&format!("/{hint}"))
                || p.contains(&needle_file)
                || p.contains(&needle_dir)
                || p.starts_with(hint.as_str()))
                && seen_found.insert(p.clone())
            {
                found.push((p.clone(), String::new(), allow_fixture));
                if found.len() >= import_cap {
                    break;
                }
            }
        }
    }

    if !allow_disk_scan || found.len() >= limit || callee_symbols.is_empty() {
        return found;
    }

    // Precompute declaration patterns ONCE before parallel scan to avoid per-file allocations.
    let compiled_patterns: Vec<(usize, &str, [String; 10])> = callee_symbols
        .iter()
        .take(5)
        .enumerate()
        .map(|(rank, sym)| {
            (
                rank,
                sym.as_str(),
                [
                    format!("def {sym}("),
                    format!("fn {sym}"),
                    format!("func {sym}"),
                    format!(") {sym}("),
                    format!("\t{sym}("),
                    format!("  {sym}("),
                    format!("class {sym}"),
                    format!("struct {sym}"),
                    format!("interface {sym}"),
                    format!("function {sym}"),
                ],
            )
        })
        .collect();

    // 2. Fast parallel definition check across remaining tier-0 source files for top callees/types.
    let candidates: Vec<(&String, usize)> = all_paths
        .par_iter()
        .filter(|p| {
            !seed_set.contains(p.as_str())
                && !seen_found.contains(p.as_str())
                && slice_path_tier(p) == 0
        })
        .map_init(Vec::new, |bytes, p| {
            let abs = root.join(p);
            let meta = std::fs::metadata(&abs).ok()?;
            if !meta.is_file() || meta.len() > MAX_LIVE_AST_BYTES as u64 {
                return None;
            }
            if !read_live_text(&abs, bytes) {
                return None;
            }
            let text = std::str::from_utf8(bytes).ok()?;
            let mut matched_idx = None;
            for &(rank, sym, ref patterns) in &compiled_patterns {
                if text.contains(sym) && patterns.iter().any(|pat| text.contains(pat.as_str())) {
                    matched_idx = Some(rank);
                    break;
                }
            }
            matched_idx.map(|r| (p, r))
        })
        .flatten()
        .collect();

    let mut sorted = candidates;
    sorted.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
    for (p, rank) in sorted {
        if found.len() >= limit {
            break;
        }
        if seen_found.insert(p.clone()) {
            found.push((p.clone(), callee_symbols[rank].clone(), false));
        }
    }
    found
}

/// Assemble a multi-region, 1-hop, test- and fixture-aware source context
/// slice for a natural-language question or search query inside `max_bytes`.
pub fn slice(
    root: &Path,
    storage_path: &Path,
    question: &str,
    max_bytes: Option<usize>,
) -> Result<String, ApiError> {
    if question.trim().is_empty() {
        return Err(ApiError::invalid_argument(
            "slice needs a non-empty question",
        ));
    }
    let budget_bytes = resolve_max_bytes(max_bytes, true).unwrap_or(DEFAULT_MAX_BYTES);
    let (terms, wants_tests) = extract_slice_terms(question);
    if terms.is_empty() {
        return Err(ApiError::invalid_argument(
            "slice could not extract any searchable terms from the question",
        ));
    }

    let paths = live_walk(root, &[])?.collect::<Vec<_>>();
    let total_files = paths.len().max(1) as f64;
    let all_paths = paths
        .iter()
        .map(|p| project_path(root, p))
        .collect::<Vec<_>>();

    let mut files = paths
        .par_iter()
        .map_init(Vec::new, |bytes, path| {
            let rel = project_path(root, path);
            let rel_lower = rel.to_ascii_lowercase();
            let file_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if is_slice_skipped_asset(&rel_lower, file_len, wants_tests, &terms) {
                return None;
            }
            if !read_live_text(path, bytes) {
                return None;
            }
            let text = std::str::from_utf8(bytes).ok()?;
            let path_matched = terms
                .iter()
                .any(|t| t.is_path_hint && rel_lower.contains(&t.lower));
            let (mut term_hits, mut hit_lines, exact_decl_hits, line_count) =
                find_slice_term_lines(text, &terms);
            if hit_lines.is_empty() && !path_matched {
                return None;
            }
            if hit_lines.is_empty() && path_matched {
                for (idx, t) in terms.iter().enumerate() {
                    if t.is_path_hint && rel_lower.contains(&t.lower) {
                        term_hits[idx] = 1;
                    }
                }
                hit_lines.push(1);
            }
            Some(SliceFileHit {
                path: rel,
                term_hits,
                hit_lines,
                exact_decl_hits,
                line_count,
                score: 0.0,
            })
        })
        .flatten()
        .collect::<Vec<_>>();

    let mut df = vec![0usize; terms.len()];
    let mut total_matched_lines = 0usize;
    for f in &files {
        total_matched_lines += f.line_count;
        for (idx, &cnt) in f.term_hits.iter().enumerate() {
            if cnt > 0 {
                df[idx] += 1;
            }
        }
    }
    let avg_lines = (total_matched_lines as f64 / files.len().max(1) as f64).clamp(220.0, 850.0);
    let idfs: Vec<f64> = df
        .iter()
        .map(|&n| ((total_files - n as f64 + 0.5) / (n as f64 + 0.5) + 1.0).ln())
        .collect();

    let path_prefixes: Vec<String> = terms
        .iter()
        .filter(|t| t.is_path_hint && t.lower.len() >= 4)
        .map(|t| t.lower.trim_end_matches('/').to_string())
        .collect();
    let scope_prefixes_active = !path_prefixes.is_empty()
        && files
            .iter()
            .filter(|f| {
                let lp = f.path.to_ascii_lowercase();
                path_prefixes
                    .iter()
                    .any(|p| lp.starts_with(p) || lp.contains(&format!("/{p}/")))
            })
            .count()
            >= 2;

    for f in &mut files {
        let lower_path = f.path.to_ascii_lowercase();
        let len_norm = (0.45 + 0.55 * (f.line_count as f64 / avg_lines)).clamp(0.6, 3.0);
        let mut score = 0.0_f64;
        let mut distinct_strong = 0usize;
        let mut distinct_matched = 0usize;
        for (idx, (&cnt, term)) in f.term_hits.iter().zip(&terms).enumerate() {
            if cnt > 0 {
                let tf = cnt as f64;
                let bm25_tf = (2.2 * tf) / (tf + 1.2 * len_norm);
                score += idfs[idx] * term.boost * bm25_tf;
                distinct_matched += 1;
                if term.boost >= 1.5 {
                    distinct_strong += 1;
                }
            }
            if path_matches_term(&lower_path, &term.lower) {
                score += idfs[idx] * if term.is_path_hint { 3.5 } else { 1.9 };
            }
        }
        if distinct_matched >= 3 {
            score *= 1.0 + 0.14 * (distinct_matched.min(8) as f64);
        }
        if distinct_strong >= 2 {
            score *= 1.0 + 0.25 * (distinct_strong.min(4) as f64);
        }
        if f.exact_decl_hits > 0 {
            score += 8.0 * (f.exact_decl_hits.min(3) as f64);
        }
        if scope_prefixes_active {
            let in_scope = path_prefixes
                .iter()
                .any(|p| lower_path.starts_with(p) || lower_path.contains(&format!("/{p}/")));
            if in_scope {
                score *= 2.5;
            } else if f.exact_decl_hits == 0 {
                score *= 0.38;
            }
        }
        match slice_path_tier(&f.path) {
            0 => score *= if wants_tests { 1.15 } else { 1.45 },
            1 => score *= if wants_tests { 1.30 } else { 0.32 },
            _ => score *= if wants_tests { 0.16 } else { 0.10 },
        }
        f.score = score;
    }

    files.sort_unstable_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.cmp(&b.path))
    });

    // Apply primary language and module-subtree cohesion from top-ranked files
    // so polyglot repositories do not pollute slices with unrelated UI/polyfill noise.
    let explicit_ts_ui = terms.iter().any(|t| {
        matches!(
            t.lower.as_str(),
            "tsx" | "jsx" | "react" | "component" | "frontend" | "browser" | "dom" | "css" | "html"
        )
    });
    let anchor = if wants_tests {
        files.iter().find(|f| slice_path_tier(&f.path) <= 1)
    } else {
        files.iter().find(|f| slice_path_tier(&f.path) == 0)
    };
    if let Some(top_anchor) = anchor {
        let top_ext = top_anchor
            .path
            .rsplit('.')
            .next()
            .map(str::to_ascii_lowercase);
        let top_dir = top_anchor.path.rsplit_once('/').map(|(d, _)| d.to_string());
        let top_pkg = top_anchor
            .path
            .split('/')
            .take(2)
            .collect::<Vec<_>>()
            .join("/");
        for f in &mut files {
            let f_ext = f.path.rsplit('.').next().map(str::to_ascii_lowercase);
            let same_dir = top_dir.as_ref().is_some_and(|d| {
                f.path.starts_with(d) && f.path.as_bytes().get(d.len()) == Some(&b'/')
            });
            if same_dir {
                f.score *= if wants_tests { 1.45 } else { 1.35 };
            } else if !top_pkg.is_empty() && f.path.starts_with(&top_pkg) {
                f.score *= 1.15;
            }
            if !wants_tests
                && !explicit_ts_ui
                && matches!(top_ext.as_deref(), Some("go" | "rs" | "py"))
                && matches!(f_ext.as_deref(), Some("ts" | "tsx" | "js" | "jsx"))
            {
                f.score *= 0.45;
            }
        }
        files.sort_unstable_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.path.cmp(&b.path))
        });
    }

    // Keep top exact-declaring files + submodular term-diverse seed files (so multi-part prompts hit all subsystems).
    let extra_str: Vec<&str> = terms
        .iter()
        .filter(|t| t.boost >= 1.2)
        .map(|t| t.text.as_str())
        .collect();
    let top_score = files.first().map_or(0.0, |f| f.score);
    let mut seed_paths: Vec<String> = Vec::new();
    let mut seed_cov = vec![0u32; terms.len()];
    let record_seed = |f: &SliceFileHit, seed_paths: &mut Vec<String>, seed_cov: &mut [u32]| {
        seed_paths.push(f.path.clone());
        for (idx, &cnt) in f.term_hits.iter().enumerate() {
            if cnt > 0 {
                seed_cov[idx] += 1;
            }
        }
    };
    for f in files
        .iter()
        .filter(|f| f.exact_decl_hits > 0 && slice_path_tier(&f.path) <= u8::from(wants_tests))
        .take(5)
    {
        if f.score >= top_score * 0.14 && !seed_paths.contains(&f.path) {
            record_seed(f, &mut seed_paths, &mut seed_cov);
        }
    }
    let seed_target = 12usize;
    while seed_paths.len() < seed_target {
        let mut best_pos: Option<usize> = None;
        let mut best_val = 0.0_f64;
        for (i, f) in files.iter().enumerate().take(60) {
            let tier = slice_path_tier(&f.path);
            if !wants_tests && tier != 0 {
                continue;
            }
            if tier == 2 || seed_paths.contains(&f.path) {
                continue;
            }
            let mut marginal = f.score * 0.65;
            for (t_idx, (&cnt, term)) in f.term_hits.iter().zip(&terms).enumerate() {
                if cnt > 0 {
                    let decay = 0.45_f64.powi(seed_cov[t_idx] as i32);
                    marginal += idfs[t_idx] * term.boost * decay * 3.5;
                }
            }
            if tier == 0 {
                marginal *= if wants_tests { 1.10 } else { 1.35 };
            } else {
                marginal *= if wants_tests { 1.25 } else { 0.40 };
            }
            if marginal > best_val {
                best_val = marginal;
                best_pos = Some(i);
            }
        }
        let Some(pos) = best_pos else {
            break;
        };
        record_seed(&files[pos], &mut seed_paths, &mut seed_cov);
    }
    // Pull small companion files inside already-selected leaf fixture/spec directories
    // (e.g. `tests/specs/.../<case>/{main.ts,cov.out,__test__.jsonc}` or `/testdata/*`).
    let existing_seeds = seed_paths.clone();
    for s_path in &existing_seeds {
        if seed_paths.len() >= 14 {
            break;
        }
        let sl = s_path.to_ascii_lowercase();
        if !(sl.contains("/specs/")
            || sl.contains("/testdata/")
            || sl.contains("/fixtures/")
            || sl.contains(".golden."))
        {
            continue;
        }
        if let Some((dir, _)) = s_path.rsplit_once('/') {
            let prefix = format!("{dir}/");
            let mut added_sibs = 0usize;
            for p in &all_paths {
                if added_sibs >= 3 || seed_paths.len() >= 14 {
                    break;
                }
                if p.starts_with(&prefix)
                    && !p[prefix.len()..].contains('/')
                    && !seed_paths.contains(p)
                {
                    let abs = root.join(p);
                    let meta_len = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(u64::MAX);
                    if meta_len > 0
                        && meta_len <= 16 * 1024
                        && !is_slice_skipped_asset(&p.to_ascii_lowercase(), meta_len, true, &terms)
                    {
                        seed_paths.push(p.clone());
                        added_sibs += 1;
                    }
                }
            }
        }
    }
    let mut coupled_test_paths = Vec::new();
    for prod in files
        .iter()
        .filter(|f| slice_path_tier(&f.path) == 0)
        .take(2)
    {
        for rel in codestory_workspace::cochange::find_related_tests_and_fixtures_in_paths(
            root,
            &prod.path,
            Some(&all_paths),
            &extra_str,
            3,
        ) {
            if wants_tests && !seed_paths.contains(&rel.path) && seed_paths.len() < 14 {
                seed_paths.push(rel.path.clone());
            }
            if !coupled_test_paths.contains(&rel.path) {
                coupled_test_paths.push(rel.path);
            }
        }
    }

    Ok(with_pinned_index(storage_path, |store| {
        let mut index = store.map(Index::new);
        let mut candidates: Vec<SliceCandidateBlock> = Vec::new();
        let file_map: HashMap<&str, &SliceFileHit> =
            files.iter().map(|f| (f.path.as_str(), f)).collect();

        let eval_block = |path: &str,
                          lines: &[&str],
                          non_code: &[bool],
                          first: u32,
                          last: u32,
                          title: String,
                          tier: u8,
                          file_score: f64,
                          hop_bonus: f64,
                          def_id: Option<NodeId>|
         -> SliceCandidateBlock {
            let mut mask = vec![false; terms.len()];
            let mut base_score = hop_bonus + (file_score.min(100.0) * 0.18);
            let s = (first as usize).saturating_sub(1);
            let e = (last as usize).min(lines.len());
            let lower_path = path.to_ascii_lowercase();
            let mut distinct_block_terms = 0usize;
            let mut code_hits = 0usize;
            for (idx, term) in terms.iter().enumerate() {
                if path_matches_term(&lower_path, &term.lower) {
                    base_score += idfs[idx] * 0.65;
                }
                let mut hit = false;
                let mut code_hit = false;
                for (offset, l) in lines[s..e].iter().enumerate() {
                    let line_matches = if term.case_sensitive {
                        l.contains(&term.text)
                    } else {
                        l.to_ascii_lowercase().contains(&term.lower)
                    };
                    if line_matches {
                        hit = true;
                        if !non_code.get(s + offset).copied().unwrap_or(false) {
                            code_hit = true;
                        }
                    }
                }
                if hit {
                    mask[idx] = true;
                    distinct_block_terms += 1;
                    base_score += idfs[idx] * term.boost;
                    if code_hit {
                        code_hits += 1;
                    }
                }
            }
            if distinct_block_terms >= 3 {
                base_score *= 1.0 + 0.1 * (distinct_block_terms.min(6) as f64);
            }
            if code_hits > 0 {
                base_score *= 1.25;
            } else if distinct_block_terms > 0 {
                base_score *= 0.55;
            }
            if !title.is_empty() {
                for (idx, term) in terms.iter().enumerate() {
                    if title.contains(&term.text)
                        || title.to_ascii_lowercase().contains(&term.lower)
                    {
                        base_score += idfs[idx] * 2.4;
                    }
                }
                if (title.ends_with(".__init__") || title.ends_with("::__init__"))
                    && e.saturating_sub(s) > 20
                {
                    base_score *= 0.70;
                }
            }
            let effective_tier =
                if tier == 0 && (title.contains(" test::") || title.contains(" tests::")) {
                    1
                } else {
                    tier
                };
            if effective_tier == 0 {
                base_score *= if wants_tests { 1.15 } else { 1.45 };
            } else if effective_tier == 1 {
                base_score *= if wants_tests { 1.30 } else { 0.35 };
            } else {
                base_score *= 0.15;
            }
            let span_lines = e.saturating_sub(s).max(1);
            let non_code_count = non_code[s..e].iter().filter(|&&b| b).count();
            if span_lines >= 6 && non_code_count * 3 >= span_lines * 2 {
                base_score *= 0.35;
            } else if lines.len() > 80 && first <= 20 && span_lines <= 35 {
                let import_lines = lines[s..e]
                    .iter()
                    .filter(|l| {
                        let t = l.trim_start();
                        t.starts_with("import ")
                            || t.starts_with("from ")
                            || t.starts_with("use ")
                            || t.starts_with("package ")
                            || t.starts_with("//")
                            || t.starts_with('#')
                    })
                    .count();
                if import_lines * 2 >= span_lines {
                    base_score *= 0.25;
                }
            }
            let rendered = render_slice_block(path, lines, first, last, &title);
            let bytes = rendered.len();
            let is_whole_small_file =
                first == 1 && e >= lines.len() && lines.len() <= 80 && bytes <= 2_200;
            SliceCandidateBlock {
                path: path.to_string(),
                first,
                last,
                title,
                rendered,
                bytes,
                term_mask: mask,
                base_score,
                file_score,
                tier: effective_tier,
                hop_bonus,
                def_id,
                is_whole_small_file,
            }
        };

        let mut seed_samples: Vec<(String, String)> = Vec::new();
        let mut cross_file_hops_added = 0usize;
        for rel_path in &seed_paths {
            let abs = root.join(rel_path);
            let Some((live_file, bytes)) = open_live(root, abs) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            let tier = slice_path_tier(rel_path);
            if seed_samples.len() < 6 && tier <= 1 {
                seed_samples.push((rel_path.clone(), text.chars().take(14_000).collect()));
            }
            let lines = text.lines().collect::<Vec<_>>();
            if lines.is_empty() {
                continue;
            }
            let non_code = line_is_non_code_flags(&lines);
            let count = lines.len() as u32;
            let file_score = file_map.get(rel_path.as_str()).map_or(2.5, |fh| fh.score);
            let is_coupled = coupled_test_paths.contains(rel_path);
            let hop_bonus = if is_coupled && wants_tests { 1.5 } else { 0.0 };

            let indexed_defs = index.as_mut().and_then(|idx| idx.serve(&live_file).1);
            let live_ast = if indexed_defs.is_none() {
                live_ast_slice_facts(rel_path, &text)
            } else {
                None
            };
            let defs_ref: Option<&[Definition]> = indexed_defs
                .as_deref()
                .map(Vec::as_slice)
                .or_else(|| live_ast.as_ref().map(|a| a.definitions.as_slice()));

            let raw_hit_lines = if let Some(fh) = file_map.get(rel_path.as_str()) {
                fh.hit_lines.clone()
            } else {
                let (_, mut hl, _, _) = find_slice_term_lines(&text, &terms);
                if hl.is_empty() && count <= 80 {
                    hl.push(1);
                }
                hl
            };

            // Pick up to 12 well-separated hit lines (ranked by local code term density) across the file.
            let mut scored_hits: Vec<(f64, u32)> = raw_hit_lines
                .iter()
                .map(|&hl| {
                    let s = (hl as usize).saturating_sub(1);
                    let e = (s + 5).min(lines.len());
                    let mut sc = 0.0_f64;
                    for (idx, t) in terms.iter().enumerate() {
                        if lines[s..e].iter().any(|l| {
                            if t.case_sensitive {
                                l.contains(&t.text)
                            } else {
                                l.to_ascii_lowercase().contains(&t.lower)
                            }
                        }) {
                            sc += idfs[idx] * t.boost;
                        }
                    }
                    if !non_code.get(s).copied().unwrap_or(false) && starts_declaration(lines[s]) {
                        sc += 4.0;
                    }
                    (sc, hl)
                })
                .collect();
            scored_hits.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            let mut clustered_hits: Vec<u32> = Vec::new();
            for (_, hl) in scored_hits {
                if clustered_hits.iter().all(|&prev| prev.abs_diff(hl) >= 14) {
                    clustered_hits.push(hl);
                    if clustered_hits.len() >= 12 {
                        break;
                    }
                }
            }
            if clustered_hits.is_empty() && count <= 100 {
                clustered_hits.push(1);
            }

            let mut ranges: Vec<(u32, u32, String, Option<NodeId>)> = Vec::new();
            if (count <= 130 && bytes.len() <= 4_800) || count <= 60 {
                ranges.push((1, count, String::new(), None));
            } else if let Some(defs_vec) = defs_ref {
                let mut long_def_hits: HashMap<NodeId, usize> = HashMap::new();
                let mut seen_small = HashSet::new();
                // Directly admit definitions whose name matches a strong query identifier.
                for def in defs_vec {
                    if !(is_scope_kind(def.kind) || def.kind == NodeKind::CONSTANT) {
                        continue;
                    }
                    let exact_name_hit = terms.iter().any(|t| {
                        t.boost >= 1.5
                            && (def.name == t.text
                                || name_segments(&def.qualified).last() == Some(&t.text.as_str())
                                || (t.lower.len() >= 6 && def.name.eq_ignore_ascii_case(&t.lower)))
                    });
                    if exact_name_hit && let Some((f, l, _)) = declared_range(&lines, def) {
                        let span = l + 1 - f;
                        if span <= 85 && seen_small.insert(def.id) {
                            ranges.push((
                                leading_doc_comment_start(&lines, f),
                                l,
                                format!(" {}", def.qualified),
                                Some(def.id),
                            ));
                        }
                    }
                }
                for &hl in &clustered_hits {
                    if let Some(def) = enclosing_scope(defs_vec, hl) {
                        if let Some((f, l, _)) = declared_range(&lines, def) {
                            let span = l + 1 - f;
                            if (4..=85).contains(&span) {
                                if seen_small.insert(def.id) {
                                    ranges.push((
                                        leading_doc_comment_start(&lines, f),
                                        l,
                                        format!(" {}", def.qualified),
                                        Some(def.id),
                                    ));
                                }
                            } else if span < 4 {
                                let (ef, el) = enclosing_live_range(&lines, hl, 32);
                                let ef = ef.min(leading_doc_comment_start(&lines, f));
                                ranges.push((ef, el, format!(" {}", def.qualified), Some(def.id)));
                            } else {
                                let count_for_def = long_def_hits.entry(def.id).or_insert(0);
                                if *count_for_def < 5 {
                                    *count_for_def += 1;
                                    let w = window_around(hl, 15, f, l);
                                    let w0 = if w.0 <= f + 4 {
                                        leading_doc_comment_start(&lines, f)
                                    } else {
                                        w.0
                                    };
                                    let max_w = if w.0 <= f + 4 { 46 } else { 36 };
                                    ranges.push((
                                        w0,
                                        w.1.min(w0 + max_w),
                                        format!(" {}", def.qualified),
                                        Some(def.id),
                                    ));
                                }
                            }
                        }
                    } else {
                        let (f, l) = enclosing_live_range(&lines, hl, 36);
                        ranges.push((f, l, String::new(), None));
                    }
                }
            } else {
                for &hl in &clustered_hits {
                    let (f, l) = enclosing_live_range(&lines, hl, 36);
                    ranges.push((f, l, String::new(), None));
                }
            }

            // Merge overlapping nearby ranges while preserving distant distinct regions.
            ranges.sort_by_key(|r| (r.0, r.1));
            let mut deduped: Vec<(u32, u32, String, Option<NodeId>)> = Vec::new();
            for r in ranges {
                if let Some(last) = deduped.last_mut()
                    && r.0 <= last.1
                    && (r.1.saturating_sub(last.0) <= 55)
                {
                    last.1 = last.1.max(r.1);
                    if last.2.is_empty() {
                        last.2 = r.2;
                    }
                    if last.3.is_none() {
                        last.3 = r.3;
                    }
                    continue;
                }
                deduped.push(r);
            }

            let mut file_blocks = deduped
                .iter()
                .map(|(f, l, title, def_id)| {
                    eval_block(
                        rel_path,
                        &lines,
                        &non_code,
                        *f,
                        *l,
                        title.clone(),
                        tier,
                        file_score,
                        hop_bonus,
                        *def_id,
                    )
                })
                .collect::<Vec<_>>();
            file_blocks.sort_by(|a, b| b.base_score.total_cmp(&a.base_score));
            file_blocks.truncate(8);

            let top_def_ids: Vec<NodeId> = file_blocks
                .iter()
                .filter_map(|b| b.def_id)
                .take(6)
                .collect();
            candidates.extend(file_blocks);

            let mut indexed_added = 0usize;
            if let Some(idx_ref) = index.as_mut()
                && indexed_defs.is_some()
            {
                let mut seen_target_ids = HashSet::new();
                let mut push_located =
                    |target: &Located,
                     hop_bonus_val: f64,
                     candidates: &mut Vec<SliceCandidateBlock>| {
                        let same_file = target.path() == rel_path;
                        let t_tier = slice_path_tier(target.path());
                        let t_span = target.last + 1 - target.first;
                        if (!same_file && t_span < 5) || (!wants_tests && !same_file && t_tier != 0)
                        {
                            return false;
                        }
                        let t_lines = target.home.text.lines().collect::<Vec<_>>();
                        let t_non_code = line_is_non_code_flags(&t_lines);
                        let (tf, tl) = if t_span <= 75 {
                            (
                                leading_doc_comment_start(&t_lines, target.first),
                                target.last,
                            )
                        } else {
                            let (_, hl, _, _) = find_slice_term_lines(&target.home.text, &terms);
                            let anchor = hl
                                .into_iter()
                                .find(|&line| line >= target.first && line <= target.last)
                                .unwrap_or(target.first);
                            let w = window_around(anchor, 14, target.first, target.last);
                            (w.0, w.1.min(w.0 + 38))
                        };
                        let blk = eval_block(
                            target.path(),
                            &t_lines,
                            &t_non_code,
                            tf,
                            tl,
                            format!(" {}", target.definition().qualified),
                            if same_file { tier } else { t_tier },
                            if same_file {
                                file_score * 0.9
                            } else {
                                file_score * 0.6
                            },
                            hop_bonus_val,
                            Some(target.definition().id),
                        );
                        let keep =
                            same_file || blk.term_mask.iter().any(|&m| m) || hop_bonus_val >= 0.8;
                        if keep {
                            if !same_file {
                                cross_file_hops_added += 1;
                            }
                            candidates.push(blk);
                        }
                        keep
                    };

                for &id in &top_def_ids {
                    if !seen_target_ids.insert(id) {
                        continue;
                    }
                    if let Some(owner) = idx_ref.locate(root, id) {
                        let targets = used_by(idx_ref, root, &owner, 6, true, |_| false);
                        let mut added_for_owner = 0usize;
                        let mut hop2_seeds = Vec::new();
                        for target in targets {
                            if added_for_owner >= 4 {
                                break;
                            }
                            if !seen_target_ids.insert(target.definition().id) {
                                continue;
                            }
                            let same_file = target.path() == rel_path;
                            let t_span = target.last + 1 - target.first;
                            if push_located(
                                &target,
                                if same_file { 1.6 } else { 0.95 },
                                &mut candidates,
                            ) {
                                added_for_owner += 1;
                                indexed_added += 1;
                                if t_span <= 24 {
                                    hop2_seeds.push(target);
                                }
                            }
                        }
                        // Step one extra hop through thin service/interface forwarders.
                        for wrapper in hop2_seeds.into_iter().take(2) {
                            let hop2 = used_by(idx_ref, root, &wrapper, 3, true, |_| false);
                            for t2 in hop2 {
                                if seen_target_ids.insert(t2.definition().id) {
                                    let _ = push_located(&t2, 0.70, &mut candidates);
                                }
                            }
                        }
                        // Incoming implementations (OVERRIDE) and callers (CALL / USAGE).
                        let mut incoming = idx_ref
                            .store
                            .get_incoming_edges_for_node_id(
                                id,
                                &[],
                                TrailCallerScope::IncludeTestsAndBenches,
                                true,
                            )
                            .unwrap_or_default();
                        incoming.sort_by_key(|e| match e.kind {
                            EdgeKind::OVERRIDE => 0u8,
                            EdgeKind::CALL => 1,
                            _ => 2,
                        });
                        let mut incoming_added = 0usize;
                        for edge in incoming {
                            if incoming_added >= 3 {
                                break;
                            }
                            if !(edge.kind == EdgeKind::OVERRIDE || is_certain(&edge)) {
                                continue;
                            }
                            let src_id = edge.effective_source();
                            if !seen_target_ids.insert(src_id) {
                                continue;
                            }
                            if let Some(caller) = idx_ref.locate(root, src_id) {
                                let bonus = if edge.kind == EdgeKind::OVERRIDE {
                                    1.4
                                } else {
                                    0.9
                                };
                                if push_located(&caller, bonus, &mut candidates) {
                                    incoming_added += 1;
                                    indexed_added += 1;
                                }
                            }
                        }
                    }
                }
            }
            if indexed_added == 0
                && let Some(ast) = &live_ast
            {
                let mut seen_same = top_def_ids.iter().copied().collect::<HashSet<_>>();
                let mut added_same = 0usize;
                for id in &top_def_ids {
                    if let Some(targets) = ast.same_file_edges.get(id) {
                        for target_id in targets {
                            if added_same >= 3 || !seen_same.insert(*target_id) {
                                continue;
                            }
                            if let Some(def) = ast.definitions.iter().find(|d| d.id == *target_id)
                                && let Some((f, l, _)) = declared_range(&lines, def)
                            {
                                let span = l + 1 - f;
                                if span < 4 {
                                    continue;
                                }
                                let (tf, tl) = if span <= 65 {
                                    (f, l)
                                } else {
                                    let w = window_around(f, 14, f, l);
                                    (w.0, w.1.min(w.0 + 38))
                                };
                                added_same += 1;
                                candidates.push(eval_block(
                                    rel_path,
                                    &lines,
                                    &non_code,
                                    tf,
                                    tl,
                                    format!(" {}", def.qualified),
                                    tier,
                                    file_score * 0.9,
                                    1.55,
                                    Some(def.id),
                                ));
                            }
                        }
                    }
                }
            }
        }

        // Cross-file 1-hop live callee, import, & quoted fixture path expansion.
        if (index.is_none() || cross_file_hops_added < 3) && !candidates.is_empty() {
            let mut top_blocks: Vec<&SliceCandidateBlock> = candidates
                .iter()
                .filter(|b| b.tier <= u8::from(wants_tests))
                .collect();
            top_blocks.sort_by(|a, b| b.base_score.total_cmp(&a.base_score));
            top_blocks.truncate(5);
            let sample_refs: Vec<(&str, &str)> = seed_samples
                .iter()
                .map(|(p, s)| (p.as_str(), s.as_str()))
                .collect();
            let (path_hints, callee_syms) =
                extract_cross_file_hints(&top_blocks, &sample_refs, &all_paths);
            let seed_set: HashSet<&str> = seed_paths.iter().map(String::as_str).collect();
            let hop_files = discover_live_hop_paths(&LiveHopQuery {
                root,
                all_paths: &all_paths,
                seed_set: &seed_set,
                path_hints: &path_hints,
                callee_symbols: &callee_syms,
                terms: &terms,
                wants_tests,
                allow_disk_scan: index.is_none(),
                limit: 5,
            });
            for (hop_path, matched_sym, is_direct_fixture) in hop_files {
                let abs = root.join(&hop_path);
                let Some((_, bytes)) = open_live(root, abs) else {
                    continue;
                };
                let text = String::from_utf8_lossy(&bytes);
                let lines = text.lines().collect::<Vec<_>>();
                if lines.is_empty() {
                    continue;
                }
                let non_code = line_is_non_code_flags(&lines);
                let hop_tier = slice_path_tier(&hop_path);
                let ast = if is_direct_fixture {
                    None
                } else {
                    live_ast_slice_facts(&hop_path, &text)
                };
                let mut hop_ranges: Vec<(u32, u32, String, Option<NodeId>)> = Vec::new();
                if is_direct_fixture && bytes.len() <= 8_000 {
                    hop_ranges.push((1, (lines.len() as u32).min(70), String::new(), None));
                } else if let Some(ref ast_facts) = ast {
                    let (_, hop_hl, _, _) = find_slice_term_lines(&text, &terms);
                    for def in &ast_facts.definitions {
                        if !is_scope_kind(def.kind) {
                            continue;
                        }
                        let name_hit = (!matched_sym.is_empty()
                            && (def.name == matched_sym || def.qualified.contains(&matched_sym)))
                            || callee_syms
                                .iter()
                                .any(|s| def.name == *s || def.qualified.ends_with(s))
                            || terms
                                .iter()
                                .any(|t| def.name.to_ascii_lowercase().contains(&t.lower));
                        if name_hit && let Some((f, l, _)) = declared_range(&lines, def) {
                            let span = l + 1 - f;
                            if span < 5
                                && !terms.iter().any(|t| t.boost >= 1.5 && def.name == t.text)
                            {
                                continue;
                            }
                            let (tf, tl) = if span <= 65 {
                                (f, l)
                            } else {
                                let anchor = hop_hl
                                    .iter()
                                    .copied()
                                    .find(|&ln| ln >= f && ln <= l)
                                    .unwrap_or(f);
                                let w = window_around(anchor, 16, f, l);
                                (w.0, w.1.min(w.0 + 38))
                            };
                            hop_ranges.push((tf, tl, format!(" {}", def.qualified), Some(def.id)));
                            if hop_ranges.len() >= 3 {
                                break;
                            }
                        }
                    }
                }
                if hop_ranges.is_empty() {
                    let (_, mut hl, _, _) = find_slice_term_lines(&text, &terms);
                    if hl.is_empty() && lines.len() <= 100 {
                        hl.push(1);
                    }
                    if let Some(&first_hl) = hl.first() {
                        let (f, l) = enclosing_live_range(&lines, first_hl, 36);
                        hop_ranges.push((f, l, String::new(), None));
                    }
                }
                let hop_file_score = if is_direct_fixture {
                    top_score.max(12.0) * 0.78
                } else {
                    6.0
                };
                let hop_bonus_val = if is_direct_fixture { 3.2 } else { 1.45 };
                for (f, l, title, def_id) in hop_ranges {
                    candidates.push(eval_block(
                        &hop_path,
                        &lines,
                        &non_code,
                        f,
                        l,
                        title,
                        if is_direct_fixture && wants_tests {
                            1
                        } else {
                            hop_tier
                        },
                        hop_file_score,
                        hop_bonus_val,
                        def_id,
                    ));
                }
            }
        }

        // Submodular greedy packing maximizing novel IDF + file quality per byte inside `budget_bytes`.
        let mut chosen: Vec<SliceCandidateBlock> = Vec::new();
        let mut term_cov = vec![0u32; terms.len()];
        let mut file_cov: HashMap<String, u32> = HashMap::new();
        let mut used_bytes = 0usize;
        let mut remaining = candidates;
        let footer_reserve = 240usize.min(budget_bytes / 10);

        while !remaining.is_empty() {
            let mut best_idx = None;
            let mut best_gain = 0.0_f64;
            for (idx, blk) in remaining.iter().enumerate() {
                if blk.bytes > budget_bytes.saturating_sub(used_bytes + footer_reserve)
                    && !chosen.is_empty()
                {
                    continue;
                }
                if blk.bytes > budget_bytes {
                    continue;
                }
                let seen_in_file = file_cov.get(&blk.path).copied().unwrap_or(0);
                let file_cap = if blk.tier >= 1 {
                    2
                } else if blk.file_score >= top_score * 0.85 {
                    3
                } else {
                    2
                };
                if seen_in_file >= file_cap {
                    continue;
                }
                let blk_len = blk.last.saturating_sub(blk.first) + 1;
                let overlaps_chosen = chosen.iter().any(|c| {
                    if c.path != blk.path || c.first > blk.last || blk.first > c.last {
                        return false;
                    }
                    let overlap = c.last.min(blk.last).saturating_sub(c.first.max(blk.first)) + 1;
                    overlap > 4 && overlap * 3 >= blk_len
                });
                if overlaps_chosen {
                    continue;
                }
                let mut novel_score = blk.hop_bonus + blk.base_score * 0.45;
                let mut distinct_blk_terms = 0usize;
                let mut has_strong_idf_term = false;
                for (t_idx, (&covered, term)) in blk.term_mask.iter().zip(&terms).enumerate() {
                    if covered {
                        distinct_blk_terms += 1;
                        if term.boost >= 1.5 && idfs[t_idx] >= 2.2 {
                            has_strong_idf_term = true;
                        }
                        let decay_base = if term.boost >= 1.5 {
                            0.72_f64
                        } else {
                            0.45_f64
                        };
                        let decay = decay_base.powi(term_cov[t_idx] as i32);
                        novel_score += idfs[t_idx] * term.boost * decay;
                    }
                }
                if blk.file_score < top_score * 0.35
                    && blk.hop_bonus < 1.5
                    && distinct_blk_terms < 2
                    && !has_strong_idf_term
                {
                    continue;
                }
                let repeat_base = if blk.file_score >= top_score * 0.60 {
                    0.86_f64
                } else {
                    0.68_f64
                };
                novel_score +=
                    blk.file_score.min(100.0) * 0.35 * repeat_base.powi(seen_in_file as i32);
                if blk_len <= 6 && !blk.is_whole_small_file && blk.hop_bonus < 2.5 {
                    let matches_strong_title = terms
                        .iter()
                        .any(|t| t.boost >= 1.5 && blk.title.contains(&t.text));
                    if !matches_strong_title {
                        novel_score *= 0.50;
                    }
                }
                if blk.tier == 0 {
                    novel_score *= if wants_tests { 1.18 } else { 1.50 };
                } else if blk.tier == 1 {
                    novel_score *= if wants_tests { 1.24 } else { 0.32 };
                } else {
                    novel_score *= 0.12;
                }
                let size_div = if blk.is_whole_small_file {
                    (blk.bytes.max(360) as f64).powf(0.45)
                } else {
                    (blk.bytes.max(850) as f64).powf(0.55)
                };
                let gain = novel_score / size_div;
                if gain > best_gain {
                    best_gain = gain;
                    best_idx = Some(idx);
                }
            }
            let Some(idx) = best_idx else {
                break;
            };
            let picked = remaining.swap_remove(idx);
            if used_bytes + picked.bytes > budget_bytes {
                continue;
            }
            used_bytes += picked.bytes;
            for (t_idx, &covered) in picked.term_mask.iter().enumerate() {
                if covered {
                    term_cov[t_idx] += 1;
                }
            }
            *file_cov.entry(picked.path.clone()).or_insert(0) += 1;
            chosen.push(picked);
        }

        // Fill remaining space up to `budget_bytes - footer_reserve` with a trimmed top non-overlapping block.
        let avail = budget_bytes.saturating_sub(used_bytes + footer_reserve);
        if avail >= 260 {
            remaining.sort_by(|a, b| b.base_score.total_cmp(&a.base_score));
            if let Some(pos) = remaining.iter().position(|blk| {
                !chosen
                    .iter()
                    .any(|c| c.path == blk.path && c.first <= blk.last && blk.first <= c.last)
            }) {
                let mut extra = remaining.remove(pos);
                let mut kept_lines = String::new();
                let mut end_line = extra.first;
                let mut lines_iter = extra.rendered.lines();
                if let Some(header) = lines_iter.next() {
                    kept_lines.push_str(header);
                    kept_lines.push('\n');
                    for body_line in lines_iter {
                        if kept_lines.len() + body_line.len() + 1 > avail {
                            break;
                        }
                        if let Some((num_s, _)) = body_line.split_once(':')
                            && let Ok(n) = num_s.trim().parse::<u32>()
                        {
                            end_line = n;
                        }
                        kept_lines.push_str(body_line);
                        kept_lines.push('\n');
                    }
                    if end_line > extra.first + 5 {
                        extra.rendered = kept_lines;
                        extra.bytes = extra.rendered.len();
                        extra.last = end_line;
                        chosen.push(extra);
                    }
                }
            }
        }

        chosen.sort_by(|a, b| {
            let tier_key = |t: u8| -> u8 {
                if wants_tests && t == 1 {
                    0
                } else if wants_tests && t == 0 {
                    1
                } else {
                    t
                }
            };
            tier_key(a.tier)
                .cmp(&tier_key(b.tier))
                .then_with(|| b.base_score.total_cmp(&a.base_score))
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.first.cmp(&b.first))
        });

        let mut out = String::new();
        for blk in &chosen {
            out.push_str(&blk.rendered);
        }

        // Append pointers to remaining top candidates and coupled tests/fixtures.
        let mut pointers = Vec::new();
        let mut seen_ptrs = chosen
            .iter()
            .map(|c| format!("{}:{}-{}", c.path, c.first, c.last))
            .collect::<HashSet<_>>();
        remaining.sort_by(|a, b| b.base_score.total_cmp(&a.base_score));
        for blk in remaining.iter().filter(|b| b.tier <= 1).take(6) {
            let ptr = format!("{}:{}-{}", blk.path, blk.first, blk.last);
            if seen_ptrs.insert(ptr.clone()) {
                pointers.push(format!("  {ptr}{}\n", blk.title));
            }
        }
        for path in &coupled_test_paths {
            if !chosen.iter().any(|c| &c.path == path) && seen_ptrs.insert(path.clone()) {
                pointers.push(format!("  {path}\n"));
            }
        }
        if !pointers.is_empty() && out.len() + 14 < budget_bytes {
            let mut footer = String::from("more:\n");
            for p in pointers {
                if out.len() + footer.len() + p.len() <= budget_bytes {
                    footer.push_str(&p);
                }
            }
            if footer.len() > "more:\n".len() {
                out.push_str(&footer);
            }
        }
        out
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn edit_replaces_symbol_and_validates_syntax() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        let file = src.join("lib.rs");
        fs::write(
            &file,
            "pub fn alpha() -> i32 {\n    1\n}\n\npub fn beta() -> i32 {\n    2\n}\n",
        )
        .unwrap();
        let storage_path = root.join("codestory.db");

        // 1. Valid edit by symbol selector
        let receipt = edit(
            root,
            &storage_path,
            "src/lib.rs::alpha",
            "pub fn alpha() -> i32 {\n    10\n    + 20\n}\n",
        )
        .expect("valid edit should succeed");
        assert_eq!(
            receipt,
            "updated src/lib.rs::alpha (lines 1..4); syntax ok\n"
        );
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "pub fn alpha() -> i32 {\n    10\n    + 20\n}\n\npub fn beta() -> i32 {\n    2\n}\n"
        );

        // 2. Invalid syntax edit fails closed without modifying the file
        let err = edit(
            root,
            &storage_path,
            "src/lib.rs::beta",
            "pub fn beta() -> i32 {\n    let x =\n",
        )
        .expect_err("broken syntax must fail closed");
        assert!(
            err.message.contains("syntax error after edit"),
            "unexpected error: {}",
            err.message
        );
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "pub fn alpha() -> i32 {\n    10\n    + 20\n}\n\npub fn beta() -> i32 {\n    2\n}\n"
        );
    }

    #[test]
    fn read_with_budget_caps_multi_selector_and_large_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        let a = (1..=120)
            .map(|i| format!("pub fn fn_a_{i:03}() -> usize {{ {i} }}\n"))
            .collect::<String>();
        let b = (1..=120)
            .map(|i| format!("pub fn fn_b_{i:03}() -> usize {{ {i} }}\n"))
            .collect::<String>();
        fs::write(src.join("a.rs"), &a).unwrap();
        fs::write(src.join("b.rs"), &b).unwrap();
        let storage_path = root.join("missing.db");

        // Multi-selector read stays bounded under max_bytes and announces remainder cleanly.
        let capped = read_with_budget(
            root,
            &storage_path,
            &["src/a.rs".to_string(), "src/b.rs".to_string()],
            Some(2_000),
        )
        .expect("bounded read should succeed");
        assert!(capped.len() <= 2_400, "unexpected size: {}", capped.len());
        assert!(capped.contains("outline") || capped.contains("omitted"));
    }

    #[test]
    fn slice_unindexed_uses_live_ast_docstring_filter_and_cross_file_hop() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("pkg")).unwrap();
        fs::write(
            root.join("pkg/api.py"),
            r#"from pkg.engine import compute_composite_values

class Builder:
    def __init__(self, target, fields, values_mode, select_flag, extra_one=None):
        """
        Builder initializes values_mode, select_flag, composite values.
        values() and select_flag docs here repeated carefully.
        """
        self.target = target
        self.fields = fields
        self.values_mode = values_mode
        self.select_flag = select_flag
        self.a = 1
        self.b = 2
        self.c = 3
        self.d = 4
        self.e = 5
        self.f = 6
        self.g = 7
        self.h = 8
        self.i = 9
        self.j = 10

    def run_values_query(self):
        return compute_composite_values(self.fields)
"#,
        )
        .unwrap();
        fs::write(
            root.join("pkg/engine.py"),
            r#"def compute_composite_values(columns):
    out = []
    for col in columns:
        out.append(col)
    return tuple(out)
"#,
        )
        .unwrap();
        let storage_path = root.join("missing.db");
        let reply = slice(
            root,
            &storage_path,
            "How does run_values_query build composite values?",
            Some(4_096),
        )
        .expect("unindexed slice should succeed");
        assert!(
            reply.contains("compute_composite_values"),
            "expected live 1-hop callee declaration in unindexed slice:\n{reply}"
        );
    }

    #[test]
    fn slice_path_prefix_and_fixture_sibling_discovery() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("pkg/services/playlist/playlistimpl")).unwrap();
        fs::create_dir_all(root.join("pkg/registry/apps/playlist")).unwrap();
        fs::create_dir_all(root.join("tests/specs/coverage/nested_ignore_warning")).unwrap();
        fs::write(
            root.join("pkg/services/playlist/model.go"),
            "package playlist\ntype DeletePlaylistCommand struct { UID string }\n",
        )
        .unwrap();
        fs::write(
            root.join("pkg/services/playlist/playlistimpl/playlist.go"),
            "package playlistimpl\nfunc (s *Service) Delete() error { return nil }\n",
        )
        .unwrap();
        fs::write(
            root.join("pkg/registry/apps/playlist/legacy_storage.go"),
            "package legacy\nfunc (s *LegacyStorage) Delete() error { return nil }\n",
        )
        .unwrap();
        fs::write(
            root.join("tests/specs/coverage/nested_ignore_warning/__test__.jsonc"),
            "{ \"steps\": [{ \"args\": \"coverage cov\", \"output\": \"cov.out\" }] }\n",
        )
        .unwrap();
        fs::write(
            root.join("tests/specs/coverage/nested_ignore_warning/cov.out"),
            "Warning nested deno-coverage-ignore-start comment\n",
        )
        .unwrap();
        fs::write(
            root.join("tests/specs/coverage/nested_ignore_warning/main.ts"),
            "// deno-coverage-ignore-start\nexport const x = 1;\n",
        )
        .unwrap();
        let storage_path = root.join("missing.db");

        let scoped = slice(
            root,
            &storage_path,
            "Under `pkg/services/playlist`, show `DeletePlaylistCommand` and `Delete`",
            Some(4_096),
        )
        .expect("path-scoped slice should succeed");
        assert!(
            scoped.contains("pkg/services/playlist/model.go"),
            "{scoped}"
        );
        assert!(
            scoped.contains("pkg/services/playlist/playlistimpl/playlist.go"),
            "{scoped}"
        );

        let fixtures = slice(
            root,
            &storage_path,
            "Find the coverage regression fixtures and expected warning output for nested ignore",
            Some(4_096),
        )
        .expect("fixture slice should succeed");
        assert!(fixtures.contains("cov.out"), "{fixtures}");
        assert!(fixtures.contains("__test__.jsonc"), "{fixtures}");
        assert!(fixtures.contains("main.ts"), "{fixtures}");
    }

    #[test]
    fn refs_file_glob_narrows_matches() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/a")).unwrap();
        fs::create_dir_all(root.join("src/b")).unwrap();
        fs::write(root.join("src/a/one.rs"), "pub fn shared_target() {}\n").unwrap();
        fs::write(root.join("src/b/two.rs"), "pub fn shared_target() {}\n").unwrap();
        let storage_path = root.join("missing.db");
        let reply = refs_with_budget_and_globs(
            root,
            &storage_path,
            "shared_target",
            &["src/a/**".to_string()],
            20,
            None,
            None,
        )
        .expect("glob-filtered refs should succeed");
        assert!(reply.contains("src/a/one.rs"), "{reply}");
        assert!(!reply.contains("src/b/two.rs"), "{reply}");
    }
}
