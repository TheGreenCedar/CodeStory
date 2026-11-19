//! Exact coordinates in a question. These are live source observations, outside
//! the implementation ranking and without authority over call bindings.

use super::*;
use codestory_indexer::qualified_callable::{CallableOwner, qualified_callable_lines};
use std::ops::RangeInclusive;
use std::path::Component;

const MAX_LOCATIONS: usize = 2;
const MAX_HUNKS: usize = 4;
const MAX_SOURCE_LINES: u32 = 64;
const FILE_BYTES: usize = 192 * 1024;
const NAME_BYTES: usize = 24 * 1024 * 1024;
const NAME_ENTRIES: usize = 20_000;
const NAME_FILES: usize = 4_096;
const NAME_PARSE_FILES: usize = 24;
const PATH_DEPTH: usize = 24;
const PATH_ENTRIES: usize = 4_096;

pub(super) struct ExplicitReply {
    pub text: String,
    pub locations: usize,
    pub source_lines: usize,
}

struct Location {
    path: String,
    text: String,
    lines: RangeInclusive<u32>,
    label: &'static str,
}

pub(super) fn render(
    root: &Path,
    scope: Option<&ignore::overrides::Override>,
    question: &str,
    max_locations: usize,
    body_lines: Option<usize>,
    max_bytes: usize,
) -> ExplicitReply {
    // Keep room for ordinary question results, even when the supplied coordinates
    // match. The overall reply applies the original request's limits again.
    let limit_bytes = if max_bytes == 0 {
        DEFAULT_MAX_BYTES
    } else {
        max_bytes
    };
    let cap = (limit_bytes / 3).clamp(128, 2_500).min(limit_bytes);
    let mut reply = ExplicitReply {
        text: String::new(),
        locations: 0,
        source_lines: 0,
    };
    if cap < 128 {
        return reply;
    }
    let (prose, diffs) = split_question(question);
    let mut unique = BTreeSet::new();
    let cap_locations = max_locations.min(MAX_LOCATIONS);
    let mut locations = Vec::new();
    for diff in diffs.into_iter().take(MAX_HUNKS) {
        if let Some(hunks) = diff_locations(root, scope, &diff) {
            locations.extend(hunks);
        }
        if locations.len() >= cap_locations {
            break;
        }
    }
    let mut names = Vec::new();
    for (literal, quoted) in cited_tokens(&prose).into_iter().take(32) {
        if locations.len() >= cap_locations {
            break;
        }
        if !unique.insert((literal, quoted)) {
            continue;
        }
        if let Some(location) = explicit_path(root, scope, literal) {
            locations.push(location);
        } else if quoted
            && names.len() < MAX_LOCATIONS
            && let Some(name) = qualified_name(literal)
        {
            names.push(name);
        }
    }
    if locations.len() < cap_locations && !names.is_empty() {
        locations.extend(qualified_locations(root, scope, &names));
    }
    let mut printed = Printed::default();
    let mut budget = body_lines.unwrap_or(0).min(MAX_SOURCE_LINES as usize);
    let mut emitted = BTreeSet::new();
    for location in locations {
        if reply.locations == cap_locations {
            break;
        }
        let (first, last) = (*location.lines.start(), *location.lines.end());
        if !emitted.insert((location.path.clone(), first, last)) {
            continue;
        }
        let path = printable(&location.path);
        let header = format!("{path}:{first}-{last} ({})\n", location.label);
        if reply.text.len() + header.len() + 40 > cap {
            break;
        }
        if reply.locations == 0 {
            reply
                .text
                .push_str("Explicit locations in current source:\n");
        }
        reply.text.push_str(&header);
        if budget > 0 {
            let lines = location.text.lines().collect::<Vec<_>>();
            let before = budget;
            Page {
                path: &path,
                lines: &lines,
                printed: &mut printed,
                max_bytes: cap,
            }
            .block(&mut reply.text, &mut budget, first, last, "", "");
            reply.source_lines += before - budget;
        }
        reply.locations += 1;
    }
    reply.text = bound_reply(reply.text, cap, &[]);
    reply
}

