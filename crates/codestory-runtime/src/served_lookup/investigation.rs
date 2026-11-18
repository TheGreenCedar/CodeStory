use super::*;
use crate::callable_search::{self, CallableCoverage};
use std::ops::Range;

const MAX_RESULTS: usize = 20;

pub fn find_implementations(
    root: &Path,
    storage_path: &Path,
    question: &str,
    file_globs: &[String],
    limit: usize,
    body_lines: Option<usize>,
    max_bytes: usize,
) -> Result<LookupReply, ApiError> {
    if question.trim().is_empty() || question.len() > 4096 || !(1..=MAX_RESULTS).contains(&limit) {
        return Err(ApiError::invalid_argument(
            "expected a question of 1–4096 bytes and a limit of 1–20",
        ));
    }
    let scope = lookup_scope(root, file_globs)?;
    let (mut reply, live_query) = with_pinned_index(storage_path, |store| {
        let unavailable = || {
            (LookupReply {
            text: "Implementation index unavailable. Run codestory-cli index --core-only for this project to prepare it.\n".into(),
            refresh_recommended: true,
        }, Some(question.to_owned()))
        };
        let Some(store) = store else {
            return Ok(unavailable());
        };
        let result = callable_search::query(
            store,
            storage_path,
            root,
            question,
            limit,
            |path| in_scope(scope.as_ref(), Path::new(path)),
            || false,
        )?;
        let CallableCoverage::Ready {
            indexed_callables,
            partial_files,
            omitted_files,
            omitted_definitions,
        } = result.coverage
        else {
            return Ok(unavailable());
        };
        let named: Vec<_> = question
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|word| {
                word.len() >= 3
                    && (word.contains('_') || word.chars().skip(1).any(char::is_uppercase))
            })
            .collect();
        let named_missing = !named.is_empty()
            && named.iter().all(|term| {
                result
                    .absent_terms
                    .iter()
                    .any(|absent| term.eq_ignore_ascii_case(absent))
            });
        let mut out = String::new();
        if named_missing {
            let _ = writeln!(
                out,
                "The implementation index had no matches for {}. Ranked locations match other words in the question.",
                bounded(&named.join(", "))
            );
        }
        let mut excerpts = String::new();
        let mut index = Index::scoped(store, scope.as_ref());
        let mut roots = Vec::new();
        let mut first_source = None;
        let line_budget = body_lines.unwrap_or(0);
        let mut remaining = line_budget.div_ceil(3).min(80);
        let mut stale = result.stale_candidates;
        for hit in &result.hits {
            let Some(home) = index.open(root, hit.file_id) else {
                stale += 1;
                continue;
            };
            let Some(body) = home
                .text
                .get(hit.start_byte as usize..hit.end_byte as usize)
                .filter(|body| {
                    home.file.sha256 == hit.file_sha256
                        && sha256_hex(body.as_bytes()) == hit.body_sha256
                })
            else {
                stale += 1;
                continue;
            };
            let path = printable(&hit.path);
            let _ = writeln!(
                out,
                "{}. {path}:{}-{} {}",
                roots.len() + 1,
                hit.start_line,
                hit.end_line,
                bounded(&hit.name)
            );
            first_source
                .get_or_insert_with(|| format!("follow-up: read {path}:{}\n", hit.start_line));
            roots.push(hit.node_id);
            if remaining == 0 || named_missing {
                continue;
            }
            let body_budget = remaining.min(if roots.len() == 1 { 26 } else { 16 });
            let _ = writeln!(
                excerpts,
                "== {path}:{} {}",
                hit.start_line,
                bounded(&hit.name)
            );
            remaining -=
                append_witness_lines(&mut excerpts, question, body, hit.start_line, body_budget);
        }
        if roots.is_empty() {
            out.push_str("No verified implementation matched the question. Try a literal identifier or file path.\n");
        }
        if !named_missing && !result.absent_terms.is_empty() {
            let mut absent = result.absent_terms;
            absent.sort_by_key(|term| Reverse(term.len()));
            absent.truncate(7);
            let _ = writeln!(
                out,
                "Lexical candidates. Terms absent from the implementation index: {}.",
                bounded(&absent.join(", "))
            );
        }
        if partial_files != 0
            || omitted_files != 0
            || omitted_definitions != 0
            || stale != 0
            || result.query_truncated
        {
            let _ = writeln!(
                out,
                "Coverage: {indexed_callables} indexed implementations; {partial_files} partially parsed files, {omitted_files} omitted files, {omitted_definitions} omitted definitions, {stale} unverified matches; query work limit reached: {}.",
                result.query_truncated
            );
        }
        if named_missing {
            let mut reply = lookup_reply(out, max_bytes, Some(&index), &[first_source.as_deref()]);
            reply.refresh_recommended |= result.refresh_recommended || stale > 0;
            return Ok((reply, Some(named.join(" "))));
        }
        let mut related = String::new();
        append_call_witnesses(
            &mut related,
            root,
            &mut index,
            &roots,
            question,
            (line_budget / 3).min(80),
        );
        let mut mentions = String::new();
        super::mention_witnesses::append_mentions(
            &mut mentions,
            root,
            storage_path,
            &mut index,
            question,
            &result.hits,
            body_lines.map(|n| n / 3),
            max_bytes,
        );
        append_sections(&mut out, &[&excerpts, &related, &mentions], max_bytes);
        let mut reply = lookup_reply(out, max_bytes, Some(&index), &[first_source.as_deref()]);
        reply.refresh_recommended |= result.refresh_recommended || stale > 0;
        Ok((reply, roots.is_empty().then(|| question.to_owned())))
    })?;
    if let Some(terms) = live_query {
        let live = super::source_candidates::live_candidates(
            root, &terms, file_globs, limit, body_lines, max_bytes,
        )?;
        reply.text = bound_reply(format!("{}{live}", reply.text), max_bytes, &[]);
    }
    Ok(reply)
}

