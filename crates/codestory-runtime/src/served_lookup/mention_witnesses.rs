//! Exact source mentions expand focused lookup without implying a call binding.

use super::*;
use crate::callable_search::{CallableHit, open_generation_index};
use crate::search_publication::{
    read_search_generation_completion, search_index_path_for_publication,
};

const CANDIDATES: usize = 24;
const VERIFIED_BYTES: u64 = 6 * 1024 * 1024;
const VERIFIED_FILE_BYTES: u64 = 768 * 1024;
const WITNESSES: usize = 3;

#[derive(PartialEq, Eq)]
struct Anchor {
    member: String,
    qualifier: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn append_mentions(
    out: &mut String,
    root: &Path,
    storage_path: &Path,
    index: &mut Index<'_>,
    question: &str,
    hits: &[CallableHit],
    body_lines: Option<usize>,
    max_bytes: usize,
) {
    let budget = max_bytes.saturating_sub(out.len()).min(max_bytes / 3);
    if budget < 200 || hits.is_empty() {
        return;
    }
    let mut anchors = Vec::new();
    for hit in hits {
        let Some(node) = index.store.get_node(hit.node_id).ok().flatten() else {
            continue;
        };
        let qualified = node
            .qualified_name
            .as_deref()
            .unwrap_or(&node.serialized_name);
        if let Some(next) = anchor(&hit.name, qualified)
            && !anchors.contains(&next)
        {
            anchors.push(next);
            if anchors.len() == 2 {
                break;
            }
        }
    }
    if anchors.is_empty() {
        return;
    }
    let Ok(Some(publication)) = index.store.get_complete_index_publication() else {
        return;
    };
    let Ok(dir) = search_index_path_for_publication(storage_path, Some(&publication)) else {
        return;
    };
    if read_search_generation_completion(&dir, &publication.generation_id).is_none() {
        return;
    }
    let Ok((_, reader)) = open_generation_index(&dir, &publication) else {
        return;
    };
    let specs = anchors
        .iter()
        .map(|anchor| (anchor.member.as_str(), anchor.qualifier.as_deref()))
        .collect::<Vec<_>>();
    let excluded = hits.iter().map(|hit| hit.node_id).collect::<Vec<_>>();
    let Ok(ranked) =
        reader.rank_reference_candidates(&specs, question, &excluded, CANDIDATES, |path| {
            in_scope(index.scope, Path::new(path))
        })
    else {
        return;
    };
    let residual = remaining_terms(question, &anchors);
    let mut verified = Vec::new();
    let mut opened = HashSet::new();
    let mut total = 0;
    let mut source_limit = false;
    for (rank, candidate) in ranked.hits.into_iter().enumerate() {
        if candidate.file_bytes > VERIFIED_FILE_BYTES {
            source_limit = true;
            continue;
        }
        if opened.insert(candidate.file_id) {
            total += candidate.file_bytes;
            if total > VERIFIED_BYTES {
                source_limit = true;
                break;
            }
        }
        let Some(home) = index.open(root, candidate.file_id) else {
            continue;
        };
        let Some(body) = home
            .text
            .get(candidate.start_byte as usize..candidate.end_byte as usize)
            .filter(|body| {
                home.file.path == candidate.path
                    && home.file.sha256 == candidate.file_sha256
                    && sha256_hex(body.as_bytes()) == candidate.body_sha256
            })
        else {
            continue;
        };
        if hits.iter().any(|hit| {
            hit.file_id == candidate.file_id
                && hit.start_line <= candidate.start_line
                && hit.end_line >= candidate.end_line
        }) {
            continue;
        }
        let member = candidate.name.rsplit([':', '.']).next().unwrap_or_default();
        if anchors.iter().any(|anchor| anchor.member == member) {
            continue;
        }
        let lines = body.lines().collect::<Vec<_>>();
        let exact_start = 1 + home.text.as_bytes()[..candidate.start_byte as usize]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count() as u32;
        if exact_start != candidate.start_line
            || exact_start + lines.len().saturating_sub(1) as u32 != candidate.end_line
        {
            continue;
        }
        let Some((line, spelling, score)) = best_reference(&lines, &anchors, &residual) else {
            continue;
        };
        let spelling = spelling.to_owned();
        verified.push((Reverse(score), rank, candidate, home, line, spelling));
    }
    verified.sort_unstable_by_key(|item| (item.0, item.1));
    let mut section =
        String::from("Text references in verified source (call binding unverified):\n");
    let mut excerpts = String::new();
    let mut shown = 0;
    let mut lines_left = body_lines.unwrap_or(0);
    let mut sites = HashSet::new();
    for (_, _, candidate, home, line, spelling) in &verified {
        if shown == WITNESSES {
            break;
        }
        let path = printable(&candidate.path);
        let line_number = candidate.start_line + *line as u32;
        if !sites.insert((candidate.file_id, line_number, spelling)) {
            continue;
        }
        let location = format!(
            "  {path}:{}-{} {} mentions {} on line {line_number}.\n",
            candidate.start_line,
            candidate.end_line,
            bounded(&candidate.name),
            bounded(spelling),
        );
        if section.len() + location.len() > budget {
            break;
        }
        section.push_str(&location);
        shown += 1;
        if lines_left == 0 {
            continue;
        }
        let Some(body) = home
            .text
            .get(candidate.start_byte as usize..candidate.end_byte as usize)
        else {
            continue;
        };
        let line_count = body.lines().count();
        // A reference can sit at the start or end of the behavior it explains.
        // Keep the whole owning function when the leading witness fits.
        let count = if shown == 1 && line_count <= 64 && body.len() + line_count * 12 < budget {
            lines_left.min(line_count)
        } else {
            lines_left.min(6).min(line_count)
        };
        let first = line.saturating_sub(count / 2).min(line_count - count);
        let source = body.lines().enumerate().skip(first).take(count);
        let mut snippet = format!("  {path}:{line_number}\n");
        for (offset, text) in source {
            let _ = writeln!(
                snippet,
                "    {}: {}",
                candidate.start_line + offset as u32,
                bounded(text)
            );
            lines_left -= 1;
        }
        excerpts.push_str(&snippet);
    }
    if shown == 0 {
        return;
    }
    if ranked.truncated || ranked.has_more || source_limit || verified.len() > shown {
        let note = "  Text reference search was bounded. Additional sites may exist.\n";
        if section.len() + note.len() <= budget {
            section.push_str(note);
        }
    }
    for line in excerpts.split_inclusive('\n') {
        if section.len() + line.len() > budget {
            break;
        }
        section.push_str(line);
    }
    out.push_str(&section);
}

fn anchor(display_name: &str, qualified_name: &str) -> Option<Anchor> {
    let member = display_name.rsplit([':', '.']).next()?;
    if member.len() < 3 || !member.chars().all(identifier) {
        return None;
    }
    let compound = member.contains('_')
        || member
            .as_bytes()
            .windows(2)
            .any(|pair| pair[0].is_ascii_lowercase() && pair[1].is_ascii_uppercase());
    let specific = member.len() >= 8 && compound;
    let qualifier = if specific {
        None
    } else {
        let mut parts = qualified_name
            .rsplit([':', '.'])
            .filter(|part| !part.is_empty());
        if parts.next()? != member {
            return None;
        }
        let receiver = parts.next()?;
        if receiver.len() < 2 || !receiver.chars().all(identifier) {
            return None;
        }
        Some(receiver.to_owned())
    };
    Some(Anchor {
        member: member.to_owned(),
        qualifier,
    })
}

fn identifier(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn exact_reference<'a>(line: &'a str, anchor: &Anchor) -> Option<&'a str> {
    for (position, name) in line.match_indices(&anchor.member) {
        let before = &line[..position];
        let after = &line[position + name.len()..];
        if before.ends_with(identifier) || after.starts_with(identifier) {
            continue;
        }
        let separator = ["::", "->", "."]
            .into_iter()
            .find(|sign| before.ends_with(sign));
        if let Some(qualifier) = &anchor.qualifier {
            let Some(separator) = separator else { continue };
            let receiver_side = &before[..before.len() - separator.len()];
            let Some(before_receiver) = receiver_side.strip_suffix(qualifier) else {
                continue;
            };
            if !before_receiver.ends_with(identifier) {
                return Some(&line[before_receiver.len()..position + name.len()]);
            }
        } else if let Some(separator) = separator {
            return Some(&line[position - separator.len()..position + name.len()]);
        } else if after.trim_start().starts_with('(') {
            return Some(name);
        }
    }
    None
}