fn split_question(question: &str) -> (String, Vec<Vec<&str>>) {
    let mut prose = String::new();
    let mut diffs = Vec::new();
    let mut fenced = None;
    let mut body = Vec::new();
    for line in question.split_terminator('\n') {
        match fenced {
            Some(is_diff) if line.trim_end() == "```" => {
                if is_diff {
                    diffs.push(std::mem::take(&mut body));
                }
                fenced = None;
            }
            Some(true) => body.push(line),
            Some(false) => {}
            None if let Some(fence) = line.trim().strip_prefix("```") => {
                fenced = Some(fence == "diff");
            }
            None => {
                prose.push_str(line);
                prose.push('\n');
            }
        }
    }
    (prose, diffs)
}

fn cited_tokens(prose: &str) -> Vec<(&str, bool)> {
    let mut chunks = prose.split('`');
    let mut result = Vec::new();
    while let Some(unquoted) = chunks.next() {
        result.extend(
            unquoted
                .split(|c: char| c.is_whitespace() || "()[]{}<>,;!?\"'".contains(c))
                .map(|token| (token.trim_end_matches('.'), false))
                .filter(|(token, _)| token.contains(['/', '\\', '.', ':'])),
        );
        if let Some(quoted) = chunks.next() {
            // A single opening backtick without a closing delimiter supplies no selector.
            if !chunks.clone().any(|_| true) {
                break;
            }
            result.push((quoted, true));
        }
    }
    result
}

fn path_spelling(path: &str) -> bool {
    path.contains(['/', std::path::MAIN_SEPARATOR, '.'])
        && !path.contains([':', '\0'])
        && Path::new(path)
            .components()
            .all(|piece| matches!(piece, Component::Normal(_) | Component::CurDir))
}

fn read_path(
    root: &Path,
    scope: Option<&ignore::overrides::Override>,
    path: &str,
) -> Option<(String, String)> {
    if !path_spelling(path) || path.chars().any(char::is_control) {
        return None;
    }
    let ProjectRelativePathResolution::Existing { absolute, relative } =
        codestory_workspace::resolve_project_relative_path(root, Path::new(path)).ok()?
    else {
        return None;
    };
    if !in_scope(scope, &relative) || !discoverable_path(root, &absolute, &relative) {
        return None;
    }
    let mut buffer = Vec::new();
    let canonical_root = root.canonicalize().ok()?;
    let text = source_candidates::load_live(&canonical_root, &absolute, FILE_BYTES, &mut buffer)?;
    if codestory_indexer::structural::is_structural_candidate_path(&absolute)
        && !codestory_indexer::structural::is_structural_source_text(text.as_bytes())
    {
        return None;
    }
    Some((project_path(root, &absolute), text.to_owned()))
}

/// Use the ordinary ignore stack for implicit reads, with the walk confined
/// to the requested file and its ancestors. Count sibling directory entries
/// first because the ignore iterator skips unmatched entries before yielding.
fn discoverable_path(root: &Path, absolute: &Path, relative: &Path) -> bool {
    let depth = relative.components().count();
    if depth == 0
        || depth > PATH_DEPTH
        || !std::fs::symlink_metadata(absolute).is_ok_and(|metadata| metadata.is_file())
    {
        return false;
    }
    let mut budget = PATH_ENTRIES;
    for directory in absolute.ancestors().skip(1).take(depth) {
        if !std::fs::symlink_metadata(directory).is_ok_and(|metadata| metadata.is_dir()) {
            return false;
        }
        let Ok(entries) = directory.read_dir() else {
            return false;
        };
        let count = entries.take(budget + 1).count();
        let Some(remaining) = budget.checked_sub(count) else {
            return false;
        };
        budget = remaining;
    }
    let target = absolute.to_path_buf();
    walk_live_source(root)
        .max_depth(Some(depth))
        .filter_entry(move |entry| {
            target.starts_with(entry.path())
                || (cfg!(windows)
                    && codestory_workspace::workspace_relative_path(entry.path(), &target)
                        .is_some())
        })
        .build()
        .flatten()
        .any(|entry| {
            entry.file_type().is_some_and(|kind| kind.is_file())
                && (entry.path() == absolute
                    || (cfg!(windows)
                        && codestory_workspace::workspace_relative_path(entry.path(), absolute)
                            .is_some_and(|path| path.as_os_str().is_empty())))
        })
}