/// Include the signature and lines near the question's strongest literal overlap.
pub(super) fn append_witness_lines(
    out: &mut String,
    question: &str,
    body: &str,
    first: u32,
    budget: usize,
) -> usize {
    let terms: HashSet<String> = question
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() >= 3)
        .map(str::to_lowercase)
        .collect();
    let lines: Vec<_> = body.lines().collect();
    let count = budget.min(lines.len());
    if count == 0 {
        return 0;
    }
    let focus = lines
        .iter()
        .enumerate()
        .max_by_key(|(i, text)| {
            let text = text.to_lowercase();
            (
                terms
                    .iter()
                    .filter(|term| text.contains(term.as_str()))
                    .map(String::len)
                    .sum::<usize>(),
                Reverse(*i),
            )
        })
        .map_or(0, |(index, _)| index);
    let start = focus.saturating_sub(count / 2).min(lines.len() - count);
    let mut rows = BTreeSet::from([0usize]);
    rows.extend(start..start + count - usize::from(start != 0));
    for &row in &rows {
        let _ = writeln!(out, "{}: {}", first + row as u32, bounded(lines[row]));
    }
    rows.len()
}

/// Give each source of evidence space; return any spare bytes to longer sections.
fn append_sections(out: &mut String, sections: &[&str], max_bytes: usize) {
    if max_bytes == 0 || out.len() + sections.iter().map(|s| s.len()).sum::<usize>() <= max_bytes {
        for section in sections {
            out.push_str(section);
        }
        return;
    }
    let mut available = max_bytes.saturating_sub(out.len());
    let mut order: Vec<_> = (0..sections.len()).collect();
    order.sort_by_key(|&i| sections[i].len());
    let mut limits = vec![0; sections.len()];
    for (rank, slot) in order.into_iter().enumerate() {
        limits[slot] = sections[slot]
            .len()
            .min(available / (sections.len() - rank));
        available -= limits[slot];
    }
    for (&section, cap) in sections.iter().zip(limits) {
        if cap > 0 {
            out.push_str(&bound_reply(section.to_string(), cap, &[]));
        }
    }
}