fn remaining_terms(question: &str, anchors: &[Anchor]) -> BTreeSet<String> {
    let anchor_names = anchors
        .iter()
        .flat_map(|anchor| {
            std::iter::once(anchor.member.to_lowercase())
                .chain(anchor.qualifier.as_ref().map(|word| word.to_lowercase()))
        })
        .collect::<BTreeSet<_>>();
    question
        .split(|character: char| !identifier(character))
        .filter(|term| term.len() >= 4)
        .map(str::to_lowercase)
        .filter(|term| {
            !anchor_names.contains(term)
                && !matches!(
                    term.as_str(),
                    "does"
                        | "from"
                        | "have"
                        | "this"
                        | "what"
                        | "when"
                        | "where"
                        | "which"
                        | "with"
                )
        })
        .take(24)
        .collect()
}

fn best_reference<'a>(
    lines: &[&'a str],
    anchors: &[Anchor],
    remaining: &BTreeSet<String>,
) -> Option<(usize, &'a str, usize)> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(line, text)| {
            let spelling = anchors
                .iter()
                .find_map(|anchor| exact_reference(text, anchor))?;
            let start = line.saturating_sub(1);
            let end = (line + 3).min(lines.len());
            let context = lines[start..end].join(" ").to_lowercase();
            let score = remaining
                .iter()
                .filter(|word| context.contains(word.as_str()))
                .map(String::len)
                .sum();
            Some((line, spelling, score))
        })
        .max_by_key(|(line, _, score)| (*score, Reverse(*line)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_member_requires_an_exact_qualifier_and_long_names_can_use_member_syntax() {
        let read = anchor("Read", "module.Parser.Read").unwrap();
        let retry = anchor("RetryDelay", "Scheduler.RetryDelay").unwrap();
        assert_eq!(
            exact_reference("return Parser.Read(stream)", &read),
            Some("Parser.Read")
        );
        assert_eq!(
            exact_reference("return pkg::Parser::Read(stream)", &read),
            Some("Parser::Read")
        );
        assert_eq!(exact_reference("return Other.Read(stream)", &read), None);
        assert_eq!(
            exact_reference("return WrappedParser.Read(stream)", &read),
            None
        );
        assert_eq!(
            exact_reference("return Parser.ReadBuffer(stream)", &read),
            None
        );
        assert_eq!(
            exact_reference("return controller.RetryDelay(clock)", &retry),
            Some(".RetryDelay")
        );
        assert_eq!(
            exact_reference("return RetryDelay(clock)", &retry),
            Some("RetryDelay")
        );
    }
}