fn explicit_path(
    root: &Path,
    scope: Option<&ignore::overrides::Override>,
    token: &str,
) -> Option<Location> {
    let (path, target) = parse_selector(token)?;
    if matches!(target, Target::Symbol(_)) {
        return None;
    }
    let (path, text) = read_path(root, scope, path)?;
    let count = text.lines().count() as u32;
    let (first, last) = match target {
        Target::File => (1, count.min(8)),
        Target::Line(line) => (line, line),
        Target::Range(first, last) => (first, last),
        Target::Symbol(_) => return None,
    };
    if first == 0 || first > last || last > count || last - first >= MAX_SOURCE_LINES {
        return None;
    }
    Some(Location {
        path,
        text,
        lines: first..=last,
        label: "supplied path",
    })
}

struct QualifiedName<'a> {
    owner: CallableOwner<'a>,
    member: &'a str,
    module: Option<Vec<&'a str>>,
}

fn identifier(s: &str) -> bool {
    let mut bytes = s.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn qualified_name(text: &str) -> Option<QualifiedName<'_>> {
    if let Some((owner, member)) = text.strip_prefix("(*").and_then(|s| s.split_once(").")) {
        return (identifier(owner) && identifier(member)).then_some(QualifiedName {
            owner: CallableOwner::GoPointer(owner),
            member,
            module: None,
        });
    }
    if let Some((owner, member)) = text.split_once("::") {
        return (identifier(owner) && identifier(member)).then_some(QualifiedName {
            owner: CallableOwner::RustImpl(owner),
            member,
            module: None,
        });
    }
    let mut module = text.split('.').collect::<Vec<_>>();
    if !(3..=7).contains(&module.len()) || !module.iter().copied().all(identifier) {
        return None;
    }
    let member = module.pop()?;
    let owner = CallableOwner::PythonClass(module.pop()?);
    Some(QualifiedName {
        owner,
        member,
        module: Some(module),
    })
}

fn qualified_locations(
    root: &Path,
    scope: Option<&ignore::overrides::Override>,
    names: &[QualifiedName<'_>],
) -> Vec<Location> {
    let mut found = names.iter().map(|_| Vec::new()).collect::<Vec<_>>();
    let mut discovery = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if let Some(module) = &name.module {
            let module = module.join("/");
            for path in [format!("{module}.py"), format!("{module}/__init__.py")] {
                if let Some((relative, text)) = read_path(root, scope, &path) {
                    record_declarations(&mut found[i], &relative, &text, name);
                }
            }
        } else {
            discovery.push(i);
        }
    }
    let limited =
        !discovery.is_empty() && find_named_files(root, scope, names, &mut found, &discovery);
    found
        .into_iter()
        .enumerate()
        .filter_map(|(i, mut matches)| {
            if matches.len() != 1 {
                return None;
            }
            let mut location = matches.pop()?;
            if limited && discovery.contains(&i) {
                location.label = "qualified declaration, discovery limited";
            }
            Some(location)
        })
        .collect()
}

fn record_declarations(
    found: &mut Vec<Location>,
    path: &str,
    text: &str,
    name: &QualifiedName<'_>,
) {
    for line in qualified_callable_lines(text, &name.owner, name.member) {
        found.push(Location {
            path: path.into(),
            text: text.to_owned(),
            lines: line..=line,
            label: "qualified declaration",
        });
        if found.len() > 1 {
            break;
        }
    }
}