/// Answer one-hop questions in the same lookup. A connection crossing files
/// is particularly valuable because it is missing from the ranked function's body.
fn append_call_witnesses(
    out: &mut String,
    root: &Path,
    index: &mut Index<'_>,
    roots: &[NodeId],
    question: &str,
    mut lines_left: usize,
) {
    let mut seen_sites = HashSet::new();
    let mut peers = Vec::new();
    let mut skipped = 0;
    let mut truncated = false;
    let terms: Vec<_> = question
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(|w| w.chars().take(5).collect::<String>().to_lowercase())
        .collect();
    for (rank, &id) in roots.iter().enumerate() {
        let Ok(neighborhood) = index.store.get_bounded_raw_incident_edges(id, 96) else {
            skipped += 1;
            continue;
        };
        truncated |= neighborhood.truncated;
        let mut links = Vec::new();
        for edge in neighborhood.edges {
            if edge.kind != EdgeKind::CALL
                || (edge.effective_source() != id && edge.effective_target() != id)
            {
                continue;
            }
            let Some((caller, callee, line)) = index.verified_call(root, &edge) else {
                skipped += 1;
                continue;
            };
            if !seen_sites.insert((caller.home.id, line, callee.definition().id)) {
                continue;
            }
            let calls_out = caller.definition().id == id;
            let peer = if calls_out { &callee } else { &caller };
            let name = peer.definition().qualified.to_lowercase();
            let relevance = terms
                .iter()
                .filter(|term| name.contains(term.as_str()))
                .count();
            let priority = (
                caller.home.id != callee.home.id,
                relevance,
                calls_out,
                Reverse(rank),
            );
            let site = format!(
                "  {}:{line} {} -> {}\n",
                printable(caller.path()),
                bounded(&caller.definition().qualified),
                bounded(&callee.definition().qualified)
            );
            links.push((priority, site, if calls_out { callee } else { caller }));
        }
        links.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        truncated |= links.len() > 3;
        for (priority, site, peer) in links.into_iter().take(3) {
            if out.is_empty() {
                out.push_str(
                    "Call sites from the pinned index, verified against both endpoint files:\n",
                );
            }
            out.push_str(&site);
            if !roots.contains(&peer.definition().id) {
                peers.push((priority, peer));
            }
        }
    }
    if skipped != 0 || truncated {
        let _ = writeln!(
            out,
            "Partial call coverage. {skipped} unverified sites; additional sites omitted: {truncated}."
        );
    }
    peers.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.path().cmp(b.1.path()))
            .then_with(|| a.1.first.cmp(&b.1.first))
    });
    let mut shown = HashSet::new();
    for (_, peer) in peers {
        if lines_left == 0 {
            break;
        }
        if !peer.note.is_empty() || !shown.insert(peer.definition().id) {
            continue;
        }
        let first = peer.first;
        let count = (peer.last - first + 1) as usize;
        let body = peer
            .home
            .text
            .lines()
            .skip(first as usize - 1)
            .take(count)
            .collect::<Vec<_>>()
            .join("\n");
        let _ = writeln!(
            out,
            "== {}:{}-{} {}",
            printable(peer.path()),
            first,
            peer.last,
            bounded(&peer.definition().qualified)
        );
        lines_left -= append_witness_lines(out, question, &body, first, lines_left.min(48));
    }
}

pub fn changed_relationships(
    root: &Path,
    storage_path: &Path,
    file_globs: &[String],
    limit: usize,
    body_lines: Option<usize>,
    max_bytes: usize,
) -> Result<LookupReply, ApiError> {
    if !(1..=MAX_RESULTS).contains(&limit) {
        return Err(ApiError::invalid_argument(
            "expected a changed-file limit of 1–20",
        ));
    }
    let scope = lookup_scope(root, file_globs)?;
    let edits = codestory_workspace::read_tracked_edits(
        root,
        |path| in_scope(scope.as_ref(), path),
        limit,
        MAX_FILE_BYTES,
    )
    .map_err(|e| ApiError::invalid_argument(format!("Could not inspect tracked changes: {e}")))?;
    Ok(with_pinned_index(storage_path, |store| {
        let mut index = store.map(|s| Index::scoped(s, scope.as_ref()));
        let mut out = format!(
            "Tracked worktree changes against HEAD tree {}\n",
            edits.head_tree
        );
        if edits.files.is_empty() {
            out.push_str("No tracked changes in scope.\n");
        }
        if edits.omitted != 0 {
            let _ = writeln!(out, "{} changed paths omitted (--limit).", edits.omitted);
        }
        let mut remaining = body_lines.unwrap_or(0);
        let mut follow = None;
        let mut changed_names = BTreeSet::new();
        for edit in edits.files {
            let path = printable(&project_path(root, &root.join(&edit.path)));
            let _ = writeln!(out, "== {path}");
            if edit.conflicted {
                out.push_str("Unresolved Git conflict; bindings unavailable.\n");
                continue;
            }
            let Ok(before) = edit.before else {
                out.push_str("HEAD contents unavailable.\n");
                continue;
            };
            let live = match codestory_workspace::resolve_project_relative_path(root, &edit.path) {
                Ok(ProjectRelativePathResolution::Existing { absolute, relative })
                    if relative == edit.path =>
                {
                    let Some((_, bytes)) = open_live(root, absolute) else {
                        out.push_str("Live source unavailable or exceeds the read limit.\n");
                        continue;
                    };
                    Some(bytes)
                }
                Ok(ProjectRelativePathResolution::Missing { .. }) => None,
                _ => {
                    out.push_str("Live path unavailable or redirected.\n");
                    continue;
                }
            };
            let old_bytes = before.as_deref().unwrap_or_default();
            let new_bytes = live.as_deref().unwrap_or_default();
            let (Ok(old), Ok(new)) = (
                std::str::from_utf8(old_bytes),
                std::str::from_utf8(new_bytes),
            ) else {
                out.push_str("Non-UTF-8 contents; source ranges unavailable.\n");
                continue;
            };
            if old.contains('\0') || new.contains('\0') {
                out.push_str("Binary contents; source ranges unavailable.\n");
                continue;
            }
            if edit.before_path != edit.path {
                let _ = writeln!(
                    out,
                    "Moved from {}",
                    printable(&edit.before_path.to_string_lossy())
                );
            }
            if live.is_none() {
                out.push_str("Removed from worktree.\n");
            }
            if before.is_none() && live.is_some() {
                out.push_str("Added since HEAD.\n");
            }
            let hunks = codestory_workspace::changed_line_ranges(old, new);
            if hunks.is_empty() {
                out.push_str("No text difference between HEAD and worktree.\n");
                continue;
            }
            let old_lines: Vec<_> = old.lines().collect();
            let new_lines: Vec<_> = new.lines().collect();
            let mut old_ranges = Vec::new();
            let mut new_ranges = Vec::new();
            for (a, b) in hunks.iter().take(32) {
                let _ = writeln!(
                    out,
                    "HEAD -{},{}  worktree +{},{}",
                    diff_line(a),
                    a.len(),
                    diff_line(b),
                    b.len()
                );
                follow.get_or_insert_with(|| {
                    format!(
                        "follow-up: read {path}:{}\n",
                        (b.start + 1).min(new_lines.len().max(1) as u32)
                    )
                });
                old_ranges.push(a.clone());
                new_ranges.push(b.clone());
                for (marker, span, lines) in [('-', a, &old_lines), ('+', b, &new_lines)] {
                    for row in span.clone().take(remaining.min(3)) {
                        if let Some(line) = lines.get(row as usize) {
                            let _ = writeln!(out, "{marker}{} {}", row + 1, bounded(line));
                            remaining -= 1;
                        }
                    }
                }
            }
            if hunks.len() > 32 {
                let _ = writeln!(out, "{} more hunks omitted.", hunks.len() - 32);
            }
            if let Some(index) = &mut index {
                let current = live
                    .as_ref()
                    .and_then(|bytes| matching_file(index.store, &root.join(&edit.path), bytes));
                let base = before.as_ref().and_then(|bytes| {
                    matching_file(index.store, &root.join(&edit.before_path), bytes)
                });
                let (file_id, ranges, source) = match (current, base) {
                    (Some(id), _) => (id, &new_ranges, "worktree"),
                    (None, Some(id)) => {
                        index.refresh_recommended = true;
                        (id, &old_ranges, "HEAD before edit")
                    }
                    (None, None) => {
                        index.refresh_recommended = true;
                        out.push_str(
                            "Neither side matches an indexed file; bindings unavailable.\n",
                        );
                        continue;
                    }
                };
                changed_names.extend(show_changed_callers(
                    &mut out, root, index, file_id, ranges, source, limit,
                ));
            } else {
                out.push_str("No complete index; bindings unavailable.\n");
            }
        }
        if !changed_names.is_empty() {
            append_test_mentions(&mut out, root, file_globs, &changed_names, limit);
        }
        lookup_reply(out, max_bytes, index.as_ref(), &[follow.as_deref()])
    }))
}

fn diff_line(range: &Range<u32>) -> u32 {
    range.start + u32::from(!range.is_empty())
}

fn matching_file(store: &Store, path: &Path, bytes: &[u8]) -> Option<i64> {
    let row = store.get_file_by_path(path).ok()??;
    (store.get_file_content_hash(row.id).ok()?? == sha256_hex(bytes)).then_some(row.id)
}