/// Return whether the walk left part of the requested source scope unexamined.
fn find_named_files(
    root: &Path,
    scope: Option<&ignore::overrides::Override>,
    names: &[QualifiedName<'_>],
    found: &mut [Vec<Location>],
    discovery: &[usize],
) -> bool {
    let mut walk = walk_live_source(root);
    walk.sort_by_file_path(Path::cmp);
    let Ok(canonical_root) = root.canonicalize() else {
        return true;
    };
    let mut bytes = 0;
    let mut files = 0;
    let mut parsed = 0;
    let mut limited = false;
    let mut buffer = Vec::new();
    for (entry_no, entry) in walk.build().enumerate() {
        if entry_no == NAME_ENTRIES || files == NAME_FILES || bytes == NAME_BYTES {
            return true;
        }
        if discovery.iter().all(|&i| found[i].len() > 1) {
            break;
        }
        let Ok(entry) = entry else {
            limited = true;
            continue;
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file())
            || !entry
                .path()
                .strip_prefix(root)
                .is_ok_and(|path| in_scope(scope, path))
        {
            continue;
        }
        let extension = entry.path().extension().and_then(|ext| ext.to_str());
        let possible = discovery
            .iter()
            .copied()
            .filter(|&i| found[i].len() <= 1 && extension == Some(names[i].owner.extension()))
            .collect::<Vec<_>>();
        if possible.is_empty() {
            continue;
        }
        files += 1;
        let text = source_candidates::load_live(
            &canonical_root,
            entry.path(),
            (NAME_BYTES - bytes).min(FILE_BYTES),
            &mut buffer,
        );
        let Some(text) = text else {
            bytes += buffer.len();
            limited = true;
            continue;
        };
        bytes += text.len();
        for i in possible {
            let name = &names[i];
            if [name.owner.name(), name.member]
                .into_iter()
                .all(|word| word_offset(text, word).is_some())
            {
                if parsed == NAME_PARSE_FILES {
                    return true;
                }
                parsed += 1;
                record_declarations(&mut found[i], &project_path(root, entry.path()), text, name);
            }
        }
    }
    limited
}

fn diff_locations(
    root: &Path,
    scope: Option<&ignore::overrides::Override>,
    diff: &[&str],
) -> Option<Vec<Location>> {
    let mut input = diff.iter().copied().peekable();
    let mut file = None;
    let mut locations = Vec::new();
    while let Some(line) = input.next() {
        let header = line.strip_suffix('\r').unwrap_or(line);
        if header.starts_with("diff --git ") {
            file = None;
        } else if let Some(path) = header.strip_prefix("--- a/") {
            if input
                .next()?
                .trim_end_matches('\r')
                .strip_prefix("+++ b/")?
                != path
            {
                return None;
            }
            let (relative, text) = read_path(root, scope, path)?;
            file = Some((relative, text));
        } else if let Some(hunk) = header.strip_prefix("@@ -") {
            if locations.len() == MAX_HUNKS {
                return None;
            }
            let (before, rest) = hunk.split_once(" +")?;
            let (after, _) = rest.split_once(" @@")?;
            let (first, count) = hunk_range(before)?;
            let (_, new_count) = hunk_range(after)?;
            if first == 0
                || !(2..=MAX_SOURCE_LINES).contains(&count)
                || new_count > MAX_SOURCE_LINES + 2
            {
                return None;
            }
            let last = first.checked_add(count - 1)?;
            let (relative, text) = file.as_ref()?;
            let mut original = text.split_terminator('\n').skip((first - 1) as usize);
            let (mut old_seen, mut new_seen, mut context, mut removals) = (0, 0, 0, 0);
            while old_seen < count || new_seen < new_count {
                let current = input.next()?;
                let (marker, body) = current.split_at_checked(1)?;
                match marker {
                    " " | "-" if old_seen < count && original.next() == Some(body) => {
                        old_seen += 1;
                        if marker == " " {
                            context += 1;
                            new_seen += 1;
                        } else {
                            removals += 1;
                        }
                    }
                    "+" if new_seen < new_count => new_seen += 1,
                    _ => return None,
                }
            }
            if new_seen != new_count || context == 0 || removals == 0 {
                return None;
            }
            locations.push(Location {
                path: relative.clone(),
                text: text.clone(),
                lines: first..=last,
                label: "diff before side",
            });
        } else if !header.is_empty() && !header.starts_with("index ") {
            return None;
        }
    }
    Some(locations)
}

fn hunk_range(encoded: &str) -> Option<(u32, u32)> {
    let (first, count) = encoded.split_once(',').unwrap_or((encoded, "1"));
    Some((first.parse().ok()?, count.parse().ok()?))
}