fn show_changed_callers(
    out: &mut String,
    root: &Path,
    index: &mut Index<'_>,
    file_id: i64,
    ranges: &[Range<u32>],
    source: &str,
    limit: usize,
) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(definitions) = index.store.callable_definitions_for_file(file_id) else {
        out.push_str("Indexed declarations unavailable.\n");
        return names;
    };
    let overlaps = |first: u32, last: u32, h: &Range<u32>| {
        if h.is_empty() {
            first <= h.start && h.start < last
        } else {
            first <= h.end && h.start < last
        }
    };
    let mut seen = HashSet::new();
    let mut matched = 0;
    let mut sites = 0;
    let mut unavailable = 0;
    for def in definitions {
        let last = if def.end_col == 1 && def.end_line > def.start_line {
            def.end_line - 1
        } else {
            def.end_line
        };
        if !ranges.iter().any(|h| overlaps(def.start_line, last, h)) || !seen.insert(def.node_id) {
            continue;
        }
        matched += 1;
        if matched > limit {
            out.push_str("Additional affected declarations omitted (--limit).\n");
            break;
        }
        let _ = writeln!(
            out,
            "Affected {}:{}-{} ({source}; pinned index)",
            bounded(&def.name),
            def.start_line,
            last
        );
        if let Some(&name) = name_segments(&def.name).last() {
            names.push(name.to_string());
        }
        let Ok(edges) = index.store.get_bounded_raw_incident_edges(def.node_id, 128) else {
            unavailable += 1;
            continue;
        };
        if edges.truncated {
            out.push_str("Only a bounded set of indexed edges was inspected.\n");
        }
        let mut printed = BTreeSet::new();
        for edge in edges.edges {
            if edge.kind != EdgeKind::CALL || edge.effective_target() != def.node_id {
                continue;
            }
            if index.relationship_outside_scope(root, &edge) {
                continue;
            }
            if !is_certain(&edge) || !edge.candidate_targets.is_empty() {
                unavailable += 1;
                continue;
            }
            let Some(line) = edge.line.filter(|&line| line > 0) else {
                unavailable += 1;
                continue;
            };
            let Some(caller) = index
                .locate(root, edge.effective_source())
                .filter(|caller| caller.owns_callsite(&edge, line))
            else {
                unavailable += 1;
                continue;
            };
            if !printed.insert((caller.home.id, line, caller.definition().id)) {
                continue;
            }
            if sites == limit {
                out.push_str("Additional pinned callers omitted (--limit).\n");
                break;
            }
            let snippet = caller
                .home
                .text
                .lines()
                .nth(line as usize - 1)
                .map(bounded)
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  caller {}:{line} {} | {snippet}",
                printable(caller.path()),
                bounded(&caller.definition().qualified)
            );
            sites += 1;
        }
    }
    if matched == 0 {
        out.push_str("No indexed implementation overlaps these hunks.\n");
    }
    if unavailable != 0 {
        let _ = writeln!(
            out,
            "{unavailable} caller binding(s) could not be verified."
        );
    }
    names
}

/// Dynamic test frameworks often leave no call edge. A literal in a live
/// test is a place to inspect, never a claim that the check passed or failed.
fn append_test_mentions(
    out: &mut String,
    root: &Path,
    globs: &[String],
    names: &BTreeSet<String>,
    limit: usize,
) {
    let Ok(files) = live_walk(root, globs) else {
        return;
    };
    let Ok(canonical) = root.canonicalize() else {
        return;
    };
    let mut tests = Vec::new();
    let mut file_count = 0;
    let mut bounded_scan = false;
    for path in files.take(4097) {
        file_count += 1;
        if file_count > 4096 || tests.len() == 64 {
            bounded_scan = true;
            break;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if codestory_store::FileRole::classify_path(relative) == codestory_store::FileRole::Test {
            tests.push(path);
        }
    }
    tests.sort();
    let mut bytes = Vec::new();
    let mut count = 0;
    for path in tests {
        let Some(text) =
            super::source_candidates::load_live(&canonical, &path, 128 * 1024, &mut bytes)
        else {
            bounded_scan = true;
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if names.iter().any(|name| word_offset(line, name).is_some()) {
                if count == 0 {
                    out.push_str(
                        "Test source mentioning a changed name (text matches, unexecuted):\n",
                    );
                }
                let _ = writeln!(
                    out,
                    "  {}:{} {}",
                    printable(&project_path(root, &path)),
                    i + 1,
                    bounded(line.trim())
                );
                count += 1;
                if count == limit {
                    return;
                }
                break;
            }
        }
    }
    if count == 0 {
        out.push_str("The inspected test files contained no reference to these names.\n");
    }
    if bounded_scan {
        out.push_str("Test file search was bounded.\n");
    }
}
